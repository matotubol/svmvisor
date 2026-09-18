use svmvisor_hypervisor::{
    boot::{
        memory::{MemoryDescriptor, MemoryError},
        ownership::*,
    },
    memory::address::{AddressError, AddressPolicy, EncryptionState},
};

fn valid_page() -> [u8; HANDOFF_PAGE_BYTES] {
    let mut page = [0xa5; HANDOFF_PAGE_BYTES];
    OwnershipRecord::encode_into(
        &mut page,
        &[descriptor(0, 256, 7), descriptor(0x100000, 512, 1), descriptor(0x300000, 256, 4)],
        policy().validate(0x180000, RESIDENT_ARENA_BYTES, 4096).unwrap(),
        48,
        1,
    )
    .unwrap();
    page
}

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

fn descriptor(start: u64, pages: u64, kind: u32) -> MemoryDescriptor {
    MemoryDescriptor { memory_type: kind, physical_start: start, page_count: pages, attributes: 8 }
}

#[test]
fn exact_wire_offsets_and_roundtrip() {
    let page = valid_page();
    assert_eq!(&page[..64], &[0xa5; 64]);
    let bytes = &page[64..];
    assert_eq!(&bytes[..8], b"SVMOWN01");
    assert_eq!(&bytes[8..20], &[1, 0, 64, 0, 32, 0, 3, 0, 1, 0, 0, 0]);
    assert_eq!(&bytes[24..32], &0x180000u64.to_le_bytes());
    assert_eq!(&bytes[32..40], &0x100000u64.to_le_bytes());
    assert_eq!(
        &bytes[64..96],
        &[
            7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0,
            0, 0, 0
        ]
    );
    let record = OwnershipRecord::decode(&page, &policy()).unwrap();
    assert_eq!(record.descriptor_count(), 3);
    assert_eq!(
        record.descriptors().collect::<Vec<_>>(),
        vec![descriptor(0, 256, 7), descriptor(0x100000, 512, 1), descriptor(0x300000, 256, 4)]
    );
}

#[test]
fn guest_projection_reserves_only_arena_and_preserves_every_byte() {
    let page = valid_page();
    let record = OwnershipRecord::decode(&page, &policy()).unwrap();
    let projected: Vec<_> = record.guest_descriptors().collect();
    assert_eq!(
        projected,
        vec![
            descriptor(0, 256, 7),
            descriptor(0x100000, 128, 1),
            descriptor(0x180000, 256, 0),
            descriptor(0x280000, 128, 1),
            descriptor(0x300000, 256, 4)
        ]
    );
    assert_eq!(projected.iter().map(|d| d.page_count).sum::<u64>(), 1024);
    for (start, length, excluded) in [
        (0x17f000, 4096, false),
        (0x17f000, 8192, true),
        (0x180000, 4096, true),
        (0x27f000, 4096, true),
        (0x280000, 4096, false),
    ] {
        assert_eq!(
            record.excludes_guest_range(policy().validate(start, length, 4096).unwrap()),
            excluded
        );
    }
}

#[test]
fn encoding_refuses_without_mutating_and_requires_complete_loader_coverage() {
    let arena = policy().validate(0x100000, RESIDENT_ARENA_BYTES, 4096).unwrap();
    let cases = [
        (vec![], OwnershipError::DescriptorCount),
        (vec![descriptor(0x100000, 255, 1)], OwnershipError::ArenaUncovered),
        (
            vec![descriptor(0x100000, 128, 1), descriptor(0x181000, 127, 1)],
            OwnershipError::ArenaUncovered,
        ),
        (
            vec![descriptor(0x100000, 128, 1), descriptor(0x180000, 128, 2)],
            OwnershipError::ArenaNotLoaderCode,
        ),
        (
            vec![descriptor(0x100000, 256, 1), descriptor(0x180000, 128, 1)],
            OwnershipError::Map(MemoryError::UnsortedOrOverlapping),
        ),
        (
            vec![descriptor(0x100001, 256, 1)],
            OwnershipError::Map(MemoryError::MisalignedDescriptor),
        ),
        (vec![descriptor(0x100000, u64::MAX, 1)], OwnershipError::Map(MemoryError::Overflow)),
    ];
    for (descriptors, error) in cases {
        let mut page = [0xaa; 4096];
        assert_eq!(OwnershipRecord::encode_into(&mut page, &descriptors, arena, 48, 1), Err(error));
        assert_eq!(page, [0xaa; 4096]);
    }
    let mut page = [0; 4096];
    OwnershipRecord::encode_into(
        &mut page,
        &[descriptor(0x100000, 128, 1), descriptor(0x180000, 128, 1)],
        arena,
        48,
        1,
    )
    .unwrap();
    assert!(OwnershipRecord::decode(&page, &policy()).is_ok());
}

#[test]
fn malformed_records_fail_closed() {
    for offset in [64, 72, 74, 76, 80, 84, 104, 128 + 4, 4095] {
        let mut page = valid_page();
        page[offset] ^= 0x80;
        assert!(OwnershipRecord::decode(&page, &policy()).is_err(), "offset {offset}");
    }
    for count in [0u16, 125, u16::MAX] {
        let mut page = valid_page();
        page[78..80].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            OwnershipRecord::decode(&page, &policy()).unwrap_err(),
            OwnershipError::DescriptorCount
        );
    }
    assert_eq!(
        OwnershipRecord::decode(&valid_page()[..4095], &policy()).unwrap_err(),
        OwnershipError::Size
    );
    let mut page = valid_page();
    page[64 + 64 + 32 + 8..64 + 64 + 32 + 16].copy_from_slice(&0u64.to_le_bytes());
    assert_eq!(
        OwnershipRecord::decode(&page, &policy()).unwrap_err(),
        OwnershipError::Map(MemoryError::UnsortedOrOverlapping)
    );
}

#[test]
fn exact_capacity_is_supported_but_extra_descriptors_are_never_dropped() {
    let mut descriptors = vec![descriptor(0x100000, 256, 1)];
    for index in 1..MAX_OWNERSHIP_DESCRIPTORS {
        descriptors.push(descriptor(0x200000 + (index as u64) * 4096, 1, 7));
    }
    let mut page = [0; 4096];
    let arena = policy().validate(0x100000, RESIDENT_ARENA_BYTES, 4096).unwrap();
    OwnershipRecord::encode_into(&mut page, &descriptors, arena, 48, 1).unwrap();
    assert_eq!(OwnershipRecord::decode(&page, &policy()).unwrap().descriptor_count(), 124);
    descriptors.push(descriptor(0x400000, 1, 7));
    assert_eq!(
        OwnershipRecord::encode_into(&mut page, &descriptors, arena, 48, 1),
        Err(OwnershipError::DescriptorCount)
    );
}

#[test]
fn decode_rechecks_extent_coverage_type_and_encryption_policy() {
    let mut page = valid_page();
    // Change the arena's covering descriptor to LoaderData after encoding.
    page[160..164].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        OwnershipRecord::decode(&page, &policy()).unwrap_err(),
        OwnershipError::ArenaNotLoaderCode
    );
    let mut page = valid_page();
    page[176..184].copy_from_slice(&129u64.to_le_bytes());
    assert_eq!(
        OwnershipRecord::decode(&page, &policy()).unwrap_err(),
        OwnershipError::ArenaUncovered
    );
    let mut page = valid_page();
    page[176..184].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        OwnershipRecord::decode(&page, &policy()).unwrap_err(),
        OwnershipError::Map(MemoryError::Overflow)
    );
    let mut page = valid_page();
    page[200..208].copy_from_slice(&(1u64 << 47).to_le_bytes());
    let encrypted_bit =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: Some(47) }).unwrap();
    assert_eq!(
        OwnershipRecord::decode(&page, &encrypted_bit).unwrap_err(),
        OwnershipError::Address(AddressError::EncryptionBitEncoded)
    );
    let mut page = [0xaa; 4096];
    let wide_arena = policy().validate(1u64 << 40, RESIDENT_ARENA_BYTES, 4096).unwrap();
    assert_eq!(
        OwnershipRecord::encode_into(&mut page, &[descriptor(0x100000, 256, 1)], wide_arena, 32, 1),
        Err(OwnershipError::Address(AddressError::OutsidePhysicalWidth))
    );
    assert_eq!(page, [0xaa; 4096]);
}
