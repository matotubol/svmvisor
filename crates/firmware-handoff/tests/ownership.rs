//! Host checks for normalization only; no successful EBS is simulated or proven.

#[path = "../src/ownership.rs"]
mod ownership;

use svmvisor_hypervisor::{
    boot::{
        memory::MemoryDescriptor,
        ownership::{
            MAX_OWNERSHIP_DESCRIPTORS, MAX_OWNERSHIP_SMP_DESCRIPTORS, OwnershipRecord,
            SmpCpuIdentity, SmpResources,
        },
    },
    memory::address::{AddressPolicy, EncryptionState},
};
use uefi::mem::memory_map::{MemoryMapKey, MemoryMapMeta, MemoryMapRef};

const STRIDE: usize = 48;

#[repr(align(8))]
struct MapBytes([u8; (MAX_OWNERSHIP_SMP_DESCRIPTORS + 1) * STRIDE]);

impl MapBytes {
    fn new() -> Self {
        Self([0; (MAX_OWNERSHIP_SMP_DESCRIPTORS + 1) * STRIDE])
    }

    fn entry(&mut self, index: usize, kind: u32, start: u64, pages: u64) {
        let slot = &mut self.0[index * STRIDE..(index + 1) * STRIDE];
        slot[0..4].copy_from_slice(&kind.to_le_bytes());
        slot[4..8].fill(0xa5); // ABI padding must not become record data.
        slot[8..16].copy_from_slice(&start.to_le_bytes());
        slot[16..24].copy_from_slice(&0u64.to_le_bytes());
        slot[24..32].copy_from_slice(&pages.to_le_bytes());
        slot[32..40].copy_from_slice(&8u64.to_le_bytes());
        slot[40..48].fill(0x5a); // Version-1 descriptor extension bytes.
    }

    fn map(&self, count: usize, version: u32) -> MemoryMapRef<'_> {
        MemoryMapRef::new(
            &self.0,
            MemoryMapMeta {
                map_size: count * STRIDE,
                desc_size: STRIDE,
                map_key: MemoryMapKey::default(),
                desc_version: version,
            },
        )
        .unwrap()
    }
}

fn policy() -> AddressPolicy {
    AddressPolicy::new(52, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

fn descriptor(start: u64, pages: u64, kind: u32) -> MemoryDescriptor {
    MemoryDescriptor { memory_type: kind, physical_start: start, page_count: pages, attributes: 8 }
}

#[test]
fn unsorted_stride48_map_is_normalized_without_padding_or_extensions() {
    let mut bytes = MapBytes::new();
    bytes.entry(0, 7, 0x200000, 256);
    bytes.entry(1, 1, 0x100000, 256);
    bytes.entry(2, 0, 0, 256);
    let mut page = [0xa5; 4096];
    ownership::retain(&bytes.map(3, 1), &mut page, 0x100000).unwrap();
    assert_eq!(&page[..64], &[0xa5; 64]);
    let record = OwnershipRecord::decode(&page, &policy()).unwrap();
    assert_eq!(
        record.descriptors().collect::<Vec<_>>(),
        vec![descriptor(0, 256, 0), descriptor(0x100000, 256, 1), descriptor(0x200000, 256, 7)]
    );
    for slot in page[128..224].chunks_exact(32) {
        assert_eq!(&slot[4..8], &[0; 4]);
    }
    assert!(page[224..].iter().all(|b| *b == 0));
}

#[test]
fn fragmented_contiguous_arena_is_accepted_and_projected_entirely_reserved() {
    let mut bytes = MapBytes::new();
    bytes.entry(0, 1, 0x180000, 128);
    bytes.entry(1, 1, 0x100000, 128);
    let mut page = [0; 4096];
    ownership::retain(&bytes.map(2, 1), &mut page, 0x100000).unwrap();
    let record = OwnershipRecord::decode(&page, &policy()).unwrap();
    assert_eq!(
        record.guest_descriptors().collect::<Vec<_>>(),
        vec![descriptor(0x100000, 128, 0), descriptor(0x180000, 128, 0)]
    );
    bytes.entry(0, 1, 0x181000, 127);
    let before = page;
    assert_eq!(ownership::retain(&bytes.map(2, 1), &mut page, 0x100000), Err(()));
    assert_eq!(page, before);
}

#[test]
fn virtual_mapping_and_unknown_version_refuse_without_changing_page() {
    let mut bytes = MapBytes::new();
    bytes.entry(0, 1, 0x100000, 256);
    let mut page = [0xcc; 4096];
    assert_eq!(ownership::retain(&bytes.map(1, 2), &mut page, 0x100000), Err(()));
    assert_eq!(page, [0xcc; 4096]);
    bytes.0[16..24].copy_from_slice(&0x100000u64.to_le_bytes());
    assert_eq!(ownership::retain(&bytes.map(1, 1), &mut page, 0x100000), Err(()));
    assert_eq!(page, [0xcc; 4096]);
}

#[test]
fn overcapacity_and_empty_maps_refuse_without_omission_or_page_mutation() {
    let mut bytes = MapBytes::new();
    bytes.entry(0, 1, 0x100000, 256);
    for index in 1..=MAX_OWNERSHIP_DESCRIPTORS {
        bytes.entry(index, 7, 0x200000 + (index as u64) * 4096, 1);
    }
    let mut page = [0xcc; 4096];
    assert_eq!(
        ownership::retain(&bytes.map(MAX_OWNERSHIP_DESCRIPTORS + 1, 1), &mut page, 0x100000),
        Err(())
    );
    assert_eq!(page, [0xcc; 4096]);
    assert_eq!(ownership::retain(&bytes.map(0, 1), &mut page, 0x100000), Err(()));
    assert_eq!(page, [0xcc; 4096]);
    ownership::retain(&bytes.map(MAX_OWNERSHIP_DESCRIPTORS, 1), &mut page, 0x100000).unwrap();
    assert_eq!(
        OwnershipRecord::decode(&page, &policy()).unwrap().descriptor_count(),
        MAX_OWNERSHIP_DESCRIPTORS
    );
}

#[test]
fn smp_normalization_admits_low_page_and_rechecks_the_final_map_transactionally() {
    for base in [0x7000, 0x9f000] {
        let cpus = [0, 1].map(|id| SmpCpuIdentity {
            processor_id: id as u64,
            apic_id: id,
            signature: 0x00a00f11,
            vendor: *b"AuthenticAMD",
        });
        let smp = SmpResources::new(policy().validate(base, 4096, 4096).unwrap(), cpus, 1).unwrap();
        let mut bytes = MapBytes::new();
        bytes.entry(0, 1, 0x200000, 256);
        bytes.entry(1, 1, base, 1);
        let mut page = [0xa5; 4096];
        ownership::retain_with_smp(&bytes.map(2, 1), &mut page, 0x200000, Some(smp)).unwrap();
        let record = OwnershipRecord::decode(&page, &policy()).unwrap();
        assert_eq!(record.smp().unwrap().low_page().base(), base);
        assert_eq!(record.smp().unwrap().returned_ap_callbacks(), 1);
        assert_eq!(record.descriptor_count(), 2);
        assert_eq!(&page[..64], &[0xa5; 64]);

        let before = page;
        // A changed final map cannot inherit the preliminary map's admission.
        bytes.entry(1, 7, base, 1);
        assert_eq!(
            ownership::retain_detailed(&bytes.map(2, 1), &mut page, 0x200000, Some(smp)),
            Err(ownership::RetainError::Record(
                svmvisor_hypervisor::boot::ownership::OwnershipError::SmpPageNotLoaderCode
            ))
        );
        assert_eq!(page, before);
        assert_eq!(
            ownership::retain_with_smp(&bytes.map(2, 1), &mut page, 0x200000, Some(smp)),
            Err(())
        );
        assert_eq!(page, before);
        bytes.entry(1, 1, base + 4096, 1);
        assert_eq!(
            ownership::retain_with_smp(&bytes.map(2, 1), &mut page, 0x200000, Some(smp)),
            Err(())
        );
        assert_eq!(page, before);
    }
}

#[test]
fn smp_full_map_capacity_retains_every_descriptor_without_coalescing() {
    let cpus = [0, 1].map(|id| SmpCpuIdentity {
        processor_id: id as u64,
        apic_id: id,
        signature: 0x663,
        vendor: *b"AuthenticAMD",
    });
    let smp = SmpResources::new(policy().validate(0x9f000, 4096, 4096).unwrap(), cpus, 1).unwrap();
    let mut bytes = MapBytes::new();
    bytes.entry(0, 1, 0x200000, 256);
    bytes.entry(1, 1, 0x9f000, 1);
    // Identical adjacent descriptors deliberately remain separate. The map
    // has the observed 131 entries or the exact v2 maximum, not a subset.
    for index in 2..=MAX_OWNERSHIP_SMP_DESCRIPTORS {
        bytes.entry(index, 7, 0x400000 + index as u64 * 4096, 1);
    }
    let mut page = [0xa5; 4096];
    for count in [131, MAX_OWNERSHIP_SMP_DESCRIPTORS] {
        ownership::retain_with_smp(&bytes.map(count, 1), &mut page, 0x200000, Some(smp)).unwrap();
        let record = OwnershipRecord::decode(&page, &policy()).unwrap();
        assert_eq!(record.descriptor_count(), count);
        let decoded = record.descriptors().collect::<Vec<_>>();
        assert_eq!(decoded[0], descriptor(0x9f000, 1, 1));
        assert_eq!(decoded[1], descriptor(0x200000, 256, 1));
        for (index, actual) in decoded.iter().enumerate().skip(2) {
            assert_eq!(*actual, descriptor(0x400000 + index as u64 * 4096, 1, 7));
        }
    }
    let before = page;
    assert_eq!(
        ownership::retain_detailed(
            &bytes.map(MAX_OWNERSHIP_SMP_DESCRIPTORS + 1, 1),
            &mut page,
            0x200000,
            Some(smp)
        ),
        Err(ownership::RetainError::DescriptorCount)
    );
    assert_eq!(page, before);
    assert_eq!(
        ownership::retain_detailed(&bytes.map(131, 1), &mut page, 0x200000, None),
        Err(ownership::RetainError::DescriptorCount)
    );
    assert_eq!(page, before);
}
