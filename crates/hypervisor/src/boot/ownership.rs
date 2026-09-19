//! Bounded, pointer-free final firmware map retained in the SVMUEFI2 page.
//!
//! Metadata is not allocation or ExitBootServices evidence: the DXE producer
//! must copy the map returned by its successful EBS transition. This record
//! neither authorizes dereferencing firmware addresses nor proves DMA isolation.
//! UEFI 2.11 sections 7.2.3, 7.4.6 and Table 7.10 define map ownership:
//! Local pinned reference: docs/UEFI_Spec_Final_2.11.pdf.
//!
//! Version 1 retains its 64-byte header. Version 2 extends that header to 160
//! bytes: low-page base/length at 64/72, returned-callback and CPU counts at
//! 80/84, and two 32-byte identities at 96/128. Each identity stores processor
//! ID (u64), APIC ID (u32), signature (u32), vendor (12 bytes), then four zero
//! bytes. All integers are little endian; remaining header/tail bytes are zero.
//! Version 2 entries pack type/base/page-count/attributes at 0/4/12/20 (28
//! bytes), preserving every descriptor while omitting version 1's reserved u32.

use crate::{
    boot::memory::{MemoryDescriptor, MemoryError, ValidatedMemoryMap},
    memory::address::{AddressError, AddressPolicy, EncryptionState, PhysicalRange},
};

pub const HANDOFF_PAGE_BYTES: usize = 4096;
pub const OWNERSHIP_OFFSET: usize = 64;
pub const OWNERSHIP_HEADER_BYTES: usize = 64;
pub(crate) const OWNERSHIP_ENTRY_BYTES: usize = 32;
pub const MAX_OWNERSHIP_DESCRIPTORS: usize = 124;
pub(crate) const OWNERSHIP_VERSION: u16 = 1;
pub(crate) const OWNERSHIP_SMP_VERSION: u16 = 2;
pub const OWNERSHIP_SMP_HEADER_BYTES: usize = 160;
pub(crate) const OWNERSHIP_SMP_ENTRY_BYTES: usize = 28;
pub const MAX_OWNERSHIP_SMP_DESCRIPTORS: usize =
    (HANDOFF_PAGE_BYTES - OWNERSHIP_OFFSET - OWNERSHIP_SMP_HEADER_BYTES)
        / OWNERSHIP_SMP_ENTRY_BYTES;
pub(crate) const OWNERSHIP_MAGIC: [u8; 8] = *b"SVMOWN01";
pub const RESIDENT_ARENA_BYTES: u64 = 1024 * 1024;
const _: () = assert!(
    OWNERSHIP_OFFSET + OWNERSHIP_HEADER_BYTES + MAX_OWNERSHIP_DESCRIPTORS * OWNERSHIP_ENTRY_BYTES
        == HANDOFF_PAGE_BYTES
);
const _: () = assert!(
    OWNERSHIP_OFFSET
        + OWNERSHIP_SMP_HEADER_BYTES
        + MAX_OWNERSHIP_SMP_DESCRIPTORS * OWNERSHIP_SMP_ENTRY_BYTES
        + 8
        == HANDOFF_PAGE_BYTES
);

/// Validated view into owned handoff bytes. The whole arena and optional SIPI
/// page are monitor-reserved, excluding their GPAs from guest allocation/direct
/// identity mappings.
/// Fixture guest code/stack may explicitly share selected arena HPAs; the
/// record is neither NPT enforcement nor a published Windows memory map.
#[derive(Debug)]
pub struct OwnershipRecord<'a> {
    entries: &'a [u8],
    entry_bytes: usize,
    arena: PhysicalRange,
    smp: Option<SmpResources>,
}

impl<'a> OwnershipRecord<'a> {
    /// Decode without allocation or pointers into firmware storage. The caller
    /// supplies its independently established CPU address/encryption policy.
    pub fn decode(page: &'a [u8], policy: &AddressPolicy) -> Result<Self, OwnershipError> {
        if page.len() != HANDOFF_PAGE_BYTES {
            return Err(OwnershipError::Size);
        }
        let record = &page[OWNERSHIP_OFFSET..];
        if record[..8] != OWNERSHIP_MAGIC {
            return Err(OwnershipError::Magic);
        }
        let (header_bytes, entry_bytes) = match read_u16(record, 8) {
            OWNERSHIP_VERSION => (OWNERSHIP_HEADER_BYTES, OWNERSHIP_ENTRY_BYTES),
            OWNERSHIP_SMP_VERSION => (OWNERSHIP_SMP_HEADER_BYTES, OWNERSHIP_SMP_ENTRY_BYTES),
            _ => return Err(OwnershipError::Version),
        };
        if read_u16(record, 10) as usize != header_bytes
            || read_u16(record, 12) as usize != entry_bytes
        {
            return Err(OwnershipError::Size);
        }
        let count = read_u16(record, 14) as usize;
        validate_count(count, header_bytes, entry_bytes)?;
        if read_u32(record, 16) != 1 {
            return Err(OwnershipError::DescriptorVersion);
        }
        let used = header_bytes + count * entry_bytes;
        if record[20..24]
            .iter()
            .chain(record[40..64].iter())
            .chain(record[used..].iter())
            .any(|b| *b != 0)
        {
            return Err(OwnershipError::Reserved);
        }
        if read_u64(record, 32) != RESIDENT_ARENA_BYTES || read_u64(record, 24) == 0 {
            return Err(OwnershipError::ArenaSize);
        }
        let arena = policy
            .validate(read_u64(record, 24), read_u64(record, 32), 4096)
            .map_err(OwnershipError::Address)?;
        let smp = if header_bytes == OWNERSHIP_SMP_HEADER_BYTES {
            if record[88..96]
                .iter()
                .chain(record[124..128].iter())
                .chain(record[156..160].iter())
                .any(|b| *b != 0)
            {
                return Err(OwnershipError::Reserved);
            }
            if read_u32(record, 84) != 2 {
                return Err(OwnershipError::SmpIdentity);
            }
            let low_page = policy
                .validate(read_u64(record, 64), read_u64(record, 72), 4096)
                .map_err(OwnershipError::Address)?;
            Some(SmpResources::new(
                low_page,
                [decode_cpu(&record[96..128]), decode_cpu(&record[128..160])],
                read_u32(record, 80),
            )?)
        } else {
            None
        };
        let result = Self { entries: &record[header_bytes..used], entry_bytes, arena, smp };
        let mut previous_end = 0;
        for slot in result.entries.chunks_exact(entry_bytes) {
            if entry_bytes == OWNERSHIP_ENTRY_BYTES && read_u32(slot, 4) != 0 {
                return Err(OwnershipError::Reserved);
            }
            let descriptor = decode_entry(slot);
            // Reuse the same descriptor validity policy as firmware readers.
            ValidatedMemoryMap::new(core::slice::from_ref(&descriptor), policy.physical_bits())
                .map_err(OwnershipError::Map)?;
            policy
                .validate(descriptor.physical_start, descriptor.page_count * 4096, 4096)
                .map_err(OwnershipError::Address)?;
            if descriptor.physical_start < previous_end {
                return Err(OwnershipError::Map(MemoryError::UnsortedOrOverlapping));
            }
            previous_end = descriptor.physical_start + descriptor.page_count * 4096;
        }
        coverage(result.descriptors(), arena)?;
        if let Some(smp) = smp {
            smp_coverage(result.descriptors(), arena, smp, policy.physical_bits())?;
        }
        Ok(result)
    }

    pub const fn arena(&self) -> PhysicalRange {
        self.arena
    }
    pub const fn smp(&self) -> Option<SmpResources> {
        self.smp
    }
    pub fn descriptor_count(&self) -> usize {
        self.entries.len() / self.entry_bytes
    }
    pub fn descriptors(&self) -> impl Iterator<Item = MemoryDescriptor> + '_ {
        self.entries.chunks_exact(self.entry_bytes).map(decode_entry)
    }
    /// Project an identity-addressed guest allocation map: preserve all bytes,
    /// types and attributes outside monitor storage, and reserve the complete
    /// arena and optional SIPI page (UEFI type 0). At most four splits are added.
    /// This is a planning view, not a firmware map publication or NPT update.
    pub fn guest_descriptors(&self) -> impl Iterator<Item = MemoryDescriptor> + '_ {
        self.descriptors()
            .flat_map(move |descriptor| reserve_descriptor(descriptor, Some(self.arena)))
            .flat_map(move |descriptor| {
                reserve_descriptor(descriptor, self.smp.map(|s| s.low_page))
            })
    }
    /// True means the requested span intersects monitor-owned memory and must
    /// be excluded from generic guest RAM admission; false is not RAM approval.
    pub fn excludes_guest_range(&self, range: PhysicalRange) -> bool {
        overlaps(range, self.arena) || self.smp.is_some_and(|s| overlaps(range, s.low_page))
    }

    /// Encode sorted, complete normalized descriptors. Oversized maps fail;
    /// dropping descriptors to fit is forbidden. Failure leaves `page` intact.
    /// Bytes 0..64 retain the existing SVMUEFI2 prefix.
    pub fn encode_into(
        page: &mut [u8],
        descriptors: &[MemoryDescriptor],
        arena: PhysicalRange,
        physical_bits: u8,
        descriptor_version: u32,
    ) -> Result<(), OwnershipError> {
        Self::encode_with_smp(page, descriptors, arena, physical_bits, descriptor_version, None)
    }

    /// Version 2 additionally retains supplied CPU identities and an owned SIPI
    /// page. Version 1 bytes and capacity are unchanged when `smp` is absent.
    /// Every rejection occurs before modifying the destination page.
    pub fn encode_with_smp(
        page: &mut [u8],
        descriptors: &[MemoryDescriptor],
        arena: PhysicalRange,
        physical_bits: u8,
        descriptor_version: u32,
        smp: Option<SmpResources>,
    ) -> Result<(), OwnershipError> {
        if page.len() != HANDOFF_PAGE_BYTES {
            return Err(OwnershipError::Size);
        }
        let header_bytes =
            if smp.is_some() { OWNERSHIP_SMP_HEADER_BYTES } else { OWNERSHIP_HEADER_BYTES };
        let entry_bytes =
            if smp.is_some() { OWNERSHIP_SMP_ENTRY_BYTES } else { OWNERSHIP_ENTRY_BYTES };
        validate_count(descriptors.len(), header_bytes, entry_bytes)?;
        if descriptor_version != 1 {
            return Err(OwnershipError::DescriptorVersion);
        }
        if arena.len() != RESIDENT_ARENA_BYTES || arena.base() == 0 || arena.base() & 4095 != 0 {
            return Err(OwnershipError::ArenaSize);
        }
        ValidatedMemoryMap::new(descriptors, physical_bits).map_err(OwnershipError::Map)?;
        AddressPolicy::new(physical_bits, EncryptionState::Unencrypted { encryption_bit: None })
            .map_err(OwnershipError::Address)?
            .validate(arena.base(), arena.len(), 4096)
            .map_err(OwnershipError::Address)?;
        coverage(descriptors.iter().copied(), arena)?;
        if let Some(smp) = smp {
            smp_coverage(descriptors.iter().copied(), arena, smp, physical_bits)?;
        }
        let record = &mut page[OWNERSHIP_OFFSET..];
        record.fill(0);
        record[..8].copy_from_slice(&OWNERSHIP_MAGIC);
        put16(record, 8, if smp.is_some() { OWNERSHIP_SMP_VERSION } else { OWNERSHIP_VERSION });
        put16(record, 10, header_bytes as u16);
        put16(record, 12, entry_bytes as u16);
        put16(record, 14, descriptors.len() as u16);
        put32(record, 16, descriptor_version);
        put64(record, 24, arena.base());
        put64(record, 32, arena.len());
        if let Some(smp) = smp {
            put64(record, 64, smp.low_page.base());
            put64(record, 72, smp.low_page.len());
            put32(record, 80, smp.returned_ap_callbacks);
            put32(record, 84, 2);
            for (index, cpu) in smp.cpus.iter().enumerate() {
                let offset = 96 + index * 32;
                put64(record, offset, cpu.processor_id);
                put32(record, offset + 8, cpu.apic_id);
                put32(record, offset + 12, cpu.signature);
                record[offset + 16..offset + 28].copy_from_slice(&cpu.vendor);
            }
        }
        for (slot, descriptor) in
            record[header_bytes..].chunks_exact_mut(entry_bytes).zip(descriptors)
        {
            put32(slot, 0, descriptor.memory_type);
            let base_offset = entry_bytes - 24;
            put64(slot, base_offset, descriptor.physical_start);
            put64(slot, base_offset + 8, descriptor.page_count);
            put64(slot, base_offset + 16, descriptor.attributes);
        }
        Ok(())
    }
}

/// The deliberately bounded two-CPU handoff profile. Returned callbacks are
/// producer evidence, not proof that any firmware procedure was executed.
/// AMD APM vol. 2 rev. 3.44 chapter 16: SIPI addresses a 4-KiB page below 1 MiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmpResources {
    low_page: PhysicalRange,
    cpus: [SmpCpuIdentity; 2],
    returned_ap_callbacks: u32,
}

impl SmpResources {
    pub fn new(
        low_page: PhysicalRange,
        cpus: [SmpCpuIdentity; 2],
        returned_ap_callbacks: u32,
    ) -> Result<Self, OwnershipError> {
        if low_page.base() == 0
            || low_page.base() & 4095 != 0
            || low_page.len() != 4096
            || low_page.last_byte() >= 0x100000
        {
            return Err(OwnershipError::SmpLowPage);
        }
        for (index, cpu) in cpus.iter().enumerate() {
            if cpu.processor_id != index as u64
                || cpu.apic_id != index as u32
                || cpu.vendor != *b"AuthenticAMD"
                || cpu.signature == 0
                || cpu.signature != cpus[0].signature
            {
                return Err(OwnershipError::SmpIdentity);
            }
        }
        if returned_ap_callbacks != 1 {
            return Err(OwnershipError::SmpCallbackCount);
        }
        Ok(Self { low_page, cpus, returned_ap_callbacks })
    }
    pub const fn low_page(self) -> PhysicalRange {
        self.low_page
    }
    pub const fn cpus(&self) -> &[SmpCpuIdentity; 2] {
        &self.cpus
    }
    pub const fn returned_ap_callbacks(self) -> u32 {
        self.returned_ap_callbacks
    }
}

/// Inert, supplied CPU evidence. This does not invoke firmware or CPUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmpCpuIdentity {
    pub processor_id: u64,
    pub apic_id: u32,
    pub signature: u32,
    pub vendor: [u8; 12],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnershipError {
    Size,
    Magic,
    Version,
    Reserved,
    DescriptorCount,
    DescriptorVersion,
    ArenaSize,
    ArenaUncovered,
    ArenaNotLoaderCode,
    SmpIdentity,
    SmpCallbackCount,
    SmpLowPage,
    SmpArenaOverlap,
    SmpPageUncovered,
    SmpPageNotLoaderCode,
    Address(AddressError),
    Map(MemoryError),
}

fn validate_count(
    count: usize,
    header_bytes: usize,
    entry_bytes: usize,
) -> Result<(), OwnershipError> {
    let maximum = (HANDOFF_PAGE_BYTES - OWNERSHIP_OFFSET - header_bytes) / entry_bytes;
    if count == 0 || count > maximum { Err(OwnershipError::DescriptorCount) } else { Ok(()) }
}

fn reserve_descriptor(
    descriptor: MemoryDescriptor,
    reserved: Option<PhysicalRange>,
) -> impl Iterator<Item = MemoryDescriptor> {
    let mut segments = [None; 3];
    if let Some(reserved) = reserved {
        let start = descriptor.physical_start;
        let end = start + descriptor.page_count * 4096;
        let overlap_start = start.max(reserved.base());
        let overlap_end = end.min(reserved.last_byte() + 1);
        if overlap_start < overlap_end {
            if start < overlap_start {
                segments[0] = Some(MemoryDescriptor {
                    page_count: (overlap_start - start) / 4096,
                    ..descriptor
                });
            }
            segments[1] = Some(MemoryDescriptor {
                memory_type: 0,
                physical_start: overlap_start,
                page_count: (overlap_end - overlap_start) / 4096,
                ..descriptor
            });
            if overlap_end < end {
                segments[2] = Some(MemoryDescriptor {
                    physical_start: overlap_end,
                    page_count: (end - overlap_end) / 4096,
                    ..descriptor
                });
            }
        } else {
            segments[0] = Some(descriptor);
        }
    } else {
        segments[0] = Some(descriptor);
    }
    segments.into_iter().flatten()
}

fn smp_coverage(
    descriptors: impl Iterator<Item = MemoryDescriptor>,
    arena: PhysicalRange,
    smp: SmpResources,
    physical_bits: u8,
) -> Result<(), OwnershipError> {
    if overlaps(arena, smp.low_page) {
        return Err(OwnershipError::SmpArenaOverlap);
    }
    for descriptor in descriptors {
        if descriptor.physical_start <= smp.low_page.base()
            && descriptor.physical_start + descriptor.page_count * 4096 > smp.low_page.last_byte()
        {
            if descriptor.memory_type != 1 {
                return Err(OwnershipError::SmpPageNotLoaderCode);
            }
            // Reuse the firmware reader's WB-capability/read-protection policy.
            // This neither proves effective caching nor permits dereferencing.
            ValidatedMemoryMap::new(core::slice::from_ref(&descriptor), physical_bits)
                .map_err(OwnershipError::Map)?
                .permit_gdt_copy(smp.low_page.base(), 4096)
                .map_err(OwnershipError::Map)?;
            return Ok(());
        }
    }
    Err(OwnershipError::SmpPageUncovered)
}

fn overlaps(left: PhysicalRange, right: PhysicalRange) -> bool {
    left.base() <= right.last_byte() && right.base() <= left.last_byte()
}

fn decode_cpu(bytes: &[u8]) -> SmpCpuIdentity {
    let mut vendor = [0; 12];
    vendor.copy_from_slice(&bytes[16..28]);
    SmpCpuIdentity {
        processor_id: read_u64(bytes, 0),
        apic_id: read_u32(bytes, 8),
        signature: read_u32(bytes, 12),
        vendor,
    }
}

fn coverage(
    descriptors: impl Iterator<Item = MemoryDescriptor>,
    arena: PhysicalRange,
) -> Result<(), OwnershipError> {
    let mut next = arena.base();
    let end = arena.last_byte() + 1;
    for descriptor in descriptors {
        let descriptor_end = descriptor.physical_start + descriptor.page_count * 4096;
        if descriptor_end <= next {
            continue;
        }
        if descriptor.physical_start > next {
            return Err(OwnershipError::ArenaUncovered);
        }
        if descriptor.memory_type != 1 {
            return Err(OwnershipError::ArenaNotLoaderCode);
        }
        next = descriptor_end.min(end);
        if next == end {
            return Ok(());
        }
    }
    Err(OwnershipError::ArenaUncovered)
}

fn decode_entry(bytes: &[u8]) -> MemoryDescriptor {
    let base_offset = bytes.len() - 24;
    MemoryDescriptor {
        memory_type: read_u32(bytes, 0),
        physical_start: read_u64(bytes, base_offset),
        page_count: read_u64(bytes, base_offset + 8),
        attributes: read_u64(bytes, base_offset + 16),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
