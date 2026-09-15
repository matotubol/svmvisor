"""Explicit actual-firmware resident card integration; never programs hardware.

Uses the unchanged parent/production resident child from one audited card package,
real OVMF image services and two CPUs. BAR0 is the existing externally acknowledged
RAM journal model, qualified UC by the fixture. This does not prove physical PCI,
Windows boot, Secure Boot, Hyper-V/VBS coexistence, or native timing performance.
The historical returning run.ps1 command and backend pin remain separate.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
QEMU_SHA = "677158d2f10933bfc8770e3741a3c6ebf33466d1f7f71fee87e6aec3e009b240"
FIRMWARE_SHA = "33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a"
VARS_SHA = "5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command(args, output, log, env=None, cwd=ROOT):
    executable = shutil.which(str(args[0]))
    if not executable:
        raise RuntimeError("missing tool: " + str(args[0]))
    with (output / (log + ".log")).open("w", encoding="utf-8") as stream:
        result = subprocess.run([executable, *map(str, args[1:])], cwd=cwd, env=env,
                                stdout=stream, stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(f"{log} failed ({result.returncode}); see retained log")


def copy_checked(source, dest, expected):
    if sha(source) != expected:
        raise RuntimeError("input hash mismatch: " + str(source))
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, dest)
    if sha(dest) != expected:
        raise RuntimeError("input copy changed: " + str(source))


def run(args):
    output = args.output.resolve()
    delivery = args.delivery.resolve()
    output.mkdir(parents=True, exist_ok=False)
    record = dict(schema=1, status="building", emulatorOnly=True, hardwareAccessed=False,
                  x2apicDisabled=args.x2apic_off,
                  mode=args.mode, processors=2, journalDetail=8,
                  parentFeature="card-resident-loader", childFeature="native-resident-boot",
                  testOutputChild=False, windowsTested=False, secureBootPolicyTested=False,
                  hyperVVBSSupported=False, timingBaselineMeasured=False,
                  journalModel="RAM-backed UC page with external GDB commit acknowledgment")
    began = time.monotonic()
    try:
        manifest = json.loads((delivery / "manifest.json").read_text(encoding="utf-8-sig"))
        if manifest.get("status") != "built_review_required" or manifest.get("payload_kind") != "NativeResidentBoot":
            raise RuntimeError("complete audited NativeResidentBoot delivery required")
        copy_checked(delivery / "manifest.json", output / "delivery-manifest.json", sha(delivery / "manifest.json"))
        for name, key in [("svmvisor-dxe.efi", "loader_sha256"), ("reviewed-child.efi", "payload_sha256"),
                          ("payload/payload-slot.bin", "slot_sha256"), ("payload/pe-header.bin", "pin_sha256"),
                          ("child-build-evidence.bin", "payload_evidence_sha256")]:
            copy_checked(delivery / name, output / name, manifest[key])
        for item in manifest["source_hashes"]:
            copy_checked(delivery / "reviewed-source" / item["path"], output / "parent-source" / item["path"], item["sha256"])
        evidence = Path(manifest["resident_build_path"])
        command([sys.executable, ROOT / "firmware/squirrel/verify-resident-build.py", "--evidence", evidence,
                 "--image", output / "reviewed-child.efi"], output, "verify-production-child")
        if sha(evidence / "summary.json") != manifest["resident_summary_sha256"]:
            raise RuntimeError("resident summary differs from package")
        sources = json.loads((evidence / "source-manifest.json").read_text(encoding="utf-8-sig"))
        copy_checked(evidence / "source-manifest.json", output / "child-source-manifest.json", manifest["resident_source_manifest_sha256"])
        for name, expected in sources.items():
            copy_checked(evidence / "source" / name, output / "child-source" / name, expected)
        fixture = ROOT / "tools/native-returning-card-test"
        inputs = {str(p.relative_to(fixture)).replace("\\", "/"): sha(p) for p in fixture.rglob("*")
                  if p.is_file() and not {"target", "__pycache__"}.intersection(p.relative_to(fixture).parts)}
        for name, expected in inputs.items():
            copy_checked(fixture / name, output / "fixture-source" / name, expected)
        record["fixtureSourceHashes"] = inputs
        qemu = ROOT / "work/qemu-init-sx/build-attempt-03/runtime/bin/qemu-system-x86_64.exe"
        firmware = qemu.parent / "share/edk2-x86_64-code.fd"
        template = qemu.parent / "share/edk2-i386-vars.fd"
        for path, expected in [(qemu, QEMU_SHA), (firmware, FIRMWARE_SHA), (template, VARS_SHA)]:
            if sha(path) != expected:
                raise RuntimeError("backend or firmware pin mismatch: " + str(path))
        record["backendHashes"] = {str(path): sha(path) for path in [qemu, firmware, template]}
        tools = {}
        for name in ["cargo", "rustc", "clang", "llvm-objdump"]:
            path = Path(subprocess.check_output(["rustup", "which", name], text=True).strip()) if name in ("cargo", "rustc") else Path(shutil.which(name))
            tools[str(path)] = sha(path)
        tools[sys.executable] = sha(Path(sys.executable))
        record["toolHashes"] = tools
        env = os.environ.copy()
        env.update(SVMVISOR_RETURNING_PARENT=str(output / "svmvisor-dxe.efi"),
                   SVMVISOR_RETURNING_SLOT=str(output / "payload/payload-slot.bin"),
                   SVMVISOR_JOURNAL_DETAIL="8", SVMVISOR_RETURNING_MODE=args.mode, SVMVISOR_TERMINAL_EBS="0")
        features = "resident" + {"Positive": "", "Header": ",header-negative", "Digest": ",digest-negative", "Admission": ",admission-negative"}[args.mode]
        target = ROOT / "target/native-resident-card-fixture-cargo"
        command(["cargo", "build", "--offline", "--locked", "--manifest-path", output / "fixture-source/Cargo.toml",
                 "--release", "--target", "x86_64-unknown-uefi", "--target-dir", target,
                 "--features", features], output, "launcher-build", env, output / "fixture-source")
        launcher = target / "x86_64-unknown-uefi/release/svmvisor-native-returning-card-test.efi"
        copy_checked(launcher, output / "launcher.efi", sha(launcher))
        copy_checked(launcher, output / "esp/EFI/BOOT/BOOTX64.EFI", sha(launcher))
        command([ROOT / "target/synthetic-tools/qemu-10.1.0/qemu-img.exe", "convert", "-f", "vvfat", "-O", "raw",
                 "fat:" + str(output / "esp"), output / "esp.img"], output, "esp-convert")
        copy_checked(template, output / "vars.fd", VARS_SHA)
        record["status"] = "running"
        command([sys.executable, "-B", output / "fixture-source/journal_model.py", "--resident", "--qemu", qemu,
                 "--firmware", firmware, "--vars", output / "vars.fd", "--disk", output / "esp.img",
                 "--session", output, "--processors", "2", "--journal-detail", "8", "--mode", args.mode,
                 *(["--x2apic-off"] if args.x2apic_off else [])],
                output, "model-run")
        trace = (output / "debug.log").read_text(errors="replace")
        common = ["firmware-journal-uc-qualified", "external-journal-model-attached", "real-parent-load-start-owner",
                  "reentrant-supported-start-stop-refused", "parent-start-abi-canaries", "observed-host-state-unchanged",
                  "exact-parent-result-journal"]
        required = common + (["real-loaded-image-retained", "resident-image-options-journal-runtime-retained",
                    "resident-stop-refused-decode-retained", "resident-parent-lifecycle-journal-exclusion",
                    "resident-genuine-stale-key-ebs-refused", "resident-stage5-two-cpu-ack", "resident-loader-cpuid-efer-continuation",
                    "resident-production-cpuid-remains-native"]
                    if args.mode == "Positive" else ["real-loaded-image-count-restored", "permanent-one-attempt-latch-after-stop",
                    "stop-cleanup-decode-events", "real-pci-ownership-released", "fixture-protocols-removed", "post-card-boot-services"])
        if args.mode=="Admission": required += ["injected-admission-mp-device-error", "resident-admission-firmware-refusal-retained"]
        for marker in required:
            if trace.count("PASS " + marker + "\n") != 1:
                raise RuntimeError("missing or duplicate witness: " + marker)
        if "FAIL " in trace:
            raise RuntimeError("guest failure marker")
        model = json.loads((output / "journal-model.json").read_text())
        if model["status"] != "passed" or model["exitCode"] != 33:
            raise RuntimeError("external journal model did not pass")
        words = [commit["words"] for commit in model["commits"]]
        stages = [word[4] & 0xffff for word in words if word[7] == 0x00080013]
        if stages != ([1, 2, 3, 4, 5] if args.mode == "Positive" else []):
            raise RuntimeError("incorrect child boot stage sequence: " + str(stages))
        parents = [word for word in words if word[7] == (0x00080014 if args.mode=="Admission" else 0x00080010)]
        expected_parent = [0x704, 0, 0] if args.mode == "Positive" else [0x400 if args.mode == "Header" else 0x401, 33, 0x80000000]
        if args.mode=="Admission": expected_parent=[0x00022013,0x80000003,3]
        if len(parents) != 1 or parents[0][4:7] != expected_parent:
            raise RuntimeError("incorrect exact parent delivery record")
        if args.mode != "Positive" and len(words) != 3:
            raise RuntimeError("negative delivery generated unexpected journal records")
        record["requiredWitnesses"] = required
        record["childBootStages"] = stages
        record["commits"] = len(words)
        for name, expected in inputs.items():
            if sha(fixture / name) != expected or sha(output / "fixture-source" / name) != expected:
                raise RuntimeError("fixture source mutated: " + name)
        for name, expected in tools.items():
            if sha(Path(name)) != expected:
                raise RuntimeError("tool mutated: " + name)
        record["status"] = "passed"
        print("PASS resident card " + args.mode + ": " + str(output))
    except Exception as error:
        record["status"] = "failed"
        record["error"] = str(error)
        raise
    finally:
        record["wallSeconds"] = time.monotonic() - began
        record["artifactHashes"] = {str(p.relative_to(output)).replace("\\", "/"): sha(p)
            for p in output.rglob("*") if p.is_file() and p.name != "result.json"
            and not any(part in ("fixture-source", "parent-source", "child-source", "esp") for part in p.relative_to(output).parts)}
        (output / "result.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--delivery", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--mode", choices=("Positive", "Header", "Digest", "Admission"), default="Positive")
    parser.add_argument("--x2apic-off", action="store_true")
    run(parser.parse_args())
