#![cfg(feature = "native-preflight")]

// The included admission files name their siblings `super::{cache, cpu,
// snapshot}`; this crate root provides those same names.
// Partial include: resource-observation constants are unused here.
#[path = "../src/native/admission/cache/mod.rs"]
#[allow(dead_code)]
mod cache;
#[path = "../src/native/admission/cache_rendezvous/mod.rs"]
mod cache_rendezvous;
#[path = "../src/native/admission/cpu.rs"]
mod cpu;

use core::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr,
};
use std::{
    alloc::{Layout, alloc, dealloc},
    cell::RefCell,
    collections::BTreeMap,
};

use svmvisor_dxe::native::admission::snapshot;
use uefi_raw::{
    Boolean, Event, Guid, Status,
    table::boot::{BootServices, MemoryType, Tpl},
};

use crate::cpu::*;

const MOCK_CR3: u64 = 0x1234_5018;

thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
thread_local! { static AP_IDENTITY: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
thread_local! { static CACHE_READ_SUCCESS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }

static PROTOCOL: MpServicesProtocol = MpServicesProtocol {
    get_number_of_processors: counts,
    get_processor_info: information,
    startup_all_aps: dispatch,
    startup_this_ap: this_ap,
    switch_bsp: switch,
    enable_disable_ap: enable,
    who_am_i: identity,
};

thread_local! { static AP_ROOT_BIT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
thread_local! { static BAD_ROOT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
thread_local! { static CR4_DIFFERENCE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
thread_local! { static DIAGNOSTIC_FLAGS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

struct State {
    records: Vec<ProcessorInformation>,
    counts: Option<(usize, usize)>,
    identity: usize,
    protocol_status: Status,
    null_protocol: bool,
    allocation_status: Status,
    dispatch_status: Status,
    skip_ap: Option<usize>,
    duplicate_ap: bool,
    invalid_ap: bool,
    mutate_after_dispatch: bool,
    mutate_bsp_after_dispatch: bool,
    fail_free: usize,
    allocations: BTreeMap<usize, Layout>,
    free_calls: usize,
    dispatches: usize,
    tpl: Tpl,
    dispatch_tpl: Option<Tpl>,
    corrupt_high_previous: bool,
    calls: Vec<&'static str>,
    cache_status: BTreeMap<usize, u32>,
    cache_changes: BTreeMap<usize, fn(&mut cache::CacheSnapshot)>,
    cache_reads: Vec<usize>,
    paging_status: BTreeMap<usize, u32>,
    paging_changes: BTreeMap<usize, fn(&mut [u64; 9])>,
    paging_reads: Vec<usize>,
    concurrent_dispatch: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            records: (0..4)
                .map(|n| ProcessorInformation {
                    processor_id: 0x100 + n,
                    status_flag: if n == 0 { 7 } else { 6 },
                    package: 0,
                    core: n as u32,
                    thread: 0,
                })
                .collect(),
            counts: None,
            identity: 0,
            protocol_status: Status::SUCCESS,
            null_protocol: false,
            allocation_status: Status::SUCCESS,
            dispatch_status: Status::SUCCESS,
            skip_ap: None,
            duplicate_ap: false,
            invalid_ap: false,
            mutate_after_dispatch: false,
            mutate_bsp_after_dispatch: false,
            fail_free: 0,
            allocations: BTreeMap::new(),
            free_calls: 0,
            dispatches: 0,
            tpl: Tpl::APPLICATION,
            dispatch_tpl: None,
            corrupt_high_previous: false,
            calls: Vec::new(),
            cache_status: BTreeMap::new(),
            cache_changes: BTreeMap::new(),
            cache_reads: Vec::new(),
            paging_status: BTreeMap::new(),
            paging_changes: BTreeMap::new(),
            paging_reads: Vec::new(),
            concurrent_dispatch: false,
        }
    }
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn setup() -> BootServices {
    with(|s| {
        assert!(s.allocations.is_empty());
        *s = State::default();
    });
    let mut raw = MaybeUninit::<BootServices>::uninit();
    unsafe {
        // Unused service slots are non-null function addresses and never called.
        for i in 0..size_of::<BootServices>() / size_of::<usize>() {
            raw.as_mut_ptr().cast::<usize>().add(i).write(unused as *const () as usize);
        }
        ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(core::mem::zeroed());
        ptr::addr_of_mut!((*raw.as_mut_ptr()).raise_tpl).write(raise_tpl);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).restore_tpl).write(restore_tpl);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).locate_protocol).write(locate);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).allocate_pool).write(allocate_pool);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pool).write(free_pool);
        raw.assume_init()
    }
}

unsafe extern "efiapi" fn unused() {
    panic!("unexpected firmware call");
}

unsafe extern "efiapi" fn raise_tpl(tpl: Tpl) -> Tpl {
    with(|s| {
        assert!(tpl.0 >= s.tpl.0);
        let old = s.tpl;
        s.tpl = tpl;
        s.calls.push("raise");
        if s.corrupt_high_previous && old == Tpl::NOTIFY && tpl == Tpl::HIGH_LEVEL {
            Tpl::APPLICATION
        } else {
            old
        }
    })
}

unsafe extern "efiapi" fn restore_tpl(tpl: Tpl) {
    with(|s| {
        assert!(tpl.0 <= s.tpl.0);
        s.tpl = tpl;
        s.calls.push("restore");
    });
}

unsafe extern "efiapi" fn locate(
    guid: *const Guid,
    registration: *mut c_void,
    result: *mut *mut c_void,
) -> Status {
    assert_eq!(unsafe { *guid }, MP_SERVICES_GUID);
    assert!(registration.is_null());
    with(|s| {
        s.calls.push("locate");
        assert_eq!(s.tpl, Tpl::APPLICATION);
        unsafe {
            *result = if s.null_protocol {
                ptr::null_mut()
            } else {
                (&PROTOCOL as *const MpServicesProtocol).cast_mut().cast()
            };
        }
        s.protocol_status
    })
}

unsafe extern "efiapi" fn allocate_pool(
    kind: MemoryType,
    size: usize,
    result: *mut *mut u8,
) -> Status {
    with(|s| {
        s.calls.push("allocate");
        assert_eq!(kind, MemoryType::BOOT_SERVICES_DATA);
        assert!(s.tpl.0 <= Tpl::NOTIFY.0);
        if s.allocation_status != Status::SUCCESS {
            return s.allocation_status;
        }
        assert!(size <= MAX_PROCESSORS * 416);
        let layout = Layout::from_size_align(size, 8).unwrap();
        let p = unsafe { alloc(layout) };
        assert!(!p.is_null());
        s.allocations.insert(p as usize, layout);
        unsafe {
            *result = p;
        }
        Status::SUCCESS
    })
}

unsafe extern "efiapi" fn free_pool(p: *mut u8) -> Status {
    with(|s| {
        s.calls.push("free");
        s.free_calls += 1;
        assert!(s.tpl.0 <= Tpl::NOTIFY.0);
        if s.fail_free != 0 {
            s.fail_free -= 1;
            return Status::DEVICE_ERROR;
        }
        let layout = s.allocations.remove(&(p as usize)).expect("free owned allocation once");
        unsafe {
            dealloc(p, layout);
        }
        Status::SUCCESS
    })
}

unsafe extern "efiapi" fn counts(
    _: *const MpServicesProtocol,
    total: *mut usize,
    enabled: *mut usize,
) -> Status {
    with(|s| {
        assert!(s.tpl.0 <= Tpl::NOTIFY.0);
        s.calls.push("counts");
        let pair = s.counts.unwrap_or((
            s.records.len(),
            s.records.iter().filter(|r| r.status_flag & 2 != 0).count(),
        ));
        unsafe {
            *total = pair.0;
            *enabled = pair.1;
        }
        Status::SUCCESS
    })
}

unsafe extern "efiapi" fn information(
    _: *const MpServicesProtocol,
    index: usize,
    result: *mut ProcessorInformation,
) -> Status {
    with(|s| {
        assert!(s.tpl.0 <= Tpl::NOTIFY.0);
        s.calls.push("info");
        unsafe {
            *result = s.records[index];
        }
        Status::SUCCESS
    })
}

unsafe extern "efiapi" fn identity(_: *const MpServicesProtocol, result: *mut usize) -> Status {
    if let Some(number) = AP_IDENTITY.get() {
        unsafe { *result = number };
        return Status::SUCCESS;
    }
    with(|s| {
        assert!(s.tpl.0 <= Tpl::NOTIFY.0);
        s.calls.push("identity");
        unsafe {
            *result = s.identity;
        }
        Status::SUCCESS
    })
}

unsafe extern "efiapi" fn dispatch(
    _: *const MpServicesProtocol,
    procedure: ApProcedure,
    single: Boolean,
    event: Event,
    timeout: usize,
    argument: *mut c_void,
    failed: *mut *mut usize,
) -> Status {
    assert_eq!(single.0, 0);
    assert!(event.is_null());
    assert!(!argument.is_null());
    assert_eq!(timeout, AP_TIMEOUT_MICROSECONDS);
    assert!(timeout > 0);
    assert!(failed.is_null());
    let (status, targets, old_identity, duplicate, invalid, concurrent) = with(|s| {
        s.dispatches += 1;
        s.dispatch_tpl = Some(s.tpl);
        s.calls.push("dispatch");
        assert!(s.tpl.0 <= Tpl::NOTIFY.0);
        (
            s.dispatch_status,
            s.records
                .iter()
                .enumerate()
                .filter(|(n, r)| {
                    r.status_flag & 2 != 0 && r.status_flag & 1 == 0 && s.skip_ap != Some(*n)
                })
                .map(|(n, _)| n)
                .collect::<Vec<_>>(),
            s.identity,
            s.duplicate_ap,
            s.invalid_ap,
            s.concurrent_dispatch,
        )
    });
    if status == Status::NOT_READY {
        return status;
    }
    if concurrent {
        let argument = argument as usize;
        std::thread::scope(|scope| {
            for target in targets {
                for _ in 0..if duplicate { 2 } else { 1 } {
                    scope.spawn(move || {
                        AP_IDENTITY.set(Some(if invalid { MAX_PROCESSORS + 1 } else { target }));
                        procedure(argument as *mut c_void);
                        AP_IDENTITY.set(None);
                    });
                }
            }
        });
    } else {
        for target in targets {
            with(|s| s.identity = if invalid { MAX_PROCESSORS + 1 } else { target });
            procedure(argument);
            if duplicate {
                procedure(argument);
            }
        }
    }
    // Even TIMEOUT returns only after every callback is finished/terminated.
    with(|s| {
        s.identity = old_identity;
        if s.mutate_after_dispatch {
            s.records[1].processor_id += 100;
        }
        if s.mutate_bsp_after_dispatch {
            s.identity = 1;
        }
    });
    status
}

unsafe extern "efiapi" fn this_ap(
    _: *const MpServicesProtocol,
    _: ApProcedure,
    _: usize,
    _: Event,
    _: usize,
    _: *mut c_void,
    _: *mut Boolean,
) -> Status {
    panic!("no per-AP dispatch");
}

unsafe extern "efiapi" fn switch(_: *const MpServicesProtocol, _: usize, _: Boolean) -> Status {
    panic!("no BSP switch");
}

unsafe extern "efiapi" fn enable(
    _: *const MpServicesProtocol,
    _: usize,
    _: Boolean,
    _: *const u32,
) -> Status {
    panic!("no enable/disable");
}

fn clean() {
    with(|s| {
        assert!(s.allocations.is_empty());
        assert_eq!(s.tpl, Tpl::APPLICATION);
    });
}

struct ScopedPool<'a> {
    services: &'a BootServices,
    pool: Option<*mut u8>,
}

impl<'a> ScopedPool<'a> {
    fn prepare(services: &'a BootServices) -> Result<Self, Status> {
        with(|s| {
            assert_eq!(s.tpl, Tpl::NOTIFY);
            s.calls.push("prepare_resource");
        });
        let mut pool = ptr::null_mut();
        let status =
            unsafe { (services.allocate_pool)(MemoryType::BOOT_SERVICES_DATA, 64, &mut pool) };
        if status != Status::SUCCESS {
            return Err(status);
        }
        Ok(Self { services, pool: Some(pool) })
    }

    fn release(&mut self) -> Result<(), Status> {
        with(|s| assert_eq!(s.tpl, Tpl::NOTIFY));
        if let Some(pool) = self.pool {
            let status = unsafe { (self.services.free_pool)(pool) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.pool = None;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Status> {
        with(|s| {
            assert_eq!(s.tpl, Tpl::NOTIFY);
            s.calls.push("finish_resource");
        });
        self.release()
    }
}

impl Drop for ScopedPool<'_> {
    fn drop(&mut self) {
        with(|s| {
            assert_eq!(s.tpl, Tpl::NOTIFY);
            s.calls.push("drop_resource");
        });
        let _ = self.release();
    }
}

fn cache_snapshot(number: usize) -> cache::CacheSnapshot {
    use cache::{CacheSnapshot, TARGET_SIGNATURE, captured};
    CacheSnapshot {
        abi_version: 1,
        captured_fields: captured::REQUIRED,
        msr_reads: 30,
        signature: TARGET_SIGNATURE,
        max_basic: 0x10,
        max_extended: 0x8000_0026,
        leaf1_edx: 0x0001_1020,
        physical_bits: 48,
        encryption_eax: 1,
        encryption_ebx: 51 | (5 << 6),
        initial_apic_id: number as u32,
        rflags: if number == 0 { 2 } else { 0x8d7 },
        cr0: 0x8001_0033,
        cr4: 0x620,
        efer: 0xd00,
        sys_cfg: 1 << 20,
        pat: 0x0007_0406_0007_0406,
        mtrr_cap: 0x508,
        mtrr_default: 0x806,
        top_mem: 0x0800_0000,
        apic_base: if number == 0 { 0xfee0_0900 } else { 0xfee0_0800 },
        ..CacheSnapshot::default()
    }
}

// Explicit supplied-data reader only. No privileged instruction is executed by
// host tests; target UEFI links the separately reviewed real assembly symbol.
#[unsafe(no_mangle)]
unsafe extern "efiapi" fn svmvisor_native_cache_read(out: *mut cache::CacheSnapshot) -> u32 {
    if let Some(number) = AP_IDENTITY.get() {
        unsafe { out.write(cache_snapshot(number)) };
        CACHE_READ_SUCCESS.set(Some(number));
        return 0;
    }
    with(|s| {
        let number = s.identity;
        assert_eq!(s.tpl, if number == 0 { Tpl::HIGH_LEVEL } else { Tpl::NOTIFY });
        assert!(s.allocations.iter().any(|(base, layout)| {
            out.addr() >= *base && out.addr() + cache::SNAPSHOT_BYTES <= *base + layout.size()
        }));
        s.cache_reads.push(number);
        let mut snapshot = cache_snapshot(number);
        if let Some(change) = s.cache_changes.get(&number) {
            change(&mut snapshot);
        }
        unsafe { out.write(snapshot) };
        let status = s.cache_status.get(&number).copied().unwrap_or(0);
        CACHE_READ_SUCCESS.set(if status == 0 { Some(number) } else { None });
        status
    })
}

fn paging_snapshot(cache: &cache::CacheSnapshot) -> [u64; 9] {
    // Independent supplied bytes for native_snapshot.S's existing 72-byte ABI.
    // Distinct AP descriptor/selectors are deliberately not BSP comparisons.
    [
        0x2000 + u64::from(cache.initial_apic_id),
        0,
        0x3000 + u64::from(cache.initial_apic_id),
        0,
        0x38 + u64::from(cache.initial_apic_id) * 8,
        cache.cr0,
        MOCK_CR3,
        cache.cr4,
        cache.rflags,
    ]
}

// Explicit host stand-in. It executes no privileged instruction and is not
// compiled into the UEFI driver. The per-thread order assertion applies to the
// concurrent AP fixture as well as the serial MP/BSP paths.
#[unsafe(no_mangle)]
unsafe extern "efiapi" fn svmvisor_native_snapshot(out: *mut snapshot::NativeSnapshot) -> u32 {
    let out = out.cast::<[u64; 9]>();
    assert_eq!(out.addr() % 8, 0);
    if let Some(number) = AP_IDENTITY.get() {
        assert_eq!(CACHE_READ_SUCCESS.take(), Some(number));
        unsafe { out.write(paging_snapshot(&cache_snapshot(number))) };
        return 0;
    }
    with(|s| {
        let number = s.identity;
        assert_eq!(CACHE_READ_SUCCESS.take(), Some(number));
        assert_eq!(s.tpl, if number == 0 { Tpl::HIGH_LEVEL } else { Tpl::NOTIFY });
        assert_eq!(s.cache_reads.last(), Some(&number));
        s.paging_reads.push(number);
        let mut cache = cache_snapshot(number);
        if let Some(change) = s.cache_changes.get(&number) {
            change(&mut cache);
        }
        let mut snapshot = paging_snapshot(&cache);
        if let Some(change) = s.paging_changes.get(&number) {
            change(&mut snapshot);
        }
        unsafe { out.write(snapshot) };
        s.paging_status.get(&number).copied().unwrap_or(0)
    })
}

struct RepeatedObserver<'a>(cache_rendezvous::PreparedCacheRendezvous<'a>);

unsafe impl ApObservation for RepeatedObserver<'_> {
    unsafe fn observe(&self, ap: &DispatchedAp<'_>) {
        unsafe {
            self.0.observe(ap);
            self.0.observe(ap);
        }
    }
}

fn assert_cache_diagnostic(
    cache: &cache_rendezvous::PreparedCacheRendezvous<'_>,
    error: cache_rendezvous::RendezvousError,
    expected: u64,
) {
    let before = with(|s| {
        (
            s.calls.clone(),
            s.cache_reads.clone(),
            s.paging_reads.clone(),
            s.allocations.len(),
            s.free_calls,
        )
    });
    assert_eq!(cache.diagnostic_bits(error), expected);
    assert_eq!(expected & 0xffff_ffff_0000_ffff, 0);
    with(|s| {
        assert_eq!(
            before,
            (
                s.calls.clone(),
                s.cache_reads.clone(),
                s.paging_reads.clone(),
                s.allocations.len(),
                s.free_calls,
            ),
            "diagnostics must not invoke firmware, recapture, allocate, or release"
        );
    });
}

#[test]
fn all_enabled_aps_complete_but_only_bsp_is_a_probe_processor() {
    let services = setup();
    let report = unsafe { observe(&services) }.unwrap();
    assert_eq!((report.total_processors, report.enabled_processors, report.enabled_aps), (4, 4, 3));
    assert_eq!(report.completed_ap_callbacks, 3);
    assert_eq!(report.probe_processors, 1);
    assert_eq!((report.bsp_number, report.bsp_processor_id), (0, 0x100));
    with(|s| {
        assert_eq!(s.dispatches, 1);
        assert_eq!(s.free_calls, 1);
    });
    clean();
}

#[test]
fn disabled_cpus_are_inventoried_but_never_dispatched() {
    let services = setup();
    with(|s| s.records[2].status_flag = 0);
    let report = unsafe { observe(&services) }.unwrap();
    assert_eq!(
        (report.total_processors, report.enabled_aps, report.completed_ap_callbacks),
        (4, 2, 2)
    );
    clean();
}

#[test]
fn single_enabled_bsp_needs_no_startup_all_aps() {
    let services = setup();
    with(|s| {
        s.records.truncate(1);
    });
    let report = unsafe { observe(&services) }.unwrap();
    assert_eq!((report.enabled_aps, report.completed_ap_callbacks), (0, 0));
    with(|s| assert_eq!(s.dispatches, 0));
    clean();
}

#[test]
fn absent_null_and_failed_allocation_refuse_without_dispatch() {
    for mode in 0..3 {
        let services = setup();
        with(|s| match mode {
            0 => s.protocol_status = Status::NOT_FOUND,
            1 => s.null_protocol = true,
            _ => s.allocation_status = Status::OUT_OF_RESOURCES,
        });
        let expected = match mode {
            0 => CpuError::Protocol(Status::NOT_FOUND),
            1 => CpuError::NullProtocol,
            _ => CpuError::Allocation(Status::OUT_OF_RESOURCES),
        };
        assert_eq!(unsafe { observe(&services) }, Err(expected));
        with(|s| assert_eq!(s.dispatches, 0));
        clean();
    }
}

#[test]
fn count_bounds_refuse_before_allocation() {
    for pair in [(0, 0), (MAX_PROCESSORS + 1, 1), (4, 0), (4, 5), (usize::MAX, 2)] {
        let services = setup();
        with(|s| s.counts = Some(pair));
        assert_eq!(unsafe { observe(&services) }, Err(CpuError::Bounds));
        clean();
    }
}

#[test]
fn inconsistent_bsp_health_and_duplicate_identity_refuse_and_free() {
    for mode in 0..6 {
        let services = setup();
        with(|s| match mode {
            0 => s.identity = 1,
            1 => s.records[1].status_flag |= 1,
            2 => s.records[0].status_flag = 5,
            3 => s.records[1].status_flag = 2,
            4 => s.records[1].processor_id = s.records[0].processor_id,
            _ => s.counts = Some((4, 3)),
        });
        let expected = match mode {
            0..=2 => CpuError::NotBsp,
            3 => CpuError::Unhealthy,
            _ => CpuError::Inventory,
        };
        assert_eq!(unsafe { observe(&services) }, Err(expected));
        with(|s| {
            assert_eq!(s.dispatches, 0);
            assert_eq!(s.free_calls, 1);
        });
        clean();
    }
}

#[test]
fn not_ready_and_terminated_timeout_release_storage() {
    for status in [Status::NOT_READY, Status::TIMEOUT, Status::DEVICE_ERROR] {
        let services = setup();
        with(|s| s.dispatch_status = status);
        assert_eq!(unsafe { observe(&services) }, Err(CpuError::Dispatch(status)));
        with(|s| assert_eq!(s.free_calls, 1));
        clean();
    }
}

#[test]
fn missing_duplicate_and_out_of_range_callbacks_refuse() {
    for mode in 0..3 {
        let services = setup();
        with(|s| match mode {
            0 => s.skip_ap = Some(2),
            1 => s.duplicate_ap = true,
            _ => s.invalid_ap = true,
        });
        assert_eq!(unsafe { observe(&services) }, Err(CpuError::Completion));
        clean();
    }
}

#[test]
fn post_dispatch_inventory_and_bsp_changes_refuse() {
    for identity in [false, true] {
        let services = setup();
        with(|s| {
            if identity {
                s.mutate_bsp_after_dispatch = true
            } else {
                s.mutate_after_dispatch = true
            }
        });
        assert_eq!(unsafe { observe(&services) }, Err(CpuError::Changed));
        clean();
    }
}

#[test]
fn explicit_free_failure_retains_ownership_and_drop_retries() {
    let services = setup();
    let mut prepared = unsafe { prepare(&services) }.unwrap();
    assert_eq!(prepared.report().completed_ap_callbacks, 0);
    with(|s| s.fail_free = 1);
    assert_eq!(prepared.release(), Err(Status::DEVICE_ERROR));
    with(|s| assert_eq!(s.allocations.len(), 1));
    drop(prepared);
    with(|s| assert_eq!(s.free_calls, 2));
    clean();
}

#[test]
fn cleanup_failure_is_reported_even_when_drop_retry_succeeds() {
    let services = setup();
    with(|s| s.fail_free = 1);
    assert_eq!(unsafe { observe(&services) }, Err(CpuError::Cleanup(Status::DEVICE_ERROR)));
    with(|s| assert_eq!(s.free_calls, 2));
    clean();
}

#[test]
fn scoped_interval_blocks_callbacks_and_contains_no_firmware_calls() {
    let services = setup();
    let (report, value) = unsafe {
        with_quiescent_cpu(&services, |guard| {
            with(|s| {
                assert_eq!(s.tpl, Tpl::HIGH_LEVEL);
                assert_eq!(s.dispatch_tpl, Some(Tpl::NOTIFY));
            });
            assert_eq!(guard.report().probe_processors, 1);
            42
        })
    }
    .unwrap();
    assert_eq!(value, 42);
    assert_eq!(report.completed_ap_callbacks, 3);
    clean();
}

#[test]
fn scoped_refusal_never_calls_operation_and_restores_tpl() {
    let services = setup();
    with(|s| s.dispatch_status = Status::NOT_READY);
    assert_eq!(
        unsafe { with_quiescent_cpu::<()>(&services, |_| panic!("refused interval executed")) },
        Err(CpuError::Dispatch(Status::NOT_READY))
    );
    clean();
}

#[test]
fn elevated_entry_tpl_is_refused_and_preserved_without_protocol_access() {
    let services = setup();
    with(|s| s.tpl = Tpl::NOTIFY);
    assert_eq!(unsafe { observe(&services) }, Err(CpuError::EntryTpl));
    with(|s| {
        assert_eq!(s.tpl, Tpl::NOTIFY);
        assert_eq!(s.calls, ["raise", "restore"]);
        s.tpl = Tpl::APPLICATION;
    });
    clean();
}

#[test]
fn released_preparation_cannot_dispatch_again() {
    let services = setup();
    let mut prepared = unsafe { prepare(&services) }.unwrap();
    prepared.release().unwrap();
    assert_eq!(prepared.storage_range(), Err(CpuError::Released));
    assert_eq!(unsafe { prepared.rendezvous() }, Err(CpuError::Released));
    drop(prepared);
    clean();
}

#[test]
fn changed_cpu_after_scoped_operation_refuses_and_restores_tpl() {
    let services = setup();
    let result = unsafe {
        with_quiescent_cpu(&services, |_| {
            with(|s| s.identity = 1);
        })
    };
    assert_eq!(result, Err(CpuError::Changed));
    clean();
}

#[test]
fn changed_tpl_between_prepare_and_scope_refuses_before_dispatch() {
    let services = setup();
    let mut prepared = unsafe { prepare(&services) }.unwrap();
    with(|s| s.tpl = Tpl::NOTIFY);
    assert_eq!(
        unsafe { prepared.with_quiescent_bsp::<()>(|_| panic!("invalid TPL scope executed")) },
        Err(CpuError::EntryTpl)
    );
    with(|s| {
        assert_eq!(s.tpl, Tpl::NOTIFY);
        assert_eq!(s.dispatches, 0);
        s.tpl = Tpl::APPLICATION;
    });
    prepared.release().unwrap();
    clean();
}

#[test]
fn prepared_resources_are_created_and_freed_inside_callback_exclusion() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || ScopedPool::prepare(&services),
            |guard, resource| {
                with(|s| {
                    assert_eq!(s.tpl, Tpl::HIGH_LEVEL);
                    assert_eq!(s.dispatch_tpl, Some(Tpl::NOTIFY));
                    s.calls.push("prepared_operation");
                });
                assert_eq!(guard.report().completed_ap_callbacks, 3);
                assert!(resource.pool.is_some());
                73
            },
            ScopedPool::finish,
        )
    }
    .unwrap();
    assert_eq!(completion.outcome.unwrap().1, 73);
    assert_eq!(completion.cleanup, Ok(()));
    with(|s| {
        assert_eq!(s.tpl, Tpl::APPLICATION);
        assert_eq!(s.allocations.len(), 1); // CPU preparation alone remains.
        let index = |name| s.calls.iter().position(|call| *call == name).unwrap();
        let final_dispatch = s.calls.iter().rposition(|call| *call == "dispatch").unwrap();
        assert_eq!(s.dispatches, 2);
        assert!(index("dispatch") < index("prepare_resource"));
        assert!(index("prepare_resource") < final_dispatch);
        assert!(final_dispatch < index("prepared_operation"));
        assert!(index("prepared_operation") < index("finish_resource"));
        assert!(index("finish_resource") < index("drop_resource"));
        assert_eq!(s.calls.last(), Some(&"restore"));
    });
    cpus.release().unwrap();
    clean();
}

#[test]
fn every_cpu_refusal_after_preparation_finishes_and_drops_at_notify() {
    for mode in 0..3 {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp(
                || {
                    let resource = ScopedPool::prepare(&services)?;
                    with(|s| match mode {
                        0 => s.dispatch_status = Status::NOT_READY,
                        1 => s.dispatch_status = Status::TIMEOUT,
                        _ => s.corrupt_high_previous = true,
                    });
                    Ok::<_, Status>(resource)
                },
                |_, _| -> () { panic!("refused prepared operation executed") },
                ScopedPool::finish,
            )
        }
        .unwrap();
        let expected = match mode {
            0 => CpuError::Dispatch(Status::NOT_READY),
            1 => CpuError::Dispatch(Status::TIMEOUT),
            _ => CpuError::EntryTpl,
        };
        assert_eq!(completion.outcome, Err(expected));
        assert_eq!(completion.cleanup, Ok(()));
        with(|s| {
            assert_eq!(s.tpl, Tpl::APPLICATION);
            assert_eq!(s.allocations.len(), 1);
            assert!(s.calls.contains(&"finish_resource"));
            assert!(s.calls.contains(&"drop_resource"));
        });
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn preparation_refusal_drops_partial_resources_without_final_barrier_or_operation() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let result = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || {
                let _partial = ScopedPool::prepare(&services)?;
                Err::<ScopedPool<'_>, _>(Status::ACCESS_DENIED)
            },
            |_, _| -> () { panic!("failed preparation executed operation") },
            |_| -> () { panic!("failed preparation executed finish") },
        )
    };
    assert_eq!(result, Err(PreparedScopeError::Preparation(Status::ACCESS_DENIED)));
    with(|s| {
        assert_eq!(s.dispatches, 1); // Initial idle check alone completed.
        assert_eq!(s.allocations.len(), 1);
        assert!(s.calls.contains(&"drop_resource"));
        assert!(!s.calls.contains(&"finish_resource"));
    });
    cpus.release().unwrap();
    clean();
}

#[test]
fn prepared_cleanup_error_survives_successful_drop_retry() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || ScopedPool::prepare(&services),
            |_, _| {
                with(|s| s.fail_free = 1);
                9
            },
            ScopedPool::finish,
        )
    }
    .unwrap();
    assert_eq!(completion.outcome.unwrap().1, 9);
    assert_eq!(completion.cleanup, Err(Status::DEVICE_ERROR));
    with(|s| {
        assert_eq!(s.free_calls, 2);
        assert_eq!(s.allocations.len(), 1);
    });
    cpus.release().unwrap();
    clean();
}

#[test]
fn post_operation_cpu_error_and_cleanup_error_are_both_preserved() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || ScopedPool::prepare(&services),
            |_, _| {
                with(|s| {
                    s.identity = 1;
                    s.fail_free = 1;
                });
            },
            ScopedPool::finish,
        )
    }
    .unwrap();
    assert_eq!(completion.outcome, Err(CpuError::Changed));
    assert_eq!(completion.cleanup, Err(Status::DEVICE_ERROR));
    with(|s| {
        assert_eq!(s.free_calls, 2);
        assert_eq!(s.allocations.len(), 1);
    });
    cpus.release().unwrap();
    clean();
}

#[test]
fn already_busy_aps_refuse_before_any_preparation_dereference() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    with(|s| s.dispatch_status = Status::NOT_READY);
    let result = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || -> Result<ScopedPool<'_>, Status> { panic!("busy APs reached preparation") },
            |_, _| -> () { panic!("busy APs reached operation") },
            |_| -> () { panic!("busy APs reached finish") },
        )
    };
    assert_eq!(result, Err(PreparedScopeError::Cpu(CpuError::Dispatch(Status::NOT_READY))));
    with(|s| {
        assert_eq!(s.dispatches, 1);
        assert_eq!(s.dispatch_tpl, Some(Tpl::NOTIFY));
        assert_eq!(s.allocations.len(), 1);
        assert_eq!(s.tpl, Tpl::APPLICATION);
    });
    cpus.release().unwrap();
    clean();
}

#[test]
fn cache_capture_occurs_only_in_final_callbacks_and_high_bsp_with_owned_storage() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    assert_eq!(cpus.storage_range().unwrap().1, 4 * 32);
    let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
    let storage = cache.storage_range().unwrap();
    assert_eq!(storage.1, 4 * 416);
    assert_eq!(
        cache.bsp_cr3(),
        Err(cache_rendezvous::RendezvousError::Incomplete { processor: 0 })
    );
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp_and_ap_observation(
            || {
                with(|s| assert!(s.cache_reads.is_empty())); // Initial barrier is identity only.
                Ok::<_, ()>(cache)
            },
            |cache| cache,
            |guard, cache| {
                let calls = with(|s| s.calls.clone());
                let report = cache.capture_bsp_and_compare(guard).unwrap();
                assert_eq!(report.enabled_processors, 4);
                assert_eq!(report.completed_ap_captures, 3);
                assert_eq!(report.rendezvous, guard.rendezvous());
                assert_eq!(cache.bsp_snapshot().unwrap(), &cache_snapshot(0));
                assert_eq!(cache.bsp_cr3(), Ok(MOCK_CR3));
                assert_eq!(cache.capture_bsp_and_compare(guard).unwrap(), report);
                with(|s| {
                    assert_eq!(s.calls, calls); // No Boot/MP services at HIGH.
                    assert_eq!(s.cache_reads, [1, 2, 3, 0, 0]);
                    assert_eq!(s.paging_reads, [1, 2, 3, 0, 0]);
                });
                report
            },
            |cache| {
                with(|s| {
                    assert_eq!(s.tpl, Tpl::NOTIFY);
                    assert_eq!(s.allocations.len(), 2);
                });
                let result = cache.release();
                assert_eq!(cache.storage_range(), Err(cache_rendezvous::RendezvousError::Released));
                assert_eq!(cache.bsp_cr3(), Err(cache_rendezvous::RendezvousError::Released));
                result
            },
        )
    }
    .unwrap();
    assert!(completion.outcome.is_ok());
    assert_eq!(completion.cleanup, Ok(()));
    cpus.release().unwrap();
    clean();
}

#[test]
fn cache_capture_omits_disabled_cpus_and_handles_one_enabled_bsp() {
    for single in [false, true] {
        let services = setup();
        with(|s| {
            if single {
                s.records.truncate(1);
            } else {
                s.records[2].status_flag = 0;
            }
        });
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| cache.capture_bsp_and_compare(guard),
                |cache| cache.release(),
            )
        }
        .unwrap();
        assert_eq!(
            completion.outcome.unwrap().1.unwrap().enabled_processors,
            if single { 1 } else { 3 }
        );
        with(|s| {
            assert_eq!(s.cache_reads, if single { vec![0] } else { vec![1, 3, 0] });
            assert_eq!(s.paging_reads, s.cache_reads);
        });
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn cache_refusal_and_coerced_success_never_pass_comparison() {
    use cache::CaptureError;
    use cache_rendezvous::{ConfigurationField as Field, RendezvousError as Error};
    for mode in 0..7 {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        with(|s| match mode {
            0 => {
                s.cache_status.insert(2, 2);
            }
            1 => {
                s.cache_status.insert(2, 3);
            }
            2 => {
                s.cache_changes.insert(2, |s| s.cr4 ^= 1 << 18);
            }
            3 => {
                s.cache_changes.insert(2, |s| s.pat ^= 1);
            }
            4 => {
                s.cache_changes.insert(2, |s| s.rflags |= 1 << 9);
            }
            5 => {
                s.cache_changes.insert(2, |s| s.msr_reads -= 1);
            }
            _ => {
                s.cache_changes.insert(2, |s| s.sys_cfg ^= 1 << 23);
            }
        });
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| cache.capture_bsp_and_compare(guard),
                |cache| cache.release(),
            )
        }
        .unwrap();
        let expected = match mode {
            0 | 4 => Error::Capture { processor: 2, error: CaptureError::PrivilegeOrFlags },
            1 => Error::Capture { processor: 2, error: CaptureError::UnsupportedCpu },
            2 => Error::Mismatch { processor: 2, field: Field::Cr4 },
            3 => Error::Mismatch { processor: 2, field: Field::Pat },
            5 => Error::CaptureShape { processor: 2 },
            _ => Error::Mismatch { processor: 2, field: Field::SysCfg },
        };
        assert_eq!(completion.outcome.unwrap().1, Err(expected));
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn actual_ap_cr3_requires_exact_root_and_pwt_pcd_agreement() {
    use cache_rendezvous::{ConfigurationField, RendezvousError};
    // Check every architecturally retained CR3 bit individually. Equivalent
    // translations from a different root are intentionally not considered.
    for bit in [3, 4].into_iter().chain(12..48) {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        // Only the host stand-in receives this per-test supplied mutation.
        with(|s| {
            s.paging_changes.insert(2, |words| {
                words[6] ^= 1 << AP_ROOT_BIT.get();
            });
        });
        AP_ROOT_BIT.set(bit);
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| {
                    let result = cache.capture_bsp_and_compare(guard);
                    assert_eq!(cache.bsp_cr3(), Ok(MOCK_CR3));
                    result
                },
                |cache| cache.release(),
            )
        }
        .unwrap();
        assert_eq!(
            completion.outcome.unwrap().1,
            Err(RendezvousError::Mismatch { processor: 2, field: ConfigurationField::Cr3 }),
            "CR3 bit {bit}"
        );
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn zero_and_reserved_cr3_bits_refuse_on_either_bsp_or_ap() {
    use cache_rendezvous::{PagingRootError, RendezvousError};
    for processor in [0, 2] {
        for root in [0, 0x18]
            .into_iter()
            .chain((0..3).chain(5..12).chain(48..64).map(|bit| MOCK_CR3 | (1u64 << bit)))
        {
            let services = setup();
            let mut cpus = unsafe { prepare(&services) }.unwrap();
            let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
            BAD_ROOT.set(root);
            with(|s| {
                s.paging_changes.insert(processor, |words| words[6] = BAD_ROOT.get());
            });
            let completion = unsafe {
                cpus.with_prepared_quiescent_bsp_and_ap_observation(
                    || Ok::<_, ()>(cache),
                    |cache| cache,
                    |guard, cache| cache.capture_bsp_and_compare(guard),
                    |cache| cache.release(),
                )
            }
            .unwrap();
            assert_eq!(
                completion.outcome.unwrap().1,
                Err(RendezvousError::PagingRoot { processor, error: PagingRootError::InvalidCr3 }),
                "CPU {processor} root {root:#x}"
            );
            assert_eq!(completion.cleanup, Ok(()));
            cpus.release().unwrap();
            clean();
        }
    }
}

#[test]
fn paging_helper_failures_and_inconsistent_success_refuse() {
    use cache_rendezvous::{PagingRootError as Paging, RendezvousError};
    for processor in [0, 2] {
        for mode in 0..12 {
            let services = setup();
            let mut cpus = unsafe { prepare(&services) }.unwrap();
            let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
            with(|s| match mode {
                0 => {
                    s.paging_status.insert(processor, 1);
                }
                1 => {
                    s.paging_status.insert(processor, 77);
                }
                2 => {
                    s.paging_changes.insert(processor, |words| words[4] |= 3);
                }
                3 => {
                    s.paging_changes.insert(processor, |words| words[5] ^= 1 << 16);
                }
                4 => {
                    s.paging_changes.insert(processor, |words| words[7] ^= 1 << 18);
                }
                5..=9 => {
                    AP_ROOT_BIT.set([8, 9, 10, 14, 18][mode - 5]);
                    s.paging_changes.insert(processor, |words| words[8] |= 1 << AP_ROOT_BIT.get());
                }
                10 => {
                    s.paging_changes.insert(processor, |words| words[8] ^= 1 << 12);
                }
                _ => {
                    s.paging_changes.insert(processor, |words| words[8] ^= 0x8d5);
                }
            });
            let completion = unsafe {
                cpus.with_prepared_quiescent_bsp_and_ap_observation(
                    || Ok::<_, ()>(cache),
                    |cache| cache,
                    |guard, cache| cache.capture_bsp_and_compare(guard),
                    |cache| cache.release(),
                )
            }
            .unwrap();
            let result = completion.outcome.unwrap().1;
            if mode == 11 {
                assert!(result.is_ok()); // Only ABI-volatile arithmetic flags changed.
            } else {
                let error = match mode {
                    0 | 2 => Paging::PrivilegeLevel,
                    1 => Paging::UnexpectedStatus,
                    3 => Paging::InconsistentCr0,
                    4 => Paging::InconsistentCr4,
                    5..=9 => Paging::UnsupportedFlags,
                    _ => Paging::InconsistentFlags,
                };
                assert_eq!(result, Err(RendezvousError::PagingRoot { processor, error }));
            }
            assert_eq!(completion.cleanup, Ok(()));
            cpus.release().unwrap();
            clean();
        }
    }
}

#[test]
fn paging_root_requires_the_restricted_native_mode() {
    use cache_rendezvous::{PagingRootError, RendezvousError};
    for mode in 0..8 {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let change: fn(&mut cache::CacheSnapshot) = match mode {
            0 => |cache| cache.cr0 &= !(1 << 31),
            1 => |cache| cache.cr0 &= !1,
            2 => |cache| cache.cr4 &= !(1 << 5),
            3 => |cache| cache.cr4 |= 1 << 12,
            4 => |cache| cache.cr4 |= 1 << 17,
            5 => |cache| cache.efer &= !(1 << 8),
            6 => |cache| cache.efer &= !(1 << 10),
            _ => |cache| cache.physical_bits = 47,
        };
        with(|s| {
            for processor in 0..4 {
                s.cache_changes.insert(processor, change);
            }
        });
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| cache.capture_bsp_and_compare(guard),
                |cache| cache.release(),
            )
        }
        .unwrap();
        assert_eq!(
            completion.outcome.unwrap().1,
            Err(RendezvousError::PagingRoot {
                processor: 0,
                error: PagingRootError::UnsupportedPagingMode,
            })
        );
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn refused_cache_reader_never_reaches_the_cr3_helper() {
    use cache_rendezvous::RendezvousError;
    for processor in [0, 2] {
        for status in [1, 2, 3, 4, 5, 6, 7, u32::MAX] {
            let services = setup();
            let mut cpus = unsafe { prepare(&services) }.unwrap();
            let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
            with(|s| {
                s.cache_status.insert(processor, status);
            });
            let completion = unsafe {
                cpus.with_prepared_quiescent_bsp_and_ap_observation(
                    || Ok::<_, ()>(cache),
                    |cache| cache,
                    |guard, cache| cache.capture_bsp_and_compare(guard),
                    |cache| cache.release(),
                )
            }
            .unwrap();
            assert!(matches!(
                completion.outcome.unwrap().1,
                Err(RendezvousError::Capture { processor: failed, .. }) if failed == processor
            ));
            with(|s| {
                assert!(s.cache_reads.contains(&processor));
                assert!(!s.paging_reads.contains(&processor));
                assert_eq!(s.paging_reads.len(), 3);
            });
            cpus.release().unwrap();
            clean();
        }
    }
}

#[test]
fn bsp_recapture_replaces_or_refuses_the_actual_root_without_stale_fallback() {
    use cache_rendezvous::{ConfigurationField, PagingRootError, RendezvousError};
    for mode in 0..3 {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| {
                    assert!(cache.capture_bsp_and_compare(guard).is_ok());
                    assert_eq!(cache.bsp_cr3(), Ok(MOCK_CR3));
                    with(|s| match mode {
                        0 => {
                            s.cache_status.insert(0, 2);
                        }
                        1 => {
                            s.paging_status.insert(0, 1);
                        }
                        _ => {
                            s.paging_changes.insert(0, |words| words[6] ^= 1 << 12);
                        }
                    });
                    let result = cache.capture_bsp_and_compare(guard);
                    let expected = match mode {
                        0 => RendezvousError::Capture {
                            processor: 0,
                            error: cache::CaptureError::PrivilegeOrFlags,
                        },
                        1 => RendezvousError::PagingRoot {
                            processor: 0,
                            error: PagingRootError::PrivilegeLevel,
                        },
                        _ => RendezvousError::Mismatch {
                            processor: 1,
                            field: ConfigurationField::Cr3,
                        },
                    };
                    assert_eq!(result, Err(expected));
                    assert_eq!(
                        cache.bsp_cr3(),
                        if mode == 2 { Ok(MOCK_CR3 ^ (1 << 12)) } else { Err(expected) }
                    );
                    with(|s| {
                        assert_eq!(s.cache_reads, [1, 2, 3, 0, 0]);
                        assert_eq!(s.paging_reads.len(), if mode == 0 { 4 } else { 5 });
                    });
                },
                |cache| cache.release(),
            )
        }
        .unwrap();
        assert!(completion.outcome.is_ok());
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn final_cache_dispatch_failures_free_only_after_return_and_keep_cleanup_failure() {
    for mode in 0..4 {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || {
                    with(|s| {
                        match mode {
                            0 => s.dispatch_status = Status::NOT_READY,
                            1 => {
                                s.dispatch_status = Status::TIMEOUT;
                                s.skip_ap = Some(3);
                            }
                            2 => s.duplicate_ap = true,
                            _ => s.skip_ap = Some(2),
                        }
                        s.fail_free = 1;
                    });
                    Ok::<_, ()>(cache)
                },
                |cache| cache,
                |_, _| -> () { panic!("failed final callback set reached operation") },
                |cache| {
                    with(|s| {
                        assert_eq!(s.tpl, Tpl::NOTIFY);
                        assert_eq!(s.free_calls, 0);
                        assert!(!s.cache_reads.contains(&0));
                        if mode == 1 {
                            assert_eq!(s.cache_reads, [1, 2]);
                        }
                        if mode == 2 {
                            assert_eq!(s.cache_reads, [1, 2, 3]);
                        }
                    });
                    cache.release()
                },
            )
        }
        .unwrap();
        let expected = match mode {
            0 => CpuError::Dispatch(Status::NOT_READY),
            1 => CpuError::Dispatch(Status::TIMEOUT),
            _ => CpuError::Completion,
        };
        assert_eq!(completion.outcome, Err(expected));
        assert_eq!(completion.cleanup, Err(Status::DEVICE_ERROR));
        with(|s| {
            assert_eq!(s.free_calls, 2);
            assert_eq!(s.allocations.len(), 1);
        });
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn concurrent_ap_slots_publish_before_bsp_and_duplicate_claims_cannot_write_twice() {
    for duplicate in [false, true] {
        let services = setup();
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || {
                    with(|s| {
                        s.concurrent_dispatch = true;
                        s.duplicate_ap = duplicate;
                    });
                    Ok::<_, ()>(cache)
                },
                |cache| cache,
                |guard, cache| cache.capture_bsp_and_compare(guard),
                |cache| cache.release(),
            )
        }
        .unwrap();
        if duplicate {
            assert_eq!(completion.outcome, Err(CpuError::Completion));
        } else {
            assert_eq!(completion.outcome.unwrap().1.unwrap().completed_ap_captures, 3);
        }
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn configuration_comparison_excludes_exactly_identity_role_and_arithmetic_bits() {
    let bsp = cache_snapshot(0);
    for byte in 0..cache::SNAPSHOT_BYTES {
        for bit in 0..8 {
            let mut ap = bsp;
            unsafe {
                *(&mut ap as *mut cache::CacheSnapshot).cast::<u8>().add(byte) ^= 1 << bit;
            }
            // Independent whitelist in the fixed 352-byte capture ABI.
            let allowed = (72..76).contains(&byte)
                || (byte == 80 && (0xd5u8 & (1 << bit) != 0))
                || (byte == 81 && bit == 3)
                || (byte == 177 && bit == 0);
            assert_eq!(
                cache_rendezvous::compare_configuration(&bsp, &ap).is_ok(),
                allowed,
                "byte {byte} bit {bit}"
            );
        }
    }
}

#[test]
fn ap_configuration_adds_exactly_cr4_de_to_the_existing_abi_exclusions() {
    let bsp = cache_snapshot(0);
    for byte in 0..cache::SNAPSHOT_BYTES {
        for bit in 0..8 {
            let mut ap = bsp;
            unsafe {
                *(&mut ap as *mut cache::CacheSnapshot).cast::<u8>().add(byte) ^= 1 << bit;
            }
            // Independently enumerate exclusions using the fixed capture ABI.
            let allowed = (72..76).contains(&byte)
                || (byte == 80 && (0xd5u8 & (1 << bit) != 0))
                || (byte == 81 && bit == 3)
                || (byte == 177 && bit == 0)
                || (byte == 96 && bit == 3);
            assert_eq!(
                cache_rendezvous::compare_ap_configuration(&bsp, &ap).is_ok(),
                allowed,
                "byte {byte} bit {bit}"
            );
        }
    }
}

#[test]
fn cr4_de_is_ap_only_and_never_masks_another_cr4_difference_or_changes_order() {
    use cache_rendezvous::{
        ConfigurationField as Field, compare_ap_configuration, compare_configuration,
    };
    let before = cache_snapshot(0);
    let mut after = before;
    after.cr4 ^= 8;
    assert_eq!(compare_configuration(&before, &after), Err(Field::Cr4));
    assert_eq!(compare_configuration(&after, &before), Err(Field::Cr4));
    assert_eq!(compare_ap_configuration(&before, &after), Ok(()));
    for bit in (0..64).filter(|bit| *bit != 3) {
        let mut ap = after;
        ap.cr4 ^= 1u64 << bit;
        assert_eq!(compare_ap_configuration(&before, &ap), Err(Field::Cr4));
        assert_eq!(compare_ap_configuration(&ap, &before), Err(Field::Cr4));
    }
    // CR0 still precedes CR4, which still precedes EFER in both comparisons.
    after.cr0 ^= 1;
    after.efer ^= 1;
    assert_eq!(compare_configuration(&before, &after), Err(Field::Cr0));
    assert_eq!(compare_ap_configuration(&before, &after), Err(Field::Cr0));
    after.cr0 = before.cr0;
    assert_eq!(compare_configuration(&before, &after), Err(Field::Cr4));
    assert_eq!(compare_ap_configuration(&before, &after), Err(Field::Efer));
    after.cr4 ^= 1 << 18;
    assert_eq!(compare_ap_configuration(&before, &after), Err(Field::Cr4));
}

#[test]
fn actual_de_only_ap_difference_keeps_raw_observations_counts_and_cleanup() {
    for processor in [0, 2] {
        let services = setup();
        with(|s| {
            s.cache_changes.insert(processor, |cache| cache.cr4 ^= 8);
        });
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| {
                    let calls = with(|s| s.calls.clone());
                    let report = cache.capture_bsp_and_compare(guard).unwrap();
                    assert_eq!(report.enabled_processors, 4);
                    assert_eq!(report.completed_ap_captures, 3);
                    assert_eq!(report.rendezvous, guard.rendezvous());
                    let mut expected_bsp = cache_snapshot(0);
                    if processor == 0 {
                        expected_bsp.cr4 ^= 8;
                    }
                    assert_eq!(cache.bsp_snapshot().unwrap(), &expected_bsp);
                    assert_eq!(cache.bsp_cr3(), Ok(MOCK_CR3));
                    assert_eq!(cache.capture_bsp_and_compare(guard).unwrap(), report);
                    with(|s| {
                        assert_eq!(s.calls, calls);
                        assert_eq!(s.cache_reads, [1, 2, 3, 0, 0]);
                        assert_eq!(s.paging_reads, s.cache_reads);
                    });
                },
                |cache| cache.release(),
            )
        }
        .unwrap();
        assert!(completion.outcome.is_ok());
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn de_differences_do_not_relax_same_cpu_cr4_raw_cr3_cache_or_reader_guards() {
    use cache_rendezvous::{
        ConfigurationField as Field, PagingRootError as Paging, RendezvousError as Error,
    };
    for processor in [0, 2] {
        for mode in 0..5 {
            let services = setup();
            with(|s| {
                s.cache_changes.insert(2, |cache| cache.cr4 ^= 8);
                match mode {
                    0 => {
                        s.paging_changes.insert(processor, |words| words[7] ^= 8);
                    }
                    1 => {
                        s.paging_changes.insert(processor, |words| words[6] ^= 8);
                    }
                    2 => {
                        s.cache_changes.insert(processor, |cache| {
                            cache.cr4 ^= 8;
                            cache.pat ^= 1;
                        });
                    }
                    3 => {
                        s.cache_changes.insert(processor, |cache| {
                            cache.cr4 ^= 8;
                            cache.variable[0].base ^= 1;
                        });
                    }
                    _ => {
                        s.cache_changes.insert(processor, |cache| {
                            cache.cr4 ^= 8;
                            cache.rflags |= 1 << 9;
                        });
                    }
                }
            });
            let mut cpus = unsafe { prepare(&services) }.unwrap();
            let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
            let completion = unsafe {
                cpus.with_prepared_quiescent_bsp_and_ap_observation(
                    || Ok::<_, ()>(cache),
                    |cache| cache,
                    |guard, cache| cache.capture_bsp_and_compare(guard),
                    |cache| cache.release(),
                )
            }
            .unwrap();
            let mismatch_processor = if processor == 0 { 1 } else { processor };
            let expected = match mode {
                0 => Error::PagingRoot { processor, error: Paging::InconsistentCr4 },
                1 => Error::Mismatch { processor: mismatch_processor, field: Field::Cr3 },
                2 => Error::Mismatch { processor: mismatch_processor, field: Field::Pat },
                3 => Error::Mismatch { processor: mismatch_processor, field: Field::VariableMtrrs },
                _ => Error::Capture { processor, error: cache::CaptureError::PrivilegeOrFlags },
            };
            assert_eq!(completion.outcome.unwrap().1, Err(expected));
            assert_eq!(completion.cleanup, Ok(()));
            cpus.release().unwrap();
            clean();
        }
    }
}

#[test]
fn cache_slot_is_one_shot_even_if_an_observer_forwards_the_callback_twice() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp_and_ap_observation(
            || Ok::<_, ()>(RepeatedObserver(cache)),
            |observer| observer,
            |guard, observer| observer.0.capture_bsp_and_compare(guard),
            |observer| observer.0.release(),
        )
    }
    .unwrap();
    assert_eq!(
        completion.outcome.unwrap().1,
        Err(cache_rendezvous::RendezvousError::ReusedOrInvalidCallback)
    );
    with(|s| assert_eq!(s.cache_reads, [1, 2, 3]));
    cpus.release().unwrap();
    clean();
}

#[test]
fn stale_ap_round_refuses_even_with_identical_successful_registers() {
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp_and_ap_observation(
            || Ok::<_, ()>(cache),
            |cache| cache,
            |guard, cache| {
                assert!(cache.capture_bsp_and_compare(guard).is_ok());
                // Deliberately corrupt only AP 2's stored round word. This is
                // test fault injection into exclusive owned storage after all
                // callbacks finished, with no live snapshot reference.
                let (base, _) = cache.storage_range().unwrap();
                (base as *mut u8).add(2 * 416 + 32).cast::<usize>().write(guard.rendezvous() - 1);
                cache.capture_bsp_and_compare(guard)
            },
            |cache| cache.release(),
        )
    }
    .unwrap();
    assert_eq!(
        completion.outcome.unwrap().1,
        Err(cache_rendezvous::RendezvousError::Stale { processor: 2 })
    );
    cpus.release().unwrap();
    clean();
}

#[test]
fn cache_allocation_failure_and_released_inventory_do_not_dispatch_or_leak() {
    use cache_rendezvous::RendezvousError;
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    with(|s| s.allocation_status = Status::OUT_OF_RESOURCES);
    assert!(matches!(
        unsafe { cache_rendezvous::prepare(&services, &cpus) },
        Err(RendezvousError::Allocation(Status::OUT_OF_RESOURCES))
    ));
    with(|s| {
        assert_eq!(s.allocations.len(), 1);
        assert_eq!(s.dispatches, 0);
    });
    cpus.release().unwrap();
    assert!(matches!(
        unsafe { cache_rendezvous::prepare(&services, &cpus) },
        Err(RendezvousError::Cpu(CpuError::Released))
    ));
    clean();
}

#[test]
fn cache_diagnostic_codes_are_stable_and_never_truncate_processor_numbers() {
    use cache::CaptureError as Capture;
    use cache_rendezvous::{
        ConfigurationField as Field, PagingRootError as Paging, RendezvousError as Error,
    };
    let services = setup();
    let mut cpus = unsafe { prepare(&services) }.unwrap();
    let mut cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
    for (error, detail) in [
        (Error::Cpu(CpuError::Released), 0x01),
        (Error::Allocation(Status::OUT_OF_RESOURCES), 0x02),
        (Error::Layout, 0x03),
        (Error::Released, 0x04),
        (Error::Bounds, 0x05),
        (Error::Inventory, 0x06),
        (Error::ReusedOrInvalidCallback, 0x07),
        (Error::Cleanup(Status::DEVICE_ERROR), 0x08),
    ] {
        assert_cache_diagnostic(&cache, error, detail << 16);
    }
    for processor in [0, 255, 256, usize::MAX] {
        let expected = |detail: u64| {
            if processor <= 255 { ((processor as u64) << 24) | (detail << 16) } else { 0x007f_0000 }
        };
        for (error, detail) in [
            (Capture::OutputAddress, 0x10),
            (Capture::PrivilegeOrFlags, 0x11),
            (Capture::UnsupportedCpu, 0x12),
            (Capture::UnsupportedFeatures, 0x13),
            (Capture::AddressEncryptionActive, 0x14),
            (Capture::UnsupportedMtrrCount, 0x15),
            (Capture::UnexpectedStatus, 0x16),
        ] {
            assert_cache_diagnostic(&cache, Error::Capture { processor, error }, expected(detail));
        }
        for (error, detail) in [
            (Error::CaptureShape { processor }, 0x20),
            (Error::Incomplete { processor }, 0x21),
            (Error::Stale { processor }, 0x22),
        ] {
            assert_cache_diagnostic(&cache, error, expected(detail));
        }
        for (error, detail) in [
            (Paging::NotCaptured, 0x30),
            (Paging::PrivilegeLevel, 0x31),
            (Paging::UnexpectedStatus, 0x32),
            (Paging::InconsistentCr0, 0x33),
            (Paging::InconsistentCr4, 0x34),
            (Paging::UnsupportedFlags, 0x35),
            (Paging::InconsistentFlags, 0x36),
            (Paging::UnsupportedPagingMode, 0x37),
            (Paging::InvalidCr3, 0x38),
        ] {
            assert_cache_diagnostic(
                &cache,
                Error::PagingRoot { processor, error },
                expected(detail),
            );
        }
        for (field, detail) in [
            (Field::AbiVersion, 0x40),
            (Field::CapturedFields, 0x41),
            (Field::Refusal, 0x42),
            (Field::MsrReads, 0x43),
            (Field::Signature, 0x44),
            (Field::MaximumBasicLeaf, 0x45),
            (Field::MaximumExtendedLeaf, 0x46),
            (Field::Leaf1Ecx, 0x47),
            (Field::Leaf1Edx, 0x48),
            (Field::PhysicalBits, 0x49),
            (Field::EncryptionEax, 0x4a),
            (Field::EncryptionEbx, 0x4b),
            (Field::MultiKeyEax, 0x4c),
            (Field::MultiKeyEbx, 0x4d),
            (Field::Reserved, 0x4e),
            (Field::Rflags, 0x4f),
            (Field::Cr0, 0x50),
            (Field::Cr3, 0x51),
            (Field::Cr4, 0x52),
            (Field::Efer, 0x53),
            (Field::SysCfg, 0x54),
            (Field::SevStatus, 0x55),
            (Field::Pat, 0x56),
            (Field::MtrrCap, 0x57),
            (Field::MtrrDefault, 0x58),
            (Field::TopMem, 0x59),
            (Field::SmmAddress, 0x5a),
            (Field::SmmMask, 0x5b),
            (Field::ApicBase, 0x5c),
            (Field::MmioConfig, 0x5d),
            (Field::Iorr, 0x5e),
            (Field::VariableMtrrs, 0x5f),
        ] {
            assert_cache_diagnostic(&cache, Error::Mismatch { processor, field }, expected(detail));
        }
    }
    cache.release().unwrap();
    assert_cache_diagnostic(
        &cache,
        Error::Capture { processor: 0, error: Capture::PrivilegeOrFlags },
        0x0011_0000,
    );
    cpus.release().unwrap();
    clean();
}

#[test]
fn actual_refused_captures_report_each_observed_flag_without_recapture_or_mutation() {
    use cache::CaptureError;
    use cache_rendezvous::RendezvousError;
    // BSP and AP failures, including the highest accepted processor number.
    for processor in [0, 2, 255] {
        for mask in 0u64..32 {
            let services = setup();
            with(|s| {
                if processor == 255 {
                    s.records = (0..256)
                        .map(|n| ProcessorInformation {
                            processor_id: 0x100 + n,
                            status_flag: if n == 0 { 7 } else { 6 },
                            core: n as u32,
                            ..ProcessorInformation::default()
                        })
                        .collect();
                }
                s.cache_status.insert(processor, 2);
                // Arithmetic flags and unrelated IOPL bits must not enter the
                // compact TF/IF/DF/NT/AC mask.
                let mut flags = 0x3000 | 0x8d5;
                for (index, bit) in [8, 9, 10, 14, 18].into_iter().enumerate() {
                    if mask & (1 << index) != 0 {
                        flags |= 1 << bit;
                    }
                }
                DIAGNOSTIC_FLAGS.set(flags);
                s.cache_changes
                    .insert(processor, |snapshot| snapshot.rflags |= DIAGNOSTIC_FLAGS.get());
            });
            let mut cpus = unsafe { prepare(&services) }.unwrap();
            let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
            let completion = unsafe {
                cpus.with_prepared_quiescent_bsp_and_ap_observation(
                    || Ok::<_, ()>(cache),
                    |cache| cache,
                    |guard, cache| {
                        let error = cache.capture_bsp_and_compare(guard).unwrap_err();
                        assert_eq!(
                            error,
                            RendezvousError::Capture {
                                processor,
                                error: CaptureError::PrivilegeOrFlags
                            }
                        );
                        let bits = ((processor as u64) << 24) | ((0x80 | mask) << 16);
                        assert_cache_diagnostic(cache, error, bits);
                        assert_eq!(0x4000 | 11 | cache.diagnostic_bits(error), bits | 0x400b);
                        // The immutable AP evidence still produces exactly the
                        // same refusal when the BSP is explicitly recaptured.
                        assert_eq!(cache.capture_bsp_and_compare(guard), Err(error));
                        assert_cache_diagnostic(cache, error, bits);
                    },
                    |cache| cache.release(),
                )
            }
            .unwrap();
            assert!(completion.outcome.is_ok());
            assert_eq!(completion.cleanup, Ok(()));
            with(|s| assert!(!s.paging_reads.contains(&processor)));
            cpus.release().unwrap();
            clean();
        }
    }
}

#[test]
fn final_rendezvous_and_post_capture_errors_keep_stage_and_original_error() {
    use cache::CaptureError;
    use cache_rendezvous::{ConfigurationField, PagingRootError, RendezvousError};
    for after in [false, true] {
        for mode in 0..5 {
            let services = setup();
            let mut cpus = unsafe { prepare(&services) }.unwrap();
            let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
            let completion = unsafe {
                cpus.with_prepared_quiescent_bsp_and_ap_observation(
                    || Ok::<_, ()>(cache),
                    |cache| cache,
                    |guard, cache| {
                        if after {
                            assert!(cache.capture_bsp_and_compare(guard).is_ok());
                            // A constructible error cannot manufacture flag
                            // evidence from a successful, flag-clear record.
                            assert_cache_diagnostic(
                                cache,
                                RendezvousError::Capture {
                                    processor: 0,
                                    error: CaptureError::PrivilegeOrFlags,
                                },
                                0x0011_0000,
                            );
                        }
                        with(|s| match mode {
                            0 => {
                                s.cache_status.insert(0, 4);
                            }
                            1 => {
                                s.cache_changes.insert(0, |snapshot| snapshot.rflags |= 1 << 9);
                            }
                            2 => {
                                s.cache_changes.insert(0, |snapshot| snapshot.msr_reads -= 1);
                            }
                            3 => {
                                s.paging_status.insert(0, 1);
                            }
                            _ => {
                                s.paging_changes.insert(0, |words| words[6] ^= 1 << 12);
                            }
                        });
                        let error = cache.capture_bsp_and_compare(guard).unwrap_err();
                        let (expected, bits) = match mode {
                            0 => (
                                RendezvousError::Capture {
                                    processor: 0,
                                    error: CaptureError::UnsupportedFeatures,
                                },
                                0x0013_0000,
                            ),
                            1 => (
                                RendezvousError::Capture {
                                    processor: 0,
                                    error: CaptureError::PrivilegeOrFlags,
                                },
                                0x0082_0000,
                            ),
                            2 => (RendezvousError::CaptureShape { processor: 0 }, 0x0020_0000),
                            3 => (
                                RendezvousError::PagingRoot {
                                    processor: 0,
                                    error: PagingRootError::PrivilegeLevel,
                                },
                                0x0031_0000,
                            ),
                            _ => (
                                RendezvousError::Mismatch {
                                    processor: 1,
                                    field: ConfigurationField::Cr3,
                                },
                                0x0151_0000,
                            ),
                        };
                        assert_eq!(error, expected);
                        assert_cache_diagnostic(cache, error, bits);
                        if mode == 0 {
                            assert_cache_diagnostic(
                                cache,
                                RendezvousError::Capture {
                                    processor: 0,
                                    error: CaptureError::PrivilegeOrFlags,
                                },
                                0x0011_0000,
                            );
                        }
                        let stage = if after { 23 } else { 11 };
                        assert_eq!(
                            0x4000 | stage | cache.diagnostic_bits(error),
                            bits | if after { 0x4017 } else { 0x400b }
                        );
                        if mode <= 2 {
                            let snapshot_error = cache.bsp_snapshot().unwrap_err();
                            assert_eq!(snapshot_error, error);
                            assert_cache_diagnostic(cache, snapshot_error, bits);
                        }
                    },
                    |cache| cache.release(),
                )
            }
            .unwrap();
            assert!(completion.outcome.is_ok());
            assert_eq!(completion.cleanup, Ok(()));
            cpus.release().unwrap();
            clean();
        }
    }
}

#[test]
fn actual_ap_cr4_refusal_encodes_one_residual_bit_in_both_stages_through_cpu255() {
    use cache_rendezvous::{ConfigurationField as Field, RendezvousError as Error};
    for processor in [2, 255] {
        for bit in (0..64).filter(|bit| *bit != 3) {
            for de in [0, 8] {
                let services = setup();
                CR4_DIFFERENCE.set((1u64 << bit) | de);
                with(|s| {
                    if processor == 255 {
                        s.records = (0..256)
                            .map(|n| ProcessorInformation {
                                processor_id: 0x100 + n,
                                status_flag: if n == 0 { 7 } else { 6 },
                                core: n as u32,
                                ..ProcessorInformation::default()
                            })
                            .collect();
                    }
                    s.cache_changes.insert(processor, |cache| cache.cr4 ^= CR4_DIFFERENCE.get());
                });
                let mut cpus = unsafe { prepare(&services) }.unwrap();
                let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
                let completion = unsafe {
                    cpus.with_prepared_quiescent_bsp_and_ap_observation(
                        || Ok::<_, ()>(cache),
                        |cache| cache,
                        |guard, cache| {
                            for stage in [0x400b, 0x4017] {
                                let error = cache.capture_bsp_and_compare(guard).unwrap_err();
                                assert_eq!(error, Error::Mismatch { processor, field: Field::Cr4 });
                                let expected = ((processor as u64) << 24) | ((0xc0u64 | bit) << 16);
                                assert_cache_diagnostic(cache, error, expected);
                                assert_eq!(cache.diagnostic_bits(error) | stage, expected | stage);
                                assert_eq!(
                                    cache.bsp_snapshot().unwrap().cr4,
                                    cache_snapshot(0).cr4
                                );
                            }
                        },
                        |cache| cache.release(),
                    )
                }
                .unwrap();
                assert!(completion.outcome.is_ok());
                assert_eq!(completion.cleanup, Ok(()));
                with(|s| {
                    assert_eq!(s.cache_reads.len(), processor.max(3) + 2);
                    assert_eq!(s.paging_reads, s.cache_reads);
                });
                cpus.release().unwrap();
                clean();
            }
        }
    }
}

#[test]
fn cr4_diagnostic_requires_actual_association_and_clears_it_before_a_new_capture() {
    use cache_rendezvous::{ConfigurationField as Field, RendezvousError as Error};
    for refused_recapture in [false, true] {
        let services = setup();
        with(|s| s.records.truncate(2));
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let error = Error::Mismatch { processor: 1, field: Field::Cr4 };
        assert_cache_diagnostic(&cache, error, 0x0152_0000); // No captures.
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| {
                    assert!(cache.capture_bsp_and_compare(guard).is_ok());
                    assert_cache_diagnostic(cache, error, 0x0152_0000); // Matching evidence.
                    with(|s| {
                        s.cache_changes.insert(0, |cache| cache.cr4 ^= 1 << 18);
                    });
                    assert_eq!(cache.capture_bsp_and_compare(guard), Err(error));
                    assert_cache_diagnostic(cache, error, 0x01d2_0000);
                    assert_cache_diagnostic(
                        cache,
                        Error::Mismatch { processor: 0, field: Field::Cr4 },
                        0x0052_0000,
                    );
                    assert_cache_diagnostic(
                        cache,
                        Error::Mismatch { processor: 2, field: Field::Cr4 },
                        0x0252_0000,
                    );
                    with(|s| {
                        if refused_recapture {
                            s.cache_status.insert(0, 4);
                        } else {
                            s.cache_changes.remove(&0);
                        }
                    });
                    let result = cache.capture_bsp_and_compare(guard);
                    if refused_recapture {
                        assert_eq!(
                            result,
                            Err(Error::Capture {
                                processor: 0,
                                error: cache::CaptureError::UnsupportedFeatures,
                            })
                        );
                    } else {
                        assert!(result.is_ok());
                    }
                    assert_cache_diagnostic(cache, error, 0x0152_0000);
                },
                |cache| {
                    cache.release().unwrap();
                    assert_cache_diagnostic(cache, error, 0x0152_0000);
                    Ok::<_, Status>(())
                },
            )
        }
        .unwrap();
        assert!(completion.outcome.is_ok());
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}

#[test]
fn cr4_diagnostic_never_enriches_stale_incomplete_invalid_or_non_single_bit_records() {
    use cache_rendezvous::{ConfigurationField as Field, RendezvousError as Error};
    for mode in 0..13 {
        let services = setup();
        with(|s| {
            s.cache_changes.insert(2, |cache| cache.cr4 ^= 8 | (1 << 18));
        });
        let mut cpus = unsafe { prepare(&services) }.unwrap();
        let cache = unsafe { cache_rendezvous::prepare(&services, &cpus) }.unwrap();
        let completion = unsafe {
            cpus.with_prepared_quiescent_bsp_and_ap_observation(
                || Ok::<_, ()>(cache),
                |cache| cache,
                |guard, cache| {
                    let error = cache.capture_bsp_and_compare(guard).unwrap_err();
                    assert_eq!(error, Error::Mismatch { processor: 2, field: Field::Cr4 });
                    assert_cache_diagnostic(cache, error, 0x02d2_0000);
                    // Exclusive test fault injection after every callback has
                    // returned. Fixed Slot ABI: state24, round32, status40,
                    // snapshot48. Production never performs these mutations.
                    let (base, _) = cache.storage_range().unwrap();
                    let bsp = base as *mut u8;
                    let ap = bsp.add(2 * 416);
                    match mode {
                        0 => ap.add(32).cast::<usize>().write(guard.rendezvous() - 1),
                        1 => bsp.add(32).cast::<usize>().write(guard.rendezvous() - 1),
                        2 => ap.add(24).cast::<usize>().write(0),
                        3 => ap.add(24).cast::<usize>().write(1),
                        4 => bsp.add(24).cast::<usize>().write(0),
                        5 => (*ap.add(48).cast::<cache::CacheSnapshot>()).msr_reads = 0,
                        6 => (*bsp.add(48).cast::<cache::CacheSnapshot>()).captured_fields = 0,
                        7 => ap.add(40).cast::<u32>().write(2),
                        8 => {
                            (*ap.add(48).cast::<cache::CacheSnapshot>()).cr4 = cache_snapshot(0).cr4
                        }
                        9 => {
                            (*ap.add(48).cast::<cache::CacheSnapshot>()).cr4 =
                                cache_snapshot(0).cr4 ^ 8
                        }
                        10 => (*ap.add(48).cast::<cache::CacheSnapshot>()).cr4 ^= 1 << 20,
                        11 => (*ap.add(48).cast::<cache::CacheSnapshot>()).cr0 ^= 1,
                        _ => ap.add(8).cast::<u32>().write(0),
                    }
                    assert_cache_diagnostic(cache, error, 0x0252_0000);
                },
                |cache| cache.release(),
            )
        }
        .unwrap();
        assert!(completion.outcome.is_ok());
        assert_eq!(completion.cleanup, Ok(()));
        cpus.release().unwrap();
        clean();
    }
}
