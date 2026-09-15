import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("card_package", Path(__file__).resolve().parents[1] / "package-card-payload.py")
card = importlib.util.module_from_spec(spec)
spec.loader.exec_module(card)


def package():
    image = struct.pack("<Q", 0x100010) + bytes(56)
    return struct.pack("<8s7Q", b"SVMRELO1", 0x100000, 0x100000, 64, 128, 8, 1, 0) + image + struct.pack("<QQ", 0, 8)


class CardPackageTests(unittest.TestCase):
    def test_slot_exact_contract_and_roundtrip(self):
        payload = package()
        slot = card.build_slot(payload)
        self.assertEqual(len(slot), 0x100000)
        self.assertEqual(slot[:8], b"SVMCRD01")
        self.assertEqual(slot[48:80].hex(), card.digest(payload))
        self.assertEqual(card.validate_slot(slot, card.digest(payload)), payload)

    def test_digest_is_external_binding_not_only_self_report(self):
        slot = card.build_slot(package())
        with self.assertRaisesRegex(ValueError, "digest"):
            card.validate_slot(slot, "00" * 32)

    def test_corrupt_package_and_header_digest_rejected(self):
        for index in (48, 128 + 64):
            slot = bytearray(card.build_slot(package()))
            slot[index] ^= 1
            with self.subTest(index=index), self.assertRaisesRegex(ValueError, "digest"):
                card.validate_slot(slot, card.digest(package()))

    def test_truncated_and_oversized_slot_rejected(self):
        slot = card.build_slot(package())
        for corrupt in (slot[:-1], slot + b"\xff", slot[:127]):
            with self.subTest(size=len(corrupt)), self.assertRaises(ValueError):
                card.validate_slot(corrupt, card.digest(package()))

    def test_reserved_flags_and_offset_rejected(self):
        for index in (0, 8, 12, 24, 32, 40, 80, 127):
            slot = bytearray(card.build_slot(package()))
            slot[index] ^= 1
            with self.subTest(index=index), self.assertRaisesRegex(ValueError, "envelope"):
                card.validate_slot(slot, card.digest(package()))

    def test_slot_capacity_overflow_rejected(self):
        slot = bytearray(card.build_slot(package()))
        struct.pack_into("<Q", slot, 16, 0xFFFFFFFFFFFFFFFF)
        with self.assertRaisesRegex(ValueError, "exceeds"):
            card.validate_slot(slot, card.digest(package()))

    def test_non_erased_tail_rejected(self):
        slot = bytearray(card.build_slot(package()))
        slot[-1] = 0
        with self.assertRaisesRegex(ValueError, "unused"):
            card.validate_slot(slot, card.digest(package()))

    def test_package_truncation_and_trailing_data_rejected(self):
        for corrupt in (package()[:63], package()[:-1], package() + b"\0"):
            with self.subTest(size=len(corrupt)), self.assertRaises(ValueError):
                card.build_slot(corrupt)

    def test_package_ownership_and_relocations_rejected(self):
        for offset, value in ((32, 0x100000), (40, 64), (48, 0xFFFFFFFFFFFFFFFF),
                              (128, 64), (136, 3), (64, 0x400000)):
            corrupt = bytearray(package())
            struct.pack_into("<Q", corrupt, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                card.build_slot(corrupt)

    def test_configuration_boundary_exact_preservation(self):
        configuration = bytes(range(256)) * (0x400000 // 256)
        slot = card.build_slot(package())
        combined = card.combine(configuration, slot, card.digest(package()))
        self.assertEqual(combined[:0x400000], configuration)
        self.assertEqual(combined[0x400000:], slot)
        self.assertEqual(len(combined), 0x500000)
        self.assertLess(len(combined), 0x1000000)

    def test_short_configuration_padding_and_invalid_extents(self):
        slot = card.build_slot(package())
        combined = card.combine(b"configuration", slot, card.digest(package()))
        self.assertEqual(combined[13:0x400000], b"\xff" * (0x400000 - 13))
        for configuration in (b"", bytes(0x400001)):
            with self.subTest(size=len(configuration)), self.assertRaisesRegex(ValueError, "4 MiB"):
                card.combine(configuration, slot, card.digest(package()))

    def test_artifacts_are_reproducible_and_review_only(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp)
            manifest = card.write_artifacts(package(), output, b"configuration")
            first = {p.name: p.read_bytes() for p in output.iterdir()}
            card.write_artifacts(package(), output, b"configuration")
            self.assertEqual(first, {p.name: p.read_bytes() for p in output.iterdir()})
            self.assertFalse(manifest["physical_run_ready"])
            self.assertFalse(manifest["execution_allowed"])
            self.assertFalse(manifest["flash_operation_authorized"])
            self.assertEqual(manifest["combined_sha256"], card.digest(first["combined-review.bin"]))


if __name__ == "__main__":
    unittest.main()
