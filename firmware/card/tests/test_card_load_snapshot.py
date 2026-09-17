import importlib.util
from pathlib import Path
import struct
import unittest
import zlib

spec = importlib.util.spec_from_file_location("card_snapshot_check", Path(__file__).resolve().parents[1] / "check-card-load-snapshot.py")
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)
MANIFEST = {"card_load_only": True, "image_kind": "card_load_only_review", "fpga_build_id": "4d65aa8a78f3ccdb", "rom_build_id": "1e163de89051834a"}


def frame(phase=0x40, detail=0x2004, status=0x1ad, context=0x0000000100010001, writes=54):
    words = [0x50414e53, 0x02000001, 0x78f3ccdb, 0x4d65aa8a, 0x9051834a, 0x1e163de8, 77, 6, phase | detail << 16, 0, context & 0xffffffff, context >> 32, status, 1000, writes]
    data = struct.pack("<15I", *words)
    raw = data + struct.pack("<I", zlib.crc32(data))
    return raw[::-1].hex()


def paired(**kwargs):
    value = frame(**kwargs)
    return f"SNAPSHOT:{value}\nSNAPSHOT:{value}\n"


class CardSnapshotTests(unittest.TestCase):
    def test_happy_observation_never_grants_qualification(self):
        result = check.assess(paired(), MANIFEST, "normal", first_boot_after_reconfiguration=True)
        self.assertEqual(result["observation_result"], "PASS")
        self.assertFalse(result["grants_flash_authorization"])
        self.assertFalse(result["updates_manifest_qualification"])
        self.assertEqual(result["payload_load_evidence"], "loaded_and_freed")

    def test_freshness_is_explicit_and_old_success_is_never_accepted(self):
        self.assertEqual(check.assess(paired(), MANIFEST, "normal")["observation_result"], "REVIEW_REQUIRED")
        self.assertEqual(check.assess(paired(), MANIFEST, "normal", previous_boot_id=77)["observation_result"], "REVIEW_REQUIRED")
        self.assertEqual(check.assess(paired(), MANIFEST, "normal", previous_boot_id=76)["observation_result"], "PASS")
        with self.assertRaises(ValueError):
            check.assess(paired(), MANIFEST, "normal", first_boot_after_reconfiguration=True, previous_boot_id=76)
        with self.assertRaises(ValueError):
            check.assess(paired(), MANIFEST, "normal", previous_boot_id=-1)

    def test_windows_report_is_independent_and_required(self):
        for report in ("not-reported", "failed"):
            result = check.assess(paired(), MANIFEST, report, first_boot_after_reconfiguration=True)
            self.assertEqual(result["observation_result"], "REVIEW_REQUIRED")
            self.assertEqual(result["payload_load_evidence"], "loaded_and_freed")

    def test_each_failure_or_incomplete_stage_is_retained(self):
        for kwargs in ({"phase": 0x10, "detail": 5, "context": 0x44414f4c44524143}, {"phase": 0x1f, "detail": 5, "context": 0x44414f4c44524143}, {"detail": 4}, {"detail": 0x2404}, {"detail": 0x6004}, {"context": 0x0000000100010002}, {"status": 0x21ad}, {"status": 0x3ad}, {"status": 0xad}, {"status": 0x1ac}, {"writes": 0}):
            with self.subTest(kwargs=kwargs):
                self.assertEqual(check.assess(paired(**kwargs), MANIFEST, "normal", first_boot_after_reconfiguration=True)["observation_result"], "REVIEW_REQUIRED")

    def test_crc_pair_and_manifest_identity_are_mandatory(self):
        for text in (frame(), "SNAPSHOT:" + "0" * 128 + "\n" * 2, frame() + "\n" + frame(detail=4)):
            with self.assertRaises(ValueError):
                check.assess(text, MANIFEST, "normal")
        with self.assertRaises(ValueError):
            check.assess(paired(), {**MANIFEST, "rom_build_id": "0000000000000000"}, "normal")
        with self.assertRaises(ValueError):
            check.assess(paired(), {**MANIFEST, "card_load_only": False}, "normal")


if __name__ == "__main__":
    unittest.main()
