use svmvisor_hypervisor::{
    address::{AddressPolicy, EncryptionState},
    dispatch::{DispatchError, DispatchOutcome, StopReason, handle_exit_with_instruction},
    exit::{ExitAction, ExitSnapshot, ResumeError},
    guest_state::GuestStateRequest,
    registers::GuestRegisters,
    vmcb::Vmcb,
};

fn state(rip: u64, rax: u64) -> (Vmcb, GuestRegisters) {
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
            rip,
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
            rbx: u64::MAX,
            rcx: u64::MAX,
            rdx: u64::MAX,
            r15: 0xfeed,
            ..GuestRegisters::default()
        },
    )
}

fn snapshot(code: u64, rip: u64) -> ExitSnapshot {
    ExitSnapshot {
        code,
        rip,
        info1: 0,
        info2: 0,
        nrip: 0,
    }
}

#[test]
fn shutdown_precedes_undefined_saved_rip_and_instruction_validation() {
    let (mut vmcb, mut frame) = state(0x1000, 1);
    let bytes = *vmcb.bytes();
    let registers = frame;
    assert_eq!(
        handle_exit_with_instruction(snapshot(0x7f, u64::MAX), &mut vmcb, &mut frame, &[]),
        Ok(DispatchOutcome::Stop(StopReason::Exit(
            ExitAction::Shutdown
        )))
    );
    assert_eq!(vmcb.bytes(), &bytes);
    assert_eq!(frame, registers);
}

#[test]
fn exact_cpuid_and_query_resume_without_nrip_evidence() {
    let (mut vmcb, mut frame) = state(0x1000, 1);
    let mut expected_frame = frame;
    expected_frame.rbx = 0;
    expected_frame.rcx = 1 << 31;
    expected_frame.rdx = (1 << 5) | (1 << 6);
    let mut expected_vmcb = *vmcb.bytes();
    expected_vmcb[0x578..0x580].copy_from_slice(&0x1002_u64.to_le_bytes());
    expected_vmcb[0x5f8..0x600].fill(0);
    assert_eq!(
        handle_exit_with_instruction(snapshot(0x72, 0x1000), &mut vmcb, &mut frame, &[0x0f, 0xa2]),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!(frame, expected_frame);
    assert_eq!(vmcb.bytes(), &expected_vmcb);

    let (mut vmcb, mut frame) = state(0x1000, 0);
    let before_frame = frame;
    assert_eq!(
        handle_exit_with_instruction(
            snapshot(0x81, 0x1000),
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x01, 0xd9]
        ),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!((vmcb.guest_rip(), vmcb.guest_rax()), (0x1003, 1));
    assert_eq!(frame, before_frame);
}

#[test]
fn wrong_opcode_prefix_truncation_and_trailing_bytes_preserve_all_state() {
    for (code, bytes) in [
        (0x72, &[][..]),
        (0x72, &[0x0f][..]),
        (0x72, &[0x66, 0x0f, 0xa2][..]),
        (0x72, &[0x0f, 0xa2, 0x90][..]),
        (0x72, &[0x0f, 0x01, 0xd9][..]),
        (0x81, &[0x0f, 0xa2][..]),
        (0x81, &[0x0f, 0x01, 0xc1][..]),
    ] {
        let (mut vmcb, mut frame) = state(0x1000, 0);
        let before_vmcb = *vmcb.bytes();
        let before_frame = frame;
        assert_eq!(
            handle_exit_with_instruction(snapshot(code, 0x1000), &mut vmcb, &mut frame, bytes),
            Err(DispatchError::Resume(
                ResumeError::UnsupportedInstructionBytes
            ))
        );
        assert_eq!(vmcb.bytes(), &before_vmcb);
        assert_eq!(frame, before_frame);
    }
}

#[test]
fn stale_snapshot_and_address_boundary_fail_transactionally() {
    for (rip, snapshot_rip, error) in [
        (0x1000, 0x1001, DispatchError::SnapshotRipMismatch),
        (
            u64::MAX - 1,
            u64::MAX - 1,
            DispatchError::Resume(ResumeError::InvalidInstructionLength),
        ),
        (
            0x7fff_ffff_ffff,
            0x7fff_ffff_ffff,
            DispatchError::Resume(ResumeError::NonCanonicalNrip),
        ),
    ] {
        let (mut vmcb, mut frame) = state(rip, 0);
        let before_vmcb = *vmcb.bytes();
        let before_frame = frame;
        assert_eq!(
            handle_exit_with_instruction(
                snapshot(0x72, snapshot_rip),
                &mut vmcb,
                &mut frame,
                &[0x0f, 0xa2]
            ),
            Err(error)
        );
        assert_eq!(vmcb.bytes(), &before_vmcb);
        assert_eq!(frame, before_frame);
    }
    assert_eq!(
        snapshot(0x72, 0x8000_0000_0000).resume_candidate_from_instruction(&[0x0f, 0xa2]),
        Err(ResumeError::NonCanonicalRip)
    );
}

#[test]
fn terminal_exits_ignore_continuation_bytes_and_do_not_mutate() {
    for (code, rax, reason) in [
        (0x81, 1, StopReason::Requested),
        (0x81, 99, StopReason::UnsupportedHypercall { opcode: 99 }),
        (0x78, 0, StopReason::Exit(ExitAction::StopOnHlt)),
    ] {
        let (mut vmcb, mut frame) = state(0x1000, rax);
        let before_vmcb = *vmcb.bytes();
        let before_frame = frame;
        assert_eq!(
            handle_exit_with_instruction(snapshot(code, 0x1000), &mut vmcb, &mut frame, &[]),
            Ok(DispatchOutcome::Stop(reason))
        );
        assert_eq!(vmcb.bytes(), &before_vmcb);
        assert_eq!(frame, before_frame);
    }
    assert_eq!(
        snapshot(0x78, 0x1000).resume_candidate_from_instruction(&[0x0f, 0xa2]),
        Err(ResumeError::ExitDoesNotPermitCandidate)
    );
}
