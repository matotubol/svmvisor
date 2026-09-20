//! Real PI MP Services inventory and finished AP rendezvous for native DXE.
//! A report is an observation, not a persistent ownership token. See the scoped
//! entry contract below before using it for an assembly interval.

use core::{
    convert::Infallible,
    ffi::c_void,
    marker::PhantomData,
    mem::{align_of, size_of},
    ptr::{self, NonNull},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use uefi_raw::{
    Boolean, Event, Guid, Status, guid,
    table::boot::{BootServices, MemoryType, Tpl},
};

pub const MAX_PROCESSORS: usize = 256;
pub const AP_TIMEOUT_MICROSECONDS: usize = 1_000_000;
pub const MP_SERVICES_GUID: Guid = guid!("3fdda605-a76e-4f46-ad29-12f4531b3d08");
const BSP: u32 = 1;
pub(super) const ENABLED: u32 = 2;
const HEALTHY: u32 = 4;

static NEXT_RENDEZVOUS: AtomicUsize = AtomicUsize::new(1);

/// Prepared storage. It never parks APs and does not modify their enable state.
/// Drop is best effort; call release to observe cleanup failures or retry them.
pub struct PreparedCpus<'a> {
    services: &'a BootServices,
    protocol: &'a MpServicesProtocol,
    pool: Option<NonNull<Record>>,
    report: CpuReport,
    rendezvous: usize,
    // Pool/protocol access is confined to the invoking firmware BSP.
    not_send_sync: PhantomData<*mut ()>,
}

impl PreparedCpus<'_> {
    pub const fn report(&self) -> CpuReport {
        self.report
    }

    fn record(&self, number: usize) -> Result<&Record, CpuError> {
        if number >= self.report.total_processors {
            return Err(CpuError::Bounds);
        }
        let pool = self.pool.ok_or(CpuError::Released)?;
        Ok(unsafe { &*pool.as_ptr().add(number) })
    }

    /// Exact live allocation byte span for retained operand coverage. Rounded
    /// containing pages are borrowed mappings, not exclusively owned pages.
    pub fn storage_range(&self) -> Result<(u64, usize), CpuError> {
        let pool = self.pool.ok_or(CpuError::Released)?;
        Ok((pool.as_ptr().addr() as u64, self.report.total_processors * size_of::<Record>()))
    }

    /// Perform the bounded blocking barrier, then verify every callback and
    /// repeat the entire inventory. Firmware calls afterward stale this report.
    ///
    /// # Safety
    /// Must run synchronously on the original BSP at TPL <= TPL_NOTIFY, with live
    /// conforming Boot/MP Services. A blocking timeout MUST terminate every
    /// dispatched callback before returning, as PI specifies. Firmware that
    /// violates that lifetime guarantee cannot safely use this adapter.
    pub unsafe fn rendezvous(&mut self) -> Result<CpuReport, CpuError> {
        unsafe { self.rendezvous_with_observation(&()) }
    }

    unsafe fn rendezvous_with_observation(
        &mut self,
        observation: &dyn ApObservation,
    ) -> Result<CpuReport, CpuError> {
        unsafe { self.recheck() }?;
        let pool = self.pool.ok_or(CpuError::Released)?;
        for number in 0..self.report.total_processors {
            self.record(number)?.completed.store(0, Ordering::Relaxed);
        }
        self.report.completed_ap_callbacks = 0;
        self.rendezvous = NEXT_RENDEZVOUS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
            .map_err(|_| CpuError::Bounds)?;
        let slot = Dispatch {
            protocol: self.protocol,
            records: pool.as_ptr(),
            count: self.report.total_processors,
            bsp: self.report.bsp_number,
            invalid: AtomicBool::new(false),
            observation,
            rendezvous: self.rendezvous,
        };
        if self.report.enabled_aps != 0 {
            let status = unsafe {
                (self.protocol.startup_all_aps)(
                    self.protocol,
                    ap_observation,
                    Boolean(0),
                    ptr::null_mut(), // blocking: never retain stack arguments
                    AP_TIMEOUT_MICROSECONDS,
                    (&slot as *const Dispatch<'_>).cast_mut().cast(),
                    ptr::null_mut(), // no optional failed-list allocation
                )
            };
            if status != Status::SUCCESS {
                return Err(CpuError::Dispatch(status));
            }
        }
        if slot.invalid.load(Ordering::Acquire) {
            return Err(CpuError::Completion);
        }
        for number in 0..self.report.total_processors {
            let record = self.record(number)?;
            let expected = usize::from(
                number != self.report.bsp_number && record.information.status_flag & ENABLED != 0,
            );
            if record.completed.load(Ordering::Acquire) != 2 * expected {
                return Err(CpuError::Completion);
            }
        }
        unsafe { self.recheck() }?;
        self.report.completed_ap_callbacks = self.report.enabled_aps;
        Ok(self.report)
    }

    unsafe fn recheck(&self) -> Result<(), CpuError> {
        let (total, enabled) = unsafe { counts(self.protocol) }?;
        if total != self.report.total_processors || enabled != self.report.enabled_processors {
            return Err(CpuError::Changed);
        }
        for number in 0..total {
            let information = unsafe { information(self.protocol, number) }?;
            if self.record(number)?.information != information {
                return Err(CpuError::Changed);
            }
        }
        if unsafe { identity(self.protocol) }? != self.report.bsp_number {
            return Err(CpuError::Changed);
        }
        Ok(())
    }

    /// Enter a short high-TPL BSP-only interval after a finished rendezvous.
    /// The guard is borrowed, cannot escape, and is neither Send nor Sync.
    ///
    /// # Safety
    /// Entry must be at TPL_APPLICATION. The conforming provider must support
    /// blocking dispatch at TPL_NOTIFY (the default protocol maximum), without
    /// relying on delivery of lower/equal-TPL events to finish or time out. The
    /// final barrier and inventory run at NOTIFY, masking callback AP dispatch;
    /// HIGH_LEVEL covers only the closure. Hardware interrupt handlers and SMM
    /// must obey the firmware's normal rule against calling MP Services there.
    /// The closure must make no firmware calls, allocate, dispatch APs, unwind,
    /// change TPL, or leave altered machine state on return. It must preserve the
    /// high-TPL interrupt-disabled state. Ordinary SMM/NMI/INIT are not excluded;
    /// any SVM interval must separately implement their architectural handling.
    pub unsafe fn with_quiescent_bsp<R>(
        &mut self,
        operation: impl FnOnce(&QuiescentBsp<'_>) -> R,
    ) -> Result<(CpuReport, R), CpuError> {
        match unsafe {
            self.with_prepared_quiescent_bsp(
                || Ok::<(), Infallible>(()),
                |guard, ()| operation(guard),
                |()| (),
            )
        } {
            Ok(completion) => completion.outcome,
            Err(PreparedScopeError::Cpu(error)) => Err(error),
            Err(PreparedScopeError::Preparation(never)) => match never {},
        }
    }

    /// Prepare resources and release them without opening a firmware callback
    /// window before or after the high-TPL operation. Preparation and finish
    /// run at NOTIFY; only operation runs at HIGH_LEVEL. An initial rendezvous
    /// rejects already-busy APs before preparation can dereference firmware
    /// mappings. A second rendezvous validates completion after preparation.
    /// A successful prepare
    /// always receives finish, including CPU refusal paths, and P is dropped
    /// before APPLICATION is restored. The result preserves CPU and explicit
    /// cleanup errors independently; Drop remains a final cleanup safeguard.
    ///
    /// # Safety
    /// All with_quiescent_bsp requirements apply. Preparation and finish must
    /// obey TPL_NOTIFY restrictions, must not alter TPL or unwind, and must not
    /// leak/store their owned resources outside P. Preparation owns cleanup if
    /// it returns Err. Operation and its captured destructors must not call
    /// firmware at HIGH_LEVEL. Neither operation nor finish may move a resource
    /// out of P to extend its lifetime past this scope. P::drop must be valid at
    /// NOTIFY. Failed explicit cleanup remains visible even if Drop retries.
    pub unsafe fn with_prepared_quiescent_bsp<P, E, R, F>(
        &mut self,
        prepare: impl FnOnce() -> Result<P, E>,
        operation: impl FnOnce(&QuiescentBsp<'_>, &mut P) -> R,
        finish: impl FnOnce(&mut P) -> F,
    ) -> Result<PreparedScopeCompletion<R, F>, PreparedScopeError<E>> {
        unsafe {
            self.with_prepared_quiescent_bsp_and_ap_observation(prepare, |_| &(), operation, finish)
        }
    }

    /// Extend the final rendezvous with an observer borrowed from P. Only that
    /// shared Sync projection reaches APs; P itself need not be Sync and is not
    /// mutably borrowed during dispatch. The borrow ends before operation/finish.
    /// The initial identity-only barrier and all cleanup paths are unchanged.
    ///
    /// # Safety
    /// All with_prepared_quiescent_bsp contracts apply. The observer must uphold
    /// ApObservation's callback contract and remain allocated through blocking
    /// return, including timeout termination. Projection runs at NOTIFY and
    /// must not unwind, alter TPL, or transfer scoped resources elsewhere.
    pub unsafe fn with_prepared_quiescent_bsp_and_ap_observation<P, E, R, F>(
        &mut self,
        prepare: impl FnOnce() -> Result<P, E>,
        observation: impl FnOnce(&P) -> &dyn ApObservation,
        operation: impl FnOnce(&QuiescentBsp<'_>, &mut P) -> R,
        finish: impl FnOnce(&mut P) -> F,
    ) -> Result<PreparedScopeCompletion<R, F>, PreparedScopeError<E>> {
        // Determine TPL without ever lowering it via RaiseTPL. Preparation
        // already checked entry; this also rejects a changed invocation TPL.
        let previous = unsafe { (self.services.raise_tpl)(Tpl::HIGH_LEVEL) };
        unsafe { (self.services.restore_tpl)(previous) };
        if previous != Tpl::APPLICATION {
            return Err(PreparedScopeError::Cpu(CpuError::EntryTpl));
        }
        let previous = unsafe { (self.services.raise_tpl)(Tpl::NOTIFY) };
        let notify = TplScope { services: self.services, previous };
        if previous != Tpl::APPLICATION {
            return Err(PreparedScopeError::Cpu(CpuError::EntryTpl));
        }
        // Preparation may read validated firmware pointers. Reject outstanding
        // AP work first, while NOTIFY already masks new callback dispatches.
        unsafe { self.rendezvous() }.map_err(PreparedScopeError::Cpu)?;
        // Declared after notify so even ordinary Rust early-exit cleanup drops
        // the prepared value before the TPL guard can deliver pending callbacks.
        let mut prepared = prepare().map_err(PreparedScopeError::Preparation)?;
        let outcome = unsafe { self.operate_at_high(&mut prepared, observation, operation) };
        let cleanup = finish(&mut prepared);
        drop(prepared);
        drop(notify); // Restoring APPLICATION may dispatch pending events.
        Ok(PreparedScopeCompletion { outcome, cleanup })
    }

    // Internal scope called only with TPL_NOTIFY held by the outer owner. Keep
    // fallible high/CPU paths here so every return rejoins explicit finish.
    unsafe fn operate_at_high<P, R>(
        &mut self,
        prepared: &mut P,
        observation: impl FnOnce(&P) -> &dyn ApObservation,
        operation: impl FnOnce(&QuiescentBsp<'_>, &mut P) -> R,
    ) -> Result<(CpuReport, R), CpuError> {
        let report = unsafe { self.rendezvous_with_observation(observation(prepared)) }?;
        let previous = unsafe { (self.services.raise_tpl)(Tpl::HIGH_LEVEL) };
        let high = TplScope {
            services: self.services,
            // NOTIFY is the state we established, even if the provider reports
            // a contradictory old value. Restore it before finishing refusal.
            previous: Tpl::NOTIFY,
        };
        let result =
            Self::checked_high_operation(previous, report, self.rendezvous, prepared, operation);
        drop(high); // No MP or pool operations at HIGH_LEVEL.
        let result = result?;
        unsafe { self.recheck() }?;
        Ok((report, result))
    }

    fn checked_high_operation<P, R>(
        previous: Tpl,
        report: CpuReport,
        rendezvous: usize,
        prepared: &mut P,
        operation: impl FnOnce(&QuiescentBsp<'_>, &mut P) -> R,
    ) -> Result<R, CpuError> {
        if previous != Tpl::NOTIFY {
            Err(CpuError::EntryTpl)
        } else {
            let guard =
                QuiescentBsp { report, rendezvous, scope: PhantomData, not_send_sync: PhantomData };
            Ok(operation(&guard, prepared))
        }
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

impl Drop for PreparedCpus<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub struct QuiescentBsp<'a> {
    report: CpuReport,
    rendezvous: usize,
    scope: PhantomData<&'a mut ()>,
    not_send_sync: PhantomData<*mut ()>,
}

impl QuiescentBsp<'_> {
    pub const fn report(&self) -> CpuReport {
        self.report
    }

    pub const fn rendezvous(&self) -> usize {
        self.rendezvous
    }
}

/// A callback identity, created only after real WhoAmI and an exclusive slot
/// claim. This is an AP callback scope, never a BSP/high-TPL ownership guard.
pub struct DispatchedAp<'a> {
    number: usize,
    information: ProcessorInformation,
    rendezvous: usize,
    scope: PhantomData<&'a ()>,
    not_send_sync: PhantomData<*mut ()>,
}

impl DispatchedAp<'_> {
    pub const fn number(&self) -> usize {
        self.number
    }

    pub const fn information(&self) -> ProcessorInformation {
        self.information
    }

    /// Freshness binding shared with the BSP guard of this completed round.
    pub const fn rendezvous(&self) -> usize {
        self.rendezvous
    }
}

/// Shared work performed once by each enabled AP during the final barrier.
///
/// # Safety
/// Implementations must use disjoint or synchronized storage, return without
/// unwinding, preserve the AP's architectural state, and make no firmware calls,
/// allocate, change TPL, dispatch work, or retain the guard. WhoAmI has already
/// run in the CPU helper. No particular incoming AP interrupt/flag profile is
/// promised: an observer must check any extra architectural requirements and
/// retain a refusal when unsupported. All storage must survive blocking return,
/// including the provider's termination of outstanding callbacks on timeout.
pub unsafe trait ApObservation: Sync {
    /// # Safety
    /// Called only within a conforming MP callback with the supplied identity.
    unsafe fn observe(&self, ap: &DispatchedAp<'_>);
}

unsafe impl ApObservation for () {
    unsafe fn observe(&self, _: &DispatchedAp<'_>) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuReport {
    pub total_processors: usize,
    pub enabled_processors: usize,
    pub enabled_aps: usize,
    pub bsp_number: usize,
    pub bsp_processor_id: u64,
    pub completed_ap_callbacks: usize,
    /// This adapter dispatches only an observation callback on APs. A later
    /// synchronous probe interval runs on the BSP alone, irrespective of APs.
    pub probe_processors: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedScopeCompletion<R, F> {
    pub outcome: Result<(CpuReport, R), CpuError>,
    pub cleanup: F,
}

/// PI's legacy (non-CPU_V2_EXTENDED_TOPOLOGY) processor record.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessorInformation {
    pub processor_id: u64,
    pub status_flag: u32,
    pub package: u32,
    pub core: u32,
    pub thread: u32,
}

const _: () = assert!(size_of::<ProcessorInformation>() == 24);

pub type ApProcedure = extern "efiapi" fn(*mut c_void);

/// uefi-raw 0.15.1 has no MP Services definition. Keep all seven PI slots in
/// their specified order, using uefi-raw's ABI scalar types.
#[repr(C)]
pub struct MpServicesProtocol {
    pub get_number_of_processors:
        unsafe extern "efiapi" fn(*const Self, *mut usize, *mut usize) -> Status,
    pub get_processor_info:
        unsafe extern "efiapi" fn(*const Self, usize, *mut ProcessorInformation) -> Status,
    pub startup_all_aps: unsafe extern "efiapi" fn(
        *const Self,
        ApProcedure,
        Boolean,
        Event,
        usize,
        *mut c_void,
        *mut *mut usize,
    ) -> Status,
    pub startup_this_ap: unsafe extern "efiapi" fn(
        *const Self,
        ApProcedure,
        usize,
        Event,
        usize,
        *mut c_void,
        *mut Boolean,
    ) -> Status,
    pub switch_bsp: unsafe extern "efiapi" fn(*const Self, usize, Boolean) -> Status,
    pub enable_disable_ap:
        unsafe extern "efiapi" fn(*const Self, usize, Boolean, *const u32) -> Status,
    pub who_am_i: unsafe extern "efiapi" fn(*const Self, *mut usize) -> Status,
}

const _: () = assert!(size_of::<MpServicesProtocol>() == 7 * size_of::<usize>());

#[repr(C)]
struct Record {
    information: ProcessorInformation,
    completed: AtomicUsize,
}
const _: () = assert!(align_of::<Record>() <= 8);

struct Dispatch<'a> {
    protocol: *const MpServicesProtocol,
    records: *const Record,
    count: usize,
    bsp: usize,
    invalid: AtomicBool,
    observation: &'a dyn ApObservation,
    rendezvous: usize,
}

struct TplScope<'a> {
    services: &'a BootServices,
    previous: Tpl,
}

impl Drop for TplScope<'_> {
    fn drop(&mut self) {
        unsafe { (self.services.restore_tpl)(self.previous) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuError {
    Protocol(Status),
    NullProtocol,
    Firmware(Status),
    Bounds,
    Inventory,
    NotBsp,
    Unhealthy,
    Changed,
    Allocation(Status),
    Layout,
    Dispatch(Status),
    Completion,
    EntryTpl,
    Released,
    Cleanup(Status),
}

/// Failure before preparation produced an owned value. Once preparation has
/// succeeded, CPU errors are returned alongside the explicit cleanup outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedScopeError<E> {
    Cpu(CpuError),
    Preparation(E),
}

/// Allocate bounded storage and bind the caller to the sole enabled healthy BSP.
/// An unhealthy enabled CPU, inconsistent inventory, or absent MP protocol is a
/// refusal. No single-CPU fallback invents MP ownership evidence.
///
/// # Safety
/// Live trusted Boot Services at TPL_APPLICATION, before ExitBootServices. The
/// located protocol must remain installed and services live until release.
pub unsafe fn prepare(services: &BootServices) -> Result<PreparedCpus<'_>, CpuError> {
    let previous = unsafe { (services.raise_tpl)(Tpl::HIGH_LEVEL) };
    unsafe { (services.restore_tpl)(previous) };
    if previous != Tpl::APPLICATION {
        return Err(CpuError::EntryTpl);
    }
    let mut interface = ptr::null_mut();
    let status =
        unsafe { (services.locate_protocol)(&MP_SERVICES_GUID, ptr::null_mut(), &mut interface) };
    if status != Status::SUCCESS {
        return Err(CpuError::Protocol(status));
    }
    if interface.is_null() || interface.addr() % align_of::<MpServicesProtocol>() != 0 {
        return Err(CpuError::NullProtocol);
    }
    let protocol = unsafe { &*interface.cast::<MpServicesProtocol>() };
    let bsp = unsafe { identity(protocol) }?;
    let (total, enabled) = unsafe { counts(protocol) }?;
    if bsp >= total {
        return Err(CpuError::NotBsp);
    }
    let mut raw = ptr::null_mut();
    let status = unsafe {
        (services.allocate_pool)(
            MemoryType::BOOT_SERVICES_DATA,
            total * size_of::<Record>(),
            &mut raw,
        )
    };
    if status != Status::SUCCESS {
        return Err(CpuError::Allocation(status));
    }
    let pool = NonNull::new(raw.cast::<Record>()).ok_or(CpuError::Layout)?;
    let mut owned = PreparedCpus {
        services,
        protocol,
        pool: Some(pool),
        report: CpuReport {
            total_processors: total,
            enabled_processors: enabled,
            enabled_aps: enabled - 1,
            bsp_number: bsp,
            bsp_processor_id: 0,
            completed_ap_callbacks: 0,
            probe_processors: 1,
        },
        rendezvous: 0,
        not_send_sync: PhantomData,
    };
    let result = unsafe { initialize(&mut owned) };
    if let Err(error) = result {
        owned.release().map_err(CpuError::Cleanup)?;
        return Err(error);
    }
    Ok(owned)
}

/// Complete the real observation and release all temporary pool storage.
///
/// # Safety
/// Same contracts as prepare and rendezvous. This reports no retained lease.
pub unsafe fn observe(services: &BootServices) -> Result<CpuReport, CpuError> {
    let mut prepared = unsafe { prepare(services) }?;
    let result = unsafe { prepared.rendezvous() };
    prepared.release().map_err(CpuError::Cleanup)?;
    result
}

/// Prepare and release storage around one scoped BSP operation. Prefer prepare
/// plus PreparedCpus::with_quiescent_bsp when other buffers must be allocated
/// before the final barrier. The result report is historical once this returns.
///
/// # Safety
/// All contracts of prepare and PreparedCpus::with_quiescent_bsp apply.
pub unsafe fn with_quiescent_cpu<R>(
    services: &BootServices,
    operation: impl FnOnce(&QuiescentBsp<'_>) -> R,
) -> Result<(CpuReport, R), CpuError> {
    let mut prepared = unsafe { prepare(services) }?;
    let result = unsafe { prepared.with_quiescent_bsp(operation) };
    prepared.release().map_err(CpuError::Cleanup)?;
    result
}

unsafe fn initialize(owned: &mut PreparedCpus<'_>) -> Result<(), CpuError> {
    let pool = owned.pool.ok_or(CpuError::Released)?;
    if pool.as_ptr().addr() % align_of::<Record>() != 0 {
        return Err(CpuError::Layout);
    }
    let mut enabled = 0;
    for number in 0..owned.report.total_processors {
        let information = unsafe { information(owned.protocol, number) }?;
        if information.status_flag & !7 != 0 {
            return Err(CpuError::Inventory);
        }
        if (information.status_flag & BSP != 0) != (number == owned.report.bsp_number) {
            return Err(CpuError::NotBsp);
        }
        if number == owned.report.bsp_number {
            if information.status_flag & ENABLED == 0 {
                return Err(CpuError::NotBsp);
            }
            owned.report.bsp_processor_id = information.processor_id;
        }
        if information.status_flag & ENABLED != 0 {
            enabled += 1;
            if information.status_flag & HEALTHY == 0 {
                return Err(CpuError::Unhealthy);
            }
        }
        for earlier in 0..number {
            if owned.record(earlier)?.information.processor_id == information.processor_id {
                return Err(CpuError::Inventory);
            }
        }
        unsafe {
            pool.as_ptr().add(number).write(Record { information, completed: AtomicUsize::new(0) });
        }
    }
    if enabled != owned.report.enabled_processors {
        return Err(CpuError::Inventory);
    }
    unsafe { owned.recheck() }
}

extern "efiapi" fn ap_observation(argument: *mut c_void) {
    // This slot and the record pool outlive a blocking dispatch, including the
    // provider's timeout termination. No allocation/locks/ordinary BS on APs.
    let Some(slot) = (unsafe { argument.cast::<Dispatch<'_>>().as_ref() }) else {
        return;
    };
    let mut number = usize::MAX;
    let status = unsafe { ((*slot.protocol).who_am_i)(slot.protocol, &mut number) };
    if status != Status::SUCCESS || number >= slot.count || number == slot.bsp {
        slot.invalid.store(true, Ordering::Release);
        return;
    }
    let record = unsafe { &*slot.records.add(number) };
    if record.information.status_flag & ENABLED == 0
        || record.completed.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err()
    {
        slot.invalid.store(true, Ordering::Release);
        return;
    }
    let ap = DispatchedAp {
        number,
        information: record.information,
        rendezvous: slot.rendezvous,
        scope: PhantomData,
        not_send_sync: PhantomData,
    };
    unsafe { slot.observation.observe(&ap) };
    record.completed.store(2, Ordering::Release);
}

unsafe fn identity(protocol: &MpServicesProtocol) -> Result<usize, CpuError> {
    let mut number = usize::MAX;
    let status = unsafe { (protocol.who_am_i)(protocol, &mut number) };
    if status != Status::SUCCESS {
        return Err(CpuError::Firmware(status));
    }
    Ok(number)
}

unsafe fn counts(protocol: &MpServicesProtocol) -> Result<(usize, usize), CpuError> {
    let (mut total, mut enabled) = (0, 0);
    let status = unsafe { (protocol.get_number_of_processors)(protocol, &mut total, &mut enabled) };
    if status != Status::SUCCESS {
        return Err(CpuError::Firmware(status));
    }
    if total == 0 || total > MAX_PROCESSORS || enabled == 0 || enabled > total {
        return Err(CpuError::Bounds);
    }
    Ok((total, enabled))
}

unsafe fn information(
    protocol: &MpServicesProtocol,
    number: usize,
) -> Result<ProcessorInformation, CpuError> {
    let mut information = ProcessorInformation::default();
    let status = unsafe { (protocol.get_processor_info)(protocol, number, &mut information) };
    if status != Status::SUCCESS {
        return Err(CpuError::Firmware(status));
    }
    Ok(information)
}
