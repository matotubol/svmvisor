use core::mem::{align_of, size_of};
use svmvisor_hypervisor::xstate::*;

fn capabilities() -> XstateCapabilities {
    XstateCapabilities {
        leaf1_ecx: (1 << 26) | (1 << 28),
        leaf1_edx: 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26),
        supported_xcr0: 7,
        enabled_size: 576,
        max_size: 832,
        avx_size: 256,
        avx_offset: 576,
        avx_flags: 0,
    }
}

#[test]
fn selects_avx_without_requiring_os_to_have_already_enabled_it() {
    let layout = XstateLayout::detect(capabilities()).unwrap();
    assert_eq!(
        (layout.mask(), layout.size(), layout.avx_offset()),
        (7, 832, Some(576))
    );
    assert!(layout.uses_xsave());
    assert_eq!(
        layout.validate_enabled_size(576),
        Err(XstateError::EnabledSizeMismatch)
    );
    assert_eq!(layout.validate_enabled_size(832), Ok(()));
}

#[test]
fn fx_fallback_ignores_absent_leaf_d() {
    let mut caps = capabilities();
    caps.leaf1_ecx = 0;
    caps.supported_xcr0 = 0;
    caps.max_size = 0;
    let layout = XstateLayout::detect(caps).unwrap();
    assert!(!layout.uses_xsave());
    assert_eq!(
        (layout.mask(), layout.size(), layout.avx_offset()),
        (3, 512, None)
    );
}

#[test]
fn xsave_without_avx_has_only_legacy_components_and_header() {
    let mut caps = capabilities();
    caps.leaf1_ecx &= !(1 << 28);
    caps.supported_xcr0 = 3;
    caps.max_size = 576;
    let layout = XstateLayout::detect(caps).unwrap();
    assert!(layout.uses_xsave());
    assert_eq!(
        (layout.mask(), layout.size(), layout.avx_offset()),
        (3, 576, None)
    );
}

#[test]
fn every_required_legacy_feature_and_component_is_checked() {
    for bit in [0, 23, 24, 25, 26] {
        let mut caps = capabilities();
        caps.leaf1_edx &= !(1 << bit);
        assert_eq!(
            XstateLayout::detect(caps),
            Err(XstateError::MissingLegacyFeatures)
        );
    }
    for bit in [0, 1, 2] {
        let mut caps = capabilities();
        caps.supported_xcr0 &= !(1 << bit);
        assert_eq!(
            XstateLayout::detect(caps),
            Err(XstateError::UnsupportedMask)
        );
    }
}

#[test]
fn selected_layout_is_bounded_without_rejecting_unselected_large_components() {
    let mut caps = capabilities();
    caps.supported_xcr0 |= 0xe0;
    caps.max_size = 65536;
    assert_eq!(XstateLayout::detect(caps).unwrap().size(), 832);
    caps.avx_offset = 4096 - 256;
    assert_eq!(XstateLayout::detect(caps).unwrap().size(), 4096);
    caps.avx_offset += 1;
    assert_eq!(XstateLayout::detect(caps), Err(XstateError::AreaTooSmall));
    caps.avx_offset = u32::MAX;
    assert_eq!(XstateLayout::detect(caps), Err(XstateError::InvalidLayout));
}

#[test]
fn overlapping_malformed_supervisor_and_truncated_layouts_fail() {
    for (offset, size, flags, max) in [
        (512, 256, 0, 832),
        (575, 256, 0, 832),
        (576, 255, 0, 832),
        (576, 257, 0, 833),
        (576, 256, 1, 832),
        (576, 256, 4, 832),
        (576, 256, 0, 831),
    ] {
        let mut caps = capabilities();
        caps.avx_offset = offset;
        caps.avx_size = size;
        caps.avx_flags = flags;
        caps.max_size = max;
        assert_eq!(XstateLayout::detect(caps), Err(XstateError::InvalidLayout));
    }
}

#[test]
fn avx_component_attributes_are_retained_only_for_an_admitted_component() {
    for flags in [0, 2] {
        let mut caps = capabilities();
        caps.avx_flags = flags;
        assert_eq!(XstateLayout::detect(caps).unwrap().avx_flags(), flags);
        caps.leaf1_ecx &= !(1 << 28);
        assert_eq!(XstateLayout::detect(caps).unwrap().avx_flags(), 0);
        caps.leaf1_ecx = 0;
        assert_eq!(XstateLayout::detect(caps).unwrap().avx_flags(), 0);
    }
}

#[test]
fn immutable_xcr0_profile_rejects_subsets_dependencies_and_extra_components() {
    let layout = XstateLayout::detect(capabilities()).unwrap();
    assert_eq!(layout.validate_xcr0(7), Ok(()));
    for mask in [0, 1, 3, 4, 5, 6, 0xff, 1 << 63] {
        assert_eq!(
            layout.validate_xcr0(mask),
            Err(XstateError::UnsupportedMask)
        );
    }
}

#[test]
fn guest_xsetbv_accepts_architectural_subsets_only_inside_admitted_components() {
    for avx in [false, true] {
        let mut caps = capabilities();
        if !avx {
            caps.leaf1_ecx &= !(1 << 28);
            caps.supported_xcr0 = 3;
            caps.max_size = 576;
        }
        let layout = XstateLayout::detect(caps).unwrap();
        for mask in 0..=255 {
            let expected = if mask == 1 || mask == 3 || (avx && mask == 7) {
                Ok(())
            } else {
                Err(XsetbvFault::GeneralProtection)
            };
            assert_eq!(
                layout.validate_guest_xcr0(1 << 18, 0, 0, mask),
                expected,
                "AVX={avx} XCR0={mask:#x}"
            );
        }
        for bit in 8..64 {
            assert_eq!(
                layout.validate_guest_xcr0(1 << 18, 0, 0, 3 | (1 << bit)),
                Err(XsetbvFault::GeneralProtection),
                "reserved or unowned component {bit}"
            );
        }
    }
}

#[test]
fn guest_xsetbv_faults_obey_availability_privilege_and_index() {
    let layout = XstateLayout::detect(capabilities()).unwrap();
    for cpl in 0..=3 {
        for ecx in [0, 1, 2, u32::MAX] {
            // Instruction unavailability takes priority over bad operands/CPL.
            assert_eq!(
                layout.validate_guest_xcr0(0, cpl, ecx, u64::MAX),
                Err(XsetbvFault::UndefinedOpcode)
            );
            assert_eq!(
                layout.validate_guest_xcr0(1 << 18, cpl, ecx, 3),
                if cpl == 0 && ecx == 0 {
                    Ok(())
                } else {
                    Err(XsetbvFault::GeneralProtection)
                }
            );
        }
    }
    let mut caps = capabilities();
    caps.leaf1_ecx = 0;
    let fx = XstateLayout::detect(caps).unwrap();
    // Setting OSXSAVE in supplied stopped state cannot invent XSAVE support.
    for cr4 in [0, 1 << 18] {
        assert_eq!(
            fx.validate_guest_xcr0(cr4, 0, 0, 1),
            Err(XsetbvFault::UndefinedOpcode)
        );
    }
}

#[test]
fn owned_area_reset_clears_prior_session_and_materializes_all_selected_state() {
    let layout = XstateLayout::detect(capabilities()).unwrap();
    let mut area = XstateArea::new();
    area.bytes_mut().fill(0xff);
    area.reset(layout, 0).unwrap();
    assert_eq!(align_of::<XstateArea>(), 64);
    assert_eq!(size_of::<XstateArea>(), 4096);
    assert_eq!(area.as_ptr() as usize % 64, 0);
    assert_eq!(&area.bytes()[0..2], &0x037fu16.to_le_bytes());
    assert_eq!(&area.bytes()[24..28], &MXCSR_INITIAL.to_le_bytes());
    assert_eq!(&area.bytes()[28..32], &MXCSR_DEFAULT_MASK.to_le_bytes());
    assert_eq!(&area.bytes()[512..520], &7u64.to_le_bytes());
    assert!(area.bytes()[32..512].iter().all(|&b| b == 0));
    assert!(area.bytes()[520..].iter().all(|&b| b == 0));
    assert_eq!(area.validate(layout, 0), Ok(()));
}

#[test]
fn mxcsr_masks_and_reserved_values_fail_before_restore() {
    assert_eq!(effective_mxcsr_mask(0), Ok(0xffbf));
    assert_eq!(effective_mxcsr_mask(0xffff), Ok(0xffff));
    assert_eq!(effective_mxcsr_mask(0x2ffff), Ok(0x2ffff));
    for mask in [1, 0x1f00, 0x1ffff, 0x3ffff, 0x6ffff, 0x8002ffff] {
        assert_eq!(
            effective_mxcsr_mask(mask),
            Err(XstateError::InvalidMxcsrMask)
        );
    }
    let layout = XstateLayout::detect(capabilities()).unwrap();
    let mut area = XstateArea::new();
    area.reset(layout, 0).unwrap();
    area.bytes_mut()[24] |= 0x40; // DAZ unsupported by default mask.
    assert_eq!(area.validate(layout, 0), Err(XstateError::InvalidMxcsr));
    assert_eq!(area.validate(layout, 0xffff), Ok(()));
    area.reset(layout, 0x2ffff).unwrap();
    assert_eq!(area.validate(layout, 0x2ffff), Ok(()));
    area.bytes_mut()[26] = 2; // MM enabled only when observed hardware supports it.
    assert_eq!(area.validate(layout, 0x2ffff), Ok(()));
    assert_eq!(area.validate(layout, 0xffff), Err(XstateError::InvalidMxcsr));
    area.bytes_mut()[27] = 1;
    assert_eq!(
        area.validate(layout, 0xffff),
        Err(XstateError::InvalidMxcsr)
    );
}

#[test]
fn unsupported_component_and_compacted_or_reserved_headers_are_rejected() {
    let layout = XstateLayout::detect(capabilities()).unwrap();
    let mut area = XstateArea::new();
    for offset in [512, 519, 520, 527, 528, 575] {
        area.reset(layout, 0).unwrap();
        area.bytes_mut()[offset] |= 0x80;
        assert_eq!(area.validate(layout, 0), Err(XstateError::InvalidHeader));
    }
    area.reset(layout, 0).unwrap();
    area.bytes_mut()[512..520].fill(0); // XRSTOR architectural init state.
    assert_eq!(area.validate(layout, 0), Ok(()));
}
