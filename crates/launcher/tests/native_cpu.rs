#![cfg(feature = "native-preflight")]

// Partial include: the accessors only the removed cache rendezvous read are unused here.
#[path = "../src/native/admission/cpu.rs"]
#[allow(dead_code)]
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

use uefi_raw::{
    Boolean, Event, Guid, Status,
    table::boot::{BootServices, MemoryType, Tpl},
};

use crate::cpu::*;

thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
thread_local! { static AP_IDENTITY: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }

static PROTOCOL: MpServicesProtocol = MpServicesProtocol {
    get_number_of_processors: counts,
    get_processor_info: information,
    startup_all_aps: dispatch,
    startup_this_ap: this_ap,
    switch_bsp: switch,
    enable_disable_ap: enable,
    who_am_i: identity,
};

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
