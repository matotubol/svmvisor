use svmvisor_hypervisor::guest::state::{
    GuestStateError, GuestStateRequest, SYNTHETIC_CR0, SYNTHETIC_CR4, SYNTHETIC_EFER,
    SYNTHETIC_RFLAGS,
};
use svmvisor_hypervisor::memory::address::{AddressError, AddressPolicy, EncryptionState};

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

fn request() -> GuestStateRequest {
    GuestStateRequest {
        rip: 0x1000,
        rsp: 0x8000,
        rflags: 0x2,
        cr0: 0x8001_0033,
        cr3: 0x9000,
        cr4: 0x20,
        efer: 0x1500,
        rax: 0xfedc_ba98_7654_3210,
    }
}

#[test]
fn exact_profile_preserves_the_register_tuple() {
    let requested = request();
    let state = requested.validate(&policy()).unwrap();
    assert_eq!(state.rip(), requested.rip);
    assert_eq!(state.rsp(), requested.rsp);
    assert_eq!(state.rflags(), SYNTHETIC_RFLAGS);
    assert_eq!(state.cr0(), SYNTHETIC_CR0);
    assert_eq!(state.cr3(), requested.cr3);
    assert_eq!(state.cr4(), SYNTHETIC_CR4);
    assert_eq!(state.efer(), SYNTHETIC_EFER);
    assert_eq!(state.rax(), requested.rax);
}

#[test]
fn canonical_48_boundary_requires_bit_47_sign_extension_for_both_pointers() {
    for pointer in [0, 0x0000_7fff_ffff_ffff, 0xffff_8000_0000_0000, u64::MAX] {
        let mut value = request();
        value.rip = pointer;
        value.rsp = pointer;
        assert!(value.validate(&policy()).is_ok());
    }
    for pointer in
        [0x0000_8000_0000_0000, 0xffff_7fff_ffff_ffff, 0x0001_0000_0000_0000, 0xff00_0000_0000_0000]
    {
        let mut value = request();
        value.rip = pointer;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::NonCanonicalRip));
        value = request();
        value.rsp = pointer;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::NonCanonicalRsp));
    }
}

#[test]
fn every_control_bit_deviation_is_rejected_including_optional_modes() {
    // Exhausting single-bit deviations catches lost required bits as well as
    // accidental acceptance of IF/VM, NXE, LA57, PCID, CET or OSXSAVE.
    for bit in 0..64 {
        let mut value = request();
        value.rflags ^= 1 << bit;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::UnsupportedRflags));
        value = request();
        value.cr0 ^= 1 << bit;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::UnsupportedCr0));
        value = request();
        value.cr4 ^= 1 << bit;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::UnsupportedCr4));
        value = request();
        value.efer ^= 1 << bit;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::UnsupportedEfer));
    }
}

#[test]
fn cr3_requires_an_unencrypted_aligned_whole_page_within_numeric_bounds() {
    let mut value = request();
    let limit = 1u64 << 48;
    value.cr3 = limit - 4096;
    assert!(value.validate(&policy()).is_ok());
    value.cr3 = limit;
    assert_eq!(
        value.validate(&policy()),
        Err(GuestStateError::Cr3(AddressError::OutsidePhysicalWidth))
    );
    for low_bit in 0..12 {
        value.cr3 = 0x9000 | (1 << low_bit);
        assert_eq!(value.validate(&policy()), Err(GuestStateError::Cr3(AddressError::Misaligned)));
    }
    let encrypted_bit_policy =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: Some(47) }).unwrap();
    value.cr3 = 1 << 47;
    assert_eq!(
        value.validate(&encrypted_bit_policy),
        Err(GuestStateError::Cr3(AddressError::EncryptionBitEncoded))
    );
    // The tuple makes no claim that GPA zero is backed; it is numerically valid.
    value.cr3 = 0;
    assert!(value.validate(&policy()).is_ok());
}

#[test]
fn extended_state_is_an_explicit_exact_profile_without_loosening_baseline() {
    use svmvisor_hypervisor::arch::x86_64::xstate::{XstateCapabilities, XstateLayout};
    for (ecx, expected_cr4) in [(0, 0x620), (1 << 26, 0x40620), ((1 << 26) | (1 << 28), 0x40620)] {
        let layout = XstateLayout::detect(XstateCapabilities {
            leaf1_ecx: ecx,
            leaf1_edx: 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26),
            supported_xcr0: 7,
            max_size: 832,
            avx_size: 256,
            avx_offset: 576,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(layout.guest_cr4(), expected_cr4);
        let mut value = request();
        value.cr4 = expected_cr4;
        assert_eq!(value.validate(&policy()), Err(GuestStateError::UnsupportedCr4));
        assert_eq!(value.validate_with_xstate(&policy(), layout).unwrap().cr4(), expected_cr4);
        for bit in 0..64 {
            value.cr4 = expected_cr4 ^ (1 << bit);
            assert_eq!(
                value.validate_with_xstate(&policy(), layout),
                Err(GuestStateError::UnsupportedCr4)
            );
        }
        value.cr4 = expected_cr4;
        value.cr0 ^= 8;
        assert_eq!(
            value.validate_with_xstate(&policy(), layout),
            Err(GuestStateError::UnsupportedCr0)
        );
        value.cr0 = SYNTHETIC_CR0;
        value.cr3 += 1;
        assert!(matches!(
            value.validate_with_xstate(&policy(), layout),
            Err(GuestStateError::Cr3(_))
        ));
    }
}

#[test]
fn captured_arithmetic_flags_preserved_without_loosening_fixed_guest_or_system_state() {
    use svmvisor_hypervisor::arch::x86_64::xstate::{XstateCapabilities, XstateLayout};
    let layout = XstateLayout::detect(XstateCapabilities {
        leaf1_edx: 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26),
        ..Default::default()
    })
    .unwrap();
    let mut value = request();
    value.cr4 = layout.guest_cr4();
    for flags in [2, 3, 0x8d7] {
        value.rflags = flags;
        assert_eq!(
            value.validate_continuation_with_xstate(&policy(), layout).unwrap().rflags(),
            flags
        );
        if flags != 2 {
            assert_eq!(
                value.validate_with_xstate(&policy(), layout),
                Err(GuestStateError::UnsupportedRflags)
            );
        }
    }
    for bit in 0..64 {
        if (1u64 << bit) & 0x8d7 == 0 {
            value.rflags = 2 | (1u64 << bit);
            assert_eq!(
                value.validate_continuation_with_xstate(&policy(), layout),
                Err(GuestStateError::UnsupportedRflags)
            );
        }
    }
    value.rflags = 1;
    assert_eq!(
        value.validate_continuation_with_xstate(&policy(), layout),
        Err(GuestStateError::UnsupportedRflags)
    );
    value.rflags = 3;
    value.cr3 |= 1;
    assert!(matches!(
        value.validate_continuation_with_xstate(&policy(), layout),
        Err(GuestStateError::Cr3(_))
    ));
    value.cr3 = 0x9000;
    value.efer |= 0x800;
    assert_eq!(
        value.validate_continuation_with_xstate(&policy(), layout),
        Err(GuestStateError::UnsupportedEfer)
    );
}
