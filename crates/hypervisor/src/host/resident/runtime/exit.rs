//! The dispatcher: exit history, exit sequencing against the startup service and
//! the acknowledged guest's exit handlers.

use core::{arch::x86_64::__cpuid_count, ptr, sync::atomic::Ordering};

#[cfg(feature = "resident-runtime-test")]
use crate::host::resident::runtime::debug::read_native_apic;
use crate::{
    arch::x86_64::{
        apic,
        msr::{EFER, HWCR, HWCR_CPUID_FLT_EN, MMIO_CFG_BASE_ADDR, SYS_CFG, VM_CR},
        registers::GuestRegisters,
    },
    host::resident::{
        BridgeContext,
        runtime::{
            ASSIGNED_APIC_ID, CONTEXT, EXIT_HISTORY, EXIT_HISTORY_COUNT, EXIT_HISTORY_NEXT, FRAME,
            NPT, STATE, State, VMCB,
            avic::handle_avic_exit,
            cache,
            debug::{debug, diagnostic_record, hex},
            diagnostics,
            guest_reader::{GuestReader, fetch_instruction},
            irq::capture_physical_irq,
            msr::{handle_avic_msr, handle_mcax_msr, handle_syscfg, read_msr},
            startup::{
                NMI_DRAIN_STALL, acknowledge_init, mailboxes, nmi_drain_stalled,
                route_physical_nmi_to_guest, service_startup,
            },
            stop::{stop, terminal_control, terminal_enabled, terminal_finish, terminal_requested},
            svmvisor_resident_init_acks,
        },
        terminal::{self, X2AvicStop},
    },
    svm::{
        dispatch::{self, NativeMsrOutcome},
        exit::ExitSnapshot,
        vmcb::{ReinjectOutcome, Vmcb},
    },
};

/// The stopped exit that `dispatch_body` is handling.
struct ExitContext<'a> {
    state: &'a mut State,
    vmcb: &'a mut Vmcb,
    frame: &'a mut GuestRegisters,
    exit: ExitSnapshot,
}

/// Where queued startup commands are serviced relative to an exit's own
/// handler, once the guest has acknowledged its bootstrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExitOrder {
    /// The exit reports completed guest work (Table 15-22 p567 lists the ICRL
    /// write and the level EOI write as traps) or a physical interrupt at an
    /// instruction boundary. Its effect belongs before a later INIT, so the
    /// handler runs first; the startup service then runs as for any exit.
    TrapThenStartup,
    /// An instruction intercept or other fault-style exit: the guest
    /// instruction has not run. A queued INIT resets the guest first and the
    /// intercepted instruction is then never completed.
    StartupThenExit,
}

/// # Safety
/// Called only by the audited integer assembly after VMEXIT, host auxiliary
/// restore, private stack and GIF/IF clear. The one stopped guest is exclusive.
pub(super) unsafe extern "win64" fn dispatch(context: *mut BridgeContext) -> bool {
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if context != ptr::addr_of_mut!(CONTEXT) {
        let exit = unsafe { &*ptr::addr_of!(VMCB) }.exit_snapshot();
        stop(state, exit.code, exit.rip, 0xf10a, context as u64);
        unsafe { terminal_finish(state) };
        return false;
    }
    if unsafe { terminal_requested(state) } {
        unsafe { terminal_finish(state) };
        return false;
    }
    let before = unsafe { &*ptr::addr_of!(VMCB) }.exit_snapshot();
    unsafe {
        let frame = ptr::read_volatile(ptr::addr_of!(FRAME));
        let rax = (&*ptr::addr_of!(VMCB)).guest_rax();
        let index = EXIT_HISTORY_NEXT;
        ptr::addr_of_mut!(EXIT_HISTORY).cast::<[u64; 6]>().add(index).write([
            before.rip,
            before.code,
            before.info1,
            before.info2,
            frame.rcx,
            (rax as u32 as u64) | ((frame.rdx as u32 as u64) << 32),
        ]);
        EXIT_HISTORY_NEXT = (index + 1) % 5;
        EXIT_HISTORY_COUNT = (EXIT_HISTORY_COUNT + 1).min(5);
    }

    // The inner body's RAII route guards must be gone before terminal work.
    let resume = unsafe { dispatch_body(context) };
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    // Every terminal return needs evidence, including an unforeseen callee
    // refusal. Route guards have unwound before publishing the stop/barrier.
    if !unsafe { terminal_requested(state) } {
        record_unexplained_stop(state, resume, before);
    }
    if unsafe { terminal_requested(state) } || (!resume && state.stopped_valid) {
        unsafe { terminal_finish(state) };
        return false;
    }
    if resume && before.code != 0x77 && (before.code != 0x72 || state.exits & 0xff == 1) {
        let vmcb = unsafe { &*ptr::addr_of!(VMCB) };
        unsafe {
            diagnostic_record(
                2,
                false,
                [
                    vmcb.guest_rip(),
                    before.code,
                    before.info1,
                    before.info2,
                    vmcb.guest_cr3(),
                    state.exits,
                ],
                0,
            );
        }
    }
    resume
}

pub(super) const fn exit_order(code: u64) -> ExitOrder {
    match code {
        // 0x61 (physical NMI) is an asynchronous event at an instruction
        // boundary, like INTR/AVIC: its V_NMI re-presentation is complete
        // before a later queued INIT, so handle it first.
        0x60 | 0x61 | 0x401 | 0x402 => ExitOrder::TrapThenStartup,
        _ => ExitOrder::StartupThenExit,
    }
}

/// Run an exit's handler and the startup service in `order`. The service
/// returns `Some(resume)` when it decided the exit (a changed guest or a
/// stop) and `None` when there was nothing to service.
pub(super) fn sequence_exit<C>(
    context: &mut C,
    order: ExitOrder,
    handle: impl FnOnce(&mut C) -> bool,
    service: impl FnOnce(&mut C) -> Option<bool>,
) -> bool {
    match order {
        ExitOrder::TrapThenStartup => handle(context) && service(context).unwrap_or(true),
        ExitOrder::StartupThenExit => match service(context) {
            Some(resume) => resume,
            None => handle(context),
        },
    }
}

/// Contention performed no guest/register/queue/hardware commit. Re-enter at
/// the unchanged faulting instruction, bounded to1024 attempts; do not turn a
/// normal concurrent AP mode transition into a falsely completed instruction.
pub(super) fn retry_routing(state: &mut State, vmcb: &Vmcb) -> bool {
    state.routing_retries = state.routing_retries.saturating_add(1);
    if state.routing_retries <= 1024 {
        return true;
    }
    let exit = vmcb.exit_snapshot();
    stop(state, exit.code, exit.rip, 0xf107, state.routing_retries as u64)
}

pub(super) fn check_exit_event(state: &mut State, vmcb: &mut Vmcb) -> bool {
    let code = vmcb.exit_snapshot().code;
    // APM2 15.14.3: shutdown leaves saved guest state undefined. Invalid entry
    // likewise cannot establish event delivery. Neither authorizes a retry.
    if matches!(code, 0x7f | u64::MAX) {
        return stop(state, code, 0, 0xf110, 0);
    }
    let interrupted = u64::from_le_bytes(vmcb.bytes()[0x088..0x090].try_into().unwrap());
    if state.pending_fault {
        if vmcb.clear_event_injection_after_exit().is_err() {
            let exit = vmcb.exit_snapshot();
            return stop(state, exit.code, exit.rip, 0xf10c, interrupted);
        }
        state.pending_fault = false;
    }
    // APM2 rev3.44 15.7.2-3 p509-511 / 15.20 p531: EXITINTINFO.V means the
    // guest was delivering an event through the IDT when this intercept fired
    // and delivery did not finish. A bare NPF/INTR/NMI/AVIC retry would lose
    // an acknowledged interrupt, so complete delivery by re-injecting the
    // recorded event through EVENTINJ (`Vmcb::reinject_interrupted_delivery`).
    // This runs before the exit's own handler; the exits that carry
    // EXITINTINFO.V here (0x60 INTR, 0x61 NMI, 0x401/0x402 AVIC, an interrupted
    // NPF) do not themselves write EVENTINJ, so re-injection is not clobbered.
    // A physical INIT does not reset this guest (it is redirected to #SX), so
    // no interrupted event is discarded here. TYPE 4 software interrupts and
    // reserved types stay terminal (15.20 p531-532 needs nRIP emulation this
    // path does not implement); a conflicting queued event stays terminal too.
    if interrupted & (1 << 31) != 0 {
        let exit = vmcb.exit_snapshot();
        return match vmcb.reinject_interrupted_delivery() {
            ReinjectOutcome::Reinjected { .. } | ReinjectOutcome::NoEvent => true,
            ReinjectOutcome::Unsupported { interrupted } => {
                stop(state, exit.code, exit.rip, 0xf10f, interrupted)
            }
            ReinjectOutcome::Conflict => stop(state, exit.code, exit.rip, 0xf112, interrupted),
        };
    }
    true
}

pub(super) fn record_unexplained_stop(
    state: &mut State,
    resume: bool,
    exit: crate::svm::exit::ExitSnapshot,
) {
    if !resume && !state.stopped_valid {
        stop(state, exit.code, exit.rip, 0xf10e, exit.info1);
    }
}

unsafe fn dispatch_body(context: *mut BridgeContext) -> bool {
    // The outer dispatcher validated context without dereferencing it.
    debug_assert!(context == ptr::addr_of_mut!(CONTEXT));
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    let frame = unsafe { &mut *ptr::addr_of_mut!(FRAME) };
    state.exits = state.exits.saturating_add(1);
    if !state.armed {
        // F10Bh without detail: an exit before arm. With the startup-route
        // record retired, `stop_words` exports it as an unhandled exit.
        let exit = vmcb.exit_snapshot();
        return stop(state, exit.code, exit.rip, 0xf10b, 0);
    }
    if state.avic.as_ref().is_none_or(|profile| vmcb.validate_native_x2avic(profile).is_err()) {
        let exit = vmcb.exit_snapshot();
        let (tag, value) = terminal::profile_mismatch_at_entry(vmcb.virtual_interrupt_control());
        return stop(state, exit.code, exit.rip, tag, value);
    }
    let observed = vmcb.exit_snapshot();
    // Every unusual exit and MSR boundary; common CPUID/PAUSE samples are
    // bounded to avoid making diagnostic PCI traffic dominate guest execution.
    if !matches!(observed.code, 0x72 | 0x77) || state.exits & 0xff == 1 {
        unsafe {
            diagnostic_record(
                1,
                false,
                [
                    observed.rip,
                    observed.code,
                    observed.info1,
                    observed.info2,
                    vmcb.guest_cr3(),
                    state.exits,
                ],
                if observed.code == 0x7c { frame.rcx as u32 } else { 0 },
            );
        }
    }
    // Audited assembly has returned from VMRUN on this CPU. Consume its old
    // flush before INIT, EFER or any other dispatcher mutation can re-arm it.
    // An invalid entry does not establish that the requested flush occurred.
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    let mut nmi_drained = false;
    if state.startup_owned {
        #[cfg(feature = "resident-runtime-test")]
        let irq_witness = unsafe {
            (
                read_native_apic(apic::TPR),
                read_native_apic(apic::IRR + 7 * 16),
                read_native_apic(apic::ISR + 7 * 16),
            )
        };
        let Some(acknowledged) = (unsafe { acknowledge_init() }) else {
            return stop(state, vmcb.exit_snapshot().code, vmcb.guest_rip(), 0xf102, 0);
        };
        // A physical NMI held pending in a host GIF window (including the one
        // acknowledge_init just opened, and the one still pending after a
        // VMEXIT_NMI) is taken by the host vector-2 gate, which sets the NMI
        // flag. Re-present it to the guest as V_NMI before the next VMRUN
        // (15.21.10 p536), so a physical NMI never re-fires on entry.
        nmi_drained = unsafe { route_physical_nmi_to_guest(state, vmcb) };
        if vmcb.exit_snapshot().code == 0x63 && acknowledged == 0 {
            return stop(state, 0x63, vmcb.guest_rip(), 0xf105, 0);
        }
        if vmcb.exit_snapshot().code == 0x63 {
            state.intr = state.intr.saturating_add(1);
        }
        // Broadcast notifications also reach unrelated CPUs. Keep their actual
        // wake count, but emit command evidence only on the queued target so
        // simultaneous empty wakes cannot interleave diagnostic port writes.
        if vmcb.exit_snapshot().code == 0x63
            && (unsafe { mailboxes(state.count) })[state.slot].peek().is_some()
        {
            debug(b"resident-physical-init cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b" rip=");
            hex(vmcb.guest_rip());
            debug(b" count=");
            hex(state.intr);
            debug(b" sx-acks=");
            hex(svmvisor_resident_init_acks.load(Ordering::Acquire));
            #[cfg(feature = "resident-runtime-test")]
            {
                debug(b" tpr=");
                hex(irq_witness.0);
                debug(b" irr-f1=");
                hex(irq_witness.1 & (1 << 17));
                debug(b" isr-f1=");
                hex(irq_witness.2 & (1 << 17));
                // Evidence only: asynchronous device arrivals may legitimately
                // change IRR. The controlled fixture checks its own stable case.
                debug(b" tpr-after=");
                hex(unsafe { read_native_apic(apic::TPR) });
                debug(b" irr-f1-after=");
                hex(unsafe { read_native_apic(apic::IRR + 7 * 16) } & (1 << 17));
                debug(b" isr-f1-after=");
                hex(unsafe { read_native_apic(apic::ISR + 7 * 16) } & (1 << 17));
            }
            debug(b"\n");
        }
    }
    // After the only host GIF window before this exit's handler (above): a
    // VMEXIT_NMI whose pending NMI that window did not take would exit again
    // at the next VMRUN.
    if nmi_drain_stalled(&mut state.nmi_drain_misses, observed.code, nmi_drained) {
        return stop(
            state,
            observed.code,
            observed.rip,
            NMI_DRAIN_STALL,
            u64::from(state.nmi_drain_misses),
        );
    }
    if !check_exit_event(state, vmcb) {
        return false;
    }
    let exit = vmcb.exit_snapshot();
    let Some(ack) = state.ack.as_mut() else {
        return stop(state, exit.code, exit.rip, 0xf10d, 0);
    };
    if !ack.acknowledged() {
        // A physical source can arrive before the bootstrap VMMCALL. Capture
        // it without falsely treating that asynchronous exit as a failed
        // guest ACK; no startup command is serviced before the ACK.
        if exit.code == 0x60 {
            return unsafe { capture_physical_irq(state, vmcb) };
        }
        // Likewise a physical NMI held pending while arm ran with GIF=0
        // (Table 15-10 p530) exits at the first VMRUN (Table 15-13 p536). The
        // window above drained it and set V_NMI; the drain watchdog ran.
        if exit.code == 0x61 {
            return true;
        }
        if ack.acknowledge(vmcb, frame).is_ok() {
            if state.startup_owned {
                (unsafe { mailboxes(state.count) })[state.slot].mark_running();
                if terminal_enabled(state) {
                    // Separate monotonic initial-ACK mask, not mutable guest
                    // startup readiness. Last initial guest ACK opens export.
                    unsafe { terminal_control() }.initial_ack(state.slot, state.count);
                    unsafe {
                        diagnostic_record(
                            9,
                            false,
                            [
                                vmcb.guest_rip(),
                                vmcb.guest_cr3(),
                                state.slot as u64,
                                state.count as u64,
                                0,
                                0,
                            ],
                            0,
                        );
                    }
                }
            }
            debug(b"resident-ack\n");
            return true;
        }
        return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
    }
    let mut context = ExitContext { state, vmcb, frame, exit };
    sequence_exit(
        &mut context,
        exit_order(exit.code),
        // SAFETY: this CPU's armed dispatcher with its guest stopped and
        // IF/GIF clear, the contract of every exit handler below.
        |context| unsafe { handle_exit(context) },
        |context| unsafe { startup_service(context) },
    )
}

/// `service_startup` for one exit, with its stop fallback.
/// # Safety
/// As `service_startup`.
unsafe fn startup_service(context: &mut ExitContext<'_>) -> Option<bool> {
    let ExitContext { state, vmcb, frame, exit } = context;
    let (state, vmcb, frame, exit) = (&mut **state, &mut **vmcb, &mut **frame, *exit);
    if !state.startup_owned {
        return None;
    }
    match unsafe { service_startup(state, vmcb, frame) } {
        Some(true) => Some(true),
        Some(false) if unsafe { terminal_requested(state) } => Some(false),
        Some(false) if state.stopped_valid => Some(false),
        Some(false) => Some(stop(state, exit.code, exit.rip, 0xf103, 0)),
        None => None,
    }
}

/// The acknowledged guest's exit handlers.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
unsafe fn handle_exit(context: &mut ExitContext<'_>) -> bool {
    let ExitContext { state, vmcb, frame, exit } = context;
    let (state, vmcb, frame, exit) = (&mut **state, &mut **vmcb, &mut **frame, *exit);
    #[cfg(feature = "resident-runtime-test")]
    if exit.code == 0x400 && state.cache_fixture && state.cache_active {
        debug(b"native-cache-fixture-low-npf gpa=");
        hex(exit.info2);
        debug(b" root=");
        hex(vmcb.nested_root());
        debug(b"\n");
    }
    match exit.code {
        0x60 => return unsafe { capture_physical_irq(state, vmcb) },
        0x61 => {
            // Physical NMI intercept under NMI virtualization (Table 15-13
            // p536, 15.21.10 p536): the NMI is still pending after the exit
            // and was drained by acknowledge_init's GIF window (host vector-2
            // gate) before this handler runs. Re-present it to the guest as a
            // virtual NMI so Windows still receives platform NMIs. Setting
            // V_NMI is idempotent with the flag routing that already ran.
            return match state.avic {
                Some(profile) => match vmcb.set_guest_v_nmi_pending(&profile) {
                    Ok(()) => {
                        state.routing_retries = 0;
                        true
                    }
                    Err(_) => stop(state, exit.code, exit.rip, exit.info1, exit.info2),
                },
                None => stop(state, exit.code, exit.rip, X2AvicStop::ProfileMismatch as u64, 0),
            };
        }
        0x401 | 0x402 => return unsafe { handle_avic_exit(state, vmcb, frame) },
        0x77 => {
            // Reenter unchanged: VMRUN replenishes the nonzero PAUSE count,
            // and hardware executes this instruction, including debug state.
            unsafe {
                diagnostic_record(5, false, [exit.rip, vmcb.guest_cr3(), state.exits, 0, 0, 0], 0);
            }
            if crate::svm::vmcb::native_pause_retry_ready(vmcb) {
                return true;
            }
            return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
        }
        0x63 if state.startup_owned => {
            // Actual host #SX(error1) acknowledgment was checked above. A
            // notification is only a wakeup: commands live in the mailbox,
            // and multiple notifications may coalesce without losing commands.
            // `check_exit_event` owns the pending-event state: like every
            // intercept, this one may report an interrupted delivery (15.7.2
            // p509), which it has re-injected, so EVENTINJ.V is not a refusal.
            return true;
        }
        0x72 => {
            // The per-CPU arm observation is retained in this private runtime.
            // APM2 15.7.1 makes hardware nRIP authoritative for instruction
            // intercepts. Select this path before any guest-memory access;
            // invalid nRIP must stop, never fall back to rereading the opcode.
            let hardware_nrip =
                state.capabilities.filter(|caps| caps.optional_features().nrip_save);
            let mut prefixed = None;
            if let Some(caps) = hardware_nrip.as_ref() {
                let next = match exit.resume_candidate(caps) {
                    Ok(next) if next.instruction_bytes() >= 2 => next,
                    _ => return stop(state, exit.code, exit.rip, 0xf001, 0x100),
                };
                if !dispatch::native_cpuid_mode(vmcb, next.address(), state.startup_owned) {
                    return stop(state, exit.code, exit.rip, 0xf001, 0x101);
                }
                if next.instruction_bytes() > 2 {
                    let length = next.instruction_bytes() as usize;
                    let mut reader =
                        match unsafe { GuestReader::new(vmcb, state.startup_owned, state.count) } {
                            Ok(reader) => reader,
                            Err(reason) => {
                                return stop(state, exit.code, exit.rip, 0xf001, reason as u64);
                            }
                        };
                    let bytes = super::fetch::cpuid_instruction(
                        vmcb,
                        reader.width,
                        reader.guest_pat,
                        length,
                        state.startup_owned,
                        |address, bytes| unsafe { reader.read(address, bytes) },
                    );
                    match bytes {
                        Ok(bytes) => prefixed = Some((bytes, length)),
                        Err(error) => {
                            let reason = reader
                                .failure
                                .map_or_else(|| terminal::fetch_failure_code(error), |e| e as u16);
                            if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 {
                                return retry_routing(state, vmcb);
                            }
                            return stop(state, exit.code, exit.rip, 0xf001, reason as u64);
                        }
                    }
                }
            }
            let instruction = if hardware_nrip.is_none() {
                let bytes = match unsafe { cache::fetch(state, vmcb) } {
                    Ok(bytes) => bytes,
                    Err(reason)
                        if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 =>
                    {
                        return retry_routing(state, vmcb);
                    }
                    Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
                };
                Some(bytes)
            } else {
                None
            };
            let leaf = vmcb.guest_rax() as u32;
            let native = __cpuid_count(leaf, frame.rcx as u32);
            let response = [native.eax, native.ebx, native.ecx, native.edx];
            #[cfg(feature = "resident-runtime-test")]
            let response = if leaf == 0x4fff_ca00 {
                match unsafe { cache::fixture_control(state, vmcb, frame.rcx as u32) } {
                    Some(value) => value,
                    None => return stop(state, exit.code, exit.rip, 0xf4ff, frame.rcx),
                }
            } else {
                response
            };
            #[cfg(feature = "resident-runtime-test")]
            let response = if leaf == 0x4fff0000 {
                [0x53564d52, state.exits as u32, state.cpuid as u32, state.msr as u32]
            } else {
                response
            };
            state.cpuid = state.cpuid.saturating_add(1);
            let cpuid_user_disabled =
                vmcb.bytes()[0x4cb] != 0 && unsafe { read_msr(HWCR) } & HWCR_CPUID_FLT_EN != 0;
            let result = if let Some(caps) = hardware_nrip {
                dispatch::handle_native_cpuid_with_nrip(
                    vmcb,
                    frame,
                    &caps,
                    response,
                    state.startup_owned,
                    cpuid_user_disabled,
                    prefixed.as_ref().map(|(bytes, length)| &bytes[..*length]),
                )
            } else if cpuid_user_disabled {
                // The byte fallback has no owned CPUID-fault injection path.
                return stop(state, exit.code, exit.rip, 0xf111, 1 << 35);
            } else if state.startup_owned {
                dispatch::handle_native_startup_cpuid(vmcb, frame, &instruction.unwrap(), response)
            } else {
                dispatch::handle_native_cpuid(vmcb, frame, &instruction.unwrap(), response)
            };
            if result.is_ok() {
                if result == Ok(dispatch::DispatchOutcome::GeneralProtectionPrepared) {
                    state.pending_fault = true;
                }
                state.routing_retries = 0;
                return true;
            }
        }
        0x7b if diagnostics::available() => return unsafe { diagnostics::handle_io(state, vmcb) },
        0x7c => {
            if state.cache_observation.is_some() && crate::svm::cache::owned_msr(frame.rcx as u32) {
                return unsafe { cache::handle(state, vmcb, frame) };
            }
            if frame.rcx as u32 == MMIO_CFG_BASE_ADDR && diagnostics::available() {
                // Dynamic ECAM relocation is not yet an owned instruction path.
                // Keep the actual stopped operands before any native write.
                let requested =
                    (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32);
                return stop(state, exit.code, exit.rip, 0xf202, requested);
            }
            if frame.rcx as u32 == SYS_CFG {
                return unsafe { handle_syscfg(state, vmcb, frame) };
            }
            if frame.rcx as u32 == apic::APIC_BASE
                || (apic::X2APIC_MSR_FIRST..=apic::X2APIC_MSR_LAST).contains(&(frame.rcx as u32))
            {
                return unsafe { handle_avic_msr(state, vmcb, frame) };
            }
            if let Some(resume) = unsafe { handle_mcax_msr(state, vmcb, frame) } {
                return resume;
            }
            // Actual same-CPU MSR exit plus NRIPS owns the decoded instruction
            // length, including prefixes. Route owned MSRs before guest-byte fetch.
            // Other MSRs and non-long64 profiles retain their existing byte owner.
            let hardware_nrip = state.capabilities.filter(|caps| {
                caps.optional_features().nrip_save
                    && vmcb.guest_in_64_bit_code()
                    && vmcb.bytes()[0x4cb] == 0
                    && matches!(frame.rcx as u32, EFER | VM_CR)
            });
            if frame.rcx as u32 == VM_CR
                && let Some(caps) = hardware_nrip
            {
                match dispatch::handle_native_vmcr_with_nrip(
                    vmcb,
                    frame,
                    &caps,
                    state.startup_owned,
                ) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) => {
                        let (reason, value) = super::terminal::vmcr_nrip_failure(
                            error,
                            vmcb,
                            dispatch::NATIVE_VM_CR_VALUE,
                        );
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }
            if let (Some(caps), Some(efer)) = (hardware_nrip, state.efer.as_mut()) {
                match dispatch::handle_native_efer_with_nrip(efer, vmcb, frame, &caps) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) => {
                        let (reason, value) =
                            super::terminal::efer_nrip_failure(error, vmcb, efer.logical());
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }
            let instruction = match unsafe {
                fetch_instruction(vmcb, state.startup_owned, state.count)
            } {
                Ok(bytes) => bytes,
                Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => {
                    return retry_routing(state, vmcb);
                }
                Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
            };
            state.msr = state.msr.saturating_add(1);
            if frame.rcx as u32 == VM_CR {
                match dispatch::handle_native_vmcr(vmcb, frame, &instruction, state.startup_owned) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) => {
                        let (reason, value) = super::terminal::vmcr_failure(
                            error,
                            vmcb,
                            dispatch::NATIVE_VM_CR_VALUE,
                            instruction,
                        );
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }

            if let Some(efer) = state.efer.as_mut() {
                match dispatch::handle_native_efer(efer, vmcb, frame, &instruction) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) if frame.rcx as u32 == EFER => {
                        let (reason, value) =
                            super::terminal::efer_failure(error, vmcb, efer.logical(), instruction);
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                    Err(_) => {}
                }
            }
            // A decoded but unsupported/failed MSR exit remains stopped. Keep
            // its operand index in the typed terminal record; VMCB/GPR state
            // and the raw architectural exit fields remain untouched.
            return stop(state, exit.code, exit.rip, 0xf104, frame.rcx);
        }
        0x400 if state.startup_owned => {
            if let Some(endpoint) = unsafe { diagnostics::endpoint() }
                && let Some((base, bytes)) = endpoint.config_aperture()
                && let Ok((start, end)) = crate::memory::npt::identity_protection_range(base, bytes)
                && (start..end).contains(&exit.info2)
            {
                return unsafe { handle_diagnostic_ecam(state, vmcb, base, bytes) };
            }
            return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
        }
        _ => {}
    }
    stop(state, exit.code, exit.rip, exit.info1, exit.info2)
}

unsafe fn handle_diagnostic_ecam(
    state: &mut State,
    vmcb: &mut Vmcb,
    base: u64,
    bytes: u64,
) -> bool {
    let exit = vmcb.exit_snapshot();
    if exit.info1 & 0x1f != 7 {
        return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
    }
    let Some(guard) = (unsafe { terminal_control() }).diagnostic_lock() else {
        return retry_routing(state, vmcb);
    };
    // Remove only our write restriction after noting the first such write.
    // Hardware retries the original instruction; no GPR/RIP/event is emulated.
    unsafe {
        diagnostics::note_config_write(exit.info2, 0, 0, 1);
    }
    let result = crate::memory::npt::restore_identity_write_range(
        unsafe { &mut *ptr::addr_of_mut!(NPT) },
        ptr::addr_of!(NPT) as u64,
        base,
        bytes,
    );
    drop(guard);
    if result.is_err() {
        return stop(state, exit.code, exit.rip, 0xf203, exit.info2);
    }
    vmcb.request_full_tlb_flush();
    state.routing_retries = 0;
    true
}
