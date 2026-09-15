"""New startup contexts through the unchanged three-DWORD snapshot rotation."""
import json
import os
import unittest
from read_snapshot import resident_boot_observation, startup_route_diagnostic, startup_target_diagnostic
from read_snapshot import decode_frame
from test_snapshot_reader import frame


def decode(w):
    words = [0] * 16
    words[10], words[11], words[9] = w
    return resident_boot_observation(tuple(words), 0x13)


class StartupDiagnostics(unittest.TestCase):
    def test_bad_records_are_not_presented_as_evidence(self):
        for code, value in [(0, 0), (15, 0), (7 | 32 | 64, 0), (7, 1 << 63),
                            (7, 5 << 56), (2, 1 << 48), (2 | 64, 0), (7, 0)]:
            self.assertIsNone(startup_route_diagnostic(code, value))
        for code in [0, 2, 1 | 8, 5 | 8, 7, 7 | (10 << 3), 6 | (14 << 3), 4 | (84 << 3)]:
            self.assertIsNone(startup_target_diagnostic(code, 0))

    def test_self_and_missing_assignment_are_distinct(self):
        for predicate in (2, 3):
            r = decode([0x1e000083 | (predicate << 13), 0xc500, 0x1010])
            self.assertTrue(r['encoding_valid'])
            f = r['startup_route_failure']
            self.assertEqual(f['predicate'], 'self_destination' if predicate == 2 else 'destination_unassigned')
            self.assertEqual(f['source_apic_id'], 16)
            self.assertEqual(f['destination_apic_id'], 16)
            self.assertIsNone(f['recipient_apic_id'])

    def test_rust_generated_wire_vectors(self):
        path = os.environ.get('SVMVISOR_REFUSAL_VECTORS')
        if not path:
            self.skipTest('Set SVMVISOR_REFUSAL_VECTORS to the Rust-generated vector file')
        with open(path) as f:
            vectors = json.load(f)
        self.assertEqual(len(vectors), 19)
        for v in vectors:
            r = decode(v['words'])
            self.assertTrue(r['encoding_valid'], v)
            metadata, low, high = v['words']
            through_crc = decode_frame(frame(phase_detail=0x00080013, cpu_id=high,
                context=(metadata, low)))['native_resident_observation']
            self.assertEqual(through_crc, r)
            self.assertIsNone(r['guest_rip'])
            if v['name'] == 'foreign':
                f = r['startup_route_failure']
                self.assertEqual((f['source_apic_id'], f['destination_apic_id'], f['recipient_apic_id']), (8, 16, 32))
                self.assertEqual(f['recipient_mode'], 'extended_xapic_4bit')
                self.assertEqual(f['recipient_mode_cause'], 'guest_init')
                self.assertEqual(f['recipient_init_count'], v['count'])
                self.assertEqual(f['recipient_init_count_saturated'], v['count'] == 31)
                self.assertEqual(f['icr'], 0x10_0000_c500)
                self.assertEqual(r['exit_code'], 0x7c if v['x2'] else 0x400)
            elif v['name'] == 'wide':
                f = r['startup_route_failure']
                self.assertEqual(f['icr'], v['icr'])
                self.assertIsNone(f['source_apic_id'])
                self.assertTrue(f['identity_history_omitted'])
            elif v['name'] == 'target':
                f = r['startup_target_failure']
                self.assertEqual(f['observed_value'], v['value'])
                self.assertEqual(f['register_offset'], 0x100)
                self.assertEqual(f['target_apic_id'], 16 if v['value'] <= 0xffffffff else None)
                self.assertIsNone(r['exit_code'])
            else:
                self.assertIsNone(r['startup_route_failure'])
                self.assertEqual(r['exit_code'], 0x400)


if __name__ == '__main__':
    unittest.main()
