use svmvisor_hypervisor::{firmware_xstate::*, xstate::*};
fn evidence(mask: u64) -> FirmwareXstateEvidence {
    FirmwareXstateEvidence {
        max_basic_leaf: 0xd,
        capabilities: XstateCapabilities {
            leaf1_ecx: (1 << 26) | (1 << 27) | (1 << 28),
            leaf1_edx: 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26),
            supported_xcr0: 7,
            enabled_size: if mask == 3 { 576 } else { 832 },
            max_size: 832,
            avx_size: 256,
            avx_offset: 576,
            avx_flags: 0,
        },
        leaf_d1_eax: 0,
        supported_xss: 0,
        original: FirmwareXstateControls {
            cr0: 0x80010033,
            cr4: 0x40620,
            efer: 0xd00,
            xcr0: Some(mask),
            xss: None,
        },
    }
}
#[test]
fn original_sse_profile_is_preserved_even_on_avx_capable_cpu() {
    let input = evidence(3);
    let plan = FirmwareXstatePlan::validate(input).unwrap();
    assert_eq!(plan.layout().mask(), 3);
    assert_eq!(plan.layout().size(), 576);
    assert_eq!(plan.original_controls(), input.original);
    assert_eq!(plan.capture_controls(), input.original);
    assert!(
        plan.obligations()
            .qualify_x87_exception_pointer_preservation
    );
    assert!(plan.obligations().capture_before_simd_or_fpu_use);
}
#[test]
fn original_avx_profile_keeps_every_enabled_component() {
    let input = evidence(7);
    let plan = FirmwareXstatePlan::validate(input).unwrap();
    assert_eq!(plan.layout().mask(), 7);
    assert_eq!(plan.layout().size(), 832);
    for mask in [0, 1, 2, 4, 5, 6, 0xe7, 0x207, 1u64 << 63] {
        let mut bad = input;
        bad.original.xcr0 = Some(mask);
        assert_eq!(
            FirmwareXstatePlan::validate(bad),
            Err(FirmwareXstateError::UnsupportedOriginalXcr0)
        );
    }
}
#[test]
fn temporary_enablement_changes_are_exact_and_must_be_undone() {
    let mut input = evidence(7);
    input.original.cr0 |= 12;
    input.original.efer |= 1 << 14;
    let plan = FirmwareXstatePlan::validate(input).unwrap();
    let active = plan.capture_controls();
    assert_eq!(active.cr0, input.original.cr0 & !12);
    assert_eq!(active.efer, input.original.efer & !(1 << 14));
    assert_eq!(active.cr4, input.original.cr4);
    assert_eq!(active.xcr0, Some(7));
    assert_eq!(
        plan.validate_restored_controls(active),
        Err(FirmwareXstateError::OriginalControlsNotRestored)
    );
    assert_eq!(plan.validate_restored_controls(input.original), Ok(()));
    for bit in 0..64 {
        for field in 0..3 {
            let mut bad = input.original;
            match field {
                0 => bad.cr0 ^= 1 << bit,
                1 => bad.cr4 ^= 1 << bit,
                _ => bad.efer ^= 1 << bit,
            };
            assert_eq!(
                plan.validate_restored_controls(bad),
                Err(FirmwareXstateError::OriginalControlsNotRestored)
            );
        }
    }
    let mut bad = input.original;
    bad.xcr0 = Some(3);
    assert!(plan.validate_restored_controls(bad).is_err());
    bad = input.original;
    bad.xss = Some(0);
    assert!(plan.validate_restored_controls(bad).is_err());
}
#[test]
fn legacy_fallback_requires_evidence_that_xsave_profile_is_absent() {
    let mut input = evidence(3);
    input.max_basic_leaf = 1;
    input.capabilities.leaf1_ecx = 0;
    input.capabilities.supported_xcr0 = 0;
    input.capabilities.enabled_size = 0;
    input.capabilities.max_size = 0;
    input.original.cr4 &= !(1 << 18);
    input.original.xcr0 = None;
    let plan = FirmwareXstatePlan::validate(input).unwrap();
    assert!(!plan.layout().uses_xsave());
    assert_eq!(plan.layout().size(), 512);
    input.original.xcr0 = Some(3);
    assert_eq!(
        FirmwareXstatePlan::validate(input),
        Err(FirmwareXstateError::InconsistentEnablement)
    );
}
#[test]
fn missing_cpuid_control_and_supervisor_observations_fail_closed() {
    let input = evidence(7);
    let mut bad = input;
    bad.max_basic_leaf = 0;
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::MissingCpuidEvidence)
    );
    bad = input;
    bad.max_basic_leaf = 0xc;
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::MissingCpuidEvidence)
    );
    bad = input;
    bad.original.xcr0 = None;
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::MissingOriginalXcr0)
    );
    bad = input;
    bad.original.cr4 &= !(1 << 18);
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::InconsistentEnablement)
    );
    bad = input;
    bad.capabilities.leaf1_ecx &= !(1 << 27);
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::InconsistentEnablement)
    );
    bad = input;
    bad.leaf_d1_eax = 8;
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::MissingOriginalXss)
    );
    bad.original.xss = Some(0);
    bad.supported_xss = 1 << 11;
    assert!(FirmwareXstatePlan::validate(bad).is_ok());
    bad.original.xss = Some(1 << 11);
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::SupervisorStateEnabled)
    );
    bad = input;
    bad.original.xss = Some(0);
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::InconsistentSupervisorEvidence)
    );
}
#[test]
fn unsupported_dynamic_state_and_control_modes_are_not_assumed_inactive() {
    for bit in 4..32 {
        let mut bad = evidence(7);
        bad.leaf_d1_eax |= 1 << bit;
        assert_eq!(
            FirmwareXstatePlan::validate(bad),
            Err(FirmwareXstateError::UnsupportedExtendedControls)
        );
    }
    for bit in 22..64 {
        let mut bad = evidence(7);
        bad.original.cr4 |= 1 << bit;
        assert_eq!(
            FirmwareXstatePlan::validate(bad),
            Err(FirmwareXstateError::UnsupportedExtendedControls)
        );
    }
    for (field, bit) in [(0, 31), (0, 0), (1, 5), (1, 9), (2, 8), (2, 10)] {
        let mut bad = evidence(7);
        match field {
            0 => bad.original.cr0 &= !(1 << bit),
            1 => bad.original.cr4 &= !(1 << bit),
            _ => bad.original.efer &= !(1 << bit),
        };
        assert_eq!(
            FirmwareXstatePlan::validate(bad),
            Err(FirmwareXstateError::UnsupportedControlState)
        );
    }
}
#[test]
fn current_enabled_size_is_validated_without_modifying_xcr0() {
    let mut bad = evidence(3);
    bad.capabilities.enabled_size = 832;
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::Layout(
            XstateError::EnabledSizeMismatch
        ))
    );
    bad = evidence(7);
    bad.capabilities.avx_offset = 4096;
    bad.capabilities.max_size = 65536;
    assert_eq!(
        FirmwareXstatePlan::validate(bad),
        Err(FirmwareXstateError::Layout(XstateError::AreaTooSmall))
    );
    bad = evidence(3);
    bad.capabilities.supported_xcr0 |= 0xe0;
    bad.capabilities.max_size = 65536;
    assert_eq!(
        FirmwareXstatePlan::validate(bad).unwrap().layout().mask(),
        3
    );
}
#[test]
fn original_image_validation_is_read_only_and_does_not_manufacture_capture_proof() {
    let plan = FirmwareXstatePlan::validate(evidence(7)).unwrap();
    let mut image = XstateArea::new();
    image.reset(plan.layout(), 0).unwrap();
    image.bytes_mut()[32] = 0x99;
    let before = *image.bytes();
    assert_eq!(plan.validate_original_image(&image, 0), Ok(()));
    assert_eq!(image.bytes(), &before);
    image.bytes_mut()[512] |= 0x80;
    assert_eq!(
        plan.validate_original_image(&image, 0),
        Err(FirmwareXstateError::Layout(XstateError::InvalidHeader))
    );
    image.reset(plan.layout(), 0).unwrap();
    image.bytes_mut()[27] = 0xff;
    assert_eq!(
        plan.validate_original_image(&image, 0),
        Err(FirmwareXstateError::Layout(XstateError::InvalidMxcsr))
    );
}
