use crate::host::resident::runtime::RawVmexitCapture;

#[test]
fn raw_boundary_requires_same_generation_vmcb_and_physical_cpu() {
    let mut raw: RawVmexitCapture = unsafe { core::mem::zeroed() };
    raw.entry_sequence = 19;
    raw.exit_sequence = 19;
    raw.entry_vmcb_pa = 0x123000;
    raw.exit_vmcb_pa = 0x123000;
    raw.context_vmcb_pa = 0x123000;
    raw.physical_apic_id = 21;
    raw.code = 0x7c;
    raw.guest_rcx = 0xc001_0010;
    raw.physical_cache_valid = 1;
    raw.exit_rip = 0xffff800000001111;
    raw.nrip = 0xffff800000002222;
    raw.entry_rip = 0xffff800000003333;
    raw.guest_cr0 = 0xe0000011;
    raw.mtrr_def_type = 0xc06;
    raw.host_cr0 = 0x80010011;
    assert_eq!(
        raw.stop_record(0x123000, 21, 0xf400, 16),
        Some((
            10,
            [
                0xffff800000001111,
                0xffff800000002222,
                0xffff800000003333,
                0xe0000011,
                0xc06,
                0x80010011
            ]
        ))
    );
    for failure in 0..6 {
        let mut bad = raw;
        match failure {
            0 => bad.entry_sequence = 0,
            1 => bad.exit_sequence = 18,
            2 => bad.entry_vmcb_pa += 4096,
            3 => bad.exit_vmcb_pa += 4096,
            4 => bad.context_vmcb_pa += 4096,
            _ => bad.physical_apic_id = 13,
        }
        let record = bad.stop_record(0x123000, 21, 0xf400, 16).unwrap();
        assert_eq!(record.0, 11);
        assert_eq!(
            record.1,
            [
                bad.exit_vmcb_pa,
                0x123000,
                (bad.physical_apic_id << 32) | 21,
                raw.exit_rip,
                raw.nrip,
                raw.entry_rip
            ]
        );
    }
    raw.physical_cache_valid = 0;
    assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
    raw.physical_cache_valid = 1;
    raw.guest_rcx = 0xc0000080;
    assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
}

#[test]
fn raw_cache_operands_require_provenance_but_no_syscfg_physical_sample() {
    let mut raw: RawVmexitCapture = unsafe { core::mem::zeroed() };
    raw.entry_sequence = 29;
    raw.exit_sequence = 29;
    raw.entry_vmcb_pa = 0x123000;
    raw.exit_vmcb_pa = 0x123000;
    raw.context_vmcb_pa = 0x123000;
    raw.physical_apic_id = 21;
    raw.code = 0x7c;
    raw.exit_rip = 0xffff800000001111;
    raw.nrip = raw.exit_rip + 2;
    raw.guest_rax = 0xfeedface76543210;
    raw.guest_rdx = 0xdeadc0defedcba98;
    for index in crate::svm::native_cache::owned_msrs().filter(|&index| index != 0xc001_0010) {
        raw.guest_rcx = 0x1234567800000000 | u64::from(index);
        assert_eq!(
            raw.stop_record(0x123000, 21, 0x12345678_f400, 0xfedcba98_00000010),
            Some((
                13,
                [
                    raw.exit_rip,
                    raw.guest_rcx,
                    0xfedcba9876543210,
                    raw.nrip,
                    0x12345678_f400,
                    0xfedcba98_00000010
                ]
            ))
        );
    }
    for failure in 0..6 {
        let mut bad = raw;
        match failure {
            0 => bad.entry_sequence = 0,
            1 => bad.exit_sequence -= 1,
            2 => bad.entry_vmcb_pa += 4096,
            3 => bad.exit_vmcb_pa += 4096,
            4 => bad.context_vmcb_pa += 4096,
            _ => bad.physical_apic_id += 1,
        }
        assert_eq!(bad.stop_record(0x123000, 21, 0xf400, 16).unwrap().0, 11);
    }
    raw.guest_rcx = 0xc000_00e9;
    assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
    raw.guest_rcx = 0xc001_0015;
    raw.code = 0x72;
    assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
}
