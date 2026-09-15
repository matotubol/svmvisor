use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        events::ExternalInterruptError,
        exit::{ExitAction, ResumeError},
        local_apic::{Error, LocalApic},
        vmcb::Vmcb,
        x2apic::{Cr8Error, FIXTURE_APIC_BASE, FixtureApic, handle_fixture_cr8_write},
    },
};

// Inert images only; these tests neither execute VMRUN nor establish exit provenance.
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

fn setup(tpr: u8) -> (FixtureApic, Vmcb, GuestRegisters) {
    let mut controller = LocalApic::admit_enabled();
    controller.set_task_priority(tpr).unwrap();
    let apic = FixtureApic::admit_fixed_bsp(controller, FIXTURE_APIC_BASE).unwrap();
    let mut v = Vmcb::new();
    write(&mut v, 0x70, 0x18);
    write(&mut v, 0x78, u64::MAX); // decode-assist is not admitted or consulted
    write(&mut v, 0x80, u64::MAX);
    write(&mut v, 0xc8, u64::MAX); // nRIP is not admitted or consulted
    write(&mut v, 0x578, 0x1000);
    write(&mut v, 0x570, 0x246);
    write(&mut v, 0x4d0, 1 << 10);
    write(&mut v, 0x410, 1 << 25); // saved CS.L at attribute bit 9
    write(&mut v, 0x60, (1 << 24) | (tpr >> 4) as u64);
    write(&mut v, 0x5f8, 0xaabb_ccdd_1234_5678);
    write(&mut v, 0x5d8, 0xbbcc_ddee_2345_6789);
    let frame = GuestRegisters {
        rcx: 0x11_1111_1111,
        rdx: 0x22_2222_2222,
        rbx: 0x33_3333_3333,
        rbp: 0x55_5555_5555,
        rsi: 0x66_6666_6666,
        rdi: 0x77_7777_7777,
        r8: 0x88_8888_8888,
        r9: 0x99_9999_9999,
        r10: 0xaa_aaaa_aaaa,
        r11: 0xbb_bbbb_bbbb,
        r12: 0xcc_cccc_cccc,
        r13: 0xdd_dddd_dddd,
        r14: 0xee_eeee_eeee,
        r15: 0xff_ffff_ffff,
    };
    (apic, v, frame)
}

fn operand(v: &mut Vmcb, f: &mut GuestRegisters, index: u8, value: u64) {
    match index {
        0 => write(v, 0x5f8, value),
        1 => f.rcx = value,
        2 => f.rdx = value,
        3 => f.rbx = value,
        4 => write(v, 0x5d8, value),
        5 => f.rbp = value,
        6 => f.rsi = value,
        7 => f.rdi = value,
        8 => f.r8 = value,
        9 => f.r9 = value,
        10 => f.r10 = value,
        11 => f.r11 = value,
        12 => f.r12 = value,
        13 => f.r13 = value,
        14 => f.r14 = value,
        15 => f.r15 = value,
        _ => panic!("invalid register"),
    }
}

fn instruction(index: u8, w: bool) -> [u8; 4] {
    [
        0x44 | (index >> 3) | ((w as u8) << 3),
        0x0f,
        0x22,
        0xc0 | (index & 7),
    ]
}

fn refused(a: &mut FixtureApic, v: &mut Vmcb, f: &GuestRegisters, bytes: &[u8]) -> Cr8Error {
    let before = *v.bytes();
    let owner = format!("{a:?}");
    let registers = *f;
    let error = handle_fixture_cr8_write(a, v, f, bytes).unwrap_err();
    assert_eq!(v.bytes(), &before);
    assert_eq!(format!("{a:?}"), owner);
    assert_eq!(*f, registers);
    error
}

#[test]
fn all_registers_and_rex_w_variants_write_full_tpr_without_clobbering_state() {
    for index in 0..16 {
        for w in [false, true] {
            for value in 0..16 {
                let (mut a, mut v, mut f) = setup(0x5b);
                a.queue(0xe1).unwrap();
                operand(&mut v, &mut f, index, value);
                let frame = f;
                let mut expected = *v.bytes();
                expected[0x60] = value as u8;
                expected[0x578..0x580].copy_from_slice(&0x1004u64.to_le_bytes());
                handle_fixture_cr8_write(&mut a, &mut v, &f, &instruction(index, w)).unwrap();
                assert_eq!(f, frame);
                assert_eq!(v.bytes(), &expected);
                assert_eq!(a.controller().task_priority(), (value as u8) << 4);
                assert_eq!(a.controller().processor_priority(), (value as u8) << 4);
                assert!(a.controller().pending(0xe1));
            }
        }
    }
}

#[test]
fn each_reserved_operand_bit_is_refused_from_every_source_without_truncation() {
    for index in 0..16 {
        for bit in 4..64 {
            let (mut a, mut v, mut f) = setup(0);
            let value = (1u64 << bit) | 5;
            operand(&mut v, &mut f, index, value);
            assert_eq!(
                refused(&mut a, &mut v, &f, &instruction(index, false)),
                Cr8Error::InvalidOperand { value }
            );
        }
    }
}

#[test]
fn other_encodings_and_exits_remain_unsupported() {
    let (mut a, mut v, mut f) = setup(0);
    operand(&mut v, &mut f, 0, 5);
    for rex in 0..=255 {
        if matches!(rex, 0x44 | 0x45 | 0x4c | 0x4d) {
            continue;
        }
        assert_eq!(
            refused(&mut a, &mut v, &f, &[rex, 0x0f, 0x22, 0xc0]),
            Cr8Error::Continuation(ResumeError::UnsupportedInstructionBytes)
        );
    }
    for modrm in 0..=255 {
        if (0xc0..=0xc7).contains(&modrm) {
            continue;
        }
        assert_eq!(
            refused(&mut a, &mut v, &f, &[0x44, 0x0f, 0x22, modrm]),
            Cr8Error::Continuation(ResumeError::UnsupportedInstructionBytes)
        );
    }
    for bytes in [
        &[][..],
        &[0x0f, 0x22, 0xc0],
        &[0x44, 0x0f, 0x20, 0xc0],
        &[0x44, 0x0f, 0x22],
        &[0x44, 0x0f, 0x22, 0xc0, 0x90],
        &[0xf0, 0x0f, 0x22, 0xc0],
        &[0x66, 0x44, 0x0f, 0x22, 0xc0],
    ] {
        assert_eq!(
            refused(&mut a, &mut v, &f, bytes),
            Cr8Error::Continuation(ResumeError::UnsupportedInstructionBytes)
        );
    }
    for code in [0x08, 0x10, 0x17, 0x19, 0x4d, 0x65, 0x7c, 0x400, u64::MAX] {
        write(&mut v, 0x70, code);
        assert_eq!(
            refused(&mut a, &mut v, &f, &instruction(0, false)),
            Cr8Error::Continuation(ResumeError::ExitDoesNotPermitCandidate)
        );
    }
    write(&mut v, 0x70, 0x18);
    assert_eq!(
        v.exit_snapshot().action(),
        ExitAction::Unsupported { code: 0x18 }
    );
    assert!(
        v.exit_snapshot()
            .resume_candidate_from_instruction(&instruction(0, false))
            .is_err()
    );
}

#[test]
fn non_64_bit_code_and_nonzero_cpl_are_refused() {
    for (efer, cs) in [(0, 0), (0, 1 << 25), (1 << 10, 0)] {
        let (mut a, mut v, mut f) = setup(0);
        operand(&mut v, &mut f, 0, 0);
        write(&mut v, 0x4d0, efer);
        write(&mut v, 0x410, cs);
        assert_eq!(
            refused(&mut a, &mut v, &f, &instruction(0, false)),
            Cr8Error::UnsupportedGuestMode
        );
    }
    for cpl in 1..=3 {
        let (mut a, mut v, mut f) = setup(0);
        operand(&mut v, &mut f, 0, 0);
        write(&mut v, 0x4c8, cpl << 24);
        assert_eq!(
            refused(&mut a, &mut v, &f, &instruction(0, false)),
            Cr8Error::UnsupportedPrivilege
        );
    }
}

#[test]
fn competing_events_unsupported_controls_and_tpr_mismatch_preserve_all_state() {
    for (offset, value, error) in [
        (
            0x60,
            0,
            Cr8Error::PendingState(ExternalInterruptError::UnsupportedControl { control: 0 }),
        ),
        (
            0xa8,
            1 << 31,
            Cr8Error::PendingState(ExternalInterruptError::PendingInjection),
        ),
        (
            0x88,
            1 << 31,
            Cr8Error::PendingState(ExternalInterruptError::NestedDeliveryUnsupported),
        ),
        (
            0x60,
            (1 << 24) | (1 << 8),
            Cr8Error::PendingState(ExternalInterruptError::PendingVirtualInterrupt),
        ),
        (
            0x60,
            (1 << 24) | 1,
            Cr8Error::Controller(Error::TaskPriorityMismatch),
        ),
    ] {
        let (mut a, mut v, mut f) = setup(0);
        operand(&mut v, &mut f, 0, 5);
        write(&mut v, offset, value);
        assert_eq!(refused(&mut a, &mut v, &f, &instruction(0, false)), error);
    }
    for (offset, value) in [(0x60, 1 << 20), (0x60, 1 << 25), (0x60, 1 << 31), (0x90, 3)] {
        let (mut a, mut v, mut f) = setup(0);
        operand(&mut v, &mut f, 0, 5);
        write(&mut v, offset, value);
        assert!(matches!(
            refused(&mut a, &mut v, &f, &instruction(0, false)),
            Cr8Error::PendingState(_)
        ));
    }
    let (mut a, mut v, mut f) = setup(0);
    operand(&mut v, &mut f, 0, 5);
    a.queue(0x50).unwrap();
    a.arm(&mut v).unwrap();
    // A cleared hardware V_IRQ does not retire the owner's armed record.
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8));
    assert_eq!(
        refused(&mut a, &mut v, &f, &instruction(0, false)),
        Cr8Error::Controller(Error::Armed)
    );
}

#[test]
fn continuation_is_required_only_for_success_and_refusal_never_advances() {
    for (rip, error) in [
        (0x0000_8000_0000_0000, ResumeError::NonCanonicalRip),
        (0x0000_7fff_ffff_fffc, ResumeError::NonCanonicalNrip),
        (u64::MAX - 3, ResumeError::InvalidInstructionLength),
    ] {
        let (mut a, mut v, mut f) = setup(0);
        operand(&mut v, &mut f, 0, 5);
        write(&mut v, 0x578, rip);
        assert_eq!(
            refused(&mut a, &mut v, &f, &instruction(0, false)),
            Cr8Error::Continuation(error)
        );
    }
    let (mut a, mut v, mut f) = setup(0);
    operand(&mut v, &mut f, 0, 16);
    write(&mut v, 0x578, 0x0000_7fff_ffff_fffc);
    assert_eq!(
        refused(&mut a, &mut v, &f, &instruction(0, false)),
        Cr8Error::InvalidOperand { value: 16 }
    );
}
