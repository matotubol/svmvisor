//! Bounded observations under the trusted UEFI x64 boot identity-map contract.
//! Retention permits a service-free comparison, not an immutable mapping lease.
//! No setter, table write or SVM instruction is provided. See the unsafe API
//! contracts: revalidation cannot make a stale/unmapped pointer safe again.

use core::{
    marker::PhantomData,
    mem::size_of,
    ptr::{self, NonNull},
};

use svmvisor_dxe::native::admission::{
    cpu::QuiescentBsp, memory as native_memory, snapshot::NativeSnapshot,
};
use svmvisor_hypervisor::{
    boot::{
        descriptors::{CapturedGdtPage, FirmwareSelectors, parse_firmware_gdt},
        memory::{MAX_GDT_BYTES, ValidatedMemoryMap},
    },
    host::{
        descriptors::HostTablePointer,
        paging::{self as host_paging, PagingConfig},
    },
    memory::address::is_canonical_48,
};
use uefi_raw::{
    Status,
    protocol::memory_protection::MemoryAttributeProtocol,
    table::boot::{BootServices, MemoryAttribute, MemoryType, Tpl},
};

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
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;

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
        use svmvisor_dxe::native::admission::snapshot as native_snapshot;
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

impl RetainedWalks {
    fn entries(&self) -> Result<&[EntryObservation], TableError> {
        self.entries.get(..self.entry_count).ok_or(TableError::Bounds)
    }

    fn pages(&self) -> Result<&[TablePageObservation], TableError> {
        self.table_pages.get(..self.page_count).ok_or(TableError::Bounds)
    }

    fn translate(
        &mut self,
        config: PagingConfig,
        linear: u64,
        read: &mut impl FnMut(u64) -> Result<u64, TableError>,
    ) -> Result<host_paging::Translation, TableError> {
        let mut failure = None;
        let mut level = 4u8;
        let mut trace = [0u64; 4];
        let mut trace_count = 0usize;
        // With PCIDE set these CR3 bits are PCID bits, not fetch selectors.
        // Compatibility prepare still supports that mode, but owned-resource
        // preparation and the cache-facing table view conservatively refuse it.
        let mut fetch_index = if config.pcid { None } else { Some(((config.cr3 >> 3) & 3) as u8) };
        let translation = host_paging::translate(config, linear, |address| {
            let result = read(address).and_then(|value| {
                self.remember_entry(address, value)?;
                self.remember_fetch(address & !4095, level, fetch_index)?;
                *trace.get_mut(trace_count).ok_or(TableError::Bounds)? = address;
                trace_count += 1;
                level = level.checked_sub(1).ok_or(TableError::Bounds)?;
                // The walker will call again only for a validated non-leaf.
                // Leaf PCD/PWT therefore never become a table-fetch encoding.
                fetch_index = Some(((value >> 3) & 3) as u8);
                Ok(value)
            });
            match result {
                Ok(value) => Some(value),
                Err(error) => {
                    failure = Some(error);
                    None
                }
            }
        })
        .map_err(|_| failure.unwrap_or(TableError::Translation))?;
        // A successful complete walk validates every source role. Its final
        // entry is the leaf; preceding entries are non-leaves. Do not infer a
        // leaf from bit 7 alone (that bit is PAT in a 4 KiB PTE). Accumulate per
        // physical slot if recursive/shared tables use it in multiple roles.
        for (index, address) in
            trace.get(..trace_count).ok_or(TableError::Bounds)?.iter().enumerate()
        {
            let entry = self
                .entries
                .get_mut(..self.entry_count)
                .ok_or(TableError::Bounds)?
                .iter_mut()
                .find(|entry| entry.address == *address)
                .ok_or(TableError::Bounds)?;
            entry.allowed_set_bits |= if index + 1 == trace_count { 0x60 } else { 0x20 };
        }
        Ok(translation)
    }

    fn remember_entry(&mut self, address: u64, value: u64) -> Result<(), TableError> {
        if address & 7 != 0 {
            return Err(TableError::Metadata);
        }
        if let Some(previous) = self.entries()?.iter().find(|entry| entry.address == address) {
            return if previous.value == value { Ok(()) } else { Err(TableError::Changed) };
        }
        self.remember_page(address & !4095)?;
        *self.entries.get_mut(self.entry_count).ok_or(TableError::Bounds)? =
            EntryObservation { address, value, allowed_set_bits: 0 };
        self.entry_count += 1;
        Ok(())
    }

    fn remember_page(&mut self, page: u64) -> Result<(), TableError> {
        if !is_canonical_48(page) || !is_canonical_48(page | 4095) || page & 4095 != 0 {
            return Err(TableError::Context);
        }
        let pages = self.table_pages.get(..self.page_count).ok_or(TableError::Bounds)?;
        if pages.iter().any(|entry| entry.physical_page == page) {
            return Ok(());
        }
        *self.table_pages.get_mut(self.page_count).ok_or(TableError::Bounds)? =
            TablePageObservation { physical_page: page, ..TablePageObservation::default() };
        self.page_count += 1;
        Ok(())
    }

    fn remember_fetch(
        &mut self,
        page: u64,
        level: u8,
        pat_index: Option<u8>,
    ) -> Result<(), TableError> {
        if !(1..=4).contains(&level) || pat_index.is_some_and(|index| index > 3) {
            return Err(TableError::Bounds);
        }
        self.remember_page(page)?;
        let observed = self
            .table_pages
            .get_mut(..self.page_count)
            .ok_or(TableError::Bounds)?
            .iter_mut()
            .find(|entry| entry.physical_page == page)
            .ok_or(TableError::Bounds)?;
        observed.levels |= 1u8
            .checked_shl(u32::from(level.checked_sub(1).ok_or(TableError::Bounds)?))
            .ok_or(TableError::Bounds)?;
        if let Some(index) = pat_index {
            observed.fetch_pat_indices |=
                1u8.checked_shl(u32::from(index)).ok_or(TableError::Bounds)?;
        }
        Ok(())
    }

    /// Walk the identity mapping of the root and EVERY entry-source page,
    /// including any new dependency pages those walks discover. This closes
    /// self/root mapping dependencies without recursion or an unchecked root.
    fn close_dependencies(
        &mut self,
        config: PagingConfig,
        read: &mut impl FnMut(u64) -> Result<u64, TableError>,
    ) -> Result<(), TableError> {
        self.remember_page(config.cr3 & ADDRESS)?;
        let mut index = 0;
        while index < self.page_count {
            let page = self.table_pages.get(index).ok_or(TableError::Bounds)?.physical_page;
            let translation = self.translate(config, page, read)?;
            self.table_pages.get_mut(index).ok_or(TableError::Bounds)?.alias =
                LeafObservation::identity(page, translation)?;
            // Table sources need read access only. Effective RW is separately
            // required for GDT pages, which the eventual VMEXIT may update.
            index += 1;
        }
        Ok(())
    }

    fn compare(&self, mut read: impl FnMut(u64) -> u64) -> Result<(), TableError> {
        for entry in self.entries()? {
            if read(entry.address) != entry.value {
                return Err(TableError::Changed);
            }
        }
        Ok(())
    }

    /// Preparation itself may set A/D while touching newly observed pages.
    /// Establish the final exact baseline only after all pages/GDT were read.
    /// Admit only 0->1 A on validated non-leaves or A/D on validated leaves;
    /// this is architectural compatibility, not proof of the change's cause.
    /// Address/permissions/presence and clearing any
    /// bit remain refusal. Subsequent comparisons permit no changes at all.
    fn settle_accessed_dirty(
        &mut self,
        mut read: impl FnMut(u64) -> u64,
    ) -> Result<(), TableError> {
        let entries = self.entries.get_mut(..self.entry_count).ok_or(TableError::Bounds)?;
        for entry in entries {
            let current = read(entry.address);
            if (entry.value ^ current) & !entry.allowed_set_bits != 0 || entry.value & !current != 0
            {
                return Err(TableError::Changed);
            }
            entry.value = current;
        }
        Ok(())
    }
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

/// The real interface is always preferred. Only exact EFI_NOT_FOUND can select
/// the explicitly enabled F7 compatibility reader; malformed SUCCESS pointers,
/// warnings and other failures keep their original lookup diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttributeSource {
    Firmware(NonNull<MemoryAttributeProtocol>),
    #[cfg(feature = "memory-attribute-f7")]
    F7,
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

/// All pool allocations precede the final map. A real memory attribute protocol
/// is preferred. The explicit F7 feature permits only exact EFI_NOT_FOUND to
/// select an internal Get-only reader under the same unsafe initial-access
/// contract below. It installs no protocol and does not relax retained checks.
///
/// # Safety
/// Live conforming UEFI x64 Boot Services, CPL0 and admitted AMD/MSR/long-mode
/// CPUID at APPLICATION or NOTIFY. Caller uses the trusted firmware identity-map
/// contract for initial reads. All preparation must precede the no-service
/// interval. No invalidating allocations/frees/permission changes may intervene
/// while observing; protocol calls must preserve the observed live mappings.
/// For retained use, also satisfy revalidate's stronger lifetime contract.
#[cfg(any(target_os = "uefi", test))]
pub unsafe fn prepare(
    services: &BootServices,
    physical_bits: u8,
    page1gb: bool,
) -> Result<PreparedTables<'_>, TableError> {
    unsafe { prepare_owned_ranges(services, physical_bits, page1gb, &[]) }
}

/// Prepare actual mappings for caller-owned native allocation ranges along with
/// the original GDT/table closure. No resource bytes are read or changed.
///
/// # Safety
/// All of prepare's firmware/initial-reader conditions apply. The caller must
/// already own each supplied, allocated RAM extent exclusively and retain it
/// through the entire prepared lifetime and service-free interval. The map and
/// these numeric inputs cannot establish allocation ownership. Allocate/fill
/// resources before this call; do not free/reassign them or change mappings or
/// permissions while observations are retained. The original tables/GDT are
/// borrowed firmware allocations and must not overlap any supplied extent.
/// The current restricted resource profile requires PCIDE=0. It observes data
/// read/write mappings; executable paths and other live operands need separate
/// coverage. Success does not prove WB, encryption state, TLB agreement, global
/// alias compatibility, cross-CPU consistency, DMA exclusion or SVM admission.
#[cfg(any(target_os = "uefi", test))]
pub unsafe fn prepare_owned_ranges<'a>(
    services: &'a BootServices,
    physical_bits: u8,
    page1gb: bool,
    ranges: &[OwnedRange],
) -> Result<PreparedTables<'a>, TableError> {
    unsafe { prepare_resource_ranges(services, physical_bits, page1gb, ranges, &[]) }
}

/// Retain caller-owned allocations, exact borrowed operand spans, GDT leaves,
/// both internal pool extents, and the complete bounded table-source closure.
/// Caller spans precede the retained pool and map pool in borrowed_spans().
///
/// # Safety
/// All prepare_owned_ranges firmware, lifetime and ownership conditions apply.
/// Caller borrowed bytes must describe actual live operands and remain valid
/// through restoration; the requested access is independently verified. Do not
/// invent a code length or stack extent from a pointer. Borrowed bytes may
/// overlap other borrowed spans, but must not overlap an owned arena extent.
/// Caller-owned storage borrowed under this API remains the caller's lifetime
/// responsibility. Internal pools are retained by PreparedTables itself.
/// This API does not read/write/execute caller operand bytes or prove AP stack,
/// alternate-alias, TLB/PDC, DMA, SMM/NMI or complete native-launch admission.
#[cfg(any(target_os = "uefi", test))]
pub unsafe fn prepare_resource_ranges<'a>(
    services: &'a BootServices,
    physical_bits: u8,
    page1gb: bool,
    ranges: &[OwnedRange],
    borrowed: &[BorrowedSpan],
) -> Result<PreparedTables<'a>, TableError> {
    unsafe { prepare_resource_ranges_detailed(services, physical_bits, page1gb, ranges, borrowed) }
        .map_err(|failure| failure.kind)
}

/// The same preparation as prepare_resource_ranges, retaining the immediate
/// attribute-protocol lookup failure for the compact resource diagnostic.
///
/// # Safety
/// All prepare_resource_ranges firmware, lifetime and ownership conditions apply.
#[cfg(any(target_os = "uefi", test))]
#[cfg_attr(feature = "native-returning", inline(never))]
pub unsafe fn prepare_resource_ranges_detailed<'a>(
    services: &'a BootServices,
    physical_bits: u8,
    page1gb: bool,
    ranges: &[OwnedRange],
    borrowed: &[BorrowedSpan],
) -> Result<PreparedTables<'a>, TableFailure> {
    use TableError as E;
    validate_owned_ranges(ranges, physical_bits)?;
    validate_borrowed_spans(borrowed, physical_bits, ranges, false)?;
    let previous = unsafe { (services.raise_tpl)(Tpl::HIGH_LEVEL) };
    unsafe { (services.restore_tpl)(previous) };
    if previous != Tpl::APPLICATION && previous != Tpl::NOTIFY {
        return Err(E::EntryTpl.into());
    }
    let source = select_attribute_source(unsafe { acquire_memory_attributes(services) })?;
    let mut buffer = ptr::null_mut();
    let status = unsafe {
        (services.allocate_pool)(
            MemoryType::BOOT_SERVICES_DATA,
            size_of::<TableStorage>(),
            &mut buffer,
        )
    };
    if status != Status::SUCCESS || buffer.is_null() {
        return Err(E::Allocation.into());
    }
    let mut prepared = PreparedTables {
        services,
        storage: NonNull::new(buffer.cast()),
        map: None,
        before: NativeSnapshot::default(),
        efer: 0,
        report: TableReport { descriptors: 0, pages: 0, reads: 0, gdt_bytes: 0 },
        not_send_sync: PhantomData,
    };
    let result = if buffer.addr() % core::mem::align_of::<TableStorage>() != 0 {
        Err(E::Allocation.into())
    } else {
        unsafe { ptr::write_bytes(buffer, 0, size_of::<TableStorage>()) };
        unsafe { initialize(&mut prepared, source, physical_bits, page1gb, ranges, borrowed) }
    };
    if let Err(error) = result {
        prepared.release()?;
        return Err(error.into());
    }
    Ok(prepared)
}

/// Convenience observation with explicit cleanup and compatible report/refusal
/// numbers. The returned report is historical; nothing remains retained.
///
/// # Safety
/// The same preparation contract applies. This does not create CPU ownership.
#[cfg(target_os = "uefi")]
#[allow(dead_code)] // Compatibility observation API; production preflight uses scoped preparation.
pub unsafe fn observe(
    services: &BootServices,
    physical_bits: u8,
    page1gb: bool,
) -> Result<TableReport, TableError> {
    let mut prepared = unsafe { prepare(services, physical_bits, page1gb) }?;
    let report = prepared.report();
    prepared.release()?;
    Ok(report)
}

// Host tests exercise actual lookup, ordering, wrappers and allocation refusal.
// Reaching privileged initialization is always a test error, never a provider
// or native-reader substitute. Production uses only the UEFI implementation.
#[cfg(all(test, not(target_os = "uefi")))]
unsafe fn initialize(
    _: &mut PreparedTables<'_>,
    _: AttributeSource,
    _: u8,
    _: bool,
    _: &[OwnedRange],
    _: &[BorrowedSpan],
) -> Result<(), TableFailure> {
    panic!("host lookup tests must stop before privileged table initialization")
}

#[cfg(target_os = "uefi")]
unsafe fn initialize(
    prepared: &mut PreparedTables<'_>,
    source: AttributeSource,
    physical_bits: u8,
    page1gb: bool,
    ranges: &[OwnedRange],
    borrowed: &[BorrowedSpan],
) -> Result<(), TableFailure> {
    use TableError as E;
    use svmvisor_dxe::native::admission::snapshot as native_snapshot;
    prepared.map = Some(unsafe { native_memory::collect(prepared.services) }.map_err(|error| {
        // Preserve an explicit nested free failure even if its Drop later
        // succeeds; the outer caller must not report complete cleanup.
        if matches!(error, native_memory::MemoryMapError::Cleanup(_)) {
            E::Cleanup
        } else {
            E::MemoryMap
        }
    })?);
    let map = prepared.map.as_ref().ok_or(E::Released)?;
    let map_range = map.storage_range().map_err(|_| E::Released)?;
    let table_base = prepared.storage.ok_or(E::Released)?.as_ptr() as u64;
    let internal = [
        BorrowedSpan {
            base: table_base,
            bytes: size_of::<TableStorage>() as u64,
            access: BorrowedAccess::ReadWrite,
        },
        BorrowedSpan {
            base: map_range.base,
            bytes: map_range.bytes,
            access: BorrowedAccess::ReadWrite,
        },
    ];
    validate_borrowed_spans(&internal, physical_bits, ranges, true)?;
    let descriptors = map.descriptors();
    let memory = ValidatedMemoryMap::new(descriptors, physical_bits).map_err(|_| E::Metadata)?;
    let before = unsafe { native_snapshot::capture() }.map_err(|_| E::Snapshot)?;
    if before.cr0 & 0x80000001 != 0x80000001
        || before.cr4 & (1 << 5) == 0
        || before.cr4 & (1 << 12) != 0
        || before.cr4 & ((1 << 21) | (1 << 22) | (1 << 24)) != 0
    {
        return Err(E::Context.into());
    }
    let efer = unsafe { read_efer() };
    if efer & (1 << 10) == 0 {
        return Err(E::Context.into());
    }
    let config = PagingConfig {
        cr3: before.cr3,
        physical_bits,
        la57: false,
        nxe: efer & (1 << 11) != 0,
        pcid: before.cr4 & (1 << 17) != 0,
        page1gb,
    };
    if (!ranges.is_empty() || !borrowed.is_empty()) && config.pcid {
        return Err(E::TableFetch.into());
    }
    #[cfg(feature = "memory-attribute-f7")]
    let fallback = core::cell::RefCell::new(match source {
        AttributeSource::Firmware(_) => None,
        AttributeSource::F7 => Some(
            unsafe {
                // The explicit prepare contract supplies the initial firmware
                // identity/residency premise. Metadata and this constructor do not
                // invent permission proof or qualify arbitrary physical pointers.
                svmvisor_dxe::memory_attributes::f7::F7TableReader::new_detailed(
                    &memory,
                    svmvisor_memory_attributes::Config {
                        root: config.cr3 & ADDRESS,
                        physical_bits,
                        nxe: config.nxe,
                        page1gb,
                    },
                )
            }
            .map_err(|failure| TableFailure::fallback(E::AttributeFallback, failure.code()))?,
        ),
    });
    #[cfg(feature = "memory-attribute-f7")]
    let fallback_failed = core::cell::Cell::new(false);
    #[cfg(feature = "memory-attribute-f7")]
    let fallback_reason = core::cell::Cell::new(0u16);
    let current = |address: u64, access: BorrowedAccess| match source {
        AttributeSource::Firmware(interface) => unsafe {
            current_access(interface.as_ref(), address, access)
        },
        #[cfg(feature = "memory-attribute-f7")]
        AttributeSource::F7 => {
            let Ok(mut reader) = fallback.try_borrow_mut() else {
                fallback_reason.set(0xfffe);
                fallback_failed.set(true);
                return false;
            };
            let query = match reader.as_mut() {
                Some(reader) => reader.get_detailed(address & !4095, 4096).map_err(|failure| {
                    fallback_reason.set(failure.code());
                }),
                None => {
                    // Structurally unreachable: selection and construction are
                    // paired above. Keep a distinct internal invariant code.
                    fallback_reason.set(u16::MAX);
                    Err(())
                }
            };
            internal_access(query, access, &fallback_failed)
        }
    };
    let result = (|| -> Result<TableReport, TableError> {
        let base = before.gdtr.base();
        let bytes = usize::from(before.gdtr.limit()) + 1;
        reject_owned_overlap(ranges, base, bytes as u64)?;
        memory.permit_gdt_copy(base, bytes).map_err(|_| E::Metadata)?;
        let last = base.checked_add(bytes as u64 - 1).ok_or(E::Metadata)?;
        if !is_canonical_48(base) || !is_canonical_48(last) {
            return Err(E::Context);
        }
        let first_page = base & !4095;
        let count = (((last & !4095) - first_page) / 4096 + 1) as usize;
        let mut coverage =
            [CapturedGdtPage { linear_page: 0, present: false, writable: false }; MAX_GDT_PAGES];
        let pages = coverage.get_mut(..count).ok_or(E::Metadata)?;
        let storage = unsafe { prepared.storage.ok_or(E::Released)?.as_mut() };
        let mut reads = 0;
        let mut reader = |address| {
            let value = read_checked_entry(
                &memory,
                address,
                |address| current(address, BorrowedAccess::Read),
                |address| unsafe { ptr::read_volatile(address as *const u64) },
            )?;
            reads += 1;
            Ok(value)
        };
        for (index, page) in pages.iter_mut().enumerate() {
            let linear = first_page + index as u64 * 4096;
            let translation = storage.walks.translate(config, linear, &mut reader)?;
            let observed = LeafObservation::identity(linear, translation)?;
            if !translation.writable {
                return Err(E::NotWritable);
            }
            memory.permit_gdt_copy(linear, 4096).map_err(|_| E::Metadata)?;
            if !current(linear, BorrowedAccess::ReadWrite) {
                return Err(E::ReadPermission);
            }
            *page = CapturedGdtPage { linear_page: linear, present: true, writable: true };
            *storage.gdt_mappings.get_mut(index).ok_or(E::Bounds)? = observed;
        }
        storage.gdt_page_count = count;
        // Keep the established diagnostic count limited to the GDT walks. Root and
        // every entry page are additionally mapped below; closure is bounded too.
        let gdt_reads = reads;
        let mut reader = |address| {
            read_checked_entry(
                &memory,
                address,
                |address| current(address, BorrowedAccess::Read),
                |address| unsafe { ptr::read_volatile(address as *const u64) },
            )
        };
        storage.walks.close_dependencies(config, &mut reader)?;
        retain_owned_mappings(
            storage,
            ranges,
            &memory,
            config,
            &mut |address| current(address, BorrowedAccess::ReadWrite),
            &mut reader,
        )?;
        let smep = before.cr4 & (1 << 20) != 0;
        retain_borrowed_mappings(
            storage,
            borrowed,
            ranges,
            &memory,
            config,
            smep,
            false,
            &mut |address, access| current(address, access),
            &mut reader,
        )?;
        // Both exact allocation extents are available only after the final map.
        // They use reserved storage in the already allocated pool; no new pool or
        // recursive preparation is needed. RW includes subsequent retained writes.
        retain_borrowed_mappings(
            storage,
            &internal,
            ranges,
            &memory,
            config,
            smep,
            true,
            &mut |address, access| current(address, access),
            &mut reader,
        )?;
        storage.walks.close_dependencies(config, &mut reader)?;
        for page in storage.walks.pages()? {
            reject_owned_overlap(ranges, page.physical_page, 4096)?;
        }
        let output = storage.gdt.get_mut(..bytes).ok_or(E::Bounds)?;
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = unsafe { ptr::read_volatile((base + index as u64) as *const u8) };
        }
        let parsed = parse_firmware_gdt(
            HostTablePointer { base, limit: before.gdtr.limit() },
            FirmwareSelectors { cs: before.cs, ss: before.ss, ds: before.ds, es: before.es },
            output,
        )
        .map_err(|_| E::Descriptor)?;
        parsed.validate_mapping_capture(pages).map_err(|_| E::Translation)?;
        storage.walks.settle_accessed_dirty(|address| unsafe {
            ptr::read_volatile(address as *const u64)
        })?;
        Ok(TableReport {
            descriptors: descriptors.len(),
            pages: count,
            reads: gdt_reads,
            gdt_bytes: bytes,
        })
    })();
    #[cfg(feature = "memory-attribute-f7")]
    {
        if fallback_failed.get() {
            return Err(TableFailure::fallback(E::AttributeFallbackRead, fallback_reason.get()));
        }
        if let Some(reader) = fallback
            .try_borrow_mut()
            .map_err(|_| TableFailure::fallback(E::AttributeFallbackRead, 0xfffe))?
            .as_mut()
        {
            // The bounded closure is already retained by PreparedTables. Check
            // original controls once more before dropping this Get-only view;
            // later HIGH comparison follows only retained, verified addresses.
            reader.finish_handoff_detailed().map_err(|failure| {
                TableFailure::fallback(E::AttributeFallbackRead, failure.code())
            })?;
        }
        drop(fallback);
    }
    let report = result?;
    prepared.before = before;
    prepared.efer = efer;
    prepared.report = report;
    unsafe { prepared.compare_live(false) }.map_err(TableFailure::from)
}

/// Perform exactly one raw lookup, accepting only SUCCESS, non-NULL and aligned.
/// A returned pointer on non-SUCCESS is ignored, without dereferencing it.
///
/// # Safety
/// Live conforming UEFI x64 Boot Services, at TPL <= NOTIFY. The caller retains
/// the protocol's firmware lifetime contract before using the resulting pointer.
unsafe fn acquire_memory_attributes(
    services: &BootServices,
) -> Result<NonNull<MemoryAttributeProtocol>, AttributeLookupFailure> {
    // The persisted interface remainder is defined for the UEFI x64 ABI.
    const _: () = assert!(core::mem::align_of::<MemoryAttributeProtocol>() == 8);
    const _: () = assert!(usize::BITS == 64);
    let mut interface = ptr::null_mut();
    let status = unsafe {
        (services.locate_protocol)(&MemoryAttributeProtocol::GUID, ptr::null_mut(), &mut interface)
    };
    if status != Status::SUCCESS {
        return Err(AttributeLookupFailure::NonSuccess(status));
    }
    let interface = NonNull::new(interface.cast::<MemoryAttributeProtocol>())
        .ok_or(AttributeLookupFailure::SuccessNull)?;
    let remainder = interface.as_ptr().addr() % core::mem::align_of::<MemoryAttributeProtocol>();
    if remainder != 0 {
        return Err(AttributeLookupFailure::SuccessUnaligned { remainder: remainder as u8 });
    }
    Ok(interface)
}

fn select_attribute_source(
    lookup: Result<NonNull<MemoryAttributeProtocol>, AttributeLookupFailure>,
) -> Result<AttributeSource, AttributeLookupFailure> {
    match lookup {
        Ok(interface) => Ok(AttributeSource::Firmware(interface)),
        #[cfg(feature = "memory-attribute-f7")]
        Err(AttributeLookupFailure::NonSuccess(Status::NOT_FOUND)) => Ok(AttributeSource::F7),
        Err(error) => Err(error),
    }
}

unsafe fn current_access(
    protocol: &MemoryAttributeProtocol,
    address: u64,
    access: BorrowedAccess,
) -> bool {
    let mut attributes = MemoryAttribute::empty();
    let status = unsafe {
        (protocol.get_memory_attributes)(protocol, address & !4095, 4096, &mut attributes)
    };
    attributes_allow(status, attributes, access)
}

#[cfg(any(feature = "memory-attribute-f7", test))]
fn internal_access<E>(
    query: Result<u64, E>,
    access: BorrowedAccess,
    failed: &core::cell::Cell<bool>,
) -> bool {
    match query {
        Ok(attributes) => {
            attributes_allow(Status::SUCCESS, MemoryAttribute::from_bits_retain(attributes), access)
        }
        Err(_) => {
            failed.set(true);
            false
        }
    }
}

fn attributes_allow(status: Status, attributes: MemoryAttribute, access: BorrowedAccess) -> bool {
    status == Status::SUCCESS
        && !attributes.contains(MemoryAttribute::READ_PROTECT)
        && (access != BorrowedAccess::ReadWrite || attributes.bits() & 0x20000 == 0)
        && (access != BorrowedAccess::ReadExecute
            || !attributes.contains(MemoryAttribute::EXECUTE_PROTECT))
}

fn retain_owned_mappings(
    storage: &mut TableStorage,
    ranges: &[OwnedRange],
    memory: &ValidatedMemoryMap<'_>,
    config: PagingConfig,
    current_writable: &mut impl FnMut(u64) -> bool,
    read: &mut impl FnMut(u64) -> Result<u64, TableError>,
) -> Result<(), TableError> {
    let page_count = validate_owned_ranges(ranges, config.physical_bits)?;
    if !ranges.is_empty() && config.pcid {
        return Err(TableError::TableFetch);
    }
    // Reject any hole/unallocated/non-RAM page across the entire request before
    // invoking current-permission queries or any page-table reader for it.
    for range in ranges {
        for index in 0..range.bytes / 4096 {
            let page = range.base + index * 4096; // Shape/extent validated above.
            memory.permit_gdt_copy(page, 4096).map_err(|_| TableError::Metadata)?;
        }
    }
    let mut next = 0usize;
    for range in ranges {
        for index in 0..range.bytes / 4096 {
            let page = range.base + index * 4096;
            if !current_writable(page) {
                return Err(TableError::ReadPermission);
            }
            let translation = storage.walks.translate(config, page, read)?;
            let observed = LeafObservation::identity(page, translation)?;
            if !translation.writable {
                return Err(TableError::NotWritable);
            }
            *storage.owned_mappings.get_mut(next).ok_or(TableError::Bounds)? = observed;
            next += 1;
        }
    }
    if next != page_count {
        return Err(TableError::Bounds);
    }
    for (destination, source) in storage
        .owned_ranges
        .get_mut(..ranges.len())
        .ok_or(TableError::Bounds)?
        .iter_mut()
        .zip(ranges)
    {
        *destination = *source;
    }
    storage.owned_range_count = ranges.len();
    storage.owned_page_count = page_count;
    Ok(())
}

fn retain_borrowed_mappings(
    storage: &mut TableStorage,
    spans: &[BorrowedSpan],
    owned: &[OwnedRange],
    memory: &ValidatedMemoryMap<'_>,
    config: PagingConfig,
    smep: bool,
    internal: bool,
    current: &mut impl FnMut(u64, BorrowedAccess) -> bool,
    read: &mut impl FnMut(u64) -> Result<u64, TableError>,
) -> Result<(), TableError> {
    let page_count = validate_borrowed_spans(spans, config.physical_bits, owned, internal)?;
    if !spans.is_empty() && !internal && config.pcid {
        return Err(TableError::TableFetch);
    }
    let first_span = storage.borrowed_span_count;
    let end_span = first_span.checked_add(spans.len()).ok_or(TableError::Bounds)?;
    let first_mapping = storage.borrowed_page_count;
    let end_mapping = first_mapping.checked_add(page_count).ok_or(TableError::Bounds)?;
    storage.borrowed_spans.get(first_span..end_span).ok_or(TableError::Bounds)?;
    storage.borrowed_mappings.get(first_mapping..end_mapping).ok_or(TableError::Bounds)?;
    // Metadata for this complete request precedes its permission queries and
    // walks. These callbacks never load or execute the borrowed operand bytes.
    for span in spans {
        let (first, count) = covering_pages(*span, config.physical_bits)?;
        for index in 0..count {
            memory
                .permit_gdt_copy(first + index as u64 * 4096, 4096)
                .map_err(|_| TableError::Metadata)?;
        }
    }
    let mut next = first_mapping;
    for span in spans {
        let (first, count) = covering_pages(*span, config.physical_bits)?;
        for index in 0..count {
            let page = first + index as u64 * 4096;
            if !current(page, span.access) {
                return Err(TableError::ReadPermission);
            }
            let translation = storage.walks.translate(config, page, read)?;
            let observed = LeafObservation::identity(page, translation)?;
            if span.access == BorrowedAccess::ReadWrite && !translation.writable {
                return Err(TableError::NotWritable);
            }
            if span.access == BorrowedAccess::ReadExecute
                && (!translation.executable || (smep && translation.user))
            {
                return Err(TableError::NotExecutable);
            }
            *storage.borrowed_mappings.get_mut(next).ok_or(TableError::Bounds)? = observed;
            next += 1;
        }
    }
    if next != end_mapping {
        return Err(TableError::Bounds);
    }
    for (destination, source) in storage
        .borrowed_spans
        .get_mut(first_span..end_span)
        .ok_or(TableError::Bounds)?
        .iter_mut()
        .zip(spans)
    {
        *destination = *source;
    }
    storage.borrowed_span_count = end_span;
    storage.borrowed_page_count = end_mapping;
    if !internal {
        storage.caller_borrowed_span_count = end_span;
    }
    Ok(())
}

fn validate_owned_ranges(ranges: &[OwnedRange], physical_bits: u8) -> Result<usize, TableError> {
    if ranges.len() > MAX_OWNED_RANGES {
        return Err(TableError::Bounds);
    }
    if ranges.is_empty() {
        return Ok(0);
    }
    if !(32..=52).contains(&physical_bits) {
        return Err(TableError::OwnedRange);
    }
    let mut pages = 0u64;
    for (index, range) in ranges.iter().enumerate() {
        let end = range.base.checked_add(range.bytes).ok_or(TableError::OwnedRange)?;
        if range.bytes == 0
            || (range.base | range.bytes) & 4095 != 0
            || end > (1u64 << physical_bits)
            || !is_canonical_48(range.base)
            || !is_canonical_48(end - 1)
        {
            return Err(TableError::OwnedRange);
        }
        pages = pages.checked_add(range.bytes / 4096).ok_or(TableError::Bounds)?;
        if pages > MAX_OWNED_PAGES as u64 {
            return Err(TableError::Bounds);
        }
        for previous in ranges.get(..index).ok_or(TableError::Bounds)? {
            let previous_end =
                previous.base.checked_add(previous.bytes).ok_or(TableError::OwnedRange)?;
            if range.base < previous_end && previous.base < end {
                return Err(TableError::OwnedRange);
            }
        }
    }
    Ok(pages as usize)
}

fn validate_borrowed_spans(
    spans: &[BorrowedSpan],
    physical_bits: u8,
    owned: &[OwnedRange],
    internal: bool,
) -> Result<usize, TableError> {
    let span_limit = if internal { INTERNAL_SPANS } else { MAX_BORROWED_SPANS };
    let page_limit = if internal { MAX_INTERNAL_PAGES } else { MAX_BORROWED_PAGES };
    if spans.len() > span_limit {
        return Err(TableError::Bounds);
    }
    let mut pages = 0usize;
    for span in spans {
        let (_, count) = covering_pages(*span, physical_bits)?;
        pages = pages.checked_add(count).ok_or(TableError::Bounds)?;
        if pages > page_limit {
            return Err(TableError::Bounds);
        }
        // Only the actual byte extents assert conflicting provenance. Borrowed
        // spans can share covering pages and overlap one another freely.
        reject_owned_overlap(owned, span.base, span.bytes)?;
    }
    Ok(pages)
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

fn reject_owned_overlap(
    ranges: &[OwnedRange],
    borrowed_base: u64,
    borrowed_bytes: u64,
) -> Result<(), TableError> {
    let borrowed_end = borrowed_base.checked_add(borrowed_bytes).ok_or(TableError::Bounds)?;
    for range in ranges {
        let end = range.base.checked_add(range.bytes).ok_or(TableError::OwnedRange)?;
        if range.base < borrowed_end && borrowed_base < end {
            return Err(TableError::OwnedRange);
        }
    }
    Ok(())
}

/// Metadata and the actual current-permission query precede every source load.
/// The final callback is the only operation permitted to dereference a source.
fn read_checked_entry(
    memory: &ValidatedMemoryMap<'_>,
    address: u64,
    current: impl FnOnce(u64) -> bool,
    read: impl FnOnce(u64) -> u64,
) -> Result<u64, TableError> {
    if !is_canonical_48(address)
        || !is_canonical_48(address | 4095)
        || memory.permit_table_entry(address).is_err()
        || memory.permit_gdt_copy(address & !4095, 4096).is_err()
        || !current(address)
    {
        return Err(TableError::ReadPermission);
    }
    Ok(read(address))
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
        core::arch::asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") low, out("edx") high,
        options(nostack, nomem, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}

#[cfg(test)]
mod lookup_tests {
    use super::*;
    use core::{ffi::c_void, mem::MaybeUninit};
    use std::cell::RefCell;
    use uefi_raw::Guid;

    struct State {
        status: Status,
        output: Option<usize>,
        tpl: Tpl,
        calls: Vec<&'static str>,
        unaligned_allocation: bool,
        free_failures: usize,
    }
    thread_local! {
        static STATE: RefCell<State> = const { RefCell::new(State {
            status: Status::SUCCESS,
            output: None,
            tpl: Tpl::NOTIFY,
            calls: Vec::new(),
            unaligned_allocation: false,
            free_failures: 0,
        }) };
    }
    fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
        STATE.with(|state| f(&mut state.borrow_mut()))
    }
    fn setup(status: Status, output: Option<usize>) -> BootServices {
        with(|state| {
            *state = State {
                status,
                output,
                tpl: Tpl::NOTIFY,
                calls: Vec::new(),
                unaligned_allocation: false,
                free_failures: 0,
            };
        });
        let mut raw = MaybeUninit::<BootServices>::uninit();
        unsafe {
            // Match the existing native_cpu host fixture: every unused service
            // slot has a non-NULL address and must never be invoked.
            for index in 0..size_of::<BootServices>() / size_of::<usize>() {
                raw.as_mut_ptr().cast::<usize>().add(index).write(unused as *const () as usize);
            }
            ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(core::mem::zeroed());
            ptr::addr_of_mut!((*raw.as_mut_ptr()).raise_tpl).write(raise_tpl);
            ptr::addr_of_mut!((*raw.as_mut_ptr()).restore_tpl).write(restore_tpl);
            ptr::addr_of_mut!((*raw.as_mut_ptr()).locate_protocol).write(locate);
            ptr::addr_of_mut!((*raw.as_mut_ptr()).allocate_pool).write(allocate_pool);
            ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pool).write(free_pool);
            raw.assume_init()
        }
    }
    unsafe extern "efiapi" fn unused() {
        panic!("unexpected firmware service in lookup test")
    }
    unsafe extern "efiapi" fn raise_tpl(tpl: Tpl) -> Tpl {
        with(|state| {
            assert_eq!(tpl, Tpl::HIGH_LEVEL);
            state.calls.push("raise");
            let old = state.tpl;
            state.tpl = tpl;
            old
        })
    }
    unsafe extern "efiapi" fn restore_tpl(tpl: Tpl) {
        with(|state| {
            assert_eq!(state.tpl, Tpl::HIGH_LEVEL);
            state.calls.push("restore");
            state.tpl = tpl;
        });
    }
    unsafe extern "efiapi" fn locate(
        guid: *const Guid,
        registration: *mut c_void,
        output: *mut *mut c_void,
    ) -> Status {
        // Independent literal, not the same symbol passed by the helper.
        assert_eq!(unsafe { *guid }, uefi_raw::guid!("f4560cf6-40ec-4b4a-a192-bf1d57d0b189"));
        assert!(registration.is_null());
        assert!(!output.is_null());
        assert!(unsafe { *output }.is_null());
        with(|state| {
            assert!(state.tpl == Tpl::APPLICATION || state.tpl == Tpl::NOTIFY);
            state.calls.push("locate");
            if let Some(address) = state.output {
                unsafe { *output = address as *mut c_void };
            }
            state.status
        })
    }
    unsafe extern "efiapi" fn allocate_pool(
        kind: MemoryType,
        bytes: usize,
        output: *mut *mut u8,
    ) -> Status {
        assert_eq!(kind, MemoryType::BOOT_SERVICES_DATA);
        assert_eq!(bytes, size_of::<TableStorage>());
        assert!(unsafe { *output }.is_null());
        with(|state| {
            state.calls.push("allocate");
            if state.unaligned_allocation {
                // No backing allocation is needed: this sentinel is rejected
                // before writes and the fixture free only records its address.
                unsafe { *output = 1usize as *mut u8 };
                Status::SUCCESS
            } else {
                Status::OUT_OF_RESOURCES
            }
        })
    }
    unsafe extern "efiapi" fn free_pool(pointer: *mut u8) -> Status {
        assert_eq!(pointer.addr(), 1);
        with(|state| {
            state.calls.push("free");
            if state.free_failures != 0 {
                state.free_failures -= 1;
                Status::DEVICE_ERROR
            } else {
                Status::SUCCESS
            }
        })
    }
    unsafe extern "efiapi" fn get_attributes(
        this: *const MemoryAttributeProtocol,
        base: u64,
        bytes: u64,
        attributes: *mut MemoryAttribute,
    ) -> Status {
        assert_eq!(this, &PROTOCOL);
        assert_eq!(base, 0x9000);
        assert_eq!(bytes, 4096);
        assert_eq!(unsafe { *attributes }, MemoryAttribute::empty());
        with(|state| state.calls.push("get"));
        Status::SUCCESS
    }
    unsafe extern "efiapi" fn change_attributes(
        _: *const MemoryAttributeProtocol,
        _: u64,
        _: u64,
        _: MemoryAttribute,
    ) -> Status {
        panic!("lookup must not change attributes")
    }
    static PROTOCOL: MemoryAttributeProtocol = MemoryAttributeProtocol {
        get_memory_attributes: get_attributes,
        set_memory_attributes: change_attributes,
        clear_memory_attributes: change_attributes,
    };
    fn protocol_address() -> usize {
        (&PROTOCOL as *const MemoryAttributeProtocol).addr()
    }
    fn failed_preparation(status: Status, output: Option<usize>) -> TableFailure {
        let services = setup(status, output);
        let failure = unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
            .err()
            .expect("lookup must refuse");
        with(|state| {
            assert_eq!(state.calls, ["raise", "restore", "locate"]);
            assert_eq!(state.tpl, Tpl::NOTIFY);
        });
        assert_eq!(failure.kind, TableError::AttributeProtocol);
        assert_eq!(failure.resource_code() & 0xffff, 0x101);
        failure
    }

    #[test]
    fn actual_efiapi_lookup_accepts_aligned_success_without_allocation_or_query() {
        for tpl in [Tpl::APPLICATION, Tpl::NOTIFY] {
            let services = setup(Status::SUCCESS, Some(protocol_address()));
            with(|state| state.tpl = tpl);
            let pointer = unsafe { acquire_memory_attributes(&services) }.unwrap();
            assert_eq!(pointer.as_ptr().addr(), protocol_address());
            with(|state| assert_eq!(state.calls, ["locate"]));
            assert!(unsafe { current_access(pointer.as_ref(), 0x9123, BorrowedAccess::ReadWrite) });
            with(|state| assert_eq!(state.calls, ["locate", "get"]));
        }
    }

    #[test]
    fn every_standard_error_and_warning_preserves_exact_status_before_pointer_checks() {
        let errors = [
            (Status::LOAD_ERROR, 1),
            (Status::INVALID_PARAMETER, 2),
            (Status::UNSUPPORTED, 3),
            (Status::BAD_BUFFER_SIZE, 4),
            (Status::BUFFER_TOO_SMALL, 5),
            (Status::NOT_READY, 6),
            (Status::DEVICE_ERROR, 7),
            (Status::WRITE_PROTECTED, 8),
            (Status::OUT_OF_RESOURCES, 9),
            (Status::VOLUME_CORRUPTED, 10),
            (Status::VOLUME_FULL, 11),
            (Status::NO_MEDIA, 12),
            (Status::MEDIA_CHANGED, 13),
            (Status::NOT_FOUND, 14),
            (Status::ACCESS_DENIED, 15),
            (Status::NO_RESPONSE, 16),
            (Status::NO_MAPPING, 17),
            (Status::TIMEOUT, 18),
            (Status::NOT_STARTED, 19),
            (Status::ALREADY_STARTED, 20),
            (Status::ABORTED, 21),
            (Status::ICMP_ERROR, 22),
            (Status::TFTP_ERROR, 23),
            (Status::PROTOCOL_ERROR, 24),
            (Status::INCOMPATIBLE_VERSION, 25),
            (Status::SECURITY_VIOLATION, 26),
            (Status::CRC_ERROR, 27),
            (Status::END_OF_MEDIA, 28),
            (Status::END_OF_FILE, 31),
            (Status::INVALID_LANGUAGE, 32),
            (Status::COMPROMISED_DATA, 33),
            (Status::IP_ADDRESS_CONFLICT, 34),
            (Status::HTTP_ERROR, 35),
        ];
        let warnings = [
            (Status::WARN_UNKNOWN_GLYPH, 1),
            (Status::WARN_DELETE_FAILURE, 2),
            (Status::WARN_WRITE_FAILURE, 3),
            (Status::WARN_BUFFER_TOO_SMALL, 4),
            (Status::WARN_STALE_DATA, 5),
            (Status::WARN_FILE_SYSTEM, 6),
            (Status::WARN_RESET_REQUIRED, 7),
        ];
        for (cases, prefix) in [(&errors[..], 0x1800_4101u64), (&warnings[..], 0x1000_4101)] {
            for &(status, code) in cases {
                if cfg!(feature = "memory-attribute-f7") && status == Status::NOT_FOUND {
                    continue; // Exact fallback selection has dedicated tests below.
                }
                // Valid, NULL, unaligned and deliberately unusable aligned
                // outputs must all be ignored when status is non-SUCCESS.
                for output in
                    [None, Some(0), Some(protocol_address()), Some(1), Some(8), Some(usize::MAX)]
                {
                    let failure = failed_preparation(status, output);
                    assert_eq!(failure.lookup, Some(AttributeLookupFailure::NonSuccess(status)));
                    assert_eq!(0x4000 | failure.resource_code(), prefix | (code << 16));
                }
            }
        }
    }

    #[test]
    fn success_null_and_all_unaligned_remainders_are_distinct_refusals() {
        for output in [None, Some(0)] {
            let failure = failed_preparation(Status::SUCCESS, output);
            assert_eq!(failure.lookup, Some(AttributeLookupFailure::SuccessNull));
            assert_eq!(0x4000 | failure.resource_code(), 0x2000_4101);
        }
        for remainder in 1..=7u8 {
            let failure = failed_preparation(
                Status::SUCCESS,
                Some(protocol_address() + usize::from(remainder)),
            );
            assert_eq!(
                failure.lookup,
                Some(AttributeLookupFailure::SuccessUnaligned { remainder })
            );
            assert_eq!(
                0x4000 | failure.resource_code(),
                0x3000_4101 | (u64::from(remainder) << 16)
            );
        }
    }

    #[test]
    fn implementation_status_bits_are_loss_marked_and_cannot_alias_exact_not_found() {
        for error in [0, 1usize << 63] {
            for code in [0usize, 14, 0x3ff] {
                let raw = error | code;
                if raw != 0
                    && !(cfg!(feature = "memory-attribute-f7") && Status(raw) == Status::NOT_FOUND)
                {
                    let exact = failed_preparation(Status(raw), Some(1)).resource_code();
                    assert_eq!(exact & (1 << 26), 0);
                    assert_eq!((exact >> 16) & 0x3ff, code as u64);
                    assert_eq!(exact & (1 << 27) != 0, error != 0);
                }
                for bit in 10..63 {
                    let raw = raw | (1usize << bit);
                    let failure = failed_preparation(Status(raw), Some(protocol_address()));
                    assert_eq!(
                        failure.lookup,
                        Some(AttributeLookupFailure::NonSuccess(Status(raw)))
                    );
                    let encoded = 0x4000 | failure.resource_code();
                    assert!(encoded <= u32::MAX as u64);
                    assert_eq!(encoded >> 28, 1);
                    assert_eq!(encoded & (1 << 26), 1 << 26);
                    assert_eq!((encoded >> 16) & 0x3ff, code as u64);
                    assert_eq!(encoded & (1 << 27) != 0, error != 0);
                    assert_ne!(encoded, 0x180e_4101);
                }
            }
        }
        let incomplete = failed_preparation(Status(0x8000_0000_0000_040e), None);
        assert_eq!(0x4000 | incomplete.resource_code(), 0x1c0e_4101);
        let all_bits = failed_preparation(Status(usize::MAX), Some(1));
        assert_eq!(0x4000 | all_bits.resource_code(), 0x1fff_4101);
    }

    #[test]
    fn legacy_wrappers_keep_kind_and_original_acquisition_family() {
        for (status, output) in [
            (Status::NOT_FOUND, Some(protocol_address())),
            (Status::SUCCESS, None),
            (Status::SUCCESS, Some(1)),
        ] {
            if cfg!(feature = "memory-attribute-f7") && status == Status::NOT_FOUND {
                continue;
            }
            let services = setup(status, output);
            assert_eq!(
                unsafe { prepare(&services, 48, true) }.err(),
                Some(TableError::AttributeProtocol)
            );
            let services = setup(status, output);
            assert_eq!(
                unsafe { prepare_owned_ranges(&services, 48, true, &[]) }.err(),
                Some(TableError::AttributeProtocol)
            );
            let services = setup(status, output);
            assert_eq!(
                unsafe { prepare_resource_ranges(&services, 48, true, &[], &[]) }.err(),
                Some(TableError::AttributeProtocol)
            );
        }
        let legacy = TableFailure::from(TableError::AttributeProtocol);
        assert_eq!(legacy.lookup, None);
        assert_eq!(0x4000 | legacy.resource_code(), 0x0000_4101);
    }

    #[test]
    fn validation_and_tpl_refuse_before_lookup_with_unchanged_codes() {
        let services = setup(Status::NOT_FOUND, None);
        let failure = unsafe {
            prepare_resource_ranges_detailed(
                &services,
                48,
                true,
                &[OwnedRange { base: 1, bytes: 4096 }],
                &[],
            )
        }
        .err()
        .unwrap();
        assert_eq!(failure, TableFailure::from(TableError::OwnedRange));
        assert_eq!(failure.resource_code(), 0x111);
        with(|state| assert!(state.calls.is_empty()));
        let services = setup(Status::NOT_FOUND, None);
        with(|state| state.tpl = Tpl::CALLBACK);
        let failure = unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
            .err()
            .unwrap();
        assert_eq!(failure, TableFailure::from(TableError::EntryTpl));
        assert_eq!(failure.resource_code(), 0x110);
        with(|state| {
            assert_eq!(state.calls, ["raise", "restore"]);
            assert_eq!(state.tpl, Tpl::CALLBACK);
        });
    }

    #[test]
    fn later_allocation_and_cleanup_failure_never_inherit_lookup_tags() {
        let services = setup(Status::SUCCESS, Some(protocol_address()));
        let failure = unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
            .err()
            .unwrap();
        assert_eq!(failure, TableFailure::from(TableError::Allocation));
        assert_eq!(failure.resource_code(), 0x102);
        with(|state| assert_eq!(state.calls, ["raise", "restore", "locate", "allocate"]));
        for free_failures in [0, 1] {
            let services = setup(Status::SUCCESS, Some(protocol_address()));
            with(|state| {
                state.unaligned_allocation = true;
                state.free_failures = free_failures;
            });
            let failure =
                unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
                    .err()
                    .unwrap();
            if free_failures == 0 {
                assert_eq!(failure, TableFailure::from(TableError::Allocation));
                assert_eq!(failure.resource_code(), 0x102);
                with(|state| {
                    assert_eq!(state.calls, ["raise", "restore", "locate", "allocate", "free"])
                });
            } else {
                assert_eq!(failure, TableFailure::from(TableError::Cleanup));
                assert_eq!(failure.resource_code(), 0x10d);
                // The explicit first free failed. A later successful Drop must
                // never turn that observation into reported complete cleanup.
                with(|state| {
                    assert_eq!(
                        state.calls,
                        ["raise", "restore", "locate", "allocate", "free", "free"]
                    )
                });
            }
        }
        let cleanup = TableFailure {
            kind: TableError::Cleanup,
            lookup: Some(AttributeLookupFailure::NonSuccess(Status::NOT_FOUND)),
            fallback_reason: Some(0x1234),
        };
        assert_eq!(cleanup.resource_code(), 0x10d);
    }

    #[test]
    fn source_selection_preserves_firmware_and_only_accepts_exact_missing_status() {
        let interface = NonNull::new(protocol_address() as *mut MemoryAttributeProtocol).unwrap();
        assert_eq!(
            select_attribute_source(Ok(interface)),
            Ok(AttributeSource::Firmware(interface))
        );
        for failure in [
            AttributeLookupFailure::SuccessNull,
            AttributeLookupFailure::SuccessUnaligned { remainder: 1 },
            AttributeLookupFailure::NonSuccess(Status::UNSUPPORTED),
            AttributeLookupFailure::NonSuccess(Status::ACCESS_DENIED),
            AttributeLookupFailure::NonSuccess(Status::DEVICE_ERROR),
            AttributeLookupFailure::NonSuccess(Status(14)),
            AttributeLookupFailure::NonSuccess(Status(Status::NOT_FOUND.0 | 0x400)),
        ] {
            assert_eq!(select_attribute_source(Err(failure)), Err(failure));
        }
        let missing = AttributeLookupFailure::NonSuccess(Status::NOT_FOUND);
        #[cfg(feature = "memory-attribute-f7")]
        assert_eq!(select_attribute_source(Err(missing)), Ok(AttributeSource::F7));
        #[cfg(not(feature = "memory-attribute-f7"))]
        assert_eq!(select_attribute_source(Err(missing)), Err(missing));
    }

    #[test]
    #[cfg(feature = "memory-attribute-f7")]
    fn exact_not_found_fallback_reaches_allocation_without_reading_returned_pointer() {
        for output in [None, Some(0), Some(1), Some(8), Some(usize::MAX), Some(protocol_address())]
        {
            let services = setup(Status::NOT_FOUND, output);
            let failure =
                unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
                    .err()
                    .unwrap();
            assert_eq!(failure, TableFailure::from(TableError::Allocation));
            with(|state| assert_eq!(state.calls, ["raise", "restore", "locate", "allocate"]));
        }
    }

    #[test]
    fn fallback_failures_have_distinct_codes_without_corrupting_lookup_diagnostics() {
        let acquisition = TableFailure::from(TableError::AttributeFallback);
        let reader = TableFailure::from(TableError::AttributeFallbackRead);
        assert_eq!(0x4000 | acquisition.resource_code(), 0x4115);
        assert_eq!(0x4000 | reader.resource_code(), 0x4116);
        assert_eq!(acquisition.lookup, None);
        assert_eq!(reader.lookup, None);
        for reason in [1u16, 16, 24, 0xfffe, 0xffff] {
            for (kind, low) in [
                (TableError::AttributeFallback, 0x4115u64),
                (TableError::AttributeFallbackRead, 0x4116u64),
            ] {
                let detailed = TableFailure::fallback(kind, reason);
                let code = 0x4000 | detailed.resource_code();
                assert_eq!(code & 0xffff, low);
                assert_eq!(code >> 16, u64::from(reason));
                assert_eq!(detailed.lookup, None);
                assert_ne!(code & 0xffff, 0x4101);
            }
        }
        let missing = TableFailure::from(AttributeLookupFailure::NonSuccess(Status::NOT_FOUND));
        assert_eq!(0x4000 | missing.resource_code(), 0x180e4101);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use svmvisor_hypervisor::boot::memory::MemoryDescriptor;

    fn empty() -> RetainedWalks {
        RetainedWalks {
            entries: [EntryObservation { address: 0, value: 0, allowed_set_bits: 0 };
                MAX_RETAINED_ENTRIES],
            entry_count: 0,
            table_pages: [TablePageObservation::default(); MAX_TABLE_PAGES],
            page_count: 0,
        }
    }
    fn config() -> PagingConfig {
        PagingConfig {
            cr3: 0x1000,
            physical_bits: 48,
            la57: false,
            nxe: true,
            pcid: false,
            page1gb: true,
        }
    }
    fn read_large(address: u64) -> Result<u64, TableError> {
        match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3000 => Ok(0x83),
            _ => Err(TableError::ReadPermission),
        }
    }

    fn empty_storage() -> TableStorage {
        TableStorage {
            gdt: [0; MAX_GDT_BYTES],
            gdt_mappings: [LeafObservation::default(); MAX_GDT_PAGES],
            gdt_page_count: 0,
            walks: empty(),
            owned_ranges: [OwnedRange::default(); MAX_OWNED_RANGES],
            owned_range_count: 0,
            owned_mappings: [LeafObservation::default(); MAX_OWNED_PAGES],
            owned_page_count: 0,
            borrowed_spans: [BorrowedSpan::default(); MAX_BORROWED_SPANS + INTERNAL_SPANS],
            borrowed_span_count: 0,
            caller_borrowed_span_count: 0,
            borrowed_mappings: [LeafObservation::default();
                MAX_BORROWED_PAGES + MAX_INTERNAL_PAGES],
            borrowed_page_count: 0,
        }
    }

    fn ram(base: u64, pages: u64) -> MemoryDescriptor {
        MemoryDescriptor { memory_type: 4, physical_start: base, page_count: pages, attributes: 8 }
    }

    fn span(base: u64, bytes: u64, access: BorrowedAccess) -> BorrowedSpan {
        BorrowedSpan { base, bytes, access }
    }

    #[test]
    fn borrowed_exact_extents_rounding_overlap_and_separate_bounds() {
        use BorrowedAccess::*;
        let unaligned = span(0x1fff, 2, Read);
        assert_eq!(covering_pages(unaligned, 48), Ok((0x1000, 2)));
        assert_eq!(validate_borrowed_spans(&[unaligned, unaligned], 48, &[], false), Ok(4));
        for invalid in [
            span(0x1000, 0, Read),
            span(u64::MAX, 1, Read),
            span(1 << 47, 1, Read),
            span((1 << 47) - 1, 2, Read),
            span(1 << 48, 4096, Read),
        ] {
            assert_eq!(covering_pages(invalid, 48), Err(TableError::BorrowedSpan));
        }
        assert_eq!(covering_pages(unaligned, 53), Err(TableError::BorrowedSpan));
        assert_eq!(
            validate_borrowed_spans(&[unaligned; MAX_BORROWED_SPANS + 1], 48, &[], false),
            Err(TableError::Bounds)
        );
        let page_limit = span(0x1000, MAX_BORROWED_PAGES as u64 * 4096, ReadWrite);
        assert_eq!(validate_borrowed_spans(&[page_limit], 48, &[], false), Ok(MAX_BORROWED_PAGES));
        assert_eq!(
            validate_borrowed_spans(
                &[BorrowedSpan { bytes: page_limit.bytes + 1, ..page_limit }],
                48,
                &[],
                false
            ),
            Err(TableError::Bounds)
        );
        let maximum_map = span(0x1001, (1024 * 1024 + 128 * 1024) as u64, ReadWrite);
        let table_pool = span(0x20_0001, size_of::<TableStorage>() as u64, ReadWrite);
        let internal_pages =
            validate_borrowed_spans(&[table_pool, maximum_map], 48, &[], true).unwrap();
        assert!(internal_pages > MAX_OWNED_PAGES && internal_pages <= MAX_INTERNAL_PAGES);
        assert_eq!(
            validate_borrowed_spans(
                &[span(0x1000, (MAX_INTERNAL_PAGES as u64 + 1) * 4096, Read)],
                48,
                &[],
                true
            ),
            Err(TableError::Bounds)
        );
        assert_eq!(
            validate_borrowed_spans(&[unaligned; 3], 48, &[], true),
            Err(TableError::Bounds)
        );
        let owned = [OwnedRange { base: 0x2000, bytes: 4096 }];
        assert_eq!(
            validate_borrowed_spans(&[unaligned], 48, &owned, false),
            Err(TableError::OwnedRange)
        );
        assert_eq!(
            validate_borrowed_spans(
                &[span(0x1fff, 1, Read), span(0x3000, 1, Read)],
                48,
                &owned,
                false
            ),
            Ok(2)
        );
    }

    #[test]
    fn current_attribute_permissions_match_each_required_access() {
        use BorrowedAccess::*;
        for access in [Read, ReadWrite, ReadExecute] {
            assert!(attributes_allow(Status::SUCCESS, MemoryAttribute::empty(), access));
            assert!(!attributes_allow(Status::UNSUPPORTED, MemoryAttribute::empty(), access));
            assert!(!attributes_allow(Status::SUCCESS, MemoryAttribute::READ_PROTECT, access));
        }
        let ro = MemoryAttribute::from_bits_retain(0x20000);
        assert!(attributes_allow(Status::SUCCESS, ro, Read));
        assert!(!attributes_allow(Status::SUCCESS, ro, ReadWrite));
        assert!(attributes_allow(Status::SUCCESS, ro, ReadExecute));
        let xp = MemoryAttribute::EXECUTE_PROTECT;
        assert!(attributes_allow(Status::SUCCESS, xp, Read));
        assert!(attributes_allow(Status::SUCCESS, xp, ReadWrite));
        assert!(!attributes_allow(Status::SUCCESS, xp, ReadExecute));
    }

    #[test]
    fn internal_query_failures_and_real_denials_remain_distinct_and_block_source_loads() {
        let descriptors = [ram(0x1000, 1)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let failed = core::cell::Cell::new(false);
        for query in [Ok(0x2000u64), Err(())] {
            assert_eq!(
                read_checked_entry(
                    &memory,
                    0x1000,
                    |_| internal_access(query, BorrowedAccess::Read, &failed),
                    |_| panic!("failed internal permission must precede direct read"),
                ),
                Err(TableError::ReadPermission),
            );
            assert_eq!(failed.get(), query.is_err());
        }
        let failed = core::cell::Cell::new(false);
        assert!(internal_access(Ok::<u64, ()>(0x20000), BorrowedAccess::Read, &failed));
        assert!(!internal_access(Ok::<u64, ()>(0x20000), BorrowedAccess::ReadWrite, &failed));
        assert!(!internal_access(Ok::<u64, ()>(0x4000), BorrowedAccess::ReadExecute, &failed));
        assert!(!failed.get());
    }

    #[test]
    fn borrowed_metadata_and_current_denial_precede_all_operand_walks() {
        use BorrowedAccess::*;
        let descriptors = [ram(0x1000, 1)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let mut storage = empty_storage();
        assert_eq!(
            retain_borrowed_mappings(
                &mut storage,
                &[span(0x1fff, 2, Read)],
                &[],
                &memory,
                config(),
                false,
                false,
                &mut |_, _| panic!("metadata denial must precede current query"),
                &mut |_| panic!("metadata denial must precede table read")
            ),
            Err(TableError::Metadata)
        );
        assert_eq!(
            retain_borrowed_mappings(
                &mut storage,
                &[span(0x1001, 1, Read)],
                &[],
                &memory,
                config(),
                false,
                false,
                &mut |page, access| {
                    assert_eq!((page, access), (0x1000, Read));
                    false
                },
                &mut |_| panic!("current denial must precede table read")
            ),
            Err(TableError::ReadPermission)
        );
        assert_eq!(storage.walks.entry_count, 0);
    }

    #[test]
    fn borrowed_access_requires_effective_rw_nx_and_actual_smep_user_compatibility() {
        use BorrowedAccess::*;
        let descriptors = [ram(0, 512)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        for (access, smep, ro, nx, user, expected) in [
            (Read, true, true, true, true, Ok(())),
            (ReadWrite, false, true, false, false, Err(TableError::NotWritable)),
            (ReadExecute, false, true, false, false, Ok(())),
            (ReadExecute, false, false, true, false, Err(TableError::NotExecutable)),
            (ReadExecute, true, false, false, true, Err(TableError::NotExecutable)),
            (ReadExecute, false, false, false, true, Ok(())),
            (ReadExecute, true, false, false, false, Ok(())),
        ] {
            let mut storage = empty_storage();
            assert_eq!(
                retain_borrowed_mappings(
                    &mut storage,
                    &[span(0x9001, 4096, access)],
                    &[],
                    &memory,
                    config(),
                    smep,
                    false,
                    &mut |_, required| {
                        assert_eq!(required, access);
                        true
                    },
                    &mut |address| read_large(address).map(|mut value| {
                        if ro && address == 0x1000 {
                            value &= !2;
                        }
                        if nx && address == 0x2000 {
                            value |= 1 << 63;
                        }
                        if user {
                            value |= 4;
                        }
                        value
                    })
                ),
                expected
            );
        }
        let mut storage = empty_storage();
        assert_eq!(
            retain_borrowed_mappings(
                &mut storage,
                &[span(0x9001, 1, Read)],
                &[],
                &memory,
                config(),
                false,
                false,
                &mut |_, _| true,
                &mut |address| read_large(address).map(|value| if address == 0x3000 {
                    value | 0x20_0000
                } else {
                    value
                })
            ),
            Err(TableError::NonIdentity)
        );
    }

    #[test]
    fn overlapping_spans_retain_exact_bytes_access_and_duplicate_actual_leaves() {
        use BorrowedAccess::*;
        let descriptors = [ram(0, 512)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let requested =
            [span(0x8fff, 2, Read), span(0x9000, 2, ReadExecute), span(0x9001, 1, ReadWrite)];
        let mut queried = 0;
        let mut storage = empty_storage();
        retain_borrowed_mappings(
            &mut storage,
            &requested,
            &[],
            &memory,
            config(),
            true,
            false,
            &mut |_, _| {
                queried += 1;
                true
            },
            &mut read_large,
        )
        .unwrap();
        assert_eq!(queried, 4);
        assert_eq!(&storage.borrowed_spans[..3], &requested);
        assert_eq!(storage.caller_borrowed_span_count, 3);
        assert_eq!(storage.borrowed_page_count, 4);
        for (index, page) in [0x8000, 0x9000, 0x9000, 0x9000].iter().enumerate() {
            assert_eq!(
                storage.borrowed_mappings[index],
                LeafObservation {
                    physical_page: *page,
                    leaf_physical_base: 0,
                    leaf_bytes: 0x20_0000,
                    pat_index: 0,
                }
            );
        }
        storage.walks.close_dependencies(config(), &mut read_large).unwrap();
        assert_eq!(storage.walks.compare(|address| read_large(address).unwrap()), Ok(()));
        assert_eq!(
            storage.walks.compare(
                |address| read_large(address).unwrap() ^ if address == 0x3000 { 8 } else { 0 }
            ),
            Err(TableError::Changed)
        );
    }

    #[test]
    fn internal_pool_pages_have_separate_capacity_and_legacy_pcid_compatibility() {
        use BorrowedAccess::*;
        let descriptors = [ram(0, 2048)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let mut storage = empty_storage();
        let requested = [span(0x1000, MAX_BORROWED_PAGES as u64 * 4096, Read)];
        let internal = [
            span(0x50_0001, (1024 * 1024 + 128 * 1024) as u64, ReadWrite),
            span(0x70_0001, size_of::<TableStorage>() as u64, ReadWrite),
        ];
        let mut reader = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x83),
            _ => Err(TableError::ReadPermission),
        };
        retain_borrowed_mappings(
            &mut storage,
            &requested,
            &[],
            &memory,
            config(),
            false,
            false,
            &mut |_, _| true,
            &mut reader,
        )
        .unwrap();
        retain_borrowed_mappings(
            &mut storage,
            &internal,
            &[],
            &memory,
            config(),
            false,
            true,
            &mut |_, _| true,
            &mut reader,
        )
        .unwrap();
        assert_eq!(storage.borrowed_span_count, 3);
        assert_eq!(storage.caller_borrowed_span_count, 1);
        assert!(storage.borrowed_page_count > MAX_BORROWED_PAGES + MAX_OWNED_PAGES);
        assert_eq!(&storage.borrowed_spans[1..3], &internal);
        let mut storage = empty_storage();
        let pcid = PagingConfig { pcid: true, cr3: 0x1007, ..config() };
        assert_eq!(
            retain_borrowed_mappings(
                &mut storage,
                &requested,
                &[],
                &memory,
                pcid,
                false,
                false,
                &mut |_, _| panic!("PCID refuses before query"),
                &mut |_| panic!("PCID refuses before read")
            ),
            Err(TableError::TableFetch)
        );
        retain_borrowed_mappings(
            &mut storage,
            &internal,
            &[],
            &memory,
            pcid,
            false,
            true,
            &mut |_, _| true,
            &mut reader,
        )
        .unwrap();
        assert_eq!(storage.walks.table_pages[0].fetch_pat_indices, 0);
    }

    #[test]
    fn borrowed_fragmented_layout_refuses_entry_exhaustion_without_truncating_success() {
        let descriptors = [ram(0, 1024)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let mut storage = empty_storage();
        let mut reads = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3008 => Ok(0x4003),
            0x4000..=0x47f8 if address & 7 == 0 => Ok(0x20_0003 + ((address - 0x4000) / 8) * 4096),
            _ => Err(TableError::ReadPermission),
        };
        assert_eq!(
            retain_borrowed_mappings(
                &mut storage,
                &[span(0x20_0000, 256 * 4096, BorrowedAccess::Read)],
                &[],
                &memory,
                config(),
                false,
                false,
                &mut |_, _| true,
                &mut reads
            ),
            Err(TableError::Bounds)
        );
        assert_eq!(storage.walks.entry_count, MAX_RETAINED_ENTRIES);
        assert_eq!(storage.borrowed_span_count, 0);
        assert_eq!(storage.borrowed_page_count, 0);
    }

    #[test]
    fn settlement_rejects_dirty_on_nonleaf_slots_and_accepts_valid_leaf_sizes() {
        for nonleaf in [0x1000, 0x2000] {
            let mut retained = empty();
            retained.translate(config(), 0, &mut read_large).unwrap();
            assert_eq!(
                retained.settle_accessed_dirty(|address| read_large(address).unwrap()
                    | if address == nonleaf { 0x40 } else { 0 }),
                Err(TableError::Changed)
            );
        }
        for leaf_level in [1u8, 2, 3] {
            let leaf_address = (5 - u64::from(leaf_level)) * 4096;
            let mut reader = |address| {
                if address == leaf_address {
                    Ok(0x83)
                } else if address < leaf_address {
                    Ok(address + 4096 + 3)
                } else {
                    Err(TableError::ReadPermission)
                }
            };
            let mut retained = empty();
            let translated = retained.translate(config(), 0, &mut reader).unwrap();
            assert_eq!(translated.page_bytes, 1u64 << (12 + 9 * (leaf_level - 1)));
            assert_eq!(translated.pat_index, if leaf_level == 1 { 4 } else { 0 });
            retained
                .settle_accessed_dirty(|address| {
                    reader(address).unwrap() | if address == leaf_address { 0x60 } else { 0x20 }
                })
                .unwrap();
        }
    }

    #[test]
    fn settlement_unions_validated_roles_per_slot_and_rejects_unvalidated_roles() {
        let mut retained = empty();
        // The same slot serves every level of this recursive supplied walk.
        retained.translate(config(), 0, &mut |_| Ok(0x1003)).unwrap();
        assert_eq!(retained.entry_count, 1);
        assert_eq!(retained.entries[0].allowed_set_bits, 0x60);
        retained.settle_accessed_dirty(|_| 0x1063).unwrap();
        assert_eq!(retained.compare(|_| 0x1023), Err(TableError::Changed));
        let mut invalid = empty();
        assert_eq!(
            invalid.translate(config(), 0, &mut |_| Ok(0x1083)),
            Err(TableError::Translation)
        );
        assert_eq!(invalid.entries[0].allowed_set_bits, 0);
        assert_eq!(invalid.settle_accessed_dirty(|_| 0x10a3), Err(TableError::Changed));
    }

    #[test]
    fn owned_extent_shape_bounds_and_overlap_are_checked_without_reads() {
        assert_eq!(validate_owned_ranges(&[], 0), Ok(0)); // Legacy path unchanged.
        let page = OwnedRange { base: 0x20_0000, bytes: 4096 };
        assert_eq!(validate_owned_ranges(&[page], 48), Ok(1));
        assert_eq!(validate_owned_ranges(&[page; 9], 48), Err(TableError::Bounds));
        assert_eq!(validate_owned_ranges(&[page; 2], 48), Err(TableError::OwnedRange));
        for invalid in [
            OwnedRange { bytes: 0, ..page },
            OwnedRange { base: page.base + 1, ..page },
            OwnedRange { bytes: 4097, ..page },
            OwnedRange { base: u64::MAX & !4095, ..page },
            OwnedRange { base: 1 << 48, ..page },
            OwnedRange { base: 1 << 47, ..page },
        ] {
            assert_eq!(validate_owned_ranges(&[invalid], 48), Err(TableError::OwnedRange));
        }
        assert_eq!(validate_owned_ranges(&[page], 53), Err(TableError::OwnedRange));
        assert_eq!(
            validate_owned_ranges(&[OwnedRange { bytes: 257 * 4096, ..page }], 48),
            Err(TableError::Bounds)
        );
        assert_eq!(
            validate_owned_ranges(
                &[
                    OwnedRange { bytes: 255 * 4096, ..page },
                    OwnedRange { base: 0x40_0000, bytes: 4096 },
                ],
                48
            ),
            Ok(256)
        );
        assert_eq!(
            validate_owned_ranges(
                &[
                    OwnedRange { bytes: 256 * 4096, ..page },
                    OwnedRange { base: 0x40_0000, bytes: 4096 },
                ],
                48
            ),
            Err(TableError::Bounds)
        );
        assert_eq!(
            validate_owned_ranges(&[page, OwnedRange { base: page.base + 4096, ..page }], 48),
            Ok(2)
        );
    }

    #[test]
    fn owned_metadata_holes_and_unallocated_or_non_wb_ram_never_reach_readers() {
        let range = OwnedRange { base: 0x20_0000, bytes: 8192 };
        for descriptor in [
            ram(range.base, 1),
            MemoryDescriptor { memory_type: 7, ..ram(range.base, 2) },
            MemoryDescriptor { memory_type: 11, ..ram(range.base, 2) },
            MemoryDescriptor { attributes: 0x2008, ..ram(range.base, 2) },
            MemoryDescriptor { attributes: 1, ..ram(range.base, 2) },
        ] {
            let descriptors = [descriptor];
            let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
            let mut storage = empty_storage();
            assert_eq!(
                retain_owned_mappings(
                    &mut storage,
                    &[range],
                    &memory,
                    config(),
                    &mut |_| panic!("metadata rejection must precede current permission"),
                    &mut |_| panic!("metadata rejection must precede table reads")
                ),
                Err(TableError::Metadata)
            );
            assert_eq!(storage.walks.entry_count, 0);
        }
    }

    #[test]
    fn entry_metadata_and_real_query_failure_stop_before_the_source_load() {
        let descriptors = [ram(0x1000, 1)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        for address in [0, 0x1001, 0x2000, 1 << 47] {
            assert_eq!(
                read_checked_entry(
                    &memory,
                    address,
                    |_| panic!("bad metadata reached permission query"),
                    |_| panic!("bad metadata reached source load")
                ),
                Err(TableError::ReadPermission)
            );
        }
        assert_eq!(
            read_checked_entry(&memory, 0x1000, |_| false, |_| panic!("denied source was read")),
            Err(TableError::ReadPermission)
        );
        assert_eq!(read_checked_entry(&memory, 0x1ff8, |_| true, |_| 0x2003), Ok(0x2003));
    }

    #[test]
    fn owned_pages_require_current_write_permission_identity_and_every_ancestor_rw() {
        let descriptors = [ram(0, 2048)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let ranges = [OwnedRange { base: 0x20_0000, bytes: 4096 }];
        assert_eq!(
            retain_owned_mappings(
                &mut empty_storage(),
                &ranges,
                &memory,
                config(),
                &mut |_| false,
                &mut |_| panic!("denied page reached walker")
            ),
            Err(TableError::ReadPermission)
        );
        for (root, leaf, error) in [
            (0x2001, 0x20_0083, TableError::NotWritable),
            (0x2003, 0x20_0081, TableError::NotWritable),
            (0x2003, 0x40_0083, TableError::NonIdentity),
        ] {
            let mut read = |address| match address {
                0x1000 => Ok(root),
                0x2000 => Ok(0x3003),
                0x3008 => Ok(leaf),
                _ => Err(TableError::ReadPermission),
            };
            assert_eq!(
                retain_owned_mappings(
                    &mut empty_storage(),
                    &ranges,
                    &memory,
                    config(),
                    &mut |_| true,
                    &mut read
                ),
                Err(error)
            );
        }
        assert_eq!(
            retain_owned_mappings(
                &mut empty_storage(),
                &ranges,
                &memory,
                PagingConfig { pcid: true, ..config() },
                &mut |_| panic!("PCID refusal must precede query"),
                &mut |_| panic!("PCID refusal must precede walk")
            ),
            Err(TableError::TableFetch)
        );
    }

    #[test]
    fn retains_33_owned_pages_and_separates_parent_fetch_indices_from_leaf_pat() {
        let descriptors = [ram(0, 2048)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let ranges = [OwnedRange { base: 0x20_0000, bytes: 33 * 4096 }];
        let config = PagingConfig { cr3: 0x1018, ..config() };
        let mut read = |address| match address {
            0x1000 => Ok(0x200b),    // PDPT fetched with PAT1.
            0x2000 => Ok(0x3013),    // PD fetched with PAT2.
            0x3000 => Ok(0x1083),    // Software aliases of tables select PAT4.
            0x3008 => Ok(0x20_109b), // Arena leaf selects PAT7, not table-fetch PAT.
            _ => Err(TableError::ReadPermission),
        };
        let mut storage = empty_storage();
        storage.walks.close_dependencies(config, &mut read).unwrap();
        assert_eq!(storage.walks.entry_count, 3);
        retain_owned_mappings(&mut storage, &ranges, &memory, config, &mut |_| true, &mut read)
            .unwrap();
        storage.walks.close_dependencies(config, &mut read).unwrap();
        assert_eq!(storage.owned_page_count, 33);
        assert_eq!(storage.walks.entry_count, 4);
        assert_eq!(storage.walks.page_count, 3);
        for (index, page) in storage.owned_mappings[..33].iter().enumerate() {
            assert_eq!(
                *page,
                LeafObservation {
                    physical_page: ranges[0].base + index as u64 * 4096,
                    leaf_physical_base: 0x20_0000,
                    leaf_bytes: 0x20_0000,
                    pat_index: 7
                }
            );
        }
        for (page, expected_fetch, expected_level) in
            [(0x1000, 1 << 3, 1 << 3), (0x2000, 1 << 1, 1 << 2), (0x3000, 1 << 2, 1 << 1)]
        {
            let observed = storage
                .walks
                .pages()
                .unwrap()
                .iter()
                .find(|entry| entry.physical_page == page)
                .unwrap();
            assert_eq!(observed.fetch_pat_indices, expected_fetch);
            assert_eq!(observed.levels, expected_level);
            assert_eq!(
                observed.alias,
                LeafObservation {
                    physical_page: page,
                    leaf_physical_base: 0,
                    leaf_bytes: 0x20_0000,
                    pat_index: 4
                }
            );
        }
        assert_eq!(
            storage
                .walks
                .compare(|address| read(address).unwrap() ^ if address == 0x3008 { 8 } else { 0 }),
            Err(TableError::Changed)
        );
    }

    #[test]
    fn owned_4k_walk_discovers_new_table_page_and_closes_its_software_alias() {
        let descriptors = [ram(0, 2048)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let ranges = [OwnedRange { base: 0x20_0000, bytes: 8192 }];
        let mut read = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3000 => Ok(0x83),
            0x3008 => Ok(0x401b),    // PT fetch index3, RW inherited.
            0x4000 => Ok(0x20_008b), // Actual 4K leaf PAT5.
            0x4008 => Ok(0x20_1003), // Actual next 4K leaf PAT0.
            _ => Err(TableError::ReadPermission),
        };
        let mut storage = empty_storage();
        storage.walks.close_dependencies(config(), &mut read).unwrap();
        retain_owned_mappings(&mut storage, &ranges, &memory, config(), &mut |_| true, &mut read)
            .unwrap();
        storage.walks.close_dependencies(config(), &mut read).unwrap();
        assert_eq!(storage.walks.entry_count, 6);
        assert_eq!(storage.walks.page_count, 4);
        assert_eq!(
            storage.owned_mappings[0],
            LeafObservation {
                physical_page: 0x20_0000,
                leaf_physical_base: 0x20_0000,
                leaf_bytes: 4096,
                pat_index: 5
            }
        );
        assert_eq!(
            storage.owned_mappings[1],
            LeafObservation {
                physical_page: 0x20_1000,
                leaf_physical_base: 0x20_1000,
                leaf_bytes: 4096,
                pat_index: 0
            }
        );
        let pt = storage
            .walks
            .pages()
            .unwrap()
            .iter()
            .find(|entry| entry.physical_page == 0x4000)
            .unwrap();
        assert_eq!(pt.alias.physical_page, 0x4000);
        assert_eq!(pt.alias.leaf_bytes, 0x20_0000);
        assert_eq!(pt.fetch_pat_indices, 1 << 3);
        assert_eq!(pt.levels, 1);
        assert_eq!(
            storage.walks.compare(
                |address| read(address).unwrap() ^ if address == 0x4008 { 0x1000 } else { 0 }
            ),
            Err(TableError::Changed)
        );
    }

    #[test]
    fn retains_full_capacity_across_disjoint_ranges_in_actual_one_gib_leaves() {
        let descriptors = [ram(0, 0xc_0000)];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        // Deliberately supplied out of physical order. Each allocation's own
        // view remains ascending, with full 1 GiB leaf bases preserved.
        let ranges = [
            OwnedRange { base: 0x8000_3000, bytes: 128 * 4096 },
            OwnedRange { base: 0x4000_5000, bytes: 128 * 4096 },
        ];
        let mut read = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3000 => Ok(0x83),
            0x2008 => Ok(0x4000_1083), // 1 GiB leaf, PAT4.
            0x2010 => Ok(0x8000_0093), // 1 GiB leaf, PAT2.
            _ => Err(TableError::ReadPermission),
        };
        let mut storage = empty_storage();
        retain_owned_mappings(&mut storage, &ranges, &memory, config(), &mut |_| true, &mut read)
            .unwrap();
        storage.walks.close_dependencies(config(), &mut read).unwrap();
        assert_eq!(storage.owned_page_count, MAX_OWNED_PAGES);
        assert_eq!(storage.owned_ranges[..2], ranges);
        assert_eq!(
            storage.owned_mappings[127],
            LeafObservation {
                physical_page: ranges[0].base + 127 * 4096,
                leaf_physical_base: 0x8000_0000,
                leaf_bytes: 0x4000_0000,
                pat_index: 2
            }
        );
        assert_eq!(
            storage.owned_mappings[255],
            LeafObservation {
                physical_page: ranges[1].base + 127 * 4096,
                leaf_physical_base: 0x4000_0000,
                leaf_bytes: 0x4000_0000,
                pat_index: 4
            }
        );
        assert_eq!(storage.walks.entry_count, 5);
    }

    #[test]
    fn observed_fetch_alias_roles_are_accumulated_and_invalid_roles_refuse() {
        let mut walks = empty();
        walks.remember_fetch(0x1000, 4, Some(0)).unwrap();
        walks.remember_fetch(0x1000, 1, Some(3)).unwrap();
        assert_eq!(walks.pages().unwrap()[0].fetch_pat_indices, 9);
        assert_eq!(walks.pages().unwrap()[0].levels, 9);
        for (level, index) in [(0, 0), (5, 0), (4, 4), (4, 255)] {
            assert_eq!(walks.remember_fetch(0x2000, level, Some(index)), Err(TableError::Bounds));
        }
        assert_eq!(walks.page_count, 1);
    }

    #[test]
    fn owned_ranges_must_not_overlap_any_borrowed_gdt_byte_or_table_page() {
        let ranges = [OwnedRange { base: 0x20_0000, bytes: 8192 }];
        for (base, bytes) in [(0x1f_ffff, 2), (0x20_1000, 4096), (0x20_1fff, 1)] {
            assert_eq!(reject_owned_overlap(&ranges, base, bytes), Err(TableError::OwnedRange));
        }
        assert_eq!(reject_owned_overlap(&ranges, 0x1f_f000, 4096), Ok(()));
        assert_eq!(reject_owned_overlap(&ranges, 0x20_2000, 4096), Ok(()));
    }

    #[test]
    fn closes_root_and_self_mapping_without_recursion_or_duplicate_entries() {
        let mut retained = empty();
        assert_eq!(
            retained.translate(config(), 0x8000, &mut read_large).unwrap().physical_address,
            0x8000
        );
        assert_eq!(retained.entry_count, 3);
        retained.close_dependencies(config(), &mut read_large).unwrap();
        assert_eq!(retained.entry_count, 3);
        assert_eq!(retained.page_count, 3);
        retained.compare(|address| read_large(address).unwrap()).unwrap();
    }

    #[test]
    fn closes_dependency_pages_newly_discovered_by_root_mapping() {
        // A 4 KiB mapping of the GDT at 0x9000 uses one PT entry. Checking
        // identity access to the root and table-source pages adds four others.
        let mut retained = empty();
        let mut read = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3000 => Ok(0x4003),
            0x4008 => Ok(0x1003),
            0x4010 => Ok(0x2003),
            0x4018 => Ok(0x3003),
            0x4020 => Ok(0x4003),
            0x4048 => Ok(0x9003),
            _ => Err(TableError::ReadPermission),
        };
        retained.translate(config(), 0x9000, &mut read).unwrap();
        assert_eq!(retained.entry_count, 4);
        retained.close_dependencies(config(), &mut read).unwrap();
        assert_eq!(retained.entry_count, 8);
        assert_eq!(retained.page_count, 4);
        assert!(retained.entries().unwrap().iter().any(|entry| entry.address == 0x4008));
    }

    #[test]
    fn refuses_unmapped_root_even_when_gdt_mapping_is_present() {
        let mut retained = empty();
        let mut read = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3000 => Ok(0x4003),
            0x4048 => Ok(0x9003),
            0x4008 => Ok(0),
            _ => Err(TableError::ReadPermission),
        };
        retained.translate(config(), 0x9000, &mut read).unwrap();
        assert_eq!(retained.close_dependencies(config(), &mut read), Err(TableError::Translation));
    }

    #[test]
    fn refuses_nonidentity_root_even_when_gdt_mapping_is_identity() {
        let mut retained = empty();
        let mut read = |address| match address {
            0x1000 => Ok(0x2003),
            0x2000 => Ok(0x3003),
            0x3000 => Ok(0x4003),
            0x4048 => Ok(0x9003),
            0x4008 => Ok(0x5003),
            _ => Err(TableError::ReadPermission),
        };
        retained.translate(config(), 0x9000, &mut read).unwrap();
        assert_eq!(retained.close_dependencies(config(), &mut read), Err(TableError::NonIdentity));
    }

    #[test]
    fn unavailable_current_permission_stops_before_recording_an_entry() {
        let mut retained = empty();
        assert_eq!(
            retained.translate(config(), 0x9000, &mut |_| Err(TableError::ReadPermission)),
            Err(TableError::ReadPermission)
        );
        assert_eq!(retained.entry_count, 0);
    }

    #[test]
    fn rejects_drift_on_repeated_walk_including_accessed_and_dirty_bits() {
        for bit in [0, 1, 5, 6, 12, 63] {
            let mut retained = empty();
            retained.translate(config(), 0x9000, &mut read_large).unwrap();
            assert_eq!(
                retained.translate(config(), 0x9000, &mut |address| read_large(address)
                    .map(|entry| if address == 0x1000 { entry ^ (1 << bit) } else { entry })),
                Err(TableError::Changed)
            );
        }
    }

    #[test]
    fn comparison_never_follows_a_changed_entry() {
        let mut retained = empty();
        retained.translate(config(), 0x9000, &mut read_large).unwrap();
        let mut reads = 0;
        assert_eq!(
            retained.compare(|address| {
                reads += 1;
                assert_eq!(address, 0x1000);
                0xdead_0003
            }),
            Err(TableError::Changed)
        );
        assert_eq!(reads, 1);
    }

    #[test]
    fn final_baseline_allows_only_setting_accessed_dirty_then_requires_exact_equality() {
        let mut retained = empty();
        retained.translate(config(), 0x9000, &mut read_large).unwrap();
        retained
            .settle_accessed_dirty(|address| {
                read_large(address).unwrap() | if address == 0x3000 { 0x60 } else { 0x20 }
            })
            .unwrap();
        retained
            .compare(|address| {
                read_large(address).unwrap() | if address == 0x3000 { 0x60 } else { 0x20 }
            })
            .unwrap();
        assert_eq!(
            retained.compare(|address| read_large(address).unwrap()),
            Err(TableError::Changed)
        );
        assert_eq!(
            retained.settle_accessed_dirty(|address| read_large(address).unwrap()),
            Err(TableError::Changed)
        );
        for changed in [1, 2, 4, 0x1000, 1 << 63] {
            let mut retained = empty();
            retained.translate(config(), 0x9000, &mut read_large).unwrap();
            assert_eq!(
                retained.settle_accessed_dirty(|address| read_large(address).unwrap() ^ changed),
                Err(TableError::Changed)
            );
        }
    }

    #[test]
    fn bounds_exhaustion_and_noncanonical_pages_are_explicit_refusals() {
        let mut entries = empty();
        for index in 0..MAX_RETAINED_ENTRIES {
            entries.remember_entry(0x1000 + index as u64 * 8, 1).unwrap();
        }
        assert_eq!(
            entries.remember_entry(0x1000 + MAX_RETAINED_ENTRIES as u64 * 8, 1),
            Err(TableError::Bounds)
        );
        let mut pages = empty();
        for index in 0..MAX_TABLE_PAGES {
            pages.remember_page(index as u64 * 4096).unwrap();
        }
        assert_eq!(pages.remember_page(MAX_TABLE_PAGES as u64 * 4096), Err(TableError::Bounds));
        assert_eq!(empty().remember_page(0x0000_8000_0000_0000), Err(TableError::Context));
        assert_eq!(empty().remember_entry(0x1001, 1), Err(TableError::Metadata));
    }

    #[test]
    fn high_tpl_requires_if_clear_but_allows_the_expected_if_transition() {
        let before = NativeSnapshot { rflags: 0x202, ..NativeSnapshot::default() };
        let mut after = before;
        after.rflags = 2;
        assert_eq!(context_unchanged(&before, 0x500, &after, 0x500, true), Ok(()));
        assert_eq!(
            context_unchanged(&before, 0x500, &after, 0x500, false),
            Err(TableError::Changed)
        );
        assert_eq!(
            context_unchanged(&before, 0x500, &before, 0x500, true),
            Err(TableError::Changed)
        );
        after.rflags |= 0x400;
        assert_eq!(
            context_unchanged(&before, 0x500, &after, 0x500, true),
            Err(TableError::Changed)
        );
    }

    #[test]
    fn detects_control_table_selector_and_efer_drift_before_live_reads() {
        let before = NativeSnapshot::default();
        for field in 0..10 {
            let mut after = before;
            match field {
                0 => after.cr0 = 1,
                1 => after.cr3 = 1,
                2 => after.cr4 = 1,
                3 => after.gdtr.bytes[0] = 1,
                4 => after.idtr.bytes[2] = 1,
                5 => after.cs = 1,
                6 => after.ss = 1,
                7 => after.ds = 1,
                8 => after.es = 1,
                _ => after.rflags = 0x400,
            }
            assert_eq!(
                context_unchanged(&before, 0x500, &after, 0x500, true),
                Err(TableError::Changed)
            );
        }
        assert_eq!(
            context_unchanged(&before, 0x500, &before, 0xd00, true),
            Err(TableError::Changed)
        );
    }
}
