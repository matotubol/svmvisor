//! Runtime-driver installation, qualified callback capture and loader continuation.
//!
//! UEFI 2.11 2.3.4.2/7.1/7.2/8.4.1 and AMD APM2 rev3.44 11.4/11.5/15.5/15.7.
//! This is a trusted native flat-UEFI profile, with an explicit diagnostic
//! post-EBS AP activation seam and an explicit normal-loader boot profile. The initial retained
//! map is a bounded RAM/aperture observation, not an allocation lease for future
//! firmware tables. Current paging is walked again inside the CLI callback.
//! Runtime allocation remains owned throughout; no Boot Services call occurs
//! in the callback inner or any persistent host path.
use core::{
    arch::{asm, x86_64::__cpuid_count},
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use svmvisor_dxe::native::{
    admission::{boundary::NativeBoundary, memory},
    resident::{
        self, CallbackRequest, CallbackSites, GuestStackSpan, allocation,
        delivery::Payload,
        launch::{
            Mtrrs, backing_aliases, common_backing_offset, directory_valid, native_paging_config,
            xstate_valid,
        },
    },
};
use svmvisor_hypervisor::{
    arch::x86_64::{
        apic,
        capabilities::EvidenceFlag,
        msr::{
            MTRR_CAP, PAT, SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, TARGET_PHYSICAL_BITS,
            TARGET_SIGNATURE, TOM2, VM_CR, VM_CR_SVMDIS,
        },
        registers::GuestRegisters,
    },
    boot::{
        descriptors::{FirmwareSelectors, parse_firmware_gdt},
        memory::{MemoryDescriptor, ValidatedMemoryMap},
    },
    host::{
        descriptors::HostTablePointer,
        paging::{self, PagingConfig},
        resident::{self as abi, ResidentDirectory},
    },
    memory::{
        address::{AddressPolicy, EncryptionState},
        npt::{NptEvidence, TableStorage},
    },
    svm::vmcb::Vmcb,
};
use uefi_raw::{
    Event, Handle, Status, guid,
    protocol::loaded_image::LoadedImageProtocol,
    table::{
        boot::{BootServices, EventType, MemoryType, Tpl},
        system::SystemTable,
    },
};

#[cfg(feature = "native-resident-boot")]
#[path = "boot_handoff.rs"]
mod boot_handoff;
#[cfg(feature = "native-resident-boot")]
#[path = "card_boot.rs"]
mod card_boot;
#[cfg(feature = "native-resident-smp-activate")]
#[path = "physical_boot.rs"]
mod physical;
// Shared qualification helpers also serve profiles without MP admission.
fn admission_hint(predicate: u32, item: u64, observed: u64, expected: u64) {
    #[cfg(feature = "native-resident-smp-activate")]
    physical::admission_hint(predicate, item, observed, expected);
    #[cfg(not(feature = "native-resident-smp-activate"))]
    let _ = (predicate, item, observed, expected);
}
fn admission_walk(
    error: paging::WalkError,
    cfg: PagingConfig,
    address: u64,
    last: Option<(u64, u64)>,
) {
    #[cfg(feature = "native-resident-smp-activate")]
    physical::admission_walk(error, cfg, address, last);
    #[cfg(not(feature = "native-resident-smp-activate"))]
    let _ = (error, cfg, address, last);
}
include!(concat!(env!("OUT_DIR"), "/resident-entry.rs"));
static PACKAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/resident-payload.bin"));
static INSTALLED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
const EMPTY: MemoryDescriptor =
    MemoryDescriptor { memory_type: 0, physical_start: 0, page_count: 0, attributes: 0 };
static mut MAP: [MemoryDescriptor; 4096] = [EMPTY; 4096];
static mut MAP_COUNT: usize = 0;
static mut DIRECTORIES: [ResidentDirectory; abi::MAX_RESIDENT_CPUS] = [ResidentDirectory {
    version: 0,
    arena_base: 0,
    arena_bytes: 0,
    context: 0,
    vmcb: 0,
    auxiliary: 0,
    registers: 0,
    npt: 0,
    arm: 0,
    enter: 0,
    text_end: 0,
    data_start: 0,
    memory_end: 0,
    pool_base: 0,
    pool_bytes: 0,
    cpu_slot: 0,
    apic_id: 0,
    avic_backing: 0,
    reserved: [0; 2],
}; abi::MAX_RESIDENT_CPUS];
#[unsafe(export_name = "svmvisor_resident_cpu_ids")]
static mut CPU_IDS: [u32; abi::MAX_RESIDENT_CPUS] = [u32::MAX; abi::MAX_RESIDENT_CPUS];
#[unsafe(export_name = "svmvisor_resident_cpu_count")]
static mut CPU_COUNT: usize = 0;
// Trusted one-shot boot witness, published only by the post-ACK guest
// epilogue. It is not an attestation against later guest software. Never reset
// after an AP can run; INSTALLED/ACTIVATING/start admission forbid reuse.
#[unsafe(export_name = "svmvisor_resident_guest_ack_mask")]
static GUEST_ACK: AtomicU32 = AtomicU32::new(0);
static ACTIVATING: AtomicU32 = AtomicU32::new(0);
static mut CPU: Option<Cpu> = None;
static mut IMAGE: (u64, u64) = (0, 0);
static mut GDT: [u8; 65536] = [0; 65536];
const COOKIE: usize = 0x53564d52;
const EFER: u32 = 0xc0000080;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cpu {
    physical_bits: u8,
    apic_id: u32,
    encryption: EncryptionState,
}

fn trace(code: u8) {
    #[cfg(feature = "native-resident-test")]
    unsafe {
        asm!("out dx, al", in("dx") 0xe9u16, in("al") code, options(nomem, nostack, preserves_flags));
    }
    #[cfg(not(feature = "native-resident-test"))]
    let _ = code;
}

fn trace_error(code: u64) {
    trace(b'!');
    for shift in [4, 0] {
        let nibble = ((code >> shift) & 15) as u8;
        trace(if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 });
    }
}

fn trace_detail(error: &impl core::fmt::Debug) {
    #[cfg(feature = "native-resident-test")]
    {
        use core::fmt::Write;
        struct Bounded(usize);
        impl Write for Bounded {
            fn write_str(&mut self, value: &str) -> core::fmt::Result {
                if value.len() > self.0 {
                    return Err(core::fmt::Error);
                }
                self.0 -= value.len();
                for byte in value.bytes() {
                    trace(byte);
                }
                Ok(())
            }
        }
        trace(b'[');
        let _ = write!(&mut Bounded(192), "{error:?}");
        trace(b']');
    }
    #[cfg(not(feature = "native-resident-test"))]
    let _ = error;
}

fn unsupported(code: u64) -> Status {
    trace_error(code);
    Status::UNSUPPORTED
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}
unsafe fn wrmsr(msr: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack, preserves_flags));
    }
}

// CPUID-gated MSRs only. AMD APM2 15.4 VM_CR.SVMDIS and feature leaves.
unsafe fn cpu() -> Result<Cpu, u64> {
    let basic = __cpuid_count(0, 0);
    let extended = __cpuid_count(0x80000000, 0);
    if basic.eax < 1
        || basic.ebx != 0x68747541
        || basic.edx != 0x69746e65
        || basic.ecx != 0x444d4163
        || extended.eax < 0x8000000a
    {
        let (item, observed, expected): (u64, u32, u32) = if basic.eax < 1 {
            (0, basic.eax, 1)
        } else if basic.ebx != 0x68747541 {
            (1, basic.ebx, 0x68747541)
        } else if basic.edx != 0x69746e65 {
            (2, basic.edx, 0x69746e65)
        } else if basic.ecx != 0x444d4163 {
            (3, basic.ecx, 0x444d4163)
        } else {
            (4, extended.eax, 0x8000000a)
        };
        admission_hint(101, item, observed as u64, expected as u64);
        return Err(1);
    }
    let one = __cpuid_count(1, 0);
    #[cfg(feature = "native-resident-smp-activate")]
    physical::admission_cpu_id(one.ebx >> 24);
    let ext = __cpuid_count(0x80000001, 0);
    let svm = __cpuid_count(0x8000000a, 0);
    if svmvisor_hypervisor::svm::x2avic::X2AvicCapabilities::admit(one.ecx, svm.edx).is_err() {
        if one.ecx & (1 << 21) == 0 {
            admission_hint(157, 1, one.ecx as u64, 1 << 21);
        } else {
            admission_hint(
                157,
                0x8000000a,
                svm.edx as u64,
                (1 | (1 << 13) | (1 << 18) | (1 << 25)) as u64,
            );
        }
        return Err(2);
    }
    // Preserve the loader-selected interface; never silently promote xAPIC
    // after the loader has chosen its register access method (APM2 16.10).
    let apic_base = unsafe { rdmsr(apic::APIC_BASE) };
    if apic_base & apic::APIC_BASE_X2APIC != apic::APIC_BASE_X2APIC {
        admission_hint(158, apic::APIC_BASE as u64, apic_base, apic::APIC_BASE_X2APIC);
        return Err(2);
    }
    if one.ecx & (1 << 31) != 0
        || one.edx & 0x07011020 != 0x07011020
        || ext.ecx & 4 == 0
        || ext.edx & 0x24100000 != 0x24100000
        || svm.edx & 1 != 1
        || svm.ebx < 2
        || svm.eax != 1
    {
        trace_detail(&("cpuid", one.edx, one.ecx, ext.ecx, ext.edx, svm.eax, svm.ebx, svm.edx));
        let (item, observed, expected) = if one.ecx & (1 << 31) != 0 {
            (0, one.ecx, 0)
        } else if one.edx & 0x07011020 != 0x07011020 {
            (1, one.edx, 0x07011020)
        } else if ext.ecx & 4 == 0 {
            (2, ext.ecx, 4)
        } else if ext.edx & 0x24100000 != 0x24100000 {
            (3, ext.edx, 0x24100000)
        } else if svm.edx & 1 != 1 {
            (4, svm.edx, 1)
        } else if svm.ebx < 2 {
            (5, svm.ebx, 2)
        } else {
            (6, svm.eax, 1)
        };
        admission_hint(102, item, observed as u64, expected as u64);
        return Err(2);
    }
    let width = __cpuid_count(0x80000008, 0).eax as u8;
    let leaf = (extended.eax >= 0x8000001f).then(|| {
        let enc = __cpuid_count(0x8000001f, 0);
        [enc.eax, enc.ebx, enc.ecx, enc.edx]
    });
    let plan = svmvisor_hypervisor::arch::x86_64::encryption::NativeEncryptionPlan::new(
        one.eax, width, leaf,
    )
    .map_err(|error| {
        trace_detail(&("encryption", error));
        if !(32..=52).contains(&width) {
            admission_hint(104, 0x80000008, width as u64, 32 | (52u64 << 32));
        } else if one.eax == TARGET_SIGNATURE
            && width != TARGET_PHYSICAL_BITS
            && leaf.is_some_and(|v| v.iter().any(|&x| x != 0))
        {
            admission_hint(156, 0x80000008, width as u64, TARGET_PHYSICAL_BITS as u64);
        } else {
            let values = leaf.unwrap_or([0; 4]);
            admission_hint(
                103,
                one.eax as u64,
                values[0] as u64 | ((values[1] as u64) << 32),
                values[2] as u64 | ((values[3] as u64) << 32),
            );
        }
        3u64
    })?;
    let sys_cfg = plan.sys_cfg_msr().map(|msr| unsafe { rdmsr(msr) });
    let sev_status = plan.sev_status_msr().map(|msr| unsafe { rdmsr(msr) });
    let encryption = plan.validate(sys_cfg, sev_status).map_err(|error| {
        trace_detail(&("encryption", error));
        let allowed = SYS_CFG_DEFINED & !SYS_CFG_ENCRYPTION;
        if sys_cfg.is_some_and(|v| v & !allowed != 0) {
            admission_hint(153, SYS_CFG as u64, sys_cfg.unwrap(), allowed);
        } else {
            admission_hint(154, 0xc0010131, sev_status.unwrap_or(0), 0);
        }
        3u64
    })?;
    if !(32..=52).contains(&width) {
        admission_hint(104, 0x80000008, width as u64, 32 | (52u64 << 32));
        return Err(4);
    }
    let vm_cr = unsafe { rdmsr(VM_CR) };
    if vm_cr & VM_CR_SVMDIS != 0 {
        admission_hint(151, VM_CR as u64, vm_cr, VM_CR_SVMDIS);
        return Err(4);
    }
    let efer = unsafe { rdmsr(EFER) };
    if efer & (1 << 12) != 0 {
        admission_hint(152, EFER as u64, efer, 1 << 12);
        return Err(4);
    }
    Ok(Cpu { physical_bits: width, apic_id: one.ebx >> 24, encryption })
}

/// CPU/encryption admission precedes this capture. Other admitted profiles
/// retain architectural default behavior.
unsafe fn mtrrs(physical_bits: u8) -> Result<Mtrrs, u64> {
    use svmvisor_hypervisor::memory::mtrrs::{MAX_VARIABLE, MtrrReadError};
    Mtrrs::read(physical_bits, __cpuid_count(1, 0).eax, |index| unsafe { rdmsr(index) }).map_err(
        |error| {
            match error {
                MtrrReadError::VariableCount { capability } => {
                    admission_hint(105, MTRR_CAP as u64, capability, MAX_VARIABLE as u64)
                }
                MtrrReadError::Tom2 { error, sys_cfg, tom2 } => {
                    trace_detail(&("tom2", error));
                    admission_hint(155, TOM2 as u64, tom2, sys_cfg);
                }
            }
            5
        },
    )
}

/// Actual same-CPU cache evidence; permitted MSRs and temporary thread-private
/// SYS_CFG19 visibility follow PPR57896 rev3.00 pp123,126-130,173,202-206,210.
/// Called only after native CPU/encryption admission and before guest entry.
unsafe fn cache_observation_detailed(
    processor: Cpu,
) -> Result<
    svmvisor_hypervisor::svm::native_cache::CacheObservation,
    svmvisor_hypervisor::svm::native_cache::CacheAdmissionFailure,
> {
    use svmvisor_hypervisor::svm::native_cache::{CacheObservation, native_topology_detailed};
    CacheObservation::capture_detailed(
        __cpuid_count(1, 0).eax,
        processor.physical_bits,
        native_topology_detailed(),
        |index| unsafe { rdmsr(index) },
        |index, value| unsafe { wrmsr(index, value) },
    )
}

unsafe fn config(cpu: Cpu) -> Result<PagingConfig, u64> {
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    native_paging_config(cr0, cr3, cr4, cpu.physical_bits, unsafe { rdmsr(EFER) } & (1 << 11) != 0)
        .ok_or_else(|| {
            trace_detail(&("controls", cr0, cr3, cr4));
            let (item, observed, expected) = if cr0 & 0x80000011 != 0x80000011 {
                (0, cr0, 0x80000011)
            } else if cr0 & 0x60000000 != 0 {
                (0, cr0, 0)
            } else if !svmvisor_hypervisor::guest::continuation::native_cr4_supported(cr4) {
                (4, cr4, 0)
            } else {
                (3, cr3, 0)
            };
            admission_hint(106, item, observed, expected);
            6
        })
}

// Map type 7 may have been allocated by firmware since installation. This
// reader uses it only as retained physical-RAM capability, never as current
// ownership. The caller's native identity/coherency/handler cooperation contract
// supplies safe physical reads. Current translations are checked at each use.
fn ram_span(map: &[MemoryDescriptor], base: u64, bytes: u64) -> bool {
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

unsafe fn mapped(
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
unsafe fn validate_uc_mmio(
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

/// Prepared directories of every admitted slot. Written once by the serialized
/// installer before READY; later readers only copy.
unsafe fn directories() -> &'static [ResidentDirectory] {
    unsafe { core::slice::from_raw_parts(ptr::addr_of!(DIRECTORIES).cast(), CPU_COUNT) }
}

// Check the actual prepared host closure of `directories[slot]` through its
// private root before the runtime selects that root. `directories` is the
// complete prepared pool. Current native mappings already cover every arena,
// making these retained reads safe under the initial identity contract.
unsafe fn host_closure(
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

// Preparation diagnostics belong to the serialized BSP installer only. Other
// build profiles retain their existing behavior and do not access the card.
fn preparation_step(stage: u32, address: u64) {
    #[cfg(feature = "native-resident-boot")]
    card_boot::preparation_step(stage, address);
    #[cfg(not(feature = "native-resident-boot"))]
    let _ = (stage, address);
}
fn preparation_failure(reason: u32, status: u64, address: u64) {
    #[cfg(feature = "native-resident-boot")]
    card_boot::preparation_failure(reason, status, address);
    #[cfg(not(feature = "native-resident-boot"))]
    let _ = (reason, status, address);
}
// Stable numeric codes of the stage-16/17 refusals that the preparation record
// (reasons 34-36) and the admission-hint recorder (predicates 635-637) carry;
// read_snapshot.py names them. Enum order is not wire format; this table is.
fn address_error_code(error: svmvisor_hypervisor::memory::address::AddressError) -> u64 {
    use svmvisor_hypervisor::memory::address::AddressError::*;
    match error {
        UnsupportedPhysicalWidth => 1,
        UnknownEncryption => 2,
        ActiveEncryptionUnsupported => 3,
        InvalidEncryptionBit => 4,
        EmptyRange => 5,
        InvalidAlignment => 6,
        Misaligned => 7,
        Overflow => 8,
        OutsidePhysicalWidth => 9,
        EncryptionBitEncoded => 10,
    }
}
fn identity_npt_error_code(error: svmvisor_hypervisor::memory::npt::IdentityNptError) -> u64 {
    use svmvisor_hypervisor::memory::npt::IdentityNptError::*;
    match error {
        RequiredModeNotEstablished => 1,
        OneGiBPagesNotEstablished => 2,
        PatZeroNotWriteBack => 3,
        InvalidExclusion => 4,
        TableArenaOutsideExclusion => 5,
        GuestAddressOutsideWidth => 6,
        StorageBounds => 7,
        Address(error) => 16 + address_error_code(error),
    }
}
fn resident_memory_error_code(error: resident::memory::ResidentMemoryError) -> u64 {
    use resident::memory::ResidentMemoryError::*;
    use svmvisor_hypervisor::boot::memory::MemoryError as M;
    match error {
        MonitorUncovered => 1,
        MonitorNotRuntime => 2,
        MonitorNotWriteBack => 3,
        Map(error) => {
            32 + match error {
                M::DescriptorCount => 1,
                M::PhysicalWidth => 2,
                M::EmptyDescriptor => 3,
                M::MisalignedDescriptor => 4,
                M::Overflow => 5,
                M::OutsidePhysicalWidth => 6,
                M::UnsortedOrOverlapping => 7,
                M::EmptyRange => 8,
                M::MisalignedEntry => 9,
                M::CopyTooLarge => 10,
                M::UncoveredRange => 11,
                M::UntrustedMemoryType => 12,
                M::ReadProtected => 13,
                M::MissingWriteBackCapability => 14,
                M::ReadFailed => 15,
                M::MonitorOverlap => 16,
            }
        }
        Npt(error) => 64 + identity_npt_error_code(error),
    }
}
fn preparation_map_failure(error: memory::MemoryMapError) -> Status {
    use memory::MemoryMapError::*;
    let (reason, status) = match error {
        Firmware(status) => (16, status.0 as u64),
        Bounds => (17, 0),
        Layout => (18, 0),
        RetryLimit => (19, 0),
        Cleanup(status) => (20, status.0 as u64),
        Released => (21, 0),
    };
    preparation_failure(reason, status, 0);
    Status::OUT_OF_RESOURCES
}

/// Install retained payload and the selected event, diagnostic seam or EBS interposer once.
/// # Safety
/// These are the live runtime-driver entry arguments. Firmware provides trusted
/// x64 identity mappings, coherent page tables, a flat hidden-segment profile,
/// no debugger/NMI/SMM interference during capture, and this single BSP owns
/// the eventual resident interval. No physical launch is implied by the test
/// profile: AMD routing/encryption profiles outside this admission are refused.
pub(crate) unsafe fn install(image: Handle, table: *mut SystemTable) -> Status {
    trace(b'I');
    if table.is_null() || INSTALLED.swap(true, Ordering::AcqRel) {
        return Status::UNSUPPORTED;
    }
    let bs = unsafe { (*table).boot_services };
    if bs.is_null() {
        return Status::INVALID_PARAMETER;
    }
    #[cfg(feature = "native-resident-boot")]
    let card = match unsafe { card_boot::Prepared::new(image, &*bs) } {
        Ok(value) => value,
        Err(status) => return status,
    };
    #[cfg(feature = "native-resident-boot")]
    let handoff = match {
        preparation_step(1, bs as u64);
        unsafe { boot_handoff::Prepared::new(bs) }
    } {
        Ok(value) => value,
        Err(status) => {
            if let Some(card) = card {
                card.complete(status, false);
            }
            return status;
        }
    };
    let result = unsafe { install_inner(image, &*bs) };
    #[cfg(feature = "native-resident-boot")]
    if result.is_ok() && READY.load(Ordering::Acquire) {
        unsafe { handoff.commit(bs) };
        unsafe { card_boot::stage(1, 0, 0) };
    }
    #[cfg(feature = "native-resident-boot")]
    if let Some(card) = card {
        card.complete(
            result.as_ref().err().copied().unwrap_or(Status::PROTOCOL_ERROR),
            result.is_ok() && READY.load(Ordering::Acquire),
        );
    }
    match result {
        Ok(()) => {
            trace(if READY.load(Ordering::Acquire) { b'R' } else { b'J' });
            Status::SUCCESS
        }
        Err(status) => {
            trace(b'F');
            trace_detail(&status);
            status
        }
    }
}

unsafe fn install_inner(image: Handle, bs: &BootServices) -> Result<(), Status> {
    trace(b'a');
    preparation_step(2, 0);
    let processor = unsafe { cpu() }.map_err(unsupported)?;
    trace(b'b');
    preparation_step(3, 0);
    let inventory = unsafe { resident::processors::inspect(bs) }.map_err(|error| {
        trace_detail(&error);
        Status::UNSUPPORTED
    })?;
    let count = inventory.processors().len();
    #[cfg(not(any(
        feature = "native-resident-smp-prepare",
        feature = "native-resident-smp-activate"
    )))]
    if count != 1 {
        return Err(Status::UNSUPPORTED);
    }
    let bsp = inventory.bsp_number();
    let common = inventory.processors()[bsp].identity;
    if common.apic_id != processor.apic_id {
        return Err(Status::UNSUPPORTED);
    }
    for p in inventory.processors() {
        let mut identity = p.identity;
        // Identity fields are CPU-local; all common requirements must agree.
        // Host APIC IDs above 254 are refused: the AVIC doorbell ID field and
        // x2AVIC table entry 255 (see abi::valid_pool_slot).
        identity.apic_id = common.apic_id;
        if identity != common || p.identity.apic_id > 254 {
            return Err(Status::UNSUPPORTED);
        }
    }
    trace(b'c');
    preparation_step(4, 0);
    let mut loaded = ptr::null_mut();
    let status = unsafe { (bs.handle_protocol)(image, &LoadedImageProtocol::GUID, &mut loaded) };
    if status != Status::SUCCESS {
        return Err(status);
    }
    if loaded.is_null() || loaded as usize % core::mem::align_of::<LoadedImageProtocol>() != 0 {
        return Err(Status::LOAD_ERROR);
    }
    let loaded = unsafe { &*loaded.cast::<LoadedImageProtocol>() };
    let image_base = loaded.image_base as u64;
    if loaded.image_code_type != MemoryType::RUNTIME_SERVICES_CODE
        || loaded.image_data_type != MemoryType::RUNTIME_SERVICES_DATA
        || image_base == 0
        || image_base & 4095 != 0
        || loaded.image_size == 0
        || loaded.image_size > 16 * 1024 * 1024
        || image_base.checked_add(loaded.image_size).is_none()
    {
        return Err(Status::UNSUPPORTED);
    }
    unsafe {
        IMAGE = (image_base, loaded.image_size);
    }
    trace(b'd');
    preparation_step(5, image_base);
    let package = Payload::parse(PACKAGE, RESIDENT_ENTRY_OFFSET).map_err(|error| {
        trace_detail(&error);
        Status::LOAD_ERROR
    })?;
    trace(b'e');
    preparation_step(6, 0);
    let mut arena = unsafe { allocation::allocate_for_processors(bs, count) }.map_err(|error| {
        trace_detail(&error);
        let (reason, status, address) = error.diagnostic();
        preparation_failure(reason, status, address);
        Status::OUT_OF_RESOURCES
    })?;
    trace(b'f');
    preparation_step(7, arena.base());
    let cfg = unsafe { config(processor) }.map_err(unsupported)?;
    trace(b'g');
    preparation_step(8, arena.base());
    let mt = unsafe { mtrrs(processor.physical_bits) }.map_err(unsupported)?;
    let pat = unsafe { rdmsr(PAT) };
    let policy = AddressPolicy::new(processor.physical_bits, processor.encryption)
        .map_err(|_| Status::UNSUPPORTED)?;
    {
        trace(b'h');
        preparation_step(9, 0);
        let mut map = unsafe { memory::collect(bs) }.map_err(|error| {
            trace_detail(&error);
            preparation_map_failure(error)
        })?;
        trace(b'i');
        preparation_step(10, arena.base());
        arena.validate_map(policy, map.descriptors()).map_err(|error| {
            trace_detail(&error);
            Status::UNSUPPORTED
        })?;
        trace(b'j');
        for slot in 0..count {
            preparation_step(11, arena.slot_base(slot).unwrap_or(0));
            unsafe {
                mapped(
                    map.descriptors(),
                    cfg,
                    &mt,
                    pat,
                    arena.slot_base(slot).ok_or(Status::LOAD_ERROR)?,
                    0x100000,
                    true,
                    true,
                )
            }
            .map_err(unsupported)?;
            trace(b'k');
            preparation_step(12, arena.slot_base(slot).unwrap_or(0));
            unsafe { arena.initialize_slot(&package, slot) }.map_err(|error| {
                trace_detail(&error);
                Status::LOAD_ERROR
            })?;
        }
        map.release()?;
    }
    let mut directories = [ResidentDirectory::default(); abi::MAX_RESIDENT_CPUS];
    #[cfg(feature = "native-resident-smp-activate")]
    let mut physical_storage = {
        preparation_step(13, 0);
        unsafe { physical::prepare(bs, bsp, count, cfg)? }
    };
    trace(b'l');
    for (slot, directory) in directories.iter_mut().enumerate().take(count) {
        let base = arena.slot_base(slot).ok_or(Status::LOAD_ERROR)?;
        preparation_step(14, base);
        let prepare: abi::PrepareRuntime =
            unsafe { core::mem::transmute(base as usize + RESIDENT_ENTRY_OFFSET) };
        let prepared = unsafe {
            prepare(
                base,
                directory,
                arena.base(),
                arena.bytes() as u64,
                slot as u64,
                inventory.processors()[slot].identity.apic_id as u64,
            )
        };
        if prepared != 0
            || !directory_valid(directory, base)
            || base + RESIDENT_ENTRY_OFFSET as u64 >= directory.text_end
        {
            trace_detail(&prepared);
            trace_detail(&directory);
            return Err(Status::LOAD_ERROR);
        }
    }
    // Every private root aliases each slot's backing at slot base plus one
    // common image offset; refuse a pool whose copies disagree.
    if common_backing_offset(&directories[..count]).is_none() {
        trace_detail(&("backing-offset", count));
        return Err(Status::LOAD_ERROR);
    }
    // Hardware's physical-ID table has one excluded WB backing. No processor
    // has entered yet, so all valid entries can be constructed before publication.
    {
        use svmvisor_hypervisor::svm::x2avic::PhysicalIdTable;
        let table = (arena.base() + abi::X2AVIC_TABLE_OFFSET) as *mut PhysicalIdTable;
        unsafe {
            table.write(PhysicalIdTable::new());
        }
        for d in &directories[..count] {
            unsafe { (&mut *table).insert_stopped(d.apic_id as u16, d.avic_backing, &policy) }
                .map_err(|_| Status::UNSUPPORTED)?;
        }
    }
    // Capture storage has one pool-owned backing and read-only host aliases.
    // Native boot populates it after successful EBS return; firmware can still
    // synchronize MTRRs in EBS callbacks. Every owned CPU samples before entry.
    {
        use svmvisor_hypervisor::svm::native_cache::CacheCapture;
        let capture = (arena.base() + abi::CACHE_CAPTURE_OFFSET) as *mut CacheCapture;
        unsafe {
            capture.write(CacheCapture::empty());
        }
        unsafe {
            ((arena.base() + abi::CACHE_OWNER_OFFSET)
                as *mut svmvisor_hypervisor::svm::native_cache::CacheOwner)
                .write(svmvisor_hypervisor::svm::native_cache::CacheOwner::empty());
        }
        #[cfg(feature = "native-resident-boot")]
        if __cpuid_count(1, 0).eax == TARGET_SIGNATURE {
            if !unsafe { (&mut *capture).initialize(count) } {
                return Err(Status::LOAD_ERROR);
            }
        }
    }
    #[cfg(feature = "native-resident-smp-activate")]
    {
        // Separate fixed block follows all 32 mailboxes; initialize before any
        // AP can enter. It is not a guest-writable acknowledgement buffer.
        let terminal_control =
            (arena.base() + abi::STARTUP_PAGE_OFFSET + abi::terminal::CONTROL_OFFSET)
                as *mut abi::terminal::TerminalControl;
        unsafe {
            terminal_control.write(abi::terminal::TerminalControl::new());
        }
    }
    #[cfg(feature = "native-resident-guest-startup")]
    {
        use svmvisor_hypervisor::svm::x2avic::startup::NativeStartupMailbox;
        const _: () = assert!(
            core::mem::size_of::<NativeStartupMailbox>() * abi::MAX_RESIDENT_CPUS
                <= abi::terminal::CONTROL_OFFSET as usize
        );
        let shared = (arena.base() + abi::STARTUP_PAGE_OFFSET) as *mut NativeStartupMailbox;
        for slot in 0..count {
            // The admitted pool is exclusively DXE-owned before activation.
            // Constructors publish Assigned slots; the target later marks ready.
            unsafe {
                shared.add(slot).write(NativeStartupMailbox::new(
                    inventory.processors()[slot].identity.apic_id,
                ));
            }
        }
    }
    #[cfg(feature = "native-resident-smp-prepare")]
    {
        preparation_step(15, 0);
        let mut map = unsafe { memory::collect(bs) }.map_err(preparation_map_failure)?;
        arena.validate_map(policy, map.descriptors()).map_err(|_| Status::UNSUPPORTED)?;
        let pool = policy
            .validate(arena.base(), arena.bytes() as u64, 4096)
            .map_err(|_| Status::UNSUPPORTED)?;
        for (slot, d) in directories.iter().enumerate().take(count) {
            unsafe {
                host_closure(&directories[..count], slot, map.descriptors(), processor, &mt, pat)
            }
            .map_err(unsupported)?;
            let storage = unsafe { &mut *(d.npt as *mut TableStorage) };
            let mut npt = resident::memory::prepare_identity_npt(
                storage,
                d.npt,
                policy,
                pool,
                map.descriptors(),
                NptEvidence {
                    nx_supported: EvidenceFlag::Set,
                    host_nxe: EvidenceFlag::Set,
                    host_four_level: EvidenceFlag::Set,
                },
                EvidenceFlag::Set,
                pat,
            )
            .map_err(|error| {
                trace_detail(&error);
                Status::UNSUPPORTED
            })?;
            #[cfg(feature = "native-resident-boot")]
            card_boot::protect_config(&mut npt).map_err(|_| Status::UNSUPPORTED)?;
            for other in directories.iter().take(count) {
                if npt.translate(other.arena_base).map_err(|_| Status::LOAD_ERROR)?.is_some()
                    || npt
                        .translate(other.arena_base + 0xfffff)
                        .map_err(|_| Status::LOAD_ERROR)?
                        .is_some()
                {
                    return Err(Status::LOAD_ERROR);
                }
            }
            trace_detail(&(
                "private-slot",
                slot,
                d.apic_id,
                d.arena_base,
                d.context,
                d.vmcb,
                d.npt,
            ));
        }
        map.release()?;

        let retained = arena.publish().map_err(|_| Status::DEVICE_ERROR)?;
        trace_detail(&(
            "native-smp-prepared",
            count,
            inventory.completed_ap_callbacks(),
            bsp,
            retained.base(),
            retained.bytes(),
        ));
        return Ok(());
    }
    #[cfg(feature = "native-resident-smp-activate")]
    {
        preparation_step(15, 0);
        let mut map = unsafe { memory::collect(bs) }.map_err(preparation_map_failure)?;
        arena.validate_map(policy, map.descriptors()).map_err(|_| Status::UNSUPPORTED)?;
        ValidatedMemoryMap::new(map.descriptors(), processor.physical_bits.min(40))
            .map_err(|_| Status::UNSUPPORTED)?;
        // The admission-hint recorder is armed around each slot's closure and
        // NPT work (operations 9/10) so a refusal names its predicate and code
        // on the card without any test feature; see `slot_admission_refused`.
        for (slot, d) in directories.iter().enumerate().take(count) {
            preparation_step(16, d.arena_base);
            physical::admission_begin(9, slot as u32, d.apic_id as u32);
            unsafe {
                host_closure(&directories[..count], slot, map.descriptors(), processor, &mt, pat)
            }
            .map_err(|code| unsafe {
                physical::slot_admission_refused(33, count, code, processor, map.descriptors())
            })?;
            let pool = policy.validate(d.pool_base, d.pool_bytes, 4096).map_err(|error| {
                let code = address_error_code(error);
                admission_hint(635, d.pool_base, d.pool_bytes, code);
                unsafe {
                    physical::slot_admission_refused(34, count, code, processor, map.descriptors())
                }
            })?;
            physical::admission_clear();
            preparation_step(17, d.arena_base);
            physical::admission_begin(10, slot as u32, d.apic_id as u32);
            let mut _npt = resident::memory::prepare_identity_npt(
                unsafe { &mut *(d.npt as *mut TableStorage) },
                d.npt,
                policy,
                pool,
                map.descriptors(),
                NptEvidence {
                    nx_supported: EvidenceFlag::Set,
                    host_nxe: EvidenceFlag::Set,
                    host_four_level: EvidenceFlag::Set,
                },
                EvidenceFlag::Set,
                pat,
            )
            .map_err(|error| {
                let code = resident_memory_error_code(error);
                admission_hint(636, d.npt, d.pool_base, code);
                unsafe {
                    physical::slot_admission_refused(35, count, code, processor, map.descriptors())
                }
            })?;
            #[cfg(feature = "native-resident-boot")]
            card_boot::protect_config(&mut _npt).map_err(|error| {
                let code = identity_npt_error_code(error);
                admission_hint(637, d.npt, d.pool_base, code);
                unsafe {
                    physical::slot_admission_refused(36, count, code, processor, map.descriptors())
                }
            })?;
            physical::admission_clear();
        }
        preparation_step(18, 0);
        unsafe { physical::validate(map.descriptors(), cfg, &mt, pat, count) }
            .map_err(unsupported)?;
        unsafe {
            ptr::copy_nonoverlapping(
                map.descriptors().as_ptr(),
                ptr::addr_of_mut!(MAP).cast(),
                map.descriptors().len(),
            );
            MAP_COUNT = map.descriptors().len();
            DIRECTORIES = directories;
            CPU_COUNT = count;
            CPU = Some(processor);
            for (slot, p) in inventory.processors().iter().enumerate() {
                CPU_IDS[slot] = p.identity.apic_id;
            }
        }
        map.release()?;
        preparation_step(19, 0);
        unsafe {
            physical::admit_processors(bs)?;
        }
        preparation_step(20, arena.base());
        let _retained = arena.register_and_publish(|base, bytes| unsafe {
            physical::publish(bs, count, base, bytes as u64)
        })?;
        physical_storage.retain();
        READY.store(true, Ordering::Release);
        return Ok(());
    }
    #[allow(unreachable_code)]
    let prepared = &directories[..count];
    trace(b'm');
    let mut event = ptr::null_mut();
    let mut group = guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b");
    let status = unsafe {
        (bs.create_event_ex)(
            EventType::NOTIFY_SIGNAL,
            Tpl::NOTIFY,
            Some(abi::svmvisor_resident_callback),
            COOKIE as *mut c_void,
            &mut group,
            &mut event,
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    if event.is_null() {
        return Err(Status::DEVICE_ERROR);
    }
    let finish = (|| {
        trace(b'n');
        let mut map = unsafe { memory::collect(bs) }.map_err(|error| {
            trace_detail(&error);
            preparation_map_failure(error)
        })?;
        arena.validate_map(policy, map.descriptors()).map_err(|error| {
            trace_detail(&error);
            Status::UNSUPPORTED
        })?;
        ValidatedMemoryMap::new(map.descriptors(), processor.physical_bits.min(40)).map_err(
            |error| {
                trace_detail(&error);
                Status::UNSUPPORTED
            },
        )?;
        trace(b'o');
        unsafe { host_closure(prepared, bsp, map.descriptors(), processor, &mt, pat) }
            .map_err(unsupported)?;
        unsafe {
            ptr::copy_nonoverlapping(
                map.descriptors().as_ptr(),
                ptr::addr_of_mut!(MAP).cast(),
                map.descriptors().len(),
            );
            MAP_COUNT = map.descriptors().len();
            DIRECTORIES = directories;
            CPU_COUNT = count;
            CPU_IDS[0] = processor.apic_id;
            CPU = Some(processor);
        }
        map.release()?;
        Ok(())
    })();
    if let Err(error) = finish {
        let close = unsafe { (bs.close_event)(event) };
        if close != Status::SUCCESS {
            // Returning an EFI error permits image unload while the event can
            // still call its assembly. Retain both image and raw allocation as
            // an inert success instead, leave READY=false and expose failure.
            let _retained = arena.publish().map_err(|_| Status::DEVICE_ERROR)?;
            trace(b'X');
            return Ok(());
        }
        return Err(error);
    }
    // The event may now hold numeric pointers, but READY kept it inert across
    // registration and map collection. There are no fallible firmware calls
    // after publication. Refused callbacks retain the inert arena until reset.
    trace(b'p');
    let _retained = arena.publish().map_err(|_| Status::DEVICE_ERROR)?;
    READY.store(true, Ordering::Release);
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "efiapi" fn svmvisor_resident_callback_inner(
    _event: Event,
    context: *mut c_void,
    boundary: *const NativeBoundary,
) -> u64 {
    if context as usize != COOKIE || boundary.is_null() || !READY.load(Ordering::Acquire) {
        return 1;
    }
    let id = __cpuid_count(1, 0).ebx >> 24;
    let ids =
        unsafe { core::slice::from_raw_parts(ptr::addr_of!(CPU_IDS).cast::<u32>(), CPU_COUNT) };
    let Some(slot) = ids.iter().position(|value| *value == id) else {
        return 1;
    };
    if ACTIVATING.fetch_or(1u32 << slot, Ordering::AcqRel) & (1u32 << slot) != 0 {
        return 1;
    }
    trace(b'C');
    match unsafe { callback(&*boundary, slot) } {
        Ok(()) => 0,
        Err(code) => {
            #[cfg(feature = "native-resident-boot")]
            if unsafe { physical::is_bsp(slot) } {
                unsafe {
                    card_boot::takeover_failure(slot as u32, CPU_COUNT as u32, code);
                }
            }
            trace_error(code);
            code
        }
    }
}

unsafe fn callback(b: &NativeBoundary, slot: usize) -> Result<(), u64> {
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    }
    if flags & (1 << 9) != 0 {
        return Err(23);
    }
    let processor = unsafe { cpu() }?;
    let mut expected = unsafe { CPU }.ok_or(10u64)?;
    expected.apic_id = unsafe { CPU_IDS[slot] };
    if processor != expected || !xstate_valid(b) {
        return Err(10);
    }
    let d = unsafe { DIRECTORIES[slot] };
    let map = unsafe {
        core::slice::from_raw_parts(ptr::addr_of!(MAP).cast::<MemoryDescriptor>(), MAP_COUNT)
    };
    let cfg = unsafe { config(processor) }?;
    if cfg.cr3 != b.cr3 {
        return Err(12);
    }
    let cr0: u64;
    let cr4: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    if cr0 != b.cr0 || cr4 != b.cr4 {
        return Err(12);
    }
    let mt = unsafe { mtrrs(processor.physical_bits) }?;
    let pat = unsafe { rdmsr(PAT) };
    let policy =
        AddressPolicy::new(processor.physical_bits, processor.encryption).map_err(|_| 13u64)?;
    unsafe { mapped(map, cfg, &mt, pat, d.arena_base, d.arena_bytes, true, true) }?;
    unsafe { host_closure(directories(), slot, map, processor, &mt, pat) }?;
    let current_rsp: u64;
    unsafe {
        asm!("mov {}, rsp", out(reg) current_rsp, options(nomem, nostack, preserves_flags));
    }
    let stack_base = current_rsp.checked_sub(64 * 1024).ok_or(14u64)?;
    let stack_end = b.entry_rsp.checked_add(40).ok_or(14u64)?;
    unsafe {
        mapped(
            map,
            cfg,
            &mt,
            pat,
            stack_base,
            stack_end.checked_sub(stack_base).ok_or(14u64)?,
            true,
            false,
        )
    }?;
    #[cfg(feature = "native-resident-boot")]
    unsafe { physical::cache_sample_before_activation(processor, slot) }?;
    let sites = CallbackSites {
        resume: ptr::addr_of!(abi::svmvisor_resident_guest_resume) as u64,
        ack: ptr::addr_of!(abi::svmvisor_resident_guest_ack) as u64,
        after_ack: ptr::addr_of!(abi::svmvisor_resident_guest_after_ack) as u64,
    };
    let (image, image_bytes) = unsafe { IMAGE };
    if sites.resume < image
        || sites.after_ack.checked_add(256).is_none_or(|end| end > image + image_bytes)
    {
        return Err(21);
    }
    unsafe {
        mapped(map, cfg, &mt, pat, sites.resume, 256, false, true)?;
        // The bounded post-ACK epilogue uses this immutable admitted identity
        // table and a shared runtime-image completion word. All remain outside
        // the private monitor pool and are rewalked under each captured root.
        mapped(map, cfg, &mt, pat, ptr::addr_of!(CPU_COUNT) as u64, 8, false, false)?;
        mapped(
            map,
            cfg,
            &mt,
            pat,
            ptr::addr_of!(CPU_IDS) as u64,
            core::mem::size_of::<[u32; abi::MAX_RESIDENT_CPUS]>() as u64,
            false,
            false,
        )?;
        mapped(map, cfg, &mt, pat, ptr::addr_of!(GUEST_ACK) as u64, 4, true, false)?;
        mapped(map, cfg, &mt, pat, b.entry_rip, 1, false, true)?;
        mapped(map, cfg, &mt, pat, b.gdtr.base(), u64::from(b.gdtr.limit()) + 1, true, false)?;
        mapped(map, cfg, &mt, pat, b.idtr.base(), u64::from(b.idtr.limit()) + 1, false, false)?;
        mapped(
            map,
            cfg,
            &mt,
            pat,
            ptr::addr_of!(MAP) as u64,
            core::mem::size_of::<[MemoryDescriptor; 4096]>() as u64,
            false,
            false,
        )?;
        mapped(map, cfg, &mt, pat, ptr::addr_of!(GDT) as u64, 65536, true, false)?;
    }
    let linked = unsafe { core::slice::from_raw_parts(sites.resume as *const u8, 8) };
    if linked != [0xb8, 0x41, 0x4d, 0x56, 0x53, 0x0f, 0x01, 0xd9] {
        return Err(22);
    }
    let gdt_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            ptr::addr_of_mut!(GDT).cast::<u8>(),
            usize::from(b.gdtr.limit()) + 1,
        )
    };
    unsafe {
        ptr::copy_nonoverlapping(
            b.gdtr.base() as *const u8,
            gdt_bytes.as_mut_ptr(),
            gdt_bytes.len(),
        );
    }
    let gdt = parse_firmware_gdt(
        HostTablePointer { base: b.gdtr.base(), limit: b.gdtr.limit() },
        FirmwareSelectors { cs: b.cs, ss: b.ss, ds: b.ds, es: b.es },
        gdt_bytes,
    )
    .map_err(|_| 15u64)?;
    let original_efer = unsafe { rdmsr(EFER) };
    if original_efer != b.efer {
        return Err(16);
    }
    // Read the executing CPU's feature evidence before admitting its captured
    // EFER. Boundary capture separately refuses FFXSR to retain all XMM state.
    let maximum_extended = __cpuid_count(0x8000_0000, 0).eax;
    if maximum_extended < 0x8000_0008 {
        return Err(16);
    }
    let extended = __cpuid_count(0x8000_0001, 0);
    let extended8 = __cpuid_count(0x8000_0008, 0);
    let extended21 = if maximum_extended >= 0x8000_0021 {
        Some(__cpuid_count(0x8000_0021, 0).eax)
    } else {
        None
    };
    let efer = svmvisor_hypervisor::svm::dispatch::NativeEfer::admit_native(
        original_efer,
        extended.ecx,
        extended.edx,
        extended8.ebx,
        extended21,
    )
    .map_err(|_| 16u64)?;
    // APM2 VMSAVE requires SVME. No fallible operation or Rust return exists
    // between this temporary enable and exact restoration; HSAVE is untouched.
    unsafe {
        wrmsr(EFER, original_efer | (1 << 12));
        asm!("vmsave rax", in("rax") d.auxiliary, options(nostack, preserves_flags));
        wrmsr(EFER, original_efer);
    }
    let dr6: u64;
    let dr7: u64;
    unsafe {
        asm!("mov {}, dr6", out(reg) dr6, options(nomem, nostack, preserves_flags));
        asm!("mov {}, dr7", out(reg) dr7, options(nomem, nostack, preserves_flags));
    }
    let aux = unsafe { &*(d.auxiliary as *const Vmcb) };
    // VMSAVE captures FS/GS bases. Nonzero TLS aliases must remain backed by
    // admitted guest/native memory; zero bases do not imply page zero access.
    for offset in [0x448usize, 0x458] {
        let base = u64::from_le_bytes(aux.bytes()[offset..offset + 8].try_into().unwrap());
        if base != 0 {
            unsafe { mapped(map, cfg, &mt, pat, base, 1, false, false) }?;
        }
    }
    {
        let vmcb = unsafe { &mut *(d.vmcb as *mut Vmcb) };
        let frame = unsafe { &mut *(d.registers as *mut GuestRegisters) };
        resident::prepare_callback(
            CallbackRequest {
                boundary: b,
                efer,
                gdt: &gdt,
                auxiliary: aux,
                dr6,
                dr7,
                pat,
                stack: GuestStackSpan { base: stack_base, bytes: stack_end - stack_base },
                sites,
            },
            &policy,
            vmcb,
            frame,
        )
        .map_err(|error| {
            trace_detail(&error);
            trace_detail(&(
                "callback-native",
                slot,
                b.rflags,
                b.cr0,
                b.cr3,
                b.cr4,
                b.efer,
                b.profile,
            ));
            trace_detail(&("callback-selectors", b.cs, b.ss, b.ds, b.es, b.fs, b.gs, b.ldtr, b.tr));
            trace_detail(&("callback-debug", dr6, dr7, b.cr8, pat));
            trace_detail(&(
                "callback-stack",
                b.entry_rsp,
                b as *const NativeBoundary as u64,
                b.entry_rip,
                stack_base,
                stack_end,
            ));
            for offset in [0x440usize, 0x450, 0x470, 0x490] {
                let bytes = aux.bytes();
                trace_detail(&(
                    "callback-aux",
                    offset,
                    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()),
                    u16::from_le_bytes(bytes[offset + 2..offset + 4].try_into().unwrap()),
                    u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()),
                    u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap()),
                ));
            }
            17u64
        })?;
        // APM2 18.1.1/18.12 and Table B-2: CET_SS enumerates these MSRs.
        // VMSAVE omits them. CR4.CET was refused by prepare_callback, but
        // disabled CET does not imply its MSRs are zero. Preserve the native
        // values before VMRUN starts owning their guest VMCB copies.
        if __cpuid_count(0, 0).eax >= 7 && __cpuid_count(7, 0).ecx & (1 << 7) != 0 {
            let s_cet = unsafe { rdmsr(0x6a2) };
            let isst_addr = unsafe { rdmsr(0x6a8) };
            vmcb.initialize_native_cet_msrs(s_cet, isst_addr).map_err(|_| 17u64)?;
        }
        let storage = unsafe { &mut *(d.npt as *mut TableStorage) };
        let monitor = policy.validate(d.pool_base, d.pool_bytes, 4096).map_err(|_| 18u64)?;
        let mut _npt = resident::memory::prepare_identity_npt(
            storage,
            d.npt,
            policy,
            monitor,
            map,
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
            EvidenceFlag::Set,
            pat,
        )
        .map_err(|_| 19u64)?;
        #[cfg(feature = "native-resident-boot")]
        card_boot::protect_config(&mut _npt).map_err(|_| 19u64)?;
    }
    #[cfg(feature = "native-resident-smp-activate")]
    physical::validate_x2apic()?;
    let arm: abi::ArmRuntime = unsafe { core::mem::transmute(d.arm as usize) };
    #[cfg(feature = "native-resident-guest-startup")]
    let initial_icr = unsafe { physical::initial_icr(slot)? };
    #[cfg(not(feature = "native-resident-guest-startup"))]
    let initial_icr: *const u64 = ptr::null();
    if !initial_icr.is_null() {
        // arm copies this numeric field before switching to its private root;
        // it must not retain a caller pointer through runtime virtual mapping.
        unsafe { mapped(map, cfg, &mt, pat, initial_icr as u64, 8, false, false)? };
    }
    #[cfg(feature = "native-resident-boot")]
    let terminal_endpoint = card_boot::terminal_endpoint();
    #[cfg(not(feature = "native-resident-boot"))]
    let terminal_endpoint: *const abi::terminal::TerminalEndpoint = ptr::null();
    if !terminal_endpoint.is_null() {
        unsafe {
            mapped(
                map,
                cfg,
                &mt,
                pat,
                terminal_endpoint as u64,
                core::mem::size_of::<abi::terminal::TerminalEndpoint>() as u64,
                false,
                false,
            )?;
        }
    }
    let arm_result = unsafe {
        arm(
            original_efer,
            sites.resume,
            sites.ack,
            sites.after_ack,
            map.as_ptr(),
            map.len(),
            ptr::addr_of!(CPU_IDS).cast::<u32>(),
            CPU_COUNT,
            cfg!(feature = "native-resident-guest-startup"),
            initial_icr,
            terminal_endpoint,
        )
    };
    if arm_result != 0 {
        // Preserve typed takeover evidence (arm code 11 names the refused
        // captured x2APIC register) through the AP callback home area and
        // BSP's captured refusal. Ordinary arm failures retain old code20.
        return Err(if arm_result >> 56 == abi::TAKEOVER_TAG { arm_result } else { 20 });
    }
    trace(b'E');
    let enter: abi::Enter = unsafe { core::mem::transmute(d.enter as usize) };
    // All fallible preparation and original-EFER restoration precede this
    // irreversible transition. The raw runtime never returns to this stack.
    unsafe {
        wrmsr(EFER, original_efer | (1 << 12));
        enter(d.context as *mut abi::BridgeContext)
    }
}
