use svmvisor_hypervisor::svm::{native_sources::*, x2avic::BackingPage};

fn route(index: u32, target: u16, vector: u8, physical: u8) -> Route {
    Route { source: SourceId { segment: 0, requester: 0xa0, index }, target, vector,
        completion: Completion::Directed(DirectedEoi { register: 0xfec00040, source_vector: physical }) }
}

#[test]
fn direct_eoi_uses_retained_source_vector_without_physical_isr_or_irr_injection() {
    let shared = SharedRoutes::new();
    let backing = BackingPage::new();
    let mut guard = shared.try_lock().unwrap();
    guard.install_stopped(route(0, 7, 0xa1, 0x31), &backing).unwrap();
    guard.install_stopped(route(1, 7, 0xa1, 0x32), &backing).unwrap();
    guard.install_stopped(route(2, 7, 0xa1, 0x31), &backing).unwrap();
    assert!(backing.is_level(0xa1));
    assert!(!backing.is_pending(0xa1));
    assert!(!backing.is_in_service(0xa1));
    let mut vectors = Vec::new();
    guard.complete_level(7, 0xa1, |eoi| {
        assert_eq!(eoi.register, 0xfec00040);
        assert!(matches!(shared.try_lock(), Err(Error::Busy)));
        vectors.push(eoi.source_vector);
        Ok::<_, ()>(())
    }).unwrap();
    assert_eq!(vectors, [0x31, 0x32]);
    drop(guard);
    assert!(shared.try_lock().is_ok());
}

#[test]
fn physical_eoi_alias_and_trigger_conflict_refuse_without_tmr_change() {
    let shared = SharedRoutes::new();
    let backing = BackingPage::new();
    let mut guard = shared.try_lock().unwrap();
    guard.install_stopped(route(0, 1, 0x81, 0x31), &backing).unwrap();
    assert_eq!(guard.install_stopped(route(1, 2, 0x91, 0x31), &backing), Err(Error::EoiAlias));
    assert!(!backing.is_level(0x91));
    let mut edge = route(2, 1, 0x81, 0x32);
    edge.completion = Completion::Edge;
    assert_eq!(guard.install_stopped(edge, &backing), Err(Error::TriggerConflict));
    assert!(backing.is_level(0x81));
    assert_eq!(guard.install_stopped(route(0, 1, 0x92, 0x33), &backing), Err(Error::SourceAlreadyOwned));
}

#[test]
fn unknown_or_unqualified_source_never_reaches_device() {
    let shared = SharedRoutes::new();
    let backing = BackingPage::new();
    let mut guard = shared.try_lock().unwrap();
    let mut unqualified = route(0, 1, 0x81, 0x31);
    unqualified.completion = Completion::UnqualifiedLevel;
    assert_eq!(guard.install_stopped(unqualified, &backing), Err(Error::UnqualifiedEoi));
    assert!(!backing.is_level(0x81));
    assert_eq!(guard.complete_level(1, 0x81, |_| -> Result<(), ()> { panic!("unowned device") }),
        Err(CompletionError::Ownership(Error::UnknownLevelSource)));
}

#[test]
fn a_failed_directed_write_poison_prevents_replay_and_new_publication() {
    let shared = SharedRoutes::new();
    let backing = BackingPage::new();
    let mut guard = shared.try_lock().unwrap();
    guard.install_stopped(route(0, 1, 0x81, 0x31), &backing).unwrap();
    guard.install_stopped(route(1, 1, 0x81, 0x32), &backing).unwrap();
    let mut count = 0;
    assert_eq!(guard.complete_level(1, 0x81, |_| { count += 1; if count == 2 {Err(7)} else {Ok(())} }),
        Err(CompletionError::Backend(7)));
    assert_eq!(count, 2);
    assert_eq!(guard.has_level(1, 0x81), Err(Error::Poisoned));
    assert_eq!(guard.complete_level(1, 0x81, |_| -> Result<(), i32> {panic!("replay")}),
        Err(CompletionError::Ownership(Error::Poisoned)));
    assert_eq!(guard.install_stopped(route(2, 1, 0x91, 0x41), &backing), Err(Error::Poisoned));
}

#[test]
fn pending_vector_is_not_rebound_and_capacity_fits_retained_allocation() {
    let shared = SharedRoutes::new();
    let backing = BackingPage::new();
    let mut guard = shared.try_lock().unwrap();
    backing.enqueue(0x81, false).unwrap();
    assert_eq!(guard.install_stopped(route(0, 1, 0x81, 0x31), &backing), Err(Error::PendingInterrupt));
    assert!(!backing.is_level(0x81));
    for index in 0..MAX_ROUTES {
        guard.install_stopped(route(index as u32, 1, 0x82, 0x32), &backing).unwrap();
    }
    assert_eq!(guard.install_stopped(route(256, 1, 0x83, 0x33), &backing), Err(Error::Full));
    assert!(!backing.is_level(0x83));
    assert!(core::mem::size_of::<SharedRoutes>() <= 0x4000);
}
