use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        dispatch::NativeMsrOutcome,
        ipi::{
            NativeDestinationMode, NativeIcr, NativeIcrError, NativeStartupCommand,
            NativeStartupMailbox, handle_native_apic_base, handle_native_x2apic_startup_access,
            try_lock_routes,
        },
        vmcb::Vmcb,
    },
};

fn owner() -> NativeIcr {
    let mut owner = NativeIcr::admit(0, &[0, 7]).unwrap();
    owner.enable_startup(0).unwrap();
    owner
}
fn no_access(_: u16, _: Option<u64>) -> u32 {
    panic!("physical access before admission")
}
fn no_kick(_: u32) {
    panic!("unexpected kick")
}
fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn stopped(value: u64) -> (Vmcb, GuestRegisters) {
    let mut vmcb = Vmcb::new();
    for (offset, value) in [
        (0x70, 0x7c),
        (0x78, 1),
        (0x410, 0x029b_0008),
        (0x4d0, 0x1500),
        (0x558, 0x8000_0001),
        (0x578, 0x1234_5000),
        (0x570, 0x1_0002),
        (0x68, 1),
        (0x5f8, value as u32 as u64),
    ] {
        put(&mut vmcb, offset, value);
    }
    (
        vmcb,
        GuestRegisters {
            rcx: 0x1b,
            rdx: value >> 32,
            ..Default::default()
        },
    )
}

#[test]
fn xapic_halves_publish_existing_fifo_and_hide_physical_kick() {
    let mut owner = owner();
    let boxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(7)];
    boxes[0].mark_running();
    boxes[1].mark_running();
    owner
        .xapic_access(
            0xfee0_0900,
            &boxes,
            0x310,
            Some(0x07ff_ffff),
            no_access,
            no_kick,
        )
        .unwrap();
    for (low, command) in [
        (0x4500, NativeStartupCommand::Init),
        (0x4608, NativeStartupCommand::Sipi(8)),
    ] {
        owner
            .xapic_access(0xfee0_0900, &boxes, 0x300, Some(low), no_access, |id| {
                assert_eq!(id, 7);
                assert_eq!(boxes[1].peek(), Some(command));
            })
            .unwrap();
        boxes[1].complete(command).unwrap();
        assert_eq!(
            owner.xapic_access(
                0xfee0_0900,
                &boxes,
                0x300,
                None,
                |offset, write| {
                    assert_eq!((offset, write), (0x300, None));
                    0x500
                },
                no_kick
            ),
            Ok(low)
        );
        assert_eq!(
            owner.xapic_access(0xfee0_0900, &boxes, 0x310, None, no_access, no_kick),
            Ok(7 << 24)
        );
    }
}

#[test]
fn xapic_preserves_logical_lowest_priority_and_ldr_dfr_model_bits() {
    let mut owner = owner();
    for (offset, value) in [(0xd0, 0x0200_0000), (0xe0, 0xffff_ffff)] {
        owner
            .xapic_access(
                0xfee0_0900,
                &[],
                offset,
                Some(value),
                |actual, data| {
                    let physical = if offset == 0xe0 { value & 0xf0000000 } else { value };
                    assert_eq!((actual, data), (offset, Some(physical as u64)));
                    0
                },
                no_kick,
            )
            .unwrap();
    }
    owner
        .xapic_access(0xfee0_0900, &[], 0x310, Some(2 << 24), no_access, no_kick)
        .unwrap();
    owner
        .xapic_access(
            0xfee0_0900,
            &[],
            0x300,
            Some(0x0961),
            |offset, data| {
                assert_eq!((offset, data), (0x300, Some((2 << 32) | 0x0961)));
                0
            },
            no_kick,
        )
        .unwrap();
}

#[test]
fn startup_refusal_keeps_guest_icr_and_mailbox_unchanged() {
    let mut owner = owner();
    let boxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(7)];
    owner
        .xapic_access(
            0xfee0_0900,
            &boxes,
            0x310,
            Some(7 << 24),
            no_access,
            no_kick,
        )
        .unwrap();
    assert_eq!(
        owner.xapic_access(0xfee0_0900, &boxes, 0x300, Some(0x500), no_access, no_kick),
        Err(NativeIcrError::MailboxNotReady)
    );
    assert_eq!(boxes[1].peek(), None);
    assert_eq!(
        owner.xapic_access(0xfee0_0900, &boxes, 0x300, None, |_, _| 0, no_kick),
        Ok(0)
    );
    for low in [0x0d00, 0x40500, 0x80500, 0xc0500, 0x8500] {
        assert!(
            owner
                .xapic_access(0xfee0_0900, &boxes, 0x300, Some(low), no_access, no_kick)
                .is_err()
        );
    }
    assert_eq!(
        owner.xapic_access(0xfee0_0900, &boxes, 0x20, Some(0), no_access, no_kick),
        Err(NativeIcrError::UnsupportedRegister)
    );
}

#[test]
fn promotion_tracks_physical_bus_and_uses_hardware_new_icr_high() {
    let mut owner = owner();
    owner
        .xapic_access(0xfee0_0900, &[], 0x300, Some(0x61), |_, _| 0, no_kick)
        .unwrap();
    let mut base = 0xfee0_0900;
    let (mut vmcb, frame) = stopped(0xfee0_0d00);
    assert_eq!(
        handle_native_apic_base(
            &mut owner,
            &mut base,
            &mut vmcb,
            &frame,
            &[0x0f, 0x30],
            true,
            |value| {
                assert_eq!(value, 0xfee0_0d00);
                (42 << 32) | 0x500
            }
        ),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!(base, 0xfee0_0d00);
    assert_eq!(vmcb.guest_rip(), 0x1234_5002);
    assert_eq!(
        owner.xapic_access(base, &[], 0x80, None, no_access, no_kick),
        Err(NativeIcrError::UnsupportedMode)
    );
    let (mut vmcb, mut frame) = stopped(0);
    put(&mut vmcb, 0x78, 0);
    frame.rcx = 0x830;
    assert_eq!(
        handle_native_x2apic_startup_access(
            &mut owner,
            base,
            &[],
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x32],
            |_, _| panic!("read"),
            no_kick
        ),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!((vmcb.guest_rax(), frame.rdx), (0x61, 42));
}

#[test]
fn disabled_or_relocated_apic_base_stops_without_hardware_or_guest_commit() {
    for (initial, attempted) in [
        (0xfee0_0900, 0xfee0_0100),
        (0xfee0_0d00, 0xfee0_0100),
        (0xfee0_0900, 0xfed0_0900),
    ] {
        let mut owner = owner();
        let mut base = initial;
        let (mut vmcb, frame) = stopped(attempted);
        let before = *vmcb.bytes();
        assert_eq!(
            handle_native_apic_base(
                &mut owner,
                &mut base,
                &mut vmcb,
                &frame,
                &[0x0f, 0x30],
                true,
                |_| panic!("refused mode write")
            ),
            Err(NativeIcrError::ApicBaseChange)
        );
        assert_eq!(base, initial);
        assert_eq!(*vmcb.bytes(), before);
    }
}

#[test]
fn illegal_demotion_prepares_gp_at_original_rip() {
    let mut owner = owner();
    let mut base = 0xfee0_0d00;
    let (mut vmcb, frame) = stopped(0xfee0_0900);
    assert_eq!(
        handle_native_apic_base(
            &mut owner,
            &mut base,
            &mut vmcb,
            &frame,
            &[0x0f, 0x30],
            true,
            |_| panic!("illegal hardware write")
        ),
        Ok(NativeMsrOutcome::GeneralProtectionPrepared)
    );
    assert_eq!(vmcb.guest_rip(), 0x1234_5000);
    assert_eq!(base, 0xfee0_0d00);
    assert_eq!(
        u64::from_le_bytes(vmcb.bytes()[0xa8..0xb0].try_into().unwrap()) & 0xffff_ffff,
        0x8000_0b0d
    );
}

#[test]
fn absent_x2apic_capability_preserves_xapic_and_refuses_promotion_before_hardware() {
    use svmvisor_hypervisor::svm::ipi::native_apic_mode_supported;
    assert!(native_apic_mode_supported(0x7ed8320b, false));
    assert!(!native_apic_mode_supported(0x7ed8320b, true));
    assert!(native_apic_mode_supported(1 << 21, true));
    for attempted in [0xfee0_0900, 0xfee0_0d00] {
        let mut owner = owner();
        let mut base = 0xfee0_0900;
        let (mut vmcb, frame) = stopped(attempted);
        let before = *vmcb.bytes();
        let promote = attempted & 0x400 != 0;
        assert_eq!(handle_native_apic_base(&mut owner, &mut base, &mut vmcb,
            &frame, &[0x0f, 0x30], false, |_| panic!("unsupported hardware promotion")),
            if promote { Err(NativeIcrError::UnsupportedMode) }
               else { Ok(NativeMsrOutcome::Completed) });
        assert_eq!(base, 0xfee0_0900);
        assert_eq!(vmcb.guest_rip(), if promote {0x1234_5000} else {0x1234_5002});
        if promote {
            assert_eq!(*vmcb.bytes(), before);
        }
        assert_eq!(owner.xapic_access(base, &[], 0x20, None, |_, _| 0, no_kick), Ok(0));
    }
}

#[test]
fn xapic_admission_excludes_broadcast_ids_disabled_bus_and_relocation() {
    let owner = owner();
    assert_eq!(owner.admit_apic_base(0xfee0_0900), Ok(()));
    assert_eq!(owner.admit_apic_base(0xfee0_0d00), Ok(()));
    for base in [0xfee0_0100, 0xfee0_0500, 0xfed0_0900, 0xfee0_0901] {
        assert_eq!(
            owner.admit_apic_base(base),
            Err(NativeIcrError::UnsupportedMode)
        );
    }
    let owner = NativeIcr::admit(0, &[0, 255]).unwrap();
    assert_eq!(
        owner.admit_apic_base(0xfee0_0900),
        Err(NativeIcrError::UnsupportedMode)
    );
    assert_eq!(owner.admit_apic_base(0xfee0_0d00), Ok(()));
}

#[test]
fn extended_xapic_control_observation_is_physical_only_and_guest_cannot_change_it() {
    let mut owner = NativeIcr::admit(0, &[0, 15, 16]).unwrap();
    owner.enable_startup(0).unwrap();
    assert_eq!(
        svmvisor_hypervisor::svm::ipi::admit_native_xapic_extended_profile(
            0x00b4_0f40, 0x8105_0010, 0x0004_0007, 0,
        ),
        Ok(NativeDestinationMode::ExtendedXApic4)
    );
    let boxes = [
        NativeStartupMailbox::new(0),
        NativeStartupMailbox::new(15),
        NativeStartupMailbox::new(16),
    ];
    {
        let routes = try_lock_routes(&boxes).unwrap();
        for slot in 0..3 {
            routes
                .prepare_destination_mode(slot, NativeDestinationMode::ExtendedXApic8)
                .unwrap()
                .commit_destination_mode();
        }
    }
    for mailbox in &boxes {
        mailbox.mark_running();
    }
    assert_eq!(
        owner.xapic_access(
            0xfee0_0900,
            &boxes,
            0x410,
            Some(0),
            no_access,
            no_kick
        ),
        Err(NativeIcrError::UnsupportedRegister)
    );
    assert_eq!(
        boxes[0].destination_mode(),
        Ok(NativeDestinationMode::ExtendedXApic8)
    );
}
