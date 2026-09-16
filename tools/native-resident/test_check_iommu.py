"""Focused hostile-format and admission checks; no hardware access."""
import struct
import unittest

from check_iommu import IvrsError, decode_ivrs


def table(efr=(1 << 7) | (1 << 21) | (1 << 2), devices=b"\x01\0\0\0"):
    header = bytearray(48)
    header[:4] = b"IVRS"
    header[8] = 2
    struct.pack_into("<I", header, 36, 1)
    block = struct.pack("<BBHHHQHHIQQ", 0x11, 0x30, 40 + len(devices), 2,
                        0x40, 0xF7600000, 0, 0, 0, efr, 0) + devices
    return checksum(header + block)


def checksum(data):
    data = bytearray(data)
    struct.pack_into("<I", data, 4, len(data))
    data[9] = 0
    data[9] = -sum(data) & 0xFF
    return data


class IvrsTests(unittest.TestCase):
    def test_capability_pass_is_not_configuration_proof(self):
        result = decode_ivrs(table())
        self.assertTrue(result["x2apic_direct_avic_capabilities_advertised"])
        self.assertFalse(result["live_configuration_verified"])

    def test_each_required_capability_is_independent(self):
        for efr in (0, 1 << 7, (1 << 7) | (1 << 21),
                    (1 << 7) | (2 << 21) | (1 << 2)):
            with self.subTest(efr=efr):
                self.assertFalse(decode_ivrs(table(efr))["x2apic_direct_avic_capabilities_advertised"])

    def test_current_machine_efr_refuses_xt(self):
        result = decode_ivrs(table(0x246577EFA2254AFA))
        self.assertEqual(len(result["iommu_checks"][0]["failures"]), 1)
        self.assertIn("XTSup", result["iommu_checks"][0]["failures"][0])

    def test_checksum_and_length_refused(self):
        data = table()
        data[20] ^= 1
        with self.assertRaises(IvrsError):
            decode_ivrs(data)
        with self.assertRaises(IvrsError):
            decode_ivrs(table()[:-1])

    def test_zero_or_truncated_block_cannot_loop_or_escape(self):
        for length in (0, 3, 0xFFFF):
            data = table()
            struct.pack_into("<H", data, 50, length)
            with self.subTest(length=length), self.assertRaises(IvrsError):
                decode_ivrs(checksum(data))

    def test_complete_alias_range_retains_actual_requester(self):
        entries = bytes.fromhex("4300ff0000a5000004ffff00")
        devices = decode_ivrs(table(devices=entries))["blocks"][0]["devices"]
        self.assertEqual(devices[0]["source_device_id"], 0xA5)
        self.assertEqual(devices[1]["range_start"], 0xFF00)

    def test_bad_range_and_unknown_entry_refused(self):
        for entries in (bytes.fromhex("03010000"), bytes.fromhex("04010000"),
                        bytes.fromhex("0302000004010000"), bytes.fromhex("5000000000000000")):
            with self.subTest(entries=entries), self.assertRaises(IvrsError):
                decode_ivrs(table(devices=entries))

    def test_duplicate_extended_description_must_agree(self):
        data = table()
        block = bytearray(data[48:])
        block[0] = 0x40
        block[24] ^= 4
        with self.assertRaises(IvrsError):
            decode_ivrs(checksum(data + block))

    def test_truncated_hid_entry_refused(self):
        data = table(devices=b"\xf0" + bytes(20) + b"\x08")
        data[48] = 0x40
        with self.assertRaises(IvrsError):
            decode_ivrs(checksum(data))


if __name__ == "__main__":
    unittest.main()
