//! Intercepted guest MSR accesses (x2APIC/APIC_BASE, MCAX, SYS_CFG), their
//! completion, and this CPU's own RDMSR/WRMSR.

use core::{
    arch::{asm, x86_64::__cpuid_count},
    ptr,
};

use crate::{
    arch::x86_64::{
        apic::{self, HostX2Apic},
        msr::{HWCR, HWCR_MC_STATUS_WR_EN, SYS_CFG},
        registers::GuestRegisters,
    },
    host::resident::{
        runtime::{
            AVIC_BACKING, PHYSICAL_BITS, State,
            debug::diagnostic_record,
            exit::retry_routing,
            guest_reader::fetch_instruction,
            irq::{local_msrpm, sync_eoi_intercept},
            startup::mailboxes,
            stop::stop,
        },
        terminal::{self, IrqSite, X2AvicStop},
    },
    svm::{
        dispatch,
        exit::ResumeCandidate,
        vmcb::Vmcb,
        x2avic::{NativeX2AvicProfile, registers::Emulation, startup::try_lock_routes},
    },
};

/// How one register-owner outcome completes an intercepted RDMSR/WRMSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MsrCompletion {
    /// Continue at nRIP. A RDMSR loads EDX:EAX with `read`; in 64-bit mode
    /// the upper halves of RAX and RDX become zero.
    Complete { read: Option<u64> },
    /// Queue #GP(0) at the unchanged RIP.
    Fault,
    /// Stop with this reason and value; the instruction is not completed.
    Stop(u64, u64),
}

/// Intercepted guest x2APIC (800h-8FFh) and APIC_BASE accesses (D1). The
/// register owner emulates each one (D2-D4, D6); this boundary owns the
/// instruction evidence, the continuation and the fault/stop mapping. Every
/// fallible check precedes the emulation, whose only fallible effect (a level
/// completion after a software EOI) is terminal.
pub(super) unsafe fn handle_avic_msr(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> bool {
    use crate::svm::exit::MsrInstruction;
    let exit = vmcb.exit_snapshot();
    let boundary = X2AvicStop::MsrBoundary as u64;
    let Some(profile) = state.avic.filter(|_| state.guest_apic.is_some()) else {
        return stop(state, exit.code, exit.rip, boundary, 0);
    };
    if vmcb.validate_native_x2avic(&profile).is_err()
        || vmcb.validate_external_interrupt_conflicts().is_err()
    {
        return stop(state, exit.code, exit.rip, boundary, 1);
    }
    // APM2 15.7.1 p509: MSR intercepts save nRIP. Guest code outside 64-bit
    // mode (an AP startup trampoline) keeps the byte-owned continuation of
    // the startup instruction fetch.
    let caps = state
        .capabilities
        .filter(|caps| caps.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_none() {
        match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
            Ok(bytes) => Some(bytes),
            Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => {
                return retry_routing(state, vmcb);
            }
            Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
        }
    } else {
        None
    };
    let evidence = match caps {
        Some(caps) => match MsrInstruction::hardware(exit, &caps) {
            Ok(evidence) => evidence,
            Err(_) => return stop(state, exit.code, exit.rip, boundary, 2),
        },
        None => MsrInstruction::Bytes(bytes.as_ref().unwrap()),
    };
    let Ok(next) = evidence.continuation(exit) else {
        return stop(state, exit.code, exit.rip, boundary, 2);
    };
    if !dispatch::native_startup_instruction_mode(vmcb, evidence.length())
        || vmcb.guest_rflags() & (1 << 8) != 0
    {
        return stop(state, exit.code, exit.rip, boundary, 3);
    }
    let index = frame.rcx as u32;
    let write = (exit.info1 == 1)
        .then(|| (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32));
    // RDMSR/WRMSR above CPL0 fault before the MSRPM check (15.11 p518); keep
    // that #GP(0) should such an exit ever be observed.
    let outcome = if vmcb.bytes()[0x4cb] != 0 {
        Emulation::GeneralProtection
    } else if index == apic::msr(apic::EOI)
        && write.is_some()
        && state.irq.take_eoi_replay(exit.rip)
        && write == Some(0)
    {
        // The re-execution of an EOI write that a level-EOI AVIC_NOACCEL
        // exit already completed (`irq::level_eoi_exit`): no second EOI.
        Emulation::Written
    } else {
        let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
        // SAFETY: armed dispatcher on its own CPU with IF/GIF clear; arm
        // admitted this CPU's enabled x2APIC, and only this runtime uses it.
        let mut host = unsafe { HostX2Apic::new() };
        match state.guest_apic.as_mut() {
            Some(guest) => guest.emulate(index, write, backing, &mut state.irq, &mut host),
            None => return stop(state, exit.code, exit.rip, boundary, 0),
        }
    };
    let completion = msr_completion(outcome, index, write.is_some());
    match apply_msr_completion(vmcb, frame, &profile, completion, next) {
        Ok(fault) => state.pending_fault |= fault,
        Err((tag, value)) => return stop(state, exit.code, exit.rip, tag, value),
    }
    // A software EOI can release the last held level source (D6).
    // SAFETY: this function's contract; no other MSRPM reference is live.
    sync_eoi_intercept(&state.irq, unsafe { local_msrpm() }, vmcb);
    state.routing_retries = 0;
    true
}

/// Intercepted MCAX machine-check MSR (`mcax`): the MSRPM cannot cover
/// C000_2000h-23FFh (APM2 rev3.44 Table 15-8 p518), so every guest access
/// exits and is repeated here on its own CPU. `None`: not an MCAX MSR, or a
/// boundary this path does not own (no NRIPS, outside 64-bit code, TF, a
/// changed profile or a pending event); the caller keeps its F104h stop.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
pub(super) unsafe fn handle_mcax_msr(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> Option<bool> {
    use crate::svm::{
        exit::MsrInstruction,
        mcax::{self, Access},
    };
    let exit = vmcb.exit_snapshot();
    let index = frame.rcx as u32;
    if !(mcax::FIRST..=mcax::LAST).contains(&index) {
        return None;
    }
    let write = (exit.info1 == 1)
        .then(|| (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32));
    let status_writable = unsafe { read_msr(HWCR) } & HWCR_MC_STATUS_WR_EN != 0;
    let access = mcax::plan(index, write, status_writable)?;
    let profile = state.avic?;
    // RDMSR/WRMSR above CPL0 fault before the MSRPM check (15.11 p518).
    let caps = state.capabilities.filter(|caps| {
        caps.optional_features().nrip_save
            && vmcb.guest_in_64_bit_code()
            && vmcb.bytes()[0x4cb] == 0
    })?;
    let evidence = MsrInstruction::hardware(exit, &caps).ok()?;
    let next = evidence.continuation(exit).ok()?;
    if vmcb.validate_native_x2avic(&profile).is_err()
        || vmcb.validate_external_interrupt_conflicts().is_err()
        || !dispatch::native_startup_instruction_mode(vmcb, evidence.length())
        || vmcb.guest_rflags() & (1 << 8) != 0
    {
        return None;
    }
    let completion = match access {
        // PPR57896 rev3.00 p300: unimplemented and unused registers in this
        // space are RAZ/WRIG, so neither host access can fault.
        Access::Read => MsrCompletion::Complete { read: Some(unsafe { read_msr(index) }) },
        Access::Write(value) => {
            unsafe {
                write_msr(index, value);
            }
            MsrCompletion::Complete { read: None }
        }
        Access::ReadZeroIgnoreWrite => {
            MsrCompletion::Complete { read: write.is_none().then_some(0) }
        }
        Access::GeneralProtection => MsrCompletion::Fault,
    };
    Some(match apply_msr_completion(vmcb, frame, &profile, completion, next) {
        Ok(fault) => {
            state.pending_fault |= fault;
            state.msr = state.msr.saturating_add(1);
            state.routing_retries = 0;
            true
        }
        Err((tag, value)) => stop(state, exit.code, exit.rip, tag, value),
    })
}

/// Apply one completion to the stopped guest. A completed RDMSR loads
/// EDX:EAX (in 64-bit mode the upper halves of RAX and RDX become zero), and
/// every completed access continues at `next` with the interrupt shadow and
/// RF consumed. A fault queues #GP(0) at the unchanged RIP (`Ok(true)`).
/// `Err` is a stop reason and value; the guest is then unchanged.
pub(super) fn apply_msr_completion(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    profile: &NativeX2AvicProfile,
    completion: MsrCompletion,
    next: ResumeCandidate,
) -> Result<bool, (u64, u64)> {
    match completion {
        MsrCompletion::Complete { read } => {
            let rax = match read {
                Some(value) => {
                    frame.rdx = value >> 32;
                    value & 0xffff_ffff
                }
                None => vmcb.guest_rax(),
            };
            vmcb.commit_emulated_instruction(rax, next);
            vmcb.complete_native_instruction_state();
            Ok(false)
        }
        MsrCompletion::Fault => match vmcb.queue_native_x2avic_general_protection(profile) {
            Ok(()) => Ok(true),
            Err(_) => Err((X2AvicStop::MsrBoundary as u64, 4)),
        },
        MsrCompletion::Stop(tag, value) => Err((tag, value)),
    }
}

pub(super) fn msr_completion(outcome: Emulation, index: u32, write: bool) -> MsrCompletion {
    match outcome {
        Emulation::Read(value) => MsrCompletion::Complete { read: Some(value) },
        Emulation::Written => MsrCompletion::Complete { read: None },
        Emulation::GeneralProtection => MsrCompletion::Fault,
        Emulation::Refused { reason, value } => {
            let (tag, value) = terminal::register_refusal(reason, index, write, value);
            MsrCompletion::Stop(tag, value)
        }
        Emulation::EoiFailed(error) => {
            let (tag, value) = terminal::irq_failure(IrqSite::SoftwareEoi, error);
            MsrCompletion::Stop(tag, value)
        }
    }
}

/// Complete the reviewed fixed-MTRR control transaction on the owning CPU.
/// Runtime preparation retains every host object above1MiB. Shared routing
/// lock excludes low-RAM readers and other core-shared SYS_CFG writes.
pub(super) unsafe fn handle_syscfg(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &GuestRegisters,
) -> bool {
    use crate::svm::syscfg::{self, SyscfgInstruction, SyscfgPreparation};
    let exit = vmcb.exit_snapshot();
    let caps = state
        .capabilities
        .filter(|c| c.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_some() {
        None
    } else {
        match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
            Ok(bytes) => Some(bytes),
            Err(r) if r == terminal::FetchReadFailure::MemoryControlBusy as u16 => {
                return retry_routing(state, vmcb);
            }
            Err(r) => return stop(state, exit.code, exit.rip, 0xf001, r as u64),
        }
    };
    let routes = match try_lock_routes(unsafe { mailboxes(state.count) }) {
        Ok(guard) => guard,
        Err(_) => return retry_routing(state, vmcb),
    };
    let evidence = match caps.as_ref() {
        Some(c) => SyscfgInstruction::Hardware(c),
        None => SyscfgInstruction::Bytes(bytes.as_ref().unwrap()),
    };
    let result = syscfg::prepare(
        vmcb,
        frame,
        evidence,
        state.startup_owned,
        __cpuid_count(1, 0).eax,
        unsafe { PHYSICAL_BITS },
        || unsafe { read_msr(SYS_CFG) },
    );
    let failure = match result {
        Ok(SyscfgPreparation::GeneralProtectionPrepared) => {
            state.pending_fault = true;
            return true;
        }
        Ok(SyscfgPreparation::Write(prepared)) => {
            let current = prepared.current();
            let requested = prepared.requested();
            let delta = prepared.delta();
            unsafe {
                diagnostic_record(8, false, [exit.rip, current, requested, current, delta, 0], 1);
            }
            if let Some(requested) = prepared.write_value() {
                unsafe {
                    write_msr(SYS_CFG, requested);
                }
                let observed = unsafe { read_msr(SYS_CFG) };
                if observed != requested {
                    let (tag, value) = terminal::syscfg_operands(0x85, true, requested, observed);
                    drop(routes);
                    return stop(state, exit.code, exit.rip, tag, value);
                }
            }
            prepared.commit();
            state.msr = state.msr.saturating_add(1);
            unsafe {
                diagnostic_record(8, false, [exit.rip, current, requested, requested, delta, 0], 2);
            }
            state.routing_retries = 0;
            return true;
        }
        Err(error) => error,
    };
    let (tag, value) = terminal::syscfg_failure(failure, vmcb, bytes);
    drop(routes);
    stop(state, exit.code, exit.rip, tag, value)
}

/// Owning native CPU with the selected MSR enumerated/admitted: x2APIC
/// registers require the fixed enabled bus; VM_CR requires admitted AMD SVM.
/// APM2 15.30.1/16.11 and applicable PPR57896 register definitions.
pub(super) unsafe fn read_msr(index: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr",in("ecx")index,out("eax")low,out("edx")high,
        options(nomem,nostack,preserves_flags));
    }
    low as u64 | ((high as u64) << 32)
}

/// Same owning CPU; an admitted non-APIC MSR value checked by its owner
/// (VM_CR.R_INIT preserving every other bit, SYS_CFG, HWCR, cache replay, a
/// guest MCAX write that `mcax::plan` admitted) or
/// the fixed private INIT notification ICR. Guest x2APIC state reaches the
/// physical LAPIC only through `HostX2Apic`. APM2 rev3.44 15.30.1/16.13 and
/// PPR57896 p215. No MSR may fault.
pub(super) unsafe fn write_msr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32,
            in("edx") (value >> 32) as u32, options(nostack, preserves_flags));
    }
}
