use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    memory::address::{AddressPolicy, EncryptionState},
    svm::{
        x2avic::{
            NATIVE_CONTROL, NativeX2AvicProfile, X2AvicCapabilities,
            startup::{
                NativeIcr, NativeIcrError, NativeStartupCommand as Command,
                NativeStartupEffect as Effect, NativeStartupMailbox, NativeStartupState as State,
                NativeStartupTarget,
            },
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
/// The armed x2AVIC profile that CPU startup commits require.
fn x2avic(vmcb: &mut Vmcb) -> NativeX2AvicProfile {
    let policy = AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let caps = X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18)).unwrap();
    let profile = NativeX2AvicProfile::new(caps, 0x2000, 0x3000, 37, &policy).unwrap();
    put(vmcb, 0x90, 1); // NP_ENABLE, as native preparation leaves it.
    vmcb.enable_native_x2avic(&profile).unwrap();
    profile
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
    let profile = x2avic(&mut vmcb);
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
        assert_eq!(target.apply_x2avic(Command::Init, &profile), Ok(Effect::Init));
        assert_eq!(*target.state, State::AwaitSipi);
        assert_eq!(get(target.vmcb, 0x558), 0x6000_0010);
        assert_eq!(get(target.vmcb, 0x548), 0);
        assert_eq!(get(target.vmcb, 0x550), 0);
        assert_eq!(get(target.vmcb, 0x640), 0);
        assert_eq!(get(target.vmcb, 0x4d0), 0x1000);
        assert_eq!(get(target.vmcb, 0x570), 2);
        assert_eq!(get(target.vmcb, 0x578), 0xfff0);
        // V_TPR resets to 0; the x2AVIC controls stay.
        assert_eq!(get(target.vmcb, 0x60), NATIVE_CONTROL);
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
        assert_eq!(target.apply_x2avic(Command::Sipi(vector), &profile), Ok(Effect::Started));
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
            target.apply_x2avic(Command::Sipi(vector.wrapping_add(1)), &profile),
            Ok(Effect::Ignored)
        );
        assert_eq!(*target.vmcb.bytes(), running);
    }
}

#[test]
fn target_pending_refusal_does_not_erase_cpu_state_or_mailbox_head() {
    let (mut vmcb, mut frame) = stopped(0x830, 0, true);
    let profile = x2avic(&mut vmcb);
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
    assert!(target.apply_x2avic(mailbox.peek().unwrap(), &profile).is_err());
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
fn x2avic_cpu_startup_preserves_hardware_bindings_and_rejects_stale_profile() {
    use svmvisor_hypervisor::{memory::address::{AddressPolicy, EncryptionState}, svm::x2avic::{
        NativeX2AvicProfile, X2AvicCapabilities,
    }};
    let policy = AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let caps = X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18)).unwrap();
    let profile = NativeX2AvicProfile::new(caps, 0x2000, 0x3000, 37, &policy).unwrap();
    let stale = NativeX2AvicProfile::new(caps, 0x4000, 0x3000, 37, &policy).unwrap();
    let mut vmcb = Vmcb::new();
    vmcb.configure_native_boot_intercepts().unwrap();
    put(&mut vmcb, 0x90, 1); // Hardware-independent test NPT control image.
    vmcb.enable_native_x2avic(&profile).unwrap();
    let mut frame = GuestRegisters::default();
    let mut state = State::Running;
    let before = *vmcb.bytes();
    {
        let mut target = NativeStartupTarget { vmcb: &mut vmcb, frame: &mut frame, state: &mut state, signature: 0xb40f40 };
        assert!(target.apply_x2avic(Command::Init, &stale).is_err());
    }
    assert_eq!(*vmcb.bytes(), before);
    assert_eq!(state, State::Running);
    {
        // This CPU-only fixture supplies the separate producer/reset proof;
        // it does not establish a physical global quiescence implementation.
        let mut target = NativeStartupTarget { vmcb: &mut vmcb, frame: &mut frame, state: &mut state, signature: 0xb40f40 };
        assert_eq!(target.apply_x2avic(Command::Init, &profile), Ok(Effect::Init));
        target.vmcb.validate_native_x2avic(&profile).unwrap();
        assert_eq!(target.apply_x2avic(Command::Sipi(8), &profile), Ok(Effect::Started));
        assert_eq!(target.apply_x2avic(Command::Sipi(9), &profile), Ok(Effect::Ignored));
    }
    vmcb.validate_native_x2avic(&profile).unwrap();
    assert_eq!(vmcb.guest_rip(), 0);
    assert_eq!(state, State::Running);
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
fn x2avic_startup_routes_full_identity_and_notifies_after_publication() {
    let boxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19), NativeStartupMailbox::new(275)];
    for b in &boxes { b.mark_running(); }
    let mut owner = NativeIcr::admit(7, &[7, 19, 275]).unwrap();
    for (low, expected) in [(0xc500, Command::Init), (0x608, Command::Sipi(8))] {
        owner.route_x2avic_startup((275u64 << 32) | low, &boxes, |id| {
            assert_eq!(id, 275);
            assert_eq!(boxes[1].peek(), None); // No low-byte alias.
            assert_eq!(boxes[2].peek(), Some(expected));
            assert!(svmvisor_hypervisor::svm::x2avic::startup::try_lock_routes(&boxes).is_ok());
        }).unwrap();
        boxes[2].complete(expected).unwrap();
    }
    // Legacy INIT deassert completes without erasing or publishing target state.
    owner.route_x2avic_startup((275u64 << 32) | 0x8500, &boxes, |_| panic!()).unwrap();
    assert!(boxes.iter().all(|b| b.peek().is_none()));
}

#[test]
fn x2avic_startup_refusal_and_broadcast_publication_are_atomic() {
    let boxes = [NativeStartupMailbox::new(7), NativeStartupMailbox::new(19), NativeStartupMailbox::new(31)];
    let mut owner = NativeIcr::admit(7, &[7, 19, 31]).unwrap();
    boxes[0].mark_running(); boxes[1].mark_running();
    assert_eq!(owner.route_x2avic_startup((19u64 << 32) | 0x500, &boxes, |_| panic!()), Err(NativeIcrError::MailboxNotReady));
    boxes[2].mark_running();
    for low in [0x51, 0x200, 0x400, 0x501, (1 << 11) | 0x500, (1 << 18) | 0x500] {
        assert!(owner.route_x2avic_startup((19u64 << 32) | low, &boxes, |_| panic!()).is_err());
        assert!(boxes.iter().all(|b| b.peek().is_none()));
    }
    for _ in 0..4 { boxes[2].publish(Command::Sipi(9)).unwrap(); }
    let broadcast = (0xdead_beefu64 << 32) | (3 << 18) | (1 << 11) | 0x500;
    assert_eq!(owner.route_x2avic_startup(broadcast, &boxes, |_| panic!()), Err(NativeIcrError::MailboxBusy));
    assert_eq!(boxes[0].peek(), None); assert_eq!(boxes[1].peek(), None);
    for _ in 0..4 { boxes[2].complete(Command::Sipi(9)).unwrap(); }
    owner.route_x2avic_startup(broadcast, &boxes, |id| {
        assert_eq!(id, u32::MAX);
        assert_eq!(boxes[0].peek(), None);
        assert_eq!(boxes[1].peek(), Some(Command::Init));
        assert_eq!(boxes[2].peek(), Some(Command::Init));
    }).unwrap();
}
