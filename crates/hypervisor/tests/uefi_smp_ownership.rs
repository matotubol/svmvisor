use svmvisor_hypervisor::{
    boot::{
        memory::{MemoryDescriptor, MemoryError},
        ownership::*,
    },
    memory::address::{AddressError, AddressPolicy, EncryptionState, PhysicalRange},
};

fn page() -> [u8; HANDOFF_PAGE_BYTES] {
    let mut bytes = [0xa5; HANDOFF_PAGE_BYTES];
    OwnershipRecord::encode_with_smp(&mut bytes, &descriptors(), arena(), 48, 1, Some(smp()))
        .unwrap();
    bytes
}

fn smp() -> SmpResources {
    SmpResources::new(range(0x8000, 4096), cpus(), 1).unwrap()
}

fn cpus() -> [SmpCpuIdentity; 2] {
    [0, 1].map(|id| SmpCpuIdentity {
        processor_id: id as u64,
        apic_id: id,
        signature: 0x800f12,
        vendor: *b"AuthenticAMD",
    })
}

fn arena() -> PhysicalRange {
    range(0x180000, RESIDENT_ARENA_BYTES)
}

fn range(base: u64, bytes: u64) -> PhysicalRange {
    policy().validate(base, bytes, 1).unwrap()
}

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

fn descriptors() -> [MemoryDescriptor; 1] {
    [descriptor(0, 1024, 1)]
}

fn descriptor(start: u64, pages: u64, kind: u32) -> MemoryDescriptor {
    MemoryDescriptor { memory_type: kind, physical_start: start, page_count: pages, attributes: 8 }
}

fn put32(page: &mut [u8], relative: usize, value: u32) {
    page[OWNERSHIP_OFFSET + relative..OWNERSHIP_OFFSET + relative + 4]
        .copy_from_slice(&value.to_le_bytes());
}

fn put64(page: &mut [u8], relative: usize, value: u64) {
    page[OWNERSHIP_OFFSET + relative..OWNERSHIP_OFFSET + relative + 8]
        .copy_from_slice(&value.to_le_bytes());
}

#[test]
fn smp_wire_offsets_and_owned_roundtrip() {
    let page = page();
    assert_eq!(&page[..64], &[0xa5; 64]);
    let wire = &page[OWNERSHIP_OFFSET..];
    assert_eq!(&wire[..8], b"SVMOWN01");
    assert_eq!(&wire[8..20], &[2, 0, 160, 0, 28, 0, 1, 0, 1, 0, 0, 0]);
    assert_eq!(&wire[24..32], &arena().base().to_le_bytes());
    assert_eq!(&wire[32..40], &RESIDENT_ARENA_BYTES.to_le_bytes());
    assert_eq!(&wire[64..72], &0x8000u64.to_le_bytes());
    assert_eq!(&wire[72..80], &4096u64.to_le_bytes());
    assert_eq!(&wire[80..88], &[1, 0, 0, 0, 2, 0, 0, 0]);
    for (index, cpu) in cpus().into_iter().enumerate() {
        let offset = 96 + index * 32;
        assert_eq!(&wire[offset..offset + 8], &cpu.processor_id.to_le_bytes());
        assert_eq!(&wire[offset + 8..offset + 12], &cpu.apic_id.to_le_bytes());
        assert_eq!(&wire[offset + 12..offset + 16], &cpu.signature.to_le_bytes());
        assert_eq!(&wire[offset + 16..offset + 28], &cpu.vendor);
    }
    assert_eq!(&wire[160..164], &1u32.to_le_bytes());
    assert_eq!(&wire[164..172], &0u64.to_le_bytes());
    assert_eq!(&wire[172..180], &1024u64.to_le_bytes());
    assert_eq!(&wire[180..188], &8u64.to_le_bytes());
    let record = OwnershipRecord::decode(&page, &policy()).unwrap();
    assert_eq!(record.smp(), Some(smp()));
    let retained = record.smp().unwrap();
    assert_eq!(retained.cpus(), &cpus());
    assert_eq!(retained.low_page(), range(0x8000, 4096));
    assert_eq!(retained.returned_ap_callbacks(), 1);
    assert_eq!(record.descriptors().collect::<Vec<_>>(), descriptors());
    for (offset, value) in [(8, 1u16), (10, 64), (12, 32)] {
        let mut wrong_format = page;
        wrong_format[OWNERSHIP_OFFSET + offset..OWNERSHIP_OFFSET + offset + 2]
            .copy_from_slice(&value.to_le_bytes());
        assert_eq!(
            OwnershipRecord::decode(&wrong_format, &policy()).unwrap_err(),
            OwnershipError::Size
        );
    }
}

#[test]
fn supplied_cpu_topology_and_callback_evidence_are_exact() {
    for index in 0..2 {
        for field in 0..5 {
            let mut identities = cpus();
            match field {
                0 => identities[index].processor_id ^= 1,
                1 => identities[index].apic_id ^= 1,
                2 => identities[index].vendor[0] ^= 1,
                3 => identities[index].signature = 0,
                4 => identities[index].signature ^= 1,
                _ => unreachable!(),
            }
            assert_eq!(
                SmpResources::new(range(0x8000, 4096), identities, 1),
                Err(OwnershipError::SmpIdentity)
            );
        }
    }
    for callbacks in [0, 2, u32::MAX] {
        assert_eq!(
            SmpResources::new(range(0x8000, 4096), cpus(), callbacks),
            Err(OwnershipError::SmpCallbackCount)
        );
    }
}

#[test]
fn supplied_sipi_extent_is_one_aligned_nonzero_page_below_one_megabyte() {
    for (base, bytes) in
        [(0, 4096), (1, 4096), (0x8001, 4096), (0x8000, 4095), (0x8000, 8192), (0x100000, 4096)]
    {
        assert_eq!(
            SmpResources::new(range(base, bytes), cpus(), 1),
            Err(OwnershipError::SmpLowPage)
        );
    }
    for base in [4096, 0xff000] {
        assert!(SmpResources::new(range(base, 4096), cpus(), 1).is_ok());
    }
}

#[test]
fn both_reservations_split_one_descriptor_without_losing_bytes_or_attributes() {
    let page = page();
    let record = OwnershipRecord::decode(&page, &policy()).unwrap();
    let guest: Vec<_> = record.guest_descriptors().collect();
    assert_eq!(
        guest,
        vec![
            descriptor(0, 8, 1),
            descriptor(0x8000, 1, 0),
            descriptor(0x9000, 375, 1),
            descriptor(0x180000, 256, 0),
            descriptor(0x280000, 384, 1),
        ]
    );
    assert_eq!(guest.iter().map(|d| d.page_count).sum::<u64>(), 1024);
    for (base, bytes, excluded) in [
        (0x7000, 4096, false),
        (0x7000, 4097, true),
        (0x8000, 4096, true),
        (0x8fff, 2, true),
        (0x9000, 4096, false),
        (0x17ffff, 2, true),
        (0x27ffff, 2, true),
        (0x280000, 4096, false),
    ] {
        assert_eq!(record.excludes_guest_range(range(base, bytes)), excluded);
    }
}

#[test]
fn low_page_map_eligibility_and_disjointness_refuse_transactionally() {
    for (low_descriptor, error) in [
        (descriptor(0x9000, 1, 1), OwnershipError::SmpPageUncovered),
        (descriptor(0x8000, 1, 2), OwnershipError::SmpPageNotLoaderCode),
        (
            MemoryDescriptor { attributes: 0, ..descriptor(0x8000, 1, 1) },
            OwnershipError::Map(MemoryError::MissingWriteBackCapability),
        ),
        (
            MemoryDescriptor { attributes: 0x2008, ..descriptor(0x8000, 1, 1) },
            OwnershipError::Map(MemoryError::ReadProtected),
        ),
    ] {
        let mut bytes = [0xa5; HANDOFF_PAGE_BYTES];
        assert_eq!(
            OwnershipRecord::encode_with_smp(
                &mut bytes,
                &[low_descriptor, descriptor(0x100000, 512, 1)],
                arena(),
                48,
                1,
                Some(smp())
            ),
            Err(error)
        );
        assert_eq!(bytes, [0xa5; HANDOFF_PAGE_BYTES]);
    }
    let mut bytes = [0xa5; HANDOFF_PAGE_BYTES];
    assert_eq!(
        OwnershipRecord::encode_with_smp(
            &mut bytes,
            &descriptors(),
            range(0x1000, RESIDENT_ARENA_BYTES),
            48,
            1,
            Some(smp())
        ),
        Err(OwnershipError::SmpArenaOverlap)
    );
    assert_eq!(bytes, [0xa5; HANDOFF_PAGE_BYTES]);
}

#[test]
fn decode_revalidates_smp_metadata_and_address_policy() {
    for (offset, value, error) in [
        (80, 0, OwnershipError::SmpCallbackCount),
        (80, 2, OwnershipError::SmpCallbackCount),
        (84, 0, OwnershipError::SmpIdentity),
        (84, 3, OwnershipError::SmpIdentity),
        (96, 1, OwnershipError::SmpIdentity),
        (104, 1, OwnershipError::SmpIdentity),
        (108, 0, OwnershipError::SmpIdentity),
        (128, 0, OwnershipError::SmpIdentity),
        (136, 2, OwnershipError::SmpIdentity),
        (140, 0, OwnershipError::SmpIdentity),
    ] {
        let mut bytes = page();
        put32(&mut bytes, offset, value);
        assert_eq!(
            OwnershipRecord::decode(&bytes, &policy()).unwrap_err(),
            error,
            "offset {offset}"
        );
    }
    for (base, length, error) in [
        (0, 4096, OwnershipError::SmpLowPage),
        (0x100000, 4096, OwnershipError::SmpLowPage),
        (0x8000, 8192, OwnershipError::SmpLowPage),
        (0x8001, 4096, OwnershipError::Address(AddressError::Misaligned)),
    ] {
        let mut bytes = page();
        put64(&mut bytes, 64, base);
        put64(&mut bytes, 72, length);
        assert_eq!(OwnershipRecord::decode(&bytes, &policy()).unwrap_err(), error);
    }
    let mut bytes = page();
    put64(&mut bytes, 24, 0x1000);
    assert_eq!(
        OwnershipRecord::decode(&bytes, &policy()).unwrap_err(),
        OwnershipError::SmpArenaOverlap
    );
    for offset in [112, 144] {
        let mut bytes = page();
        bytes[OWNERSHIP_OFFSET + offset] ^= 1;
        assert_eq!(
            OwnershipRecord::decode(&bytes, &policy()).unwrap_err(),
            OwnershipError::SmpIdentity
        );
    }
}

#[test]
fn every_smp_reserved_byte_is_rejected() {
    for offset in (20..24)
        .chain(40..64)
        .chain(88..96)
        .chain(124..128)
        .chain(156..160)
        .chain(188..HANDOFF_PAGE_BYTES - OWNERSHIP_OFFSET)
    {
        let mut bytes = page();
        bytes[OWNERSHIP_OFFSET + offset] = 1;
        assert_eq!(
            OwnershipRecord::decode(&bytes, &policy()).unwrap_err(),
            OwnershipError::Reserved,
            "offset {offset}"
        );
    }
}

#[test]
fn version_specific_capacity_is_exact_and_extra_descriptors_are_not_dropped() {
    assert_eq!(MAX_OWNERSHIP_SMP_DESCRIPTORS, 138);
    let mut map = vec![descriptor(0, 1024, 1)];
    for index in 1..=MAX_OWNERSHIP_SMP_DESCRIPTORS {
        map.push(descriptor(0x400000 + index as u64 * 4096, 1, 7));
    }
    let mut bytes = [0xa5; HANDOFF_PAGE_BYTES];
    OwnershipRecord::encode_with_smp(&mut bytes, &map[..138], arena(), 48, 1, Some(smp())).unwrap();
    assert_eq!(OwnershipRecord::decode(&bytes, &policy()).unwrap().descriptor_count(), 138);
    assert_eq!(&bytes[4088..], &[0; 8]);
    assert_eq!(
        OwnershipRecord::decode(&bytes, &policy()).unwrap().descriptors().collect::<Vec<_>>(),
        map[..138]
    );
    let previous = bytes;
    for count in [0, 139] {
        assert_eq!(
            OwnershipRecord::encode_with_smp(
                &mut bytes,
                &map[..count],
                arena(),
                48,
                1,
                Some(smp())
            ),
            Err(OwnershipError::DescriptorCount)
        );
        assert_eq!(bytes, previous);
    }
    for count in [0u16, 139, u16::MAX] {
        let mut malformed = previous;
        malformed[78..80].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            OwnershipRecord::decode(&malformed, &policy()).unwrap_err(),
            OwnershipError::DescriptorCount
        );
    }
    for offset in 4088..4096 {
        let mut malformed = previous;
        malformed[offset] = 1;
        assert_eq!(
            OwnershipRecord::decode(&malformed, &policy()).unwrap_err(),
            OwnershipError::Reserved
        );
    }
    OwnershipRecord::encode_into(&mut bytes, &map[..124], arena(), 48, 1).unwrap();
    assert_eq!(OwnershipRecord::decode(&bytes, &policy()).unwrap().descriptor_count(), 124);
    assert_eq!(OwnershipRecord::decode(&bytes, &policy()).unwrap().smp(), None);
    let mut explicit_none = [0xa5; HANDOFF_PAGE_BYTES];
    OwnershipRecord::encode_with_smp(&mut explicit_none, &map[..124], arena(), 48, 1, None)
        .unwrap();
    assert_eq!(bytes, explicit_none);
}

#[test]
fn decode_rechecks_low_page_final_map_permissions_and_coverage() {
    let map = [descriptor(0x8000, 1, 1), descriptor(0x100000, 512, 1)];
    let mut valid = [0; HANDOFF_PAGE_BYTES];
    OwnershipRecord::encode_with_smp(&mut valid, &map, arena(), 48, 1, Some(smp())).unwrap();
    for (offset, value, error) in [
        (160, 2, OwnershipError::SmpPageNotLoaderCode),
        (164, 0x9000, OwnershipError::SmpPageUncovered),
        (180, 0, OwnershipError::Map(MemoryError::MissingWriteBackCapability)),
        (180, 0x2008, OwnershipError::Map(MemoryError::ReadProtected)),
    ] {
        let mut bytes = valid;
        put32(&mut bytes, offset, value);
        assert_eq!(OwnershipRecord::decode(&bytes, &policy()).unwrap_err(), error);
    }
    let encryption =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: Some(15) }).unwrap();
    assert_eq!(
        OwnershipRecord::decode(&valid, &encryption).unwrap_err(),
        OwnershipError::Address(AddressError::EncryptionBitEncoded)
    );
}

#[test]
fn allocated_low_page_boundaries_accept_coexisting_wb_capabilities() {
    // The actual initial OVMF run allocated 9f000. A descriptor ending exactly
    // at A0000 covers its whole page; the maximum SIPI page ends at 100000.
    // Other advertised cache capabilities may coexist with required WB.
    for base in [0x9f000, 0xff000] {
        let resources = SmpResources::new(range(base, 4096), cpus(), 1).unwrap();
        let map = [
            MemoryDescriptor { attributes: 0xf, ..descriptor(base, 1, 1) },
            descriptor(0x100000, 512, 1),
        ];
        let mut bytes = [0; HANDOFF_PAGE_BYTES];
        OwnershipRecord::encode_with_smp(&mut bytes, &map, arena(), 48, 1, Some(resources))
            .unwrap();
        let record = OwnershipRecord::decode(&bytes, &policy()).unwrap();
        assert_eq!(record.smp(), Some(resources));
        assert_eq!(
            record.guest_descriptors().next().unwrap(),
            MemoryDescriptor { memory_type: 0, ..map[0] }
        );
    }
}
