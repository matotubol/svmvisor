//! Bounded CPU cache/address-encryption observations for PPR 57896 rev.3.00.
//! No allocation, hardware writes, firmware calls, mapping dereferences or
//! native admission token. See docs/native-cache-contract.md for the limits.

pub use svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE;

pub use self::write_back::{
    CacheError, PageMapping, WriteBackReport, classify_write_back, leaf_pat_index,
};

mod write_back;

#[cfg(test)]
mod tests;

pub const ABI_VERSION: u64 = 1;
pub const SNAPSHOT_BYTES: usize = 352;
pub const MAX_VARIABLE_MTRRS: usize = 8;
pub const MAX_PAGES: usize = 256;
const PHYSICAL_MASK: u64 = (1 << 48) - 1;
const PAGE_MASK: u64 = PHYSICAL_MASK & !0xfff;
const LOW_BOUND: u64 = 1 << 20;
const HIGH_BOUND: u64 = 1 << 32;

pub mod captured {
    pub const CPUID: u64 = 1;
    pub const CONTROLS: u64 = 2;
    pub const ADDRESS_ENCRYPTION: u64 = 4;
    pub const ROUTING: u64 = 8;
    pub const MTRRS: u64 = 16;
    pub const SEV_STATUS: u64 = 32;
    pub const REQUIRED: u64 = 31;
}

#[cfg(target_os = "uefi")]
unsafe extern "efiapi" {
    fn svmvisor_native_cache_read(out: *mut CacheSnapshot) -> u32;
}

/// Raw integer observations, deliberately constructible for supplied-data tests.
/// A value of this type alone does not authenticate a hardware observation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheSnapshot {
    pub abi_version: u64,
    pub captured_fields: u64,
    pub refusal: u64,
    pub msr_reads: u64,
    pub signature: u32,
    pub max_basic: u32,
    pub max_extended: u32,
    pub leaf1_ecx: u32,
    pub leaf1_edx: u32,
    pub physical_bits: u32,
    pub encryption_eax: u32,
    pub encryption_ebx: u32,
    pub multi_key_eax: u32,
    pub multi_key_ebx: u32,
    pub initial_apic_id: u32,
    pub reserved: u32,
    pub rflags: u64,
    pub cr0: u64,
    pub cr4: u64,
    pub efer: u64,
    pub sys_cfg: u64,
    /// Defined only when `captured::SEV_STATUS` is present.
    pub sev_status: u64,
    pub pat: u64,
    pub mtrr_cap: u64,
    pub mtrr_default: u64,
    pub top_mem: u64,
    pub smm_address: u64,
    pub smm_mask: u64,
    pub apic_base: u64,
    pub mmio_config: u64,
    pub iorr: [RegisterPair; 2],
    pub variable: [RegisterPair; MAX_VARIABLE_MTRRS],
}

const _: () = assert!(core::mem::size_of::<CacheSnapshot>() == SNAPSHOT_BYTES);
const _: () = assert!(core::mem::align_of::<CacheSnapshot>() == 8);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, signature) == 32);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, rflags) == 80);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, cr0) == 88);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, efer) == 104);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, sys_cfg) == 112);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, pat) == 128);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, mtrr_cap) == 136);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, top_mem) == 152);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, iorr) == 192);
const _: () = assert!(core::mem::offset_of!(CacheSnapshot, variable) == 224);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegisterPair {
    pub base: u64,
    pub mask: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError {
    OutputAddress,
    PrivilegeOrFlags,
    UnsupportedCpu,
    UnsupportedFeatures,
    AddressEncryptionActive,
    UnsupportedMtrrCount,
    UnexpectedStatus,
}

/// Capture named MSRs after the assembly's exact CPU/capability guards.
///
/// # Safety
/// The guard must cover the current BSP with IF/DF/TF/NT/AC clear, outside SMM,
/// in ordinary firmware execution. `out` and the stack must be accessible for
/// the entire call. No fault containment is provided for hostile interception
/// or inaccessible memory. This code is currently unlinked groundwork: this
/// batch permits compile/tests only and no physical execution. Future AP use
/// needs its own callback/flags contract and explicit integration review.
#[cfg(target_os = "uefi")]
pub unsafe fn capture_into(
    _guard: &super::cpu::QuiescentBsp<'_>,
    out: &mut CacheSnapshot,
) -> Result<(), CaptureError> {
    match unsafe { svmvisor_native_cache_read(out) } {
        0 => Ok(()),
        1 => Err(CaptureError::OutputAddress),
        2 => Err(CaptureError::PrivilegeOrFlags),
        3 => Err(CaptureError::UnsupportedCpu),
        4 => Err(CaptureError::UnsupportedFeatures),
        5 => Err(CaptureError::AddressEncryptionActive),
        6 => Err(CaptureError::UnsupportedMtrrCount),
        _ => Err(CaptureError::UnexpectedStatus),
    }
}
