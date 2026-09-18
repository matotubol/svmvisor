//! AVIC exits: the level-EOI fallback and AVIC_INCOMPLETE_IPI routing.

use core::ptr;

use crate::{
    arch::x86_64::{
        apic::{self, HostX2Apic},
        registers::GuestRegisters,
    },
    host::resident::{
        runtime::{
            ASSIGNED_APIC_ID, AVIC_BACKING, POOL, State,
            arm::backing_alias,
            debug::{debug, hex},
            image_start,
            irq::{local_msrpm, sync_eoi_intercept},
            startup::{mailboxes, notify_native_startup},
            stop::stop,
        },
        terminal::{self, IrqSite, X2AvicStop},
    },
    svm::{
        exit::ExitSnapshot,
        vmcb::Vmcb,
        x2avic::{
            AvicExit, BackingPage,
            ipi::{self, FixedIpi, Inventory, IpiAction, IpiDrop, NmiIpi},
            irq,
            startup::NativeRoutePredicate,
        },
    },
};

/// What one AVIC exit asks of the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AvicPlan {
    /// AVIC_INCOMPLETE_IPI with an ID of 0-4 (Table 15-27 p581).
    IncompleteIpi,
    /// AVIC_NOACCEL for a level-triggered EOI write (Table 15-29 p582).
    LevelEoi(u8),
    /// Stop with this reason and value.
    Stop(u64, u64),
}

/// Runtime action for one AVIC_INCOMPLETE_IPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IncompleteIpi {
    /// Route this ICR (delivery status clear) through the startup mailbox.
    Startup(u64),
    /// Publish to the target slots and doorbell the remote ones.
    Fixed(FixedIpi),
    /// Set V_NMI on each target (the sender directly, remote targets through
    /// the NMI mailbox command and the private kick).
    Nmi(NmiIpi),
    /// Deliver nothing; count the drop and resume.
    Dropped(IpiDrop),
    /// Hardware already published the IPI (ID 1); resume.
    Published,
    /// Stop with this reason and value.
    Stop(u64, u64),
}

/// AVIC exits (D5/D6). Both are handled as traps: Table 15-22 pp566-567
/// lists the ICRL write and the level-triggered EOI write as "#VMEXIT
/// (trap)", so the write has completed and RIP has advanced. 15.29.9.2 p581
/// calls the EOI exit a fault instead; `irq::level_eoi_exit` stays correct if
/// that WRMSR runs again. nRIP is not used (15.7.1 p509 saves it only for
/// instruction, MSR and IOIO intercepts). This handler never changes RIP or
/// retries a write. WRMSR leaves RCX unchanged, so the
/// frame still names the written MSR.
pub(super) unsafe fn handle_avic_exit(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &GuestRegisters,
) -> bool {
    let exit = vmcb.exit_snapshot();
    let mismatch = X2AvicStop::ProfileMismatch as u64;
    let Some(profile) = state.avic else {
        return stop(state, exit.code, exit.rip, mismatch, 0);
    };
    if vmcb.validate_native_x2avic(&profile).is_err() {
        return stop(state, exit.code, exit.rip, mismatch, 1);
    }
    match avic_exit_plan(exit) {
        AvicPlan::IncompleteIpi => unsafe {
            handle_incomplete_ipi(state, vmcb, exit, frame.rcx as u32)
        },
        AvicPlan::LevelEoi(vector) => {
            // D6 fallback for an EOI write that was not intercepted (U11:
            // Table 15-22 p566 calls it a trap, 15.29.9.2 p581 a fault).
            // `level_eoi_exit` accepts the virtual ISR bit still set (it must
            // be the highest) or clear, and records this RIP so that a
            // re-executed WRMSR cannot EOI a second vector (`handle_avic_msr`).
            let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
            // SAFETY: armed dispatcher on its own CPU with IF/GIF clear; arm
            // admitted this CPU's enabled x2APIC, and only this runtime uses it.
            let mut host = unsafe { HostX2Apic::new() };
            if let Err(error) =
                irq::level_eoi_exit(vector, exit.rip, backing, &mut state.irq, &mut host)
            {
                let (tag, value) = terminal::irq_failure(IrqSite::LevelEoiExit, error);
                return stop(state, exit.code, exit.rip, tag, value);
            }
            // SAFETY: armed dispatcher; no other MSRPM reference is live.
            sync_eoi_intercept(&state.irq, unsafe { local_msrpm() }, vmcb);
            state.routing_retries = 0;
            true
        }
        AvicPlan::Stop(tag, value) => stop(state, exit.code, exit.rip, tag, value),
    }
}

/// D1 intercepts every other access that Table 15-22 makes a trap or fault
/// before AVIC sees it (15.29.10 p583), so AVIC_NOACCEL is expected only for
/// a level-triggered EOI write. Anything else is a profile mismatch.
pub(super) fn avic_exit_plan(exit: ExitSnapshot) -> AvicPlan {
    match AvicExit::decode(exit.code, exit.info1, exit.info2) {
        Ok(AvicExit::IncompleteIpi { .. }) => AvicPlan::IncompleteIpi,
        Ok(AvicExit::NoAcceleration {
            offset: apic::EOI,
            write: true,
            eoi_vector: Some(vector),
        }) => AvicPlan::LevelEoi(vector),
        Ok(AvicExit::NoAcceleration { .. }) => {
            let (tag, value) =
                terminal::avic_exit_refusal(X2AvicStop::NoAcceleration, exit.info1, exit.info2);
            AvicPlan::Stop(tag, value)
        }
        Err(_) => {
            let (tag, value) = terminal::avic_exit_refusal(
                X2AvicStop::UndecodableAvicExit,
                exit.info1,
                exit.info2,
            );
            AvicPlan::Stop(tag, value)
        }
    }
}

/// D5 policy (`Inventory::classify`) applied to EXITINFO1, with one tolerance:
/// ICR bit 12 is ignored. Decision: 16.13 p661 makes the eliminated delivery
/// status must-be-zero for x2APIC ICR writes and 15.29.9.1 p580 calls
/// EXITINFO1 the value written, yet the hardware may leave its busy flag set
/// on an incomplete IPI (informative only: Linux KVM avic.c
/// avic_incomplete_ipi_interception). Every other reserved bit still refuses.
/// A SELF IPI write (`msr` 83Fh) is first made its to-self ICR command
/// (`ipi::written_command`).
pub(super) fn incomplete_ipi_plan(
    inventory: &Inventory,
    exit: ExitSnapshot,
    msr: u32,
) -> IncompleteIpi {
    let icr = ipi::written_command(msr, exit.info1) & !apic::ICR_DELIVERY_STATUS;
    match inventory.classify(icr, (exit.info2 >> 32) as u32) {
        Ok(IpiAction::Startup) => IncompleteIpi::Startup(icr),
        Ok(IpiAction::Fixed(ipi)) => IncompleteIpi::Fixed(ipi),
        Ok(IpiAction::Nmi(nmi)) => IncompleteIpi::Nmi(nmi),
        Ok(IpiAction::Dropped(drop)) => IncompleteIpi::Dropped(drop),
        Ok(IpiAction::Published) => IncompleteIpi::Published,
        Err(refusal) => {
            let (tag, value) = terminal::ipi_refusal(refusal, exit.info1, exit.info2);
            IncompleteIpi::Stop(tag, value)
        }
    }
}

/// Drop a delivery-status residue (bit 12) from the backing ICR low word
/// after a handled incomplete IPI, so guest ICR reads stay x2APIC-conformant
/// (16.11.3 p659: reserved bits read as zero). Only this CPU's guest, now
/// stopped, writes its ICR; remote publishers change only IRR and TMR.
pub(super) fn clear_icr_delivery_status(backing: &BackingPage) {
    let busy = apic::ICR_DELIVERY_STATUS as u32;
    if let Ok(low) = backing.read_register(apic::ICR)
        && low & busy != 0
    {
        let _ = backing.write_register_stopped(apic::ICR, low & !busy);
    }
}

/// D5 for one AVIC_INCOMPLETE_IPI exit. The ICR write has completed, so a
/// refusal is a stop, never #GP or a retry, and nothing is republished.
/// `msr` is the guest's RCX: the ICR (830h) or SELF IPI (83Fh) just written.
unsafe fn handle_incomplete_ipi(
    state: &mut State,
    vmcb: &mut Vmcb,
    exit: ExitSnapshot,
    msr: u32,
) -> bool {
    let Some(owner) = state.icr.as_mut() else {
        return stop(state, exit.code, exit.rip, X2AvicStop::ProfileMismatch as u64, 0);
    };
    match incomplete_ipi_plan(owner.inventory(), exit, msr) {
        IncompleteIpi::Startup(icr) => {
            let result =
                owner.route_x2avic_startup(icr, unsafe { mailboxes(state.count) }, |_| unsafe {
                    notify_native_startup()
                });
            if result.is_err() {
                // The hardware instruction already completed. Never reenter
                // as an instruction retry; a refusal published nothing. An
                // INIT/SIPI whose destination matches no admitted CPU is
                // ignored by real hardware (16.5 p643), so count it as a drop
                // and resume; every other malformed form still stops.
                if owner.route_failure().map(|failure| failure.predicate)
                    == Some(NativeRoutePredicate::NoMatch)
                {
                    state.ipi_drops = state.ipi_drops.saturating_add(1);
                    debug(b"resident-ipi-nomatch cpu=");
                    hex(unsafe { ASSIGNED_APIC_ID } as u64);
                    debug(b" icr=");
                    hex(exit.info1);
                    debug(b" count=");
                    hex(state.ipi_drops);
                    debug(b"\n");
                } else {
                    let (tag, value) =
                        terminal::startup_route_refusal(owner.route_failure(), exit.info1);
                    return stop(state, exit.code, exit.rip, tag, value);
                }
            }
        }
        IncompleteIpi::Nmi(nmi) => {
            // Guest NMI IPI (Table 16-4 p644 allowed it here). Remote targets
            // get the NMI mailbox command and the private kick; the sender, if
            // its own explicit destination selects it, sets V_NMI directly
            // (15.21.10 p536). NMI queues no destination record.
            let source = owner.inventory().source_slot();
            let targets = nmi.targets();
            let remote = targets & !(1 << source);
            let result =
                owner.route_x2avic_nmi(remote, unsafe { mailboxes(state.count) }, |_| unsafe {
                    notify_native_startup()
                });
            if result.is_err() {
                let (tag, value) =
                    terminal::startup_route_refusal(owner.route_failure(), exit.info1);
                return stop(state, exit.code, exit.rip, tag, value);
            }
            if targets & (1 << source) != 0 {
                match state.avic {
                    Some(profile) => {
                        if vmcb.set_guest_v_nmi_pending(&profile).is_err() {
                            return stop(
                                state,
                                exit.code,
                                exit.rip,
                                X2AvicStop::ProfileMismatch as u64,
                                3,
                            );
                        }
                    }
                    None => {
                        return stop(
                            state,
                            exit.code,
                            exit.rip,
                            X2AvicStop::ProfileMismatch as u64,
                            0,
                        );
                    }
                }
            }
        }
        IncompleteIpi::Fixed(ipi) => {
            let result = owner.inventory().deliver_fixed(
                ipi,
                // SAFETY: dispatcher under this CPU's private root; the
                // inventory resolves only slots below its admitted count,
                // which arm bound to the pool (`state.count`).
                |slot| unsafe { remote_backing(slot) },
                // SAFETY: CPL0 with AVIC admitted at arm (CPUID Fn8000_000A
                // EDX[13]); `DoorbellTarget` bounds the ID, so the WRMSR
                // cannot fault whatever the receiver does. The receiver need
                // not be armed: AP guests run before the BSP arms. DXE
                // published every table entry (valid, backing page, host ID)
                // and every slot's prepared backing page before the first
                // arm, nothing clears V or IsRunning, and each CPU sets its
                // own IsRunning at the end of its arm, so every published
                // target page stays valid. A doorbell to a core in host mode
                // has no defined effect (15.29.8.2 p579 defines guest-mode
                // receipt only); that core evaluates the page's IRR at its
                // next VMRUN (15.29.8.3 p579).
                |target| unsafe { apic::ring_avic_doorbell(target) },
            );
            if let Err(error) = result {
                let (tag, value) = terminal::fan_out_failure(error, exit.info1);
                return stop(state, exit.code, exit.rip, tag, value);
            }
        }
        IncompleteIpi::Dropped(_drop) => {
            state.ipi_drops = state.ipi_drops.saturating_add(1);
            debug(b"resident-ipi-drop cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b" icr=");
            hex(exit.info1);
            debug(b" count=");
            hex(state.ipi_drops);
            debug(b"\n");
        }
        // ID 1: hardware published every valid target (15.29.6.1 step 5
        // p577); a target that is not running evaluates IRR at its first
        // VMRUN (15.29.8.3 p579).
        IncompleteIpi::Published => {}
        IncompleteIpi::Stop(tag, value) => return stop(state, exit.code, exit.rip, tag, value),
    }
    clear_icr_delivery_status(unsafe { &*ptr::addr_of!(AVIC_BACKING) });
    state.routing_retries = 0;
    true
}

/// Backing page of dense slot `slot` through this CPU's private-root alias
/// (D7): `prepare` maps one RW/NX alias per pool slot, this CPU's included,
/// and DXE walks all of them before arm.
/// # Safety
/// This CPU's private root is loaded (a dispatcher path after
/// `svmvisor_resident_enter`; arm still runs on the caller's root), `slot` is
/// below the armed pool slot count, and the caller uses only the page's
/// atomic operations, as for any shared backing page.
unsafe fn remote_backing(slot: usize) -> &'static BackingPage {
    debug_assert!((slot as u64) < unsafe { POOL.1 } >> 20);
    unsafe {
        &*(backing_alias(ptr::addr_of!(image_start) as u64, slot as u64) as *const BackingPage)
    }
}
