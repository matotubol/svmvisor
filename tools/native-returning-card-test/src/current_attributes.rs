//! QEMU/OVMF-only missing-protocol fixture, never a production fallback.
//! Independent integer page walker reads existing CR3 tables without changing
//! any entry. Its physical-table access relies explicitly on this launcher's
//! pinned OVMF boot identity mapping and 256MiB QEMU RAM layout. Only table pages
//! in [1MiB,256MiB) are read. This is not a native physical-reader proof.
use core::arch::{asm, x86_64::__cpuid};
use uefi::{Handle, Status, boot};
use uefi_raw::{
    protocol::memory_protection::MemoryAttributeProtocol, table::boot::MemoryAttribute,
};

static PROTOCOL: MemoryAttributeProtocol = MemoryAttributeProtocol {
    get_memory_attributes: get,
    set_memory_attributes: deny_write,
    clear_memory_attributes: deny_write,
};

fn tcg() -> bool {
    let r = __cpuid(0x40000000);
    let mut vendor = [0u8; 12];
    vendor[..4].copy_from_slice(&r.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&r.ecx.to_le_bytes());
    vendor[8..].copy_from_slice(&r.edx.to_le_bytes());
    // The hypervisor-off test still retains this TCG signature. Do not fake
    // the production driver's independent CPUID hypervisor-bit observation.
    &vendor == b"TCGTCGTCGTCG"
}

/// Only the disposable launcher may call this, before the observed StartImage.
pub unsafe fn install() -> uefi::Result<Handle> {
    if !tcg() {
        return Err(Status::UNSUPPORTED.into());
    }
    // Refuse an existing provider so LocateProtocol cannot silently select a
    // different implementation and invalidate fixture evidence.
    match boot::locate_handle_buffer(boot::SearchType::ByProtocol(&MemoryAttributeProtocol::GUID)) {
        Ok(_) => return Err(Status::ALREADY_STARTED.into()),
        Err(error) if error.status() == Status::NOT_FOUND => {}
        Err(error) => return Err(error),
    }
    unsafe {
        boot::install_protocol_interface(
            None,
            &MemoryAttributeProtocol::GUID,
            core::ptr::addr_of!(PROTOCOL).cast(),
        )
    }
}
pub unsafe fn uninstall(handle: Handle) -> uefi::Result {
    unsafe {
        boot::uninstall_protocol_interface(
            handle,
            &MemoryAttributeProtocol::GUID,
            core::ptr::addr_of!(PROTOCOL).cast(),
        )
    }
}
unsafe extern "efiapi" fn deny_write(
    _: *const MemoryAttributeProtocol,
    _: u64,
    _: u64,
    _: MemoryAttribute,
) -> Status {
    Status::UNSUPPORTED
}
unsafe extern "efiapi" fn get(
    this: *const MemoryAttributeProtocol,
    base: u64,
    length: u64,
    out: *mut MemoryAttribute,
) -> Status {
    if this != core::ptr::addr_of!(PROTOCOL) || out.is_null() || base & 4095 != 0 || length != 4096
    {
        return Status::INVALID_PARAMETER;
    }
    if !tcg() {
        return Status::UNSUPPORTED;
    }
    let cs: u16;
    unsafe {
        asm!("mov {:x}, cs",out(reg) cs,options(nomem,nostack,preserves_flags));
    }
    if cs & 3 != 0 {
        return Status::UNSUPPORTED;
    }
    if cfg!(feature = "attribute-read-denied") {
        // Explicit refusal fixture only; positive mode derives permissions.
        unsafe {
            out.write(MemoryAttribute::READ_PROTECT);
        }
        return Status::SUCCESS;
    }
    match unsafe { permissions(base) } {
        Some(value) => {
            unsafe {
                out.write(value);
            }
            Status::SUCCESS
        }
        None => Status::UNSUPPORTED,
    }
}

unsafe fn permissions(linear: u64) -> Option<MemoryAttribute> {
    // Fixture requests are low identity-mapped RAM pages, not MMIO/firmwareROM.
    if !(0x100000..0x10000000).contains(&linear) {
        return None;
    }
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    let low: u32;
    let high: u32;
    unsafe {
        asm!("mov {}, cr0",out(reg) cr0,options(nostack,preserves_flags));
        asm!("mov {}, cr3",out(reg) cr3,options(nostack,preserves_flags));
        asm!("mov {}, cr4",out(reg) cr4,options(nostack,preserves_flags));
        asm!("rdmsr",in("ecx") 0xc0000080u32,out("eax") low,out("edx") high,options(nostack,preserves_flags));
    }
    let efer = u64::from(low) | (u64::from(high) << 32);
    if cr0 & 0x80000001 != 0x80000001
        || cr4 & (1 << 5) == 0
        || cr4 & ((1 << 12) | (1 << 21) | (1 << 22) | (1 << 23) | (1 << 24)) != 0
        || efer & 0x500 != 0x500
    {
        return None;
    }
    let bits = __cpuid(0x80000008).eax & 255;
    if !(32..=52).contains(&bits) {
        return None;
    }
    let physical_mask = (1u64 << bits) - 1;
    let address_mask = 0x000f_ffff_ffff_f000u64;
    let allowed_cr3_low = if cr4 & (1 << 17) != 0 { 0xfff } else { 0x18 };
    if cr3 & !((physical_mask & address_mask) | allowed_cr3_low) != 0 {
        return None;
    }
    let mut table = cr3 & address_mask;
    let mut writable = true;
    let mut executable = true;
    for (level, shift) in [(4u8, 39u32), (3, 30), (2, 21), (1, 12)] {
        if !(0x100000..0x10000000).contains(&table) || table & 4095 != 0 {
            return None;
        }
        let slot = table + ((linear >> shift) & 511) * 8;
        // Sole physical-table read: complete slot is inside the admitted RAM
        // table page under this independent pinned OVMF identity assumption.
        let entry = unsafe { (slot as *const u64).read_volatile() };
        if entry & 1 == 0 {
            return Some(MemoryAttribute::READ_PROTECT);
        }
        if entry & address_mask & !physical_mask != 0
            || entry & 0x7ff0_0000_0000_0000 != 0
            || (efer & (1 << 11) == 0 && entry & (1 << 63) != 0)
        {
            return None;
        }
        writable &= entry & 2 != 0;
        executable &= entry & (1 << 63) == 0;
        let large = level > 1 && entry & 128 != 0;
        if (level == 4 && large)
            || (level == 3 && large && __cpuid(0x80000001).edx & (1 << 26) == 0)
        {
            return None;
        }
        if level == 1 || large {
            let page_bytes = 1u64 << shift;
            if large && entry & ((page_bytes - 1) & !0x1fff) != 0 {
                return None;
            }
            let physical = (entry & address_mask & !(page_bytes - 1)) + (linear & (page_bytes - 1));
            if physical != linear {
                return None;
            }
            let mut value = MemoryAttribute::empty();
            if !writable {
                value |= MemoryAttribute::READ_ONLY;
            }
            if !executable {
                value |= MemoryAttribute::EXECUTE_PROTECT;
            }
            return Some(value);
        }
        table = entry & address_mask;
    }
    None
}
