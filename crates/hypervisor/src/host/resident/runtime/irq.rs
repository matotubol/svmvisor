//! Physical interrupt capture into the host IRQ bridge and the guest EOI intercept.

use core::ptr;

use crate::{
    arch::x86_64::apic::{HostX2Apic, PhysicalX2Apic},
    host::resident::{
        runtime::{
            AVIC_BACKING, MSRPM, State, exit::retry_routing, stop::stop,
            svmvisor_resident_accept_irq,
        },
        terminal::{self, IrqSite, X2AvicStop},
    },
    svm::{
        permission_maps::Msrpm,
        vmcb::Vmcb,
        x2avic::{
            BackingPage,
            irq::{self, Capture, PhysicalIrqLedger},
        },
    },
};

/// Accept one physical source through the bounded assembly mailbox and hand
/// it to the host IRQ bridge (`capture_accepted`). The gate touches no Rust
/// owner; IF/GIF are clear before this function reads state.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped.
pub(super) unsafe fn capture_physical_irq(state: &mut State, vmcb: &mut Vmcb) -> bool {
    let exit = vmcb.exit_snapshot();
    let vector = unsafe { svmvisor_resident_accept_irq() };
    if vector == u32::MAX {
        return retry_routing(state, vmcb);
    }
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    // SAFETY: armed dispatcher on its own CPU with IF/GIF clear; arm admitted
    // this CPU's enabled x2APIC, and only this runtime uses it.
    let mut host = unsafe { HostX2Apic::new() };
    // SAFETY: this function's contract; no other MSRPM reference is live.
    let msrpm = unsafe { local_msrpm() };
    match capture_accepted(vector, backing, &mut state.irq, &mut host, msrpm, vmcb) {
        // A physical spurious interrupt: nothing to publish or acknowledge.
        Ok(None) => {}
        Ok(Some(Capture::Discarded)) => state.irq_discards = state.irq_discards.saturating_add(1),
        Ok(Some(_)) => state.intr = state.intr.saturating_add(1),
        Err((tag, value)) => return stop(state, exit.code, exit.rip, tag, value),
    }
    state.routing_retries = 0;
    true
}

/// Bridge one vector that the acceptance helper returned (`irq::capture`),
/// then resynchronize the guest EOI intercept, since a newly held level
/// source needs its guest EOI intercepted (D6). `Err` is a stop reason and
/// value: a helper result above 255, or a bridge failure (vectors 16-31
/// included, which the host IDT reports through the window gates).
pub(super) fn capture_accepted(
    vector: u32,
    backing: &BackingPage,
    ledger: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
    msrpm: &mut Msrpm,
    vmcb: &mut Vmcb,
) -> Result<Option<Capture>, (u64, u64)> {
    let Ok(vector) = u8::try_from(vector) else {
        return Err((X2AvicStop::AcceptedVector as u64, u64::from(vector)));
    };
    let capture = irq::capture(vector, backing, ledger, physical)
        .map_err(|error| terminal::irq_failure(IrqSite::Capture, error))?;
    sync_eoi_intercept(ledger, msrpm, vmcb);
    Ok(capture)
}

/// D6: guest EOI writes are intercepted exactly while this CPU's ledger holds
/// a level source or expects a re-executed EOI write
/// (`PhysicalIrqLedger::intercepts_eoi`). APM2 rev3.44 Figure 15-4 p527 does not say whether VMRUN
/// caches the map contents, so a change also clears every VMCB clean bit.
pub(super) fn sync_eoi_intercept(ledger: &PhysicalIrqLedger, msrpm: &mut Msrpm, vmcb: &mut Vmcb) {
    if msrpm.update_x2apic_eoi_intercept(ledger) {
        vmcb.invalidate_all();
    }
}

/// This CPU's private MSRPM, which only its own VMCB names.
/// # Safety
/// The armed dispatcher (or arm) of this CPU with its guest stopped, holding
/// no other reference to the map.
pub(super) unsafe fn local_msrpm() -> &'static mut Msrpm {
    unsafe { &mut *ptr::addr_of_mut!(MSRPM) }
}
