"""Recompute a retained passing audit and optionally match a packaging PE.

Usage: python tools/native-stack-audit/verify.py --evidence work/native-stack-audit-final --image path/to/driver.efi
No compilation or firmware execution occurs. Current workspace sources must
still match the build manifest, and every retained artifact is rehashed.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

from audit import Coff, PE, Refusal, audit_build, require, sha256
from run import ROOT, archive_member, effective_settings, source_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--image", type=Path)
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    try:
        result = json.loads((evidence / "result.json").read_text())
        require(result.get("status") == "pass", "no passing result")
        manifest = json.loads((evidence / "manifest.json").read_text())
        require(result["manifest_sha256"] == sha256(evidence / "manifest.json"), "manifest identity changed")
        require(manifest["sources"] == source_hashes(), "current workspace source/build inputs differ from audited checkpoint")
        artifacts = {}
        for relative, digest in manifest["artifacts"].items():
            path = (evidence / relative).resolve()
            require(path.is_relative_to(evidence), "artifact path escapes evidence directory")
            require(sha256(path) == digest, f"artifact identity changed: {relative}")
            artifacts[relative] = path
        require(effective_settings((evidence / "build.log").read_text()) == manifest["actual_compiler_settings"], "actual compiler invocation differs from manifest")
        for record in manifest["runtime_archive_members"]:
            require(record["archive"] in artifacts and record["object"] in artifacts, "runtime archive/member is not a retained artifact")
            require(archive_member(artifacts[record["archive"]], record["member"]) == artifacts[record["object"]].read_bytes(), "runtime member differs from retained compiler archive")

        def single(suffix):
            matches = [path for path in artifacts.values() if str(path).endswith(suffix)]
            require(len(matches) == 1, f"ambiguous artifact suffix {suffix}")
            return matches[0]

        image = single("svmvisor-dxe.efi")
        require(sha256(image) == result["pe_sha256"], "result/image identity differs")
        if args.image:
            require(sha256(args.image) == result["pe_sha256"], "packaging PE is not the exact audited image")
        linked = subprocess.run(["llvm-objdump", "--disassemble", "--x86-asm-syntax=intel", str(image)], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        require(linked.returncode == 0, "linked image disassembly failed")
        objects = []
        for record in result["object_binding"]:
            path = Path(record["object"])
            require(path.resolve().is_relative_to(evidence), "linked object path escapes evidence directory")
            require(sha256(path) == record["sha256"], "linked object identity changed")
            objects.append(Coff(path))
        computed = audit_build(single(".s").read_text(), single(".ll").read_text(), (evidence / "linked.map").read_text(), linked.stdout, PE(image), objects, (evidence / "linked.lldmap").read_text())
        for key, value in computed.items():
            require(result[key] == value, f"recomputed audit differs: {key}")
        print(json.dumps({"status": "pass", "pe_sha256": result["pe_sha256"], "maximum_below_sampled_rsp": result["maximum_below_sampled_rsp"], "coverage_margin_bytes": result["coverage_margin_bytes"]}, indent=2))
    except (Refusal, OSError, ValueError, KeyError) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
