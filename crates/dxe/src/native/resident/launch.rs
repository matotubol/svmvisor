//! Pure checks used by the actual resident firmware activation adapter.
use svmvisor_hypervisor::{
    arch::x86_64::{registers::GuestRegisters, xstate::effective_mxcsr_mask},
    guest::continuation::native_cr4_supported,
    host::{
        paging::PagingConfig,
        resident::{
            BridgeContext, DIRECTORY_VERSION, MAX_RESIDENT_CPUS, ResidentDirectory,
            X2AVIC_BACKING_ALIASES_OFFSET,
        },
    },
};

use crate::native::{
    admission::boundary::NativeBoundary,
    resident::delivery::{ARENA_BYTES, is_valid_arena},
};

pub use svmvisor_hypervisor::memory::mtrrs::Mtrrs;

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
    Some(PagingConfig { cr3, physical_bits, la57: false, nxe, pcid, page1gb: true })
}

/// Expected remote backing alias leaves (`X2AVIC_BACKING_ALIASES_OFFSET`) in
/// the private root of `directories[slot]`: `(alias, Some(target))` for every
/// dense pool slot, then `(alias, None)`, i.e. absent, for the rest of the
/// `MAX_RESIDENT_CPUS`-page range. Refuses what `common_backing_offset`
/// refuses. This is the plan only; the caller walks the actual root.
pub fn backing_aliases(
    directories: &[ResidentDirectory],
    slot: usize,
) -> Option<impl Iterator<Item = (u64, Option<u64>)>> {
    let offset = common_backing_offset(directories)?;
    let d = directories.get(slot)?;
    let (base, pool, count) = (d.arena_base, d.pool_base, directories.len() as u64);
    Some((0..MAX_RESIDENT_CPUS as u64).map(move |s| {
        (
            base + X2AVIC_BACKING_ALIASES_OFFSET + s * 4096,
            (s < count).then(|| pool + s * ARENA_BYTES as u64 + offset),
        )
    }))
}

/// Common image offset of every slot's retained x2AVIC backing page. Each
/// runtime copy maps all remote backing aliases from its own linked offset,
/// which is correct only because every slot holds the same relocated image.
/// Accept only the complete dense pool (slots 0..len in order) whose entries
/// each pass `directory_valid`, so the page is aligned inside the image below
/// the alias range, and agree on pool and offset. Numeric agreement is not
/// allocation, mapping or ownership proof.
pub fn common_backing_offset(directories: &[ResidentDirectory]) -> Option<u64> {
    let first = directories.first()?;
    let offset = first.avic_backing.checked_sub(first.arena_base)?;
    if directories.len() > MAX_RESIDENT_CPUS {
        return None;
    }
    let pool_bytes = directories.len() as u64 * ARENA_BYTES as u64;
    directories
        .iter()
        .enumerate()
        .all(|(slot, d)| {
            first.pool_base.checked_add(slot as u64 * ARENA_BYTES as u64).is_some_and(|base| {
                directory_valid(d, base)
                    && d.cpu_slot == slot as u64
                    && d.pool_base == first.pool_base
                    && d.pool_bytes == pool_bytes
                    && d.avic_backing.checked_sub(base) == Some(offset)
            })
        })
        .then_some(offset)
}

pub fn directory_valid(d: &ResidentDirectory, base: u64) -> bool {
    let Some(end) = base.checked_add(ARENA_BYTES as u64) else {
        return false;
    };
    if !is_valid_arena(base)
        || !svmvisor_hypervisor::host::resident::is_valid_pool_slot(
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
            && d.memory_end <= end - (ARENA_BYTES as u64 - X2AVIC_BACKING_ALIASES_OFFSET))
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
        (d.registers, core::mem::size_of::<GuestRegisters>() as u64, 8),
        (
            d.npt,
            core::mem::size_of::<svmvisor_hypervisor::memory::npt::TableStorage>() as u64,
            4096,
        ),
        (d.avic_backing, 4096, 4096),
    ];
    for (i, &(address, bytes, alignment)) in objects.iter().enumerate() {
        if address < d.data_start
            || address & (alignment - 1) != 0
            || address.checked_add(bytes).is_none_or(|last| last > d.memory_end)
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
