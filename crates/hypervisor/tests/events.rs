use svmvisor_hypervisor::svm::{
    events::{ReflectedException, ReflectionError},
    vmcb::Vmcb,
};

// Emulate hardware writes to the uniquely owned aligned storage. No CPU runs
// against this fixture; the pointer originates from &mut, not the byte view.
fn hardware_write(vmcb: &mut Vmcb, offset: usize, value: u64) {
    assert!(offset + 8 <= 4096);
    let ptr = (vmcb as *mut Vmcb).cast::<u8>();
    unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), ptr.add(offset), 8) };
}

fn stopped(code: u64, info1: u64, info2: u64) -> Vmcb {
    let mut vmcb = Vmcb::new();
    for (offset, value) in [
        (0x070, code),
        (0x078, info1),
        (0x080, info2),
        (0x578, 0xffff_8000_1234_5678),
        (0x5d8, 0xffff_8000_9999_0000),
        (0x5f8, 0xdead_beef),
        (0x640, 0x1122_3344),
        (0x0c0, 0xfeed_face_ffff_ffff),
    ] {
        hardware_write(&mut vmcb, offset, value);
    }
    vmcb
}

#[test]
fn exact_ud_encoding_ignores_undefined_info_and_preserves_every_other_byte() {
    let mut vmcb = stopped(0x46, u64::MAX, u64::MAX);
    let mut expected = *vmcb.bytes();
    expected[0x0a8..0x0b0].copy_from_slice(&[6, 3, 0, 0x80, 0, 0, 0, 0]);
    expected[0x0c0..0x0c4].fill(0);
    assert_eq!(
        vmcb.reflect_exception(),
        Ok(ReflectedException::InvalidOpcode)
    );
    assert_eq!(vmcb.bytes(), &expected);
}

#[test]
fn exact_gp_encoding_keeps_selector_error_and_fault_rip() {
    for error in [0u32, 0x1237, 0xffff] {
        let mut vmcb = stopped(0x4d, error as u64, u64::MAX);
        let mut expected = *vmcb.bytes();
        expected[0x0a8..0x0ac].copy_from_slice(&[13, 11, 0, 0x80]);
        expected[0x0ac..0x0b0].copy_from_slice(&error.to_le_bytes());
        expected[0x0c0..0x0c4].fill(0);
        assert_eq!(
            vmcb.reflect_exception(),
            Ok(ReflectedException::GeneralProtection { error_code: error })
        );
        assert_eq!(vmcb.bytes(), &expected);
    }
}

#[test]
fn pf_encoding_updates_cr2_from_fault_address_and_preserves_rip() {
    for error in [0u32, 0x17, 0x7f] {
        let address: u64 = 0xffff_aaaa_bbbb_cdef;
        let mut vmcb = stopped(0x4e, error as u64, address);
        let mut expected = *vmcb.bytes();
        expected[0x0a8..0x0ac].copy_from_slice(&[14, 11, 0, 0x80]);
        expected[0x0ac..0x0b0].copy_from_slice(&error.to_le_bytes());
        expected[0x640..0x648].copy_from_slice(&address.to_le_bytes());
        expected[0x0c0..0x0c4].fill(0);
        assert_eq!(
            vmcb.reflect_exception(),
            Ok(ReflectedException::PageFault {
                error_code: error,
                address
            })
        );
        assert_eq!(vmcb.bytes(), &expected);
        assert_eq!(vmcb.guest_cr2(), address);
    }
}

#[test]
fn all_refusals_are_transactional_even_for_pf_cr2_and_clean_bits() {
    let cases = [
        (0x4e, 0, 0x8000_0306, 0, ReflectionError::PendingInjection),
        (
            0x4e,
            0,
            0,
            0x8000_0b08,
            ReflectionError::NestedDeliveryUnsupported,
        ),
        (
            0x48,
            0,
            0,
            0,
            ReflectionError::UnsupportedExit { code: 0x48 },
        ),
        (
            0x400,
            0,
            0,
            0,
            ReflectionError::UnsupportedExit { code: 0x400 },
        ),
        (
            0x1_0000_0046,
            0,
            0,
            0,
            ReflectionError::UnsupportedExit {
                code: 0x1_0000_0046,
            },
        ),
        (
            0x4d,
            0x10000,
            0,
            0,
            ReflectionError::InvalidGeneralProtectionError {
                error_code: 0x10000,
            },
        ),
        (
            0x4d,
            1 << 32,
            0,
            0,
            ReflectionError::InvalidGeneralProtectionError {
                error_code: 1 << 32,
            },
        ),
        (
            0x4e,
            0x80,
            0,
            0,
            ReflectionError::UnsupportedPageFaultError { error_code: 0x80 },
        ),
        (
            0x4e,
            1 << 31,
            0,
            0,
            ReflectionError::UnsupportedPageFaultError {
                error_code: 1 << 31,
            },
        ),
        (
            0x4e,
            1 << 32,
            0,
            0,
            ReflectionError::UnsupportedPageFaultError {
                error_code: 1 << 32,
            },
        ),
    ];
    for (code, error, injection, interrupted, refusal) in cases {
        let mut vmcb = stopped(code, error, 0xcafe_f00d);
        hardware_write(&mut vmcb, 0x0a8, injection);
        hardware_write(&mut vmcb, 0x088, interrupted);
        let before = *vmcb.bytes();
        assert_eq!(vmcb.reflect_exception(), Err(refusal));
        assert_eq!(vmcb.bytes(), &before);
    }
}

#[test]
fn queued_injection_requires_explicit_completed_exit_retirement() {
    let mut vmcb = stopped(0x46, 0, 0);
    vmcb.reflect_exception().unwrap();
    let before = *vmcb.bytes();
    assert_eq!(
        vmcb.reflect_exception(),
        Err(ReflectionError::PendingInjection)
    );
    assert_eq!(vmcb.bytes(), &before);
    hardware_write(&mut vmcb, 0x070, 0x81); // guest handler VMMCALL
    vmcb.clear_event_injection_after_exit().unwrap();
    assert_eq!(vmcb.event_injection(), 0);
    hardware_write(&mut vmcb, 0x070, 0x4d);
    assert_eq!(vmcb.reflect_exception().unwrap().vector(), 13);
}

#[test]
fn retirement_refuses_failed_entry_and_interrupted_delivery_without_mutation() {
    for (code, interrupted, refusal) in [
        (u64::MAX, 0, ReflectionError::InvalidEntry),
        (
            0x4b,
            0x8000_0b0e,
            ReflectionError::NestedDeliveryUnsupported,
        ),
    ] {
        let mut vmcb = stopped(code, 0, 0);
        hardware_write(&mut vmcb, 0x0a8, 0x8000_0b0e);
        hardware_write(&mut vmcb, 0x088, interrupted);
        let before = *vmcb.bytes();
        assert_eq!(vmcb.clear_event_injection_after_exit(), Err(refusal));
        assert_eq!(vmcb.bytes(), &before);
    }
}

#[test]
fn invalid_event_fields_are_ignored_and_replaced_with_reserved_bits_clear() {
    let mut vmcb = stopped(0x46, 0, 0);
    hardware_write(&mut vmcb, 0x088, !(1 << 31));
    hardware_write(&mut vmcb, 0x0a8, !(1 << 31));
    vmcb.reflect_exception().unwrap();
    assert_eq!(vmcb.event_injection(), 0x8000_0306);
}
