//! Owned, one-shot actual cache and paging-root observations from the final MP rendezvous.
//! Register agreement is evidence, not a mapping/cache-coherence admission token.

#[cfg(any(target_os = "uefi", test))]
use core::sync::atomic::AtomicBool;
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

#[cfg(any(target_os = "uefi", test))]
use super::snapshot::NativeSnapshot;
use super::{
    cache::{self as native_cache, CacheSnapshot, CaptureError},
    cpu::{CpuError, CpuReport, MAX_PROCESSORS, PreparedCpus, ProcessorInformation},
};

pub use self::comparison::{compare_ap_configuration, compare_configuration};

mod capture;
mod comparison;
mod diagnostic;

const ENABLED: u32 = 2;
const EMPTY: usize = 0;
#[cfg(any(target_os = "uefi", test))]
const WRITING: usize = 1;
const COMPLETE: usize = 2;
const ARITHMETIC_FLAGS: u64 = 0x8d5; // CF/PF/AF/ZF/SF/OF only.
const UNSUPPORTED_FLAGS: u64 = 0x44700; // TF/IF/DF/NT/AC.
const AP_CR4_DIFFERENCE: u64 = 1 << 3; // DE only: AP I/O debug extensions.

#[cfg(any(target_os = "uefi", test))]
unsafe extern "efiapi" {
    fn svmvisor_native_cache_read(out: *mut CacheSnapshot) -> u32;
    // Exact unchanged native_snapshot.S ABI: exclusive, aligned 72-byte output.
    fn svmvisor_native_snapshot(out: *mut NativeSnapshot) -> u32;
}

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

impl PreparedCacheRendezvous<'_> {
    /// Exact allocation byte span for the separately retained resource walker.
    /// Pool ownership does not itself qualify current mappings or memory type.
    pub fn storage_range(&self) -> Result<(u64, usize), RendezvousError> {
        let pool = self.pool.ok_or(RendezvousError::Released)?;
        Ok((pool.as_ptr().addr() as u64, self.report.total_processors * size_of::<Slot>()))
    }

    /// Read the latest completed actual BSP capture. This is deliberately just
    /// a borrowed observation; comparison/resource admission may have refused.
    pub fn bsp_snapshot(&self) -> Result<&CacheSnapshot, RendezvousError> {
        self.completed_snapshot(self.report.bsp_number)
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

    fn slot(&self, number: usize) -> Result<&Slot, RendezvousError> {
        if number >= self.report.total_processors {
            return Err(RendezvousError::Bounds);
        }
        let pool = self.pool.ok_or(RendezvousError::Released)?;
        Ok(unsafe { &*pool.as_ptr().add(number) })
    }

    /// Latest completed actual BSP CR3, including its PWT/PCD bits. This value
    /// alone is not comparison success, retained-table provenance or a lease.
    /// A refused BSP recapture invalidates access to the preceding root.
    pub fn bsp_cr3(&self) -> Result<u64, RendezvousError> {
        self.completed_cr3(self.report.bsp_number)
    }

    fn completed_cr3(&self, number: usize) -> Result<u64, RendezvousError> {
        // This acquire and the cache checks precede every paging-root read.
        self.completed_snapshot(number)?;
        let slot = self.slot(number)?;
        unsafe { *slot.paging_root.get() }
            .map_err(|error| RendezvousError::PagingRoot { processor: number, error })
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
}

// Shared methods neither access services nor mutate/free the pool. AP writes
// claim a single slot atomically; snapshot reads acquire COMPLETE. Free and BSP
// recapture need &mut self, excluding shared observers and borrowed snapshots.
unsafe impl Sync for PreparedCacheRendezvous<'_> {}

impl Drop for PreparedCacheRendezvous<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
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

/// Historical counts: every enabled CPU was captured and compared in the named
/// final round. No claim about disabled CPUs, aliases, TLBs, SMM or DMA follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConsistencyReport {
    pub enabled_processors: usize,
    pub completed_ap_captures: usize,
    pub bsp_number: usize,
    pub rendezvous: usize,
}

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
