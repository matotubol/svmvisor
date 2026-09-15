use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        dispatch::NativeMsrOutcome,
        ipi::{
            NativeIcr, NativeIcrError, NativeStartupCommand as Command,
            NativeStartupEffect as Effect, NativeStartupMailbox, NativeStartupState as State,
            NativeStartupTarget, handle_native_x2apic_startup_access,
        },
        vmcb::Vmcb,
    },
};

fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    // Test emulation of hardware-saved fields; no privileged instructions.
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn get(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}
fn stopped(index: u32, value: u64, write: bool) -> (Vmcb, GuestRegisters) {
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x70, 0x7c);
    put(&mut vmcb, 0x78, u64::from(write));
    put(&mut vmcb, 0x410, 0x029b_0008);
    put(&mut vmcb, 0x4d0, 0x1500);
    put(&mut vmcb, 0x558, 0x8000_0001);
    put(&mut vmcb, 0x578, 0x1234_5000);
    put(&mut vmcb, 0x570, 0x1_0002);
    put(&mut vmcb, 0x68, 1);
    put(&mut vmcb, 0x5f8, value as u32 as u64);
    put(&mut vmcb, 0x60, 0);
    (
        vmcb,
        GuestRegisters {
            rcx: u64::from(index),
            rdx: value >> 32,
            rbx: 0xdead_beef,
            ..GuestRegisters::default()
        },
    )
}

#[test]
fn native_running_init_preserves_retained_state_and_starts_real16_repeatedly() {
    let (mut vmcb, mut frame) = stopped(0x830, 0, true);
    put(&mut vmcb, 0x558, 0xe005_003f);
    for (offset, value) in [
        (0x560, 0xdead_2400),
        (0x568, 0xabcd_0ff0),
        (0x600, 0x1234),
        (0x608, 0x5678),
        (0x610, 0x9abc),
        (0x618, 0xdef0),
        (0x620, 0x1122),
        (0x628, 0x3344),
        (0x630, 0x5566),
        (0x638, 0x7788),
        (0x668, 0x0007_0406),
    ] {
        put(&mut vmcb, offset, value);
    }
    let before = *vmcb.bytes();
    let mut state = State::Running;
    let mut target = NativeStartupTarget {
        vmcb: &mut vmcb,
        frame: &mut frame,
        state: &mut state,
        signature: 0xa20f12,
    };
    for vector in [0x08, 0xff, 0] {
        put(target.vmcb, 0x560, 0x0001_0400);
        put(target.vmcb, 0x568, 0xffff_0ff1);
        assert_eq!(target.apply(Command::Init), Ok(Effect::Init));
        assert_eq!(*target.state, State::AwaitSipi);
        assert_eq!(get(target.vmcb, 0x558), 0x6000_0010);
        assert_eq!(get(target.vmcb, 0x548), 0);
        assert_eq!(get(target.vmcb, 0x550), 0);
        assert_eq!(get(target.vmcb, 0x640), 0);
        assert_eq!(get(target.vmcb, 0x4d0), 0x1000);
        assert_eq!(get(target.vmcb, 0x570), 2);
        assert_eq!(get(target.vmcb, 0x578), 0xfff0);
        assert_eq!(get(target.vmcb, 0x60), 0);
        assert_eq!(
            *target.frame,
            GuestRegisters {
                rdx: 0xa20f12,
                ..GuestRegisters::default()
            }
        );
        assert_eq!(get(target.vmcb, 0x560), 0x400);
        assert_eq!(get(target.vmcb, 0x568), 0xffff_0ff0);
        for (start, end) in [(0x600, 0x640), (0x668, 0x670)] {
            assert_eq!(&target.vmcb.bytes()[start..end], &before[start..end]);
        }
        // INIT segment attributes are 16-bit real mode, not a long64 capture.
        assert_eq!(
            &target.vmcb.bytes()[0x410..0x414],
            &[0x00, 0xf0, 0x9a, 0x00]
        );
        assert_eq!(target.apply(Command::Sipi(vector)), Ok(Effect::Started));
        assert_eq!(*target.state, State::Running);
        assert_eq!(get(target.vmcb, 0x418), u64::from(vector) << 12);
        assert_eq!(
            &target.vmcb.bytes()[0x410..0x412],
            &(u16::from(vector) << 8).to_le_bytes()
        );
        assert_eq!(get(target.vmcb, 0x578), 0);
        // A running guest may change debug state again; duplicate SIPI is a
        // notification, not another INIT and must not erase those changes.
        put(target.vmcb, 0x560, 0x0001_0400);
        put(target.vmcb, 0x568, 0xffff_0ff1);
        let running = *target.vmcb.bytes();
        assert_eq!(
            target.apply(Command::Sipi(vector.wrapping_add(1))),
            Ok(Effect::Ignored)
        );
        assert_eq!(*target.vmcb.bytes(), running);
    }
}

#[test]
fn target_pending_refusal_does_not_erase_cpu_state_or_mailbox_head() {
    let (mut vmcb, mut frame) = stopped(0x830, 0, true);
    put(&mut vmcb, 0xa8, 1 << 31);
    put(&mut vmcb, 0x560, 0x0001_0400);
    put(&mut vmcb, 0x568, 0xffff_0ff1);
    let original = *vmcb.bytes();
    let original_frame = frame;
    let mut state = State::Running;
    let mailbox = NativeStartupMailbox::new(19);
    mailbox.mark_running();
    mailbox.publish(Command::Init).unwrap();
    let mut target = NativeStartupTarget {
        vmcb: &mut vmcb,
        frame: &mut frame,
        state: &mut state,
        signature: 1,
    };
    assert!(target.apply(mailbox.peek().unwrap()).is_err());
    assert_eq!(*target.vmcb.bytes(), original);
    assert_eq!(*target.frame, original_frame);
    assert_eq!(*target.state, State::Running);
    assert_eq!(mailbox.peek(), Some(Command::Init));
}

#[test]
fn mailbox_readiness_fifo_capacity_and_completion_head_are_checked() {
    assert_eq!(core::mem::size_of::<NativeStartupMailbox>(), 64);
    let mailbox = NativeStartupMailbox::new(19);
    assert_eq!(
        mailbox.publish(Command::Init),
        Err(NativeIcrError::MailboxNotReady)
    );
    assert_eq!(mailbox.peek(), None);
    mailbox.mark_running();
    let commands = [
        Command::Init,
        Command::Sipi(8),
        Command::Init,
        Command::Sipi(9),
    ];
    for command in commands {
        mailbox.publish(command).unwrap();
    }
    assert_eq!(
        mailbox.publish(Command::Sipi(10)),
        Err(NativeIcrError::MailboxBusy)
    );
    assert_eq!(
        mailbox.complete(Command::Sipi(8)),
        Err(NativeIcrError::MailboxMismatch)
    );
    for command in commands {
        assert_eq!(mailbox.peek(), Some(command));
        mailbox.complete(command).unwrap();
    }
    assert_eq!(mailbox.peek(), None);
}

#[test]
fn source_startup_publishes_before_kick_and_completes_with_guest_readback() {
    let mut owner = NativeIcr::admit(7, &[7, 19]).unwrap();
    owner.enable_startup(0).unwrap();
    let mailboxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
    for mailbox in &mailboxes {
        mailbox.mark_running();
    }
    let value = (19u64 << 32) | 0xc500;
    let (mut vmcb, mut frame) = stopped(0x830, value, true);
    let original_frame = frame;
    assert_eq!(
        handle_native_x2apic_startup_access(
            &mut owner,
            0xfee0_0d00,
            &mailboxes,
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x30],
            |_, _| panic!("physical INIT escaped"),
            |id| {
                assert_eq!(id, 19);
                assert_eq!(mailboxes[1].peek(), Some(Command::Init));
            }
        ),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!(vmcb.guest_rip(), 0x1234_5002);
    assert_eq!(frame, original_frame);
    let (mut vmcb, mut frame) = stopped(0x830, 0, false);
    assert_eq!(
        handle_native_x2apic_startup_access(
            &mut owner,
            0xfee0_0d00,
            &mailboxes,
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x32],
            |_, _| panic!(),
            |_| panic!()
        ),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!(vmcb.guest_rax(), value as u32 as u64);
    assert_eq!(frame.rdx, value >> 32);
}

#[test]
fn refused_source_has_no_publication_kick_completion_or_shadow_change() {
    for case in 0..7 {
        let mut owner = NativeIcr::admit(7, &[7, 19]).unwrap();
        owner.enable_startup(0x55).unwrap();
        let mailboxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
        mailboxes[0].mark_running();
        if case != 0 {
            mailboxes[1].mark_running();
        }
        let value = match case {
            1 => (7u64 << 32) | 0x500,
            2 => (19u64 << 32) | 0x8501,
            3 => (19u64 << 32) | 0x501,
            4 => (19u64 << 32) | (1 << 11) | 0x500,
            _ => (19u64 << 32) | 0x500,
        };
        let (mut vmcb, mut frame) = stopped(0x830, value, true);
        if case == 5 {
            put(&mut vmcb, 0x578, 0x7fff_ffff_ffff);
        }
        if case == 6 {
            put(&mut vmcb, 0xa8, 1 << 31);
        }
        let before = *vmcb.bytes();
        let old_frame = frame;
        assert!(
            handle_native_x2apic_startup_access(
                &mut owner,
                0xfee0_0d00,
                &mailboxes,
                &mut vmcb,
                &mut frame,
                &[0x0f, 0x30],
                |_, _| panic!(),
                |_| panic!()
            )
            .is_err()
        );
        assert_eq!(*vmcb.bytes(), before);
        assert_eq!(frame, old_frame);
        assert_eq!(mailboxes[1].peek(), None);
        let (mut vmcb, mut frame) = stopped(0x830, 0, false);
        handle_native_x2apic_startup_access(
            &mut owner,
            0xfee0_0d00,
            &mailboxes,
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x32],
            |_, _| panic!(),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(vmcb.guest_rax(), 0x55);
    }
}

#[test]
fn init_icr_readback_zero_does_not_take_ownership_of_native_svr_or_tpr() {
    let mut owner = NativeIcr::admit(7, &[7, 19]).unwrap();
    owner.enable_startup(0x1234).unwrap();
    owner.reset_after_init().unwrap();
    let mailboxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
    for (index, expected) in [(0x830, 0)] {
        let (mut vmcb, mut frame) = stopped(index, 0, false);
        handle_native_x2apic_startup_access(
            &mut owner,
            0xfee0_0d00,
            &mailboxes,
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x32],
            |_, _| panic!(),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(vmcb.guest_rax(), expected);
    }
    for index in [0x808, 0x80b, 0x80f, 0x817, 0x827] {
        let (mut vmcb, mut frame) = stopped(index, 0xef, true);
        let before = *vmcb.bytes();
        assert_eq!(
            handle_native_x2apic_startup_access(
                &mut owner,
                0xfee0_0d00,
                &mailboxes,
                &mut vmcb,
                &mut frame,
                &[0x0f, 0x30],
                |_, _| panic!(),
                |_| panic!(),
            ),
            Err(NativeIcrError::UnsupportedMsr)
        );
        assert_eq!(*vmcb.bytes(), before);
    }
}

#[test]
fn all_fixed_vectors_including_f1_remain_native_with_guest_priority_retained() {
    let mut owner = NativeIcr::admit(7, &[7, 19]).unwrap();
    owner.enable_startup(0).unwrap();
    let mailboxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
    for vector in [0x20, 0xef, 0xf0, 0xf1, 0xff] {
        let value = (19u64 << 32) | vector;
        let (mut vmcb, mut frame) = stopped(0x830, value, true);
        put(&mut vmcb, 0x60, 15); // Native CR8 class, not virtual masking.
        handle_native_x2apic_startup_access(
            &mut owner,
            0xfee0_0d00,
            &mailboxes,
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x30],
            |index, sent| assert_eq!((index, sent), (0x830, value)),
            |_| panic!("ordinary fixed IPI used startup notification"),
        )
        .unwrap();
        assert_eq!(vmcb.virtual_interrupt_control(), 15);
        assert_eq!(mailboxes[1].peek(), None);
    }
}

#[test]
fn init_notification_controls_preserve_native_irq_priority_and_reject_virtual_state() {
    use svmvisor_hypervisor::svm::vmcb::EventIntercept;
    let (mut vmcb, _) = stopped(0x830, 0, true);
    put(&mut vmcb, 0x60, 15);
    let before = *vmcb.bytes();
    vmcb.enable_native_startup_interrupts().unwrap();
    assert!(vmcb.event_intercept(EventIntercept::Init));
    assert!(!vmcb.event_intercept(EventIntercept::PhysicalInterrupt));
    assert_eq!(vmcb.virtual_interrupt_control(), 15);
    assert_eq!(&vmcb.bytes()[..4], &before[..4]); // CR8 intercepts unchanged.
    assert_eq!(&vmcb.bytes()[0x400..], &before[0x400..]);
    for (offset, value) in [
        (0x60, 1 << 24),
        (0x60, 1 << 8),
        (0xa8, 1 << 31),
        (0x88, 1 << 31),
    ] {
        let (mut vmcb, _) = stopped(0x830, 0, true);
        put(&mut vmcb, offset, value);
        let before = *vmcb.bytes();
        assert!(vmcb.enable_native_startup_interrupts().is_err());
        assert_eq!(*vmcb.bytes(), before);
    }
}

#[test]
fn concurrent_publishers_and_destination_preserve_every_accepted_fifo_entry() {
    use std::{sync::Barrier, thread};
    // Each producer has a disjoint vector range and an ordered stream. The
    // destination's peek/complete pair races both appends; full-queue refusals
    // retry as new attempts, so all 4096 accepted requests must appear once.
    for _ in 0..16 {
        let mailbox = NativeStartupMailbox::new(19);
        mailbox.mark_running();
        let barrier = Barrier::new(3);
        thread::scope(|scope| {
            for producer in 0u8..2 {
                let mailbox = &mailbox;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    for sequence in 0u8..128 {
                        let command = Command::Sipi((producer << 7) | sequence);
                        let mut accepted = false;
                        for _ in 0..1_000_000 {
                            match mailbox.publish(command) {
                                Ok(()) => {
                                    accepted = true;
                                    break;
                                }
                                Err(NativeIcrError::MailboxBusy) => thread::yield_now(),
                                error => panic!("unexpected publication: {error:?}"),
                            }
                        }
                        assert!(accepted, "bounded host stress progress timeout");
                    }
                });
            }
            barrier.wait();
            let mut expected = [0u16; 2];
            let mut count = 0;
            for _ in 0..2_000_000 {
                if let Some(command) = mailbox.peek() {
                    let Command::Sipi(vector) = command else {
                        panic!("unexpected INIT")
                    };
                    let producer = usize::from(vector >> 7);
                    assert_eq!(u16::from(vector & 0x7f), expected[producer]);
                    mailbox.complete(command).unwrap();
                    expected[producer] += 1;
                    count += 1;
                    if count == 256 {
                        break;
                    }
                } else {
                    thread::yield_now();
                }
            }
            assert_eq!(expected, [128, 128]);
            assert_eq!(mailbox.peek(), None);
        });
    }
}

#[test]
fn x2apic_init_deassert_completes_without_target_action_and_checks_pending_state() {
    for pending in [false, true] {
        let mut owner = NativeIcr::admit(7, &[7, 19]).unwrap();
        owner.enable_startup(0x55).unwrap();
        let boxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
        for b in &boxes { b.mark_running(); }
        boxes[1].publish(Command::Init).unwrap();
        let value = (19u64 << 32) | 0x8500;
        let (mut vmcb, mut frame) = stopped(0x830, value, true);
        if pending { put(&mut vmcb, 0xa8, 1 << 31); }
        let before = *vmcb.bytes();
        let before_frame = frame;
        let result = handle_native_x2apic_startup_access(
            &mut owner, 0xfee0_0d00, &boxes, &mut vmcb, &mut frame,
            &[0x0f, 0x30], |_, _| panic!("deassert physical write"),
            |_| panic!("deassert kick"),
        );
        if pending {
            assert!(result.is_err());
            assert_eq!(*vmcb.bytes(), before);
        } else {
            assert_eq!(result, Ok(NativeMsrOutcome::Completed));
            assert_eq!(vmcb.guest_rip(), 0x1234_5002);
        }
        assert_eq!(frame, before_frame);
        assert_eq!(boxes[1].peek(), Some(Command::Init));
        boxes[1].complete(Command::Init).unwrap();
        assert_eq!(boxes[1].peek(), None);
        let (mut vmcb, mut frame) = stopped(0x830, 0, false);
        handle_native_x2apic_startup_access(
            &mut owner, 0xfee0_0d00, &boxes, &mut vmcb, &mut frame,
            &[0x0f, 0x32], |_, _| panic!(), |_| panic!(),
        ).unwrap();
        assert_eq!(vmcb.guest_rax(), if pending { 0x55 } else { 0x8500 });
        assert_eq!(frame.rdx, if pending { 0 } else { 19 });
    }
}
