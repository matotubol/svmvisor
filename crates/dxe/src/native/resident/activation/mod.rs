//! Runtime-driver installation, qualified callback capture and loader continuation.
//!
//! UEFI 2.11 2.3.4.2/7.1/7.2/8.4.1 and AMD APM2 rev3.44 11.4/11.5/15.5/15.7.
//! This is a trusted native flat-UEFI profile, with an explicit diagnostic
//! post-EBS AP activation seam and an explicit normal-loader boot profile. The initial retained
//! map is a bounded RAM/aperture observation, not an allocation lease for future
//! firmware tables. Current paging is walked again inside the CLI callback.
//! Runtime allocation remains owned throughout; no Boot Services call occurs
//! in the callback inner or any persistent host path.
use core::sync::atomic::{AtomicBool, AtomicU32};

use svmvisor_hypervisor::{
    boot::memory::MemoryDescriptor,
    host::resident::{self as abi, ResidentDirectory},
    memory::address::EncryptionState,
};

// Names the mounted children `boot_handoff`, `card_boot` and `physical_boot` take through their
// `use super::*;`. Nothing else in this file uses them.
#[cfg(feature = "native-resident-smp-activate")]
use core::{
    arch::{asm, x86_64::__cpuid_count},
    ffi::c_void,
    ptr,
    sync::atomic::Ordering,
};

#[cfg(feature = "native-resident-boot")]
use svmvisor_dxe::native::admission::boundary::NativeBoundary;
#[cfg(feature = "native-resident-smp-activate")]
use svmvisor_dxe::native::{
    admission::memory,
    resident::{self, launch::Mtrrs},
};
#[cfg(feature = "native-resident-boot")]
use svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE;
#[cfg(feature = "native-resident-smp-activate")]
use svmvisor_hypervisor::{
    arch::x86_64::{
        apic,
        msr::{MTRR_CAP, PAT, VM_CR},
    },
    host::paging::{self, PagingConfig},
};
#[cfg(feature = "native-resident-boot")]
use uefi_raw::{Handle, protocol::loaded_image::LoadedImageProtocol, table::boot::Tpl};
#[cfg(feature = "native-resident-smp-activate")]
use uefi_raw::{
    Status,
    table::boot::{BootServices, MemoryType},
};

#[cfg(feature = "native-resident-boot")]
use self::{capture::cache_observation_detailed, mapping::validate_uc_mmio};
#[cfg(feature = "native-resident-smp-activate")]
use self::{
    capture::{config, cpu, mtrrs, rdmsr, wrmsr},
    closure::{directories, host_closure},
    diagnostic::{trace_detail, unsupported},
    mapping::{mapped, ram_span},
    preparation::{preparation_failure, preparation_map_failure, preparation_step},
};

pub(crate) use self::install::install;

#[cfg(feature = "native-resident-boot")]
mod boot_handoff;
mod callback;
mod capture;
#[cfg(feature = "native-resident-boot")]
mod card_boot;
mod closure;
mod diagnostic;
mod install;
mod mapping;
#[cfg(feature = "native-resident-smp-activate")]
mod physical_boot;
mod preparation;

include!(concat!(env!("OUT_DIR"), "/resident-entry.rs"));
const EMPTY: MemoryDescriptor =
    MemoryDescriptor { memory_type: 0, physical_start: 0, page_count: 0, attributes: 0 };
const COOKIE: usize = 0x53564d52;
const EFER: u32 = 0xc0000080;

static PACKAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/resident-payload.bin"));
static INSTALLED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cpu {
    physical_bits: u8,
    apic_id: u32,
    encryption: EncryptionState,
}
