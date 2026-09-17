import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("returning_package", Path(__file__).resolve().parents[1] / "package-payload.py")
card = importlib.util.module_from_spec(spec)
spec.loader.exec_module(card)


def pe():
    data = bytearray(1536)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3c, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<HH", data, 68, 0x8664, 2)
    struct.pack_into("<HH", data, 84, 240, 2)
    struct.pack_into("<H", data, 88, 0x20b)
    for offset, value in ((104,4096),(120,4096),(124,512),(144,12288),(148,512),(196,16),(240,8192),(244,12)):
        struct.pack_into("<I",data,offset,value)
    struct.pack_into("<H", data, 156, 11)
    struct.pack_into("<8s8I",data,328,b".text",1,4096,512,512,0,0,0,0x60000020)
    struct.pack_into("<8s8I",data,368,b".reloc",12,8192,512,1024,0,0,0,0x42000040)
    data[512] = 0xc3
    struct.pack_into("<IIHH", data,1024,4096,12,0,0)
    return bytes(data)


class ReturningPackageTests(unittest.TestCase):
    def test_exact_slot_header_and_metadata(self):
        data=pe(); header=card.build_header(data); slot=card.build_slot(data)
        self.assertEqual(header[:8],b"SVMPE001")
        self.assertEqual(len(header),128)
        self.assertEqual(header[48:80].hex(),card.digest(data))
        self.assertEqual(card.validate_slot(slot,header),data)
        self.assertEqual(card.validate_pe(data)["entry_rva"],4096)

    def test_resident_profile_is_separate_and_digest_bound(self):
        data = bytearray(pe()); struct.pack_into("<H", data, 156, 12); data = bytes(data)
        with self.assertRaises(ValueError): card.build_slot(data)
        with self.assertRaises(ValueError): card.build_slot(pe(), resident=True)
        header = card.build_header(data, resident=True)
        slot = card.build_slot(data, resident=True)
        self.assertEqual(header[:8], b"SVMBPE01")
        self.assertEqual(struct.unpack_from("<Q", header, 40)[0], 4)
        self.assertEqual(struct.unpack_from("<H", header, 82)[0], 12)
        self.assertEqual(card.validate_slot(slot, header, resident=True), data)
        with self.assertRaises(ValueError): card.validate_slot(slot, header)
        corrupt = bytearray(slot); corrupt[128 + 512] ^= 1
        with self.assertRaises(ValueError): card.validate_slot(corrupt, header, resident=True)
        combined = card.combine(b"configuration", slot, header, resident=True)
        self.assertEqual(len(combined), 0x500000)
        with tempfile.TemporaryDirectory() as temp:
            result = card.write_artifacts(data, Path(temp)/"resident", resident=True)
            self.assertEqual(result["required_parent_feature"], "card-resident-loader")
            self.assertFalse(result["physical_run_ready"])

    def test_old_flat_package_rejected(self):
        with self.assertRaises(ValueError): card.build_slot(b"SVMRELO1"+bytes(1528))

    def test_all_pinned_header_fields_and_payload_authenticate(self):
        slot=card.build_slot(pe()); header=slot[:128]
        for offset in (0,8,12,16,24,32,40,48,80,82,84,86,88,92,96,100,104,108,112,127,128+512):
            corrupt=bytearray(slot); corrupt[offset]^=1
            with self.subTest(offset=offset), self.assertRaises(ValueError): card.validate_slot(corrupt,header)

    def test_pinning_does_not_admit_wrong_pe_type_or_entry(self):
        for fmt,offset,value in (("H",68,0x14c),("H",156,10),("I",104,8192),("I",104,0),("I",340,0),("I",364,0xe0000020),("I",196,15),("Q",208,1),("Q",272,1),("I",240,0)):
            data=bytearray(pe()); struct.pack_into("<"+fmt,data,offset,value)
            with self.subTest(offset=offset,value=value), self.assertRaises(ValueError): card.build_slot(data)

    def test_truncation_and_section_overflow(self):
        for size in (0,63,127,511,1000):
            with self.subTest(size=size), self.assertRaises(ValueError): card.build_slot(pe()[:size])
        for offset,value in ((0x3c,0xfffffff0),(340,0xfffff000),(344,0xfffffe00),(348,0xfffffe00),(388,512)):
            data=bytearray(pe()); struct.pack_into("<I",data,offset,value)
            with self.subTest(offset=offset), self.assertRaises(ValueError): card.build_slot(data)

    def test_padding_and_oversized_slot(self):
        header=card.build_header(pe()); slot=card.build_slot(pe())
        for data in (slot[:-1],slot+b"\xff",slot[:-1]+b"\0"):
            with self.assertRaises(ValueError): card.validate_slot(data,header)

    def test_combined_exact_configuration_and_partition(self):
        configuration=bytes(range(256))*100
        slot=card.build_slot(pe()); header=slot[:128]
        combined=card.combine(configuration,slot,header)
        self.assertEqual(len(combined),0x500000)
        self.assertEqual(combined[:len(configuration)],configuration)
        self.assertEqual(combined[len(configuration):0x400000],b"\xff"*(0x400000-len(configuration)))
        self.assertEqual(combined[0x400000:],slot)
        for invalid in (b"",bytes(0x400001)):
            with self.assertRaises(ValueError): card.combine(invalid,slot,header)

    def test_new_output_only_and_reproducibility(self):
        with tempfile.TemporaryDirectory() as temp:
            a,b=Path(temp)/"a",Path(temp)/"b"
            result=card.write_artifacts(pe(),a,b"configuration")
            card.write_artifacts(pe(),b,b"configuration")
            self.assertEqual({p.name:p.read_bytes() for p in a.iterdir()},{p.name:p.read_bytes() for p in b.iterdir()})
            with self.assertRaises(FileExistsError): card.write_artifacts(pe(),a)
            self.assertFalse(result["physical_run_ready"])
            self.assertEqual(result["combined_bytes"],0x500000)


if __name__ == "__main__": unittest.main()
