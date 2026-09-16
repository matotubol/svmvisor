use core::arch::{asm, x86_64::__cpuid_count};
use svmvisor_hypervisor::{boot::xstate::*, arch::x86_64::xstate::XstateCapabilities};

pub struct Cpu {
    pub plan: FirmwareXstatePlan,
    pub physical_bits: u8,
}
pub fn cpuid(leaf: u32, sub: u32) -> core::arch::x86_64::CpuidResult {
    unsafe { __cpuid_count(leaf, sub) }
}
pub unsafe fn msr(id: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") id, out("eax") low, out("edx") high, options(nostack));
    }
    ((high as u64) << 32) | low as u64
}
pub fn tcg_guard() -> bool {
    let leaf = cpuid(1, 0);
    let hv = cpuid(0x40000000, 0);
    let mut vendor = [0; 12];
    vendor[..4].copy_from_slice(&hv.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&hv.ecx.to_le_bytes());
    vendor[8..].copy_from_slice(&hv.edx.to_le_bytes());
    leaf.ecx & (1 << 31) != 0 && &vendor == b"TCGTCGTCGTCG"
}
/// Call only after the exact TCG hypervisor guard. Native policy intentionally
/// rejects this reported hypervisor; no fabricated clear bit is used here.
pub unsafe fn collect() -> Option<Cpu> {
    if !tcg_guard() {
        return None;
    }
    let vendor = cpuid(0, 0);
    if [vendor.ebx, vendor.edx, vendor.ecx] != [0x68747541, 0x69746e65, 0x444d4163] {
        return None;
    }
    if cpuid(0x80000000, 0).eax < 0x8000000a {
        return None;
    }
    let svm = cpuid(0x8000000a, 0);
    let ext = cpuid(0x80000001, 0);
    if ext.ecx & 4 == 0 || ext.edx & (1 << 20) == 0 || svm.edx & 1 == 0 || svm.ebx < 2 {
        return None;
    }
    let efer = unsafe { msr(0xc0000080) };
    if efer & ((1 << 12) | (1 << 14)) != 0
        || efer & (1 << 11) == 0
        || unsafe { msr(0xc0010114) } & (1 << 4) != 0
        || unsafe { msr(0xc0010117) } != 0
    {
        return None;
    }
    let cr0: u64;
    let cr4: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nostack));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack));
    }
    if cr0 & 12 != 0 || cr4 & (1 << 12) != 0 {
        return None;
    }
    let one = cpuid(1, 0);
    let has_xsave = one.ecx & (1 << 26) != 0;
    // OSXSAVE-off callers remain rejected by FirmwareXstatePlan. An explicit
    // enclosing emulator assembly fixture can configure3/7 before this call.
    let flags: u64;
    let dr7: u64;
    // Fixed OVMF fixture assumes GD is clear before this privileged read.
    unsafe {
        asm!("pushfq; pop {}", out(reg) flags);
        asm!("mov {}, dr7", out(reg) dr7, options(nostack));
    }
    if flags & ((1 << 8) | (1 << 17)) != 0 || dr7 & 0x23ff != 0 {
        return None;
    }
    let d0 = if has_xsave {
        cpuid(0xd, 0)
    } else {
        cpuid(0, 0)
    };
    let d1 = if has_xsave {
        cpuid(0xd, 1)
    } else {
        cpuid(0, 0)
    };
    let avx = if has_xsave {
        cpuid(0xd, 2)
    } else {
        cpuid(0, 0)
    };
    // This first assembly ABI does not touch or save supervisor/XSS state.
    if has_xsave && d1.eax & (1 << 3) != 0 {
        return None;
    }
    let xcr0 = if has_xsave && cr4 & (1 << 18) != 0 {
        let low: u32;
        let high: u32;
        unsafe {
            asm!("xgetbv", in("ecx") 0u32, out("eax") low, out("edx") high, options(nostack));
        }
        Some(((high as u64) << 32) | low as u64)
    } else {
        None
    };
    let caps = XstateCapabilities {
        leaf1_ecx: one.ecx,
        leaf1_edx: one.edx,
        supported_xcr0: if has_xsave {
            ((d0.edx as u64) << 32) | d0.eax as u64
        } else {
            0
        },
        enabled_size: if has_xsave { d0.ebx } else { 0 },
        max_size: if has_xsave { d0.ecx } else { 0 },
        avx_size: if has_xsave { avx.eax } else { 0 },
        avx_offset: if has_xsave { avx.ebx } else { 0 },
        avx_flags: if has_xsave { avx.ecx } else { 0 },
    };
    let plan = FirmwareXstatePlan::validate(FirmwareXstateEvidence {
        max_basic_leaf: vendor.eax,
        capabilities: caps,
        leaf_d1_eax: if has_xsave { d1.eax } else { 0 },
        supported_xss: if has_xsave {
            ((d1.edx as u64) << 32) | d1.ecx as u64
        } else {
            0
        },
        original: FirmwareXstateControls {
            cr0,
            cr4,
            efer,
            xcr0,
            xss: None,
        },
    })
    .ok()?;
    Some(Cpu {
        plan,
        physical_bits: (cpuid(0x80000008, 0).eax & 0xff) as u8,
    })
}

/// Read-only admission for the explicit emulator outer fixture, not native policy.
/// Original XCR0 when OSXSAVE is off is checked inside the assembly boundary.
pub unsafe fn fixture_supported(mask: u64) -> bool {
    if !tcg_guard() || !matches!(mask, 3 | 7) {
        return false;
    }
    let basic = cpuid(0, 0);
    if basic.eax < 0xd || [basic.ebx, basic.edx, basic.ecx] != [0x68747541, 0x69746e65, 0x444d4163]
    {
        return false;
    }
    let one = cpuid(1, 0);
    let legacy = 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26);
    if one.edx & legacy != legacy || one.ecx & (1 << 26) == 0 {
        return false;
    }
    let d0 = cpuid(0xd, 0);
    let d1 = cpuid(0xd, 1);
    let avx = cpuid(0xd, 2);
    let supported = (u64::from(d0.edx) << 32) | u64::from(d0.eax);
    if supported & mask != mask || d1.eax & !7 != 0 || d1.ecx != 0 || d1.edx != 0 {
        return false;
    }
    if mask == 7
        && (one.ecx & (1 << 28) == 0
            || avx.eax != 256
            || avx.ebx < 576
            || avx.ebx > 3840
            || avx.ecx & !2 != 0)
    {
        return false;
    }
    if d0.ecx < if mask == 7 { avx.ebx + 256 } else { 576 } {
        return false;
    }
    let cr0: u64;
    let cr4: u64;
    let flags: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nostack));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack));
        asm!("pushfq; pop {}", out(reg) flags);
    }
    if cr0 & 12 != 0
        || cr4 >> 22 != 0
        || cr4 & (1 << 12) != 0
        || cr4 & 0x220 != 0x220
        || flags & ((1 << 8) | (1 << 17)) != 0
    {
        return false;
    }
    let efer = unsafe { msr(0xc0000080) };
    if efer & ((1 << 12) | (1 << 14)) != 0 || efer & 0xd00 != 0xd00 {
        return false;
    }
    if cr4 & (1 << 18) != 0 {
        let low: u32;
        let high: u32;
        unsafe {
            asm!("xgetbv", in("ecx") 0u32, out("eax") low, out("edx") high, options(nostack));
        }
        let original = (u64::from(high) << 32) | u64::from(low);
        if !matches!(original, 1 | 3 | 7) || original & !mask != 0 {
            return false;
        }
    }
    true
}

/// Snapshot only the ten defined IDTR bytes; padding is not architectural state.
pub fn idtr() -> [u8; 10] {
    let mut value = [0u8; 10];
    unsafe {
        asm!("sidt [{}]", in(reg) value.as_mut_ptr(), options(nostack, preserves_flags));
    }
    value
}
