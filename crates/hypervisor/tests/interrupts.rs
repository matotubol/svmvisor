use svmvisor_hypervisor::svm::{
    events::{
        ExternalInterruptError as Error, ExternalInterruptState as State, PendingExternalInterrupt,
        ReflectionError,
    },
    vmcb::{EventIntercept, Vmcb},
};

// Hardware-write fixture; storage is exclusively owned and no CPU runs it.
fn hardware_write(vmcb: &mut Vmcb, offset: usize, value: u64) {
    assert!(offset + 8 <= 4096);
    let ptr = (vmcb as *mut Vmcb).cast::<u8>();
    unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), ptr.add(offset), 8) };
}

fn stopped() -> Vmcb {
    let mut vmcb = Vmcb::new();
    for (offset, value) in [
        (0x070, 0x81),
        (0x090, 1),
        (0x0c0, 0xfeed_face_ffff_ffff),
        (0x570, 2), // IF clear; arming must retain the request for hardware.
        (0x578, 0xffff_8000_1234_5678),
        (0x5d8, 0xffff_8000_9999_0000),
        (0x5f8, 0xdead_beef),
    ] {
        hardware_write(&mut vmcb, offset, value);
    }
    vmcb
}

fn armed() -> (Vmcb, PendingExternalInterrupt) {
    let mut vmcb = stopped();
    let mut request = PendingExternalInterrupt::new(0x51).unwrap();
    vmcb.arm_external_interrupt(&mut request).unwrap();
    (vmcb, request)
}

#[test]
fn irq_encoding_and_virtual_intercept_are_byte_exact() {
    for vector in [32, 0x51, 0xff] {
        let mut vmcb = stopped();
        vmcb.set_virtual_interrupt_tpr(6).unwrap();
        let mut request = PendingExternalInterrupt::new(vector).unwrap();
        assert_eq!(request.vector(), vector);
        let mut expected = *vmcb.bytes();
        expected[0x060..0x068].copy_from_slice(&[6, 1, vector >> 4, 1, vector, 0, 0, 0]);
        expected[0x0c0..0x0c4].fill(0);
        vmcb.arm_external_interrupt(&mut request).unwrap();
        assert_eq!(request.state(), State::Armed);
        assert_eq!(vmcb.bytes(), &expected);
        vmcb.set_event_intercept(EventIntercept::VirtualInterrupt, true);
        expected[0x00c] |= 1 << 4;
        assert_eq!(vmcb.bytes(), &expected);
        assert!(vmcb.event_intercept(EventIntercept::VirtualInterrupt));
        vmcb.set_event_intercept(EventIntercept::VirtualInterrupt, false);
        expected[0x00c] &= !(1 << 4);
        assert_eq!(vmcb.bytes(), &expected);
    }
}

#[test]
fn vector_and_tpr_validation_preserve_all_bytes() {
    for vector in 0..32 {
        assert_eq!(
            PendingExternalInterrupt::new(vector),
            Err(Error::ReservedVector { vector })
        );
    }
    let (mut vmcb, _) = armed();
    for priority in 16..=255 {
        let before = *vmcb.bytes();
        assert_eq!(
            vmcb.set_virtual_interrupt_tpr(priority),
            Err(Error::InvalidTaskPriority { priority })
        );
        assert_eq!(vmcb.bytes(), &before);
    }
    for priority in [15, 6, 5, 4, 0] {
        let mut expected = *vmcb.bytes();
        expected[0x060] = priority;
        expected[0x0c0..0x0c4].fill(0);
        vmcb.set_virtual_interrupt_tpr(priority).unwrap();
        assert_eq!(vmcb.bytes(), &expected);
    }
}

#[test]
fn arming_conflicts_preserve_request_and_entire_vmcb() {
    let cases = [
        (0x0a8, 0x8000_0306, Error::PendingInjection),
        (0x088, 0x8000_0051, Error::NestedDeliveryUnsupported),
        (0x060, 1 << 8, Error::PendingVirtualInterrupt),
        (
            0x060,
            1 << 20,
            Error::UnsupportedControl { control: 1 << 20 },
        ),
        (0x090, 3, Error::UnsupportedNestedControl { control: 3 }),
    ];
    for (offset, value, error) in cases {
        let mut vmcb = stopped();
        let mut request = PendingExternalInterrupt::new(0x51).unwrap();
        hardware_write(&mut vmcb, offset, value);
        let before = *vmcb.bytes();
        assert_eq!(vmcb.arm_external_interrupt(&mut request), Err(error));
        assert_eq!(vmcb.bytes(), &before);
        assert_eq!(request.state(), State::Queued);
    }
}

#[test]
fn unsupported_control_bits_are_never_erased_by_arming_or_tpr_edit() {
    let supported = 0xf | (1u64 << 8) | (0xf << 16) | (1 << 24) | (0xff << 32);
    for bit in 0..64 {
        let control = 1u64 << bit;
        if control & supported != 0 {
            continue;
        }
        let mut vmcb = stopped();
        hardware_write(&mut vmcb, 0x060, control);
        let before = *vmcb.bytes();
        let mut request = PendingExternalInterrupt::new(0x51).unwrap();
        assert_eq!(
            vmcb.arm_external_interrupt(&mut request),
            Err(Error::UnsupportedControl { control })
        );
        assert_eq!(
            vmcb.set_virtual_interrupt_tpr(0),
            Err(Error::UnsupportedControl { control })
        );
        assert_eq!(vmcb.bytes(), &before);
        assert_eq!(request.state(), State::Queued);
    }
}

#[test]
fn blocked_and_window_exits_keep_one_request_until_hardware_consumes_it() {
    let (mut vmcb, mut request) = armed();
    // Stopped IF/shadow/priority are diagnostic, not permission for software to
    // discard/reinject. Hardware owns masking; same V_IRQ survives every exit.
    for (flags, shadow, tpr, code) in [
        (2, 0, 0, 0x81),
        (0x202, 1, 0, 0x81),
        (0x202, 0, 6, 0x81),
        (0x202, 0, 4, 0x64),
    ] {
        hardware_write(&mut vmcb, 0x570, flags);
        hardware_write(&mut vmcb, 0x068, shadow);
        hardware_write(&mut vmcb, 0x070, code);
        vmcb.set_virtual_interrupt_tpr(tpr).unwrap();
        let before = *vmcb.bytes();
        assert_eq!(vmcb.interrupt_shadow(), shadow != 0);
        assert_eq!(
            vmcb.observe_external_interrupt_after_exit(&mut request),
            Ok(State::Armed)
        );
        assert_eq!(
            vmcb.arm_external_interrupt(&mut request),
            Err(Error::RequestNotQueued)
        );
        assert_eq!(vmcb.bytes(), &before);
    }
    let control = vmcb.virtual_interrupt_control() & !(1 << 8);
    hardware_write(&mut vmcb, 0x060, control);
    hardware_write(&mut vmcb, 0x070, 0x81);
    let before = *vmcb.bytes();
    assert_eq!(
        vmcb.observe_external_interrupt_after_exit(&mut request),
        Ok(State::Consumed)
    );
    assert_eq!(
        vmcb.observe_external_interrupt_after_exit(&mut request),
        Err(Error::RequestNotArmed)
    );
    assert_eq!(
        vmcb.arm_external_interrupt(&mut request),
        Err(Error::RequestNotQueued)
    );
    assert_eq!(vmcb.bytes(), &before);
}

#[test]
fn consumed_bit_never_hides_failed_or_interrupted_delivery_or_competing_event() {
    for (offset, value, error) in [
        (0x070, u64::MAX, Error::InvalidEntry),
        (0x088, 0x8000_0051, Error::NestedDeliveryUnsupported),
        (0x0a8, 0x8000_0306, Error::PendingInjection),
        (0x070, 0x64, Error::InconsistentVirtualInterruptExit),
    ] {
        let (mut vmcb, mut request) = armed();
        let control = vmcb.virtual_interrupt_control() & !(1 << 8);
        hardware_write(&mut vmcb, 0x060, control);
        hardware_write(&mut vmcb, offset, value);
        let before = *vmcb.bytes();
        assert_eq!(
            vmcb.observe_external_interrupt_after_exit(&mut request),
            Err(error)
        );
        assert_eq!(request.state(), State::Armed);
        assert_eq!(vmcb.bytes(), &before);
    }
}

#[test]
fn observation_refuses_changed_vector_priority_masking_and_advanced_modes() {
    for bit in [16, 24, 32, 9, 20, 25, 26, 31, 63] {
        let (mut vmcb, mut request) = armed();
        let control = vmcb.virtual_interrupt_control() ^ (1u64 << bit);
        hardware_write(&mut vmcb, 0x060, control);
        let before = *vmcb.bytes();
        let result = vmcb.observe_external_interrupt_after_exit(&mut request);
        assert!(matches!(
            result,
            Err(Error::ControlMismatch | Error::UnsupportedControl { .. })
        ));
        assert_eq!(request.state(), State::Armed);
        assert_eq!(vmcb.bytes(), &before);
    }
    let (mut vmcb, mut request) = armed();
    hardware_write(&mut vmcb, 0x090, 5);
    let before = *vmcb.bytes();
    assert_eq!(
        vmcb.observe_external_interrupt_after_exit(&mut request),
        Err(Error::UnsupportedNestedControl { control: 5 })
    );
    assert_eq!(request.state(), State::Armed);
    assert_eq!(vmcb.bytes(), &before);
}

#[test]
fn reflection_refuses_reverse_conflict_with_an_armed_irq() {
    let (mut vmcb, request) = armed();
    hardware_write(&mut vmcb, 0x070, 0x4e);
    hardware_write(&mut vmcb, 0x080, 0xfeed_beef);
    let before = *vmcb.bytes();
    assert_eq!(
        vmcb.reflect_exception(),
        Err(ReflectionError::PendingVirtualInterrupt)
    );
    assert_eq!(vmcb.bytes(), &before);
    assert_eq!(request.state(), State::Armed);
}
