#![cfg(feature = "card-load-only")]

use svmvisor_card_loader::delivery::card::{self, Manifest};

fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// Explicit integration check for a real Python-produced card artifact.
/// It writes only an ordinary host Vec and cannot transfer execution.
#[test]
#[ignore = "requires explicit retained card artifact and digest environment"]
fn python_card_artifact_relocates_without_execution() {
    let path = std::env::var("SVMVISOR_TEST_CARD_SLOT").expect("explicit card slot path");
    let pin = card::parse_pin(
        &std::env::var("SVMVISOR_TEST_CARD_SHA256").expect("explicit package digest"),
    )
    .unwrap();
    let slot = std::fs::read(path).unwrap();
    assert_eq!(slot.len(), card::SLOT_BYTES);
    let manifest = Manifest::parse(&slot[..card::HEADER_BYTES], &pin).unwrap();
    let bytes = &slot[card::HEADER_BYTES..card::HEADER_BYTES + manifest.package_bytes()];
    let payload = manifest.package(bytes).unwrap();
    let linked_base = word(bytes, 8);
    let image_bytes = word(bytes, 24) as usize;
    let memory_bytes = word(bytes, 32) as usize;
    let entry = word(bytes, 40);
    let count = word(bytes, 48) as usize;
    assert!(count > 100, "integration input must contain the real runtime relocations");
    for base in [0x300000_u64, 0x4000000] {
        let mut arena = vec![0xa5_u8; 0x100000];
        payload.load(&mut arena, base).unwrap();
        let mut expected = vec![0; 0x100000];
        expected[..image_bytes].copy_from_slice(&bytes[64..64 + image_bytes]);
        for index in 0..count {
            let record = 64 + image_bytes + index * 16;
            let offset = word(bytes, record) as usize;
            let width = word(bytes, record + 8) as usize;
            let original = if width == 8 {
                word(bytes, 64 + offset)
            } else {
                u32::from_le_bytes(bytes[64 + offset..68 + offset].try_into().unwrap()) as u64
            };
            let relocated = original - linked_base + base;
            expected[offset..offset + width].copy_from_slice(&relocated.to_le_bytes()[..width]);
        }
        expected[0xff000..0xff008].copy_from_slice(b"SVMUEFI2");
        for (i, value) in
            [base, 0x100000, image_bytes as u64, memory_bytes as u64, entry].iter().enumerate()
        {
            expected[0xff008 + i * 8..0xff010 + i * 8].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(arena, expected, "all relocated bytes, BSS, padding, and header");
    }
    println!(
        "PASS actual-card-package bytes={} relocations={count} bases=3MiB,64MiB execution=none",
        bytes.len()
    );
}
