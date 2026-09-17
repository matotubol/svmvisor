import json
from pathlib import Path
import struct
import unittest
import zlib
import contextlib
import io
import tempfile
from unittest import mock
import read_snapshot

from read_snapshot import (cache_rendezvous_diagnostic, consistent_snapshot, decode_frame,
                           f7_fallback_diagnostic)


class CacheRendezvousDiagnostics(unittest.TestCase):
    def test_cpu_field_and_stage_survive_persistent_wire(self):
        for family, stage in ((0x400B, "before_transition"), (0x4017, "after_transition")):
            for processor in (0, 1, 127, 128, 255):
                for detail, category, reason in (
                    (0x12, "capture", "unsupported_cpu"),
                    (0x21, "rendezvous", "incomplete"),
                    (0x22, "rendezvous", "stale"),
                    (0x34, "paging_root", "inconsistent_cr4"),
                    (0x51, "configuration_mismatch", "cr3"),
                    (0x52, "configuration_mismatch", "cr4"),
                    (0x56, "configuration_mismatch", "pat"),
                    (0x5F, "configuration_mismatch", "variable_mtrrs"),
                ):
                    refusal = (processor << 24) | (detail << 16) | family
                    for phase in (0x10, 0x28, 0x35, 0x40):
                        observed = decode_frame(frame(phase_detail=0x40070000 | phase,
                            cpu_id=0x010843C1, context=(refusal, 0)))["native_returning_observation"]
                        self.assertEqual(observed["cache_rendezvous"], {
                            "stage": stage, "detail_code": detail, "processor_index": processor,
                            "category": category, "reason": reason})
                        self.assertEqual(observed["refusal_code"], refusal)
                        self.assertEqual(observed["attempted_entries"], 0)
                        self.assertEqual(observed["completed_exits"], 0)

    def test_all_comparison_fields_have_stable_names(self):
        fields = ("abi_version", "captured_fields", "refusal", "msr_reads", "signature",
                  "maximum_basic_leaf", "maximum_extended_leaf", "leaf1_ecx", "leaf1_edx",
                  "physical_bits", "encryption_eax", "encryption_ebx", "multi_key_eax",
                  "multi_key_ebx", "reserved", "rflags", "cr0", "cr3", "cr4", "efer",
                  "syscfg", "sev_status", "pat", "mtrr_cap", "mtrr_default", "top_mem",
                  "smm_address", "smm_mask", "apic_base", "mmio_config", "iorr", "variable_mtrrs")
        for detail, expected in enumerate(fields, 0x40):
            decoded = cache_rendezvous_diagnostic(0xFF00400B | (detail << 16))
            self.assertEqual(decoded["reason"], expected)
            self.assertEqual(decoded["processor_index"], 255)

    def test_flag_failure_reports_only_the_five_observed_bits(self):
        flag_bits = (("TF", 0x100), ("IF", 0x200), ("DF", 0x400), ("NT", 0x4000), ("AC", 0x40000))
        for mask in range(32):
            refusal = 0x0380400B | (mask << 16)
            observed = decode_frame(frame(phase_detail=0x40070040, cpu_id=0x010843C1,
                context=(refusal, 0)))["native_returning_observation"]["cache_rendezvous"]
            expected = [(name, bit) for index, (name, bit) in enumerate(flag_bits) if mask & (1 << index)]
            self.assertEqual(observed["unsupported_flags"], [name for name, _ in expected])
            self.assertEqual(observed["unsupported_rflags_mask"], sum(bit for _, bit in expected))
            self.assertTrue(observed["flags_observed"])
            self.assertEqual(observed["processor_index"], 3)
            self.assertEqual(observed["reason"], "privilege_or_flags")
            self.assertNotIn("privilege_level", observed)
        plain = cache_rendezvous_diagnostic(0x0311400B)
        self.assertNotIn("flags_observed", plain)
        self.assertNotIn("unsupported_flags", plain)

    def test_global_failures_never_invent_processor_zero(self):
        for code, reason in ((1, "cpu_scope"), (2, "allocation"), (3, "layout"),
                             (4, "released"), (5, "bounds"), (6, "inventory"),
                             (7, "reused_or_invalid_callback"), (8, "cleanup"),
                             (0x7F, "processor_out_of_range")):
            decoded = cache_rendezvous_diagnostic((code << 16) | 0x400B)
            self.assertIsNone(decoded["processor_index"])
            self.assertEqual(decoded["reason"], reason)
            for processor in (1, 255):
                self.assertIsNone(cache_rendezvous_diagnostic((processor << 24) | (code << 16) | 0x400B))

    def test_unknown_legacy_invalid_and_saturated_details_stay_raw(self):
        for refusal in (0x400B, 0x4017, 0x0100400B, 0x0160400B, 0x01A0400B,
                        0x0112400C, 0x01124115, -1, 0x10112400B):
            self.assertIsNone(cache_rendezvous_diagnostic(refusal))
        for metadata in (0x010843C1 | (1 << 29), 0x010843C1 | (1 << 31)):
            observed = decode_frame(frame(phase_detail=0x40070040, cpu_id=metadata,
                context=(0x0152400B, 0)))["native_returning_observation"]
            self.assertNotIn("cache_rendezvous", observed)
            self.assertEqual(observed["raw_words"]["refusal_u32"], 0x0152400B)

    def test_afterfailure_preserves_real_attempts_and_restoration(self):
        observed = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x01085FC2,
            context=(0x02564017, 0x00010001)))["native_returning_observation"]
        self.assertEqual(observed["cache_rendezvous"]["stage"], "after_transition")
        self.assertEqual(observed["cache_rendezvous"]["reason"], "pat")
        self.assertEqual(observed["attempted_entries"], 1)
        self.assertEqual(observed["completed_exits"], 1)
        self.assertTrue(observed["restoration_complete"])
        self.assertTrue(observed["canary_called"])

    def test_single_rejected_cr4_bit_survives_both_stage_families(self):
        for bit in range(64):
            if bit == 3:
                continue
            for stage, phase in ((0x400B, "before_transition"), (0x4017, "after_transition")):
                for processor in (0, 1, 255):
                    refusal = (processor << 24) | ((0xC0 + bit) << 16) | stage
                    observation = decode_frame(frame(phase_detail=0x80070040,
                        cpu_id=0x01085FC2, context=(refusal, 0x00010001)))["native_returning_observation"]
                    diagnostic = observation["cache_rendezvous"]
                    self.assertEqual(diagnostic["stage"], phase)
                    self.assertEqual(diagnostic["reason"], "cr4")
                    self.assertEqual(diagnostic["processor_index"], processor)
                    self.assertEqual(diagnostic["rejected_cr4_bit"], bit)
                    self.assertEqual(diagnostic["rejected_cr4_mask"], f"0x{1 << bit:016x}")
                    self.assertEqual(diagnostic["ignored_cr4_mask"], "0x0000000000000008")
                    self.assertNotIn("bsp_cr4", diagnostic)
                    self.assertNotIn("ap_cr4", diagnostic)
                    self.assertEqual(observation["attempted_entries"], 1)
                    self.assertEqual(observation["completed_exits"], 1)
                    self.assertTrue(observation["restoration_complete"])
        self.assertEqual(cache_rendezvous_diagnostic(0x01C7400B)["rejected_cr4_bit_name"], "PGE")
        self.assertIsNone(cache_rendezvous_diagnostic(0x01FF400B)["rejected_cr4_bit_name"])

    def test_residual_cr4_diagnostics_do_not_reinterpret_legacy_or_invalid_data(self):
        legacy = cache_rendezvous_diagnostic(0x0152400B)
        self.assertEqual(legacy["reason"], "cr4")
        self.assertNotIn("rejected_cr4_bit", legacy)
        for refusal in (0x01C3400B, 0xFFC34017, 0x01C74012, 0x01C74115, 0x101C7400B):
            self.assertIsNone(cache_rendezvous_diagnostic(refusal))
        for flag in (1 << 29, 1 << 31):
            observation = decode_frame(frame(phase_detail=0x40070040,
                cpu_id=0x010843C1 | flag, context=(0x01C7400B, 0)))["native_returning_observation"]
            self.assertNotIn("cache_rendezvous", observation)
            self.assertEqual(observation["raw_words"]["refusal_u32"], 0x01C7400B)


class F7FallbackDiagnostics(unittest.TestCase):
    def test_construction_and_query_tags_survive_full_persistent_wire(self):
        for family, phase in ((0x4115, "construction"), (0x4116, "query_or_handoff")):
            for reason, name in ((3, "cpu_signature"), (8, "sme_capability"),
                                 (16, "root_memory_metadata"), (18, "cpu_context_changed"),
                                 (21, "no_mapping"), (0xFFFE, "reader_borrow_conflict")):
                refusal = (reason << 16) | family
                decoded = decode_frame(frame(phase_detail=0x40070040, cpu_id=1,
                                             context=(refusal, 0)))
                self.assertEqual(decoded["native_returning_observation"]["f7_get_fallback"],
                                 {"phase": phase, "reason_code": reason, "reason": name})
                self.assertEqual(decoded["native_returning_observation"]["refusal_code"], refusal)

    def test_legacy_unknown_and_invalid_records_remain_honest(self):
        for code in (0, 0x4101, 0x180E4101, 0x4115, 0x4116, 0x12344114, 0x100004115):
            self.assertIsNone(f7_fallback_diagnostic(code))
        self.assertEqual(f7_fallback_diagnostic(0x12344115),
                         {"phase": "construction", "reason_code": 0x1234, "reason": "unknown"})
        for metadata in (1 << 29, 1 << 31):
            decoded = decode_frame(frame(phase_detail=0x40070040, cpu_id=metadata,
                                         context=(0x00034115, 0)))
            self.assertNotIn("f7_get_fallback", decoded["native_returning_observation"])


def frame(sequence=42, version=0x02000001, status=0, phase_detail=0x00170009,
          context=(0xFEDCBA98, 0x76543210), cpu_id=0x1234):
    body = struct.pack("<15I", 0x50414E53, version, 0x04030201, 0x08070605,
                       0x14131211, 0x18171615, 0x12345678, sequence, phase_detail,
                       cpu_id, *context, status, 0, 9)
    return (body + struct.pack("<I", zlib.crc32(body)))[::-1].hex()


class SnapshotReaderTests(unittest.TestCase):
    def test_fields_and_endianness(self):
        decoded = decode_frame(frame())
        self.assertEqual(decoded["fpga_build_id"], "0807060504030201")
        self.assertEqual(decoded["rom_build_id"], "1817161514131211")
        self.assertEqual(decoded["context"], "76543210fedcba98")
        self.assertEqual((decoded["sequence"], decoded["phase"], decoded["detail"]), (42, 9, 23))

    def test_two_consecutive_frames(self):
        valid = frame()
        manifest = {"fpga_build_id": "0807060504030201", "rom_build_id": "1817161514131211"}
        self.assertEqual(consistent_snapshot(f"SNAPSHOT:{valid}\nlog\nSNAPSHOT:{valid}", manifest)["sequence"], 42)
        for text in (valid, valid+"\n"+frame(43), valid+"\nSNAPSHOT:bad\n"+valid):
            with self.assertRaises(ValueError):
                consistent_snapshot(text)
        with self.assertRaises(ValueError):
            consistent_snapshot(valid+"\n"+valid, {**manifest, "rom_build_id": "0"*16})

    def test_card_result_survives_lifecycle_without_reinterpreting_older_records(self):
        for phase in (0x28, 0x35, 0x40):
            for bits, expected in ((0, "not_reported"), (0x2000, "loaded_and_freed"),
                                   (0x4000, "load_failed"), (0x6000, "invalid_status")):
                self.assertEqual(decode_frame(frame(phase_detail=((bits | 4) << 16) | phase))
                                 ["card_payload_status"], expected)
        for word in (0x00040040, 0x20050040, 0x20040009):
            self.assertEqual(decode_frame(frame(phase_detail=word))["card_payload_status"], "not_reported")
        for phase, expected in ((0x10, "loaded_and_freed"), (0x1F, "load_failed")):
            word = 0x00050000 | phase
            self.assertEqual(decode_frame(frame(phase_detail=word, context=(0x44524143, 0x44414F4C)))
                             ["card_payload_status"], expected)
            self.assertEqual(decode_frame(frame(phase_detail=word))["card_payload_status"], "not_reported")

    def test_corruption_length_schema_reserved_bits(self):
        valid = frame()
        for invalid in (valid[:-1], valid+"0", "0"*128, ("0" if valid[0]!="0" else "1")+valid[1:],
                        frame(version=0x02000002), frame(status=0x80000000)):
            with self.assertRaises(ValueError):
                decode_frame(invalid)

    def test_returning_result_distinguishes_status_and_immediate_bounded_fields(self):
        # Adapter journal DWORDs 4/5/6 are outcome/counts/stage. The shipped
        # snapshot RTL places journal 6 in frame 9 and journal 4/5 in 10/11.
        immediate = decode_frame(frame(phase_detail=0x20060010, cpu_id=4,
                                       context=(2, 0x00010001)))
        self.assertEqual(immediate["native_returning_status"], "completed")
        self.assertEqual(immediate["card_payload_status"], "not_reported")
        self.assertEqual(immediate["native_returning_observation"], {
            "outcome_low16": 2, "refusal_low16": 0,
            "attempted_entries_low16": 1, "completed_exits_low16": 1,
            "delivery_stage": 4,
        })
        refused = decode_frame(frame(phase_detail=0x40060010, cpu_id=4,
                                     context=(0x40120001, 0)))
        self.assertEqual(refused["native_returning_status"], "refused")
        self.assertEqual(refused["native_returning_observation"]["refusal_low16"], 0x4012)
        failed = decode_frame(frame(phase_detail=0x80060010, cpu_id=4,
                                    context=(0x40160002, 0x00010001)))
        self.assertEqual(failed["native_returning_status"], "failed")
        self.assertEqual(failed["native_returning_observation"]["attempted_entries_low16"], 1)

    def test_returning_wire_fields_do_not_rotate_or_swap_counter_halves(self):
        # Distinct values exercise the raw transport layout, independently of
        # whether such counts would qualify as a valid one-entry probe result.
        decoded = decode_frame(frame(phase_detail=0x80060010, cpu_id=4,
                                     context=(0xABCD1234, 0x56789ABC)))
        self.assertEqual(decoded["native_returning_observation"], {
            "outcome_low16": 0x1234, "refusal_low16": 0xABCD,
            "attempted_entries_low16": 0x9ABC, "completed_exits_low16": 0x5678,
            "delivery_stage": 4,
        })

    def test_returning_lifecycle_preserves_result_without_reinterpreting_context(self):
        for phase in (0x28, 0x35, 0x40):
            for bits, status in ((0, "not_reported"), (0x2000, "completed"),
                                 (0x4000, "refused"), (0x8000, "failed"),
                                 (0x6000, "invalid_status"), (0xE000, "invalid_status")):
                decoded = decode_frame(frame(phase_detail=((bits | 0x106) << 16) | phase))
                self.assertEqual(decoded["native_returning_status"], status)
                self.assertIsNone(decoded["native_returning_observation"])
        for word in (0x20040040, 0x20050010, 0x20060009, 0x20080040):
            decoded = decode_frame(frame(phase_detail=word))
            self.assertEqual(decoded["native_returning_status"], "not_reported")
            self.assertIsNone(decoded["native_returning_observation"])

    def test_persistent_refusal_survives_all_lifecycle_notifications(self):
        # Literal wire values from the detail-7 contract. Refusal 5 represents
        # the structured CPUID refusal fixture used by the real parent harness.
        for phase, metadata, lifecycle in (
            (0x10, 0x000003C1, (0, 0, 0)),
            (0x28, 0x000043C1, (1, 0, 0)),
            (0x35, 0x000843C1, (1, 1, 0)),
            (0x40, 0x010843C1, (1, 1, 1)),
        ):
            decoded = decode_frame(frame(phase_detail=0x40070000 | phase,
                                         cpu_id=metadata, context=(5, 0)))
            observed = decoded["native_returning_observation"]
            self.assertEqual(decoded["native_returning_status"], "refused")
            self.assertIsNone(decoded["cpu_id"])
            self.assertEqual((observed["outcome"], observed["refusal_code"],
                              observed["attempted_entries"], observed["completed_exits"],
                              observed["delivery_stage"]), (1, 5, 0, 0, 4))
            self.assertEqual(tuple(observed["lifecycle_counts"].values()), lifecycle)
            self.assertTrue(observed["rust_entered"] and observed["rust_completed"]
                            and observed["cleanup_complete"])
            self.assertFalse(observed["canary_called"] or observed["canary_observed"]
                             or observed["restoration_complete"])
            self.assertTrue(observed["numeric_fields_exact"] and observed["boolean_fields_valid"]
                            and observed["inner_header_valid"])

    def test_persistent_completed_pristine_and_failed_entries_are_distinct(self):
        for phase_detail, metadata, refusal, counts, expected in (
            (0x20070040, 0x01085FC2, 0, 0x00010001, ("completed", 2, 1, 1, True)),
            (0x40070040, 0x01084040, 0, 0, ("refused", 0, 0, 0, False)),
            (0x80070040, 0x01085FC2, 0x4016, 0x00010002, ("failed", 2, 2, 1, True)),
        ):
            decoded = decode_frame(frame(phase_detail=phase_detail,
                                         cpu_id=metadata, context=(refusal, counts)))
            observed = decoded["native_returning_observation"]
            self.assertEqual((decoded["native_returning_status"], observed["outcome"],
                              observed["attempted_entries"], observed["completed_exits"],
                              observed["canary_called"]), expected)
            self.assertEqual(observed["refusal_code"], refusal)
            self.assertEqual(tuple(observed["lifecycle_counts"].values()), (1, 1, 1))

    def test_lookup_failure_details_survive_lifecycle_frames(self):
        for phase in (0x10, 0x28, 0x35, 0x40):
            for refusal, name, status in (
                (0x180E4101, "EFI_NOT_FOUND", "0x800000000000000e"),
                (0x180F4101, "EFI_ACCESS_DENIED", "0x800000000000000f"),
                (0x10014101, None, "0x0000000000000001"),
            ):
                with self.subTest(phase=phase, refusal=hex(refusal)):
                    observed = decode_frame(frame(phase_detail=0x40070000 | phase,
                        cpu_id=0x010843C1, context=(refusal, 0)))["native_returning_observation"]
                    self.assertEqual(observed["refusal_code"], refusal)
                    detail = observed["attribute_lookup"]
                    self.assertEqual(detail["kind"], "efi_status")
                    self.assertEqual(detail["efi_status"], status)
                    self.assertEqual(detail["efi_status_name"], name)
                    self.assertEqual(detail["status_error_bit"], bool(refusal & 0x08000000))
                    self.assertTrue(detail["status_exact"])
                    self.assertFalse(detail["status_bits_omitted"])

    def test_incomplete_status_cannot_alias_a_standard_error(self):
        # These could be raw EFI_STATUS 0x800000000000040e or 0x40000000:
        # lost high bits must never turn them into NOT_FOUND or SUCCESS.
        for refusal, low, error in ((0x1C0E4101, 14, True), (0x14004101, 0, False)):
            observed = decode_frame(frame(phase_detail=0x40070040, cpu_id=0x010843C1,
                context=(refusal, 0)))["native_returning_observation"]
            detail = observed["attribute_lookup"]
            self.assertEqual(detail["status_low10"], low)
            self.assertEqual(detail["status_error_bit"], error)
            self.assertFalse(detail["status_exact"])
            self.assertTrue(detail["status_bits_omitted"])
            self.assertIsNone(detail["efi_status"])
            self.assertIsNone(detail["efi_status_name"])

    def test_success_with_invalid_interface_has_no_invented_status_failure(self):
        for refusal, expected in ((0x20004101, {"kind": "success_null_interface"}),
                *((0x30004101 | (remainder << 16),
                   {"kind": "success_unaligned_interface", "alignment_remainder": remainder})
                  for remainder in range(1, 8))):
            observed = decode_frame(frame(phase_detail=0x40070040, cpu_id=0x010843C1,
                context=(refusal, 0)))["native_returning_observation"]
            self.assertEqual(observed["attribute_lookup"], expected)
            self.assertEqual(observed["raw_words"]["refusal_u32"], refusal)

    def test_legacy_unknown_and_malformed_lookup_codes_remain_raw(self):
        for refusal in (0x4101, 0x00014101, 0x40004101, 0xF00E4101, 0x10004101,
                        0x20014101, 0x28004101, 0x24004101, 0x30004101,
                        0x30084101, 0x38014101, 0x34014101, 0x180E4102):
            with self.subTest(refusal=hex(refusal)):
                observed = decode_frame(frame(phase_detail=0x40070040, cpu_id=0x010843C1,
                    context=(refusal, 0)))["native_returning_observation"]
                self.assertNotIn("attribute_lookup", observed)
                self.assertEqual(observed["refusal_code"], refusal)
                self.assertEqual(observed["raw_words"]["refusal_u32"], refusal)

    def test_invalid_header_or_overflow_suppresses_lookup_interpretation(self):
        for invalid in (1 << 29, 1 << 31, (1 << 29) | (1 << 31)):
            observed = decode_frame(frame(phase_detail=0x40070040,
                cpu_id=0x010843C1 | invalid, context=(0x180E4101, 0)))["native_returning_observation"]
            self.assertNotIn("attribute_lookup", observed)
            self.assertEqual(observed["raw_words"]["refusal_u32"], 0x180E4101)

    def test_persistent_invalid_encoding_does_not_invent_exact_values(self):
        overflow = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x3FFFFFFF,
                                      context=(0xFFFFFFFF, 0xFFFFFFFF)))["native_returning_observation"]
        self.assertTrue(overflow["encoding_overflow"])
        for field in ("outcome", "refusal_code", "attempted_entries", "completed_exits", "delivery_stage"):
            self.assertIsNone(overflow[field])
        self.assertTrue(all(value is None for value in overflow["lifecycle_counts"].values()))
        self.assertEqual(overflow["raw_words"]["refusal_u32"], 0xFFFFFFFF)
        invalid_bool = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x41085FC2,
                                         context=(0x4016, 0x00010001)))["native_returning_observation"]
        for field in ("rust_entered", "rust_completed", "cleanup_complete", "restoration_complete",
                      "canary_called", "canary_observed"):
            self.assertIsNone(invalid_bool[field])
        self.assertEqual(invalid_bool["refusal_code"], 0x4016)
        invalid_header = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x81085FC2,
                                           context=(0x4016, 0x00010001)))["native_returning_observation"]
        self.assertFalse(invalid_header["inner_header_valid"])

    def test_persistent_counts_and_failure_flag_do_not_overlap(self):
        metadata = 0x00002040 | (16 << 14) | (15 << 19) | (14 << 24)
        observed = decode_frame(frame(phase_detail=0x87070040, cpu_id=metadata,
                                      context=(0x12345678, 0x56789ABC)))["native_returning_observation"]
        self.assertEqual(observed["refusal_code"], 0x12345678)
        self.assertEqual((observed["attempted_entries"], observed["completed_exits"]), (0x9ABC, 0x5678))
        self.assertEqual(tuple(observed["lifecycle_counts"].values()), (16, 15, 14))
        self.assertTrue(observed["canary_failure_present"])

    def test_reader_configuration_has_no_programming_path(self):
        config = (Path(__file__).parent / "openocd/read_snapshot.cfg").read_text()
        commands = [line.strip() for line in config.splitlines() if line.strip() and not line.lstrip().startswith("#")]
        allowed = {"interface", "ftdi_vid_pid", "ftdi_channel", "ftdi_layout_init", "reset_config",
                   "adapter", "adapter_khz", "jtag", "init", "irscan", "for", "echo", "sleep", "}", "shutdown"}
        self.assertTrue(all(line.split()[0] in allowed for line in commands))
        self.assertEqual([line for line in commands if line.startswith("adapter ")],
                         ["adapter driver ftdi", "adapter speed 1000"])
        self.assertEqual([line for line in commands if line.startswith("irscan")], ["irscan xc7.tap 0x03", "irscan xc7.tap 0x22"])
        self.assertIn('echo "SNAPSHOT:[drscan xc7.tap 512 0]"', commands)
        self.assertIn('echo "CPU_SNAPSHOT:[drscan xc7.tap 1024 $bank]"', commands)
        self.assertIn('echo "CPU_REQUEST:$bank"', commands)
        self.assertNotIn("format %x", config)
        for forbidden in ("jtagspi", "pld load", "JPROGRAM", "JSTART", "CFG_IN", "source "):
            self.assertNotIn(forbidden.lower(), "\n".join(commands).lower())


class MultiExitObservations(unittest.TestCase):
    def test_complete_profile_and_partial_failures_keep_actual_counts(self):
        decoded = decode_frame(frame(phase_detail=0x20070040, cpu_id=0x01085FCC,
                                     context=(0, 0x00410041)))
        self.assertEqual(decoded["native_returning_status"], "completed")
        observed = decoded["native_returning_observation"]
        self.assertEqual(observed["outcome"], 12)
        self.assertEqual((observed["attempted_entries"], observed["completed_exits"]), (65, 65))
        self.assertTrue(observed["restoration_complete"])
        self.assertNotIn("multi_exit_failure", observed)
        reasons = ("entry_exit_count", "exit_site", "operand", "guest_registers",
                   "guest_stack_or_flags", "pending_event", "next_rip", "unexpected_exit", "entry_limit")
        for code, reason in enumerate(reasons, 0x4301):
            for entries, exits in ((1, 1), (17, 17), (65, 64)):
                decoded = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x01085FC7,
                    context=(code, entries | (exits << 16))))
                self.assertEqual(decoded["native_returning_status"], "failed")
                observed = decoded["native_returning_observation"]
                self.assertEqual(observed["multi_exit_failure"], {"reason": reason})
                self.assertEqual((observed["attempted_entries"], observed["completed_exits"]), (entries, exits))

    def test_invalid_and_unknown_failure_encodings_stay_raw(self):
        for code in (0x4300, 0x430A, 0x14303, 0x4025):
            observed = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x01085FC7,
                context=(code, 0x00110011)))["native_returning_observation"]
            self.assertNotIn("multi_exit_failure", observed)
        for invalid in (1 << 29, 1 << 31):
            observed = decode_frame(frame(phase_detail=0x80070040, cpu_id=0x01085FC7 | invalid,
                context=(0x4303, 0x00110011)))["native_returning_observation"]
            self.assertNotIn("multi_exit_failure", observed)
            self.assertEqual(observed["raw_words"]["refusal_u32"], 0x4303)




class ResidentBootSnapshotTests(unittest.TestCase):
    def test_ap_failure_preserves_slot_predicate_and_full_observation(self):
        for slot, count in ((0, 1), (17, 24), (31, 32)):
            for reason in (*range(1, 34), 0x40, 0x46, 0x6f, 0x7f):
                metadata = 0x04000082 | (reason << 8) | (slot << 16) | ((count - 1) << 21)
                observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=0x12345678,
                    context=(metadata, 0xe0010033)))["native_resident_observation"]
                self.assertTrue(observed["encoding_valid"])
                self.assertEqual(observed["activation_failure"], 33)
                self.assertEqual((observed["processor_slot"], observed["processor_count"]), (slot, count))
                self.assertEqual(observed["observed_value"], 0x12345678e0010033)
                self.assertEqual(observed["boundary_predicate"], reason if reason < 0x40 else None)
                self.assertEqual(observed["callback_return_code"], reason - 0x40 if reason >= 0x40 else None)
                self.assertFalse(observed["windows_boot_proven"] or observed["all_cpus_activated"])

    def test_ap_wait_failures_keep_identity_and_ack_distinct(self):
        for reason, actual, expected in ((0x81, 31, 17), (0x82, 0x400, 1 << 17)):
            metadata = 0x04000082 | (reason << 8) | (17 << 16) | (23 << 21)
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=expected,
                context=(metadata, actual)))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            if reason == 0x81:
                self.assertEqual((observed["observed_apic_id"], observed["expected_apic_id"]), (actual, expected))
                self.assertIsNone(observed["guest_ack_mask"])
            else:
                self.assertEqual((observed["guest_ack_mask"], observed["required_ack_bit"]), (actual, expected))
                self.assertIsNone(observed["observed_apic_id"])
            self.assertIsNone(observed["observed_value"])

    def test_ap_large_callback_code_is_lossless_and_not_cr0(self):
        metadata = 0x04008382 | (17 << 16) | (23 << 21)
        for code in (64, 0x8000000000000003):
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=code >> 32,
                context=(metadata, code & 0xffffffff)))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertEqual(observed["callback_return_code"], code)
            self.assertIsNone(observed["observed_value"])
            self.assertIsNone(observed["observation_kind"])

    def test_ap_failure_invalid_metadata_and_inconsistent_evidence_stay_raw(self):
        good = 0x04000082 | (17 << 16) | (23 << 21)
        for metadata, actual, expected in (
                (good | (34 << 8), 0, 0), (good, 0, 0),
                ((good | (1 << 8)) ^ (1 << 26), 0, 0),
                (good | (1 << 8) | (1 << 27), 0, 0),
                (0x04000182 | (31 << 16), 0, 0),
                (good | (0x81 << 8), 17, 17), (good | (0x81 << 8), 256, 17),
                (good | (0x82 << 8), 0, 1 << 16),
                (good | (0x83 << 8), 63, 0),
                (good | (0x82 << 8), 1 << 17, 1 << 17)):
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=expected,
                context=(metadata, actual)))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            for field in ("activation_failure", "processor_slot", "reason", "observed_value"):
                self.assertIsNone(observed[field])
            self.assertEqual(observed["raw_words"]["journal4"], metadata)

    def test_legacy_failure33_does_not_invent_ap_refusal(self):
        observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=24,
            context=(0x80, 33)))["native_resident_observation"]
        self.assertTrue(observed["encoding_valid"])
        self.assertEqual(observed["activation_failure"], 33)
        self.assertNotIn("reason", observed)

    def test_bsp_routing_failure_exports_exact_selected_field_in_crc_frame(self):
        for predicate, name, actual, expected in (
                (1, "signature", 0xb40f41, 0xb40f40),
                (2, "version", 0x81050011, 0x81050010),
                (3, "feature", 0x40006, 0x40007),
                (4, "topology", 31, 24),
                (5, "extended_control_reserved_bits", 0x80000008, 0)):
            metadata = 0x01180081 | (predicate << 8)
            encoded = frame(phase_detail=0x00080013, cpu_id=expected,
                            context=(metadata, actual))
            observed = decode_frame(encoded)["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertEqual(observed["activation_failure"], 42)
            self.assertEqual(observed["predicate"], name)
            self.assertEqual(observed["processor_count"], 24)
            self.assertEqual(observed["observed_value"], actual)
            self.assertEqual(observed["expected_value"], expected)
            self.assertEqual(observed["raw_words"],
                             {"journal4": metadata, "journal5": actual, "journal6": expected})
            self.assertFalse(observed["all_cpus_activated"] or observed["windows_boot_proven"]
                             or observed["post_loader_failure_exported"])
            # Corrupt this newly encoded payload without recomputing its CRC.
            corrupted = list(encoded)
            corrupted[40] = "0" if corrupted[40] != "0" else "1"
            with self.assertRaises(ValueError):
                decode_frame("".join(corrupted))

    def test_legacy_failure42_does_not_invent_a_routing_predicate(self):
        observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=24,
            context=(0x80, 42)))["native_resident_observation"]
        self.assertTrue(observed["encoding_valid"])
        self.assertEqual(observed["activation_failure"], 42)
        self.assertEqual(observed["format"], "resident_boot_v1")
        self.assertNotIn("predicate", observed)

    def test_malformed_routing_records_stay_raw(self):
        for metadata, actual, expected in (
                (0x00180181, 1, 0xb40f40), (0x02180181, 1, 0xb40f40),
                (0x01000181, 1, 0xb40f40), (0x01210181, 1, 0xb40f40),
                (0x01180081, 1, 0), (0x01180681, 1, 0),
                (0x01180181, 0xb40f40, 0xb40f40), (0x01180281, 1, 2),
                (0x01180481, 31, 23), (0x01180581, 0, 0),
                (0x01180581, 12, 0), (0x01180581, 8, 4)):
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=expected,
                context=(metadata, actual)))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            for key in ("predicate", "activation_failure", "observed_value", "expected_value"):
                self.assertIsNone(observed[key])
            self.assertEqual(observed["raw_words"]["journal4"], metadata)

    def test_preparation_failure_preserves_reason_status_and_address(self):
        observed = decode_frame(frame(phase_detail=0x00080014, cpu_id=0x56789abc,
            context=(0x12340206, 0x80000003)))["native_resident_observation"]
        self.assertTrue(observed["encoding_valid"])
        self.assertEqual(observed["stage_name"], "resident_allocation")
        self.assertEqual(observed["reason_name"], "allocation_address")
        self.assertEqual(observed["underlying_status"], 0x8000000000000003)
        self.assertEqual(observed["address"], 0x123456789abc)
        self.assertFalse(observed["hook_armed"])
        self.assertFalse(observed["windows_boot_proven"])

    def test_preparation_record_rejects_unknown_codes_without_guessing(self):
        for metadata in (0, 255, 0xff06):
            observed = decode_frame(frame(phase_detail=0x00080014, cpu_id=0,
                context=(metadata, 0x80000009)))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertIsNone(observed["stage_name"])
            self.assertIsNone(observed["hook_armed"])

    def test_bootstrap_gate_records_preserve_observed_values(self):
        for stage, name, value in ((25, "processor_count", 24),
                (26, "CPUID.1:ECX", 0x7ed8320b), (27, "VM_CR", 2),
                (28, "APIC_BASE", 0xfee00900)):
            observed = decode_frame(frame(phase_detail=0x00080014, cpu_id=value,
                context=(stage, 0x80000003)))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertEqual(observed["observed_register"], name)
            self.assertEqual(observed["observed_value"], value)
            self.assertFalse(observed["all_cpus_activated"])

    def test_native_boot_rotated_words_and_truthful_milestones(self):
        for stage in (1, 2, 3, 4, 5, 0x80):
            slot = 17 if stage in (3, 4) else 0
            result = decode_frame(frame(phase_detail=0x00080013, cpu_id=24,
                context=(stage | (slot << 16), 45 if stage == 0x80 else 0)))
            observed = result["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertEqual(observed["stage"], stage)
            self.assertEqual(observed["processor_count"], 24)
            self.assertEqual(observed["all_cpus_activated"], stage == 5)
            self.assertFalse(observed["windows_boot_proven"])
            self.assertIsNone(result["cpu_id"])
            self.assertEqual(result["native_returning_status"], "not_reported")

    def test_parent_failure_preserves_full_status_without_claiming_activation(self):
        observed = decode_frame(frame(phase_detail=0x00080010, cpu_id=0x80000000,
            context=(3 | 0x400, 26)))["native_resident_observation"]
        self.assertEqual(observed["failure"], 0x800000000000001a)
        self.assertEqual(observed["failure_kind"], "efi_status")
        self.assertFalse(observed["hook_armed"])
        self.assertFalse(observed["windows_boot_proven"])

    def test_invalid_resident_encodings_remain_raw(self):
        for metadata, count, detail in ((9, 24, 0), (3 | (24 << 16), 24, 0),
                                       (5, 0, 0), (5, 33, 0), (5, 24, 1)):
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=count,
                context=(metadata, detail)))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertFalse(observed["all_cpus_activated"])
            self.assertEqual(observed["raw_words"]["journal4"], metadata)


class TerminalStopDiagnostics(unittest.TestCase):
    def test_activation_reports_prepared_observer_without_claiming_transport_or_boot(self):
        for detail, prepared in ((0, None), (0x01000000, False), (0x01000001, True)):
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=24,
                context=(5, detail)))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertTrue(observed["all_cpus_activated"])
            self.assertEqual(observed["terminal_reporter_prepared"], prepared)
            self.assertFalse(observed["post_loader_failure_exported"])
            self.assertFalse(observed["windows_boot_proven"])
        for stage, detail in ((5, 1), (5, 0x01000002), (5, 0x02000001), (4, 0x01000001)):
            observed = decode_frame(frame(phase_detail=0x00080013, cpu_id=24,
                context=(stage, detail)))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertIsNone(observed["terminal_reporter_prepared"])

    @staticmethod
    def record(kind, code, value, slot=23, version=1):
        metadata = 0x83 | (slot << 8) | (code << 13) | (kind << 24) | (version << 28)
        return frame(phase_detail=0x00080013, cpu_id=value >> 32,
                     context=(metadata, value & 0xffffffff))

    def test_full_width_context_and_exit_survive_crc_frame(self):
        cases = ((0, 0x72, 0xfffff80012345678, "unhandled_exit", "guest_rip"),
                 (1, 0x7b, 0xfffff80087654321, "instruction_fetch_refusal", "guest_rip"),
                 (2, 0x7c, 0xc0011020, "msr_refusal", "msr_index"),
                 (3, 0x400, 0x123456789abc, "nested_page_fault", "guest_physical_address"),
                 (4, 0x7ff, 0xffffffffffffffff, "wide_exit_code", "raw_exit_code"),
                 (5, 0x63, 0x123456789abc, "init_ack_refusal", "guest_rip"),
                 (6, 0x72, 0x123456789abc, "startup_service_refusal", "guest_rip"),
                 (7, 0x63, 0x123456789abc, "unexpected_init", "guest_rip"),
                 (8, 0x400, 0xfee00300, "apic_mmio_refusal", "guest_physical_address"),
                 (9, 0x7c, 0x830, "route_refusal", "msr_index"))
        for kind, code, value, reason, context_kind in cases:
            with self.subTest(kind=kind):
                encoded = self.record(kind, code, value)
                observed = decode_frame(encoded)["native_resident_observation"]
                self.assertTrue(observed["encoding_valid"])
                self.assertEqual(observed["processor_slot"], 23)
                self.assertEqual(observed["exit_code"], value if kind == 4 else code)
                self.assertEqual(observed["reason"], reason)
                self.assertEqual(observed["context_kind"], context_kind)
                self.assertEqual(observed["context_value"], value)
                self.assertTrue(observed["post_activation_terminal_failure_exported"])
                self.assertFalse(observed["windows_boot_proven"])
                self.assertFalse(observed["post_loader_failure_exported"])
                self.assertIsNone(observed["processor_count"])
                for key in ("guest_rip", "msr_index", "guest_physical_address"):
                    self.assertEqual(observed[key], value if context_kind == key else None)
                corrupted = list(encoded)
                corrupted[40] = "0" if corrupted[40] != "0" else "1"
                with self.assertRaises(ValueError):
                    decode_frame("".join(corrupted))

    def test_detailed_fetch_preserves_canonical_rip_and_exact_predicate(self):
        for rip in (0, 0x7fffffffffff, 0xffff800000000000, 0xfffff800b363797a):
            for failure, predicate, level in ((4, "unsupported_guest_cache_control", None),
                    (0x314, "unreadable_table", 3), (0x39, "physical_memory_not_proven_wb", None)):
                value = (failure << 48) | (rip & 0xffffffffffff)
                observed = decode_frame(self.record(10, 0x7c, value))["native_resident_observation"]
                self.assertTrue(observed["encoding_valid"])
                self.assertEqual(observed["guest_rip"], rip)
                self.assertEqual(observed["fetch_failure"], {"code": failure, "predicate": predicate, "walk_level": level})
                self.assertIsNone(observed["msr_index"])
                self.assertFalse(observed["windows_boot_proven"])
        for failure in (0, 0x14, 0x514, 0x130, 0x3b, 0xffff):
            observed = decode_frame(self.record(10, 0x7c, failure << 48))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertIsNone(observed["guest_rip"])
            self.assertIsNone(observed["fetch_failure"])

    def test_apic_diagnostic_operands_and_old_wire_remain_distinct(self):
        for code, value, predicate, operand in (
                (10, 0x1800020, "unsupported_cr4_controls", "guest_cr4"),
                (0x19, 0xffffffffffffffff, "nested_fault_information", "raw_exit_info1"),
                (0x654, 0x4030, "unreadable_table", "physical_read_address"),
                (0x84, 0x30, "unsupported_register", "register_access_context")):
            result=read_snapshot.apic_failure_diagnostic(code,value)
            self.assertEqual((result["predicate"], result["operand_kind"]),(predicate,operand))
        result=read_snapshot.apic_failure_diagnostic(0x12,(2<<56)|0xc38b)
        self.assertEqual(result["fetched_bytes"],[0x8b,0xc3])
        self.assertFalse(result["truncated"])
        result=read_snapshot.apic_failure_diagnostic(0x17,(12<<56)|0x07060504030201)
        self.assertEqual(result["fetched_length"],12)
        self.assertTrue(result["truncated"])

    def test_apic_full_frame_and_all_wire_reason_values_are_bounded(self):
        for code in range(2048):
            value=(min(code-0x10,7)<<56)|1 if 0x11 <= code <= 0x17 else 0
            detail=read_snapshot.apic_failure_diagnostic(code,value)
            observed=decode_frame(self.record(13,code,value))["native_resident_observation"]
            self.assertEqual(observed["encoding_valid"],detail is not None,code)
            if detail is not None:
                self.assertEqual(observed["exit_code"],0x400)
                self.assertEqual(observed["apic_failure"],detail)
                self.assertIsNone(observed["guest_physical_address"])
                self.assertIsNone(observed["guest_rip"])
                self.assertIsNone(observed["msr_index"])
                self.assertFalse(observed["windows_boot_proven"])
        old=decode_frame(self.record(8,0x400,0xfee00030))["native_resident_observation"]
        self.assertIsNone(old["apic_failure"])
        self.assertEqual(old["guest_physical_address"],0xfee00030)

    def test_apic_diagnostic_rejects_invalid_codes_lengths_and_operands(self):
        for code, value in ((0,0),(0x10,0),(0x414,0),(0x554,0),(0x800,0),
                (0x12,(1<<56)|0x8b),(0x11,(1<<56)|0xc38b),(0x17,(16<<56)|1),
                (0x42,1),(0x40,256),(0x84,1<<17),(0x84,1<<32)):
            self.assertIsNone(read_snapshot.apic_failure_diagnostic(code,value),(code,value))

    def test_vmcr_error_identity_direction_and_operand_provenance(self):
        for code, value, operand in ((0x206, 0xfedcba9876543210, "attempted_vmcr_value"),
                (0x454, 0xffffffffffffffff, "hardware_next_rip"),
                (0x15, 0x320f, "fetched_opcode_le16"), (0x13, 0x800000000001, "proposed_next_rip")):
            observed = decode_frame(self.record(12, code, value))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertEqual(observed["exit_code"], 0x7c)
            self.assertEqual(observed["msr_index"], 0xc0010114)
            self.assertEqual(observed["reason"], "vmcr_refusal")
            self.assertIsNone(observed["efer_failure"])
            failure = observed["vmcr_failure"]
            self.assertEqual(failure["operand_kind"], operand)
            self.assertEqual(failure["operand_value"], value)
            self.assertEqual(failure["access"], ("read", "write", "invalid_exit_info1")[code >> 9])
            self.assertEqual(failure["fetched_opcode_bytes"], [15, 50] if code == 0x15 else None)
            self.assertFalse(observed["windows_boot_proven"])
        for code, value in ((1, 0x10), (2, 0), (0x55, 0), (0x600, 0), (0x14, 3), (0x15, 0x10000)):
            observed = decode_frame(self.record(12, code, value))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertIsNone(observed["vmcr_failure"])
            self.assertIsNone(observed["msr_index"])

    def test_efer_hardware_continuation_has_distinct_operand_provenance(self):
        for reason in (0x51, 0x53, 0x54, 0x61, 0x63, 0x64):
            observed = decode_frame(self.record(11, reason, 0xffffffffffffffff))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            failure = observed["efer_failure"]
            self.assertEqual(failure["operand_kind"], "hardware_next_rip")
            self.assertEqual(failure["operand_value"], 0xffffffffffffffff)
            self.assertIsNone(failure["fetched_opcode_bytes"])
            self.assertFalse(observed["windows_boot_proven"])
        for reason in (0x55, 0x65):
            self.assertFalse(decode_frame(self.record(11, reason, 0))["native_resident_observation"]["encoding_valid"])

    def test_efer_error_preserves_full_operand_and_implicit_msr_identity(self):
        for code, value, error, subreason in (
                (0x206, 0xfedcba9876543210, "unsupported_value", None),
                (2, 0xffffffffffffffff, "backing_mismatch", None),
                (0x15, 0x320f, "instruction", "unsupported_instruction_bytes"),
                (0x13, 0x800000000001, "instruction", "noncanonical_nrip"),
                (0x428, 0xffffffffffffffff, "pending_state", "unsupported_control"),
                (0x4d, 0, "fault_state", "inconsistent_virtual_interrupt_exit")):
            observed = decode_frame(self.record(11, code, value))["native_resident_observation"]
            self.assertTrue(observed["encoding_valid"])
            self.assertEqual(observed["exit_code"], 0x7c)
            self.assertEqual(observed["msr_index"], 0xc0000080)
            self.assertIsNone(observed["guest_rip"])
            self.assertEqual(observed["efer_failure"]["error"], error)
            self.assertEqual(observed["efer_failure"]["subreason"], subreason)
            self.assertEqual(observed["efer_failure"]["operand_value"], value)
            self.assertEqual(observed["efer_failure"]["access"], ("read", "write", "invalid_exit_info1")[code >> 9])
            if code == 0x15:
                self.assertEqual(observed["efer_failure"]["fetched_opcode_bytes"], [15,50])
                self.assertEqual(observed["efer_failure"]["fetched_opcode_width"], 2)
            if code == 0x13:
                self.assertEqual(observed["efer_failure"]["operand_kind"], "proposed_next_rip")
            self.assertFalse(observed["windows_boot_proven"])
        for code, value in ((0,0), (7,0), (0x600,0), (0x22,1), (0x20,256),
                            (0x15,0x10000), (0x14,3), (0x16,0), (0x2e,0), (5,1<<32)):
            observed = decode_frame(self.record(11, code, value))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertIsNone(observed["efer_failure"])
            self.assertIsNone(observed["msr_index"])
            self.assertFalse(observed["all_cpus_activated"])

    def test_unknown_or_inconsistent_record_retains_raw_without_claims(self):
        for kind, code, value, version in ((10, 0x72, 1, 1), (15, 0, 0, 1),
                (0, 0x72, 1, 0), (0, 0x72, 1, 2), (4, 0x72, 0x1000, 1),
                (4, 0x7ff, 0x7ff, 1)):
            observed = decode_frame(self.record(kind, code, value, version=version))["native_resident_observation"]
            self.assertFalse(observed["encoding_valid"])
            self.assertFalse(observed["post_activation_terminal_failure_exported"])
            self.assertFalse(observed["all_cpus_activated"])
            for key in ("exit_code", "processor_slot", "reason", "context_kind", "context_value"):
                self.assertIsNone(observed[key])
            self.assertEqual(observed["raw_words"]["journal5"], value & 0xffffffff)
            self.assertEqual(observed["raw_words"]["journal6"], value >> 32)


class ReusableLiveCaptureTests(unittest.TestCase):
    def test_live_capture_needs_no_expected_build_file(self):
        encoded = frame(phase_detail=0x00080013, cpu_id=24, context=(0x80, 33))
        output = (f"SNAPSHOT:{encoded}\nSNAPSHOT:{encoded}\n").encode()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with mock.patch.object(read_snapshot, '__file__', str(root / 'firmware/card/read_snapshot.py')), \
                    mock.patch('sys.argv', ['read_snapshot.py', '--live']), \
                    mock.patch.object(read_snapshot.subprocess, 'run', return_value=mock.Mock(returncode=0, stdout=output)) as run, \
                    contextlib.redirect_stdout(io.StringIO()) as console:
                read_snapshot.main()
            run.assert_called_once()
            self.assertEqual(json.loads(console.getvalue())['native_resident_observation']['activation_failure'], 33)
            sessions = list((root / 'target/firmware/card/snapshots').iterdir())
            self.assertEqual(len(sessions), 1)
            self.assertTrue((sessions[0] / 'snapshot.json').is_file())
            self.assertFalse((sessions[0] / 'expected-manifest.json').exists())

    def test_unpinned_capture_still_rejects_corrupt_frames_and_retains_log(self):
        encoded = frame()
        corrupted = ('0' if encoded[0] != '0' else '1') + encoded[1:]
        output = (f"SNAPSHOT:{corrupted}\nSNAPSHOT:{corrupted}\n").encode()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with mock.patch.object(read_snapshot, '__file__', str(root / 'firmware/card/read_snapshot.py')), \
                    mock.patch('sys.argv', ['read_snapshot.py', '--live']), \
                    mock.patch.object(read_snapshot.subprocess, 'run', return_value=mock.Mock(returncode=0, stdout=output)), \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                read_snapshot.main()
            self.assertEqual(error.exception.code, 1)
            self.assertEqual(len(list(root.rglob('openocd.log'))), 1)
            self.assertEqual(len(list(root.rglob('snapshot.json'))), 0)


if __name__ == "__main__":
    unittest.main()
