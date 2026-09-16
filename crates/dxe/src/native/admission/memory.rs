//! Bounded actual GetMemoryMap collection for opt-in native preflight.
//! Allocations precede the final map call. Neither descriptors nor this module
//! dereference any physical address described by the map. The map key is stale
//! after subsequent firmware allocations/frees and is not an ExitBootServices token.
use core::{mem::size_of, ptr::NonNull};
use svmvisor_hypervisor::boot::memory::MemoryDescriptor;
use uefi_raw::{
    Status,
    table::boot::{BootServices, MemoryType},
};

const MAX_MAP_BYTES: usize = 1024 * 1024;
const MAX_DESCRIPTORS: usize = 4096;
/// Maximum pages touched by any exact pool extent, including unaligned ends.
pub const MAX_STORAGE_COVERING_PAGES: usize =
    (MAX_MAP_BYTES + MAX_DESCRIPTORS * size_of::<MemoryDescriptor>() + 8190) / 4096;
const MAX_ATTEMPTS: usize = 4;
const _: () = assert!(core::mem::align_of::<MemoryDescriptor>() <= 8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryMapError {
    Firmware(Status),
    Bounds,
    Layout,
    RetryLimit,
    Cleanup(Status),
    Released,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapMetadata {
    pub key: usize,
    pub descriptor_size: usize,
    pub descriptor_version: u32,
    pub bytes: usize,
}

/// Exact AllocatePool extent, retained independently of used map bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageRange {
    pub base: u64,
    pub bytes: u64,
}

/// Owns one BootServicesData pool holding wire map bytes and decoded records.
/// Call release explicitly to observe firmware cleanup status; Drop is a final
/// best-effort safeguard for ordinary Rust error paths. A failed explicit free
/// retains ownership so the caller may retry. Firmware FreePool failure cannot
/// be represented as successful cleanup.
pub struct MemoryMapSnapshot<'a> {
    services: &'a BootServices,
    pool: Option<NonNull<u8>>,
    allocation_bytes: usize,
    record_offset: usize,
    count: usize,
    metadata: MapMetadata,
}
impl MemoryMapSnapshot<'_> {
    pub fn storage_range(&self) -> Result<StorageRange, MemoryMapError> {
        live_storage_range(self.pool, self.allocation_bytes)
    }
    pub fn descriptors(&self) -> &[MemoryDescriptor] {
        let Some(pool) = self.pool else {
            return &[];
        };
        unsafe {
            core::slice::from_raw_parts(pool.as_ptr().add(self.record_offset).cast(), self.count)
        }
    }
    pub const fn metadata(&self) -> MapMetadata {
        self.metadata
    }
    pub fn release(&mut self) -> Result<(), Status> {
        if let Some(pool) = self.pool {
            let status = unsafe { (self.services.free_pool)(pool.as_ptr()) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.pool = None;
            self.count = 0;
        }
        Ok(())
    }
}
fn live_storage_range(
    pool: Option<NonNull<u8>>,
    allocation_bytes: usize,
) -> Result<StorageRange, MemoryMapError> {
    let pool = pool.ok_or(MemoryMapError::Released)?;
    Ok(StorageRange {
        base: pool.as_ptr() as u64,
        bytes: allocation_bytes as u64,
    })
}
impl Drop for MemoryMapSnapshot<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn capacity(required: usize, stride: usize) -> Result<(usize, usize), MemoryMapError> {
    if !(40..=MAX_MAP_BYTES).contains(&required) || !(40..=4096).contains(&stride) {
        return Err(MemoryMapError::Bounds);
    }
    let map = required
        .checked_add(stride * 8)
        .and_then(|n| n.checked_add(7))
        .map(|n| n & !7)
        .filter(|&n| n <= MAX_MAP_BYTES)
        .ok_or(MemoryMapError::Bounds)?;
    // Reserve the maximum decoded count before GetMemoryMap, avoiding a second
    // allocation invalidating the snapshot. This is bounded at128KiB.
    let total = map
        .checked_add(MAX_DESCRIPTORS * size_of::<MemoryDescriptor>())
        .ok_or(MemoryMapError::Bounds)?;
    Ok((map, total))
}

fn decode(bytes: &[u8]) -> MemoryDescriptor {
    MemoryDescriptor {
        memory_type: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        physical_start: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        page_count: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
        attributes: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
    }
}

/// # Safety
/// services must be the live Boot Services table, used synchronously at the
/// native preflight's admitted firmware TPL with conforming pool/map functions.
/// Boot Services must remain live until this object is released. This is a
/// point-in-time map, not a lease preventing firmware allocations or changes.
/// Consumers must use a short reviewed sequence without intervening caller
/// allocations/frees or permission changes. Read-only memory-attribute queries
/// may occur; they do not make this snapshot a lease or prove its stability.
pub unsafe fn collect(services: &BootServices) -> Result<MemoryMapSnapshot<'_>, MemoryMapError> {
    let mut required = 0;
    let mut metadata = MapMetadata::default();
    let status = unsafe {
        (services.get_memory_map)(
            &mut required,
            core::ptr::null_mut(),
            &mut metadata.key,
            &mut metadata.descriptor_size,
            &mut metadata.descriptor_version,
        )
    };
    if status != Status::BUFFER_TOO_SMALL {
        return Err(MemoryMapError::Firmware(status));
    }
    for _ in 0..MAX_ATTEMPTS {
        let (map_capacity, total) = capacity(required, metadata.descriptor_size)?;
        let mut raw = core::ptr::null_mut();
        let status =
            unsafe { (services.allocate_pool)(MemoryType::BOOT_SERVICES_DATA, total, &mut raw) };
        if status != Status::SUCCESS {
            return Err(MemoryMapError::Firmware(status));
        }
        let pool = NonNull::new(raw).ok_or(MemoryMapError::Layout)?;
        let mut owned = MemoryMapSnapshot {
            services,
            pool: Some(pool),
            allocation_bytes: total,
            record_offset: map_capacity,
            count: 0,
            metadata,
        };
        let mut size = map_capacity;
        let status = unsafe {
            (services.get_memory_map)(
                &mut size,
                raw.cast(),
                &mut metadata.key,
                &mut metadata.descriptor_size,
                &mut metadata.descriptor_version,
            )
        };
        if status == Status::BUFFER_TOO_SMALL {
            owned.release().map_err(MemoryMapError::Cleanup)?;
            required = size;
            continue;
        }
        let error = if status != Status::SUCCESS {
            Some(MemoryMapError::Firmware(status))
        } else if metadata.descriptor_version != 1
            || metadata.descriptor_size < 40
            || metadata.descriptor_size > 4096
            || size == 0
            || size > map_capacity
            || size % metadata.descriptor_size != 0
            || size / metadata.descriptor_size > MAX_DESCRIPTORS
        {
            Some(MemoryMapError::Layout)
        } else {
            None
        };
        if let Some(error) = error {
            owned.release().map_err(MemoryMapError::Cleanup)?;
            return Err(error);
        }
        owned.count = size / metadata.descriptor_size;
        metadata.bytes = size;
        owned.metadata = metadata;
        for index in 0..owned.count {
            let bytes = unsafe {
                core::slice::from_raw_parts(raw.add(index * metadata.descriptor_size), 40)
            };
            unsafe {
                raw.add(map_capacity)
                    .cast::<MemoryDescriptor>()
                    .add(index)
                    .write(decode(bytes));
            }
        }
        let records = unsafe {
            core::slice::from_raw_parts_mut(
                raw.add(map_capacity).cast::<MemoryDescriptor>(),
                owned.count,
            )
        };
        if let Err(error) = sort_records(records) {
            owned.release().map_err(MemoryMapError::Cleanup)?;
            return Err(error);
        }
        return Ok(owned);
    }
    Err(MemoryMapError::RetryLimit)
}

// Generic core sorting retains a comparator-consistency panic even for this
// numeric key, which violates the DXE no-reachable-panic link guard. Bounded
// insertion sorting needs no allocation, comparator callback or panic path.
fn sort_records(records: &mut [MemoryDescriptor]) -> Result<(), MemoryMapError> {
    if records.len() > MAX_DESCRIPTORS {
        return Err(MemoryMapError::Bounds);
    }
    for index in 1..records.len() {
        let value = records.get(index).copied().ok_or(MemoryMapError::Bounds)?;
        let mut position = index;
        while position > 0 {
            let previous = records
                .get(position - 1)
                .copied()
                .ok_or(MemoryMapError::Bounds)?;
            if previous.physical_start <= value.physical_start {
                break;
            }
            *records.get_mut(position).ok_or(MemoryMapError::Bounds)? = previous;
            position -= 1;
        }
        *records.get_mut(position).ok_or(MemoryMapError::Bounds)? = value;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_extent_is_exact_and_refuses_absent_released_pool() {
        let pointer = NonNull::new(0x1003usize as *mut u8);
        let (_, total) = capacity(481, 48).unwrap();
        assert_eq!(
            live_storage_range(pointer, total),
            Ok(StorageRange {
                base: 0x1003,
                bytes: total as u64
            })
        );
        assert_eq!(
            live_storage_range(None, total),
            Err(MemoryMapError::Released)
        );
        assert_eq!(MAX_STORAGE_COVERING_PAGES, 289);
    }
    #[test]
    fn wire_descriptor_uses_uefi_offsets_not_rust_layout() {
        let mut bytes = [0xee; 48];
        bytes[0..4].copy_from_slice(&4u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&0x12345000u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x234u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&8u64.to_le_bytes());
        let parsed = decode(&bytes);
        assert_eq!(parsed.memory_type, 4);
        assert_eq!(parsed.physical_start, 0x12345000);
        assert_eq!(parsed.page_count, 0x234);
        assert_eq!(parsed.attributes, 8);
    }
    #[test]
    fn map_capacity_is_aligned_bounded_and_includes_record_storage() {
        let (map, total) = capacity(481, 48).unwrap();
        assert_eq!(map % 8, 0);
        assert!(map >= 481 + 8 * 48);
        assert_eq!(total - map, MAX_DESCRIPTORS * size_of::<MemoryDescriptor>());
        for (required, stride) in [
            (0, 48),
            (40, 39),
            (MAX_MAP_BYTES, 48),
            (usize::MAX, 48),
            (4096, usize::MAX),
        ] {
            assert_eq!(capacity(required, stride), Err(MemoryMapError::Bounds));
        }
    }
}

#[cfg(test)]
#[test]
fn sorting_preserves_full_records_including_duplicate_keys() {
    let mut records = [
        MemoryDescriptor {
            physical_start: 0x3000,
            memory_type: 1,
            page_count: 3,
            attributes: 8,
        },
        MemoryDescriptor {
            physical_start: 0x1000,
            memory_type: 2,
            page_count: 1,
            attributes: 9,
        },
        MemoryDescriptor {
            physical_start: 0x2000,
            memory_type: 3,
            page_count: 2,
            attributes: 10,
        },
        MemoryDescriptor {
            physical_start: 0x1000,
            memory_type: 4,
            page_count: 4,
            attributes: 11,
        },
    ];
    let original = records;
    assert_eq!(sort_records(&mut records), Ok(()));
    assert_eq!(
        records,
        [original[1], original[3], original[2], original[0]]
    );
    assert_eq!(sort_records(&mut records), Ok(()));
    assert_eq!(
        records,
        [original[1], original[3], original[2], original[0]]
    );
    assert_eq!(sort_records(&mut []), Ok(()));
}
