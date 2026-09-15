import unittest
from read_snapshot import decode_frame, apic_takeover_diagnostic
from test_snapshot_reader import frame

class ApicTakeoverDiagnostics(unittest.TestCase):
    def test_bsp_and_ap_keep_exact_failed_register(self):
        for reason, offset in [(1, 0x480), (1, 0x4f0), (2, 0x500), (3, 0x410), (4, 0x410)]:
            for wide in [False, True]:
                code = (0xa1 << 56) | (int(wide) << 55) | (reason << 48) | (offset << 32) | 0xfedcba98
                for metadata in [0x04000084 | (23 << 21), 0x04008382 | (3 << 16) | (23 << 21)]:
                    r = decode_frame(frame(phase_detail=0x00080013, cpu_id=code >> 32,
                        context=(metadata, code & 0xffffffff)))['native_resident_observation']
                    self.assertTrue(r['encoding_valid'])
                    self.assertEqual(r['callback_return_code'], code)
                    f = r['apic_takeover_failure']
                    self.assertEqual(f['register_offset'], offset)
                    self.assertEqual(f['observed_low32'], 0xfedcba98)
                    self.assertEqual(f['observed_value'], None if wide else 0xfedcba98)
                    self.assertFalse(r['all_cpus_activated'])

    def test_bad_takeover_codes_are_not_named(self):
        for code in [0, 0xa100041000000000, 0xa101041000000000,
                     0xa102048000000000, 0xa103040000000000, 0xa105041000000000]:
            self.assertIsNone(apic_takeover_diagnostic(code))
        code = 0xa103041000000000
        for metadata in [0x04000184, 0x04010084, 0x08000084]:
            r = decode_frame(frame(phase_detail=0x00080013, cpu_id=code >> 32,
                context=(metadata, 0)))['native_resident_observation']
            self.assertFalse(r['encoding_valid'])

if __name__ == '__main__':
    unittest.main()
