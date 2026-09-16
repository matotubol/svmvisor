use svmvisor_hypervisor::svm::ipi::{
    NativeDestinationMode as Mode, NativeIcrError as Error,
    NativeStartupMailbox as Mailbox, try_lock_routes,
};

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
            .prepare_destination_mode(1, Mode::X2Apic)
            .unwrap();
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
    routes
        .prepare_destination_mode(0, Mode::X2Apic)
        .unwrap()
        .commit_destination_mode();
    assert_eq!(mailboxes[0].destination_mode(), Ok(Mode::X2Apic));
    assert!(matches!(
        routes.prepare_destination_mode(1, Mode::X2Apic),
        Err(Error::InvalidTopology)
    ));
    assert_eq!(mailboxes[1].destination_mode(), Err(Error::MailboxNotReady));
}
