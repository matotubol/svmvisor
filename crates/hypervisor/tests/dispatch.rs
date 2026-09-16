use svmvisor_hypervisor::{
    memory::address::{AddressPolicy, EncryptionState},
    arch::x86_64::capabilities::{
        CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
    },
    svm::dispatch::{DispatchError, DispatchOutcome, StopReason, handle_exit},
    svm::exit::ExitSnapshot,
    guest::state::GuestStateRequest,
    arch::x86_64::registers::GuestRegisters,
    svm::vmcb::Vmcb,
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
        encryption: EncryptionState::Unencrypted {
            encryption_bit: None,
        },
        optional: OptionalFeatures {
            nrip_save: nrip,
            ..OptionalFeatures::default()
        },
    }
    .validate()
    .unwrap()
}

fn state(rax: u64) -> (Vmcb, GuestRegisters) {
    let policy = AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let mut vmcb = Vmcb::new();
    vmcb.set_synthetic_state(
        &GuestStateRequest {
            rip: 0x1000,
            rsp: 0x8000,
            rflags: 2,
            cr0: 0x8001_0033,
            cr3: 0x9000,
            cr4: 0x20,
            efer: 0x1500,
            rax,
        }
        .validate(&policy)
        .unwrap(),
    );
    (
        vmcb,
        GuestRegisters {
            rcx: u64::MAX,
            rdx: u64::MAX,
            rbx: u64::MAX,
            rbp: 1,
            rsi: 2,
            rdi: 3,
            r8: 4,
            r9: 5,
            r10: 6,
            r11: 7,
            r12: 8,
            r13: 9,
            r14: 10,
            r15: 11,
        },
    )
}

fn snapshot(code: u64, nrip: u64) -> ExitSnapshot {
    ExitSnapshot {
        code,
        info1: 0,
        info2: 0,
        rip: 0x1000,
        nrip,
    }
}

#[test]
fn cpuid_updates_only_four_outputs_and_checked_rip_then_rejects_stale_replay() {
    let (mut vmcb, mut frame) = state(0xffff_ffff_0000_0001);
    let mut expected_frame = frame;
    expected_frame.rbx = 0;
    expected_frame.rcx = 1 << 31;
    expected_frame.rdx = (1 << 5) | (1 << 6);
    let mut expected = *vmcb.bytes();
    expected[0x578..0x580].copy_from_slice(&0x1002u64.to_le_bytes());
    expected[0x5f8..0x600].fill(0);
    let exit = snapshot(0x72, 0x1002);
    assert_eq!(
        handle_exit(exit, &mut vmcb, &mut frame, &capabilities(true)),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!(frame, expected_frame);
    assert_eq!(vmcb.bytes(), &expected);
    assert_eq!(
        handle_exit(exit, &mut vmcb, &mut frame, &capabilities(true)),
        Err(DispatchError::SnapshotRipMismatch)
    );
    assert_eq!(frame, expected_frame);
    assert_eq!(vmcb.bytes(), &expected);
}

#[test]
fn query_returns_abi_but_stop_and_unknown_opcodes_do_not_advance() {
    let (mut vmcb, mut frame) = state(0);
    let before_frame = frame;
    assert_eq!(
        handle_exit(
            snapshot(0x81, 0x1003),
            &mut vmcb,
            &mut frame,
            &capabilities(true)
        ),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!((vmcb.guest_rax(), vmcb.guest_rip()), (1, 0x1003));
    assert_eq!(frame, before_frame);
    for opcode in [1, 2, 1u64 << 32, u64::MAX] {
        let (mut vmcb, mut frame) = state(opcode);
        let before = *vmcb.bytes();
        let before_frame = frame;
        let reason = if opcode == 1 {
            StopReason::Requested
        } else {
            StopReason::UnsupportedHypercall { opcode }
        };
        assert_eq!(
            handle_exit(
                snapshot(0x81, 0),
                &mut vmcb,
                &mut frame,
                &capabilities(false)
            ),
            Ok(DispatchOutcome::Stop(reason))
        );
        assert_eq!(vmcb.bytes(), &before);
        assert_eq!(frame, before_frame);
    }
}

#[test]
fn rejected_resume_checks_and_terminal_exits_preserve_all_state() {
    for (nrip, established) in [(0, true), (0x1000, true), (0x1010, true), (0x1002, false)] {
        let (mut vmcb, mut frame) = state(1);
        let before = *vmcb.bytes();
        let before_frame = frame;
        assert!(matches!(
            handle_exit(
                snapshot(0x72, nrip),
                &mut vmcb,
                &mut frame,
                &capabilities(established)
            ),
            Err(DispatchError::Resume(_))
        ));
        assert_eq!(vmcb.bytes(), &before);
        assert_eq!(frame, before_frame);
    }
    for code in [0x78, 0x400, u64::MAX, 0x7b, 0x7c, 0x1_0000_0072] {
        let (mut vmcb, mut frame) = state(1);
        let before = *vmcb.bytes();
        let before_frame = frame;
        assert_eq!(
            handle_exit(
                snapshot(code, 0x1002),
                &mut vmcb,
                &mut frame,
                &capabilities(true)
            ),
            Ok(DispatchOutcome::Stop(StopReason::Exit(
                snapshot(code, 0x1002).action()
            )))
        );
        assert_eq!(vmcb.bytes(), &before);
        assert_eq!(frame, before_frame);
    }
}

#[test]
fn opted_in_clock_dispatch_preserves_transactional_refusal() {
    use svmvisor_hypervisor::arch::x86_64::clock::{ClockCapabilities, ClockPlan};
    use svmvisor_hypervisor::svm::dispatch::handle_exit_with_instruction_and_clock;
    let clock = ClockPlan::admit(
        ClockCapabilities::detect(0x30, 1 << 27, 0).unwrap(),
        Some(0),
        None,
        123,
    )
    .unwrap();
    let (mut vmcb, mut frame) = state(1);
    let before = *vmcb.bytes();
    let before_frame = frame;
    assert!(
        handle_exit_with_instruction_and_clock(
            snapshot(0x72, 0),
            &mut vmcb,
            &mut frame,
            &[0x90],
            &clock
        )
        .is_err()
    );
    assert_eq!(*vmcb.bytes(), before);
    assert_eq!(frame, before_frame);
    let stopped = handle_exit_with_instruction_and_clock(
        snapshot(0x7c, 0),
        &mut vmcb,
        &mut frame,
        &[],
        &clock,
    )
    .unwrap();
    assert!(matches!(stopped, DispatchOutcome::Stop(_)));
    assert_eq!(*vmcb.bytes(), before);
    assert_eq!(frame, before_frame);
    assert_eq!(
        handle_exit_with_instruction_and_clock(
            snapshot(0x72, 0),
            &mut vmcb,
            &mut frame,
            &[0x0f, 0xa2],
            &clock
        ),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!(frame.rdx, 0x70);
    assert_eq!(frame.rcx, 1 << 31);
    assert_eq!(vmcb.guest_rip(), 0x1002);
}
