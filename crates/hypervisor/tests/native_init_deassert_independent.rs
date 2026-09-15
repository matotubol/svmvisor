use svmvisor_hypervisor::svm::ipi::{NativeIcr, NativeStartupMailbox, NativeStartupCommand as C};

fn forbidden(_: u16, _: Option<u64>) -> u32 { panic!("physical APIC write escaped") }
fn no_kick(_: u32) { panic!("deassert unexpectedly kicked/reset a target") }
fn low(owner: &mut NativeIcr, boxes: &[NativeStartupMailbox], value: u32, kick: impl FnOnce(u32)) {
    owner.xapic_access(0xfee00900, boxes, 0x300, Some(value), forbidden, kick).unwrap();
}
fn readback(owner: &mut NativeIcr, boxes: &[NativeStartupMailbox], expected: u32) {
    assert_eq!(owner.xapic_access(0xfee00900, boxes, 0x300, None, |o,w| {
        assert_eq!((o,w),(0x300,None)); 0
    }, no_kick), Ok(expected));
}

// Independent FIFO/state contract: deassert may complete a source write but
// must not enqueue a reset, consume a pending command, or use queue capacity.
#[test]
fn deassert_preserves_full_fifo_and_all_ready_cpu_metadata() {
    let boxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
    for b in &boxes { b.mark_running(); }
    let mut owner=NativeIcr::admit(0, &[0,7,19]).unwrap(); owner.enable_startup(0).unwrap();
    for target in [7u32,19] {
        owner.xapic_access(0xfee00900,&boxes,0x310,Some(target<<24),forbidden,no_kick).unwrap();
        let b=boxes.iter().find(|b| b.identity()==target).unwrap();
        let commands=[C::Init,C::Sipi(8),C::Sipi(9),C::Init];
        for c in commands { b.publish(c).unwrap(); }
        for _ in 0..8 { low(&mut owner,&boxes,0x8500,no_kick); readback(&mut owner,&boxes,0x8500); }
        for c in commands { assert_eq!(b.peek(),Some(c)); b.complete(c).unwrap(); }
        assert_eq!(b.peek(),None);
        for b in &boxes { assert!(b.is_ready()); }
    }
}

#[test]
fn each_target_gets_only_assert_and_sipi_across_repeated_deassert_sequences() {
    let boxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(7), NativeStartupMailbox::new(19)];
    for b in &boxes { b.mark_running(); }
    let mut owner=NativeIcr::admit(0, &[0,7,19]).unwrap(); owner.enable_startup(0).unwrap();
    for (slot,target) in [(1,7u32),(2,19)] {
        owner.xapic_access(0xfee00900,&boxes,0x310,Some(target<<24),forbidden,no_kick).unwrap();
        for vector in [8u8,9] {
            low(&mut owner,&boxes,0x8500,no_kick); // before assertion
            assert_eq!(boxes[slot].peek(),None);
            low(&mut owner,&boxes,0xc500,|id| assert_eq!(id,target));
            low(&mut owner,&boxes,0x8500,no_kick); // assertion still pending
            assert_eq!(boxes[slot].peek(),Some(C::Init));
            boxes[slot].complete(C::Init).unwrap();
            low(&mut owner,&boxes,0x8500,no_kick); // target awaits SIPI
            assert_eq!(boxes[slot].peek(),None);
            low(&mut owner,&boxes,0x4600|u32::from(vector),|id| assert_eq!(id,target));
            low(&mut owner,&boxes,0x8500,no_kick); // SIPI still pending
            assert_eq!(boxes[slot].peek(),Some(C::Sipi(vector)));
            boxes[slot].complete(C::Sipi(vector)).unwrap();
            low(&mut owner,&boxes,0x8500,no_kick); // running target
            for b in &boxes { assert_eq!(b.peek(),None); }
            readback(&mut owner,&boxes,0x8500);
        }
    }
}

#[test]
fn deassert_does_not_bypass_physical_alias_or_route_lock_checks() {
    use svmvisor_hypervisor::svm::ipi::{try_lock_routes,NativeDestinationMode as M};
    let boxes=[NativeStartupMailbox::new(0),NativeStartupMailbox::new(7),NativeStartupMailbox::new(23)];
    for b in &boxes { b.mark_running(); }
    let mut owner=NativeIcr::admit(0,&[0,7,23]).unwrap(); owner.enable_startup(0).unwrap();
    owner.xapic_access(0xfee00900,&boxes,0x310,Some(7<<24),forbidden,no_kick).unwrap();
    {
        let routes=try_lock_routes(&boxes).unwrap();
        for slot in 0..3 { routes.prepare_destination_mode(slot,M::ExtendedXApic4).unwrap().commit_destination_mode(); }
        assert!(owner.xapic_access(0xfee00900,&boxes,0x300,Some(0x8500),forbidden,no_kick).is_err());
    }
    // IDs7 and23 alias under four-bit reset matching: ignored delivery still
    // must not create an alternative route-admission bypass.
    assert!(owner.xapic_access(0xfee00900,&boxes,0x300,Some(0x8500),forbidden,no_kick).is_err());
    readback(&mut owner,&boxes,0);
    for b in &boxes { assert_eq!(b.peek(),None); }
}
