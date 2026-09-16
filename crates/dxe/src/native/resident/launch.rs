//! Pure checks used by the actual resident firmware activation adapter.
use super::delivery::{ARENA_BYTES, valid_arena};
use crate::native::admission::boundary::NativeBoundary;
use svmvisor_hypervisor::{
    arch::x86_64::{registers::GuestRegisters, xstate::effective_mxcsr_mask},
    guest::continuation::native_cr4_supported,
    host::paging::PagingConfig,
    host::resident::{BridgeContext, DIRECTORY_VERSION, ResidentDirectory},
};

/// Decode current native controls without modifying them. AMD APM2 rev3.44
/// 3.1.3/5.5.1: CR3[11:0] is the PCID only while CR4.PCIDE is set. The native
/// adapter still requires WB paging structures, so PWT/PCD remain refused when
/// PCID is off. The existing walker validates root width and table contents.
pub fn native_paging_config(
    cr0: u64,
    cr3: u64,
    cr4: u64,
    physical_bits: u8,
    nxe: bool,
) -> Option<PagingConfig> {
    let pcid = cr4 & (1 << 17) != 0;
    if cr0 & 0x80000011 != 0x80000011
        || cr0 & 0x60000000 != 0
        || !native_cr4_supported(cr4)
        || (!pcid && cr3 & 4095 != 0)
    {
        return None;
    }
    Some(PagingConfig {
        cr3,
        physical_bits,
        la57: false,
        nxe,
        pcid,
        page1gb: true,
    })
}

pub fn directory_valid(d: &ResidentDirectory, base: u64) -> bool {
    let Some(end) = base.checked_add(ARENA_BYTES as u64) else {
        return false;
    };
    if !valid_arena(base)
        || !svmvisor_hypervisor::host::resident::valid_pool_slot(
            base,
            d.pool_base,
            d.pool_bytes,
            d.cpu_slot,
            d.apic_id,
        )
        || d.version != DIRECTORY_VERSION
        || d.arena_base != base
        || d.arena_bytes != ARENA_BYTES as u64
        || d.reserved != [0; 2]
        || !(base < d.text_end
            && d.text_end <= d.data_start
            && d.data_start < d.memory_end
            && d.memory_end <= end - (ARENA_BYTES as u64 - svmvisor_hypervisor::host::resident::SOURCE_ROUTES_OFFSET))
        || d.arm < base
        || d.arm >= d.text_end
        || d.enter < base
        || d.enter >= d.text_end
    {
        return false;
    }
    let objects = [
        (d.context, core::mem::size_of::<BridgeContext>() as u64, 16),
        (d.vmcb, 4096, 4096),
        (d.auxiliary, 4096, 4096),
        (
            d.registers,
            core::mem::size_of::<GuestRegisters>() as u64,
            8,
        ),
        (d.npt, core::mem::size_of::<svmvisor_hypervisor::memory::npt::TableStorage>() as u64, 4096),
        (d.avic_backing, 4096, 4096),
    ];
    for (i, &(address, bytes, alignment)) in objects.iter().enumerate() {
        if address < d.data_start
            || address & (alignment - 1) != 0
            || address
                .checked_add(bytes)
                .is_none_or(|last| last > d.memory_end)
        {
            return false;
        }
        for &(other, length, _) in &objects[..i] {
            if address < other + length && other < address + bytes {
                return false;
            }
        }
    }
    true
}

/// AMD APM2 11.4/11.5: reject pending x87 exceptions, reserved MXCSR state,
/// compacted/supervisor XSAVE state and components outside the captured mask.
pub fn xstate_valid(b: &NativeBoundary) -> bool {
    if !b.has_valid_shape() {
        return false;
    }
    let word = |offset| u32::from_le_bytes(b.xstate[offset..offset + 4].try_into().unwrap());
    let fsw = u16::from_le_bytes([b.xstate[2], b.xstate[3]]);
    let Ok(mask) = effective_mxcsr_mask(word(28)) else {
        return false;
    };
    if fsw & 0x80 != 0 || word(24) & !mask != 0 {
        return false;
    }
    if b.profile != 0 {
        let bitmap = u64::from_le_bytes(b.xstate[512..520].try_into().unwrap());
        if bitmap & !b.profile != 0 || b.xstate[520..576].iter().any(|byte| *byte != 0) {
            return false;
        }
    }
    true
}

pub use svmvisor_hypervisor::memory::mtrrs::Mtrrs;
