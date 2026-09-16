#![cfg(feature = "card-load-only")]
use sha2::{Digest, Sha256};
use svmvisor_dxe::delivery::card::*;
use svmvisor_firmware_handoff::layout::{LayoutError, ARENA_BYTES};
fn package() -> Vec<u8> {
    let mut b = vec![0u8; 96];
    b[..8].copy_from_slice(b"SVMRELO1");
    for (offset, value) in [
        (8, 0x100000u64),
        (16, ARENA_BYTES as u64),
        (24, 16),
        (32, 32),
        (40, 8),
        (48, 1),
        (64, 0x100010),
        (80, 0),
        (88, 8),
    ] {
        b[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    b
}
fn header(package: &[u8]) -> ([u8; 128], [u8; 32]) {
    let digest: [u8; 32] = Sha256::digest(package).into();
    let mut h = [0; 128];
    h[..8].copy_from_slice(b"SVMCRD01");
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&128u32.to_le_bytes());
    for (offset, value) in [
        (16, package.len() as u64),
        (24, SLOT_BYTES as u64),
        (32, 128),
        (40, 1),
    ] {
        h[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    h[48..80].copy_from_slice(&digest);
    (h, digest)
}
#[test]
fn pinned_package_is_copied_relocated_and_zero_filled_only_in_owned_arena() {
    let p = package();
    let (h, d) = header(&p);
    let m = Manifest::parse(&h, &d).unwrap();
    let payload = m.package(&p).unwrap();
    let mut arena = vec![0xff; ARENA_BYTES];
    payload.load(&mut arena, 0x400000).unwrap();
    assert_eq!(&arena[..8], &0x400010u64.to_le_bytes());
    assert!(arena[16..0xff000].iter().all(|&b| b == 0));
    for base in [0, 0x100001, 0x180000, 0x40000000] {
        arena.fill(0xaa);
        assert_eq!(payload.load(&mut arena, base), Err(LayoutError::Arena));
        assert!(arena.iter().all(|&b| b == 0xaa));
    }
    assert_eq!(
        payload.load(&mut arena[..ARENA_BYTES - 1], 0x400000),
        Err(LayoutError::Arena)
    );
}
#[test]
fn every_header_control_and_reserved_region_is_validated() {
    let p = package();
    let (h, d) = header(&p);
    for offset in [0, 8, 12, 24, 32, 40, 80, 127] {
        let mut bad = h;
        bad[offset] ^= 0x80;
        assert_eq!(Manifest::parse(&bad, &d), Err(CardError::Header));
    }
    for size in [0, 63, (SLOT_BYTES - 127) as u64, u64::MAX] {
        let mut bad = h;
        bad[16..24].copy_from_slice(&size.to_le_bytes());
        assert_eq!(Manifest::parse(&bad, &d), Err(CardError::Bounds));
    }
    for length in [0, 7, 127] {
        assert_eq!(Manifest::parse(&h[..length], &d), Err(CardError::Header));
    }
}
#[test]
fn package_digest_and_reviewed_pin_both_required() {
    let mut p = package();
    let (h, d) = header(&p);
    let m = Manifest::parse(&h, &d).unwrap();
    assert_eq!(Manifest::parse(&h, &[0; 32]), Err(CardError::Digest));
    p[70] ^= 1;
    assert!(matches!(m.package(&p), Err(CardError::Digest)));
    for size in [0, 40, 95] {
        assert!(matches!(m.package(&p[..size]), Err(CardError::Bounds)));
    }
}
#[test]
fn matching_pin_does_not_override_malformed_relocation_bounds() {
    for (offset, value) in [
        (80, 12u64),
        (88, 3),
        (64, 0x200000),
        (32, 0xff001),
        (40, 16),
        (48, u64::MAX),
    ] {
        let mut p = package();
        p[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        let (h, d) = header(&p);
        assert!(matches!(
            Manifest::parse(&h, &d).unwrap().package(&p),
            Err(CardError::Package(_))
        ));
    }
}
#[test]
fn pin_is_exact_hex_and_hash_matches_standard_known_vector() {
    assert_eq!(parse_pin(&"aB".repeat(32)).unwrap(), [0xab; 32]);
    for pin in ["", "0", &"x".repeat(64), &"0".repeat(65)] {
        assert_eq!(parse_pin(pin), Err(CardError::Digest));
    }
    assert_eq!(
        Sha256::digest(b"abc").as_slice(),
        parse_pin("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap()
    );
}

