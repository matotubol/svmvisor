//! UEFI memory-map metadata checks for bounded paging/GDT observations.
//!
//! This does not establish virtual accessibility, identity mapping, ownership,
//! encryption state, effective PAT/MTRR caching, or stable table contents. A safe
//! callback must independently supply access. Unlike the older m0b ACPI reader,
//! no accepted physical address is converted into a native pointer here. WB is
//! only a required advertised capability. RP rejection is conservative metadata
//! policy; separate current permission evidence is required by a live adapter.

pub const MAX_DESCRIPTORS: usize = 4096;
pub const MAX_GDT_BYTES: usize = 65536;

#[derive(Debug)]
pub struct ValidatedMemoryMap<'a> {
    descriptors: &'a [MemoryDescriptor],
    physical_end: u64,
}

impl<'a> ValidatedMemoryMap<'a> {
    /// Require sorted input, permitting linear validation without allocation.
    /// Non-RAM descriptors remain in the map but cannot authorize any read.
    pub fn new(
        descriptors: &'a [MemoryDescriptor],
        physical_bits: u8,
    ) -> Result<Self, MemoryError> {
        use MemoryError::*;
        if descriptors.is_empty() || descriptors.len() > MAX_DESCRIPTORS {
            return Err(DescriptorCount);
        }
        if !(32..=52).contains(&physical_bits) {
            return Err(PhysicalWidth);
        }
        let physical_end = 1u64 << physical_bits;
        let mut previous_end = 0;
        for descriptor in descriptors {
            if descriptor.page_count == 0 {
                return Err(EmptyDescriptor);
            }
            if descriptor.physical_start & 4095 != 0 {
                return Err(MisalignedDescriptor);
            }
            let end = descriptor
                .physical_start
                .checked_add(descriptor.page_count.checked_mul(4096).ok_or(Overflow)?)
                .ok_or(Overflow)?;
            if end > physical_end {
                return Err(OutsidePhysicalWidth);
            }
            if descriptor.physical_start < previous_end {
                return Err(UnsortedOrOverlapping);
            }
            previous_end = end;
        }
        Ok(Self { descriptors, physical_end })
    }
    /// Retained physical RAM coverage for a stopped guest. Conventional RAM may
    /// since have become guest page tables/code; this metadata is not ownership
    /// or a safe dereference. Caller must establish effective WB and use its
    /// own bounded mapping. The monitor allocation can never authorize a read.
    pub fn permit_guest_ram(
        &self,
        address: u64,
        bytes: usize,
        monitor: crate::memory::address::PhysicalRange,
    ) -> Result<PermittedRange, MemoryError> {
        let end = address.checked_add(bytes as u64).ok_or(MemoryError::Overflow)?;
        if address <= monitor.last_byte() && monitor.base() < end {
            return Err(MemoryError::MonitorOverlap);
        }
        self.permit_types(address, bytes, 7)
    }
    fn permit_types(
        &self,
        address: u64,
        bytes: usize,
        max_type: u32,
    ) -> Result<PermittedRange, MemoryError> {
        use MemoryError::*;
        if bytes == 0 {
            return Err(EmptyRange);
        }
        let end = address.checked_add(bytes as u64).ok_or(Overflow)?;
        if end > self.physical_end {
            return Err(OutsidePhysicalWidth);
        }
        let mut cursor = address;
        for descriptor in self.descriptors {
            let descriptor_end = descriptor.physical_start + descriptor.page_count * 4096;
            if descriptor_end <= cursor {
                continue;
            }
            if descriptor.physical_start > cursor {
                return Err(UncoveredRange);
            }
            // LoaderCode/Data, BootServicesCode/Data, RuntimeServicesCode/Data.
            // Conventional memory is free, not an owned table/descriptor source.
            if !(1..=max_type).contains(&descriptor.memory_type) {
                return Err(UntrustedMemoryType);
            }
            if descriptor.attributes & 0x2000 != 0 {
                return Err(ReadProtected);
            }
            // UEFI map attributes describe capabilities, not current settings.
            // Alternative cache capabilities may coexist with WB.
            if descriptor.attributes & 8 == 0 {
                return Err(MissingWriteBackCapability);
            }
            cursor = end.min(descriptor_end);
            if cursor == end {
                return Ok(PermittedRange { physical_start: address, bytes });
            }
        }
        Err(UncoveredRange)
    }
    pub fn permit_table_entry(&self, address: u64) -> Result<PermittedRange, MemoryError> {
        if address & 7 != 0 {
            return Err(MemoryError::MisalignedEntry);
        }
        self.permit(address, 8)
    }
    fn permit(&self, address: u64, bytes: usize) -> Result<PermittedRange, MemoryError> {
        self.permit_types(address, bytes, 6)
    }
    pub fn permit_gdt_copy(
        &self,
        address: u64,
        bytes: usize,
    ) -> Result<PermittedRange, MemoryError> {
        if bytes > MAX_GDT_BYTES {
            return Err(MemoryError::CopyTooLarge);
        }
        self.permit(address, bytes)
    }
    /// Suitable for host_paging's reader once a separately safe access callback
    /// exists. Rejected metadata never reaches that callback.
    pub fn read_table_entry(
        &self,
        address: u64,
        read: impl FnOnce(PermittedRange) -> Option<u64>,
    ) -> Result<u64, MemoryError> {
        read(self.permit_table_entry(address)?).ok_or(MemoryError::ReadFailed)
    }
    /// Validate the complete physical extent before handing any destination byte
    /// to the reader. Failure may leave partially filled output; discard it.
    pub fn copy_gdt(
        &self,
        address: u64,
        output: &mut [u8],
        copy: impl FnOnce(PermittedRange, &mut [u8]) -> bool,
    ) -> Result<(), MemoryError> {
        let permit = self.permit_gdt_copy(address, output.len())?;
        if copy(permit, output) { Ok(()) } else { Err(MemoryError::ReadFailed) }
    }
}

/// A metadata permission only. It is not an address that may be dereferenced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermittedRange {
    physical_start: u64,
    bytes: usize,
}

impl PermittedRange {
    pub const fn physical_start(self) -> u64 {
        self.physical_start
    }
    pub const fn bytes(self) -> usize {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryDescriptor {
    pub memory_type: u32,
    pub physical_start: u64,
    pub page_count: u64,
    pub attributes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryError {
    DescriptorCount,
    PhysicalWidth,
    EmptyDescriptor,
    MisalignedDescriptor,
    Overflow,
    OutsidePhysicalWidth,
    UnsortedOrOverlapping,
    EmptyRange,
    MisalignedEntry,
    CopyTooLarge,
    UncoveredRange,
    UntrustedMemoryType,
    ReadProtected,
    /// WB capability is absent; this never reports effective cache policy.
    MissingWriteBackCapability,
    ReadFailed,
    MonitorOverlap,
}
