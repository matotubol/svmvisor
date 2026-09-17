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
        for code in [0, 2, 1 | 8, 5 | 8, 7, 7 | (16 << 3), 6 | (14 << 3), 4 | (84 << 3)]:
            self.assertIsNone(startup_target_diagnostic(code, 0))
        # Guest INIT LAPIC stages need a well-formed failure operand.
        for stage in (10, 11):
            for value in [0, 1, 0x1000_0000, 0x1000_000c, 0x3000_0001, 0x2000_0000,
                          0x2018_0000, 0x2003_0240, 0x2003_0000 | (1 << 21)]:
                self.assertIsNone(startup_target_diagnostic(7 | (stage << 3), value), (stage, hex(value)))
            self.assertIsNone(startup_target_diagnostic(7 | (stage << 3) | 1024, 1 << 40))

    def test_service_stages_and_guest_init_failures_are_named(self):
        names = {1: 'init_acknowledgment', 4: 'mode_commit_preparation', 6: 'efer_init_reset',
                 7: 'icr_init_reset_retired', 12: 'guest_init_cpu_commit', 13: 'cache_replay',
                 14: 'guest_init_refused_retired', 15: 'startup_owner_missing'}
        for stage, name in names.items():
            f = startup_target_diagnostic(7 | (stage << 3), 1 | (16 << 32))
            self.assertEqual((f['service_stage'], f['observed_value'], f['target_apic_id']), (name, 1, 16))
            self.assertIsNone(f['guest_init_failure'])
        # terminal.rs init_error_code: IRQ bridge, unexpected physical ISR 70h.
        f = startup_target_diagnostic(7 | (10 << 3), 0x200f_0070 | (16 << 32))
        self.assertEqual(f['service_stage'], 'guest_init_lapic_preparation')
        self.assertEqual(f['guest_init_failure'], {'owner': 'host_irq_bridge',
            'error': 'unexpected_physical_isr', 'vector': 0x70, 'other_vector': None})
        # Physical ISR mismatch keeps both vectors; drain incomplete has none.
        f = startup_target_diagnostic(7 | (11 << 3), 0x2004_5043)
        self.assertEqual(f['guest_init_failure'], {'owner': 'host_irq_bridge',
            'error': 'physical_isr_mismatch', 'vector': 0x43, 'other_vector': 0x50})
        f = startup_target_diagnostic(7 | (11 << 3), 0x2015_0000)
        self.assertEqual(f['guest_init_failure']['error'], 'drain_incomplete')
        self.assertIsNone(f['guest_init_failure']['vector'])
        f = startup_target_diagnostic(7 | (10 << 3), 0x1000_0009)
        self.assertEqual(f['guest_init_failure'], {'owner': 'backing_page', 'error': 'unsupported_version'})

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
        # native_refusal_wire.rs: four target records. The xAPIC-era vectors
        # and the startup-route record (kind 14, still decoded for older
        # images) are retired.
        self.assertEqual(len(vectors), 4)
        self.assertEqual({v['name'] for v in vectors}, {'target'})
        for v in vectors:
            r = decode(v['words'])
            self.assertTrue(r['encoding_valid'], v)
            metadata, low, high = v['words']
            through_crc = decode_frame(frame(phase_detail=0x00080013, cpu_id=high,
                context=(metadata, low)))['native_resident_observation']
            self.assertEqual(through_crc, r)
            self.assertIsNone(r['guest_rip'])
            if v['name'] == 'target':
                f = r['startup_target_failure']
                self.assertEqual(f['observed_value'], v['value'])
                self.assertEqual(f['register_offset'], 0x100)
                self.assertEqual(f['target_apic_id'], 16 if v['value'] <= 0xffffffff else None)
                self.assertIsNone(r['exit_code'])
            else:
                self.fail(v)


if __name__ == '__main__':
    unittest.main()
