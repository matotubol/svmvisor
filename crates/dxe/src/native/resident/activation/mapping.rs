//! Current-root mapping validation: retained RAM spans, WB identity mappings and the
//! card's UC MMIO leaf.
use core::ptr;

use svmvisor_dxe::native::resident::launch::Mtrrs;
use svmvisor_hypervisor::{
    boot::memory::MemoryDescriptor,
    host::paging::{self, PagingConfig},
};

use super::diagnostic::{admission_hint, admission_walk, trace_detail};

pub(super) unsafe fn mapped(
    map: &[MemoryDescriptor],
    cfg: PagingConfig,
    mt: &Mtrrs,
    pat: u64,
    base: u64,
    bytes: u64,
    write: bool,
    execute: bool,
) -> Result<(), u64> {
    let end = base.checked_add(bytes).ok_or_else(|| {
        admission_hint(107, base, bytes, u64::MAX - base);
        7u64
    })?;
    if bytes == 0 || bytes > 2 * 1024 * 1024 || !ram_span(map, base, bytes) {
        admission_hint(107, base, bytes, 2 * 1024 * 1024);
        return Err(7);
    }
    let mut page = base & !4095;
    while page < end {
        let mut last = None;
        let translated = paging::translate(cfg, page, |address| {
            if !ram_span(map, address, 8) || !mt.page_is_wb(address & !4095) {
                trace_detail(&(
                    "table-backing",
                    address,
                    ram_span(map, address, 8),
                    mt.page_is_wb(address & !4095),
                ));
                admission_hint(
                    108,
                    address,
                    u64::from(ram_span(map, address, 8))
                        | u64::from(mt.page_is_wb(address & !4095)) << 1,
                    3,
                );
                return None;
            }
            // Restrict every paging-structure fetch to PAT index0 (WB below).
            let entry = unsafe { ptr::read_volatile(address as *const u64) };
            last = Some((address, entry));
            if entry & 0x18 != 0 {
                admission_hint(109, address, entry, 0x18);
                trace_detail(&("table-cache", address, entry));
                return None;
            }
            Some(entry)
        })
        .map_err(|error| {
            trace_detail(&error);
            admission_walk(error, cfg, page, last);
            8u64
        })?;
        if translated.physical_address != page
            || (write && !translated.writable)
            || (execute && !translated.executable)
            || !mt.page_is_wb(page)
            || ((pat >> (translated.pat_index * 8)) & 255) != 6
            || pat & 255 != 6
        {
            trace_detail(&("mapping", page, translated, mt.page_is_wb(page), pat));
            let (predicate, observed, expected) = if translated.physical_address != page {
                (110, translated.physical_address, page)
            } else if write && !translated.writable {
                (111, 0, 1)
            } else if execute && !translated.executable {
                (112, 0, 1)
            } else if !mt.page_is_wb(page) {
                (113, 0, 1)
            } else {
                (114, pat, translated.pat_index as u64)
            };
            admission_hint(predicate, page, observed, expected);
            return Err(9);
        }
        page = page.checked_add(4096).ok_or(7u64)?;
    }
    Ok(())
}

/// Validate one direct UC supervisor MMIO leaf of the card through the admitted
/// WB paging-structure reader. No BAR read precedes it.
#[cfg(feature = "native-resident-boot")]
pub(super) unsafe fn validate_uc_mmio(
    map: &[MemoryDescriptor],
    cfg: PagingConfig,
    mt: &Mtrrs,
    pat: u64,
    base: u64,
) -> Result<(), u64> {
    if base == 0 || base > 0xfffff000 || base & 4095 != 0 || pat & 255 != 6 {
        admission_hint(147, base, pat, 6);
        return Err(47);
    }
    let mut level = 4;
    let mut last = None;
    let translated = paging::translate(cfg, base, |address| {
        if !ram_span(map, address, 8) || !mt.page_is_wb(address & !4095) {
            admission_hint(
                108,
                address,
                u64::from(ram_span(map, address, 8))
                    | u64::from(mt.page_is_wb(address & !4095)) << 1,
                3,
            );
            return None;
        }
        let entry = unsafe { ptr::read_volatile(address as *const u64) };
        last = Some((address, entry));
        let leaf = level == 1 || (level < 4 && entry & 0x80 != 0);
        level -= 1;
        if !leaf && entry & 0x18 != 0 {
            admission_hint(109, address, entry, 0x18);
            None
        } else {
            Some(entry)
        }
    })
    .map_err(|error| {
        admission_walk(error, cfg, base, last);
        47u64
    })?;
    if translated.physical_address != base
        || !translated.writable
        || translated.user
        || !mt.page_is_uc(base, ((pat >> (translated.pat_index * 8)) & 255) as u8)
    {
        let (predicate, observed, expected) = if translated.physical_address != base {
            (110, translated.physical_address, base)
        } else if !translated.writable {
            (111, 0, 1)
        } else if translated.user {
            (148, 1, 0)
        } else {
            (149, pat, translated.pat_index as u64)
        };
        admission_hint(predicate, base, observed, expected);
        return Err(47);
    }
    Ok(())
}

// Map type 7 may have been allocated by firmware since installation. This
// reader uses it only as retained physical-RAM capability, never as current
// ownership. The caller's native identity/coherency/handler cooperation contract
// supplies safe physical reads. Current translations are checked at each use.
pub(super) fn ram_span(map: &[MemoryDescriptor], base: u64, bytes: u64) -> bool {
    let Some(end) = base.checked_add(bytes) else {
        return false;
    };
    let mut cursor = base;
    for d in map {
        let last = d.physical_start + d.page_count * 4096;
        if last <= cursor {
            continue;
        }
        if d.physical_start > cursor
            || !(1..=7).contains(&d.memory_type)
            || d.attributes & 8 == 0
            || d.attributes & 0x2000 != 0
        {
            return false;
        }
        cursor = last.min(end);
        if cursor == end {
            return true;
        }
    }
    false
}
