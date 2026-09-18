//! Owned, one-shot actual cache and paging-root observations from the final MP rendezvous.
//! Register agreement is evidence, not a mapping/cache-coherence admission token.
#[cfg(any(target_os = "uefi", test))]
use super::snapshot::NativeSnapshot;
use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::{align_of, size_of},
    ptr::{self, NonNull},
    sync::atomic::{AtomicUsize, Ordering},
};
use uefi_raw::{
    Status,
    table::boot::{BootServices, MemoryType},
};

use super::{
    cache::{self as native_cache, CacheSnapshot, CaptureError},
    cpu::{CpuError, CpuReport, MAX_PROCESSORS, PreparedCpus, ProcessorInformation},
};
// The live capture adapters exist only for firmware and host-test builds.
#[cfg(any(target_os = "uefi", test))]
use super::cpu::{ApObservation, DispatchedAp, QuiescentBsp};
#[cfg(any(target_os = "uefi", test))]
use core::sync::atomic::AtomicBool;

const ENABLED: u32 = 2;
const EMPTY: usize = 0;
#[cfg(any(target_os = "uefi", test))]
const WRITING: usize = 1;
const COMPLETE: usize = 2;
const ARITHMETIC_FLAGS: u64 = 0x8d5; // CF/PF/AF/ZF/SF/OF only.
const UNSUPPORTED_FLAGS: u64 = 0x44700; // TF/IF/DF/NT/AC.
const AP_CR4_DIFFERENCE: u64 = 1 << 3; // DE only: AP I/O debug extensions.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigurationField {
    AbiVersion,
    CapturedFields,
    Refusal,
    MsrReads,
    Signature,
    MaximumBasicLeaf,
    MaximumExtendedLeaf,
    Leaf1Ecx,
    Leaf1Edx,
    PhysicalBits,
    EncryptionEax,
    EncryptionEbx,
    MultiKeyEax,
    MultiKeyEbx,
    Reserved,
    Rflags,
    Cr0,
    Cr3,
    Cr4,
    Efer,
    SysCfg,
    SevStatus,
    Pat,
    MtrrCap,
    MtrrDefault,
    TopMem,
    SmmAddress,
    SmmMask,
    ApicBase,
    MmioConfig,
    Iorr,
    VariableMtrrs,
}

/// A paging observation is separate from the unchanged 352-byte cache ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingRootError {
    NotCaptured,
    PrivilegeLevel,
    UnexpectedStatus,
    InconsistentCr0,
    InconsistentCr4,
    UnsupportedFlags,
    InconsistentFlags,
    UnsupportedPagingMode,
    InvalidCr3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendezvousError {
    Cpu(CpuError),
    Allocation(Status),
    Layout,
    Released,
    Bounds,
    Inventory,
    ReusedOrInvalidCallback,
    Incomplete { processor: usize },
    Stale { processor: usize },
    Capture { processor: usize, error: CaptureError },
    CaptureShape { processor: usize },
    PagingRoot { processor: usize, error: PagingRootError },
    Mismatch { processor: usize, field: ConfigurationField },
    Cleanup(Status),
}

/// Historical counts: every enabled CPU was captured and compared in the named
/// final round. No claim about disabled CPUs, aliases, TLBs, SMM or DMA follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConsistencyReport {
    pub enabled_processors: usize,
    pub completed_ap_captures: usize,
    pub bsp_number: usize,
    pub rendezvous: usize,
}

#[repr(C)]
struct Slot {
    information: ProcessorInformation,
    state: AtomicUsize,
    // All cells have one writer, selected by EMPTY -> WRITING. AP cells
    // are never overwritten. A release COMPLETE publishes them together.
    rendezvous: UnsafeCell<usize>,
    status: UnsafeCell<u32>,
    snapshot: UnsafeCell<CacheSnapshot>,
    paging_root: UnsafeCell<Result<u64, PagingRootError>>,
}
const _: () = assert!(align_of::<Slot>() == 8);
const _: () = assert!(size_of::<Slot>() == 416);

/// Prepared on the BSP before the retained map is collected, then moved into P
/// and projected as its final AP observer. Release/Drop require TPL <= NOTIFY.
/// AP slots are single-use; there is no shared reset or snapshot replacement.
pub struct PreparedCacheRendezvous<'a> {
    services: &'a BootServices,
    pool: Option<NonNull<Slot>>,
    report: CpuReport,
    #[cfg(any(target_os = "uefi", test))]
    invalid: AtomicBool,
    bsp_rendezvous: usize,
    // Only the most recent actual AP comparison can associate CR4 evidence.
    // Every attempted BSP capture invalidates the preceding association.
    cr4_mismatch_processor: Option<usize>,
    not_send: PhantomData<*mut ()>,
}

// Shared methods neither access services nor mutate/free the pool. AP writes
// claim a single slot atomically; snapshot reads acquire COMPLETE. Free and BSP
// recapture need &mut self, excluding shared observers and borrowed snapshots.
unsafe impl Sync for PreparedCacheRendezvous<'_> {}

// Keep these typed scalar encoders separate from processor selection and flag
// enrichment. The combined match lowered to computed jumps in the HIGH scope;
// the mandatory linked audit requires direct control flow. No enum layout or
// numeric Rust discriminant is part of the diagnostic wire contract.
#[inline(never)]
fn diagnostic_global(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::Cpu(_) => 0x01,
        RendezvousError::Allocation(_) => 0x02,
        RendezvousError::Layout => 0x03,
        RendezvousError::Released => 0x04,
        RendezvousError::Bounds => 0x05,
        RendezvousError::Inventory => 0x06,
        RendezvousError::ReusedOrInvalidCallback => 0x07,
        RendezvousError::Cleanup(_) => 0x08,
        RendezvousError::Capture { .. }
        | RendezvousError::CaptureShape { .. }
        | RendezvousError::Incomplete { .. }
        | RendezvousError::Stale { .. }
        | RendezvousError::PagingRoot { .. }
        | RendezvousError::Mismatch { .. } => 0,
    }
}

#[inline(never)]
fn diagnostic_capture(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::Capture { error, .. } => match error {
            CaptureError::OutputAddress => 0x10,
            CaptureError::PrivilegeOrFlags => 0x11,
            CaptureError::UnsupportedCpu => 0x12,
            CaptureError::UnsupportedFeatures => 0x13,
            CaptureError::AddressEncryptionActive => 0x14,
            CaptureError::UnsupportedMtrrCount => 0x15,
            CaptureError::UnexpectedStatus => 0x16,
        },
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_record(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::CaptureShape { .. } => 0x20,
        RendezvousError::Incomplete { .. } => 0x21,
        RendezvousError::Stale { .. } => 0x22,
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_paging(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::PagingRoot { error, .. } => match error {
            PagingRootError::NotCaptured => 0x30,
            PagingRootError::PrivilegeLevel => 0x31,
            PagingRootError::UnexpectedStatus => 0x32,
            PagingRootError::InconsistentCr0 => 0x33,
            PagingRootError::InconsistentCr4 => 0x34,
            PagingRootError::UnsupportedFlags => 0x35,
            PagingRootError::InconsistentFlags => 0x36,
            PagingRootError::UnsupportedPagingMode => 0x37,
            PagingRootError::InvalidCr3 => 0x38,
        },
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_mismatch(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::Mismatch { field, .. } => match field {
            ConfigurationField::AbiVersion => 0x40,
            ConfigurationField::CapturedFields => 0x41,
            ConfigurationField::Refusal => 0x42,
            ConfigurationField::MsrReads => 0x43,
            ConfigurationField::Signature => 0x44,
            ConfigurationField::MaximumBasicLeaf => 0x45,
            ConfigurationField::MaximumExtendedLeaf => 0x46,
            ConfigurationField::Leaf1Ecx => 0x47,
            ConfigurationField::Leaf1Edx => 0x48,
            ConfigurationField::PhysicalBits => 0x49,
            ConfigurationField::EncryptionEax => 0x4a,
            ConfigurationField::EncryptionEbx => 0x4b,
            ConfigurationField::MultiKeyEax => 0x4c,
            ConfigurationField::MultiKeyEbx => 0x4d,
            ConfigurationField::Reserved => 0x4e,
            ConfigurationField::Rflags => 0x4f,
            ConfigurationField::Cr0 => 0x50,
            ConfigurationField::Cr3 => 0x51,
            ConfigurationField::Cr4 => 0x52,
            ConfigurationField::Efer => 0x53,
            ConfigurationField::SysCfg => 0x54,
            ConfigurationField::SevStatus => 0x55,
            ConfigurationField::Pat => 0x56,
            ConfigurationField::MtrrCap => 0x57,
            ConfigurationField::MtrrDefault => 0x58,
            ConfigurationField::TopMem => 0x59,
            ConfigurationField::SmmAddress => 0x5a,
            ConfigurationField::SmmMask => 0x5b,
            ConfigurationField::ApicBase => 0x5c,
            ConfigurationField::MmioConfig => 0x5d,
            ConfigurationField::Iorr => 0x5e,
            ConfigurationField::VariableMtrrs => 0x5f,
        },
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_processor(error: &RendezvousError) -> Option<usize> {
    match error {
        RendezvousError::Capture { processor, .. }
        | RendezvousError::CaptureShape { processor }
        | RendezvousError::Incomplete { processor }
        | RendezvousError::Stale { processor }
        | RendezvousError::PagingRoot { processor, .. }
        | RendezvousError::Mismatch { processor, .. } => Some(*processor),
        _ => None,
    }
}

impl PreparedCacheRendezvous<'_> {
    /// Stable diagnostic bits for the parent's existing refusal word. Bits
    /// 31:24 hold a processor number for processor-specific errors; bits 23:16
    /// hold the explicit detail code. The low 16 bits remain the caller's stage.
    /// Global errors use processor byte zero, which is not a BSP observation.
    ///
    /// PrivilegeOrFlags consults only an acquired COMPLETE slot. Its
    /// 0x80..=0x9f detail records observed TF/IF/DF/NT/AC in bits 0..4;
    /// zero observed flags does not identify CPL or the unclassified reason.
    /// An associated current-round AP CR4 mismatch may use 0xc0..=0xff for
    /// exactly one remaining rejected bit 0..63 after excluding AP CR4.DE.
    /// This records a bit index, not the full XOR or a normalized register.
    /// This does not recapture, normalize, or admit any observation.
    /// The caller must supply this object's actual error from the current round,
    /// before another capture. An encoded error alone authenticates no evidence.
    pub fn diagnostic_bits(&self, error: RendezvousError) -> u64 {
        let processor = diagnostic_processor(&error);
        // Exactly one disjoint category supplies a nonzero detail code.
        let mut detail = diagnostic_global(&error)
            | diagnostic_capture(&error)
            | diagnostic_record(&error)
            | diagnostic_paging(&error)
            | diagnostic_mismatch(&error);
        let Some(processor) = processor else {
            return u64::from(detail) << 16;
        };
        const _: () = assert!(MAX_PROCESSORS == 256);
        if processor >= MAX_PROCESSORS {
            // Do not truncate an unrepresentable processor to a different CPU.
            return 0x007f_0000;
        }
        if detail == 0x52 {
            detail = self.diagnostic_cr4_mismatch(processor);
        }
        if detail == 0x11
            && let Ok(slot) = self.slot(processor)
            && slot.state.load(Ordering::Acquire) == COMPLETE
        {
            // COMPLETE publishes the immutable AP record. &self also excludes
            // BSP replacement and release while status and flags are read.
            let status = unsafe { *slot.status.get() };
            let flags = unsafe { (*slot.snapshot.get()).rflags };
            if status == 2 || (status == 0 && flags & UNSUPPORTED_FLAGS != 0) {
                detail = 0x80
                    | ((flags >> 8) & 1) as u8
                    | (((flags >> 9) & 1) as u8) << 1
                    | (((flags >> 10) & 1) as u8) << 2
                    | (((flags >> 14) & 1) as u8) << 3
                    | (((flags >> 18) & 1) as u8) << 4;
            }
        }
        ((processor as u64) << 24) | (u64::from(detail) << 16)
    }

    #[inline(never)]
    fn diagnostic_cr4_mismatch(&self, processor: usize) -> u8 {
        if self.cr4_mismatch_processor != Some(processor)
            || processor == self.report.bsp_number
            || self.bsp_rendezvous == 0
        {
            return 0x52;
        }
        let Ok(bsp_slot) = self.slot(self.report.bsp_number) else {
            return 0x52;
        };
        let Ok(ap_slot) = self.slot(processor) else {
            return 0x52;
        };
        // COMPLETE is acquired before any published record or round is read.
        let Ok(bsp) = self.completed_snapshot(self.report.bsp_number) else {
            return 0x52;
        };
        let Ok(ap) = self.completed_snapshot(processor) else {
            return 0x52;
        };
        if ap_slot.information.status_flag & ENABLED == 0
            || unsafe { *bsp_slot.rendezvous.get() } != self.bsp_rendezvous
            || unsafe { *ap_slot.rendezvous.get() } != self.bsp_rendezvous
            || compare_ap_configuration(bsp, ap) != Err(ConfigurationField::Cr4)
        {
            return 0x52;
        }
        let rejected = (bsp.cr4 ^ ap.cr4) & !AP_CR4_DIFFERENCE;
        if rejected.is_power_of_two() { 0xc0 | rejected.trailing_zeros() as u8 } else { 0x52 }
    }

    fn slot(&self, number: usize) -> Result<&Slot, RendezvousError> {
        if number >= self.report.total_processors {
            return Err(RendezvousError::Bounds);
        }
        let pool = self.pool.ok_or(RendezvousError::Released)?;
        Ok(unsafe { &*pool.as_ptr().add(number) })
    }

    /// Exact allocation byte span for the separately retained resource walker.
    /// Pool ownership does not itself qualify current mappings or memory type.
    pub fn storage_range(&self) -> Result<(u64, usize), RendezvousError> {
        let pool = self.pool.ok_or(RendezvousError::Released)?;
        Ok((pool.as_ptr().addr() as u64, self.report.total_processors * size_of::<Slot>()))
    }

    pub fn release(&mut self) -> Result<(), Status> {
        if let Some(pool) = self.pool {
            let status = unsafe { (self.services.free_pool)(pool.as_ptr().cast()) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.pool = None;
        }
        Ok(())
    }

    fn completed_snapshot(&self, number: usize) -> Result<&CacheSnapshot, RendezvousError> {
        let slot = self.slot(number)?;
        if slot.state.load(Ordering::Acquire) != COMPLETE {
            return Err(RendezvousError::Incomplete { processor: number });
        }
        let status = unsafe { *slot.status.get() };
        decode_status(status)
            .map_err(|error| RendezvousError::Capture { processor: number, error })?;
        let snapshot = unsafe { &*slot.snapshot.get() };
        if snapshot.rflags & UNSUPPORTED_FLAGS != 0 {
            return Err(RendezvousError::Capture {
                processor: number,
                error: CaptureError::PrivilegeOrFlags,
            });
        }
        if !complete_shape(snapshot) {
            return Err(RendezvousError::CaptureShape { processor: number });
        }
        Ok(snapshot)
    }

    /// Read the latest completed actual BSP capture. This is deliberately just
    /// a borrowed observation; comparison/resource admission may have refused.
    pub fn bsp_snapshot(&self) -> Result<&CacheSnapshot, RendezvousError> {
        self.completed_snapshot(self.report.bsp_number)
    }

    fn completed_cr3(&self, number: usize) -> Result<u64, RendezvousError> {
        // This acquire and the cache checks precede every paging-root read.
        self.completed_snapshot(number)?;
        let slot = self.slot(number)?;
        unsafe { *slot.paging_root.get() }
            .map_err(|error| RendezvousError::PagingRoot { processor: number, error })
    }

    /// Latest completed actual BSP CR3, including its PWT/PCD bits. This value
    /// alone is not comparison success, retained-table provenance or a lease.
    /// A refused BSP recapture invalidates access to the preceding root.
    pub fn bsp_cr3(&self) -> Result<u64, RendezvousError> {
        self.completed_cr3(self.report.bsp_number)
    }

    /// Capture the actual BSP directly into its owned slot and compare every
    /// enabled AP's actual final-round observations. May be repeated with &mut
    /// self before/after the transition; AP records remain immutable.
    ///
    /// # Safety
    /// The guard must cover this BSP at HIGH, outside SMM, with IF/DF/TF/NT/AC
    /// clear. This object must have been selected as that round's final observer.
    /// The pool, code and stack must remain accessible/coherent throughout this
    /// service-free call, under the native cache reader's ordinary firmware
    /// contract. No fault containment is provided. Finish/release must run only
    /// after conforming blocking dispatch terminated every callback, at NOTIFY.
    #[cfg(any(target_os = "uefi", test))]
    pub unsafe fn capture_bsp_and_compare(
        &mut self,
        guard: &QuiescentBsp<'_>,
    ) -> Result<CacheConsistencyReport, RendezvousError> {
        self.cr4_mismatch_processor = None;
        let current = guard.report();
        if current.total_processors != self.report.total_processors
            || current.enabled_processors != self.report.enabled_processors
            || current.enabled_aps != self.report.enabled_aps
            || current.bsp_number != self.report.bsp_number
            || current.bsp_processor_id != self.report.bsp_processor_id
            || current.completed_ap_callbacks != self.report.enabled_aps
        {
            return Err(RendezvousError::Inventory);
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err(RendezvousError::ReusedOrInvalidCallback);
        }
        // Bind even a single-BSP machine to one scope. Repeated BSP captures
        // are allowed only inside that scope; no old object crosses a new one.
        bind_bsp_round(&mut self.bsp_rendezvous, guard.rendezvous(), current.bsp_number)?;
        let slot = self.slot(current.bsp_number)?;
        slot.state.store(WRITING, Ordering::Relaxed);
        // &mut self excludes all BSP snapshot references while it is replaced.
        unsafe {
            *slot.rendezvous.get() = guard.rendezvous();
            capture_slot(slot);
        }
        slot.state.store(COMPLETE, Ordering::Release);
        let bsp = self.completed_snapshot(current.bsp_number)?;
        let bsp_cr3 = self.completed_cr3(current.bsp_number)?;
        for number in 0..current.total_processors {
            let slot = self.slot(number)?;
            if slot.information.status_flag & ENABLED == 0 || number == current.bsp_number {
                continue;
            }
            let ap = self.completed_snapshot(number)?;
            // COMPLETE's acquire covers rendezvous and snapshot together.
            if unsafe { *slot.rendezvous.get() } != guard.rendezvous() {
                return Err(RendezvousError::Stale { processor: number });
            }
            if let Err(field) = compare_ap_configuration(bsp, ap) {
                if field == ConfigurationField::Cr4 {
                    self.cr4_mismatch_processor = Some(number);
                }
                return Err(RendezvousError::Mismatch { processor: number, field });
            }
            if self.completed_cr3(number)? != bsp_cr3 {
                return Err(RendezvousError::Mismatch {
                    processor: number,
                    field: ConfigurationField::Cr3,
                });
            }
        }
        Ok(CacheConsistencyReport {
            enabled_processors: current.enabled_processors,
            completed_ap_captures: current.enabled_aps,
            bsp_number: current.bsp_number,
            rendezvous: guard.rendezvous(),
        })
    }
}

impl Drop for PreparedCacheRendezvous<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(any(target_os = "uefi", test))]
unsafe extern "efiapi" {
    fn svmvisor_native_cache_read(out: *mut CacheSnapshot) -> u32;
    // Exact unchanged native_snapshot.S ABI: exclusive, aligned 72-byte output.
    fn svmvisor_native_snapshot(out: *mut NativeSnapshot) -> u32;
}

/// The caller exclusively owns this WRITING slot and supplies a legitimate AP
/// callback or current HIGH BSP guard. COMPLETE is published only on return.
#[cfg(any(target_os = "uefi", test))]
unsafe fn capture_slot(slot: &Slot) {
    let status = unsafe { svmvisor_native_cache_read(slot.snapshot.get()) };
    unsafe {
        *slot.status.get() = status;
        // Never retain the previous BSP root after any refused recapture.
        *slot.paging_root.get() = if status == 0 {
            capture_paging_root(&*slot.snapshot.get())
        } else {
            Err(PagingRootError::NotCaptured)
        };
    }
}

/// Called only after the real cache reader's CPL/flags/target guards succeed.
/// The unchanged helper independently checks CPL before SGDT/SIDT/CR reads;
/// it does not read MSRs, dereference tables or write privileged state.
#[cfg(any(target_os = "uefi", test))]
unsafe fn capture_paging_root(cache: &CacheSnapshot) -> Result<u64, PagingRootError> {
    // Nine u64 words give the helper's exact 72-byte size and 8-byte alignment.
    // CR0/CR3/CR4/RFLAGS occupy byte offsets 40/48/56/64; CS starts at 32.
    let mut words = [0u64; 9];
    match unsafe { svmvisor_native_snapshot(words.as_mut_ptr().cast()) } {
        0 => {}
        1 => return Err(PagingRootError::PrivilegeLevel),
        _ => return Err(PagingRootError::UnexpectedStatus),
    }
    if words[4] & 3 != 0 {
        return Err(PagingRootError::PrivilegeLevel);
    }
    if words[5] != cache.cr0 {
        return Err(PagingRootError::InconsistentCr0);
    }
    if words[7] != cache.cr4 {
        return Err(PagingRootError::InconsistentCr4);
    }
    if words[8] & UNSUPPORTED_FLAGS != 0 {
        return Err(PagingRootError::UnsupportedFlags);
    }
    if words[8] & !ARITHMETIC_FLAGS != cache.rflags & !ARITHMETIC_FLAGS {
        return Err(PagingRootError::InconsistentFlags);
    }
    // Restricted four-level, 48-bit target with PCIDE clear: CR3[4:3] retain
    // the root fetch's PCD/PWT, not PCID bits. Do not normalize any control.
    if cache.physical_bits != 48
        || cache.cr0 & 0x8000_0001 != 0x8000_0001 // PG and PE.
        || cache.cr4 & (1 << 5) == 0 // PAE.
        || cache.cr4 & ((1 << 12) | (1 << 17)) != 0 // LA57 or PCIDE.
        || cache.efer & 0x500 != 0x500
    // LME and LMA.
    {
        return Err(PagingRootError::UnsupportedPagingMode);
    }
    const ROOT_ADDRESS: u64 = 0x0000_ffff_ffff_f000;
    let cr3 = words[6];
    if cr3 & !(ROOT_ADDRESS | 0x18) != 0 || cr3 & ROOT_ADDRESS == 0 {
        return Err(PagingRootError::InvalidCr3);
    }
    Ok(cr3)
}

// This is the legitimate AP adapter for the unchanged reader. It receives no
// mutable P or BSP guard and does not normalize flags. Unsupported IF and other
// incoming flags are recorded as reader refusal before any RDMSR.
#[cfg(any(target_os = "uefi", test))]
unsafe impl ApObservation for PreparedCacheRendezvous<'_> {
    unsafe fn observe(&self, ap: &DispatchedAp<'_>) {
        let Ok(slot) = self.slot(ap.number()) else {
            self.invalid.store(true, Ordering::Release);
            return;
        };
        if ap.number() == self.report.bsp_number
            || slot.information != ap.information()
            || slot.information.status_flag & ENABLED == 0
            || slot
                .state
                .compare_exchange(EMPTY, WRITING, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            self.invalid.store(true, Ordering::Release);
            return;
        }
        unsafe {
            *slot.rendezvous.get() = ap.rendezvous();
            capture_slot(slot);
        }
        slot.state.store(COMPLETE, Ordering::Release);
    }
}

/// Allocate one bounded pool and copy the CPU helper's validated inventory.
/// Allocate before collecting the final memory map and move this into scoped P.
///
/// # Safety
/// Live trusted Boot Services on the original BSP at TPL <= NOTIFY, before
/// ExitBootServices. `cpus` must belong to these same live services. Its inventory
/// is rechecked by the final CPU scope; neither historical report is a lease.
pub unsafe fn prepare<'a>(
    services: &'a BootServices,
    cpus: &PreparedCpus<'_>,
) -> Result<PreparedCacheRendezvous<'a>, RendezvousError> {
    let report = cpus.report();
    if report.total_processors == 0 || report.total_processors > MAX_PROCESSORS {
        return Err(RendezvousError::Bounds);
    }
    // Verify CPU storage is live before creating a second allocation.
    cpus.processor_information(report.bsp_number).map_err(RendezvousError::Cpu)?;
    let mut raw = ptr::null_mut();
    let status = unsafe {
        (services.allocate_pool)(
            MemoryType::BOOT_SERVICES_DATA,
            report.total_processors * size_of::<Slot>(),
            &mut raw,
        )
    };
    if status != Status::SUCCESS {
        return Err(RendezvousError::Allocation(status));
    }
    let pool = NonNull::new(raw.cast::<Slot>()).ok_or(RendezvousError::Layout)?;
    let mut owned = PreparedCacheRendezvous {
        services,
        pool: Some(pool),
        report,
        #[cfg(any(target_os = "uefi", test))]
        invalid: AtomicBool::new(false),
        bsp_rendezvous: 0,
        cr4_mismatch_processor: None,
        not_send: PhantomData,
    };
    let initialized = initialize(&owned, cpus);
    if let Err(error) = initialized {
        owned.release().map_err(RendezvousError::Cleanup)?;
        return Err(error);
    }
    Ok(owned)
}

fn initialize(
    owned: &PreparedCacheRendezvous<'_>,
    cpus: &PreparedCpus<'_>,
) -> Result<(), RendezvousError> {
    let pool = owned.pool.ok_or(RendezvousError::Released)?;
    if pool.as_ptr().addr() % align_of::<Slot>() != 0 {
        return Err(RendezvousError::Layout);
    }
    for number in 0..owned.report.total_processors {
        let information = cpus.processor_information(number).map_err(RendezvousError::Cpu)?;
        unsafe {
            pool.as_ptr().add(number).write(Slot {
                information,
                state: AtomicUsize::new(EMPTY),
                rendezvous: UnsafeCell::new(0),
                status: UnsafeCell::new(u32::MAX),
                snapshot: UnsafeCell::new(CacheSnapshot::default()),
                paging_root: UnsafeCell::new(Err(PagingRootError::NotCaptured)),
            });
        }
    }
    Ok(())
}

fn decode_status(status: u32) -> Result<(), CaptureError> {
    match status {
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

#[cfg(any(target_os = "uefi", test))]
fn bind_bsp_round(
    bound: &mut usize,
    current: usize,
    processor: usize,
) -> Result<(), RendezvousError> {
    if current == 0 || (*bound != 0 && *bound != current) {
        return Err(RendezvousError::Stale { processor });
    }
    *bound = current;
    Ok(())
}

fn complete_shape(snapshot: &CacheSnapshot) -> bool {
    let sev = snapshot.encryption_eax & 2 != 0;
    let fields =
        native_cache::captured::REQUIRED | if sev { native_cache::captured::SEV_STATUS } else { 0 };
    let count = snapshot.mtrr_cap & 255;
    snapshot.abi_version == native_cache::ABI_VERSION
        && snapshot.refusal == 0
        && snapshot.captured_fields == fields
        && snapshot.reserved == 0
        && count <= native_cache::MAX_VARIABLE_MTRRS as u64
        && snapshot.msr_reads == 14 + u64::from(sev) + 2 * count
}

/// Pure comparison of supplied observations. Only initial APIC ID, APIC_BASE's
/// BSP role bit and arithmetic flags may differ. This is intentionally stricter
/// than cache equivalence: all CR0/CR4/EFER and common CPUID bits must match, even
/// when a legitimate AP profile difference would not change a memory type.
pub fn compare_configuration(
    bsp: &CacheSnapshot,
    ap: &CacheSnapshot,
) -> Result<(), ConfigurationField> {
    compare_configuration_with_cr4_mask(bsp, ap, 0)
}

/// Pure cross-CPU comparison for the final AP rendezvous only. The firmware
/// may leave CR4.DE different on APs; it controls I/O debug extensions, not
/// translation or memory type. Every other CR4 bit retains exact comparison.
/// Same-CPU capture consistency and BSP before/after checks remain exact.
pub fn compare_ap_configuration(
    bsp: &CacheSnapshot,
    ap: &CacheSnapshot,
) -> Result<(), ConfigurationField> {
    compare_configuration_with_cr4_mask(bsp, ap, AP_CR4_DIFFERENCE)
}

fn compare_configuration_with_cr4_mask(
    bsp: &CacheSnapshot,
    ap: &CacheSnapshot,
    allowed_cr4_difference: u64,
) -> Result<(), ConfigurationField> {
    macro_rules! equal {
        ($($member:ident => $field:ident),* $(,)?) => {$ (
            if bsp.$member != ap.$member { return Err(ConfigurationField::$field); }
        )*};
    }
    equal!(
        abi_version => AbiVersion, captured_fields => CapturedFields,
        refusal => Refusal, msr_reads => MsrReads, signature => Signature,
        max_basic => MaximumBasicLeaf, max_extended => MaximumExtendedLeaf,
        leaf1_ecx => Leaf1Ecx, leaf1_edx => Leaf1Edx, physical_bits => PhysicalBits,
        encryption_eax => EncryptionEax, encryption_ebx => EncryptionEbx,
        multi_key_eax => MultiKeyEax, multi_key_ebx => MultiKeyEbx, reserved => Reserved,
        cr0 => Cr0,
    );
    if (bsp.cr4 ^ ap.cr4) & !allowed_cr4_difference != 0 {
        return Err(ConfigurationField::Cr4);
    }
    equal!(
        efer => Efer, sys_cfg => SysCfg, sev_status => SevStatus,
        pat => Pat, mtrr_cap => MtrrCap, mtrr_default => MtrrDefault, top_mem => TopMem,
        smm_address => SmmAddress, smm_mask => SmmMask, mmio_config => MmioConfig,
        iorr => Iorr, variable => VariableMtrrs,
    );
    if bsp.rflags & !ARITHMETIC_FLAGS != ap.rflags & !ARITHMETIC_FLAGS {
        return Err(ConfigurationField::Rflags);
    }
    if bsp.apic_base & !(1 << 8) != ap.apic_base & !(1 << 8) {
        return Err(ConfigurationField::ApicBase);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bsp_binding_rejects_a_new_round_even_without_any_ap_slot() {
        let mut bound = 0;
        assert_eq!(bind_bsp_round(&mut bound, 0, 0), Err(RendezvousError::Stale { processor: 0 }));
        assert_eq!(bind_bsp_round(&mut bound, 7, 0), Ok(()));
        assert_eq!(bind_bsp_round(&mut bound, 7, 0), Ok(()));
        assert_eq!(bind_bsp_round(&mut bound, 8, 0), Err(RendezvousError::Stale { processor: 0 }));
        assert_eq!(bound, 7);
    }
}
