use svmvisor_hypervisor::memory::mtrrs::{Mtrrs, Tom2Default, Tom2Error};

const LAPIC: u64 = 0xfee0_0000;

fn observation(default_type: u64) -> Mtrrs {
    Mtrrs {
        default: 0x800 | default_type,
        count: 0,
        variable: [(0, 0); 16],
        physical_bits: 48,
        tom2_default: None,
    }
}

fn range(mt: &mut Mtrrs, base: u64, bytes: u64, kind: u64) {
    let physical = ((1u64 << mt.physical_bits) - 1) & !4095;
    mt.variable[mt.count] = (base | kind, (physical & !(bytes - 1)) | 0x800);
    mt.count += 1;
}

#[test]
fn effective_uc_matches_every_pat_byte_and_architectural_mtrr_type() {
    for mtrr in [0, 1, 4, 5, 6] {
        let mt = observation(mtrr);
        for pat in 0..=255u8 {
            assert_eq!(
                mt.page_is_uc(LAPIC, pat),
                mtrr == 0 && matches!(pat, 0 | 4 | 5 | 6 | 7),
                "MTRR={mtrr}, PAT={pat}"
            );
        }
        assert_eq!(mt.page_is_wb(LAPIC), mtrr == 6);
    }
}

#[test]
fn lapic_uc_range_overrides_wb_default_at_exact_page_boundaries() {
    let mut mt = observation(6);
    range(&mut mt, LAPIC, 4096, 0);
    assert!(mt.page_is_wb(LAPIC - 4096));
    assert!(mt.page_is_uc(LAPIC, 6));
    assert!(!mt.page_is_wb(LAPIC));
    assert!(mt.page_is_wb(LAPIC + 4096));
}

#[test]
fn uc_overlap_dominates_and_undefined_overlap_is_refused() {
    let mut mt = observation(6);
    range(&mut mt, LAPIC & !0x1fffff, 0x200000, 6);
    range(&mut mt, LAPIC, 4096, 0);
    assert!(mt.page_is_uc(LAPIC, 0));
    mt.variable[1].0 = LAPIC | 4;
    assert!(!mt.page_is_uc(LAPIC, 0));
    assert!(!mt.page_is_wb(LAPIC));
    mt.variable[1].0 = LAPIC | 1;
    assert!(!mt.page_is_uc(LAPIC, 0));
    assert!(!mt.page_is_wb(LAPIC));
}

#[test]
fn disabled_out_of_range_and_malformed_observations_are_refused() {
    let good = observation(0);
    for page in [0, 0xff000, LAPIC + 1, 1u64 << 48] {
        assert!(!good.page_is_uc(page, 0));
    }
    let mut bad = good;
    bad.default = 0;
    assert!(!bad.page_is_uc(LAPIC, 0));
    bad = good;
    bad.default |= 2;
    assert!(!bad.page_is_uc(LAPIC, 0));
    bad = good;
    bad.count = 17;
    assert!(!bad.page_is_uc(LAPIC, 0));
    bad = good;
    range(&mut bad, LAPIC, 4096, 2);
    assert!(!bad.page_is_uc(LAPIC, 0));
    bad.variable[0].0 = LAPIC;
    bad.variable[0].1 &= !(1 << 14);
    assert!(!bad.page_is_uc(LAPIC, 0));
}

const SIGNATURE: u32 = 0x00b4_0f40;
const FOUR_GIB: u64 = 0x1_0000_0000;
const TOM2: u64 = 0x2_0000_0000;
const TOM2_ENABLED_WB: u64 = (1 << 21) | (1 << 22);

fn tom2_observation(default: u64) -> Mtrrs {
    let mut mt = observation(default);
    mt.tom2_default = Tom2Default::new(SIGNATURE, 48, TOM2_ENABLED_WB, TOM2).unwrap();
    mt
}

#[test]
fn tom2_changes_only_complete_high_pages_with_no_variable_match() {
    for default in [0, 1, 4, 5, 6] {
        let mt = tom2_observation(default);
        assert_eq!(mt.page_is_wb(FOUR_GIB - 4096), default == 6);
        assert!(mt.page_is_wb(FOUR_GIB));
        assert!(mt.page_is_wb(TOM2 - 4096));
        assert_eq!(mt.page_is_wb(TOM2), default == 6);
        assert_eq!(mt.page_is_wb(TOM2 + 4096), default == 6);
        assert!(!mt.page_is_wb(TOM2 - 1));
        assert!(!mt.page_is_wb(1 << 48));
        assert!(!mt.page_is_wb(0xff000));
        assert_eq!(mt.page_is_uc(LAPIC, 6), default == 0);
    }
}

#[test]
fn tom2_requires_both_sys_cfg_enables_and_enabled_mtrrs() {
    for enables in 0..4u64 {
        for mtrr_enabled in [false, true] {
            let value = Tom2Default::new(SIGNATURE, 48, enables << 21, TOM2);
            if enables == 2 {
                assert_eq!(value, Err(Tom2Error::Tom2Disabled));
                continue;
            }
            let mut mt = observation(0);
            mt.tom2_default = value.unwrap();
            if !mtrr_enabled {
                mt.default &= !(1 << 11);
            }
            assert_eq!(mt.page_is_wb(FOUR_GIB), enables == 3 && mtrr_enabled);
            // Disabled MTRRs remain outside the existing classifier's profile.
            assert_eq!(mt.page_is_uc(FOUR_GIB, 6), enables != 3 && mtrr_enabled);
        }
    }
    // Inactive TOM2 reset contents have no bearing on ordinary MTRR defaults.
    assert_eq!(Tom2Default::new(SIGNATURE, 48, 0, u64::MAX), Ok(None));
    assert_eq!(Tom2Default::new(SIGNATURE, 48, 1 << 21, u64::MAX), Ok(None));
}

#[test]
fn tom2_rejects_other_targets_widths_reserved_bits_and_encryption() {
    for signature in [0, SIGNATURE - 1, SIGNATURE + 1] {
        assert!(!Tom2Default::supported_profile(signature, 48));
        assert_eq!(
            Tom2Default::new(signature, 48, TOM2_ENABLED_WB, TOM2),
            Err(Tom2Error::UnsupportedProfile)
        );
    }
    for width in [0, 32, 47, 49, 52, 64] {
        assert!(!Tom2Default::supported_profile(SIGNATURE, width));
        assert_eq!(
            Tom2Default::new(SIGNATURE, width, TOM2_ENABLED_WB, TOM2),
            Err(Tom2Error::UnsupportedProfile)
        );
    }
    for bit in 0..64 {
        if !(18..=26).contains(&bit) {
            assert_eq!(
                Tom2Default::new(SIGNATURE, 48, TOM2_ENABLED_WB | 1 << bit, TOM2),
                Err(Tom2Error::ReservedControlBits)
            );
        }
    }
    for bit in 23..=26 {
        assert_eq!(
            Tom2Default::new(SIGNATURE, 48, TOM2_ENABLED_WB | 1 << bit, TOM2),
            Err(Tom2Error::ActiveEncryptionUnsupported)
        );
    }
}

#[test]
fn tom2_validates_every_reserved_address_bit_and_range_boundary() {
    for bit in 0..64 {
        if !(23..48).contains(&bit) {
            assert_eq!(
                Tom2Default::new(SIGNATURE, 48, TOM2_ENABLED_WB, TOM2 | 1 << bit),
                Err(Tom2Error::InvalidTopOfMemory)
            );
        }
    }
    for end in [0, FOUR_GIB - (1 << 23), FOUR_GIB] {
        assert_eq!(
            Tom2Default::new(SIGNATURE, 48, TOM2_ENABLED_WB, end),
            Err(Tom2Error::InvalidTopOfMemory)
        );
    }
    for end in [FOUR_GIB + (1 << 23), 0x0000_ffff_ff80_0000] {
        let mut mt = observation(0);
        mt.tom2_default = Tom2Default::new(SIGNATURE, 48, TOM2_ENABLED_WB, end).unwrap();
        assert!(mt.page_is_wb(end - 4096));
        assert!(mt.page_is_uc(end, 6));
    }
    let mut mt = tom2_observation(0);
    mt.physical_bits = 52;
    assert!(!mt.page_is_wb(FOUR_GIB));
}

#[test]
fn variable_types_and_overlaps_keep_precedence_over_tom2_default() {
    for kind in [0, 1, 4, 5, 6] {
        let mut mt = tom2_observation(0);
        range(&mut mt, FOUR_GIB, 4096, kind);
        assert_eq!(mt.page_is_wb(FOUR_GIB), kind == 6);
        assert_eq!(mt.page_is_uc(FOUR_GIB, 6), kind == 0);
        assert!(mt.page_is_wb(FOUR_GIB + 4096));
    }
    for first in [0, 1, 4, 5, 6] {
        for second in [0, 1, 4, 5, 6] {
            let mut mt = tom2_observation(0);
            range(&mut mt, FOUR_GIB, 4096, first);
            range(&mut mt, FOUR_GIB, 4096, second);
            assert_eq!(mt.page_is_wb(FOUR_GIB), first == 6 && second == 6);
            assert_eq!(mt.page_is_uc(FOUR_GIB, 6), first == 0 || second == 0);
        }
    }
}

#[test]
fn tom2_does_not_hide_invalid_mtrrs_or_modify_observation_on_refusal() {
    let mut mt = tom2_observation(0);
    range(&mut mt, FOUR_GIB, 4096, 2);
    let original = mt.variable;
    let original_default = mt.default;
    let original_tom2 = mt.tom2_default;
    assert!(!mt.page_is_wb(FOUR_GIB));
    assert!(!mt.page_is_wb(FOUR_GIB + 4096));
    assert_eq!(mt.variable, original);
    assert_eq!(mt.default, original_default);
    assert_eq!(mt.tom2_default, original_tom2);
    mt.variable[0] = (FOUR_GIB | 6, 0); // A disabled variable range cannot override.
    assert!(mt.page_is_wb(FOUR_GIB));
    mt.default |= 1 << 8;
    assert!(!mt.page_is_wb(FOUR_GIB));
}
