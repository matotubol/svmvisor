//! Final EBS map normalization into the exclusively allocated handoff page.
//! UEFI 2.11 7.2.3/7.4.6; pinned uefi-rs 0.39.0 entries() respects stride.
use core::arch::x86_64::__cpuid;
use svmvisor_hypervisor::{
    boot::{
        memory::MemoryDescriptor,
        ownership::{
            MAX_OWNERSHIP_DESCRIPTORS, MAX_OWNERSHIP_SMP_DESCRIPTORS, OwnershipError,
            OwnershipRecord, SmpResources,
        },
    },
    memory::address::{AddressError, AddressPolicy, EncryptionState},
};
use uefi::mem::memory_map::MemoryMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainError {
    DescriptorCount,
    VirtualMapping,
    MissingAddressLeaf,
    Address(AddressError),
    Record(OwnershipError),
}

/// The caller supplies only the map returned by successful ExitBootServices.
/// This normalized physical snapshot is not a SetVirtualAddressMap input.
/// Descriptor extensions/padding are omitted; nonzero virtual mappings and
/// descriptor versions other than 1 are refused. Unencrypted TCG is a fixture
/// assumption, not native platform qualification. No allocation occurs here.
pub fn retain(map: &impl MemoryMap, page: &mut [u8], arena_base: u64) -> Result<(), ()> {
    retain_with_smp(map, page, arena_base, None)
}

/// Normalize a snapshot with optional admitted two-CPU resources. The caller
/// uses a preliminary map before EBS for admission and must overwrite it with
/// the final successful EBS map before transferring to the resident runtime.
pub fn retain_with_smp(
    map: &impl MemoryMap,
    page: &mut [u8],
    arena_base: u64,
    smp: Option<SmpResources>,
) -> Result<(), ()> {
    retain_detailed(map, page, arena_base, smp).map_err(|_| ())
}

/// Same transactional normalization with a bounded, allocation-free error.
pub fn retain_detailed(
    map: &impl MemoryMap,
    page: &mut [u8],
    arena_base: u64,
    smp: Option<SmpResources>,
) -> Result<(), RetainError> {
    let capacity = if smp.is_some() {
        MAX_OWNERSHIP_SMP_DESCRIPTORS
    } else {
        MAX_OWNERSHIP_DESCRIPTORS
    };
    if map.len() == 0 || map.len() > capacity {
        return Err(RetainError::DescriptorCount);
    }
    // Version 2 stores all descriptor fields in 28 bytes, omitting only ABI
    // padding. Its larger bounded capacity retains the complete two-CPU map;
    // no descriptor is dropped, merged, or relabeled to fit the handoff page.
    let mut descriptors = [MemoryDescriptor::default(); MAX_OWNERSHIP_SMP_DESCRIPTORS];
    for (slot, descriptor) in descriptors.iter_mut().zip(map.entries()) {
        if descriptor.virt_start != 0 {
            return Err(RetainError::VirtualMapping);
        }
        *slot = MemoryDescriptor {
            memory_type: descriptor.ty.0,
            physical_start: descriptor.phys_start,
            page_count: descriptor.page_count,
            attributes: descriptor.att.bits(),
        };
    }
    let descriptors = &mut descriptors[..map.len()];
    // Bounded insertion sort: the UEFI map has no sorted-order guarantee.
    for index in 1..descriptors.len() {
        let mut cursor = index;
        while cursor > 0
            && descriptors[cursor].physical_start < descriptors[cursor - 1].physical_start
        {
            descriptors.swap(cursor, cursor - 1);
            cursor -= 1;
        }
    }
    if __cpuid(0x80000000).eax < 0x80000008 {
        return Err(RetainError::MissingAddressLeaf);
    }
    let bits = __cpuid(0x80000008).eax as u8;
    let policy = AddressPolicy::new(
        bits,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .map_err(RetainError::Address)?;
    let arena = policy
        .validate(arena_base, 0x100000, 4096)
        .map_err(RetainError::Address)?;
    OwnershipRecord::encode_with_smp(page, descriptors, arena, bits, map.meta().desc_version, smp)
        .map_err(RetainError::Record)
}
