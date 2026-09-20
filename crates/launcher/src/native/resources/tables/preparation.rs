//! Preparation entry points and the privileged in-place initialization.

use core::{
    marker::PhantomData,
    mem::size_of,
    ptr::{self, NonNull},
};

use svmvisor_dxe::native::admission::{memory as native_memory, snapshot::NativeSnapshot};
#[cfg(feature = "memory-attribute-f7")]
use svmvisor_hypervisor::memory::address::ADDRESS_MASK;
use svmvisor_hypervisor::{
    boot::{
        descriptors::{CapturedGdtPage, FirmwareSelectors, parse_firmware_gdt},
        memory::ValidatedMemoryMap,
    },
    host::{descriptors::HostTablePointer, paging::PagingConfig},
    memory::address::is_canonical_48,
};
use uefi_raw::{
    Status,
    table::boot::{BootServices, MemoryType, Tpl},
};

#[cfg(feature = "memory-attribute-f7")]
use super::attribute::internal_access;
#[cfg(target_os = "uefi")]
use super::read_efer;
use super::{
    BorrowedAccess, BorrowedSpan, LeafObservation, MAX_GDT_PAGES, OwnedRange, PreparedTables,
    TableError, TableFailure, TableReport, TableStorage,
    attribute::{
        AttributeSource, acquire_memory_attributes, current_access, select_attribute_source,
    },
    mapping::{
        read_checked_entry, reject_owned_overlap, retain_borrowed_mappings, retain_owned_mappings,
        validate_borrowed_spans, validate_owned_ranges,
    },
};

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
                        root: config.cr3 & ADDRESS_MASK,
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
