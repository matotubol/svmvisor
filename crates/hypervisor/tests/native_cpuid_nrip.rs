use svmvisor_hypervisor::{
    arch::x86_64::capabilities::{
        CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
    },
    arch::x86_64::registers::GuestRegisters,
    host::resident::fetch,
    memory::address::EncryptionState,
    svm::{
        dispatch::{DispatchOutcome, handle_native_cpuid_with_nrip},
        vmcb::Vmcb,
    },
};

fn capabilities(nrip: bool) -> ValidatedCapabilities {
    CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(1),
        asid_count: Some(16),
        physical_address_bits: Some(48),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption: EncryptionState::Unencrypted { encryption_bit: None },
        optional: OptionalFeatures { nrip_save: nrip, ..Default::default() },
    }
    .validate()
    .unwrap()
}

// Model an exclusively stopped hardware VMCB. Production exposes no mutable
// byte accessor; these tests make no claim of executing the physical CPU.
fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn word(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}
fn stopped(rip: u64) -> (Vmcb, GuestRegisters) {
    let mut vmcb = Vmcb::new();
    for (offset, value) in [
        (0x70, 0x72),
        (0xc8, rip + 2),
        (0x410, 0x29bu64 << 16),
        (0x4d0, 0x1500),
        (0x558, 0x80000001),
        (0x548, 0x20),
        (0x550, 0x1000),
        (0x578, rip),
        (0x570, 0x10002),
        (0x5f8, 0),
        (0x668, 0x0007040600070606),
    ] {
        put(&mut vmcb, offset, value);
    }
    (
        vmcb,
        GuestRegisters {
            rbx: u64::MAX,
            rcx: u64::MAX,
            rdx: u64::MAX,
            rbp: 0x1234,
            r15: 0x5678,
            ..Default::default()
        },
    )
}

#[test]
fn hardware_cpuid_completes_without_rewalking_legal_wb_cache_selection() {
    let (mut vmcb, mut frame) = stopped(0xfffff806a77701d9);
    // PAT1 is WB; CR3.PWT selects it for the first table. Fetch now accepts
    // that selector, but NRIP completion must still need no physical backing.
    put(&mut vmcb, 0x550, 0x1008);
    let mut reads = 0;
    assert!(matches!(
        fetch::instruction(&vmcb, 48, word(&vmcb, 0x668), |_, _| {
            reads += 1;
            None
        }),
        Err(fetch::FetchError::Walk(
            svmvisor_hypervisor::host::paging::WalkError::UnreadableTable { .. }
        ))
    ));
    assert_eq!(reads, 1);
    let original = frame;
    assert_eq!(
        handle_native_cpuid_with_nrip(
            &mut vmcb,
            &mut frame,
            &capabilities(true),
            [10, 11, 12, 13],
            true,
            false,
            None
        ),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!(vmcb.guest_rip(), 0xfffff806a77701db);
    assert_eq!(vmcb.guest_rax(), 10);
    assert_eq!((frame.rbx, frame.rcx, frame.rdx), (11, 12, 13));
    assert_eq!((frame.rbp, frame.r15), (original.rbp, original.r15));
    assert_eq!(word(&vmcb, 0x550), 0x1008);
    assert_eq!(word(&vmcb, 0x570), 2); // Existing retirement clears RF, not other flags.
}

#[test]
fn malformed_or_unowned_hardware_continuations_leave_all_state_unchanged() {
    for case in 0..15 {
        let (mut vmcb, mut frame) = stopped(0x2000);
        let mut supported = true;
        let mut startup = true;
        match case {
            0 => supported = false,
            1 => put(&mut vmcb, 0xc8, 0),
            2 => put(&mut vmcb, 0xc8, 0x2001),
            3 => put(&mut vmcb, 0xc8, 0x2010), // Greater than maximum decoded length.
            4 => put(&mut vmcb, 0xc8, 0x1fff),
            5 => put(&mut vmcb, 0xc8, 0x0000800000000000),
            6 => put(&mut vmcb, 0x70, 0x81),
            7 => put(&mut vmcb, 0x4c8, 4 << 24), // Invalid CPL.
            8 => put(&mut vmcb, 0x4d0, 0x1000),
            9 => put(&mut vmcb, 0x410, 0x9b << 16),
            10 => put(&mut vmcb, 0x570, 0x102),
            11 => put(&mut vmcb, 0xa8, 1 << 31),
            12 => put(&mut vmcb, 0x88, 1 << 31),
            13 => put(&mut vmcb, 0x60, 1 << 25),
            _ => {
                put(&mut vmcb, 0x60, 1 << 24);
                startup = false;
            }
        }
        let bytes = *vmcb.bytes();
        let old = frame;
        assert!(
            handle_native_cpuid_with_nrip(
                &mut vmcb,
                &mut frame,
                &capabilities(supported),
                [10, 11, 12, 13],
                startup,
                false,
                None
            )
            .is_err(),
            "case {case}"
        );
        assert_eq!(vmcb.bytes(), &bytes, "case {case}");
        assert_eq!(frame, old, "case {case}");
    }
}

#[test]
fn hardware_continuation_reuses_native_cpuid_policy_and_interrupt_shadow_retirement() {
    let (mut vmcb, mut frame) = stopped(0x2fff); // Page-crossing opcode needs no memory reread.
    put(&mut vmcb, 0x5f8, 1);
    put(&mut vmcb, 0x548, 0x20 | (1 << 18));
    put(&mut vmcb, 0x68, 1);
    put(&mut vmcb, 0x60, 1 << 24);
    handle_native_cpuid_with_nrip(
        &mut vmcb,
        &mut frame,
        &capabilities(true),
        [1, 2, 1 << 26, 4],
        true,
        false,
        None,
    )
    .unwrap();
    assert_eq!(frame.rcx, (1 << 26) | (1 << 27)); // OSXSAVE follows guest CR4 when XSAVE exists.
    assert_eq!(vmcb.guest_rip(), 0x3001);
    assert_eq!(word(&vmcb, 0x68), 0);
    assert_eq!(word(&vmcb, 0x60), 1 << 24);
}

fn code_mode(vmcb: &mut Vmcb, long_mode: bool, code64: bool, default32: bool, cpl: u64) {
    let attributes =
        0x9b | (cpl << 5) | if code64 { 0x200 } else { 0 } | if default32 { 0x400 } else { 0 };
    put(vmcb, 0x410, cpl | (attributes << 16) | (0xffff_ffff << 32));
    put(vmcb, 0x4c8, cpl << 24);
    if !long_mode {
        put(vmcb, 0x4d0, 0x1000);
        put(vmcb, 0x558, 1);
    }
}

#[test]
fn hardware_cpuid_supports_user_compatibility_and_native_startup_with_decoded_prefixes() {
    // Inert hardware exit model: actual legal-prefix execution belongs to the
    // emulator/native fixture. Prefix bytes have explicit caller provenance.
    for (long_mode, code64, default32) in [
        (true, true, false),
        (true, false, true),
        (true, false, false),
        (false, false, true),
        (false, false, false),
    ] {
        for cpl in 0..=3 {
            for length in 2..=15 {
                let (mut vmcb, mut frame) = stopped(0x2fff);
                code_mode(&mut vmcb, long_mode, code64, default32, cpl);
                put(&mut vmcb, 0xc8, 0x2fff + length);
                put(&mut vmcb, 0x418, 0x1000_0000); // Nonzero CS.base: nRIP remains an offset.
                put(&mut vmcb, 0x570, 0x10ad7); // RF and arithmetic flags, TF clear.
                let original = frame;
                let mut bytes = [0x66; 15];
                bytes[length as usize - 2..length as usize].copy_from_slice(&[0x0f, 0xa2]);
                assert_eq!(
                    handle_native_cpuid_with_nrip(
                        &mut vmcb,
                        &mut frame,
                        &capabilities(true),
                        [10, 11, 12, 13],
                        true,
                        false,
                        Some(&bytes[..length as usize])
                    ),
                    Ok(DispatchOutcome::ResumePrepared),
                    "mode {long_mode}/{code64}/{default32} CPL{cpl} len{length}"
                );
                assert_eq!(vmcb.guest_rip(), 0x2fff + length);
                assert_eq!((vmcb.guest_rax(), frame.rbx, frame.rcx, frame.rdx), (10, 11, 12, 13));
                assert_eq!((frame.rbp, frame.r15), (original.rbp, original.r15));
                assert_eq!(word(&vmcb, 0x570), 0xad7);
            }
        }
    }
}

#[test]
fn user_cpuid_disable_queues_gp_without_completing_instruction() {
    for cpl in 0..=3 {
        for compat in [false, true] {
            let (mut vmcb, mut frame) = stopped(0x2000);
            code_mode(&mut vmcb, true, !compat, compat, cpl);
            put(&mut vmcb, 0xc8, 0x2005);
            let before = *vmcb.bytes();
            let original = frame;
            let result = handle_native_cpuid_with_nrip(
                &mut vmcb,
                &mut frame,
                &capabilities(true),
                [10, 11, 12, 13],
                true,
                true,
                Some(&[0x26, 0x66, 0x67, 0x0f, 0xa2]),
            );
            if cpl == 0 {
                assert_eq!(result, Ok(DispatchOutcome::ResumePrepared));
                assert_eq!(vmcb.guest_rip(), 0x2005);
            } else {
                assert_eq!(result, Ok(DispatchOutcome::GeneralProtectionPrepared));
                assert_eq!(vmcb.guest_rip(), 0x2000);
                assert_eq!(frame, original);
                assert_eq!(&vmcb.bytes()[0x400..], &before[0x400..]);
                assert_eq!(word(&vmcb, 0xa8), 0x8000_0b0d);
            }
        }
    }
}

#[test]
fn unsupported_mode_boundaries_and_fault_conflicts_preserve_stopped_state() {
    for case in 0..12 {
        let (mut vmcb, mut frame) = stopped(0xfffe);
        code_mode(&mut vmcb, true, false, false, 3);
        if case >= 3 {
            put(&mut vmcb, 0x578, 0x2000);
            put(&mut vmcb, 0xc8, 0x2002);
        }
        let mut startup = true;
        match case {
            0 => {} // 16-bit next-IP overflow even with a large CS limit.
            1 => {
                put(&mut vmcb, 0x578, 0xffff_fffe);
                put(&mut vmcb, 0xc8, 0x1_0000_0000);
                code_mode(&mut vmcb, true, false, true, 3);
            }
            2 => {
                put(&mut vmcb, 0x578, 0xffff_fffe);
                put(&mut vmcb, 0xc8, 0);
                code_mode(&mut vmcb, true, false, true, 3);
            } // Wrapped nRIP.
            3 => {
                put(&mut vmcb, 0x578, 0x2000);
                put(&mut vmcb, 0xc8, 0x2002);
                put(&mut vmcb, 0x410, (0x20_00u64 << 32) | (0xfb << 16) | 3);
            } // CS limit.
            4 => code_mode(&mut vmcb, true, true, true, 3), // CS.L and D both set.
            5 => put(&mut vmcb, 0x570, 0x20002),            // VM86 unsupported.
            6 => {
                code_mode(&mut vmcb, false, false, true, 3);
                startup = false;
            }
            7 => {
                code_mode(&mut vmcb, false, false, true, 3);
                put(&mut vmcb, 0x558, 0x80000001);
            }
            8 => put(&mut vmcb, 0x548, 0x1020), // LA57 outside native profile.
            9 => put(&mut vmcb, 0xa8, 1 << 31), // Existing injection + user fault.
            10 => put(&mut vmcb, 0x88, 1 << 31), // Interrupted delivery + user fault.
            _ => put(&mut vmcb, 0x570, 0x102),  // Single step has no owner.
        }
        let before = *vmcb.bytes();
        let original = frame;
        assert!(
            handle_native_cpuid_with_nrip(
                &mut vmcb,
                &mut frame,
                &capabilities(true),
                [10, 11, 12, 13],
                startup,
                true,
                None
            )
            .is_err(),
            "case {case}"
        );
        assert_eq!(vmcb.bytes(), &before, "case {case}");
        assert_eq!(frame, original, "case {case}");
    }
}

#[test]
fn prefixed_cpuid_requires_exact_non_lock_bytes_and_mode_correct_rex() {
    for code64 in [false, true] {
        for bytes in [
            &[0xf0, 0x0f, 0xa2][..],
            &[0xf3, 0x0f, 0xa2],
            &[0xf2, 0x0f, 0xa2],
            &[0x66, 0x0f, 0xa3],
            &[0x48, 0x66, 0x0f, 0xa2],
        ] {
            let (mut vmcb, mut frame) = stopped(0x2000);
            code_mode(&mut vmcb, true, code64, !code64, 3);
            put(&mut vmcb, 0xc8, 0x2000 + bytes.len() as u64);
            let before = *vmcb.bytes();
            let original = frame;
            assert!(
                handle_native_cpuid_with_nrip(
                    &mut vmcb,
                    &mut frame,
                    &capabilities(true),
                    [10, 11, 12, 13],
                    true,
                    false,
                    Some(bytes)
                )
                .is_err()
            );
            assert_eq!(vmcb.bytes(), &before);
            assert_eq!(frame, original);
        }
        let (mut vmcb, mut frame) = stopped(0x2000);
        code_mode(&mut vmcb, true, code64, !code64, 3);
        put(&mut vmcb, 0xc8, 0x2003);
        let before = *vmcb.bytes();
        let original = frame;
        for bytes in [None, Some(&[0x0f, 0xa2][..]), Some(&[0x66, 0x66, 0x0f, 0xa2][..])] {
            assert!(
                handle_native_cpuid_with_nrip(
                    &mut vmcb,
                    &mut frame,
                    &capabilities(true),
                    [10, 11, 12, 13],
                    true,
                    false,
                    bytes
                )
                .is_err()
            );
            assert_eq!(vmcb.bytes(), &before);
            assert_eq!(frame, original);
        }
        let result = handle_native_cpuid_with_nrip(
            &mut vmcb,
            &mut frame,
            &capabilities(true),
            [10, 11, 12, 13],
            true,
            false,
            Some(&[0x48, 0x0f, 0xa2]),
        );
        if code64 {
            assert_eq!(result, Ok(DispatchOutcome::ResumePrepared));
        } else {
            assert!(result.is_err());
            assert_eq!(vmcb.bytes(), &before);
            assert_eq!(frame, original);
        }
    }
}

#[test]
fn cpuid_prefix_fetch_crosses_pages_in_compatibility_mode_without_changing_other_fetch_modes() {
    let (mut vmcb, _) = stopped(0xffe);
    code_mode(&mut vmcb, true, false, true, 3);
    put(&mut vmcb, 0xc8, 0x1002);
    put(&mut vmcb, 0x418, 0x1000); // Fetch linear1ffe..2001, not offsetffe..1001.
    let source = |address: u64, width: usize| match (address, width) {
        (0x1000, 8) => Some(0x2007),
        (0x2000, 8) => Some(0x3007),
        (0x3000, 8) => Some(0x4007),
        (0x4008, 8) => Some(0x9007),
        (0x4010, 8) => Some(0xb007),
        (0x9ffe, 1) => Some(0x66),
        (0x9fff, 1) => Some(0x67),
        (0xb000, 1) => Some(0x0f),
        (0xb001, 1) => Some(0xa2),
        _ => None,
    };
    let before = *vmcb.bytes();
    let mut byte_reads = 0;
    let bytes = fetch::cpuid_instruction(&vmcb, 48, 6, 4, true, |address, width| {
        byte_reads += usize::from(width == 1);
        source(address, width)
    })
    .unwrap();
    assert_eq!(&bytes[..4], &[0x66, 0x67, 0x0f, 0xa2]);
    assert_eq!(byte_reads, 4);
    assert_eq!(vmcb.bytes(), &before);
    assert_eq!(fetch::instruction(&vmcb, 48, 6, source), Err(fetch::FetchError::UnsupportedMode));
    for (address, replacement, expected) in [
        (0x4010, Some(0xb003), fetch::FetchError::PrivilegeMismatch),
        (0x4010, Some(0x8000_0000_0000_b007), fetch::FetchError::NotExecutable),
        (0xb001, None, fetch::FetchError::UnreadableInstruction { address: 0xb001 }),
    ] {
        // NX is supported for this rejection test.
        put(&mut vmcb, 0x4d0, 0x1d00);
        assert_eq!(
            fetch::cpuid_instruction(&vmcb, 48, 6, 4, true, |a, w| {
                if a == address { replacement } else { source(a, w) }
            }),
            Err(expected)
        );
    }
}

#[test]
fn cpuid_prefix_fetch_bounds_entire_span_before_touching_legacy_memory() {
    let (mut vmcb, _) = stopped(0x100);
    code_mode(&mut vmcb, false, false, false, 0);
    put(&mut vmcb, 0xc8, 0x103);
    put(&mut vmcb, 0x418, 0x7000);
    let mut reads = 0;
    let result = fetch::cpuid_instruction(&vmcb, 48, 6, 3, true, |address, width| {
        assert_eq!(width, 1);
        assert!((0x7100..0x7103).contains(&address));
        reads += 1;
        Some([0x66, 0x0f, 0xa2][(address - 0x7100) as usize])
    })
    .unwrap();
    assert_eq!(&result[..3], &[0x66, 0x0f, 0xa2]);
    assert_eq!(reads, 3);
    for length in [0, 2, 4, 16, usize::MAX] {
        assert!(
            fetch::cpuid_instruction(&vmcb, 48, 6, length, true, |_, _| panic!(
                "invalid span read"
            ))
            .is_err()
        );
    }
    assert!(
        fetch::cpuid_instruction(&vmcb, 48, 6, 3, false, |_, _| panic!("unowned read")).is_err()
    );
    put(&mut vmcb, 0x418, u32::MAX as u64);
    assert!(
        fetch::cpuid_instruction(&vmcb, 48, 6, 3, true, |_, _| panic!("wrapped read")).is_err()
    );
}
