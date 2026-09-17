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

    def test_captured_register_refusal_names_the_msr(self):
        # host::resident::captured_register_refusal (arm code 11).
        for msr, name in [(0x808, 'tpr'), (0x80f, 'svr'), (0x830, 'icr'), (0x832, 'lvt_timer'),
                          (0x837, 'lvt_error'), (0x838, 'timer_initial_count'), (0x83e, 'timer_divide')]:
            for value in [0x1_2345, 0x1_0000_00ef]:
                code = (0xa1 << 56) | (int(value >> 32 != 0) << 55) | (5 << 48) | (msr << 32) | (value & 0xffffffff)
                for metadata in [0x04000084 | (23 << 21), 0x04008382 | (3 << 16) | (23 << 21)]:
                    r = decode_frame(frame(phase_detail=0x00080013, cpu_id=code >> 32,
                        context=(metadata, code & 0xffffffff)))['native_resident_observation']
                    self.assertTrue(r['encoding_valid'])
                    f = r['apic_takeover_failure']
                    self.assertEqual((f['reason'], f['msr'], f['register_name'], f['register_offset']),
                                     ('captured_x2apic_register_unsupported', msr, name, None))
                    self.assertEqual(f['observed_low32'], value & 0xffffffff)
                    self.assertEqual(f['observed_value'], None if value >> 32 else value)
        for msr in (0x800, 0x80b, 0x831, 0x839, 0x83f, 0x410):
            self.assertIsNone(apic_takeover_diagnostic((0xa1 << 56) | (5 << 48) | (msr << 32)))

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
