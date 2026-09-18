//! Bounded CPU cache/address-encryption observations for PPR 57896 rev.3.00.
//! No allocation, hardware writes, firmware calls, mapping dereferences or
//! native admission token. See docs/native-cache-contract.md for the limits.

pub use svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE;

pub const ABI_VERSION: u64 = 1;
pub const SNAPSHOT_BYTES: usize = 352;
pub const MAX_VARIABLE_MTRRS: usize = 8;
pub const MAX_PAGES: usize = 256;
const PHYSICAL_MASK: u64 = (1 << 48) - 1;
const PAGE_MASK: u64 = PHYSICAL_MASK & !0xfff;
const LOW_BOUND: u64 = 1 << 20;
const HIGH_BOUND: u64 = 1 << 32;

pub mod captured {
    pub const CPUID: u64 = 1;
    pub const CONTROLS: u64 = 2;
    pub const ADDRESS_ENCRYPTION: u64 = 4;
    pub const ROUTING: u64 = 8;
    pub const MTRRS: u64 = 16;
    pub const SEV_STATUS: u64 = 32;
    pub const REQUIRED: u64 = 31;
}

#[cfg(target_os = "uefi")]
unsafe extern "efiapi" {
    fn svmvisor_native_cache_read(out: *mut CacheSnapshot) -> u32;
}

/// Raw integer observations, deliberately constructible for supplied-data tests.
/// A value of this type alone does not authenticate a hardware observation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheSnapshot {
    pub abi_version: u64,
    pub captured_fields: u64,
    pub refusal: u64,
    pub msr_reads: u64,
    pub signature: u32,
    pub max_basic: u32,
    pub max_extended: u32,
    pub leaf1_ecx: u32,
    pub leaf1_edx: u32,
    pub physical_bits: u32,
    pub encryption_eax: u32,
    pub encryption_ebx: u32,
    pub multi_key_eax: u32,
    pub multi_key_ebx: u32,
    pub initial_apic_id: u32,
    pub reserved: u32,
    pub rflags: u64,
    pub cr0: u64,
    pub cr4: u64,
    pub efer: u64,
    pub sys_cfg: u64,
    /// Defined only when `captured::SEV_STATUS` is present.
    pub sev_status: u64,
    pub pat: u64,
    pub mtrr_cap: u64,
    pub mtrr_default: u64,
    pub top_mem: u64,
    pub smm_address: u64,
    pub smm_mask: u64,
    pub apic_base: u64,
    pub mmio_config: u64,
    pub iorr: [RegisterPair; 2],
    pub variable: [RegisterPair; MAX_VARIABLE_MTRRS],
}

const _: () = assert!(core::mem::size_of::<CacheSnapshot>() == SNAPSHOT_BYTES);
const _: () = assert!(core::mem::align_of::<CacheSnapshot>() == 8);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, signature) == 32);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, rflags) == 80);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, cr0) == 88);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, efer) == 104);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, sys_cfg) == 112);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, pat) == 128);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, mtrr_cap) == 136);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, top_mem) == 152);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, iorr) == 192);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, variable) == 224);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegisterPair {
    pub base: u64,
    pub mask: u64,
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
pub enum CaptureError {
    OutputAddress,
    PrivilegeOrFlags,
    UnsupportedCpu,
    UnsupportedFeatures,
    AddressEncryptionActive,
    UnsupportedMtrrCount,
    UnexpectedStatus,
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

/// Capture named MSRs after the assembly's exact CPU/capability guards.
///
/// # Safety
/// The guard must cover the current BSP with IF/DF/TF/NT/AC clear, outside SMM,
/// in ordinary firmware execution. `out` and the stack must be accessible for
/// the entire call. No fault containment is provided for hostile interception
/// or inaccessible memory. This code is currently unlinked groundwork: this
/// batch permits compile/tests only and no physical execution. Future AP use
/// needs its own callback/flags contract and explicit integration review.
#[cfg(target_os = "uefi")]
pub unsafe fn capture_into(
    _guard: &super::cpu::QuiescentBsp<'_>,
    out: &mut CacheSnapshot,
) -> Result<(), CaptureError> {
    match unsafe { svmvisor_native_cache_read(out) } {
        0 => Ok(()),
        1 => Err(CaptureError::OutputAddress),
        2 => Err(CaptureError::PrivilegeOrFlags),
        3 => Err(CaptureError::UnsupportedCpu),
        4 => Err(CaptureError::UnsupportedFeatures),
        5 => Err(CaptureError::AddressEncryptionActive),
        6 => Err(CaptureError::UnsupportedMtrrCount),
        _ => Err(CaptureError::UnexpectedStatus),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> CacheSnapshot {
        CacheSnapshot {
            abi_version: 1,
            captured_fields: captured::REQUIRED,
            msr_reads: 30,
            signature: TARGET_SIGNATURE,
            max_basic: 0x10,
            max_extended: 0x8000_0026,
            leaf1_edx: 0x0001_1020,
            physical_bits: 48,
            encryption_eax: 1,
            encryption_ebx: 51 | (5 << 6),
            cr0: 0x8001_0033,
            cr4: 0x620,
            efer: 0xd00,
            sys_cfg: 1 << 20,
            pat: 0x0007_0406_0007_0406,
            mtrr_cap: 0x508,
            mtrr_default: 0x806,
            top_mem: 0x0800_0000,
            apic_base: 0xfee0_0900,
            ..CacheSnapshot::default()
        }
    }
    fn mapping() -> PageMapping {
        PageMapping {
            physical_page: 0x201000,
            leaf_physical_base: 0x200000,
            leaf_bytes: 0x200000,
            pat_index: 0,
        }
    }
    fn pair(base: u64, bytes: u64, kind: u64) -> RegisterPair {
        RegisterPair { base: base | kind, mask: (PHYSICAL_MASK & !(bytes - 1)) | 0x800 }
    }
    fn check(s: &CacheSnapshot) -> Result<WriteBackReport, CacheError> {
        classify_write_back(s, 0x201000, 4096, &[mapping()])
    }

    #[test]
    fn actual_default_and_variable_wb_keep_advertised_encryption_bit() {
        let mut s = snapshot();
        let report = check(&s).unwrap();
        assert_eq!(
            report,
            WriteBackReport {
                pages: 1,
                leaf_checks: 1,
                mtrr_segments: 1,
                physical_bits: 48,
                encryption_bit: 51,
                encryption_physical_reduction: 5
            }
        );
        s.mtrr_default = 0x800; // UC outside the actual variable WB range.
        s.variable[0] = pair(0x200000, 0x200000, 6);
        assert_eq!(check(&s).unwrap().mtrr_segments, 1);
        s.variable[1] = pair(0x200000, 0x100000, 6);
        assert_eq!(check(&s).unwrap().mtrr_segments, 2); // Same-type overlap is defined.
    }

    #[test]
    fn leaf_pat_selection_uses_actual_leaf_bit_position_and_current_pat_byte() {
        for index in 0..8u8 {
            let low = u64::from(index & 3) << 3;
            assert_eq!(leaf_pat_index(1 | low | (u64::from(index >> 2) << 7), 4096), Ok(index));
            for bytes in [0x20_0000, 0x4000_0000] {
                assert_eq!(
                    leaf_pat_index(0x81 | low | (u64::from(index >> 2) << 12), bytes),
                    Ok(index)
                );
            }
        }
        assert_eq!(leaf_pat_index(0, 4096), Err(CacheError::InvalidLeaf));
        assert_eq!(leaf_pat_index(1, 0x200000), Err(CacheError::InvalidLeaf));
        assert_eq!(leaf_pat_index(1, 8192), Err(CacheError::InvalidLeaf));
        let mut s = snapshot();
        s.pat = (s.pat & !255) | 4;
        assert_eq!(check(&s), Err(CacheError::PatNotWriteBack));
        let mut m = mapping();
        m.pat_index = 4;
        assert!(classify_write_back(&s, 0x201000, 4096, &[m]).is_ok());
        s.pat |= 7; // PA0=7 is reserved in this exact target's PPR.
        assert_eq!(check(&s), Err(CacheError::InvalidPat));
    }

    #[test]
    fn large_leaf_rejects_non_wb_outside_the_allocated_page_and_detects_holes() {
        let mut s = snapshot();
        s.variable[0] = pair(0x300000, 0x100000, 0);
        assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
        let mut m = mapping();
        m.leaf_physical_base = m.physical_page;
        m.leaf_bytes = 4096;
        assert!(classify_write_back(&s, m.physical_page, 4096, &[m]).is_ok());
        s.mtrr_default = 0x800;
        s.variable[0] = pair(0x200000, 0x100000, 6);
        s.variable[1] = pair(0x300000, 0x100000, 6);
        assert_eq!(check(&s).unwrap().mtrr_segments, 2);
        s.variable[1] = pair(0x300000, 0x80000, 6);
        assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
    }

    #[test]
    fn all_seventeen_possible_mtrr_segments_are_checked() {
        let mut s = snapshot();
        for (index, out) in s.variable.iter_mut().enumerate() {
            *out = pair(0x201000 + index as u64 * 8192, 4096, 6);
        }
        assert_eq!(check(&s).unwrap().mtrr_segments, 17);
        s.variable[7].base = (s.variable[7].base & !7) | 4;
        assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
    }

    #[test]
    fn overlap_precedence_is_order_independent_and_undefined_combinations_refuse() {
        let mut s = snapshot();
        s.variable[0] = pair(0x200000, 0x200000, 6);
        s.variable[1] = pair(0x200000, 0x200000, 4);
        assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack)); // WT+WB -> WT.
        s.variable[1] = pair(0x200000, 0x200000, 1);
        assert_eq!(check(&s), Err(CacheError::UndefinedMtrrOverlap));
        s.variable[7] = pair(0x200000, 0x200000, 0);
        assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack)); // UC dominates all.
        s.variable.reverse();
        assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
    }

    #[test]
    fn malformed_active_ranges_and_reserved_bits_refuse() {
        let mut s = snapshot();
        s.variable[0] = pair(0x200000, 0x200000, 6);
        s.variable[0].mask ^= 1 << 22; // Noncontiguous mask.
        assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
        s.variable[0] = pair(0x201000, 0x200000, 6); // Base not size-aligned.
        assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
        s.variable[0] = pair(0x200000, 0x200000, 3);
        assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
        s.variable[0].mask &= !0x800; // Inactive type has no effect.
        assert!(check(&s).is_ok());
        s.variable[0].base |= 1 << 48;
        assert_eq!(check(&s), Err(CacheError::ReservedRegisterBits));
        let mut s = snapshot();
        s.iorr[0] = pair(0x201000, 0x200000, 0x18);
        assert_eq!(check(&s), Err(CacheError::InvalidIorr));
    }

    #[test]
    fn priority_routing_ranges_refuse_even_outside_the_allocated_page() {
        let mut s = snapshot();
        s.iorr[0] = pair(0x300000, 0x100000, 0x18);
        assert_eq!(check(&s), Err(CacheError::IorrOverlap));
        s.iorr[0] = pair(0x500000, 0x100000, 0);
        assert!(check(&s).is_ok());
        s.smm_address = 0x300000;
        s.smm_mask = (PHYSICAL_MASK & !0xfffff) | 2;
        assert_eq!(check(&s), Err(CacheError::SmmOverlap));
        s.smm_mask |= 3 << 12;
        assert_eq!(check(&s), Err(CacheError::InvalidSmmRange));
        s.smm_mask = 0;
        s.apic_base = 0x300000 | 0x900;
        assert_eq!(check(&s), Err(CacheError::ApicOverlap));
        s.apic_base &= !(1 << 11);
        assert!(check(&s).is_ok());
        s.mmio_config = 0x300000 | 1; // One1MiB bus window.
        assert_eq!(check(&s), Err(CacheError::MmioConfigurationOverlap));
        s.mmio_config = 0x500000 | (1 << 2) | 1; // Misaligned2MiB window.
        assert_eq!(check(&s), Err(CacheError::InvalidMmioWindow));
    }

    #[test]
    fn encryption_and_missing_optional_observations_never_become_disabled_by_default() {
        for bit in 23..=26 {
            let mut s = snapshot();
            s.sys_cfg |= 1 << bit;
            assert_eq!(check(&s), Err(CacheError::AddressEncryptionActive));
        }
        let mut s = snapshot();
        s.encryption_eax |= 2;
        assert_eq!(check(&s), Err(CacheError::IncompleteCapture));
        s.captured_fields |= captured::SEV_STATUS;
        s.msr_reads += 1;
        assert!(check(&s).is_ok());
        s.sev_status = 1;
        assert_eq!(check(&s), Err(CacheError::AddressEncryptionActive));
        s.sev_status = 1 << 63;
        assert_eq!(check(&s), Err(CacheError::AddressEncryptionActive));
        let mut s = snapshot();
        s.encryption_eax = 0;
        assert_eq!(check(&s), Err(CacheError::UnsupportedFeatures));
        s.encryption_eax = 1;
        s.encryption_ebx = 47;
        assert_eq!(check(&s), Err(CacheError::UnsupportedFeatures));
    }

    #[test]
    fn capture_shape_cache_mode_and_reserved_state_fail_closed() {
        let initial = snapshot();
        for mutate in [
            |s: &mut CacheSnapshot| s.abi_version = 0,
            |s: &mut CacheSnapshot| s.refusal = 5,
            |s: &mut CacheSnapshot| s.captured_fields &= !captured::MTRRS,
            |s: &mut CacheSnapshot| s.msr_reads -= 1,
        ] {
            let mut s = initial;
            mutate(&mut s);
            assert_eq!(check(&s), Err(CacheError::IncompleteCapture));
        }
        let mut s = initial;
        s.signature += 1;
        assert_eq!(check(&s), Err(CacheError::UnsupportedCpu));
        s = initial;
        s.leaf1_ecx |= 1 << 31;
        assert_eq!(check(&s), Err(CacheError::UnsupportedFeatures));
        s = initial;
        s.cr0 |= 1 << 30;
        assert_eq!(check(&s), Err(CacheError::CacheDisabled));
        s = initial;
        s.cr0 |= 1 << 29;
        assert_eq!(check(&s), Err(CacheError::CacheDisabled));
        s = initial;
        s.mtrr_default &= !0x800;
        assert_eq!(check(&s), Err(CacheError::MtrrsDisabled));
        s = initial;
        s.sys_cfg &= !(1 << 20);
        assert_eq!(check(&s), Err(CacheError::DramRoutingDisabled));
        s = initial;
        s.mtrr_cap = 0x509;
        assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
        s = initial;
        s.mtrr_cap |= 1 << 11;
        assert_eq!(check(&s), Err(CacheError::ReservedRegisterBits));
    }

    #[test]
    fn whole_leaf_low_ram_bounds_and_per_page_coverage_are_required() {
        let s = snapshot();
        let mut m = mapping();
        m.leaf_physical_base = 0;
        m.physical_page = 0x100000;
        assert_eq!(
            classify_write_back(&s, m.physical_page, 4096, &[m]),
            Err(CacheError::LeafOutsideLowRam)
        );
        m = mapping();
        m.leaf_physical_base = 0x4000000;
        m.physical_page = 0x4001000;
        m.leaf_bytes = 0x4000000; // Unsupported64MiB leaf.
        assert_eq!(
            classify_write_back(&s, m.physical_page, 4096, &[m]),
            Err(CacheError::InvalidLeaf)
        );
        m = mapping();
        m.pat_index = 8;
        assert_eq!(
            classify_write_back(&s, m.physical_page, 4096, &[m]),
            Err(CacheError::InvalidLeaf)
        );
        assert_eq!(
            classify_write_back(&s, 0x201000, 4096, &[]),
            Err(CacheError::MappingCountMismatch)
        );
        let mut second = mapping();
        second.physical_page += 8192;
        assert_eq!(
            classify_write_back(&s, 0x201000, 8192, &[mapping(), second]),
            Err(CacheError::NoncontiguousMapping)
        );
        second.physical_page -= 4096;
        assert_eq!(classify_write_back(&s, 0x201000, 8192, &[mapping(), second]).unwrap().pages, 2);
        assert_eq!(
            classify_write_back(&s, 0x201000, (MAX_PAGES as u64 + 1) * 4096, &[]),
            Err(CacheError::TooManyPages)
        );
        assert_eq!(
            classify_write_back(&s, u64::MAX - 4095, 4096, &[mapping()]),
            Err(CacheError::InvalidAllocation)
        );
        let mut s = s;
        s.top_mem += 4096;
        assert_eq!(check(&s), Err(CacheError::InvalidTopOfMemory));
    }
}
