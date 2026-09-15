"""Compile and inspect native continuation assembly. Never executes SVM.

This is an assembly seam audit, not a resident payload/dispatcher closure audit.
Use a new output directory; failures and their exact commands remain retained.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCES = ROOT / "crates/dxe/src/native"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(args, output, name):
    executable = shutil.which(str(args[0]))
    if executable is None: raise RuntimeError(f"missing tool: {args[0]}")
    result = subprocess.run([executable, *[str(x) for x in args[1:]]], cwd=ROOT, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (output / (name + ".log")).write_text(result.stdout, encoding="utf-8")
    if result.returncode:
        raise RuntimeError(f"{name}: exit {result.returncode}: {result.stdout[-3000:]}")
    return result.stdout


def symbols(path, output, name):
    text = run(["llvm-nm", "--numeric-sort", "--defined-only", path], output, name)
    return {m[3]: int(m[1], 16) for line in text.splitlines()
            if (m := re.fullmatch(r"([0-9a-fA-F]+)\s+(\w)\s+(\S+)", line.strip()))}


def audit(output: Path, baseline: Path):
    output.mkdir(parents=True, exist_ok=False)
    source_paths = [SOURCES / "resident/bridge.S", SOURCES / "resident/runtime.S",
                    SOURCES / "admission/boundary.S", baseline, Path(__file__)]
    manifest = {str(path): sha(path) for path in source_paths}
    (output / "sources.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    for name, source in [("callback", source_paths[0]), ("boundary", source_paths[2]),
                         ("baseline", baseline)]:
        run(["clang", "--target=x86_64-pc-windows-msvc", "-c", source,
             "-o", output / (name + ".obj")], output, name + "-compile")
        # llvm-objcopy cannot dump COFF sections on all installed releases;
        # COFF section bytes are read directly below.
    import struct
    def text_section(path):
        data = path.read_bytes()
        machine, sections = struct.unpack_from("<HH", data)
        assert machine == 0x8664, "wrong COFF architecture"
        optional = struct.unpack_from("<H", data, 16)[0]
        for index in range(sections):
            offset = 20 + optional + index * 40
            if data[offset:offset+8].rstrip(b"\0") == b".text":
                size, pointer = struct.unpack_from("<II", data, offset + 16)
                assert pointer + size <= len(data)
                return data[pointer:pointer+size]
        raise AssertionError("missing text")
    assert text_section(output / "boundary.obj") == text_section(output / "baseline.obj"), \
        "ungated original capture changed"
    syms = symbols(output / "callback.obj", output, "callback-symbols")
    resume, ack, after = [syms["svmvisor_resident_guest_" + x]
                          for x in ("resume", "ack", "after_ack")]
    callback = text_section(output / "callback.obj")
    assert callback[resume:after] == bytes.fromhex("b8 41 4d 56 53 0f 01 d9")
    assert ack == resume + 5 and after == ack + 3
    assert callback[:2] == bytes.fromhex("9c fa"), "PUSHFQ must precede CLI"
    return_end = syms["svmvisor_resident_callback_return_end"]
    assert callback[return_end-3:return_end] == bytes.fromhex("58 9d c3"), "restore RAX/flags then RET"
    assert return_end - resume <= 256, "guest restoration exceeds admitted epilogue span"
    run(["clang", "--target=x86_64-unknown-none", "-c", SOURCES / "resident/runtime.S",
         "-o", output / "runtime.o"], output, "runtime-compile")
    run(["ld.lld", "-m", "elf_x86_64", "--image-base=0x100000", "-e", "svmvisor_resident_enter", "-Ttext=0x101000",
         output / "runtime.o", "-o", output / "runtime.elf"], output, "runtime-link")
    undefined = run(["llvm-nm", "--undefined-only", output / "runtime.elf"], output, "runtime-undefined")
    assert not undefined.strip(), "runtime directly references external code"
    disassembly = run(["llvm-objdump", "-d", "--no-show-raw-insn", output / "runtime.elf"],
                      output, "runtime-disassembly")
    instructions = [m[1] for line in disassembly.splitlines()
                    if (m := re.fullmatch(r"\s*[0-9a-f]+:\s+(.+)", line))]
    assert instructions
    # Conservative allowlist: additions require review. In particular, no FP,
    # vector, XCR0, debug-register, STGI or direct firmware call is accepted.
    allowed = {"cli", "clgi", "movq", "movl", "movw", "movabsq", "subq", "addq",
               "lgdtq", "lidtq", "pushq", "leaq", "lretq", "xorq", "xorl", "lldtw",
               "ltrw", "incl", "shrq", "rdmsr", "orl", "orq", "wrmsr", "vmsave", "vmload", "vmrun", "cld",
               "rep", "callq", "testb", "jne", "jmp", "hlt", "nop", "nopl", "nopw",
               "int3"}
    for instruction in instructions:
        mnemonic = instruction.split()[0]
        assert mnemonic in allowed, f"unreviewed runtime instruction: {instruction}"
        assert not re.search(r"%(?:[xyz]mm|mm|st|dr)\d", instruction), instruction
    calls = [x for x in instructions if x.startswith("call")]
    assert calls == ["callq\t*0x60(%rcx)"], f"dispatch seam changed: {calls}"
    runtime_symbols = symbols(output / "runtime.elf", output, "runtime-symbols")
    result = {
        "scope": "compiled assembly seam only; no VM entry or firmware execution",
        "original_boundary_text_unchanged": True,
        "callback_ack_bytes": callback[resume:after].hex(),
        "callback_offsets": {"resume": resume, "ack": ack, "after_ack": after},
        "runtime_instruction_count": len(instructions),
        "runtime_bytes": runtime_symbols["svmvisor_resident_runtime_end"] - runtime_symbols["svmvisor_resident_enter"],
        "runtime_direct_external_symbols": 0,
        "outside_this_audit": ["linked dispatch and fault-handler closure audit", "runtime allocation and lifecycle integration",
                    "actual native guest callback return", "ExitBootServices continuation", "Windows boot"],
        "artifacts": {p.name: sha(p) for p in output.iterdir() if p.is_file()},
    }
    assert all(sha(path) == digest for path, digest in ((Path(p), h) for p, h in manifest.items()))
    (output / "summary.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline-boundary", type=Path, required=True)
    args = parser.parse_args()
    audit(args.output.resolve(), args.baseline_boundary.resolve())
