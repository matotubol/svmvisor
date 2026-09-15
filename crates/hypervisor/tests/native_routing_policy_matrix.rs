//! Exhaustive small-topology counterexamples for the native delivery policy.
//! This exercises the production owner, not a replacement routing model.
use svmvisor_hypervisor::svm::ipi::{
    NativeDestinationMode as M, NativeIcr, NativeIcrError, NativeStartupCommand as C,
    NativeStartupMailbox, NativeRoutePredicate as P, NativeDestinationCause as H, try_lock_routes,
};

fn boxes(ids: [u32; 3], modes: [M; 3]) -> [NativeStartupMailbox; 3] {
    let b = ids.map(NativeStartupMailbox::new);
    for m in &b { m.mark_running(); }
    let guard = try_lock_routes(&b).unwrap();
    for (i, mode) in modes.into_iter().enumerate() {
        guard.prepare_destination_mode(i, mode).unwrap().commit_destination_mode();
    }
    drop(guard);
    b
}

fn send(owner: &mut NativeIcr, b: &[NativeStartupMailbox], id: u32, command: u32)
    -> Result<u32, NativeIcrError>
{
    owner.xapic_access(0xfee00900, b, 0x310, Some(id << 24),
        |_, _| panic!("high shadow escaped"), |_| panic!("high shadow kick")).unwrap();
    owner.xapic_access(0xfee00900, b, 0x300, Some(command),
        |_, _| panic!("physical guest startup escaped"), |actual| assert_eq!(actual, id))
}

#[test]
fn all_modes_sources_and_inventory_orders_require_exact_target_and_normalization() {
    let modes = [M::XApic, M::ExtendedXApic4, M::ExtendedXApic8, M::X2Apic];
    for ids in [[0,16,32], [0,32,16], [16,0,32], [16,32,0], [32,0,16], [32,16,0]] {
        for source in ids {
            for a in modes { for b in modes { for c in modes {
                let selected = [a,b,c];
                let mailboxes = boxes(ids, selected);
                let mut owner = NativeIcr::admit(source, &ids).unwrap();
                owner.enable_startup(0).unwrap();
                let allowed = source != 16 && selected.iter().all(|&mode| mode != M::ExtendedXApic4);
                let result = send(&mut owner, &mailboxes, 16, 0xc500);
                if allowed { assert_eq!(result, Ok(0)); assert!(owner.route_failure().is_none()); }
                else { assert_eq!(result, Err(if source == 16 { NativeIcrError::UnownedStartup {
                    value: 0x10_0000_c500
                } } else { NativeIcrError::UnsupportedMode }));
                    let failure = owner.route_failure().unwrap();
                    assert_eq!(failure.source, source);
                    assert_eq!(failure.value, 0x10_0000_c500);
                    assert_eq!(failure.predicate, if source == 16 { P::SelfDestination } else { P::RecipientModeInvalid });
                    if source != 16 {
                        let r = failure.recipient.unwrap();
                        assert_eq!(r.mode, Some(M::ExtendedXApic4));
                        assert_eq!(r.init_count, 0);
                    }
                }
                for mailbox in &mailboxes {
                    assert_eq!(mailbox.peek(),
                        if allowed && mailbox.identity() == 16 { Some(C::Init) } else { None });
                }
            }}}
        }
    }
}

#[test]
fn absent_exact_assignment_is_separate_from_physical_alias() {
    let b = boxes([0,1,32], [M::ExtendedXApic8;3]);
    let mut owner = NativeIcr::admit(0, &[0,1,32]).unwrap();
    owner.enable_startup(0).unwrap();
    assert_eq!(send(&mut owner, &b, 16, 0xc500),
        Err(NativeIcrError::UnownedStartup { value: 0x10_0000_c500 }));
    assert!(b.iter().all(|m| m.peek().is_none()));
    assert_eq!(owner.route_failure().unwrap().predicate, P::DestinationUnassigned);
}

#[test]
fn repeated_guest_init_keeps_exact_target_routes_and_physical_forwarding_consistent() {
    let b = boxes([0,16,32], [M::ExtendedXApic8;3]);
    let mut owner = NativeIcr::admit(0, &[0,16,32]).unwrap();
    owner.enable_startup(0).unwrap();
    send(&mut owner, &b, 16, 0xc500).unwrap();
    // Model the production target's local guest INIT/reset-width commit.
    {
        let guard = try_lock_routes(&b).unwrap();
        guard.prepare_destination_mode(1, M::ExtendedXApic8).unwrap()
            .commit_destination_mode_from(H::GuestInit);
        b[1].complete(C::Init).unwrap();
    }
    send(&mut owner, &b, 16, 0x4608).unwrap();
    b[1].complete(C::Sipi(8)).unwrap();
    send(&mut owner, &b, 32, 0xc500).unwrap();
    assert_eq!(b[2].peek(), Some(C::Init));
    assert!(owner.route_failure().is_none());
    let mut physical = None;
    owner.xapic_access(0xfee00900, &b, 0x300, Some(0xf1),
        |offset,value| { physical = Some((offset,value)); 0 },
        |_| panic!("ordinary IPI used private notification")).unwrap();
    assert_eq!(physical, Some((0x300, Some(0x20_0000_00f1))));
}
