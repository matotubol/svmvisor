use svmvisor_hypervisor::{
    guest::pages::{
        GuestPages, GuestPagesError as E, PagePermissions as P, TableStorage, WINDOW_BYTES,
    },
    memory::address::{AddressError, AddressPolicy, EncryptionState},
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: Some(47) }).unwrap()
}

fn snapshot(pages: &GuestPages<'_>) -> Vec<Vec<u8>> {
    (0..4).map(|i| pages.table(i).unwrap().bytes.to_vec()).collect()
}

fn entry(bytes: &[u8; 4096], index: usize) -> u64 {
    u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap())
}

#[test]
fn exported_four_level_walk_has_supervisor_permissions_and_no_nx() {
    let mut storage = TableStorage([[0xa5; 4096]; 4]);
    let mut pages = GuestPages::new(&mut storage, 0x100000, policy()).unwrap();
    assert_eq!(pages.root_address(), 0x100000);
    assert!(pages.table(4).is_none());
    assert_eq!(pages.translate(0), Ok(None));
    pages.map_page(0x1000, 0x1000, P::ReadOnly).unwrap();
    pages.map_page(0x8000, 0x8000, P::ReadWrite).unwrap();
    for index in 0..3 {
        let table = pages.table(index).unwrap();
        assert_eq!(table.guest_address, 0x100000 + index as u64 * 4096);
        assert_eq!(entry(table.bytes, 0), (0x101000 + index as u64 * 4096) | 3);
        assert!(table.bytes[8..].iter().all(|&byte| byte == 0));
    }
    let leaves = pages.table(3).unwrap();
    assert_eq!(entry(leaves.bytes, 1), 0x1001);
    assert_eq!(entry(leaves.bytes, 8), 0x8003);
    for (va, gpa, permissions) in [(0x1fff, 0x1fff, P::ReadOnly), (0x8fff, 0x8fff, P::ReadWrite)] {
        let translation = pages.translate(va).unwrap().unwrap();
        assert_eq!(translation.guest_address, gpa);
        assert_eq!(translation.permissions, permissions);
    }
    assert_eq!(pages.translate(0x7000), Ok(None));
    assert_eq!(pages.translate(0x9000), Ok(None));
}

#[test]
fn rejected_mappings_are_atomic_including_aliases_and_arena_overlap() {
    let mut storage = TableStorage([[0; 4096]; 4]);
    let mut pages = GuestPages::new(&mut storage, 0x100000, policy()).unwrap();
    pages.map_page(0x1000, 0x4000, P::ReadOnly).unwrap();
    let before = snapshot(&pages);
    for (va, gpa, error) in [
        (0x1000, 0x5000, E::AlreadyMapped),
        (0x2000, 0x4000, E::GuestPageAlias),
        (0x2000, 0x100000, E::TableArenaOverlap),
        (0x2000, 0x103000, E::TableArenaOverlap),
        (0x2001, 0x5000, E::VirtualAddressMisaligned),
        (WINDOW_BYTES, 0x5000, E::VirtualAddressOutsideWindow),
        (0xffff_8000_0000_0000, 0x5000, E::VirtualAddressOutsideWindow),
        (0x2000, 0x5001, E::Address(AddressError::Misaligned)),
        (0x2000, 1 << 48, E::Address(AddressError::OutsidePhysicalWidth)),
        (0x2000, 1 << 47, E::Address(AddressError::EncryptionBitEncoded)),
    ] {
        assert_eq!(pages.map_page(va, gpa, P::ReadWrite), Err(error));
        assert_eq!(snapshot(&pages), before);
    }
    assert_eq!(pages.translate(WINDOW_BYTES), Err(E::VirtualAddressOutsideWindow));
}

#[test]
fn invalid_arena_does_not_clear_storage() {
    let mut storage = TableStorage([[0xa5; 4096]; 4]);
    for (base, expected) in [
        (1, AddressError::Misaligned),
        ((1 << 48) - 4096, AddressError::OutsidePhysicalWidth),
        ((1 << 47) - 4096, AddressError::EncryptionBitEncoded),
        (u64::MAX - 4095, AddressError::Overflow),
    ] {
        assert!(
            matches!(GuestPages::new(&mut storage, base, policy()), Err(E::Address(e)) if e == expected)
        );
        assert!(storage.0.iter().flatten().all(|&byte| byte == 0xa5));
    }
}

#[test]
fn complete_window_capacity_and_highest_numeric_gpa_are_bounded() {
    let wide =
        AddressPolicy::new(52, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let mut storage = TableStorage([[0; 4096]; 4]);
    let mut pages = GuestPages::new(&mut storage, 0x400000, wide).unwrap();
    for index in 0..512 {
        pages.map_page(index * 4096, (1 << 52) - (512 - index) * 4096, P::ReadWrite).unwrap();
    }
    assert_eq!(pages.translate(WINDOW_BYTES - 1).unwrap().unwrap().guest_address, (1 << 52) - 1);
    let before = snapshot(&pages);
    assert_eq!(pages.map_page(WINDOW_BYTES, 0, P::ReadOnly), Err(E::VirtualAddressOutsideWindow));
    assert_eq!(snapshot(&pages), before);
    assert_eq!(pages.map_page(0, 0, P::ReadOnly), Err(E::AlreadyMapped));
    assert_eq!(snapshot(&pages), before);
}

#[test]
fn relocated_windows_encode_all_parent_indices_and_preserve_full_virtual_addresses() {
    // Include both canonical boundaries and a window exercising all three
    // parent indices; a translation helper alone cannot detect wrong parents.
    for base in [
        WINDOW_BYTES,
        64 * 1024 * 1024,
        (3 << 39) | (5 << 30) | (7 << 21),
        (1 << 47) - WINDOW_BYTES,
        0xffff_8000_0000_0000,
        u64::MAX - (WINDOW_BYTES - 1),
    ] {
        let mut storage = TableStorage([[0xa5; 4096]; 4]);
        let mut pages = GuestPages::new_in_window(&mut storage, 0x100000, policy(), base).unwrap();
        assert_eq!(pages.virtual_window_base(), base);
        for (table, shift) in [39, 30, 21].into_iter().enumerate() {
            for index in 0..512 {
                let expected = if index == ((base >> shift) & 511) as usize {
                    (0x101000 + table as u64 * 4096) | 3
                } else {
                    0
                };
                assert_eq!(entry(pages.table(table).unwrap().bytes, index), expected);
            }
        }
        pages.map_page(base, 0x4000, P::ReadOnly).unwrap();
        pages.map_page(base + (WINDOW_BYTES - 4096), 0x9000, P::ReadWrite).unwrap();
        for (va, gpa, permissions) in [
            (base, 0x4000, P::ReadOnly),
            (base + 4095, 0x4fff, P::ReadOnly),
            (base + (WINDOW_BYTES - 1), 0x9fff, P::ReadWrite),
        ] {
            let translated = pages.translate(va).unwrap().unwrap();
            assert_eq!(translated.guest_address, gpa);
            assert_eq!(translated.permissions, permissions);
        }
        assert_eq!(pages.translate(base + 4096), Ok(None));
        let before = snapshot(&pages);
        for outside in [base.checked_sub(1), base.checked_add(WINDOW_BYTES)].into_iter().flatten() {
            assert_eq!(pages.translate(outside), Err(E::VirtualAddressOutsideWindow));
            assert_eq!(
                pages.map_page(outside, 0xa000, P::ReadWrite),
                Err(E::VirtualAddressOutsideWindow)
            );
            assert_eq!(snapshot(&pages), before);
        }
        assert_eq!(pages.map_page(base + 4096, 0x4000, P::ReadWrite), Err(E::GuestPageAlias));
        assert_eq!(pages.map_page(base + 4096, 0x101000, P::ReadWrite), Err(E::TableArenaOverlap));
        assert_eq!(
            pages.map_page(base + 1, 0xa000, P::ReadWrite),
            Err(E::VirtualAddressMisaligned)
        );
        assert_eq!(snapshot(&pages), before);
    }
}

#[test]
fn invalid_virtual_window_or_arena_preserves_caller_storage() {
    let mut storage = TableStorage([[0xa5; 4096]; 4]);
    for (base, arena, expected) in [
        (1, 0x100000, E::VirtualWindowMisaligned),
        (3 * 1024 * 1024, 0x100000, E::VirtualWindowMisaligned),
        (1 << 47, 0x100000, E::VirtualWindowNonCanonical),
        (0xffff_7fff_ffe0_0000, 0x100000, E::VirtualWindowNonCanonical),
        (u64::MAX, 0x100000, E::VirtualWindowOverflow),
        (u64::MAX - WINDOW_BYTES + 2, 0x100000, E::VirtualWindowOverflow),
        (64 * 1024 * 1024, 1, E::Address(AddressError::Misaligned)),
    ] {
        assert!(
            matches!(GuestPages::new_in_window(&mut storage, arena, policy(), base), Err(e) if e == expected)
        );
        assert!(storage.0.iter().flatten().all(|&byte| byte == 0xa5));
    }
}
