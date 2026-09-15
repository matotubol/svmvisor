"""Bind anonymous read-only relocations in real COFF/PE; never execute fixtures."""
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest

from audit import Coff, PE, Refusal, contributions_from_lld_map, symbols_from_map, verify_objects


ROOT = Path(__file__).resolve().parents[2]
SOURCE = """
.text
.def efi_main;
.scl 2;
.type 32;
.endef
.globl efi_main
efi_main:
 lea .Ltable(%rip), %rax
 ret
.Lcase:
 ret
.section .rdata,"dr"
.p2align 2
.Ltable:
 .long .Lcase-.Ltable
 .long efi_main-.Ltable
 .long .Lcase-.Ltable
 .long efi_main-.Ltable
"""


class ReadonlyBindingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        for tool in ("clang", "lld-link"):
            if shutil.which(tool) is None:
                raise unittest.SkipTest(f"requires {tool}")
        cls.temporary = tempfile.TemporaryDirectory(prefix="readonly-binding-", dir=ROOT / "work")
        cls.directory = Path(cls.temporary.name)

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def fixture(self, writable=False):
        source, obj, image, link_map, lld_map = [self.directory / f"fixture.{ext}" for ext in ("s", "obj", "efi", "map", "lldmap")]
        source.write_text(SOURCE.replace('.rdata,"dr"', '.data,"dw"') if writable else SOURCE)
        for command in (["clang", "--target=x86_64-pc-windows-msvc", "-c", source, "-o", obj], ["lld-link", "/entry:efi_main", "/subsystem:efi_boot_service_driver", "/nodefaultlib", f"/out:{image}", f"/map:{link_map}", f"/lldmap:{lld_map}", obj]):
            result = subprocess.run([str(item) for item in command], cwd=ROOT, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return PE(image), Coff(obj), symbols_from_map(link_map.read_text()), lld_map.read_text()

    def test_anonymous_table_all_relocations_are_bound(self):
        pe, obj, symbols, lld = self.fixture()
        ranges, reports = verify_objects(pe, [obj], symbols, lld)
        binding = reports[0]["anonymous_readonly_bindings"]
        self.assertEqual(len(binding), 1)
        self.assertEqual(binding[0]["bytes"], 16)
        self.assertEqual(len(obj.sections[binding[0]["section"] - 1]["relocations"]), 4)
        self.assertFalse(any(start <= int(binding[0]["address"], 16) < end for start, end in ranges))

    def test_anonymous_data_without_lld_map_refuses(self):
        pe, obj, symbols, _ = self.fixture()
        with self.assertRaisesRegex(Refusal, "requires LLD"):
            verify_objects(pe, [obj], symbols)

    def test_changed_table_byte_refuses(self):
        pe, obj, symbols, lld = self.fixture()
        readonly = next(row for row in contributions_from_lld_map(lld, pe.base) if row[4] == ".rdata" and row[1])
        address = readonly[0]
        start, _, ptr, _ = next(section for section in pe.sections if section[0] <= address < section[0] + section[1])
        data = bytearray(pe.data)
        data[ptr + address - start] ^= 1
        pe.data = bytes(data)
        with self.assertRaisesRegex(Refusal, "missing, changed or ambiguous"):
            verify_objects(pe, [obj], symbols, lld)

    def test_wrong_contribution_owner_refuses(self):
        pe, obj, symbols, lld = self.fixture()
        lld = "\n".join(line.replace("fixture.obj", "another.obj") if ":(.rdata)" in line else line for line in lld.splitlines())
        with self.assertRaisesRegex(Refusal, "missing, changed or ambiguous"):
            verify_objects(pe, [obj], symbols, lld)

    def test_missing_or_wrong_contribution_extent_refuses(self):
        for missing in (True, False):
            pe, obj, symbols, lld = self.fixture()
            lld = "\n".join(("" if missing else line.replace("00000010", "0000000c")) if ":(.rdata)" in line else line for line in lld.splitlines())
            with self.subTest(missing=missing), self.assertRaisesRegex(Refusal, "missing, changed or ambiguous"):
                verify_objects(pe, [obj], symbols, lld)

    def test_anonymous_writable_data_is_not_admitted(self):
        pe, obj, symbols, lld = self.fixture(writable=True)
        with self.assertRaisesRegex(Refusal, "not read-only"):
            verify_objects(pe, [obj], symbols, lld)

    def test_second_byte_valid_contribution_is_ambiguous(self):
        pe, obj, symbols, lld = self.fixture()
        address, size, alignment, owner, name = next(row for row in contributions_from_lld_map(lld, pe.base) if row[4] == ".rdata" and row[1])
        # Create a second relocated copy: each REL32 target stays the same
        # while the new table is 64 KiB higher. Both candidates byte-match;
        # the verifier must reject the missing unique identity.
        delta = 65536
        duplicate = bytearray(pe.bytes(address, size))
        for offset in range(0, size, 4):
            struct.pack_into("<i", duplicate, offset, struct.unpack_from("<i", duplicate, offset)[0] - delta)
        pe.sections.append((address + delta, size, len(pe.data), 0x40000040))
        pe.data += duplicate
        lld += f"\n{address + delta - pe.base:08x} {size:08x} {alignment} {owner}:({name})\n"
        with self.assertRaisesRegex(Refusal, "missing, changed or ambiguous"):
            verify_objects(pe, [obj], symbols, lld)

    def test_unknown_or_out_of_range_data_relocation_refuses(self):
        for malformed_type in (True, False):
            pe, obj, symbols, lld = self.fixture()
            section = next(section for section in obj.sections if section["name"] == ".rdata")
            offset, target, typ = section["relocations"][0]
            section["relocations"][0] = (offset, target, 0xffff) if malformed_type else (len(section["raw"]), target, typ)
            with self.subTest(malformed_type=malformed_type), self.assertRaisesRegex(Refusal, "unknown executable COFF relocation|COFF relocation outside section"):
                verify_objects(pe, [obj], symbols, lld)

    def test_writable_or_unreadable_pe_backing_refuses(self):
        for flags in (0xc0000040, 0x00000040):
            pe, obj, symbols, lld = self.fixture()
            address = next(row[0] for row in contributions_from_lld_map(lld, pe.base) if row[4] == ".rdata" and row[1])
            pe.sections = [(start, size, ptr, flags if start <= address < start + size else previous) for start, size, ptr, previous in pe.sections]
            with self.subTest(flags=flags), self.assertRaisesRegex(Refusal, "PE backing is not read-only"):
                verify_objects(pe, [obj], symbols, lld)

    def test_overlapping_pe_backing_refuses(self):
        pe, obj, symbols, lld = self.fixture()
        address = next(row[0] for row in contributions_from_lld_map(lld, pe.base) if row[4] == ".rdata" and row[1])
        pe.sections.append(next(section for section in pe.sections if section[0] <= address < section[0] + section[1]))
        with self.assertRaisesRegex(Refusal, "ambiguous/missing PE backing"):
            verify_objects(pe, [obj], symbols, lld)

    def test_uefi_merged_executable_readonly_output_does_not_admit_data_as_code(self):
        pe, obj, symbols, lld = self.fixture()
        address = next(row[0] for row in contributions_from_lld_map(lld, pe.base) if row[4] == ".rdata" and row[1])
        pe.sections = [(start, size, ptr, flags | 0x20000000) for start, size, ptr, flags in pe.sections]
        ranges, _ = verify_objects(pe, [obj], symbols, lld)
        self.assertFalse(any(start <= address < end for start, end in ranges))


if __name__ == "__main__":
    unittest.main()
