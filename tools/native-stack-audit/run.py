"""Build, bind and audit one fresh optimized native-returning image. No execution.

Usage: python tools/native-stack-audit/run.py --output work/native-stack-audit-final
The destination must not exist, so a stale pass can never survive a failed rerun.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tomllib
from pathlib import Path

from audit import Coff, PE, Refusal, audit_build, require, sha256


ROOT = Path(__file__).resolve().parents[2]
# Explicit source paths; linked object names and audit checks remain unchanged.
ASSEMBLY_SOURCES = {
    "memory_attribute_probe": "crates/dxe/src/memory_attributes/probe.S",
    "native_boundary": "crates/dxe/src/native/admission/boundary.S",
    "native_snapshot": "crates/dxe/src/native/admission/snapshot.S",
    "native_cache": "crates/dxe/src/native/admission/cache.S",
    "native_transition": "crates/dxe/src/native/transition/run.S",
    "native_transition_canary": "crates/dxe/src/native/transition/canary.S",
    "native_transition_canary_negative": "crates/dxe/src/fixtures/canary_negative.S",
}


# Exact transitive closure of the physical native-returning build. The F7 path
# supplies only a local Get reader; mutation, publication, and guarded-fault
# experimental features are deliberately outside this reviewed build profile.
NATIVE_FEATURES = frozenset({
    "native-returning", "native-resource-observe", "native-preflight",
    "memory-attribute-f7", "memory-attribute-provider",
})


def source_hashes():
    paths = [ROOT / x for x in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml")]
    for folder in (ROOT / "crates", ROOT / "tools/native-stack-audit"):
        paths.extend(path for path in folder.rglob("*") if path.is_file() and path.suffix in {".rs", ".S", ".toml", ".py"} and "target" not in path.parts)
    return {str(path.relative_to(ROOT)).replace("\\", "/"): sha256(path) for path in sorted(set(paths))}


def command(args, *, log=None):
    result = subprocess.run([str(x) for x in args], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    if log:
        Path(log).write_text(result.stdout, encoding="utf-8")
    require(result.returncode == 0, f"command failed ({result.returncode}): {args}\n{result.stdout[-7000:]}")
    return result.stdout


def one(paths, what):
    matches = list(paths)
    require(len(matches) == 1, f"expected exactly one {what}, got {matches}")
    return matches[0]


def registry_hashes(metadata):
    dependencies = {}
    for package in metadata["packages"]:
        if package.get("source"):
            package_root = Path(package["manifest_path"]).parent
            dependencies[package["id"]] = {str(path.relative_to(package_root)).replace("\\", "/"): sha256(path) for path in sorted(package_root.rglob("*")) if path.is_file() and path.suffix in {".rs", ".toml", ".S", ".c", ".h"}}
    return dependencies


def archive_member(archive, member):
    extracted = subprocess.run(["llvm-ar", "p", str(archive), member], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    require(extracted.returncode == 0 and extracted.stdout, f"cannot extract linked runtime object {member}: {extracted.stderr.decode(errors='replace')}")
    return extracted.stdout


def linked_runtime_objects(output, map_text):
    """Retain actual compiler-runtime members named by this link, never a stub.

    A compiler-builtins memcmp and its private helper occur in the native HIGH
    graph. Their full object instructions/relocations undergo the same PE byte
    verification and CFG walk as the workspace's Rust and assembly objects.
    """
    members = sorted(set(re.findall(r'\s(lib[^:\s]+):([^\s]+\.o)', map_text)))
    require(all(re.fullmatch(r"libcompiler_builtins-[0-9a-f]+", archive) and re.fullmatch(r"[A-Za-z0-9_.-]+", member) for archive, member in members), "unreviewed linked runtime archive/member")
    if not members:
        return [], [], []
    libdir = Path(command(["rustc", "--print", "target-libdir", "--target", "x86_64-unknown-uefi"]).strip())
    destination = output / "runtime-objects"
    destination.mkdir()
    objects, artifacts, provenance = [], [], []
    archives = {}
    for library, member in members:
        if library not in archives:
            source = libdir / (library + ".rlib")
            retained = destination / source.name
            shutil.copyfile(source, retained)
            require(sha256(source) == sha256(retained), "compiler runtime archive changed during retention")
            archives[library] = retained
            artifacts.append(retained)
        archive = archives[library]
        obj = destination / member
        obj.write_bytes(archive_member(archive, member))
        objects.append(Coff(obj))
        artifacts.append(obj)
        provenance.append({"archive": str(archive.relative_to(output)).replace("\\", "/"), "member": member, "object": str(obj.relative_to(output)).replace("\\", "/")})
    return objects, artifacts, provenance


def effective_settings(build_log):
    lines = [line for line in build_log.splitlines() if "Running `" in line and "--crate-name svmvisor_dxe " in line and "--crate-type bin " in line]
    require(len(lines) == 1, "missing/ambiguous actual rustc bin invocation")
    invocation = lines[0].replace('\\"', '"')
    options = {}
    for quoted, plain in re.findall(r'-C\s+(?:"([^"]+)"|(\S+))', invocation):
        option = (quoted or plain).rstrip("`")
        key, separator, value = option.partition("=")
        options[key] = value if separator else True
    require(options.get("opt-level") == "z" and options.get("lto") in {True, "fat"} and options.get("codegen-units") == "1" and options.get("panic") == "abort", "actual rustc profile differs from required opt-z/fat-LTO/one-CGU/abort")
    features = set(re.findall(r'--cfg\s+"?feature="([^\"]+)"', invocation))
    require(features == NATIVE_FEATURES, f"actual rustc features differ: {sorted(features)}")
    require("--target x86_64-unknown-uefi " in invocation, "actual rustc target changed")
    return {"invocation": lines[0].strip(), "opt-level": "z", "lto": "fat", "codegen-units": 1, "panic": "abort", "features": sorted(features)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    require(not output.exists(), "output already exists; choose a fresh evidence directory")
    require(output.is_relative_to(ROOT / "work") or output.is_relative_to(ROOT / "target"), "evidence must be inside this worktree's work/ or target/")
    output.mkdir(parents=True)
    try:
        profile = tomllib.loads((ROOT / "Cargo.toml").read_text())["profile"]
        require(profile["dxe"]["opt-level"] == "z" and profile["dxe"]["lto"] is True and profile["dxe"]["codegen-units"] == 1 and profile["release"]["panic"] == "abort", "required DXE profile changed")
        for name in os.environ:
            require(name not in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTFLAGS"} and not name.startswith("CARGO_PROFILE_") and not name.startswith("CARGO_TARGET_X86_64_UNKNOWN_UEFI_"), f"unreviewed build environment override {name}")
        before = source_hashes()
        metadata = json.loads(command(["cargo", "metadata", "--locked", "--format-version", "1"]))
        dependencies = registry_hashes(metadata)
        versions = {name: command([name, "-vV"] if name == "rustc" else [name, "--version"]) for name in ("rustc", "cargo", "clang", "llvm-objdump", "llvm-ar")}
        # A compiler change requires an explicit review of the parser and this
        # pin. It cannot silently inherit a previous numerical stack result.
        require("rustc 1.97.1 (8bab26f4f 2026-07-14)" in versions["rustc"] and "LLVM version: 22.1.6" in versions["rustc"], "unreviewed Rust/LLVM toolchain")
        build = ["cargo", "rustc", "-v", "--locked", "-p", "svmvisor-dxe", "--profile", "dxe", "--no-default-features", "--features", "native-returning", "--bin", "svmvisor-dxe", "--target", "x86_64-unknown-uefi", "--target-dir", output / "target", "--", "--emit=llvm-ir,asm,obj,link", "-C", f"link-arg=/map:{output / 'linked.map'}", "-C", f"link-arg=/lldmap:{output / 'linked.lldmap'}"]
        actual = effective_settings(command(build, log=output / "build.log"))
        require(source_hashes() == before, "source changed during compilation; rerun from a stable checkpoint")
        target = output / "target/x86_64-unknown-uefi/dxe"
        asm = one((target / "deps").glob("svmvisor_dxe-*.s"), "optimized compiler assembly")
        ir = one((target / "deps").glob("svmvisor_dxe-*.ll"), "optimized LLVM IR")
        obj = one((target / "deps").glob("svmvisor_dxe-*.o"), "optimized Rust object")
        image = target / "svmvisor-dxe.efi"
        replay = output / "rust-assembly-replay.obj"
        command(["clang", "--target=x86_64-pc-windows-msvc", "-c", asm, "-o", replay], log=output / "assembly-replay.log")
        rust_obj = Coff(obj)
        assembly_matches = rust_obj.executable_signature() == Coff(replay).executable_signature()
        # Multi-output rustc can run separate backend emissions: its sibling .s
        # is NOT assumed to be the machine code used for .o/link. Retain the
        # diagnostic comparison, and use actual object/PE disassembly for proof.
        command(["llvm-objdump", "--disassemble", "--reloc", "--x86-asm-syntax=intel", obj], log=output / "rust-object.disassembly.txt")
        objects = [rust_obj]
        assembly_artifacts = []
        native_sources = ["native_boundary", "native_snapshot", "native_cache", "native_transition_canary", "native_transition"]
        for name in native_sources:
            linked = one((target / "build").glob(f"svmvisor-dxe-*/out/{name}.obj"), name + " linked assembly object")
            rebuilt = output / (name + "-replay.obj")
            defines = ["-DSVMVISOR_NATIVE_RETURNING=1"] if name == "native_transition_canary" else []
            command(["clang", *defines, "--target=x86_64-pc-windows-msvc", "-c", ROOT / ASSEMBLY_SOURCES[name], "-o", rebuilt])
            require(Coff(linked).executable_signature() == Coff(rebuilt).executable_signature(), f"linked {name} differs from actual assembly source replay")
            objects.append(Coff(linked))
            assembly_artifacts.extend([linked, rebuilt])
        runtime_objects, runtime_artifacts, runtime_provenance = linked_runtime_objects(output, (output / "linked.map").read_text())
        objects.extend(runtime_objects)
        disasm = command(["llvm-objdump", "--disassemble", "--x86-asm-syntax=intel", image], log=output / "linked.disassembly.txt")
        report = audit_build(asm.read_text(), ir.read_text(), (output / "linked.map").read_text(), disasm, PE(image), objects, (output / "linked.lldmap").read_text())
        report["sibling_compiler_assembly_replay_matches_linked_object"] = assembly_matches
        report["stack_instruction_authority"] = "Actual linked PE instructions, byte-bound to emitted Rust and assembly COFF objects; sibling .s is not used for numerical bounds."
        require(source_hashes() == before, "source changed during audit; no stable final-build evidence")
        require(registry_hashes(metadata) == dependencies, "registry source changed during compilation/audit")
        artifacts = [image, asm, ir, obj, replay, *assembly_artifacts, *runtime_artifacts, output / "linked.map", output / "linked.lldmap", output / "linked.disassembly.txt", output / "rust-object.disassembly.txt", output / "build.log"]
        manifest = {"schema": 1, "root": str(ROOT), "command": [str(x) for x in build], "actual_compiler_settings": actual, "toolchain": versions, "sources": before, "registry_sources": dependencies, "artifacts": {str(path.relative_to(output)).replace("\\", "/"): sha256(path) for path in artifacts}, "features": sorted(NATIVE_FEATURES), "profile": profile["dxe"], "no_reachable_panic_guard": "unchanged undefined svmvisor_dxe_must_not_panic; link succeeded and optimized IR references absent"}
        manifest["runtime_archive_members"] = runtime_provenance
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        report["manifest_sha256"] = sha256(output / "manifest.json")
        report["pe_sha256"] = sha256(image)
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({key: report[key] for key in ("status", "maximum_below_sampled_rsp", "coverage_margin_bytes", "pe_sha256")}, indent=2))
        print(f"Evidence: {output}")
    except (Refusal, OSError, ValueError, KeyError, struct.error) as error:
        (output / "result.json").write_text(json.dumps({"status": "refused", "reason": str(error)}, indent=2) + "\n")
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
