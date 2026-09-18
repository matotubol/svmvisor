//! Native allocation and retained mapping preparation shared with the returning
//! caller. The observation-only feature reports checks without entering SVM.
//! The returning image requires the exact compiled stack audit for its finite
//! window and retains the documented cooperating-firmware lifetime contracts.

use core::ptr;

use svmvisor_dxe::native::admission::{
    boundary::NativeBoundary, cache_rendezvous::PreparedCacheRendezvous,
};
#[cfg(not(feature = "native-returning"))]
use svmvisor_dxe::native::admission::{
    cache_rendezvous as native_cache_rendezvous, cpu as native_cpu,
};
#[cfg(not(feature = "native-returning"))]
use uefi_raw::table::system::SystemTable;
use uefi_raw::{
    Handle, Status,
    table::boot::{AllocateType, BootServices, MemoryType},
};

#[cfg(not(feature = "native-returning"))]
use crate::native_resource_cache::{self, ResourceCacheReport};
use crate::{
    native_image_resources,
    native_tables::{self, BorrowedAccess, BorrowedSpan, OwnedRange, PreparedTables},
};

pub const ARENA_PAGES: usize = 33;
const ARENA_BYTES: u64 = (ARENA_PAGES * 4096) as u64;
#[cfg(feature = "native-returning")]
const _: () = assert!(ARENA_PAGES == crate::native_guest_resources::ARENA_PAGES);

pub(crate) struct Arena<'a> {
    services: &'a BootServices,
    base: u64,
    owned: bool,
}

impl Arena<'_> {
    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn release(&mut self) -> Result<(), Status> {
        if self.owned {
            let status = unsafe { (self.services.free_pages)(self.base, ARENA_PAGES) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.owned = false;
        }
        Ok(())
    }
}

impl Drop for Arena<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub(crate) struct Prepared<'a> {
    pub arena: Option<Arena<'a>>,
    pub cache: PreparedCacheRendezvous<'a>,
    pub tables: Option<PreparedTables<'a>>,
    #[cfg(feature = "native-returning")]
    pub guest: Option<crate::native_guest_resources::BoundGuest>,
}

impl Prepared<'_> {
    pub fn release(&mut self) -> Result<(), u64> {
        let tables = self.tables.as_mut().map_or(Ok(()), |tables| tables.release().map_err(|_| ()));
        let arena = self.arena.as_mut().map_or(Ok(()), |arena| arena.release().map_err(|_| ()));
        let cache = self.cache.release().map_err(|_| ());
        if tables.is_err() || arena.is_err() || cache.is_err() { Err(13) } else { Ok(()) }
    }
}

#[cfg(not(feature = "native-returning"))]
pub(crate) unsafe fn observe(
    image: Handle,
    table: &SystemTable,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
) -> bool {
    match unsafe { perform(image, table, boundary, physical_bits, page1gb) } {
        Ok((cpus, resources)) => {
            for (field, value) in [
                ("resources-owned-pages", resources.owned_pages as u64),
                ("resources-borrowed-pages", resources.borrowed_pages as u64),
                ("resources-gdt-pages", resources.gdt_pages as u64),
                ("resources-table-aliases", resources.table_alias_pages as u64),
                ("resources-table-fetches", resources.table_fetch_encodings as u64),
                ("resources-cache-cpus", cpus.enabled_processors as u64),
                ("resources-cache-aps", cpus.completed_ap_captures as u64),
                ("resources-observed", 1),
                ("resources-cleanup", 1),
            ] {
                unsafe { crate::native_entry::snapshot_line(table, field, value) };
            }
            true
        }
        Err(error) => {
            unsafe { crate::native_entry::snapshot_line(table, "resources-refused", error) };
            false
        }
    }
}

#[cfg_attr(feature = "native-returning", inline(always))]
pub(crate) unsafe fn prepare<'a>(
    services: &'a BootServices,
    image: Handle,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
    cpu_storage: (u64, usize),
    cache: PreparedCacheRendezvous<'a>,
) -> Result<Prepared<'a>, u64> {
    let mut owned = Prepared {
        arena: None,
        cache,
        tables: None,
        #[cfg(feature = "native-returning")]
        guest: None,
    };
    let result = (|| {
        owned.arena = Some(unsafe { allocate_arena(services) }?);
        #[cfg(feature = "native-returning")]
        let initialized = {
            let arena = owned.arena.as_ref().ok_or(3u64)?;
            unsafe {
                crate::native_guest_resources::initialize_multi_exit(
                    arena.base() as *mut u8,
                    arena.base(),
                    boundary,
                    crate::native_guest_resources::read_cpuid_inputs(physical_bits),
                )
            }
            .map_err(|error| 0x200 + error)?
        };
        let mut spans =
            unsafe { native_image_resources::collect(image, services) }.map_err(|_| 5u64)?;
        let current_rsp: u64;
        #[cfg(feature = "native-returning")]
        #[allow(named_asm_labels)]
        unsafe {
            core::arch::asm!(
                ".globl svmvisor_native_stack_sample",
                "svmvisor_native_stack_sample:",
                "mov {}, rsp",
                out(reg) current_rsp,
                options(nostack, preserves_flags),
            )
        };
        #[cfg(not(feature = "native-returning"))]
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) current_rsp, options(nostack, nomem, preserves_flags))
        };
        let bottom = current_rsp.checked_sub(64 * 1024).ok_or(6u64)?;
        let top = boundary.entry_rsp.checked_add(40).ok_or(6u64)?;
        let boundary_start = boundary as *const NativeBoundary as u64;
        if top <= current_rsp
            || top - bottom > 128 * 1024
            || boundary_start < current_rsp
            || boundary_start
                .checked_add(core::mem::size_of::<NativeBoundary>() as u64)
                .is_none_or(|end| end > top)
        {
            return Err(6);
        }
        let cache_storage = owned.cache.storage_range().map_err(|_| 7u64)?;
        for span in [
            BorrowedSpan {
                base: cpu_storage.0,
                bytes: cpu_storage.1 as u64,
                access: BorrowedAccess::ReadWrite,
            },
            BorrowedSpan {
                base: cache_storage.0,
                bytes: cache_storage.1 as u64,
                access: BorrowedAccess::ReadWrite,
            },
            BorrowedSpan { base: bottom, bytes: top - bottom, access: BorrowedAccess::ReadWrite },
            BorrowedSpan {
                base: boundary.entry_rip,
                bytes: 1,
                access: BorrowedAccess::ReadExecute,
            },
            BorrowedSpan {
                base: boundary.idtr.base(),
                bytes: u64::from(boundary.idtr.limit()) + 1,
                access: BorrowedAccess::Read,
            },
        ] {
            spans.push(span).map_err(|_| 8u64)?;
        }
        let arena = owned.arena.as_ref().ok_or(3u64)?;
        owned.tables = Some(
            unsafe {
                native_tables::prepare_resource_ranges_detailed(
                    services,
                    physical_bits,
                    page1gb,
                    &[OwnedRange { base: arena.base(), bytes: ARENA_BYTES }],
                    spans.spans().map_err(|_| 8u64)?,
                )
            }
            .map_err(|failure| failure.resource_code())?,
        );
        #[cfg(feature = "native-returning")]
        {
            let tables = owned.tables.as_ref().ok_or(9u64)?;
            owned.guest = Some(
                unsafe {
                    initialized.bind(
                        boundary,
                        tables.captured_gdt().map_err(|_| 9u64)?,
                        svmvisor_dxe::native::transition::state::mode::MULTI_EXIT,
                    )
                }
                .map_err(|error| 0x200 + error)?,
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        owned.release()?;
        return Err(error);
    }
    Ok(owned)
}

pub(crate) unsafe fn allocate_arena(services: &BootServices) -> Result<Arena<'_>, u64> {
    let mut base = 0xffff_ffff;
    if unsafe {
        (services.allocate_pages)(
            AllocateType::MAX_ADDRESS,
            MemoryType::BOOT_SERVICES_DATA,
            ARENA_PAGES,
            &mut base,
        )
    } != Status::SUCCESS
    {
        return Err(3);
    }
    let mut arena = Arena { services, base, owned: true };
    if base < 0x100000 || base & 4095 != 0 || base > (1u64 << 32) - ARENA_BYTES {
        arena.release().map_err(|_| 13u64)?;
        return Err(4);
    }
    // Legitimate AllocatePages ownership/ordinary firmware identity contract,
    // before retained observations. This does not assert current effective WB.
    unsafe { ptr::write_bytes(base as *mut u8, 0, ARENA_BYTES as usize) };
    Ok(arena)
}

#[cfg(not(feature = "native-returning"))]
unsafe fn perform(
    image: Handle,
    table: &SystemTable,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
) -> Result<(native_cache_rendezvous::CacheConsistencyReport, ResourceCacheReport), u64> {
    let services = unsafe { &*table.boot_services };
    let mut cpus = unsafe { native_cpu::prepare(services) }.map_err(|_| 1u64)?;
    let cpu_storage = cpus.storage_range().map_err(|_| 1u64)?;
    let cache = match unsafe { native_cache_rendezvous::prepare(services, &cpus) } {
        Ok(cache) => cache,
        Err(_) => {
            cpus.release().map_err(|_| 14u64)?;
            return Err(2);
        }
    };
    let completed = unsafe {
        cpus.with_prepared_quiescent_bsp_and_ap_observation(
            || prepare(services, image, boundary, physical_bits, page1gb, cpu_storage, cache),
            |prepared| &prepared.cache,
            |guard, prepared| {
                let tables = prepared.tables.as_ref().ok_or(9u64)?;
                tables.revalidate(guard).map_err(|_| 10u64)?;
                let report = prepared.cache.capture_bsp_and_compare(guard).map_err(|_| 11u64)?;
                let before = *prepared.cache.bsp_snapshot().map_err(|_| 11u64)?;
                if prepared.cache.bsp_cr3().map_err(|_| 18u64)? != boundary.cr3 {
                    return Err(18);
                }
                let mappings =
                    native_resource_cache::qualify(&before, tables).map_err(|_| 12u64)?;
                tables.revalidate(guard).map_err(|_| 10u64)?;
                prepared.cache.capture_bsp_and_compare(guard).map_err(|_| 11u64)?;
                if prepared.cache.bsp_cr3().map_err(|_| 18u64)? != boundary.cr3 {
                    return Err(18);
                }
                native_cache_rendezvous::compare_configuration(
                    &before,
                    prepared.cache.bsp_snapshot().map_err(|_| 11u64)?,
                )
                .map_err(|_| 15u64)?;
                Ok::<_, u64>((report, mappings))
            },
            |prepared| prepared.release(),
        )
    };
    let released = cpus.release();
    if released.is_err() {
        return Err(14);
    }
    let completed = match completed {
        Ok(completed) => completed,
        Err(native_cpu::PreparedScopeError::Cpu(_)) => return Err(16),
        Err(native_cpu::PreparedScopeError::Preparation(error)) => return Err(error),
    };
    completed.cleanup?;
    completed.outcome.map_err(|_| 17u64)?.1
}
