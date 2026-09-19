//! INIT and NMI windows and the startup service: mailbox commands, guest INIT and
//! SIPI on their stopped destination CPU.

use core::{
    arch::{asm, x86_64::__cpuid_count},
    ptr,
    sync::atomic::Ordering,
};

use crate::{
    arch::x86_64::{
        apic::{self, HostX2Apic, PhysicalX2Apic},
        registers::GuestRegisters,
    },
    host::resident::{
        runtime::{
            ASSIGNED_APIC_ID, AVIC_BACKING, MSRPM, State, cache,
            debug::{debug, hex},
            image_start,
            msr::write_msr,
            stop::{stop, terminal_requested},
            svmvisor_resident_init_acks, svmvisor_resident_nmi_pending,
        },
        terminal::{self, StartupStage},
    },
    svm::{
        cache::CacheCore,
        events::ExternalInterruptError,
        permission_maps::Msrpm,
        vmcb::Vmcb,
        x2avic::{
            BackingPage, NativeX2AvicProfile, registers,
            startup::{
                NativeDestinationCause, NativeDestinationMode, NativeIcrError,
                NativeStartupCommand, NativeStartupEffect, NativeStartupMailbox,
                NativeStartupState, NativeStartupTarget, ROUTE_WAIT_ATTEMPTS, lock_routes_within,
                validate_destination_slot,
            },
        },
    },
};

/// Stop tag (`info1`) of a stalled physical-NMI drain; `info2` = the
/// consecutive misses. `stop_words` exports it as an unhandled exit 61h with
/// the guest RIP; the tag stays in the stop record and the context export.
pub(super) const NMI_DRAIN_STALL: u64 = 0xf113;

/// Consecutive undrained VMEXIT_NMI exits that stop this CPU. One miss is
/// tolerated (the NMI may already have been consumed) and a second is margin;
/// a real stall repeats at every VMRUN without guest progress, and three
/// exits still leave two earlier entries in the five-entry exit history.
pub(super) const NMI_DRAIN_MISS_LIMIT: u8 = 3;

/// Polls of an AwaitSipi wait between two GIF windows (`acknowledge_init`).
const AWAIT_SIPI_POLLS: u32 = 1 << 16;

/// Attempts of one pending command while this core's cache lease is busy.
const STARTUP_LEASE_ATTEMPTS: u32 = 1 << 20;

/// Commands a Running destination applies in one exit before it resumes its
/// guest; the rest are serviced at its next exit (every exit services them).
const STARTUP_COMMANDS_PER_EXIT: u32 = 64;

/// Outcome of one startup command (`startup_step`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupStep {
    /// Committed and removed from the mailbox.
    Applied(NativeStartupEffect),
    /// The cache lease was busy; nothing changed and the command stays queued.
    Busy,
    /// A pending guest event refused the command before any change.
    PendingEvent(ExternalInterruptError),
    /// Stop at this stage with this value (`None`: the AwaitSipi flag).
    Failed(StartupStage, Option<u64>),
}

/// Hardware owners that a guest INIT changes on its own CPU.
pub(super) struct InitOwners<'a, P> {
    pub(super) backing: &'a BackingPage,
    pub(super) physical: P,
    pub(super) msrpm: &'a mut Msrpm,
    /// CPUID Fn0000_0001 EAX, the INIT value of EDX (APM2 Table 14-2).
    pub(super) signature: u32,
}

impl InitOwners<'static, HostX2Apic> {
    /// # Safety
    /// This CPU's armed dispatcher with its guest stopped and IF/GIF clear:
    /// arm admitted the enabled physical x2APIC, and no other reference to
    /// the private MSRPM is live.
    unsafe fn local(signature: u32) -> Self {
        unsafe {
            Self {
                backing: &*ptr::addr_of!(AVIC_BACKING),
                physical: HostX2Apic::new(),
                msrpm: &mut *ptr::addr_of_mut!(MSRPM),
                signature,
            }
        }
    }
}

/// Re-present a physical NMI the host vector-2 gate swallowed (irq.S set
/// `svmvisor_resident_nmi_pending`) to the guest as a virtual NMI. APM2
/// rev3.44 15.21.10 p536: platform NMIs are re-presented under NMI
/// virtualization, so Windows still receives them. Virtual NMIs coalesce, so
/// several drained physical NMIs become one V_NMI. A failure only happens on a
/// shutdown/non-armed VMCB that is already terminal, so the NMI is dropped.
/// Returns whether the gate had taken an NMI since the previous call.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
pub(super) unsafe fn route_physical_nmi_to_guest(state: &mut State, vmcb: &mut Vmcb) -> bool {
    if svmvisor_resident_nmi_pending.swap(0, Ordering::AcqRel) == 0 {
        return false;
    }
    if let Some(profile) = state.avic {
        let _ = vmcb.set_guest_v_nmi_pending(&profile);
    }
    true
}

/// A VMEXIT_NMI leaves the physical NMI pending (APM2 rev3.44 Table 15-13
/// p536) and the host GIF window must take it (Table 15-10 p530), or the next
/// VMRUN exits again at once, without end. Count the consecutive 61h exits
/// whose window did not set the gate's flag; any other exit is guest progress
/// and a drained one is the design working, so both reset the count. Returns
/// whether this CPU must stop.
pub(super) fn nmi_drain_stalled(misses: &mut u8, code: u64, drained: bool) -> bool {
    if code != 0x61 || drained {
        *misses = 0;
        return false;
    }
    *misses = misses.saturating_add(1);
    *misses >= NMI_DRAIN_MISS_LIMIT
}

/// Service startup commands on this stopped destination only; it never
/// enters a reset-vector guest or calls firmware. Running returns when its
/// queue is empty (`Some(true)` after a guest change, `None` when nothing
/// changed) or after `STARTUP_COMMANDS_PER_EXIT` commands.
///
/// AwaitSipi has no guest to run, so it waits for its SIPI or a terminal
/// request without a bound (a CPU parked in wait-for-SIPI), polling with
/// PAUSE. Every `AWAIT_SIPI_POLLS` polls it opens a GIF window: INIT
/// notifications, NMI and external SMI are held pending while GIF=0 (APM2
/// rev3.44 Table 15-10 p530), and firmware SMM needs its SMIs. A physical NMI
/// taken in any host GIF window, this one included, reaches the returning host
/// vector-2 gate and is re-presented to the guest as V_NMI at its next VMRUN
/// (15.21.10 p536), not stopped. This AwaitSipi guest has no VMRUN until its
/// SIPI, so a physical NMI here waits in `svmvisor_resident_nmi_pending` until
/// the guest starts. A pending command whose cache lease stays busy is retried
/// `STARTUP_LEASE_ATTEMPTS` times, then stops (stage 9).
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
pub(super) unsafe fn service_startup(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> Option<bool> {
    let shared = unsafe { mailboxes(state.count) };
    let mut changed = false;
    let (mut polls, mut busy, mut applied) = (0u32, 0u32, 0u32);
    loop {
        if unsafe { terminal_requested(state) } {
            #[cfg(feature = "resident-runtime-test")]
            if state.startup == NativeStartupState::AwaitSipi {
                debug(b"resident-terminal await-sipi-peer=");
                hex(state.slot as u64);
                debug(b"\n");
            }
            return Some(false);
        }
        let Some(command) = shared[state.slot].peek() else {
            if state.startup == NativeStartupState::Running {
                return changed.then_some(true);
            }
            polls = polls.wrapping_add(1);
            if polls % AWAIT_SIPI_POLLS == 0 && unsafe { acknowledge_init() }.is_none() {
                return startup_stage_stop(state, vmcb, StartupStage::InitAcknowledgment);
            }
            core::hint::spin_loop();
            continue;
        };
        if state.startup == NativeStartupState::Running && applied == STARTUP_COMMANDS_PER_EXIT {
            return changed.then_some(true);
        }
        if state.cache_active {
            return startup_stage_stop(state, vmcb, StartupStage::CacheReplay);
        }
        if unsafe { acknowledge_init() }.is_none() {
            return startup_stage_stop(state, vmcb, StartupStage::InitAcknowledgment);
        }
        let Some(profile) = state.avic else {
            return startup_stage_stop(state, vmcb, StartupStage::OwnerMissing);
        };
        if command == NativeStartupCommand::Nmi {
            // A guest NMI IPI (`ipi::NmiIpi`) queued by another vCPU. Set this
            // stopped guest's V_NMI directly (15.21.10 p536); no LAPIC, cache
            // or route-record work. `complete` is a lock-free FIFO removal, so
            // it needs no route lease. A target in AwaitSipi has V_NMI cleared
            // by its INIT reset and the pending NMI is delivered once it starts
            // (16.5 p643: NMI held pending in the INIT state until STARTUP).
            if vmcb.set_guest_v_nmi_pending(&profile).is_err() {
                return startup_stage_stop(state, vmcb, StartupStage::TargetApplication);
            }
            if shared[state.slot].complete(command).is_err() {
                return startup_stage_stop(state, vmcb, StartupStage::MailboxCompletion);
            }
            (busy, applied) = (0, applied + 1);
            changed = true;
            debug(b"resident-guest-nmi cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b"\n");
            continue;
        }
        let signature = __cpuid_count(1, 0).eax;
        let core = state.cache_observation.is_some().then(|| unsafe { cache::core(state) });
        // SAFETY: this function's contract.
        let owners = unsafe { InitOwners::local(signature) };
        let step = startup_step(
            state,
            vmcb,
            frame,
            shared,
            &profile,
            command,
            core,
            owners,
            // SAFETY: called once by a successful INIT commit, after every
            // fallible step, on this stopped guest's CPU.
            || unsafe { svmvisor_resident_reset_guest_debug() },
        );
        match step {
            StartupStep::Applied(effect) => {
                (busy, applied) = (0, applied + 1);
                changed |= effect != NativeStartupEffect::Ignored;
                startup_debug(command, effect);
            }
            StartupStep::Busy if busy < STARTUP_LEASE_ATTEMPTS => {
                busy += 1;
                core::hint::spin_loop();
            }
            StartupStep::Busy => {
                return startup_stage_stop(state, vmcb, StartupStage::WaitExhausted);
            }
            StartupStep::PendingEvent(error) => {
                let exit = vmcb.exit_snapshot();
                let (tag, value) =
                    terminal::startup_pending_failure(error, unsafe { ASSIGNED_APIC_ID });
                stop(state, exit.code, exit.rip, tag, value);
                return Some(false);
            }
            StartupStep::Failed(stage, value) => {
                return startup_value_stop(state, vmcb, stage, value);
            }
        }
    }
}

/// APM2 15.21.8/Table15-12 and15.28: consume held INIT through private #SX,
/// with IF=0 throughout. No physical IRQ is acknowledged and no CR8/APIC state
/// is changed. The window also takes held external SMIs (firmware SMM) and
/// NMIs (Table 15-10 p530); a held physical NMI now reaches the returning host
/// vector-2 gate (irq.S), which sets `svmvisor_resident_nmi_pending` and
/// returns, so the dispatcher re-presents it to the guest as V_NMI
/// (`route_physical_nmi_to_guest`) instead of stopping this CPU (15.21.10
/// p536). Counts may coalesce; only the mailbox owns guest startup commands.
pub(super) unsafe fn acknowledge_init() -> Option<u64> {
    let before = svmvisor_resident_init_acks.load(Ordering::Acquire);
    for _ in 0..1024 {
        let previous = svmvisor_resident_init_acks.load(Ordering::Acquire);
        unsafe {
            asm!("stgi", "nop", "clgi", options(nostack));
        }
        let current = svmvisor_resident_init_acks.load(Ordering::Acquire);
        if current == previous {
            return Some(current.wrapping_sub(before));
        }
    }
    None
}

/// One peeked startup command on its stopped destination (D9). "Route lease,
/// then core lease" stays the only nesting; this step holds one at a time:
///
/// 1. Lease-free checks: no pending guest event and the armed profile
///    (`validate_x2avic`), and a destination slot that names an admitted CPU.
/// 2. Under this core's cache lease when cache replay is owned (`core`),
///    which must be idle so no sibling starts E0 between check and reset:
///    the INIT commit (`guest_init`) or the SIPI commit. The lease is
///    released before the route lease is requested.
/// 3. Under the route lease, waited for up to `ROUTE_WAIT_ATTEMPTS`: the
///    guest INIT's destination record (D9 step 7, now after step 8), then the
///    mailbox completion (step 9), which must not interleave with a source's
///    FIFO preflight and store.
///
/// A failure once step 2 has begun is terminal and leaves the command
/// queued; stages 2, 4 and 8 then follow an applied command.
#[allow(clippy::too_many_arguments)]
pub(super) fn startup_step<P: PhysicalX2Apic>(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    mailboxes: &[NativeStartupMailbox],
    profile: &NativeX2AvicProfile,
    command: NativeStartupCommand,
    core: Option<&CacheCore>,
    owners: InitOwners<'_, P>,
    reset_guest_debug: impl FnOnce(),
) -> StartupStep {
    let signature = owners.signature;
    let target = NativeStartupTarget {
        vmcb: &mut *vmcb,
        frame: &mut *frame,
        state: &mut state.startup,
        signature,
    };
    let effect = match target.validate_x2avic(command, profile) {
        Ok(effect) => effect,
        Err(NativeIcrError::PendingState(error)) => return StartupStep::PendingEvent(error),
        Err(_) => return StartupStep::Failed(StartupStage::TargetApplication, None),
    };
    let Ok(mailbox) = validate_destination_slot(mailboxes, state.slot) else {
        return StartupStep::Failed(StartupStage::ModeCommitPreparation, None);
    };
    let lease = match core.map(|core| core.try_lock()) {
        None => None,
        Some(None) => return StartupStep::Busy,
        Some(Some(lease)) if lease.phase != 0 => {
            return StartupStep::Failed(StartupStage::CacheReplay, None);
        }
        Some(lease) => lease,
    };
    if effect == NativeStartupEffect::Init {
        if let Err((stage, value)) =
            guest_init(state, vmcb, frame, profile, owners, reset_guest_debug)
        {
            return StartupStep::Failed(stage, value);
        }
    } else if (NativeStartupTarget { vmcb, frame, state: &mut state.startup, signature })
        .apply_x2avic(command, profile)
        .is_err()
    {
        return StartupStep::Failed(StartupStage::TargetApplication, None);
    }
    drop(lease);
    let Ok(routes) = lock_routes_within(mailboxes, ROUTE_WAIT_ATTEMPTS) else {
        return StartupStep::Failed(StartupStage::RouteTable, None);
    };
    if effect == NativeStartupEffect::Init {
        let Ok(destination) =
            routes.prepare_destination_mode(state.slot, NativeDestinationMode::X2Apic)
        else {
            return StartupStep::Failed(StartupStage::ModeCommitPreparation, None);
        };
        // The destination stays x2APIC; record the guest INIT.
        destination.commit_destination_mode_from(NativeDestinationCause::GuestInit);
    }
    if mailbox.complete(command).is_err() {
        return StartupStep::Failed(StartupStage::MailboxCompletion, None);
    }
    drop(routes);
    StartupStep::Applied(effect)
}

/// Same retained shared backing and inventory as arm; called only under the
/// private root. Sole target owns local CPU state, all shared writes are atomic.
pub(super) unsafe fn mailboxes(count: usize) -> &'static [NativeStartupMailbox] {
    unsafe {
        core::slice::from_raw_parts(
            (ptr::addr_of!(image_start) as u64 + super::STARTUP_PAGE_OFFSET)
                as *const NativeStartupMailbox,
            count,
        )
    }
}

/// Private notification only after the routing owner has proved every assigned
/// CPU ready and published a target queue. APM2 16.5/Table16-4: shorthand11
/// ignores destination width and excludes the source. R_INIT/#SX consumes every
/// hardware wake; CPUs with empty queues resume unchanged. No guest INIT is
/// inferred from a wake, and no guest interrupt is acknowledged here.
pub(super) unsafe fn notify_native_startup() {
    unsafe { send_native_notification() };
}

/// Same validated physical bus and already idle ICR as terminal_finish. The
/// all-ready target set has R_INIT/#SX installed. IF/GIF stay zero on sender.
pub(super) unsafe fn send_native_notification() {
    unsafe {
        asm!("mfence", options(nostack, preserves_flags));
        write_msr(apic::ICR_MSR, 0x000c_0500);
    }
}

fn startup_debug(command: NativeStartupCommand, effect: NativeStartupEffect) {
    let cpu = u64::from(unsafe { ASSIGNED_APIC_ID });
    match (command, effect) {
        (_, NativeStartupEffect::Init) => {
            debug(b"resident-guest-init cpu=");
            hex(cpu);
            debug(b" kick-acks=");
            hex(svmvisor_resident_init_acks.load(Ordering::Acquire));
        }
        (NativeStartupCommand::Sipi(vector), NativeStartupEffect::Started) => {
            debug(b"resident-guest-sipi cpu=");
            hex(cpu);
            debug(b" vector=");
            hex(u64::from(vector));
        }
        _ => {
            debug(b"resident-guest-sipi-ignored cpu=");
            hex(cpu);
        }
    }
    debug(b"\n");
}

/// D9 guest INIT on its stopped destination CPU, after `validate_x2avic`
/// returned Init, with the cache lease (if owned) held by `startup_step`.
///
/// Preparation is read-only: the backing identity and the physical ISR
/// (`registers::prepare_init`), and the guest APIC and startup-owned EFER
/// owners (the INIT EFER is computed on a copy). The commit then follows D9:
/// LAPIC (physical timer/LVT reset, level-source retirement, EOI
/// acceleration, backing reset, and the guest APIC's held-IRR record), CPU
/// state (V_TPR 0, every clean bit clear), logical EFER, live DR0-3. A
/// failure after the first commit step is terminal: the caller stops and
/// never resumes. The caller then records the INIT at the destination and
/// completes the mailbox command under the route lease. `Err` carries the
/// startup stage and its value (`None`: the AwaitSipi flag).
fn guest_init<P: PhysicalX2Apic>(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    profile: &NativeX2AvicProfile,
    mut owners: InitOwners<'_, P>,
    reset_guest_debug: impl FnOnce(),
) -> Result<(), (StartupStage, Option<u64>)> {
    let lapic =
        |stage: StartupStage, error| (stage, Some(u64::from(terminal::init_error_code(error))));
    registers::prepare_init(owners.backing, &state.irq, &mut owners.physical)
        .map_err(|error| lapic(StartupStage::InitPreparation, error))?;
    let (Some(mut efer), true) = (state.efer, state.guest_apic.is_some()) else {
        return Err((StartupStage::OwnerMissing, None));
    };
    efer.reset_after_init().map_err(|_| (StartupStage::EferReset, None))?;
    // Steps 1-4 (`registers::commit_init`). The reset page is
    // software-disabled with an empty IRR, so the guest APIC holds nothing.
    registers::commit_init(owners.backing, &mut state.irq, &mut owners.physical, owners.msrpm)
        .map_err(|error| lapic(StartupStage::InitLapicCommit, error))?;
    if let Some(guest) = state.guest_apic.as_mut() {
        guest.reset_after_init();
    }
    // Step 5: CPU INIT state (`Vmcb::initialize_ap_after_init`).
    NativeStartupTarget { vmcb, frame, state: &mut state.startup, signature: owners.signature }
        .apply_x2avic(NativeStartupCommand::Init, profile)
        .map_err(|_| (StartupStage::InitCpuCommit, None))?;
    // Step 6: INIT clears the logical EFER (`NativeEfer::reset_after_init`).
    state.efer = Some(efer);
    // Step 8: DR0-3 are live guest state (APM2 Table 14-1 p482).
    reset_guest_debug();
    Ok(())
}

fn startup_stage_stop(state: &mut State, vmcb: &Vmcb, stage: StartupStage) -> Option<bool> {
    startup_value_stop(state, vmcb, stage, None)
}

/// Startup service stop. `value` defaults to 1 for AwaitSipi, 0 for Running.
fn startup_value_stop(
    state: &mut State,
    vmcb: &Vmcb,
    stage: StartupStage,
    value: Option<u64>,
) -> Option<bool> {
    let exit = vmcb.exit_snapshot();
    let value = value.unwrap_or(u64::from(state.startup == NativeStartupState::AwaitSipi));
    let (tag, value) =
        terminal::startup_failure(7, stage as u8, value, unsafe { ASSIGNED_APIC_ID });
    stop(state, exit.code, exit.rip, tag, value);
    Some(false)
}

/// Reset only the stopped target's guest-live breakpoint addresses.
/// APM2 rev3.44 Table14-1 p482,15.5.1/15.7: INIT resets DR0-3, which are
/// not part of the ordinary VMCB state switch. DR6/7 reset in the guest VMCB.
/// # Safety
/// The sole successful target-owned INIT commit (`guest_init`) calls this
/// with IF/GIF clear, after all fallible preparation and its LAPIC and CPU
/// commits, on the same nonmigrating guest CPU. Host breakpoints/GD are
/// disabled after VMEXIT; no external debugger or host DR owner exists.
/// Never call for a private wake, refused INIT or SIPI. Keep this
/// out-of-line symbol for the exact linked debug-write audit.
#[unsafe(no_mangle)]
#[inline(never)]
unsafe extern "C" fn svmvisor_resident_reset_guest_debug() {
    unsafe {
        asm!(
            "xor eax, eax",
            "mov dr0, rax",
            "mov dr1, rax",
            "mov dr2, rax",
            "mov dr3, rax",
            out("rax") _,
            options(nostack),
        );
    }
}
