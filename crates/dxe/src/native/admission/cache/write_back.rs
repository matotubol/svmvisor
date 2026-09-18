//! Write-back classification of supplied cache observations for one allocated extent.

use super::{
    ABI_VERSION, CacheSnapshot, HIGH_BOUND, LOW_BOUND, MAX_PAGES, MAX_VARIABLE_MTRRS, PAGE_MASK,
    PHYSICAL_MASK, TARGET_SIGNATURE, captured,
};

/// A classifier result for supplied observations; not an ownership or native
/// accessibility token. Address-encryption controls were disabled in this CPU
/// snapshot; no assertion about transparent DIMM/controller encryption is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteBackReport {
    pub pages: usize,
    pub leaf_checks: usize,
    pub mtrr_segments: usize,
    pub physical_bits: u8,
    /// Retain the advertised C-bit even though the architectural mode is off.
    pub encryption_bit: u8,
    pub encryption_physical_reduction: u8,
}

/// Actual leaf information supplied by the separately validated host walk.
/// `physical_page` is the next allocated4K page in physical address order.
/// The full containing leaf is checked, including bytes outside the allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageMapping {
    pub physical_page: u64,
    pub leaf_physical_base: u64,
    pub leaf_bytes: u64,
    pub pat_index: u8,
}

#[derive(Clone, Copy, Debug)]
struct Range {
    base: u64,
    end: u64,
    memory_type: u8,
}

impl Range {
    fn overlaps(self, base: u64, end: u64) -> bool {
        self.base < end && base < self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheError {
    IncompleteCapture,
    UnsupportedCpu,
    UnsupportedFeatures,
    UnsupportedPaging,
    CacheDisabled,
    AddressEncryptionActive,
    ReservedRegisterBits,
    InvalidPat,
    InvalidMtrr,
    InvalidIorr,
    InvalidSmmRange,
    InvalidMmioWindow,
    MtrrsDisabled,
    DramRoutingDisabled,
    InvalidTopOfMemory,
    InvalidAllocation,
    TooManyPages,
    MappingCountMismatch,
    NoncontiguousMapping,
    InvalidLeaf,
    LeafOutsideLowRam,
    IorrOverlap,
    SmmOverlap,
    ApicOverlap,
    MmioConfigurationOverlap,
    PatNotWriteBack,
    MtrrNotWriteBack,
    UndefinedMtrrOverlap,
    IncompleteRangeCheck,
}

/// Extract the index from a supplied actual present leaf. This does not validate
/// its other bits, translation, permission, physical backing or coherence.
pub fn leaf_pat_index(entry: u64, leaf_bytes: u64) -> Result<u8, CacheError> {
    let pat_shift = match leaf_bytes {
        4096 => 7,
        0x20_0000 | 0x4000_0000 if entry & (1 << 7) != 0 => 12,
        _ => return Err(CacheError::InvalidLeaf),
    };
    if entry & 1 == 0 {
        return Err(CacheError::InvalidLeaf);
    }
    Ok((((entry >> pat_shift) & 1) << 2 | ((entry >> 3) & 3)) as u8)
}

/// Validate a contiguous allocated RAM extent and one actual mapping per page.
/// The caller still establishes allocation ownership/current permissions, live
/// pointer provenance, table/TLB agreement, DMA isolation, and cross-CPU memory
/// type consistency. Repeat in the retained interval if any source can change.
pub fn classify_write_back(
    snapshot: &CacheSnapshot,
    allocation_base: u64,
    allocation_bytes: u64,
    mappings: &[PageMapping],
) -> Result<WriteBackReport, CacheError> {
    use CacheError::*;
    validate_snapshot(snapshot)?;
    if allocation_bytes == 0 || allocation_base & 4095 != 0 || allocation_bytes & 4095 != 0 {
        return Err(InvalidAllocation);
    }
    let allocation_end = allocation_base.checked_add(allocation_bytes).ok_or(InvalidAllocation)?;
    let pages = usize::try_from(allocation_bytes / 4096).map_err(|_| TooManyPages)?;
    if pages > MAX_PAGES {
        return Err(TooManyPages);
    }
    if mappings.len() != pages {
        return Err(MappingCountMismatch);
    }
    let ceiling = snapshot.top_mem.min(HIGH_BOUND);
    if allocation_base < LOW_BOUND || allocation_end > ceiling {
        return Err(InvalidAllocation);
    }

    let mut ranges = [None; MAX_VARIABLE_MTRRS];
    let count = (snapshot.mtrr_cap & 255) as usize;
    for (out, pair) in ranges.iter_mut().zip(snapshot.variable.iter()).take(count) {
        if pair.mask & !(PAGE_MASK | 0x800) != 0 || pair.base & !(PAGE_MASK | 7) != 0 {
            return Err(ReservedRegisterBits);
        }
        if pair.mask & 0x800 != 0 {
            let memory_type = (pair.base & 7) as u8;
            if !mtrr_type(memory_type) {
                return Err(InvalidMtrr);
            }
            *out = Some(
                decode_range(pair.base & PAGE_MASK, pair.mask & PAGE_MASK, memory_type)
                    .ok_or(InvalidMtrr)?,
            );
        }
    }
    let mut iorr = [None; 2];
    for (out, pair) in iorr.iter_mut().zip(snapshot.iorr.iter()) {
        if pair.mask & !(PAGE_MASK | 0x800) != 0 || pair.base & !(PAGE_MASK | 0x18) != 0 {
            return Err(ReservedRegisterBits);
        }
        if pair.mask & 0x800 != 0 {
            *out = Some(
                decode_range(pair.base & PAGE_MASK, pair.mask & PAGE_MASK, 0).ok_or(InvalidIorr)?,
            );
        }
    }
    let tseg_address_mask = PHYSICAL_MASK & !0x1ffff;
    if snapshot.smm_address & !tseg_address_mask != 0
        || snapshot.smm_mask & !(tseg_address_mask | 0x773f) != 0
    {
        return Err(ReservedRegisterBits);
    }
    let tseg = if snapshot.smm_mask & 2 != 0 {
        Some(
            decode_range(snapshot.smm_address, snapshot.smm_mask & tseg_address_mask, 0)
                .ok_or(InvalidSmmRange)?,
        )
    } else {
        None
    };
    if (snapshot.smm_mask & 2 != 0 && !mtrr_type(((snapshot.smm_mask >> 12) & 7) as u8))
        || (snapshot.smm_mask & 1 != 0 && !mtrr_type(((snapshot.smm_mask >> 8) & 7) as u8))
        || (snapshot.smm_mask & 1 != 0
            && tseg.is_some_and(|range| range.overlaps(0xa0000, 0xc0000)))
    {
        return Err(InvalidSmmRange);
    }
    // ASeg is [A0000h,C0000h), excluded by the full-leaf lower bound.
    let apic = if snapshot.apic_base & (1 << 11) != 0 {
        let base = snapshot.apic_base & PAGE_MASK;
        Some(Range { base, end: base.checked_add(4096).ok_or(InvalidMmioWindow)?, memory_type: 0 })
    } else {
        None
    };
    let mmio = if snapshot.mmio_config & 1 != 0 {
        // PPR p210: 1MiB * 2^BusRange, including segment encodings9..15.
        let bytes = 1u64 << (20 + ((snapshot.mmio_config >> 2) & 15));
        let base = snapshot.mmio_config & (PHYSICAL_MASK & !0xfffff);
        if base & (bytes - 1) != 0 {
            return Err(InvalidMmioWindow);
        }
        let end = base.checked_add(bytes).ok_or(InvalidMmioWindow)?;
        if end > 1 << 48 {
            return Err(InvalidMmioWindow);
        }
        Some(Range { base, end, memory_type: 0 })
    } else {
        None
    };

    let mut expected_page = allocation_base;
    let mut segments = 0usize;
    for mapping in mappings {
        if mapping.physical_page != expected_page {
            return Err(NoncontiguousMapping);
        }
        expected_page = expected_page.checked_add(4096).ok_or(InvalidAllocation)?;
        if !matches!(mapping.leaf_bytes, 4096 | 0x20_0000 | 0x4000_0000)
            || mapping.leaf_physical_base & (mapping.leaf_bytes - 1) != 0
            || mapping.pat_index > 7
        {
            return Err(InvalidLeaf);
        }
        let leaf_end =
            mapping.leaf_physical_base.checked_add(mapping.leaf_bytes).ok_or(InvalidLeaf)?;
        if mapping.physical_page < mapping.leaf_physical_base || expected_page > leaf_end {
            return Err(InvalidLeaf);
        }
        if mapping.leaf_physical_base < LOW_BOUND || leaf_end > ceiling {
            return Err(LeafOutsideLowRam);
        }
        for range in iorr.iter().flatten() {
            if range.overlaps(mapping.leaf_physical_base, leaf_end) {
                return Err(IorrOverlap);
            }
        }
        if tseg.is_some_and(|range| range.overlaps(mapping.leaf_physical_base, leaf_end)) {
            return Err(SmmOverlap);
        }
        if apic.is_some_and(|range| range.overlaps(mapping.leaf_physical_base, leaf_end)) {
            return Err(ApicOverlap);
        }
        if mmio.is_some_and(|range| range.overlaps(mapping.leaf_physical_base, leaf_end)) {
            return Err(MmioConfigurationOverlap);
        }
        if (snapshot.pat >> (u32::from(mapping.pat_index) * 8)) & 255 != 6 {
            return Err(PatNotWriteBack);
        }
        segments = segments
            .checked_add(check_uniform_wb(
                mapping.leaf_physical_base,
                leaf_end,
                &ranges,
                (snapshot.mtrr_default & 255) as u8,
            )?)
            .ok_or(IncompleteRangeCheck)?;
    }
    Ok(WriteBackReport {
        pages,
        leaf_checks: mappings.len(),
        mtrr_segments: segments,
        physical_bits: 48,
        encryption_bit: (snapshot.encryption_ebx & 63) as u8,
        encryption_physical_reduction: ((snapshot.encryption_ebx >> 6) & 63) as u8,
    })
}

fn validate_snapshot(s: &CacheSnapshot) -> Result<(), CacheError> {
    use CacheError::*;
    let sev_supported = s.encryption_eax & 2 != 0;
    let fields = captured::REQUIRED | if sev_supported { captured::SEV_STATUS } else { 0 };
    if s.abi_version != ABI_VERSION
        || s.refusal != 0
        || s.captured_fields != fields
        || s.reserved != 0
    {
        return Err(IncompleteCapture);
    }
    if s.signature != TARGET_SIGNATURE || s.physical_bits != 48 {
        return Err(UnsupportedCpu);
    }
    if s.max_basic < 1
        || s.max_extended < 0x8000_0023
        || s.leaf1_ecx & (1 << 31) != 0
        || s.leaf1_edx & 0x0001_1020 != 0x0001_1020
        || s.encryption_eax & 1 == 0
        || s.encryption_eax & !0x41ff_ffff != 0
        || s.encryption_ebx & !0xffff != 0
        || s.encryption_ebx & 63 != 51
        || (s.encryption_ebx >> 6) & 63 > 6
        || s.multi_key_eax & !1 != 0
        || s.multi_key_ebx & !0xffff != 0
    {
        return Err(UnsupportedFeatures);
    }
    if s.sys_cfg & !0x07fc_0000 != 0
        || s.efer & !0x0034_fd01 != 0
        || s.mtrr_cap & !0x5ff != 0
        || s.mtrr_default & !0xcff != 0
        || s.apic_base & !(PAGE_MASK | 0xd00) != 0
        || s.mmio_config & !((PHYSICAL_MASK & !0xfffff) | 0x3d) != 0
    {
        return Err(ReservedRegisterBits);
    }
    if s.sys_cfg & 0x0780_0000 != 0 || s.sev_status != 0 {
        return Err(AddressEncryptionActive);
    }
    if s.rflags & 0x44700 != 0
        || s.cr0 & 0x8000_0001 != 0x8000_0001
        || s.cr4 & (1 << 5) == 0
        || s.cr4 & (1 << 12) != 0
        || s.efer & 0x500 != 0x500
        || s.efer & (1 << 20) != 0
    {
        return Err(UnsupportedPaging);
    }
    if s.cr0 & 0x6000_0000 != 0 {
        return Err(CacheDisabled);
    }
    let count = (s.mtrr_cap & 255) as usize;
    if count > MAX_VARIABLE_MTRRS || s.mtrr_cap & 0x500 != 0x500 {
        return Err(InvalidMtrr);
    }
    if s.msr_reads != 14 + u64::from(sev_supported) + 2 * count as u64 {
        return Err(IncompleteCapture);
    }
    if s.mtrr_default & 0x800 == 0 {
        return Err(MtrrsDisabled);
    }
    if !mtrr_type((s.mtrr_default & 255) as u8) {
        return Err(InvalidMtrr);
    }
    for (index, byte) in s.pat.to_le_bytes().into_iter().enumerate() {
        // PPR pp171-172 permits UC-minus7 only in PA2/PA6 for this target.
        if !mtrr_type(byte) && !(byte == 7 && matches!(index, 2 | 6)) {
            return Err(InvalidPat);
        }
    }
    if s.sys_cfg & (1 << 20) == 0 || s.sys_cfg & (1 << 19) != 0 {
        return Err(DramRoutingDisabled);
    }
    if s.top_mem <= LOW_BOUND
        || s.top_mem > HIGH_BOUND
        || s.top_mem & !((PHYSICAL_MASK) & !0x7f_ffff) != 0
    {
        return Err(InvalidTopOfMemory);
    }
    if s.apic_base & (1 << 10) != 0 && s.apic_base & (1 << 11) == 0 {
        return Err(InvalidMmioWindow);
    }
    Ok(())
}

fn mtrr_type(value: u8) -> bool {
    matches!(value, 0 | 1 | 4 | 5 | 6)
}

fn decode_range(base: u64, mask: u64, memory_type: u8) -> Option<Range> {
    let bytes = ((!mask) & PHYSICAL_MASK).checked_add(1)?;
    if !bytes.is_power_of_two() || base & (bytes - 1) != 0 {
        return None;
    }
    let end = base.checked_add(bytes)?;
    if end > 1 << 48 {
        return None;
    }
    Some(Range { base, end, memory_type })
}

fn check_uniform_wb(
    base: u64,
    end: u64,
    ranges: &[Option<Range>; MAX_VARIABLE_MTRRS],
    default: u8,
) -> Result<usize, CacheError> {
    let mut cursor = base;
    // Eight intervals create at most16 interior boundaries, hence17 segments.
    for segments in 1..=2 * MAX_VARIABLE_MTRRS + 1 {
        let mut next = end;
        let mut types = 0u8;
        for range in ranges.iter().flatten() {
            if range.base <= cursor && cursor < range.end {
                types |= 1u8 << range.memory_type;
            }
            for boundary in [range.base, range.end] {
                if boundary > cursor && boundary < next {
                    next = boundary;
                }
            }
        }
        let effective = if types == 0 {
            default
        } else if types & 1 != 0 {
            // UC wins over any other overlapping type.
            0
        } else if types.count_ones() == 1 {
            types.trailing_zeros() as u8
        } else if types == (1 << 4) | (1 << 6) {
            // WT+WB -> WT.
            4
        } else {
            return Err(CacheError::UndefinedMtrrOverlap);
        };
        if effective != 6 {
            return Err(CacheError::MtrrNotWriteBack);
        }
        cursor = next;
        if cursor == end {
            return Ok(segments);
        }
    }
    Err(CacheError::IncompleteRangeCheck)
}
