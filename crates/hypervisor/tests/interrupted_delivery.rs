use svmvisor_hypervisor::svm::{
    events::{
        DeliveryOutcome, ExternalInterruptError, GuestShutdown, ReflectedException, ReflectionError,
    },
    vmcb::{EventIntercept, Vmcb},
};

// Inert hardware-write fixtures only; actual IDT entry/exit belongs to the harness.
fn write(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

fn event(vector: u8, error: u32) -> u64 {
    (1 << 31)
        | (3 << 8)
        | vector as u64
        | if vector == 6 { 0 } else { (1 << 11) | ((error as u64) << 32) }
}

fn stopped(prior: u8, current: u8) -> Vmcb {
    let mut v = Vmcb::new();
    for (offset, value) in [
        (0x070, 0x40 + current as u64),
        (0x078, 0x12),
        (0x080, 0xffff_ffff_1234_5000),
        (0x088, event(prior, 0)),
        (0x0a8, event(prior, 0)),
        (0x0c0, u64::MAX),
        (0x570, 0x10202),
        (0x578, 0x8000),
        (0x5d8, 0xc000),
        (0x5f8, 0xaaaa),
        (0x640, 0xdead_beef),
    ] {
        write(&mut v, offset, value);
    }
    v
}

#[test]
fn all_sixteen_admitted_combinations_preserve_unrelated_state_and_pf_side_effects() {
    for prior in [6, 8, 13, 14] {
        for current in [11, 12, 13, 14] {
            let mut v = stopped(prior, current);
            let mut expected = *v.bytes();
            let combined = match (prior, current) {
                (8, _) => None,
                (14, _) | (13, 11..=13) => Some(ReflectedException::DoubleFault),
                (_, 11) => Some(ReflectedException::SegmentNotPresent { error_code: 0x12 }),
                (_, 12) => Some(ReflectedException::StackFault { error_code: 0x12 }),
                (_, 13) => Some(ReflectedException::GeneralProtection { error_code: 0x12 }),
                (_, 14) => Some(ReflectedException::PageFault {
                    error_code: 0x12,
                    address: 0xffff_ffff_1234_5000,
                }),
                _ => unreachable!(),
            };
            let outcome = if let Some(combined) = combined {
                let error = if combined == ReflectedException::DoubleFault { 0 } else { 0x12 };
                expected[0x0a8..0x0b0]
                    .copy_from_slice(&event(combined.vector(), error).to_le_bytes());
                expected[0x0c0..0x0c4].fill(0);
                if current == 14 {
                    expected[0x640..0x648].copy_from_slice(&0xffff_ffff_1234_5000u64.to_le_bytes());
                }
                DeliveryOutcome::Injected(combined)
            } else {
                DeliveryOutcome::Shutdown(GuestShutdown::ExceptionDelivery {
                    interrupted_vector: prior,
                    fault_vector: current,
                })
            };
            assert_eq!(
                v.resolve_exception_delivery_after_exit(),
                Ok(outcome),
                "{prior}->{current}"
            );
            assert_eq!(v.bytes(), &expected, "{prior}->{current}");
        }
    }
}

#[test]
fn prior_request_retirement_and_nested_resolution_are_distinct_actual_exit_boundaries() {
    let mut v = stopped(13, 11);
    let before = *v.bytes();
    assert_eq!(
        v.clear_event_injection_after_exit(),
        Err(ReflectionError::NestedDeliveryUnsupported)
    );
    assert_eq!(v.bytes(), &before);
    assert_eq!(
        v.resolve_exception_delivery_after_exit(),
        Ok(DeliveryOutcome::Injected(ReflectedException::DoubleFault))
    );
    let queued = *v.bytes();
    assert_eq!(
        v.resolve_exception_delivery_after_exit(),
        Err(ReflectionError::PriorInjectionMismatch)
    );
    assert_eq!(v.bytes(), &queued);
    // A real secondary #NP during the NEXT #DF injection is terminal; no fake
    // handler completion, request retirement, RIP movement or CR2 update.
    write(&mut v, 0x088, event(8, 0));
    let terminal = *v.bytes();
    assert_eq!(
        v.resolve_exception_delivery_after_exit(),
        Ok(DeliveryOutcome::Shutdown(GuestShutdown::ExceptionDelivery {
            interrupted_vector: 8,
            fault_vector: 11,
        }))
    );
    assert_eq!(v.bytes(), &terminal);
}

#[test]
fn completed_replacement_handler_allows_retirement_only_after_exitintinfo_clears() {
    let mut v = stopped(6, 11);
    v.resolve_exception_delivery_after_exit().unwrap();
    assert_eq!(v.event_injection(), event(11, 0x12));
    write(&mut v, 0x070, 0x81); // actual handler checkpoint
    write(&mut v, 0x088, 0);
    v.clear_event_injection_after_exit().unwrap();
    assert_eq!(v.event_injection(), 0);
    assert_eq!(v.guest_rip(), 0x8000); // pure host test does not execute handler
}

#[test]
fn undefined_error_payload_and_hardware_cleared_injection_validity_are_ignored() {
    for injection in [event(6, 0), 0xffff_ffff_7fff_ffff, 0x1234_5678_8000_0306] {
        let mut v = stopped(6, 11);
        write(&mut v, 0x088, event(6, 0) | (0xffff_ffff << 32));
        write(&mut v, 0x0a8, injection);
        assert_eq!(
            v.resolve_exception_delivery_after_exit(),
            Ok(DeliveryOutcome::Injected(ReflectedException::SegmentNotPresent {
                error_code: 0x12
            }))
        );
        assert_eq!(v.event_injection(), event(11, 0x12));
    }
}

#[test]
fn unsupported_and_malformed_nested_cases_are_byte_exact_refusals() {
    let cases = [
        (0x070, u64::MAX, ReflectionError::InvalidEntry),
        (0x070, 0x400, ReflectionError::UnsupportedExit { code: 0x400 }),
        (0x070, 0x46, ReflectionError::UnsupportedExit { code: 0x46 }),
        (0x070, 0x48, ReflectionError::UnsupportedExit { code: 0x48 }),
        (0x070, 0x1_0000_004b, ReflectionError::UnsupportedExit { code: 0x1_0000_004b }),
        (0x088, 0, ReflectionError::NoInterruptedDelivery),
        (
            0x088,
            event(6, 0) | 0x1000,
            ReflectionError::InvalidInterruptedEvent { event: event(6, 0) | 0x1000 },
        ),
        (
            0x088,
            event(6, 0) | (1 << 11),
            ReflectionError::InvalidInterruptedEvent { event: event(6, 0) | (1 << 11) },
        ),
        (
            0x088,
            event(13, 0) & !(1 << 11),
            ReflectionError::InvalidInterruptedEvent { event: event(13, 0) & !(1 << 11) },
        ),
        (0x088, event(8, 1), ReflectionError::InvalidInterruptedEvent { event: event(8, 1) }),
        (
            0x088,
            event(13, 0x10000),
            ReflectionError::InvalidInterruptedEvent { event: event(13, 0x10000) },
        ),
        (
            0x088,
            event(14, 0x80),
            ReflectionError::InvalidInterruptedEvent { event: event(14, 0x80) },
        ),
        (0x0a8, event(13, 0), ReflectionError::PriorInjectionMismatch),
        (0x060, 1 << 8, ReflectionError::PendingVirtualInterrupt),
        (
            0x060,
            1 << 31,
            ReflectionError::Control(ExternalInterruptError::UnsupportedControl {
                control: 1 << 31,
            }),
        ),
        (
            0x090,
            2,
            ReflectionError::Control(ExternalInterruptError::UnsupportedNestedControl {
                control: 2,
            }),
        ),
        (0x078, 0x10000, ReflectionError::InvalidSelectorError { vector: 11, error_code: 0x10000 }),
    ];
    for (offset, value, error) in cases {
        let mut v = stopped(6, 11);
        write(&mut v, offset, value);
        let bytes = *v.bytes();
        assert_eq!(
            v.resolve_exception_delivery_after_exit(),
            Err(error),
            "offset {offset:x} value {value:x}"
        );
        assert_eq!(v.bytes(), &bytes);
    }
    for interrupted in [
        event(0, 0),
        event(10, 0),
        event(11, 0),
        event(12, 0),
        0x8000_0050,
        0x8000_0202,
        0x8000_0480,
    ] {
        let mut v = stopped(6, 14);
        write(&mut v, 0x088, interrupted);
        let bytes = *v.bytes();
        assert_eq!(
            v.resolve_exception_delivery_after_exit(),
            Err(ReflectionError::UnsupportedInterruptedEvent { event: interrupted })
        );
        assert_eq!(v.bytes(), &bytes);
    }
}

#[test]
fn malformed_secondary_fault_errors_preserve_cr2_and_pending_request() {
    for (current, error) in [
        (12, ReflectionError::InvalidSelectorError { vector: 12, error_code: 1 << 32 }),
        (13, ReflectionError::InvalidGeneralProtectionError { error_code: 1 << 32 }),
        (14, ReflectionError::UnsupportedPageFaultError { error_code: 1 << 32 }),
    ] {
        let mut v = stopped(14, current);
        write(&mut v, 0x078, 1 << 32);
        let bytes = *v.bytes();
        assert_eq!(v.resolve_exception_delivery_after_exit(), Err(error));
        assert_eq!(v.bytes(), &bytes);
    }
}

#[test]
fn actual_shutdown_is_terminal_without_interpreting_undefined_saved_fields() {
    let mut v = stopped(6, 11);
    for offset in [0x060, 0x078, 0x080, 0x088, 0x090, 0x0a8, 0x578, 0x5d8] {
        write(&mut v, offset, u64::MAX);
    }
    write(&mut v, 0x070, 0x7f);
    let bytes = *v.bytes();
    assert_eq!(
        v.resolve_exception_delivery_after_exit(),
        Ok(DeliveryOutcome::Shutdown(GuestShutdown::Intercepted))
    );
    assert_eq!(v.clear_event_injection_after_exit(), Err(ReflectionError::GuestShutdown));
    assert_eq!(v.reflect_exception(), Err(ReflectionError::GuestShutdown));
    assert_eq!(v.bytes(), &bytes);
}

#[test]
fn shutdown_intercept_sets_only_its_architectural_bit_and_clean_bits() {
    let mut v = Vmcb::new();
    write(&mut v, 0x008, 0x0123_4567_aaaa_bbbb);
    write(&mut v, 0x0c0, u64::MAX);
    let mut expected = *v.bytes();
    expected[0x00f] |= 0x80;
    expected[0x0c0..0x0c4].fill(0);
    v.set_event_intercept(EventIntercept::Shutdown, true);
    assert!(v.event_intercept(EventIntercept::Shutdown));
    assert_eq!(v.bytes(), &expected);
    v.set_event_intercept(EventIntercept::Shutdown, false);
    assert!(!v.event_intercept(EventIntercept::Shutdown));
}
