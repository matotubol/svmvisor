use svmvisor_hypervisor::svm::x2avic::startup::{
    NativeDestinationMode as Mode, NativeIcrError as Error, NativeStartupMailbox as Mailbox,
    try_lock_routes,
};

#[test]
fn routing_guard_excludes_hardware_commit_and_releases_on_refusal() {
    let mailboxes = [Mailbox::new(0), Mailbox::new(16)];
    assert_eq!(core::mem::size_of::<Mailbox>(), 64);
    assert_eq!(mailboxes[1].destination_mode(), Err(Error::MailboxNotReady));
    {
        let routes = try_lock_routes(&mailboxes).unwrap();
        assert!(matches!(try_lock_routes(&mailboxes), Err(Error::RoutingBusy)));
        let commit = routes.prepare_destination_mode(1, Mode::X2Apic).unwrap();
        // Preparation is read-only until the caller's infallible HW commit.
        assert_eq!(mailboxes[1].destination_mode(), Err(Error::MailboxNotReady));
        commit.commit_destination_mode();
        assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::X2Apic));
        assert!(matches!(
            routes.prepare_destination_mode(2, Mode::X2Apic),
            Err(Error::MailboxMismatch)
        ));
    }
    let routes = try_lock_routes(&mailboxes).unwrap();
    assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::X2Apic));
    drop(routes);
    mailboxes[1].mark_running();
    assert_eq!(mailboxes[1].destination_mode(), Ok(Mode::X2Apic));
}

#[test]
fn wide_identity_publishes_x2apic_routing_but_broadcast_identity_does_not() {
    let mailboxes = [Mailbox::new(0x1234), Mailbox::new(u32::MAX)];
    let routes = try_lock_routes(&mailboxes).unwrap();
    assert_eq!(mailboxes[0].destination_mode(), Err(Error::MailboxNotReady));
    routes.prepare_destination_mode(0, Mode::X2Apic).unwrap().commit_destination_mode();
    assert_eq!(mailboxes[0].destination_mode(), Ok(Mode::X2Apic));
    assert!(matches!(
        routes.prepare_destination_mode(1, Mode::X2Apic),
        Err(Error::InvalidTopology)
    ));
    assert_eq!(mailboxes[1].destination_mode(), Err(Error::MailboxNotReady));
}

#[test]
fn destination_slot_validation_needs_no_route_lease() {
    use svmvisor_hypervisor::svm::x2avic::startup::validate_destination_slot;
    let mailboxes = [Mailbox::new(0), Mailbox::new(u32::MAX)];
    // A target checks its slot before its INIT commit, while a source may
    // hold the route lease.
    let routes = try_lock_routes(&mailboxes).unwrap();
    assert_eq!(validate_destination_slot(&mailboxes, 0).map(Mailbox::identity), Ok(0));
    assert_eq!(validate_destination_slot(&mailboxes, 1).err(), Some(Error::InvalidTopology));
    assert_eq!(validate_destination_slot(&mailboxes, 2).err(), Some(Error::MailboxMismatch));
    drop(routes);
}

#[test]
fn incomplete_ipi_routing_waits_out_a_busy_route_lease() {
    // The ICR write behind an AVIC_INCOMPLETE_IPI exit has completed, so the
    // source cannot retry it: it waits far longer than `try_lock_routes`.
    use std::sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    };
    use svmvisor_hypervisor::svm::x2avic::startup::{NativeIcr, NativeStartupCommand as Command};
    let mailboxes = [Mailbox::new(0), Mailbox::new(1)];
    for mailbox in &mailboxes {
        mailbox.mark_running();
    }
    let mut source = NativeIcr::admit(0, &[0, 1]).unwrap();
    let (held, released) = (Barrier::new(2), AtomicBool::new(false));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let routes = try_lock_routes(&mailboxes).unwrap();
            held.wait();
            // Far beyond the 64 attempts of `try_lock_routes`.
            std::thread::sleep(std::time::Duration::from_millis(20));
            released.store(true, Ordering::SeqCst);
            drop(routes);
        });
        held.wait();
        assert!(matches!(try_lock_routes(&mailboxes), Err(Error::RoutingBusy)));
        assert_eq!(source.route_x2avic_startup(0x0000_0001_0000_0500, &mailboxes, |_| {}), Ok(()));
        assert!(released.load(Ordering::SeqCst));
    });
    assert_eq!(mailboxes[1].peek(), Some(Command::Init));
    assert_eq!(source.route_failure(), None);
}

#[test]
fn a_lost_route_lease_holder_bounds_the_incomplete_ipi_wait() {
    use svmvisor_hypervisor::svm::x2avic::startup::{NativeIcr, NativeRoutePredicate};
    let mailboxes = [Mailbox::new(0), Mailbox::new(1)];
    for mailbox in &mailboxes {
        mailbox.mark_running();
    }
    let mut source = NativeIcr::admit(0, &[0, 1]).unwrap();
    let routes = try_lock_routes(&mailboxes).unwrap();
    assert_eq!(
        source.route_x2avic_startup(0x0000_0001_0000_0500, &mailboxes, |_| panic!("kick")),
        Err(Error::RoutingBusy)
    );
    assert_eq!(
        source.route_failure().map(|failure| failure.predicate),
        Some(NativeRoutePredicate::RouteBusy)
    );
    assert_eq!(mailboxes[1].peek(), None);
    drop(routes);
}
