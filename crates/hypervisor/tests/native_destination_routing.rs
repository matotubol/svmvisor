use svmvisor_hypervisor::svm::ipi::{
    NativeDestinationMode as Mode, NativeIcr, NativeIcrError as Error,
    NativeStartupCommand as Command, NativeStartupMailbox as Mailbox, try_lock_routes,
};

fn no_access(_: u16, _: Option<u64>) -> u32 {
    panic!("unexpected physical APIC access")
}

fn no_kick(_: u32) {
    panic!("unexpected notification")
}

fn initialized(cpus: &[(u32, Mode)]) -> Vec<Mailbox> {
    let mailboxes: Vec<_> = cpus.iter().map(|&(id, _)| Mailbox::new(id)).collect();
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        for (slot, &(_, mode)) in cpus.iter().enumerate() {
            routes
                .prepare_destination_mode(slot, mode)
                .unwrap()
                .commit_destination_mode();
        }
    }
    for mailbox in &mailboxes {
        mailbox.mark_running();
    }
    mailboxes
}

fn owner(source: u32, mailboxes: &[Mailbox], destination: u32) -> NativeIcr {
    let ids: Vec<_> = mailboxes.iter().map(Mailbox::identity).collect();
    let mut owner = NativeIcr::admit(source, &ids).unwrap();
    owner.enable_startup(u64::from(destination) << 32).unwrap();
    owner
}

#[test]
fn local_extended_profile_preserves_controls_and_reports_exact_refusal() {
    use svmvisor_hypervisor::svm::ipi::{
        NativeXApicProfileError as ProfileError, admit_native_xapic_extended_profile as admit,
    };
    for control in 0..=7 {
        assert_eq!(
            admit(0x00b4_0f40, 0x8105_0010, 0x0004_0007, control),
            Ok(if control & 4 == 0 { Mode::ExtendedXApic4 } else { Mode::ExtendedXApic8 })
        );
    }
    assert_eq!(admit(0x00b4_0f41, 0x8105_0010, 0x0004_0007, 0),
        Err(ProfileError::Signature { actual: 0x00b4_0f41 }));
    assert_eq!(admit(0x00b4_0f40, 0x8005_0010, 0x0004_0007, 0),
        Err(ProfileError::Version { actual: 0x8005_0010 }));
    assert_eq!(admit(0x00b4_0f40, 0x8105_0010, 0x0004_0003, 0),
        Err(ProfileError::Feature { actual: 0x0004_0003 }));
    for bit in 3..32 {
        let control = (1 << bit) | 7;
        assert_eq!(admit(0x00b4_0f40, 0x8105_0010, 0x0004_0007, control),
            Err(ProfileError::ReservedControl { actual: control }));
    }
}

#[test]
fn initial_four_bit_bsp_must_be_normalized_before_guest_startup() {
    let cpus: Vec<_> = (0..24).map(|id| (id, if id == 0 { Mode::ExtendedXApic4 } else { Mode::ExtendedXApic8 })).collect();
    let mailboxes = initialized(&cpus);
    let mut source = owner(0, &mailboxes, 16);
    assert_eq!(source.xapic_access(0xfee0_0900, &mailboxes, 0x300, Some(0xc500), no_access, no_kick), Err(Error::UnsupportedMode));
    assert!(mailboxes.iter().all(|m| m.peek().is_none()));
    assert_eq!(source.route_failure().unwrap().predicate,
        svmvisor_hypervisor::svm::ipi::NativeRoutePredicate::RecipientModeInvalid);
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        routes.prepare_destination_mode(0, Mode::ExtendedXApic8).unwrap().commit_destination_mode();
    }
    for destination in [16, 17] {
        source.xapic_access(0xfee0_0900, &mailboxes, 0x310, Some(destination << 24), no_access, no_kick).unwrap();
        source.xapic_access(0xfee0_0900, &mailboxes, 0x300, Some(0xc500), no_access,
            |id| assert_eq!(id, destination)).unwrap();
        assert_eq!(mailboxes[destination as usize].peek(), Some(Command::Init));
    }
    assert_eq!(mailboxes[0].peek(), None);
}

#[test]
fn ordinary_ipi_and_local_id_reads_keep_native_full_width_values() {
    let mailboxes = initialized(&[(0, Mode::ExtendedXApic8), (16, Mode::ExtendedXApic8)]);
    let mut source = owner(0, &mailboxes, 16);
    // Ordinary fixed IPI forwarding uses the same normalized eight-bit IDs.
    source.xapic_access(0xfee0_0900, &mailboxes, 0x300, Some(0x51),
        |offset, value| { assert_eq!((offset, value), (0x300, Some((16u64 << 32) | 0x51))); 0 }, no_kick).unwrap();
    assert!(mailboxes.iter().all(|mailbox| mailbox.peek().is_none()));
    let mut target = owner(16, &mailboxes, 0);
    assert_eq!(target.xapic_access(0xfee0_0800, &mailboxes, 0x20, None,
        |offset, value| { assert_eq!((offset, value), (0x20, None)); 16 << 24 }, no_kick), Ok(16 << 24));
}

#[test]
fn routing_guard_excludes_hardware_commit_and_releases_on_refusal() {
    let mailboxes = [Mailbox::new(0), Mailbox::new(16)];
    assert_eq!(core::mem::size_of::<Mailbox>(), 64);
    assert_eq!(mailboxes[1].destination_mode(), Err(Error::MailboxNotReady));
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        assert!(matches!(
            try_lock_routes(&mailboxes),
            Err(Error::RoutingBusy)
        ));
        let commit = routes
            .prepare_destination_mode(1, Mode::ExtendedXApic8)
            .unwrap();
        // Preparation is read-only until the caller's infallible HW commit.
        assert_eq!(mailboxes[1].destination_mode(), Err(Error::MailboxNotReady));
        commit.commit_destination_mode();
        assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::ExtendedXApic8));
        assert!(matches!(
            routes.prepare_destination_mode(2, Mode::XApic),
            Err(Error::MailboxMismatch)
        ));
    }
    let routes = try_lock_routes(&mailboxes).unwrap();
    assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::ExtendedXApic8));
    drop(routes);
    mailboxes[1].mark_running();
    assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::ExtendedXApic8));
}

#[test]
fn wide_identity_can_only_publish_x2apic_routing() {
    let mailboxes = [Mailbox::new(0x1234)];
    let routes = try_lock_routes(&mailboxes).unwrap();
    for mode in [Mode::XApic, Mode::ExtendedXApic4, Mode::ExtendedXApic8] {
        assert!(matches!(
            routes.prepare_destination_mode(0, mode),
            Err(Error::InvalidTopology)
        ));
    }
    assert_eq!(mailboxes[0].destination_mode(), Err(Error::MailboxNotReady));
    routes
        .prepare_destination_mode(0, Mode::X2Apic)
        .unwrap()
        .commit_destination_mode();
    assert_eq!(mailboxes[0].destination_mode(), Ok(Mode::X2Apic));
}

#[test]
fn x2apic_uses_full_destination_without_low_byte_aliases() {
    use svmvisor_hypervisor::{
        arch::x86_64::registers::GuestRegisters,
        svm::{dispatch::NativeMsrOutcome, ipi::handle_native_x2apic_startup_access, vmcb::Vmcb},
    };
    let mailboxes = initialized(&[
        (0, Mode::X2Apic),
        (0x100, Mode::X2Apic),
        (0x1100, Mode::X2Apic),
    ]);
    let mut source = owner(0, &mailboxes, 0);
    let mut vmcb = Vmcb::new();
    for (offset, value) in [
        (0x70, 0x7cu64),
        (0x78, 1),
        (0x410, 0x029b_0008),
        (0x4d0, 0x1500),
        (0x558, 0x8000_0001),
        (0x578, 0x1234_5000),
        (0x570, 0x1_0002),
        (0x68, 1),
        (0x5f8, 0x500),
    ] {
        unsafe {
            core::ptr::copy_nonoverlapping(
                value.to_le_bytes().as_ptr(),
                (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                8,
            );
        }
    }
    let mut frame = GuestRegisters {
        rcx: 0x830,
        rdx: 0x1100,
        ..Default::default()
    };
    assert_eq!(
        handle_native_x2apic_startup_access(
            &mut source,
            0xfee0_0d00,
            &mailboxes,
            &mut vmcb,
            &mut frame,
            &[0x0f, 0x30],
            |_, _| panic!("physical guest INIT"),
            |id| assert_eq!(id, 0x1100)
        ),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!(mailboxes[0].peek(), None);
    assert_eq!(mailboxes[1].peek(), None);
    assert_eq!(mailboxes[2].peek(), Some(Command::Init));
    assert_eq!(vmcb.guest_rip(), 0x1234_5002);
}

#[test]
fn reset_high_id_matches_uniquely_and_notification_runs_after_unlock() {
    let mailboxes = initialized(&[(0, Mode::ExtendedXApic8), (16, Mode::ExtendedXApic8)]);
    let mut source = owner(0, &mailboxes, 16);
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        // Guest INIT preserves the host-owned normalized physical destination width.
        routes
            .prepare_destination_mode(1, Mode::ExtendedXApic8)
            .unwrap()
            .commit_destination_mode();
    }
    source
        .xapic_access(
            0xfee0_0900,
            &mailboxes,
            0x300,
            Some(0x608),
            no_access,
            |id| {
                assert_eq!(id, 16);
                assert_eq!(mailboxes[1].peek(), Some(Command::Sipi(8)));
                // A broadcast wakeup may immediately let another CPU take this lock.
                drop(try_lock_routes(&mailboxes).unwrap());
            },
        )
        .unwrap();
    assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::ExtendedXApic8));
}

#[test]
fn unnormalized_modes_unowned_ids_and_broadcasts_never_publish() {
    for (cpus, destination) in [
        (
            vec![(0, Mode::ExtendedXApic4), (16, Mode::ExtendedXApic4)],
            16,
        ),
        (
            vec![
                (0, Mode::ExtendedXApic8),
                (1, Mode::ExtendedXApic4),
                (17, Mode::ExtendedXApic4),
            ],
            17,
        ),
        (
            vec![(0, Mode::ExtendedXApic8), (15, Mode::ExtendedXApic4)],
            15,
        ),
        (vec![(0, Mode::XApic), (255, Mode::X2Apic)], 255),
        // The physical low-four alias exists, but guest destination33 was
        // never an assigned identity. Do not invent an ID remapping.
        (
            vec![(0, Mode::ExtendedXApic8), (1, Mode::ExtendedXApic4)],
            33,
        ),
        (
            vec![(0, Mode::ExtendedXApic8), (16, Mode::ExtendedXApic4)],
            0,
        ),
    ] {
        let mailboxes = initialized(&cpus);
        let mut source = owner(0, &mailboxes, destination);
        assert!(matches!(
            source.xapic_access(
                0xfee0_0900,
                &mailboxes,
                0x300,
                Some(0x500),
                no_access,
                no_kick
            ),
            Err(Error::UnownedStartup { .. }) | Err(Error::UnsupportedMode)
        ));
        assert!(mailboxes.iter().all(|m| m.peek().is_none()));
        assert_eq!(
            source.xapic_access(0xfee0_0900, &mailboxes, 0x300, None, |_, _| 0, no_kick),
            Ok(0)
        );
        drop(try_lock_routes(&mailboxes).unwrap());
    }
}

#[test]
fn broadcast_notification_requires_every_cpu_ready_and_no_routing_commit_in_flight() {
    let mailboxes = [Mailbox::new(0), Mailbox::new(16), Mailbox::new(2)];
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        for slot in 0..3 {
            routes
                .prepare_destination_mode(slot, Mode::ExtendedXApic8)
                .unwrap()
                .commit_destination_mode();
        }
    }
    mailboxes[0].mark_running();
    mailboxes[1].mark_running();
    let mut source = owner(0, &mailboxes, 16);
    assert_eq!(
        source.xapic_access(
            0xfee0_0900,
            &mailboxes,
            0x300,
            Some(0x500),
            no_access,
            no_kick
        ),
        Err(Error::MailboxNotReady)
    );
    assert_eq!(mailboxes[1].peek(), None);
    mailboxes[2].mark_running();
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        assert_eq!(
            source.xapic_access(
                0xfee0_0900,
                &mailboxes,
                0x300,
                Some(0x500),
                no_access,
                no_kick
            ),
            Err(Error::RoutingBusy)
        );
        assert_eq!(mailboxes[1].peek(), None);
        drop(routes);
    }
    source
        .xapic_access(
            0xfee0_0900,
            &mailboxes,
            0x300,
            Some(0x500),
            no_access,
            |_| {},
        )
        .unwrap();
    assert_eq!(mailboxes[1].peek(), Some(Command::Init));
}

#[test]
fn hidden_extended_control_never_changes_host_routing() {
    let mailboxes = initialized(&[(0, Mode::ExtendedXApic8), (16, Mode::ExtendedXApic8)]);
    let mut target = owner(16, &mailboxes, 0);
    for write in [None, Some(0), Some(4), Some(u32::MAX)] {
        assert_eq!(target.xapic_access(0xfee0_0800, &mailboxes, 0x410, write, no_access, no_kick), Err(Error::UnsupportedRegister));
        assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::ExtendedXApic8));
    }
    let mut source = owner(0, &mailboxes, 16);
    source.xapic_access(0xfee0_0900, &mailboxes, 0x300, Some(0xc500), no_access, |id| assert_eq!(id,16)).unwrap();
    assert_eq!(mailboxes[1].peek(), Some(Command::Init));
}

#[test]
fn nonextended_or_reserved_control_access_stops_before_hardware() {
    let mailboxes = initialized(&[(0, Mode::XApic), (16, Mode::ExtendedXApic8)]);
    let mut source = owner(0, &mailboxes, 16);
    for write in [None, Some(4)] {
        assert_eq!(
            source.xapic_access(0xfee0_0900, &mailboxes, 0x410, write, no_access, no_kick),
            Err(Error::UnsupportedRegister)
        );
    }
    let mut target = owner(16, &mailboxes, 0);
    assert_eq!(
        target.xapic_access(0xfee0_0800, &mailboxes, 0x410, Some(8), no_access, no_kick),
        Err(Error::UnsupportedRegister)
    );
    assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::ExtendedXApic8));
    drop(try_lock_routes(&mailboxes).unwrap());
}
