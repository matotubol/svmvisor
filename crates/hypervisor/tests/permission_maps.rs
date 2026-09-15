use core::mem::{align_of, size_of};
use svmvisor_hypervisor::permission_maps::{
    IOPM_BYTES, Iopm, MSRPM_BYTES, MsrAccess, Msrpm, Permission, PermissionMapError,
};

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
