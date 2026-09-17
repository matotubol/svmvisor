"""Consume the existing production native-resident build audit; never build or execute."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def current_sources(root=ROOT):
    """The current complete build source manifest, as `cargo xtask sources`
    prints it (same {path: sha256} JSON the build records as
    source-manifest.json). Run from the repository root."""
    result = subprocess.run(["cargo", "xtask", "sources"], cwd=root,
                            capture_output=True, text=True)
    if result.returncode != 0:
        raise ValueError("cargo xtask sources failed: " + (result.stderr or result.stdout).strip())
    return json.loads(result.stdout)


def member(root, name):
    path = (root / name).resolve()
    if path == root or not path.is_relative_to(root) or not path.is_file():
        raise ValueError("invalid artifact member: " + name)
    return path


def verify(evidence, image, current=True):
    evidence = evidence.resolve()
    summary = json.loads((evidence / "summary.json").read_text(encoding="utf-8-sig"))
    if (summary.get("boot") is not True or summary.get("test_output") is not False
            or summary.get("payload_only") is not False
            or summary.get("no_fp_simd_xstate_instructions") is not True
            or summary.get("undefined_symbols") != 0
            or not isinstance(summary.get("linked_instruction_count"), int)
            or summary["linked_instruction_count"] <= 0):
        raise ValueError("requires complete production boot build and linked audit")
    bootstrap = summary.get("bootstrap_audit") or {}
    if bootstrap.get("copied_section_relocations") != 0 or not 0 < bootstrap.get("copied_wait_bytes", 0) <= 3840:
        raise ValueError("missing copied AP code audit")
    artifacts = summary.get("artifacts", {})
    required = {"driver.efi", "payload.elf", "payload.reloc", "source-manifest.json",
                "disassembly.log", "undefined.log", "boot-audit-disassembly.log",
                "physical-audit-relocations.log"}
    if not required <= artifacts.keys():
        raise ValueError("incomplete artifact manifest")
    for name, expected in artifacts.items():
        if sha(member(evidence, name)) != expected:
            raise ValueError("build artifact changed: " + name)
    if sha(image) != artifacts["driver.efi"]:
        raise ValueError("supplied PE differs from the audited production image")
    if (evidence / "undefined.log").read_text().strip():
        raise ValueError("payload has undefined symbols")
    source = json.loads((evidence / "source-manifest.json").read_text(encoding="utf-8-sig"))
    for name, expected in source.items():
        if sha(member(evidence / "source", name)) != expected:
            raise ValueError("retained build source changed: " + name)
    if current:
        if current_sources() != source:
            raise ValueError("current complete native build source differs from audit checkpoint")
    return {"status": "verified", "image_sha256": artifacts["driver.efi"],
            "summary_sha256": sha(evidence / "summary.json"),
            "source_manifest_sha256": sha(evidence / "source-manifest.json"),
            "physical_execution_proven": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--image", required=True, type=Path)
    # `cargo xtask sources` needs cargo on PATH; the caller passes this when it
    # runs under a deliberately minimal PATH and has already locked the current
    # sources against the audited manifest itself.
    parser.add_argument("--no-current-source-check", dest="current", action="store_false")
    args = parser.parse_args()
    print(json.dumps(verify(args.evidence, args.image, args.current), indent=2))
