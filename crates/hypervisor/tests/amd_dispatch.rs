//! Transactional stopped-state integration; these tests do not execute a CPU.
use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    arch::x86_64::xstate::{XstateCapabilities, XstateLayout},
    guest::state::GuestStateRequest,
    memory::address::{AddressPolicy, EncryptionState},
    svm::cpu_model::{
        AmdCpuModel, CpuIdentity, CpuModelError, GuestCpuState, HostCacheEvidence, HostCpuEvidence,
        RuntimeCpuContract,
    },
    svm::dispatch::{DispatchError, DispatchOutcome, StopReason, handle_exit_with_cpu_model},
    svm::exit::{ExitAction, ExitSnapshot, ResumeError},
    svm::vmcb::Vmcb,
};

const CPUID: &[u8] = &[0x0f, 0xa2];
const XSETBV: &[u8] = &[0x0f, 0x01, 0xd1];

fn layout() -> XstateLayout {
    XstateLayout::detect(XstateCapabilities {
        leaf1_ecx: (1 << 26) | (1 << 28),
        leaf1_edx: 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26),
        supported_xcr0: 7,
        enabled_size: 576,
        max_size: 832,
        avx_size: 256,
        avx_offset: 576,
        avx_flags: 0,
    })
    .unwrap()
}

fn model() -> AmdCpuModel {
    let identity = CpuIdentity::from_leaves(
        [0x0d, 0x6874_7541, 0x444d_4163, 0x6974_6e65],
        Some([0x0080_0f10, 0, 0, 0]),
        [0x8000_001e, 0x6874_7541, 0x444d_4163, 0x6974_6e65],
        Some([0x0080_0f10, 0, 0, 0]),
        Some([[0; 4]; 3]),
    )
    .unwrap();
    AmdCpuModel::admit(
        HostCpuEvidence {
            vendor: *b"AuthenticAMD",
            max_basic: 0x0d,
            max_extended: 0x8000_001e,
            leaf1_ecx: u32::MAX,
            leaf1_edx: u32::MAX,
            leaf7_ebx: u32::MAX,
            leaf7_ecx: u32::MAX,
            leaf7_edx: u32::MAX,
            extended8_ebx: u32::MAX,
            extended21_eax: 0,
            extended21_ebx: 0,
            extended1_ecx: u32::MAX,
            extended1_edx: u32::MAX,
            address_sizes: 48 | (48 << 8),
            clflush_bytes: 64,
            caches: HostCacheEvidence::default(),
        },
        RuntimeCpuContract {
            identity,
            xstate: layout(),
            physical_address_bits: 48,
            tsc: false,
            rdtscp: false,
            nx: false,
        },
    )
    .unwrap()
}

fn stopped(rax: u64, rcx: u64, rip: u64) -> (Vmcb, GuestRegisters, GuestCpuState) {
    let policy =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let cr4 = layout().guest_cr4();
    let mut vmcb = Vmcb::new();
    vmcb.set_synthetic_state(
        &GuestStateRequest {
            rip,
            rsp: 0x8000,
            rflags: 0x8d7,
            cr0: 0x8001_0033,
            cr3: 0x9000,
            cr4,
            efer: 0x1500,
            rax,
        }
        .validate_continuation_with_xstate(&policy, layout())
        .unwrap(),
    );
    let frame = GuestRegisters {
        rcx,
        rdx: u64::MAX,
        rbx: u64::MAX,
        rbp: 0x1111_2222_3333_4444,
        rsi: 0x2222_3333_4444_5555,
        rdi: 0x3333_4444_5555_6666,
        r8: 0x4444_5555_6666_7777,
        r9: 0x5555_6666_7777_8888,
        r10: 0x6666_7777_8888_9999,
        r11: 0x7777_8888_9999_aaaa,
        r12: 0x8888_9999_aaaa_bbbb,
        r13: 0x9999_aaaa_bbbb_cccc,
        r14: 0xaaaa_bbbb_cccc_dddd,
        r15: 0xbbbb_cccc_dddd_eeee,
    };
    (vmcb, frame, GuestCpuState { vcpu_id: 1, cr4, xcr0: 7 })
}

fn snapshot(code: u64, rip: u64) -> ExitSnapshot {
    // Byte-provenance continuation must ignore undefined nRIP/EXITINFO.
    ExitSnapshot { code, rip, info1: u64::MAX, info2: u64::MAX, nrip: u64::MAX }
}

#[test]
fn cpuid_uses_low_input_dwords_and_zero_extends_only_four_outputs() {
    for (leaf, subleaf, result) in [
        (0, 0, [0x0du32, 0x6874_7541, 0x444d_4163, 0x6974_6e65]),
        (0x0d, 2, [256, 576, 0, 0]),
        (0x0b, 1, [1, 2, 0x201, 1]),
    ] {
        let (mut vmcb, mut frame, cpu) =
            stopped(0xdead_beef_0000_0000 | leaf, 0xcafe_babe_0000_0000 | subleaf, 0x1000);
        let mut expected_vmcb = *vmcb.bytes();
        expected_vmcb[0x578..0x580].copy_from_slice(&0x1002u64.to_le_bytes());
        expected_vmcb[0x5f8..0x600].copy_from_slice(&u64::from(result[0]).to_le_bytes());
        let mut expected_frame = frame;
        expected_frame.rbx = u64::from(result[1]);
        expected_frame.rcx = u64::from(result[2]);
        expected_frame.rdx = u64::from(result[3]);
        assert_eq!(
            handle_exit_with_cpu_model(
                snapshot(0x72, 0x1000),
                &mut vmcb,
                &mut frame,
                CPUID,
                &model(),
                cpu,
            ),
            Ok(DispatchOutcome::ResumePrepared)
        );
        // Includes RFLAGS, RSP, CR4, pending events and all unrelated bytes.
        assert_eq!(vmcb.bytes(), &expected_vmcb);
        assert_eq!(frame, expected_frame);
        assert_eq!(
            handle_exit_with_cpu_model(
                snapshot(0x72, 0x1000),
                &mut vmcb,
                &mut frame,
                CPUID,
                &model(),
                cpu,
            ),
            Err(DispatchError::SnapshotRipMismatch)
        );
        assert_eq!(vmcb.bytes(), &expected_vmcb);
        assert_eq!(frame, expected_frame);
    }
}

#[test]
fn mismatched_saved_cr4_and_invalid_cpu_contract_leave_all_state_unchanged() {
    for (id, cr4_xor, mask, expected) in [
        (1, 1 << 18, 7, DispatchError::CpuStateMismatch),
        (2, 0, 7, DispatchError::CpuModel(CpuModelError::InvalidVcpuId)),
        (u32::MAX, 0, 7, DispatchError::CpuModel(CpuModelError::InvalidVcpuId)),
        (1, 0, 0, DispatchError::CpuModel(CpuModelError::InvalidGuestXstate)),
        (1, 0, 5, DispatchError::CpuModel(CpuModelError::InvalidGuestXstate)),
        (1, 0, 0x1_0000_0007, DispatchError::CpuModel(CpuModelError::InvalidGuestXstate)),
    ] {
        let (mut vmcb, mut frame, mut cpu) = stopped(0x0d, 0, 0x1000);
        cpu.vcpu_id = id;
        cpu.cr4 ^= cr4_xor;
        cpu.xcr0 = mask;
        let before_vmcb = *vmcb.bytes();
        let before_frame = frame;
        assert_eq!(
            handle_exit_with_cpu_model(
                snapshot(0x72, 0x1000),
                &mut vmcb,
                &mut frame,
                CPUID,
                &model(),
                cpu,
            ),
            Err(expected)
        );
        assert_eq!(vmcb.bytes(), &before_vmcb);
        assert_eq!(frame, before_frame);
    }
}

#[test]
fn invalid_instruction_and_noncanonical_continuation_are_transactional() {
    for instruction in
        [&[][..], &[0x0f][..], &[0x90][..], &[0x66, 0x0f, 0xa2][..], &[0x0f, 0xa2, 0x90][..]]
    {
        let (mut vmcb, mut frame, cpu) = stopped(0, 0, 0x1000);
        let before_vmcb = *vmcb.bytes();
        let before_frame = frame;
        assert_eq!(
            handle_exit_with_cpu_model(
                snapshot(0x72, 0x1000),
                &mut vmcb,
                &mut frame,
                instruction,
                &model(),
                cpu,
            ),
            Err(DispatchError::Resume(ResumeError::UnsupportedInstructionBytes))
        );
        assert_eq!(vmcb.bytes(), &before_vmcb);
        assert_eq!(frame, before_frame);
    }
    let rip = 0x0000_7fff_ffff_ffff;
    let (mut vmcb, mut frame, cpu) = stopped(0, 0, rip);
    let before_vmcb = *vmcb.bytes();
    let before_frame = frame;
    assert_eq!(
        handle_exit_with_cpu_model(
            snapshot(0x72, rip),
            &mut vmcb,
            &mut frame,
            CPUID,
            &model(),
            cpu,
        ),
        Err(DispatchError::Resume(ResumeError::NonCanonicalNrip))
    );
    assert_eq!(vmcb.bytes(), &before_vmcb);
    assert_eq!(frame, before_frame);
}

#[test]
fn xsetbv_is_terminal_until_the_separate_xstate_owner_commits_it() {
    let (mut vmcb, mut frame, cpu) = stopped(3, 0, 0x1000);
    let before_vmcb = *vmcb.bytes();
    let before_frame = frame;
    assert_eq!(
        handle_exit_with_cpu_model(
            snapshot(0x8d, 0x1000),
            &mut vmcb,
            &mut frame,
            XSETBV,
            &model(),
            cpu,
        ),
        Ok(DispatchOutcome::Stop(StopReason::Exit(ExitAction::Unsupported { code: 0x8d })))
    );
    assert_eq!(vmcb.bytes(), &before_vmcb);
    assert_eq!(frame, before_frame);
}

#[test]
fn shutdown_never_interprets_undefined_guest_state_or_requests_continuation() {
    let (mut vmcb, mut frame, mut cpu) = stopped(0, 0, 0x1000);
    cpu.cr4 ^= 1 << 18;
    cpu.vcpu_id = u32::MAX;
    cpu.xcr0 = 0;
    let before_vmcb = *vmcb.bytes();
    let before_frame = frame;
    assert_eq!(
        handle_exit_with_cpu_model(
            snapshot(0x7f, 0x0000_8000_0000_0000),
            &mut vmcb,
            &mut frame,
            &[],
            &model(),
            cpu,
        ),
        Ok(DispatchOutcome::Stop(StopReason::Exit(ExitAction::Shutdown)))
    );
    assert_eq!(vmcb.bytes(), &before_vmcb);
    assert_eq!(frame, before_frame);
}

#[test]
fn xsetbv_continuation_requires_exact_exit_bytes_and_canonical_checked_add() {
    let continuation = snapshot(0x8d, 0x1000).xsetbv_continuation(XSETBV).unwrap();
    assert_eq!((continuation.address(), continuation.instruction_bytes()), (0x1003, 3));
    for code in [0x72, 0x7c, 0x8c, 0x1_0000_008d, u64::MAX] {
        assert_eq!(
            snapshot(code, 0x1000).xsetbv_continuation(XSETBV),
            Err(ResumeError::ExitDoesNotPermitCandidate)
        );
    }
    for instruction in [
        &[][..],
        &[0x0f, 0x01][..],
        CPUID,
        &[0x0f, 0x01, 0xd0][..],
        &[0x66, 0x0f, 0x01, 0xd1][..],
        &[0x0f, 0x01, 0xd1, 0x90][..],
    ] {
        assert_eq!(
            snapshot(0x8d, 0x1000).xsetbv_continuation(instruction),
            Err(ResumeError::UnsupportedInstructionBytes)
        );
    }
    for (rip, error) in [
        (0x0000_8000_0000_0000, ResumeError::NonCanonicalRip),
        (0xffff_7fff_ffff_ffff, ResumeError::NonCanonicalRip),
        (0x0000_7fff_ffff_fffd, ResumeError::NonCanonicalNrip),
        (u64::MAX - 1, ResumeError::InvalidInstructionLength),
    ] {
        assert_eq!(snapshot(0x8d, rip).xsetbv_continuation(XSETBV), Err(error));
    }
    for rip in [0x0000_7fff_ffff_fffc, 0xffff_8000_0000_0000, u64::MAX - 3] {
        assert_eq!(snapshot(0x8d, rip).xsetbv_continuation(XSETBV).unwrap().address(), rip + 3);
    }
}
