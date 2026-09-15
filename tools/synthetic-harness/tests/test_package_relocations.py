"""ELF fixtures test relocation semantics, including linker-synthesized GOT."""
import importlib.util
from pathlib import Path
import struct
import unittest

spec = importlib.util.spec_from_file_location(
    "packager", Path(__file__).resolve().parents[1] / "package-relocations.py")
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)
B = packager.BASE


def fixture(relocations=None, target=B + 32, got_value=None):
    if relocations is None:
        relocations = [(0, 1, 0)]
    image = bytearray(64)
    struct.pack_into("<Q", image, 0, target)
    if got_value is not None:
        struct.pack_into("<Q", image, 24, got_value)
    for offset, kind, addend in relocations:
        if kind in (2, 4, 9):
            destination = B + 24 if kind == 9 and got_value is not None else target
            struct.pack_into("<i", image, offset, destination + addend - (B + offset))
        elif kind in (10, 11):
            struct.pack_into("<I", image, offset, target + addend)
        elif kind == 1:
            struct.pack_into("<Q", image, offset, target + addend)
    names = b"\0image_start\0image_load_end\0image_bss_end\0entry_uefi\0target\0"
    syms = bytes(24)
    for name, value in [(b"image_start", B), (b"image_load_end", B + 64),
                        (b"image_bss_end", B + 128), (b"entry_uefi", B + 16),
                        (b"target", target)]:
        syms += struct.pack("<IBBHQQ", names.index(name), 0x10, 0, 1, value, 0)
    rela = b"".join(struct.pack("<QQq", B + offset, (5 << 32) | kind, addend)
                    for offset, kind, addend in relocations)
    content_start = 64 + 5 * 64
    sym_start = content_start + len(image)
    str_start = sym_start + len(syms)
    rela_start = str_start + len(names)
    sections = bytes(64)
    sections += struct.pack("<IIQQQQIIQQ", 0, 1, 3, B, content_start, 64, 0, 0, 8, 0)
    sections += struct.pack("<IIQQQQIIQQ", 0, 2, 0, 0, sym_start, len(syms), 3, 1, 8, 24)
    sections += struct.pack("<IIQQQQIIQQ", 0, 3, 0, 0, str_start, len(names), 0, 0, 1, 0)
    sections += struct.pack("<IIQQQQIIQQ", 0, 4, 0, 0, rela_start, len(rela), 2, 1, 8, 24)
    header = struct.pack("<16sHHIQQQIHHHHHH", b"\x7fELF\x02\x01\x01" + bytes(9),
                         2, 62, 1, B + 16, 0, 64, 0, 64, 0, 0, 64, 5, 0)
    return header + sections + image + syms + names + rela, bytes(image)


def records(result):
    count = struct.unpack_from("<Q", result, 48)[0]
    size = struct.unpack_from("<Q", result, 24)[0]
    return [struct.unpack_from("<QQ", result, 64 + size + i * 16) for i in range(count)]


class PackageTests(unittest.TestCase):
    def test_absolute_widths_and_boundary_pointer(self):
        result = packager.package(*fixture([(0, 1, 0), (8, 10, 0), (12, 11, 0)], B + 128))
        self.assertEqual(records(result), [(0, 8), (8, 4), (12, 4)])
        self.assertEqual(struct.unpack_from("<7Q", result, 8), (B, B, 64, 128, 16, 3, 0))

    def test_relative_calls_stay_unchanged(self):
        result = packager.package(*fixture([(0, 1, 0), (8, 2, -4), (12, 4, -4)]))
        self.assertEqual(records(result), [(0, 8)])

    def test_synthetic_got_slot_is_relocated_and_deduplicated(self):
        result = packager.package(*fixture([(0, 1, 0), (8, 9, -4), (12, 9, -4)], got_value=B + 32))
        self.assertEqual(records(result), [(0, 8), (24, 8)])

    def test_relaxed_got_reference_needs_no_slot(self):
        result = packager.package(*fixture([(0, 1, 0), (8, 9, -4)]))
        self.assertEqual(records(result), [(0, 8)])

    def test_mismatching_and_external_got_slots_rejected(self):
        for slot in (B + 33, 0x400000):
            with self.subTest(slot=slot), self.assertRaisesRegex(ValueError, "GOTPCREL"):
                packager.package(*fixture([(0, 1, 0), (8, 9, -4)], got_value=slot))

    def test_external_absolute_symbol_rejected(self):
        with self.assertRaisesRegex(ValueError, "unowned"):
            packager.package(*fixture(target=B + 129))

    def test_addend_escaping_image_rejected(self):
        with self.assertRaisesRegex(ValueError, "escapes"):
            packager.package(*fixture([(0, 1, 129)]))

    def test_unsupported_relocation_rejected(self):
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            packager.package(*fixture([(0, 1, 0), (8, 6, 0)]))

    def test_no_retained_relocations_rejected(self):
        with self.assertRaisesRegex(ValueError, "emit-relocs"):
            packager.package(*fixture([]))

    def test_flat_image_substitution_rejected(self):
        elf, image = fixture()
        with self.assertRaisesRegex(ValueError, "differs"):
            packager.package(elf, image[:-1] + b"X")


if __name__ == "__main__":
    unittest.main()
