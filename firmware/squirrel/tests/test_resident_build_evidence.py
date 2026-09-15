import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("resident_verify", Path(__file__).resolve().parents[1] / "verify-resident-build.py")
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class ResidentEvidenceTests(unittest.TestCase):
    def fixture(self, root):
        (root / "source").mkdir()
        (root / "source/input.rs").write_bytes(b"source checkpoint")
        source = {"input.rs": verify.sha(root / "source/input.rs")}
        files = {"driver.efi": b"reviewed image", "payload.elf": b"ELF", "payload.reloc": b"reloc",
                 "source-manifest.json": json.dumps(source).encode(), "disassembly.log": b"audited",
                 "undefined.log": b"", "boot-audit-disassembly.log": b"boot",
                 "physical-audit-relocations.log": b"AP"}
        for name, data in files.items(): (root / name).write_bytes(data)
        summary = dict(boot=True, test_output=False, payload_only=False,
                       no_fp_simd_xstate_instructions=True, undefined_symbols=0,
                       linked_instruction_count=10,
                       bootstrap_audit=dict(copied_section_relocations=0, copied_wait_bytes=95),
                       artifacts={name: verify.sha(root / name) for name in files})
        (root / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
        return summary

    def test_audit_binds_image_artifacts_and_retained_sources(self):
        for member in ("driver.efi", "payload.elf", "source/input.rs"):
            with self.subTest(member=member), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); self.fixture(root)
                result = verify.verify(root, root / "driver.efi", current=False)
                self.assertFalse(result["physical_execution_proven"])
                (root / member).write_bytes(b"changed")
                with self.assertRaises(ValueError): verify.verify(root, root / "driver.efi", current=False)

    def test_diagnostic_and_incomplete_audits_are_refused(self):
        for field, value in (("boot", False), ("test_output", True), ("undefined_symbols", 1)):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); summary = self.fixture(root)
                summary[field] = value
                (root / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
                with self.assertRaises(ValueError): verify.verify(root, root / "driver.efi", current=False)

    def test_traversal_manifest_member_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); summary = self.fixture(root)
            summary["artifacts"]["../outside"] = "0" * 64
            (root / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
            with self.assertRaises(ValueError): verify.verify(root, root / "driver.efi", current=False)


if __name__ == "__main__": unittest.main()
