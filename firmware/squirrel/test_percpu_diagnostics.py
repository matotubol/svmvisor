import importlib.util
from pathlib import Path
import struct
import unittest
import zlib
import contextlib
import io
import json
import tempfile
from unittest import mock

spec=importlib.util.spec_from_file_location("reader",Path(__file__).with_name("read_snapshot.py"))
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)

def frame(bank=0,valid=True,fault=False):
    words=[0x55504353,0x04000001,0x89abcdef,0x01234567,0x9abcdef0,0x12345678,bank|(64 if valid else 0),0]+[0]*24
    if valid:
        words[8:27]=[7,0x104|(0x10000 if fault else 0),9,31,1,2]+list(range(12))+[123]
    raw=struct.pack("<31I",*words[:31]);return (raw+struct.pack("<I",zlib.crc32(raw)))[::-1].hex()

def admission_frame(bank=31, boot=9, operation=4, predicate=7, processor=31,
                    count=32, apic=27, observed=0xfedcba9876543208,
                    expected=0x10, status=48, item=0xc0010015, fault=True):
    words=list(struct.unpack("<32I", bytes.fromhex(frame(bank, fault=bool(bank & 32)))[::-1]))
    words[9]=0x10c | (0x10000 if fault else 0)
    words[10]=boot; words[11]=apic
    contexts=[predicate, item, observed, expected, status, processor | (count << 32)]
    for index, value in enumerate(contexts):
        words[14+index*2]=value & 0xffffffff
        words[15+index*2]=value >> 32
    words[26]=operation
    raw=struct.pack("<31I", *words[:31])
    return (raw+struct.pack("<I", zlib.crc32(raw)))[::-1].hex()

def admission_header(operation=4, predicate=7, boot=9):
    # Actual journal4/5/6 -> USER2 frame10/11/9 rotation.
    words=[0x50414e53,0x02000001,0x89abcdef,0x01234567,0x9abcdef0,0x12345678,
           boot,3,20 | (8 << 16),predicate,19 | (32 << 8) | (operation << 16),0x80000003,0,0,0]
    raw=struct.pack("<15I", *words)
    return (raw+struct.pack("<I", zlib.crc32(raw)))[::-1].hex()

class PerCpuSnapshotTests(unittest.TestCase):
    def test_raw_cache_msr_keeps_full_operands_and_original_refusal(self):
        contexts=[0xffff800000001111, 0x12345678c0010015, 0xfedcba9876543210,
                  0xffff800000001113, 0x123456780000f400, 0xfedcba9800000010]
        words=list(struct.unpack("<32I", bytes.fromhex(frame(63, fault=True))[::-1]))
        words[9]=0x1010d
        for index, value in enumerate(contexts):
            words[14+index*2]=value & 0xffffffff
            words[15+index*2]=value >> 32
        words[26]=contexts[4] & 0xffffffff
        raw=struct.pack("<31I", *words[:31])
        encoded=(raw+struct.pack("<I", zlib.crc32(raw)))[::-1].hex()
        result=r.decode_percpu_frame(encoded)
        record=result["record"]
        self.assertEqual(result["processor_slot"], 31)
        self.assertEqual(result["kind"], "first_fault")
        self.assertEqual(record["event_name"], "raw_cache_msr_boundary")
        self.assertEqual(record["contexts"], contexts)
        self.assertEqual(record["msr_index"], 0xc0010015)
        self.assertEqual(record["fields"]["edx_eax_operand"], 0xfedcba9876543210)
        self.assertEqual(record["fields"]["reason_info1"], contexts[4])
        self.assertEqual(record["fields"]["reason_info2"], contexts[5])
        self.assertEqual(record["aux_name"], "stop_reason_low32")
        self.assertEqual(record["boundary_observation"], "assembly_capture_before_dispatch")
        self.assertEqual(record["access"], "not_exported")
        self.assertEqual(record["instruction_completed"], "not_established")

    def test_post_ebs_survey_binds_only_current_boot_cache_failure(self):
        frame=r.decode_percpu_frame(admission_frame(operation=6,predicate=12,item=0x26c))
        header={"phase":19,"boot_id":9,"fpga_build_id":frame["fpga_build_id"],
            "rom_build_id":frame["rom_build_id"],"native_resident_observation":
            {"encoding_valid":True,"stage":0x80,"activation_failure":48}}
        detail=r.bind_admission_failure(header,{"frames":[frame]})
        self.assertEqual(detail["ownership_boundary"],"post_ebs_cache_survey")
        self.assertEqual(detail["predicate_name"],"physical_bank_agreement")
        header["boot_id"]=10
        self.assertEqual(r.bind_admission_failure(header,{"frames":[frame]})["status"],"full_record_unavailable")
        header["native_resident_observation"]["activation_failure"]=33
        self.assertIsNone(r.bind_admission_failure(header,{"frames":[frame]}))

    def test_active_fixed_tuple_and_extension_profile_operands_are_distinct(self):
        detail=r.decode_percpu_frame(admission_frame(predicate=20,item=0x26c,
            observed=0x1d1d1d1d1d1d1d1d,expected=0))["record"]["admission_failure"]
        self.assertEqual(detail["predicate_name"],"active_fixed_mtrr_extended_tuple")
        self.assertEqual(detail["expected_field_role"],"unused")
        detail=r.decode_percpu_frame(admission_frame(predicate=21,item=0xc0010010,
            observed=0x700000,expected=1<<18))["record"]["admission_failure"]
        self.assertEqual(detail["predicate_name"],"active_fixed_mtrr_extensions_enabled")
        self.assertEqual(detail["comparison"],"required_set_mask")

    def test_admission_full_width_operands_and_unknown_identities(self):
        record=r.decode_percpu_frame(admission_frame(item=0xfedcba9876543210,
            expected=0x123456789abcdef0, status=0x8000000000000007))["record"]
        detail=record["admission_failure"]
        self.assertEqual(record["aux_name"], "admission_operation")
        self.assertEqual(detail["register_or_address"], "0xfedcba9876543210")
        self.assertEqual(detail["observed"], "0xfedcba9876543208")
        self.assertEqual(detail["expected_or_context"], "0x123456789abcdef0")
        self.assertEqual(detail["original_status_or_code"], "0x8000000000000007")
        self.assertEqual(detail["comparison_mask"], "0x0000000000000018")
        self.assertFalse(detail["operands_truncated"])
        self.assertFalse(detail["runtime_activation_claim"])
        unknown=r.decode_percpu_frame(admission_frame(processor=0xffffffff, apic=0xffffffff))["record"]["admission_failure"]
        self.assertIsNone(unknown["processor"])
        self.assertIsNone(unknown["apic_id"])

    def test_admission_masks_and_firmware_status_are_distinct(self):
        for predicate, comparison, role in [(2,"required_set_mask","required_mask"),
                (6,"allowed_set_mask","allowed_mask"), (7,"masked_equality_mask_0x18","masked_expected_value")]:
            detail=r.decode_percpu_frame(admission_frame(predicate=predicate))["record"]["admission_failure"]
            self.assertEqual(detail["comparison"],comparison)
            self.assertEqual(detail["expected_field_role"],role)
        for predicate in (1,3,4,6,9,11,14,15,17):
            detail=r.decode_percpu_frame(admission_frame(operation=2,predicate=predicate,
                status=0x8000000000000003))["record"]["admission_failure"]
            self.assertEqual(detail["failure_origin"],"firmware_service")
        detail=r.decode_percpu_frame(admission_frame(operation=2,predicate=12,
            status=0x8000000000000003))["record"]["admission_failure"]
        self.assertEqual(detail["failure_origin"],"software_rejection")
        self.assertEqual(detail["predicate_name"],"callback_wrong_processor")
        normalized=r.decode_percpu_frame(admission_frame(operation=6,predicate=12,item=0xc0010010))["record"]["admission_failure"]
        self.assertTrue(normalized["operands_normalized"])

    def test_admission_malformed_metadata_is_rejected(self):
        for change in [dict(operation=0),dict(operation=9),dict(predicate=0),dict(predicate=1<<32),
                       dict(count=0),dict(count=33),dict(processor=32),dict(processor=30),dict(fault=False)]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                r.decode_percpu_frame(admission_frame(**change))

    def test_admission_helper_operands_keep_address_mask_and_pat_roles(self):
        detail=r.decode_percpu_frame(admission_frame(operation=3,predicate=471,
            item=0x123456789ab0, observed=0xfedcba9876543210, expected=0xffff800012345000))["record"]["admission_failure"]
        self.assertEqual(detail["predicate_name"],"reserved_entry")
        self.assertEqual(detail["paging_level"],4)
        self.assertEqual(detail["expected_field_role"],"requested_virtual_address")
        self.assertEqual(detail["expected_or_context"],"0xffff800012345000")
        unreadable=r.decode_percpu_frame(admission_frame(operation=3,predicate=469,
            item=0x123456789ab0,observed=0))["record"]["admission_failure"]
        self.assertIsNone(unreadable["observed"])
        self.assertFalse(unreadable["observed_value_available"])
        self.assertEqual(unreadable["register_or_address"],"0x0000123456789ab0")
        self.assertEqual(unreadable["observed_encoded_placeholder"],"0x0000000000000000")
        for predicate, comparison in [(109,"forbidden_set_mask"),(114,"selected_pat_and_pat0_must_be_wb"),
                                      (135,"low8_equality"),(616,"not_equal"),(624,"selected_pat_must_be_wb")]:
            detail=r.decode_percpu_frame(admission_frame(operation=3,predicate=predicate))["record"]["admission_failure"]
            self.assertEqual(detail["comparison"],comparison)
        unknown=r.decode_percpu_frame(admission_frame(operation=3,predicate=0xffffffff))["record"]["admission_failure"]
        self.assertFalse(unknown["predicate_known"])
        self.assertEqual(unknown["predicate"],0xffffffff)

    def test_admission_binding_uses_current_boot_and_rejects_conflicting_records(self):
        header=r.decode_frame(admission_header())
        def bind(*frames):
            return r.bind_admission_failure(header,r.percpu_snapshots("\n".join("CPU_SNAPSHOT:"+f for f in frames)))
        stale=admission_frame(bank=63,boot=8,observed=123)
        result=bind(admission_frame(),stale)
        self.assertEqual(result["source_bank_kind"],"last_progress")
        self.assertEqual(result["matching_record_count"],1)
        self.assertEqual(bind(admission_frame(bank=63))["source_bank_kind"],"first_fault")
        self.assertEqual(bind(admission_frame(),admission_frame(bank=63))["matching_record_count"],2)
        self.assertEqual(bind(admission_frame(),admission_frame(bank=63,observed=123))["status"],"conflicting_full_records")
        self.assertEqual(bind(admission_frame(),admission_frame(bank=30,processor=30))["status"],"conflicting_full_records")
        for unavailable in (stale, admission_frame(predicate=8), admission_frame(boot=10)):
            self.assertEqual(bind(unavailable)["status"],"full_record_unavailable")
        other=r.decode_percpu_frame(admission_frame());other["fpga_build_id"]="0000000000000000"
        self.assertEqual(r.bind_admission_failure(header,{"frames":[other]})["status"],"full_record_unavailable")
        header["native_resident_observation"]["encoding_valid"]=False
        self.assertIsNone(r.bind_admission_failure(header,{"frames":[]}))

    def test_admission_offline_cli_binds_full_record_to_preparation_refusal(self):
        with tempfile.TemporaryDirectory() as temp:
            path=Path(temp)/"capture.log"
            path.write_text("\n".join(["SNAPSHOT:"+admission_header()]*2+
                ["CPU_SNAPSHOT:"+admission_frame(),"CPU_SNAPSHOT:"+admission_frame(bank=63,boot=8)]))
            with mock.patch("sys.argv",["read_snapshot.py","--input",str(path)]), contextlib.redirect_stdout(io.StringIO()) as output:
                r.main()
            decoded=json.loads(output.getvalue())
            self.assertEqual(decoded["processor_admission_failure"]["observed"],"0xfedcba9876543208")
            self.assertEqual(decoded["native_resident_observation"]["reason_name"],"processor_admission_detail")
            self.assertFalse(decoded["native_resident_observation"]["all_cpus_activated"])

    def test_raw_boundary_distinguishes_hardware_return_from_handler_stop(self):
        def captured(event, contexts):
            words=list(struct.unpack('<32I', bytes.fromhex(frame(53,fault=True))[::-1]))
            words[9]=0x10100|event
            for i,value in enumerate(contexts):
                words[14+i*2]=value & 0xffffffff
                words[15+i*2]=value >> 32
            words[26]=0x0754f10d
            raw=struct.pack('<31I',*words[:31])
            return r.decode_percpu_frame((raw+struct.pack('<I',zlib.crc32(raw)))[::-1].hex())['record']
        rip=0xfffff8017e165f9d
        record=captured(10,[rip,rip+2,rip,0xe0010033,0xc06,0x80010033])
        self.assertFalse(record['zero_length_at_hardware_return'])
        self.assertEqual(record['fields']['guest_cr0'],0xe0010033)
        self.assertEqual(record['fields']['physical_mtrr_def_type'],0xc06)
        self.assertEqual(record['aux_name'],'stop_reason')
        self.assertTrue(captured(10,[rip+2,rip+2,rip,0,0,0])['zero_length_at_hardware_return'])
        identity=captured(11,[0x3fb51000,0x3fa51000,(25<<32)|24,rip,rip+2,rip])
        self.assertEqual(identity['captured_apic_id'],25)
        self.assertEqual(identity['assigned_apic_id'],24)
        self.assertEqual(identity['fields']['expected_vmcb_pa'],0x3fa51000)

    def test_valid_and_empty_banks(self):
        f=r.decode_percpu_frame(frame(31));self.assertEqual(f["processor_slot"],31)
        self.assertEqual(f["record"]["contexts"][0],1<<32)
        self.assertEqual(r.decode_percpu_frame(frame(63,fault=True))["kind"],"first_fault")
        self.assertIsNone(r.decode_percpu_frame(frame(2,valid=False))["record"])
    def test_corruption_wrong_build_and_false_sticky_rejected(self):
        for text in [frame()[:-2]+"00",frame(32)]:
            with self.assertRaises(ValueError):r.decode_percpu_frame(text)
        with self.assertRaises(ValueError):r.decode_percpu_frame(frame(),{"fpga_build_id":"0","rom_build_id":"0"})
    def test_partial_collection_is_explicit(self):
        result=r.percpu_snapshots("CPU_SNAPSHOT:"+frame(5)+"\nCPU_SNAPSHOT:broken")
        self.assertEqual(result["rejected_frames"],1)
        self.assertEqual(len(result["missing_banks"]),63)
        self.assertFalse(result["all_banks_read"])
    def test_cpu_evidence_does_not_require_legacy_stable_pair(self):
        with tempfile.TemporaryDirectory() as temp:
            path=Path(temp)/"capture.log";path.write_text("CPU_SNAPSHOT:"+frame(5))
            with mock.patch("sys.argv",["read_snapshot.py","--input",str(path)]), contextlib.redirect_stdout(io.StringIO()) as output:
                r.main()
            result=json.loads(output.getvalue())
            self.assertIn("legacy_snapshot_error",result)
            self.assertEqual(result["percpu_diagnostics"]["frames"][0]["processor_slot"],5)

    def test_requested_banks_require_matching_returned_banks(self):
        result=r.percpu_snapshots("CPU_REQUEST:53\nCPU_SNAPSHOT:"+frame(35,fault=True))
        self.assertEqual(result["requested_banks"],[53])
        self.assertEqual(result["unmatched_requested_banks"],[53])
        self.assertEqual(result["capture_status"],"partial")
        self.assertEqual(result["frame_count"],1)
        complete="\n".join("CPU_REQUEST:"+str(b)+"\nCPU_SNAPSHOT:"+frame(b,valid=False) for b in range(64))
        result=r.percpu_snapshots(complete)
        self.assertEqual(result["capture_status"],"complete")
        self.assertEqual(result["unmatched_requested_banks"],[])
        self.assertTrue(result["all_banks_read"])

    def test_real_rtl_vector(self):
        vector=Path(__file__).parent/"tests/percpu-record-v1.hex"
        self.assertEqual(r.decode_percpu_frame(vector.read_text().strip())["record"]["sequence"],5)

if __name__=="__main__":unittest.main()
