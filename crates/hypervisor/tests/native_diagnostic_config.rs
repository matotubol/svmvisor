use svmvisor_hypervisor::svm::{
    native_diagnostic_config::{ConfigError, prepare_io},
    vmcb::Vmcb,
};
fn put(v: &mut Vmcb, o: usize, x: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            x.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(o),
            8,
        )
    }
}
fn stopped(port: u16, size: u8, input: bool) -> Vmcb {
    let mut v = Vmcb::new();
    for (o, x) in [
        (0x70, 0x7b),
        (0x78, (u64::from(port) << 16) | (u64::from(size) << 4) | u64::from(input)),
        (0x80, 0x2002),
        (0xc8, 0xdead),
        (0x410, 0x29bu64 << 16),
        (0x4d0, 0x1500),
        (0x558, 0x80000001),
        (0x548, 0x20),
        (0x550, 0x1000),
        (0x578, 0x2000),
        (0x570, 0x10002),
        (0x5f8, 0xaabbccddeeff1122),
    ] {
        put(&mut v, o, x)
    }
    v
}
#[test]
fn configuration_data_writes_revoke_before_hardware_and_reads_preserve_width() {
    for (port, width) in [(0xcf8, 4), (0xcfc, 1), (0xcfd, 1), (0xcfe, 2), (0xcfc, 4)] {
        for input in [false, true] {
            let mut v = stopped(port, width, input);
            let p = prepare_io(&mut v).unwrap();
            assert_eq!(p.revoke(), !input && port != 0xcf8);
            assert_eq!(p.port(), port);
            assert_eq!(p.width_bytes(), width);
            p.commit(0x87654321);
            assert_eq!(v.guest_rip(), 0x2002);
            assert_eq!(
                v.guest_rax(),
                if !input {
                    0xaabbccddeeff1122
                } else {
                    match width {
                        1 => 0xaabbccddeeff1121,
                        2 => 0xaabbccddeeff4321,
                        _ => 0x87654321,
                    }
                }
            );
        }
    }
}
#[test]
fn every_configuration_data_lane_revokes_without_selector_or_bdf_assumptions() {
    for port in 0xcfc..=0xcff {
        let mut v = stopped(port, 1, false);
        assert!(prepare_io(&mut v).unwrap().revoke());
    }
}

#[test]
fn malformed_or_unsupported_never_changes_stopped_state() {
    for (port, width) in [(0xcf4, 4), (0xcf6, 2), (0xd00, 4), (0xffff, 4)] {
        let mut v = stopped(port, width, false);
        let before = *v.bytes();
        assert!(matches!(prepare_io(&mut v), Err(ConfigError::PortOrWidth)));
        assert_eq!(*v.bytes(), before);
    }
    for (o, x) in [
        (0x78, (0xcfcu64 << 16) | 0x44),
        (0x80, 0x2000),
        (0x80, 0x2010),
        (0x570, 0x102),
        (0x4c8, 3u64 << 24),
    ] {
        let mut v = stopped(0xcfc, 4, false);
        put(&mut v, o, x);
        let before = *v.bytes();
        assert!(prepare_io(&mut v).is_err());
        assert_eq!(*v.bytes(), before);
    }
}

#[test]
fn scalar_overlap_including_reset_preserves_original_port_width_and_value() {
    for (port, width) in [
        (0xcf5, 4),
        (0xcf7, 2),
        (0xcf8, 1),
        (0xcf8, 2),
        (0xcf9, 1),
        (0xcf9, 4),
        (0xcfa, 2),
        (0xcfd, 2),
        (0xcfe, 4),
        (0xcff, 2),
    ] {
        let mut v = stopped(port, width, false);
        let before_rax = v.guest_rax();
        let p = prepare_io(&mut v).unwrap();
        assert_eq!(p.port(), port);
        assert_eq!(p.width_bytes(), width);
        assert!(p.revoke());
        assert_eq!(p.output_value(), before_rax as u32);
        p.commit(0);
        assert_eq!(v.guest_rax(), before_rax);
        assert_eq!(v.guest_rip(), 0x2002);
    }
}

#[test]
fn hwcr_configuration_fault_queues_gp_without_completing_io() {
    for input in [false, true] {
        let mut v = stopped(0xcf9, 1, input);
        let old_rax = v.guest_rax();
        let old_rip = v.guest_rip();
        let old_flags = u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap());
        prepare_io(&mut v).unwrap().fault_if_disabled().unwrap();
        assert_eq!(v.guest_rax(), old_rax);
        assert_eq!(v.guest_rip(), old_rip);
        assert_eq!(u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap()), old_flags);
        assert_eq!(v.event_injection(), 0x80000b0d);
    }
}
#[test]
fn dropping_prepared_transaction_has_no_side_effect() {
    let mut v = stopped(0xcfc, 4, false);
    let before = *v.bytes();
    let _ = prepare_io(&mut v).unwrap();
    assert_eq!(*v.bytes(), before);
}
// Every armed runtime carries the x2AVIC profile; a hardware-written V_IRQ rides along.
#[test]
fn armed_x2avic_profile_is_admitted_and_a_foreign_control_bit_is_not() {
    use svmvisor_hypervisor::svm::x2avic::NATIVE_CONTROL;
    let armed = |control: u64| {
        let mut v = stopped(0xcf9, 1, false);
        for (o, x) in
            [(0x08, 0xbu64 << 32), (0x60, control), (0x90, 1), (0xe0, 0x2000), (0xf8, 0x3000 | 37)]
        {
            put(&mut v, o, x)
        }
        v
    };
    let mut v = armed(NATIVE_CONTROL | (1 << 8) | 6);
    prepare_io(&mut v).unwrap().fault_if_disabled().unwrap();
    assert_eq!(v.event_injection(), 0x80000b0d);
    let mut v = armed(NATIVE_CONTROL | (1 << 25));
    let before = *v.bytes();
    assert!(matches!(prepare_io(&mut v), Err(ConfigError::Pending(_))));
    assert_eq!(*v.bytes(), before);
}
