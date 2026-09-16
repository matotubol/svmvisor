use core::mem::{align_of, size_of};
use svmvisor_hypervisor::svm::{
    permission_maps::{IOPM_BYTES, Iopm, MSRPM_BYTES, MsrAccess, Msrpm, Permission, PermissionMapError},
    x2avic::{irq::PhysicalIrqLedger, registers},
};

fn map_bit(map: &Msrpm, msr: u32, write: bool) -> bool {
    // MSRs 0-1FFFh: two bits per MSR from byte 0, read then write.
    let bit = 2 * msr as usize + usize::from(write);
    map.bytes()[bit / 8] & (1 << (bit % 8)) != 0
}

/// D1, written out from the phase B brief rather than derived from code.
/// APM2 rev3.44 Table 15-22 pp566-568: allowed reads and accelerated writes.
fn d1_left_to_hardware(msr: u32, write: bool) -> bool {
    if write {
        matches!(msr, 0x808 | 0x80b | 0x830 | 0x83f)
    } else {
        matches!(msr, 0x802 | 0x803 | 0x808 | 0x80a | 0x80d | 0x80f | 0x810..=0x827
            | 0x828 | 0x830 | 0x832..=0x838 | 0x83e)
    }
}

/// D1's explicit intercept lists.
fn d1_intercepted(msr: u32, write: bool) -> bool {
    if write {
        !matches!(msr, 0x808 | 0x830 | 0x83f | 0x80b)
    } else {
        matches!(msr, 0x800..=0x801 | 0x804..=0x807 | 0x809 | 0x80b | 0x80c | 0x80e
            | 0x829..=0x82f | 0x831 | 0x839 | 0x83a..=0x83d | 0x83f | 0x840..=0x8ff)
    }
}

#[test]
fn x2avic_profile_matches_d1_for_every_x2apic_msr_and_access() {
    let base = Msrpm::native_boot();
    let mut map = Msrpm::native_boot();
    map.configure_native_x2avic();
    let (mut reads, mut writes) = (0, 0);
    for msr in 0x800..=0x8ffu32 {
        for write in [false, true] {
            let access = if write { MsrAccess::Write } else { MsrAccess::Read };
            // The two literal D1 lists partition the range.
            assert_eq!(d1_intercepted(msr, write), !d1_left_to_hardware(msr, write), "{msr:#x} {write}");
            assert_eq!(map_bit(&map, msr, write), d1_intercepted(msr, write), "{msr:#x} {write}");
            assert_eq!(registers::intercepted(msr, access), d1_intercepted(msr, write), "{msr:#x}");
            if map_bit(&map, msr, write) {
                if write { writes += 1 } else { reads += 1 }
            }
        }
    }
    // 256 MSRs: 40 hardware reads; 4 hardware writes (EOI dynamically).
    assert_eq!((reads, writes), (216, 252));
    // APIC_BASE stays intercepted in both directions.
    assert!(map_bit(&map, 0x1b, false) && map_bit(&map, 0x1b, true));
    // Outside the x2APIC range the profile function never releases an MSR.
    for msr in [0, 0x1b, 0x7ff, 0x900, 0xc000_0080, u32::MAX] {
        assert!(registers::intercepted(msr, MsrAccess::Read));
        assert!(registers::intercepted(msr, MsrAccess::Write));
    }
    // Only the x2APIC range and APIC_BASE differ from the native boot map.
    for msr in (0..0x2000u32).filter(|msr| !(0x800..=0x8ff).contains(msr) && *msr != 0x1b) {
        for write in [false, true] {
            assert_eq!(map_bit(&map, msr, write), map_bit(&base, msr, write), "{msr:#x}");
        }
    }
    assert_eq!(&map.bytes()[0x800..], &base.bytes()[0x800..]);
}

#[test]
fn x2avic_eoi_write_intercept_follows_held_level_sources() {
    let mut map = Msrpm::native_boot();
    map.configure_native_x2avic();
    let before = *map.bytes();
    let mut ledger = PhysicalIrqLedger::new();
    // Nothing held: EOI stays accelerated and the map is unchanged.
    assert!(!map.update_x2apic_eoi_intercept(&ledger));
    assert_eq!(*map.bytes(), before);
    ledger.commit_level_capture(0x40).unwrap();
    assert!(!map_bit(&map, 0x80b, true));
    assert!(map.update_x2apic_eoi_intercept(&ledger));
    // Writes are now intercepted; D1 intercepts EOI reads (#GP) throughout.
    assert!(map_bit(&map, 0x80b, true) && map_bit(&map, 0x80b, false));
    assert!(!map.update_x2apic_eoi_intercept(&ledger));
    let mut expected = before;
    expected[(2 * 0x80b + 1) / 8] |= 1 << ((2 * 0x80b + 1) % 8);
    assert_eq!(*map.bytes(), expected);
    // The source completes and its physical EOI is committed.
    ledger.complete_level(0x40).unwrap();
    assert!(!map.update_x2apic_eoi_intercept(&ledger));
    assert_eq!(ledger.next_eoi(Some(0x40)), Ok(Some(0x40)));
    ledger.commit_eoi(0x40).unwrap();
    assert!(map.update_x2apic_eoi_intercept(&ledger));
    assert_eq!(*map.bytes(), before);
}

#[test]
fn storage_layout_and_default_intercept_every_bit_including_padding() {
    assert_eq!(IOPM_BYTES, 12288);
    assert_eq!(MSRPM_BYTES, 8192);
    assert_eq!(size_of::<Iopm>(), 12288);
    assert_eq!(size_of::<Msrpm>(), 8192);
    assert_eq!(align_of::<Iopm>(), 4096);
    assert_eq!(align_of::<Msrpm>(), 4096);
    assert_eq!(Iopm::default().bytes(), &[0xff; 12288]);
    assert_eq!(Msrpm::default().bytes(), &[0xff; 8192]);
}

#[test]
fn io_ranges_change_only_selected_bits_and_never_clear_overrun_bits() {
    let mut map = Iopm::new();
    map.set_range(0, 1, Permission::Allow).unwrap();
    map.set_range(7, 3, Permission::Allow).unwrap();
    map.set_range(65535, 1, Permission::Allow).unwrap();
    let mut expected = [0xff; 12288];
    expected[0] = 0x7e;
    expected[1] = 0xfc;
    expected[8191] = 0x7f;
    assert_eq!(map.bytes(), &expected);
    map.set_range(8, 1, Permission::Intercept).unwrap();
    expected[1] = 0xfd;
    assert_eq!(map.bytes(), &expected);

    map.set_range(0, 65536, Permission::Allow).unwrap();
    expected[..8192].fill(0);
    assert_eq!(map.bytes(), &expected);
    map.set_range(0, 65536, Permission::Intercept).unwrap();
    assert_eq!(map.bytes(), &[0xff; 12288]);
}

#[test]
fn invalid_io_ranges_leave_existing_permissions_unchanged() {
    let mut map = Iopm::new();
    map.set_range(8, 13, Permission::Allow).unwrap();
    let before = *map.bytes();
    for (port, count, error) in [
        (0, 0, PermissionMapError::EmptyIoRange),
        (65535, 0, PermissionMapError::EmptyIoRange),
        (65535, 2, PermissionMapError::IoRangeOutsidePorts),
        (0, 65537, PermissionMapError::IoRangeOutsidePorts),
        (1, u32::MAX, PermissionMapError::IoRangeOutsidePorts),
    ] {
        for permission in [Permission::Allow, Permission::Intercept] {
            assert_eq!(map.set_range(port, count, permission), Err(error));
            assert_eq!(map.bytes(), &before);
        }
    }
}

#[test]
fn msr_range_endpoints_and_read_write_bits_match_architectural_byte_fixture() {
    let mut map = Msrpm::new();
    // Independent byte/bit fixtures for the three architectural range ends,
    // plus the adjacent MSR sharing a byte with MSR 0.
    for (msr, access) in [
        (0, MsrAccess::Read),
        (1, MsrAccess::Write),
        (0x1fff, MsrAccess::Write),
        (0xc000_0000, MsrAccess::Write),
        (0xc000_1fff, MsrAccess::Read),
        (0xc001_0000, MsrAccess::Read),
        (0xc001_1fff, MsrAccess::Write),
    ] {
        map.set(msr, access, Permission::Allow).unwrap();
    }
    let mut expected = [0xff; 8192];
    expected[0x0000] = 0xf6;
    expected[0x07ff] = 0x7f;
    expected[0x0800] = 0xfd;
    expected[0x0fff] = 0xbf;
    expected[0x1000] = 0xfe;
    expected[0x17ff] = 0x7f;
    assert_eq!(map.bytes(), &expected);
    map.set(0, MsrAccess::Write, Permission::Allow).unwrap();
    expected[0] = 0xf4;
    assert_eq!(map.bytes(), &expected);
    map.set(0, MsrAccess::Read, Permission::Intercept).unwrap();
    expected[0] = 0xf5;
    assert_eq!(map.bytes(), &expected);
}

#[test]
fn unsupported_msrs_cannot_alias_a_valid_range_or_reserved_storage() {
    let mut map = Msrpm::new();
    map.set(0x1fff, MsrAccess::Read, Permission::Allow).unwrap();
    let before = *map.bytes();
    for msr in [
        0x2000,
        0xbfff_ffff,
        0xc000_2000,
        0xc000_ffff,
        0xc001_2000,
        0xc002_0000,
        u32::MAX,
    ] {
        for access in [MsrAccess::Read, MsrAccess::Write] {
            for permission in [Permission::Allow, Permission::Intercept] {
                assert_eq!(
                    map.set(msr, access, permission),
                    Err(PermissionMapError::UnsupportedMsr)
                );
                assert_eq!(map.bytes(), &before);
            }
        }
    }
}
