use svmvisor_card_abi::package::{
    ARENA_BYTES, HANDOFF_OFFSET, LayoutError, Payload, is_valid_arena,
};

fn set_word(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn package() -> Vec<u8> {
    let mut bytes = vec![0; 64 + 32 + 32];
    bytes[..8].copy_from_slice(b"SVMRELO1");
    for (i, value) in [0x100000, ARENA_BYTES as u64, 32, 4096, 16, 2, 0].into_iter().enumerate() {
        set_word(&mut bytes, 8 + i * 8, value);
    }
    set_word(&mut bytes, 64, 0x100008);
    bytes[72..76].copy_from_slice(&0x101000u32.to_le_bytes());
    set_word(&mut bytes, 96, 0);
    set_word(&mut bytes, 104, 8);
    set_word(&mut bytes, 112, 8);
    set_word(&mut bytes, 120, 4);
    bytes
}

fn rejects(bytes: &[u8], error: LayoutError) {
    assert!(matches!(Payload::parse(bytes, 16), Err(actual) if actual == error));
}

#[test]
fn same_package_relocates_initialized_addresses_and_zeros_owned_memory() {
    let bytes = package();
    let payload = Payload::parse(&bytes, 16).unwrap();
    for base in [0x100000, 0x200000, 0x300000, 0x810000, 0x3ff00000] {
        let mut arena = vec![0xcc; ARENA_BYTES];
        payload.load(&mut arena, base).unwrap();
        assert_eq!(u64::from_le_bytes(arena[..8].try_into().unwrap()), base + 8);
        assert_eq!(u32::from_le_bytes(arena[8..12].try_into().unwrap()) as u64, base + 4096);
        assert!(arena[32..HANDOFF_OFFSET].iter().all(|&b| b == 0));
        assert_eq!(&arena[HANDOFF_OFFSET..HANDOFF_OFFSET + 8], b"SVMUEFI2");
        for (index, expected) in [base, ARENA_BYTES as u64, 32, 4096, 16].iter().enumerate() {
            assert_eq!(
                &arena[HANDOFF_OFFSET + 8 + index * 8..HANDOFF_OFFSET + 16 + index * 8],
                &expected.to_le_bytes()
            );
        }
    }
}

#[test]
fn malformed_header_and_arithmetic_overflow_are_rejected() {
    rejects(&[], LayoutError::Header);
    for (offset, value, error) in [
        (0, 0, LayoutError::Header),
        (8, u64::MAX, LayoutError::Bounds),
        (16, 0, LayoutError::Header),
        (24, u64::MAX, LayoutError::Bounds),
        (32, ARENA_BYTES as u64, LayoutError::Bounds),
        (40, 32, LayoutError::Entry),
        (48, u64::MAX, LayoutError::Bounds),
        (56, 1, LayoutError::Header),
    ] {
        let mut bytes = package();
        set_word(&mut bytes, offset, value);
        rejects(&bytes, error);
    }
    let mut bytes = package();
    bytes.push(0);
    rejects(&bytes, LayoutError::Bounds);
    bytes.truncate(100);
    rejects(&bytes, LayoutError::Bounds);
}

#[test]
fn relocation_sites_must_be_sorted_disjoint_and_initialized() {
    for (offset, value) in [(96, u64::MAX), (96, 29), (104, 3), (112, 4), (112, 0), (112, 32)] {
        let mut bytes = package();
        set_word(&mut bytes, offset, value);
        rejects(&bytes, LayoutError::Relocation);
    }
}

#[test]
fn relocation_targets_cannot_escape_declared_memory() {
    for value in [0, 0xfffff, 0x101001, u64::MAX] {
        let mut bytes = package();
        set_word(&mut bytes, 64, value);
        rejects(&bytes, LayoutError::Relocation);
    }
}

#[test]
fn invalid_arena_rejection_preserves_destination() {
    let bytes = package();
    let payload = Payload::parse(&bytes, 16).unwrap();
    let mut arena = vec![0xa5; ARENA_BYTES];
    for base in [0, 0x80000, 0x200001, 0x180000, 0x40000000, u64::MAX - 4095] {
        assert!(!is_valid_arena(base));
        assert_eq!(payload.load(&mut arena, base), Err(LayoutError::Arena));
        assert!(arena.iter().all(|&b| b == 0xa5));
    }
    assert_eq!(payload.load(&mut arena[..ARENA_BYTES - 1], 0x200000), Err(LayoutError::Arena));
}
