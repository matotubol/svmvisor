use svmvisor_hypervisor::svm::vmcb::Vmcb;
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn stopped(store: bool) -> Vmcb {
    let mut v = Vmcb::new();
    for (offset, value) in [
        (0x70, 0x7c),
        (0x78, store as u64),
        (0x578, 0x1000),
        (0x5f8, 0x1122_3344_5566_7788),
        (0x570, 0x202),
        (0x640, 0xabcdef),
        (0xc0, u64::MAX),
    ] {
        write(&mut v, offset, value);
    }
    v
}
#[test]
fn gp_zero_encoding_changes_only_injection_and_clean_bits_preserving_fault_rip() {
    for store in [false, true] {
        let mut v = stopped(store);
        let old = *v.bytes();
        v.queue_msr_general_protection(if store { &[0x0f, 0x30] } else { &[0x0f, 0x32] })
            .unwrap();
        assert_eq!(v.event_injection(), 0x8000_0b0d);
        for i in 0..4096 {
            if !(0xa8..0xb0).contains(&i) && !(0xc0..0xc8).contains(&i) {
                assert_eq!(v.bytes()[i], old[i]);
            }
        }
        let queued = *v.bytes();
        assert!(v.queue_msr_general_protection(&[0x0f, 0x30]).is_err());
        assert_eq!(v.bytes(), &queued);
    }
}
#[test]
fn wrong_exit_direction_bytes_and_noncanonical_fault_rip_refuse_transactionally() {
    for (offset, value) in [
        (0x70, u64::MAX),
        (0x70, 0x81),
        (0x78, 2),
        (0x578, 0x0000_8000_0000_0000),
    ] {
        let mut v = stopped(true);
        write(&mut v, offset, value);
        let old = *v.bytes();
        assert!(v.queue_msr_general_protection(&[0x0f, 0x30]).is_err());
        assert_eq!(v.bytes(), &old);
    }
    let mut v = stopped(true);
    let old = *v.bytes();
    assert!(v.queue_msr_general_protection(&[0x0f, 0x32]).is_err());
    assert_eq!(v.bytes(), &old);
}
#[test]
fn pending_and_unsupported_state_cannot_be_overwritten() {
    for (offset, value) in [
        (0xa8, 1 << 31),
        (0x88, 1 << 31),
        (0x60, 1 << 8),
        (0x60, 1 << 31),
        (0x90, 3),
    ] {
        let mut v = stopped(false);
        write(&mut v, offset, value);
        let old = *v.bytes();
        assert!(v.queue_msr_general_protection(&[0x0f, 0x32]).is_err());
        assert_eq!(v.bytes(), &old);
    }
}

#[test]
fn fault_at_canonical_boundary_does_not_require_sequential_continuation() {
    let mut v = stopped(true);
    write(&mut v, 0x578, 0x7fff_ffff_fffe);
    v.queue_msr_general_protection(&[0x0f, 0x30]).unwrap();
    assert_eq!(v.guest_rip(), 0x7fff_ffff_fffe);
    assert_eq!(v.event_injection(), 0x8000_0b0d);
}
