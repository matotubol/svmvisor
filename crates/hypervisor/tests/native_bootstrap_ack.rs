use svmvisor_hypervisor::{
    guest::continuation::{BootstrapAckError as E, NATIVE_BOOTSTRAP_ACK, NativeBootstrapAck},
    registers::GuestRegisters,
    vmcb::Vmcb,
};

fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    // Test-only hardware save stand-in; production has no mutable byte API.
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn initial() -> (Vmcb, GuestRegisters) {
    let mut v = Vmcb::new();
    put(&mut v, 0x578, 0x1000);
    put(&mut v, 0x5d8, 0x8f80);
    put(&mut v, 0x570, 2);
    let r = GuestRegisters {
        r14: 0x8fc0,
        r15: 0x9580,
        rcx: 0x1234,
        ..Default::default()
    };
    (v, r)
}
fn exited(v: &mut Vmcb) {
    put(v, 0x070, 0x81);
    put(v, 0x578, 0x1005);
    put(v, 0x5f8, NATIVE_BOOTSTRAP_ACK);
}

#[test]
fn only_linked_ack_advances_once_without_restoring_or_reseeding_registers() {
    let (mut v, r) = initial();
    let mut ack = NativeBootstrapAck::new(&v, &r, 0x1000, 0x1005, 0x1008).unwrap();
    exited(&mut v);
    let mut expected = *v.bytes();
    expected[0x578..0x580].copy_from_slice(&0x1008u64.to_le_bytes());
    ack.acknowledge(&mut v, &r).unwrap();
    assert_eq!(v.bytes(), &expected);
    assert!(ack.acknowledged());
    assert_eq!(ack.acknowledge(&mut v, &r), Err(E::AlreadyAcknowledged));
    assert_eq!(v.bytes(), &expected);
}

#[test]
fn wrong_exit_cookie_stack_flags_or_pending_event_is_transactional() {
    for (offset, value, error) in [
        (0x070, 0x72, E::UnexpectedExit),
        (0x578, 0x1006, E::UnexpectedExit),
        (0x5f8, 0, E::StateMismatch),
        (0x5d8, 0x9000, E::StateMismatch),
        (0x570, 0x202, E::StateMismatch),
        (0x0a8, 1 << 31, E::PendingEvent),
        (0x088, 1 << 31, E::PendingEvent),
        (0x068, 1, E::PendingEvent),
        (0x060, 1 << 8, E::PendingEvent),
    ] {
        let (mut v, r) = initial();
        let mut ack = NativeBootstrapAck::new(&v, &r, 0x1000, 0x1005, 0x1008).unwrap();
        exited(&mut v);
        put(&mut v, offset, value);
        let before = *v.bytes();
        assert_eq!(ack.acknowledge(&mut v, &r), Err(error), "offset {offset:x}");
        assert_eq!(v.bytes(), &before);
        assert!(!ack.acknowledged());
    }
}

#[test]
fn changed_frame_and_unbound_sites_cannot_authorize_return() {
    let (mut v, mut r) = initial();
    for (resume, ack, after) in [
        (0x1000, 0x1004, 0x1007),
        (0x1000, 0x1005, 0x1009),
        (0x7fff_ffff_fffc, 0x8000_0000_0001, 0x8000_0000_0004),
    ] {
        assert!(matches!(
            NativeBootstrapAck::new(&v, &r, resume, ack, after),
            Err(E::InvalidSites)
        ));
    }
    let mut ack = NativeBootstrapAck::new(&v, &r, 0x1000, 0x1005, 0x1008).unwrap();
    exited(&mut v);
    r.r14 += 64;
    let before = *v.bytes();
    assert_eq!(ack.acknowledge(&mut v, &r), Err(E::StateMismatch));
    assert_eq!(v.bytes(), &before);
}
