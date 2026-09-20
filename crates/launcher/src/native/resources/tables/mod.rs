//! Bounded observations under the trusted UEFI x64 boot identity-map contract.
//! Retention permits a service-free comparison, not an immutable mapping lease.
//! No setter, table write or SVM instruction is provided. See the unsafe API
//! contracts: revalidation cannot make a stale/unmapped pointer safe again.

use core::{
    marker::PhantomData,
    mem::size_of,
    ptr::{self, NonNull},
};

#[cfg(target_os = "uefi")]
use svmvisor_hypervisor::arch::x86_64::msr::EFER;
use svmvisor_hypervisor::{
    boot::memory::MAX_GDT_BYTES, host::paging as host_paging, memory::address::is_canonical_48,
};
use svmvisor_launcher::native::admission::{
    cpu::QuiescentBsp, memory as native_memory, snapshot::NativeSnapshot,
};
use uefi_raw::{Status, table::boot::BootServices};

// Each image calls the subset of these entry points that its features select.
#[cfg(target_os = "uefi")]
#[allow(unused_imports)]
pub use self::preparation::observe;
// Each image calls the subset of these entry points that its features select.
#[cfg(any(target_os = "uefi", test))]
#[allow(unused_imports)]
pub use self::preparation::{
    prepare, prepare_owned_ranges, prepare_resource_ranges, prepare_resource_ranges_detailed,
};

mod attribute;
mod mapping;
mod preparation;
mod walk;

#[cfg(test)]
mod tests;

const MAX_RETAINED_ENTRIES: usize = 256;
const MAX_TABLE_PAGES: usize = 128;
pub const MAX_OWNED_RANGES: usize = 8;
pub const MAX_OWNED_PAGES: usize = 256;
pub const MAX_BORROWED_SPANS: usize = 32;
pub const MAX_BORROWED_PAGES: usize = 1024;
// Separate space for the exact retained pool and maximum 1 MiB + 128 KiB map
// allocation, including each allocation's potentially unaligned covering page.
const INTERNAL_SPANS: usize = 2;
const MAX_INTERNAL_PAGES: usize = 384;
const MAX_GDT_PAGES: usize = 17;

/// Owns the final map, GDT bytes and finite mapping observations. No firmware
/// callbacks or borrowed provider pointer are retained. Release at <= NOTIFY;
/// a failed free remains owned for retry, with Drop as a best-effort safeguard.
pub struct PreparedTables<'a> {
    services: &'a BootServices,
    storage: Option<NonNull<TableStorage>>,
    map: Option<native_memory::MemoryMapSnapshot<'a>>,
    before: NativeSnapshot,
    efer: u64,
    report: TableReport,
    not_send_sync: PhantomData<*mut ()>,
}

impl PreparedTables<'_> {
    #[allow(dead_code)] // Retained preparation API; scoped entry uses revalidate's report.
    pub const fn report(&self) -> TableReport {
        self.report
    }

    #[cfg(any(feature = "native-transition-test", feature = "native-returning"))]
    pub fn captured_gdt(&self) -> Result<&[u8], TableError> {
        self.storage()?.gdt.get(..self.report.gdt_bytes).ok_or(TableError::Bounds)
    }

    fn storage(&self) -> Result<&TableStorage, TableError> {
        if self.map.is_none() {
            return Err(TableError::Released);
        }
        let storage = self.storage.ok_or(TableError::Released)?;
        Ok(unsafe { storage.as_ref() })
    }

    pub fn retained_entry_count(&self) -> Result<usize, TableError> {
        Ok(self.storage()?.walks.entry_count)
    }

    pub fn retained_table_page_count(&self) -> Result<usize, TableError> {
        Ok(self.storage()?.walks.page_count)
    }

    /// Actual GDT leaves, retained by its existing read/write mapping proof.
    pub fn gdt_mappings(&self) -> Result<&[LeafObservation], TableError> {
        let storage = self.storage()?;
        storage.gdt_mappings.get(..storage.gdt_page_count).ok_or(TableError::Bounds)
    }

    /// Caller spans first, followed by the exact retained pool and map pool.
    pub fn borrowed_spans(&self) -> Result<&[BorrowedSpan], TableError> {
        let storage = self.storage()?;
        storage.borrowed_spans.get(..storage.borrowed_span_count).ok_or(TableError::Bounds)
    }

    pub fn caller_borrowed_span_count(&self) -> Result<usize, TableError> {
        Ok(self.storage()?.caller_borrowed_span_count)
    }

    /// Ascending covering pages, retaining complete actual containing leaves.
    pub fn borrowed_span_mappings(&self, index: usize) -> Result<&[LeafObservation], TableError> {
        let storage = self.storage()?;
        let spans = self.borrowed_spans()?;
        let selected = spans.get(index).ok_or(TableError::Bounds)?;
        let mut first = 0usize;
        for span in spans.get(..index).ok_or(TableError::Bounds)? {
            first = first.checked_add(covering_pages(*span, 52)?.1).ok_or(TableError::Bounds)?;
        }
        let end = first.checked_add(covering_pages(*selected, 52)?.1).ok_or(TableError::Bounds)?;
        storage
            .borrowed_mappings
            .get(..storage.borrowed_page_count)
            .and_then(|pages| pages.get(first..end))
            .ok_or(TableError::Bounds)
    }

    /// Ordered as supplied; each range's page observations are contiguous.
    pub fn owned_ranges(&self) -> Result<&[OwnedRange], TableError> {
        let storage = self.storage()?;
        storage.owned_ranges.get(..storage.owned_range_count).ok_or(TableError::Bounds)
    }

    /// Physical address order within one supplied allocation. This view carries
    /// actual leaf encodings; it does not classify PAT/MTRRs or grant ownership.
    pub fn owned_range_mappings(&self, index: usize) -> Result<&[LeafObservation], TableError> {
        let storage = self.storage()?;
        let ranges = self.owned_ranges()?;
        let selected = ranges.get(index).ok_or(TableError::Bounds)?;
        let mut first = 0usize;
        for range in ranges.get(..index).ok_or(TableError::Bounds)? {
            first = first.checked_add((range.bytes / 4096) as usize).ok_or(TableError::Bounds)?;
        }
        let end = first.checked_add((selected.bytes / 4096) as usize).ok_or(TableError::Bounds)?;
        storage
            .owned_mappings
            .get(..storage.owned_page_count)
            .and_then(|pages| pages.get(first..end))
            .ok_or(TableError::Bounds)
    }

    /// Closed table-source identity aliases plus observed hardware fetch roles.
    /// PCID mode has no supported CR3 fetch interpretation in this adapter.
    pub fn table_page_observations(&self) -> Result<&[TablePageObservation], TableError> {
        let storage = self.storage()?;
        if self.before.cr4 & (1 << 17) != 0 {
            return Err(TableError::TableFetch);
        }
        storage.walks.pages()
    }

    /// Compare controls first, then saved entry addresses and GDT bytes, then
    /// entries and controls again. No allocation, protocol or Boot Services call.
    /// A changed entry is never followed to derive another pointer.
    ///
    /// # Safety
    /// Invoke on the original BSP inside native_cpu's HIGH_LEVEL closure, with
    /// IF clear and the provider's no-dispatch/no-callback contract satisfied.
    /// All observed table allocations and their identity/readable mappings must
    /// have remained valid since prepare. Preparation should occur at NOTIFY
    /// with no callback-dispatch window before this scope. Intervening operations
    /// must not free/reassign these allocations or change their accessibility.
    /// The guard alone cannot prove that earlier firmware activity was harmless.
    /// SMM/NMI and hardware must preserve those ordinary firmware mappings;
    /// this check neither excludes them nor contains arbitrary access faults.
    /// Keep this object alive through the subsequent no-service interval; do
    /// not release/drop it at HIGH_LEVEL. Success is a finite stable observation,
    /// not a transferable permission to mutate tables or enter SVM.
    #[cfg(target_os = "uefi")]
    pub unsafe fn revalidate(&self, _cpu: &QuiescentBsp<'_>) -> Result<TableReport, TableError> {
        unsafe { self.compare_live(true) }?;
        Ok(self.report)
    }

    #[cfg(target_os = "uefi")]
    unsafe fn compare_live(&self, high_tpl: bool) -> Result<(), TableError> {
        use svmvisor_launcher::native::admission::snapshot as native_snapshot;
        let now = unsafe { native_snapshot::capture() }.map_err(|_| TableError::Snapshot)?;
        context_unchanged(&self.before, self.efer, &now, unsafe { read_efer() }, high_tpl)?;
        let storage = self.storage()?;
        storage.walks.compare(|address| unsafe { ptr::read_volatile(address as *const u64) })?;
        let bytes = storage.gdt.get(..self.report.gdt_bytes).ok_or(TableError::Bounds)?;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte
                != unsafe {
                    ptr::read_volatile((self.before.gdtr.base() + index as u64) as *const u8)
                }
            {
                return Err(TableError::Changed);
            }
        }
        storage.walks.compare(|address| unsafe { ptr::read_volatile(address as *const u64) })?;
        let after = unsafe { native_snapshot::capture() }.map_err(|_| TableError::Snapshot)?;
        context_unchanged(&self.before, self.efer, &after, unsafe { read_efer() }, high_tpl)
    }

    pub fn release(&mut self) -> Result<(), TableError> {
        let mut failed = false;
        if let Some(map) = self.map.as_mut() {
            if map.release().is_err() {
                failed = true;
            } else {
                self.map = None;
            }
        }
        if let Some(storage) = self.storage {
            if unsafe { (self.services.free_pool)(storage.as_ptr().cast()) } != Status::SUCCESS {
                failed = true;
            } else {
                self.storage = None;
            }
        }
        if failed { Err(TableError::Cleanup) } else { Ok(()) }
    }
}

impl Drop for PreparedTables<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

// One bounded pool, allocated before the final map. Zero is a valid initial
// representation for every field; initialize in-place to avoid a large stack
// temporary. Entry/page bounds are independent of the firmware's table shape.
struct TableStorage {
    gdt: [u8; MAX_GDT_BYTES],
    gdt_mappings: [LeafObservation; MAX_GDT_PAGES],
    gdt_page_count: usize,
    walks: RetainedWalks,
    owned_ranges: [OwnedRange; MAX_OWNED_RANGES],
    owned_range_count: usize,
    owned_mappings: [LeafObservation; MAX_OWNED_PAGES],
    owned_page_count: usize,
    borrowed_spans: [BorrowedSpan; MAX_BORROWED_SPANS + INTERNAL_SPANS],
    borrowed_span_count: usize,
    caller_borrowed_span_count: usize,
    borrowed_mappings: [LeafObservation; MAX_BORROWED_PAGES + MAX_INTERNAL_PAGES],
    borrowed_page_count: usize,
}
const _: () = assert!(core::mem::align_of::<TableStorage>() <= 8);
const _: () = assert!(
    (size_of::<TableStorage>() + 8190) / 4096 + native_memory::MAX_STORAGE_COVERING_PAGES
        <= MAX_INTERNAL_PAGES
);

struct RetainedWalks {
    entries: [EntryObservation; MAX_RETAINED_ENTRIES],
    entry_count: usize,
    table_pages: [TablePageObservation; MAX_TABLE_PAGES],
    page_count: usize,
}

#[derive(Clone, Copy)]
struct EntryObservation {
    address: u64,
    value: u64,
    allowed_set_bits: u64,
}

/// A caller-owned allocation extent, not an ownership or admission token.
/// Every extent must be nonempty, 4 KiB aligned and disjoint from the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnedRange {
    pub base: u64,
    pub bytes: u64,
}

/// Required access to verify, never a caller-supplied permission proof.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BorrowedAccess {
    #[default]
    Read,
    ReadWrite,
    ReadExecute,
}

/// Exact live bytes; rounding for observations does not assert ownership of
/// padding or neighboring objects. Overlapping borrowed spans are permitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BorrowedSpan {
    pub base: u64,
    pub bytes: u64,
    pub access: BorrowedAccess,
}

/// Actual identity-alias leaf from the validated walk. No cache type is implied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeafObservation {
    pub physical_page: u64,
    pub leaf_physical_base: u64,
    pub leaf_bytes: u64,
    pub pat_index: u8,
}

impl LeafObservation {
    fn identity(page: u64, translation: host_paging::Translation) -> Result<Self, TableError> {
        if translation.physical_address != page {
            return Err(TableError::NonIdentity);
        }
        Ok(Self {
            physical_page: page,
            leaf_physical_base: translation.physical_address & !(translation.page_bytes - 1),
            leaf_bytes: translation.page_bytes,
            pat_index: translation.pat_index,
        })
    }
}

/// The software identity alias and hardware-fetch encodings are distinct.
/// Bit i in fetch_pat_indices means index i (0..3) was used to fetch this
/// physical table page. Bit n in levels means level n+1 supplied an entry.
/// These are all observed paths, not an enumeration of every firmware alias.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TablePageObservation {
    pub physical_page: u64,
    pub alias: LeafObservation,
    pub fetch_pat_indices: u8,
    pub levels: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableReport {
    pub descriptors: usize,
    pub pages: usize,
    /// Original GDT translation reads; dependency-closure reads are separate.
    pub reads: usize,
    pub gdt_bytes: usize,
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    AttributeProtocol = 1,
    Allocation,
    MemoryMap,
    Metadata,
    Snapshot,
    Context,
    ReadPermission,
    Translation,
    NonIdentity,
    NotWritable,
    Descriptor,
    Changed,
    Cleanup,
    Bounds,
    Released,
    EntryTpl,
    OwnedRange,
    TableFetch,
    BorrowedSpan,
    NotExecutable,
    /// Exact NOT_FOUND selected the F7 path, but its initial qualification failed.
    AttributeFallback,
    /// The internal F7 Get or retained-read handoff failed after selection.
    AttributeFallbackRead,
}

/// The immediate LocateProtocol observation, before any interface dereference.
/// Non-SUCCESS includes warnings and retains the complete native EFI_STATUS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeLookupFailure {
    NonSuccess(Status),
    SuccessNull,
    SuccessUnaligned { remainder: u8 },
}

impl AttributeLookupFailure {
    /// Upper-half extension of the existing acquisition refusal family. Bit 26
    /// marks omitted status bits; the retained low code must then not be named
    /// as an exact EFI status. Invalid constructed variants stay unspecified.
    fn diagnostic_bits(self) -> u32 {
        match self {
            Self::NonSuccess(status) if status != Status::SUCCESS => {
                let raw = status.0;
                (1 << 28)
                    | (u32::from(raw & Status::ERROR_BIT != 0) << 27)
                    | (u32::from(raw & !(Status::ERROR_BIT | 0x3ff) != 0) << 26)
                    | (((raw & 0x3ff) as u32) << 16)
            }
            Self::SuccessNull => 2 << 28,
            Self::SuccessUnaligned { remainder: remainder @ 1..=7 } => {
                (3 << 28) | (u32::from(remainder) << 16)
            }
            _ => 0,
        }
    }
}

/// Detailed preparation error without changing the legacy TableError numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableFailure {
    pub kind: TableError,
    pub lookup: Option<AttributeLookupFailure>,
    pub fallback_reason: Option<u16>,
}

impl TableFailure {
    fn fallback(kind: TableError, reason: u16) -> Self {
        Self { kind, lookup: None, fallback_reason: Some(reason) }
    }
    /// Resource-family code only; the returning caller composes 0x4000 later.
    /// Cleanup and every other failure retain their original untagged numbers.
    pub fn resource_code(self) -> u64 {
        let diagnostic = if self.kind == TableError::AttributeProtocol {
            self.lookup.map_or(0, AttributeLookupFailure::diagnostic_bits)
        } else if matches!(
            self.kind,
            TableError::AttributeFallback | TableError::AttributeFallbackRead
        ) {
            self.fallback_reason.map_or(0, |reason| u32::from(reason) << 16)
        } else {
            0
        };
        (0x100 + self.kind as u64) | u64::from(diagnostic)
    }
}

impl From<TableError> for TableFailure {
    fn from(kind: TableError) -> Self {
        Self { kind, lookup: None, fallback_reason: None }
    }
}

impl From<AttributeLookupFailure> for TableFailure {
    fn from(lookup: AttributeLookupFailure) -> Self {
        Self { kind: TableError::AttributeProtocol, lookup: Some(lookup), fallback_reason: None }
    }
}

fn covering_pages(span: BorrowedSpan, physical_bits: u8) -> Result<(u64, usize), TableError> {
    if !(32..=52).contains(&physical_bits) || span.bytes == 0 {
        return Err(TableError::BorrowedSpan);
    }
    let end = span.base.checked_add(span.bytes).ok_or(TableError::BorrowedSpan)?;
    let rounded_end = end.checked_add(4095).ok_or(TableError::BorrowedSpan)? & !4095;
    let first = span.base & !4095;
    if rounded_end > (1u64 << physical_bits)
        || !is_canonical_48(span.base)
        || !is_canonical_48(end - 1)
        || !is_canonical_48(first)
        || !is_canonical_48(rounded_end - 1)
    {
        return Err(TableError::BorrowedSpan);
    }
    Ok((first, ((rounded_end - first) / 4096) as usize))
}

fn context_unchanged(
    before: &NativeSnapshot,
    efer: u64,
    after: &NativeSnapshot,
    after_efer: u64,
    high_tpl: bool,
) -> Result<(), TableError> {
    // Arithmetic flags are compiler temporaries; DF is invariant. IF is
    // intentionally cleared by HIGH_LEVEL, otherwise it must remain unchanged.
    let flags_changed = (before.rflags ^ after.rflags) & if high_tpl { 0x400 } else { 0x600 } != 0;
    if before.gdtr != after.gdtr
        || before.idtr != after.idtr
        || before.cr0 != after.cr0
        || before.cr3 != after.cr3
        || before.cr4 != after.cr4
        || efer != after_efer
        || flags_changed
        || (high_tpl && after.rflags & 0x200 != 0)
        || [before.cs, before.ss, before.ds, before.es] != [after.cs, after.ss, after.ds, after.es]
    {
        Err(TableError::Changed)
    } else {
        Ok(())
    }
}

/// Named architectural EFER read; caller checked AMD/MSR/long-mode CPUID/CPL0.
#[cfg(target_os = "uefi")]
#[inline(never)]
unsafe fn read_efer() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") EFER, out("eax") low, out("edx") high,
        options(nostack, nomem, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}
