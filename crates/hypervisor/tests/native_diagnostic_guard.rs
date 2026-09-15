use std::sync::{Arc, Barrier, atomic::{AtomicU32, Ordering}};
use svmvisor_hypervisor::host::resident::terminal::TerminalControl;

#[test]
fn contending_publishers_have_one_owner_until_guard_drop() {
    for _ in 0..8 {
        let control=Arc::new(TerminalControl::new());
        let start=Arc::new(Barrier::new(8));
        let attempted=Arc::new(Barrier::new(8));
        let winners=Arc::new(AtomicU32::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let (control,start,attempted,winners)=(control.clone(),start.clone(),attempted.clone(),winners.clone());
                scope.spawn(move || {
                    start.wait();
                    let guard=control.diagnostic_lock();
                    if guard.is_some() { winners.fetch_add(1,Ordering::Relaxed); }
                    // The winner cannot release until every contender tried.
                    attempted.wait();
                    drop(guard);
                });
            }
        });
        assert_eq!(winners.load(Ordering::Relaxed),1);
        let next=control.diagnostic_lock();
        assert!(next.is_some(),"owner drop must release the lifetime gate");
        assert!(control.diagnostic_lock().is_none());
    }
}

#[test]
fn revocation_survives_owner_drop_and_subsequent_terminal_lifecycle() {
    let control=Arc::new(TerminalControl::new());
    let held=Arc::new(Barrier::new(2));
    let observed=Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let (c,h,o)=(control.clone(),held.clone(),observed.clone());
        scope.spawn(move || {
            let guard=c.diagnostic_lock().unwrap();
            c.diagnostic_revoke();
            h.wait(); o.wait();
            drop(guard);
        });
        held.wait();
        assert!(control.diagnostic_revoked());
        assert!(control.diagnostic_lock().is_none());
        observed.wait();
    });
    let guard=control.diagnostic_lock().unwrap();
    assert!(control.diagnostic_revoked());
    assert!(control.initial_ack(0,1));
    assert!(control.claim(0,1));
    assert!(control.acknowledge(0,1));
    control.finish(1);
    assert!(control.diagnostic_revoked(),"terminal changes cannot reopen stale aliases");
    drop(guard);
    assert_eq!(std::mem::size_of::<TerminalControl>(),256);
    assert_eq!(std::mem::align_of::<TerminalControl>(),64);
}
