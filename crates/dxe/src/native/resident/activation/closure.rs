//! Prepared resident directories and validation of each slot's private host closure.
use core::ptr;

use svmvisor_dxe::native::resident::launch::{Mtrrs, backing_aliases};
use svmvisor_hypervisor::{
    boot::memory::MemoryDescriptor,
    host::{
        paging::{self, PagingConfig},
        resident::{self as abi, ResidentDirectory},
    },
    memory::npt::TableStorage,
};

use super::{
    CPU_COUNT, Cpu, DIRECTORIES,
    diagnostic::{admission_hint, admission_walk},
    mapping::mapped,
};

/// Prepared directories of every admitted slot. Written once by the serialized
/// installer before READY; later readers only copy.
pub(super) unsafe fn directories() -> &'static [ResidentDirectory] {
    unsafe { core::slice::from_raw_parts(ptr::addr_of!(DIRECTORIES).cast(), CPU_COUNT) }
}

// Check the actual prepared host closure of `directories[slot]` through its
// private root before the runtime selects that root. `directories` is the
// complete prepared pool. Current native mappings already cover every arena,
// making these retained reads safe under the initial identity contract.
pub(super) unsafe fn host_closure(
    directories: &[ResidentDirectory],
    slot: usize,
    map: &[MemoryDescriptor],
    cpu: Cpu,
    mt: &Mtrrs,
    pat: u64,
) -> Result<(), u64> {
    let (Some(d), Some(aliases)) = (directories.get(slot), backing_aliases(directories, slot))
    else {
        admission_hint(633, slot as u64, directories.len() as u64, abi::MAX_RESIDENT_CPUS as u64);
        return Err(24);
    };
    let c = unsafe { &*(d.context as *const abi::BridgeContext) };
    let inside = |address: u64, bytes: u64, alignment: u64| {
        address >= d.data_start
            && address & (alignment - 1) == 0
            && address.checked_add(bytes).is_some_and(|end| end <= d.memory_end)
    };
    macro_rules! reject {
        ($bad:expr,$predicate:expr,$item:expr,$observed:expr,$expected:expr,$code:expr) => {
            if $bad {
                admission_hint($predicate, $item, $observed as u64, $expected as u64);
                return Err($code);
            }
        };
    }
    reject!(c.guest_vmcb_pa != d.vmcb, 601, d.context, c.guest_vmcb_pa, d.vmcb, 24);
    reject!(c.guest_vmcb_va != d.vmcb, 602, d.context, c.guest_vmcb_va, d.vmcb, 24);
    reject!(c.guest_frame_va != d.registers, 603, d.context, c.guest_frame_va, d.registers, 24);
    reject!(c.host_code_selector != 8, 604, d.context, c.host_code_selector, 8, 24);
    reject!(c.host_data_selector != 16, 605, d.context, c.host_data_selector, 16, 24);
    reject!(c.host_tr_selector != 24, 606, d.context, c.host_tr_selector, 24, 24);
    reject!(!inside(c.host_cr3, 16384, 4096), 607, d.context, c.host_cr3, d.memory_end, 24);
    reject!(!inside(c.hsave_pa, 4096, 4096), 608, d.context, c.hsave_pa, d.memory_end, 24);
    reject!(
        !inside(c.host_extra_pa, 4096, 4096),
        609,
        d.context,
        c.host_extra_pa,
        d.memory_end,
        24
    );
    reject!(!inside(c.owner_context, 1, 8), 610, d.context, c.owner_context, d.memory_end, 24);
    reject!(!inside(c.host_gdtr_va, 10, 1), 611, d.context, c.host_gdtr_va, d.memory_end, 24);
    reject!(!inside(c.host_idtr_va, 10, 1), 612, d.context, c.host_idtr_va, d.memory_end, 24);
    let stack_base = c.host_stack_top.checked_sub(65536).ok_or_else(|| {
        admission_hint(613, d.context, c.host_stack_top, 65536);
        24u64
    })?;
    reject!(!inside(stack_base, 65536, 16), 613, d.context, c.host_stack_top, d.memory_end, 24);
    reject!(
        (c.dispatch as usize as u64) < d.arena_base,
        614,
        d.context,
        c.dispatch as usize,
        d.arena_base,
        24
    );
    reject!(
        (c.dispatch as usize as u64) >= d.text_end,
        615,
        d.context,
        c.dispatch as usize,
        d.text_end,
        24
    );
    reject!(c.hsave_pa == d.vmcb, 616, d.context, c.hsave_pa, d.vmcb, 24);
    reject!(c.hsave_pa == d.auxiliary, 617, d.context, c.hsave_pa, d.auxiliary, 24);
    reject!(c.host_extra_pa == d.vmcb, 618, d.context, c.host_extra_pa, d.vmcb, 24);
    reject!(c.host_extra_pa == d.auxiliary, 619, d.context, c.host_extra_pa, d.auxiliary, 24);
    reject!(c.host_extra_pa == c.hsave_pa, 620, d.context, c.host_extra_pa, c.hsave_pa, 24);
    let cfg = PagingConfig {
        cr3: c.host_cr3,
        physical_bits: cpu.physical_bits,
        la57: false,
        nxe: true,
        pcid: false,
        page1gb: true,
    };
    // NXE is the runtime's explicit entry commitment; this walk checks the
    // constructed root with that setting before any live CR3/EFER change.
    unsafe {
        mapped(map, cfg, mt, pat, d.arena_base, d.text_end - d.arena_base, false, true)?;
        for (address, bytes) in [
            (d.context, core::mem::size_of::<abi::BridgeContext>() as u64),
            (d.vmcb, 4096),
            (d.registers, 112),
            (d.npt, core::mem::size_of::<TableStorage>() as u64),
            (c.hsave_pa, 4096),
            (c.host_extra_pa, 4096),
            (c.host_stack_top - 65536, 65536),
            (c.owner_context, 1),
        ] {
            mapped(map, cfg, mt, pat, address, bytes, true, false)?;
        }
        for header in [c.host_gdtr_va, c.host_idtr_va] {
            mapped(map, cfg, mt, pat, header, 10, false, false)?;
        }
    }
    // Private root tables must lie in this image's retained data.
    let walk = |address| {
        let mut last = None;
        paging::translate(cfg, address, |physical| {
            if !inside(physical & !4095, 4096, 4096) {
                admission_hint(621, physical, physical & !4095, d.memory_end);
                return None;
            }
            let entry = unsafe { (physical as *const u64).read_volatile() };
            last = Some((physical, entry));
            Some(entry)
        })
        .map_err(|error| (error, last))
    };
    // Shared aliases retain one qualified backing and their exact permissions.
    let check_alias = |address, expected, writable, check_wb| -> Result<(), u64> {
        let translated = walk(address).map_err(|(error, last)| {
            admission_walk(error, cfg, address, last);
            24u64
        })?;
        reject!(
            translated.physical_address != expected,
            622,
            address,
            translated.physical_address,
            expected,
            24
        );
        reject!(
            translated.writable != writable || translated.executable || translated.user,
            623,
            address,
            u64::from(translated.writable)
                | u64::from(translated.executable) << 1
                | u64::from(translated.user) << 2,
            u64::from(writable),
            24
        );
        reject!(
            (pat >> (translated.pat_index * 8)) & 255 != 6,
            624,
            address,
            pat,
            translated.pat_index,
            24
        );
        // page_is_wb answers for a 4KiB page base only; aliases are also probed at +4095.
        reject!(
            check_wb && !mt.page_is_wb(translated.physical_address & !4095),
            632,
            address,
            translated.physical_address,
            1,
            24
        );
        Ok(())
    };
    for offset in [0, 4095] {
        check_alias(
            d.arena_base + abi::STARTUP_PAGE_OFFSET + offset,
            d.pool_base + abi::STARTUP_PAGE_OFFSET + offset,
            true,
            false,
        )?;
        check_alias(
            d.arena_base + abi::X2AVIC_TABLE_OFFSET + offset,
            d.pool_base + abi::X2AVIC_TABLE_OFFSET + offset,
            true,
            true,
        )?;
    }
    for offset in (abi::CACHE_OWNER_OFFSET
        ..abi::CACHE_CAPTURE_OFFSET
            + core::mem::size_of::<svmvisor_hypervisor::svm::native_cache::CacheCapture>() as u64)
        .step_by(4096)
    {
        check_alias(
            d.arena_base + offset,
            d.pool_base + offset,
            offset < abi::CACHE_CAPTURE_OFFSET,
            true,
        )?;
    }
    // Remote backing aliases: one WB RW/NX leaf for every pool slot's backing
    // page (this slot's included) and no leaf for the rest of the alias range.
    for (alias, expected) in aliases {
        match expected {
            Some(backing) => check_alias(alias, backing, true, true)?,
            None => match walk(alias) {
                Err((paging::WalkError::NotPresent { level: 1 }, _)) => {}
                Err((error, last)) => {
                    admission_walk(error, cfg, alias, last);
                    return Err(24);
                }
                Ok(t) => {
                    admission_hint(634, alias, t.physical_address, 0);
                    return Err(24);
                }
            },
        }
    }
    let gdtr = unsafe { core::slice::from_raw_parts(c.host_gdtr_va as *const u8, 10) };
    let idtr = unsafe { core::slice::from_raw_parts(c.host_idtr_va as *const u8, 10) };
    let gdt = u64::from_le_bytes(gdtr[2..10].try_into().unwrap());
    let idt = u64::from_le_bytes(idtr[2..10].try_into().unwrap());
    reject!(
        gdtr[..2] != 39u16.to_le_bytes(),
        625,
        c.host_gdtr_va,
        u16::from_le_bytes(gdtr[..2].try_into().unwrap()),
        39,
        25
    );
    reject!(
        idtr[..2] != 4095u16.to_le_bytes(),
        626,
        c.host_idtr_va,
        u16::from_le_bytes(idtr[..2].try_into().unwrap()),
        4095,
        25
    );
    reject!(!inside(gdt, 40, 8), 627, c.host_gdtr_va, gdt, d.memory_end, 25);
    reject!(!inside(idt, 4096, 16), 628, c.host_idtr_va, idt, d.memory_end, 25);
    unsafe {
        mapped(map, cfg, mt, pat, gdt, 40, true, false)?;
        mapped(map, cfg, mt, pat, idt, 4096, false, false)?;
    }
    let tss_descriptor = unsafe { core::slice::from_raw_parts((gdt + 24) as *const u8, 16) };
    let tss = u64::from_le_bytes([
        tss_descriptor[2],
        tss_descriptor[3],
        tss_descriptor[4],
        tss_descriptor[7],
        tss_descriptor[8],
        tss_descriptor[9],
        tss_descriptor[10],
        tss_descriptor[11],
    ]);
    reject!(tss_descriptor[5] != 0x89, 629, gdt + 24, tss_descriptor[5], 0x89, 26);
    reject!(!inside(tss, 104, 8), 630, gdt + 24, tss, d.memory_end, 26);
    unsafe {
        mapped(map, cfg, mt, pat, tss, 104, true, false)?;
    }
    let fault_top = unsafe { ptr::read_unaligned((tss + 36) as *const u64) };
    let fault_base = fault_top.checked_sub(16384).ok_or_else(|| {
        admission_hint(631, tss + 36, fault_top, 16384);
        26u64
    })?;
    reject!(!inside(fault_base, 16384, 16), 631, tss + 36, fault_top, d.memory_end, 26);
    unsafe {
        mapped(map, cfg, mt, pat, fault_top - 16384, 16384, true, false)?;
    }
    Ok(())
}
