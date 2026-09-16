//! UEFI 2.10 §7.1.2 event groups. Notify callbacks perform no firmware calls.
use crate::{cpu, mmio::JournalMapping, pci_io::status_result};
use core::sync::atomic::{AtomicBool, Ordering};
use core::{ffi::c_void, ptr::null_mut};
use svmvisor_dxe::diagnostics::trace::{EventKind, Trace};
use uefi_raw::{
    Event, Status, guid,
    table::boot::{BootServices, EventType, Tpl},
};

// One context per ROM image, restricted by binding to its single owner. Event
// callbacks run on the BSP at the same TPL_NOTIFY and cannot preempt each other.
// Registration/teardown keep ACTIVE false across external firmware calls; no
// reference to these statics spans a call that can dispatch a notification.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static EXITED: AtomicBool = AtomicBool::new(false);
static mut MAPPING: Option<JournalMapping> = None;
static mut TRACE: Trace = Trace::new(0);
static mut EVENTS: [Event; 3] = [null_mut(); 3];

pub(crate) fn register(
    services: &BootServices,
    mapping: JournalMapping,
    boot_id: u32,
) -> Result<(), Status> {
    unsafe {
        ACTIVE.store(false, Ordering::Release);
        EXITED.store(false, Ordering::Release);
        MAPPING = Some(mapping);
        TRACE = Trace::new(boot_id);
        #[cfg(feature = "card-load-only")]
        {
            TRACE = Trace::new_card_result(boot_id, true);
        }
        #[cfg(feature = "card-returning-loader")]
        {
            TRACE = match crate::card_returning_adapter::diagnostics() {
                Some(diagnostics) => Trace::new_returning_diagnostics(boot_id, diagnostics),
                None => Trace::new_returning_result(
                    boot_id,
                    crate::card_returning_adapter::result_bits(),
                ),
            };
        }
    }
    let groups = [
        guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b"),
        guid!("3a2a00ad-98b9-4cdf-a478-702777f1c10b"),
        guid!("27abf055-b1b8-4c26-8048-748f37baa2df"),
    ];
    for (index, mut group) in groups.into_iter().enumerate() {
        let mut event = null_mut();
        // Context is a numeric group selector, never dereferenced.
        let status = unsafe {
            (services.create_event_ex)(
                EventType::NOTIFY_SIGNAL,
                Tpl::NOTIFY,
                Some(notify),
                index as *mut c_void,
                &mut group,
                &mut event,
            )
        };
        status_result(status)?;
        if event.is_null() {
            return Err(Status::DEVICE_ERROR);
        }
        unsafe {
            EVENTS[index] = event;
        }
    }
    ACTIVE.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn has_exited() -> bool {
    EXITED.load(Ordering::Acquire)
}

pub(crate) fn unregister(services: &BootServices) -> Result<(), Status> {
    ACTIVE.store(false, Ordering::Release);
    let mut result = Ok(());
    for index in 0..3 {
        let event = unsafe { EVENTS[index] };
        if !event.is_null() {
            let status = unsafe { (services.close_event)(event) };
            if status.is_error() {
                result = Err(status);
            } else {
                unsafe {
                    EVENTS[index] = null_mut();
                }
            }
        }
    }
    result
}

unsafe extern "efiapi" fn notify(_: Event, context: *mut c_void) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Some(mut mapping) = (unsafe { MAPPING }) else {
        return;
    };
    let event = match context as usize {
        0 => EventKind::ReadyToBoot,
        1 => EventKind::AfterReadyToBoot,
        2 => {
            EXITED.store(true, Ordering::Release);
            EventKind::ExitBootServices
        }
        _ => return,
    };
    #[cfg(feature = "card-resident-loader")]
    if crate::card_returning_adapter::journal_owned_by_child() {
        return;
    }
    let (tsc, cpu) = cpu::sample();
    // SAFETY: same-TPL BSP callbacks serialize access; this path has no firmware
    // calls, timer dependencies, allocation, or calls that dispatch callbacks.
    let _ = unsafe { (&mut *(&raw mut TRACE)).record(&mut mapping, event, tsc, cpu) };
}
