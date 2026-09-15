//! Single-owner clock service, checked HLT wakeup and physical INTR boundaries.
//!
//! This bounded fixture converts monotonically sampled source timestamps using
//! an explicitly admitted rational rate. It does not own a physical clock or
//! host interrupt source. The caller samples each stopped dispatch, polls while
//! parked, or supplies an actual source-attributed physical INTR exit from a
//! running guest, with its own finite budget and exclusive CPU ownership.
//! No host APIC or firmware is accessed by this core.
//!
//! Clock service is a separate commit boundary from instruction completion:
//! service charges the old timer configuration; a refused instruction preserves
//! that post-service state. Never retry service by inventing elapsed ticks.
//! APM2 rev3.44 6.5, 15.9/Table15-7, 15.13.1, 15.21.1–5, 16.4.1; APM3 rev3.37
//! HLT p388 and STI p477. PPR57896 rev3.00 p43 describes a different physical
//! rate (2xCLKIN), not evidence for this admitted synthetic conversion.

use super::{
    events::ExternalInterruptError,
    exit::{ResumeCandidate, ResumeError},
    local_apic::{Error as ApicError, TickOutcome},
    vmcb::{EventIntercept, InstructionIntercept, Vmcb},
    x2apic::{self, ApicMode, FixtureApic, MsrError},
    xapic::{self, FixtureMmioMapping, MmioError},
};
use crate::arch::x86_64::registers::GuestRegisters;

/// Absolute timestamp from one admitted, exclusively owned CPU. The CPU tag is
/// supplied by the sampler's ownership evidence, not guest TSC_AUX. Wraparound,
/// migration and backward adjustments are unsupported discontinuities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSample {
    pub ticks: u64,
    pub cpu: u32,
}

/// `timer_ticks / source_ticks` pre-divider timer ticks per source timestamp tick.
/// Nonzero integer components admit a synthetic rate, never hardware calibration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockRate {
    timer_ticks: u32,
    source_ticks: u32,
}

impl ClockRate {
    pub const fn new(timer_ticks: u32, source_ticks: u32) -> Result<Self, ScheduleError> {
        if timer_ticks == 0 || source_ticks == 0 {
            return Err(ScheduleError::InvalidRate);
        }
        Ok(Self {
            timer_ticks,
            source_ticks,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ScheduleError {
    InvalidRate,
    WrongCpu,
    ClockBackwards,
    ClockOverflow,
    DeadlineOverflow,
    Armed,
    AlreadyHalted,
    NotHalted,
    HaltedStateChanged,
    UnsupportedGuestMode,
    UnsupportedPrivilege,
    UnsupportedDebugState,
    HltInterceptMissing,
    NotPhysicalInterruptExit,
    PhysicalInterruptControlsMissing,
    Continuation(ResumeError),
    PendingState(ExternalInterruptError),
    Controller(ApicError),
    Msr(MsrError),
    Mmio(MmioError),
    Ipi(super::ipi::IpiError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    Parked,
    /// V_IRQ is armed and RIP is after the completed HLT. The caller must
    /// actually enter and observe the next exit before dispatching guest EOI.
    Ready {
        vector: u8,
    },
}

/// Clock/virtual-controller results at a caller-attributed physical INTR exit.
/// Neither field reports physical-source acknowledgement or handler completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreemptionOutcome {
    /// Previously armed V_IRQ observed consumed at this real exit.
    pub consumed: Option<u8>,
    /// New V_IRQ armed at this boundary, not an already retained flight.
    pub armed: Option<u8>,
}

/// Disposition at one actual entry/exit boundary, before instruction dispatch.
/// Neither outcome means handler completion or EOI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettlementOutcome {
    pub consumed: Option<u8>,
    /// Undispatched V_IRQ withdrawn; the same request remains in controller IRR.
    pub deferred: Option<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct Halted {
    rip: u64,
    flags: u64,
    next: ResumeCandidate,
}

/// Sole APIC/clock/halted-state owner; deliberately neither Copy nor Clone.
/// No mutable APIC escape exists. All bus adapters remain the original shared
/// implementations, reached through this owner's instruction boundary.
/// The caller must preserve the entire stopped VMCB, saved frame, code and
/// mappings while parked; the local RIP/flags checks do not prove full identity.
#[derive(Debug, PartialEq, Eq)]
pub struct ScheduledApic {
    apic: FixtureApic,
    rate: ClockRate,
    last: ClockSample,
    phase: u32,
    halted: Option<Halted>,
}

impl ScheduledApic {
    /// Transfer a stopped, unarmed APIC and establish a fresh source-clock phase.
    /// The caller admits the source's continuity and same-CPU sampling discipline.
    /// An already running timer retains its APIC divider phase; phase zero here
    /// establishes only the rational source conversion epoch.
    pub fn admit(
        apic: FixtureApic,
        rate: ClockRate,
        initial: ClockSample,
    ) -> Result<Self, (ScheduleError, FixtureApic)> {
        if apic.controller().delivery_armed() {
            return Err((ScheduleError::Armed, apic));
        }
        Ok(Self {
            apic,
            rate,
            last: initial,
            phase: 0,
            halted: None,
        })
    }

    pub fn apic(&self) -> &FixtureApic {
        &self.apic
    }
    pub fn is_halted(&self) -> bool {
        self.halted.is_some()
    }
    pub fn last_sample(&self) -> ClockSample {
        self.last
    }
    /// Numerator remainder, in units of `1 / rate.source_ticks` timer ticks.
    pub fn source_phase(&self) -> u32 {
        self.phase
    }

    /// Derive the next interrupt-producing timer deadline from the current sole
    /// owner on every query, so cancellation, masking and mode writes cannot leave
    /// a stale callback. Masked countdowns still advance during clock service.
    /// A deadline outside the nonwrapping u64 source domain is an explicit query
    /// refusal; the clock/timer is unchanged and no wrapped deadline is returned.
    pub fn deadline(&self) -> Result<Option<u64>, ScheduleError> {
        let controller = self.apic.controller();
        if self.apic.mode() == ApicMode::Disabled
            || !controller.software_enabled()
            || controller.timer_lvt() & (1 << 16) != 0
        {
            return Ok(None);
        }
        let Some(remaining) = controller.timer_source_remaining() else {
            return Ok(None);
        };
        let needed = remaining as u128 * self.rate.source_ticks as u128 - self.phase as u128;
        let source_delta = needed.div_ceil(self.rate.timer_ticks as u128);
        let deadline = self.last.ticks as u128 + source_delta;
        u64::try_from(deadline)
            .map(Some)
            .map_err(|_| ScheduleError::DeadlineOverflow)
    }

    fn conversion(&self, sample: ClockSample) -> Result<(u64, u32), ScheduleError> {
        if sample.cpu != self.last.cpu {
            return Err(ScheduleError::WrongCpu);
        }
        let elapsed = sample
            .ticks
            .checked_sub(self.last.ticks)
            .ok_or(ScheduleError::ClockBackwards)?;
        let numerator = elapsed as u128 * self.rate.timer_ticks as u128 + self.phase as u128;
        let ticks = u64::try_from(numerator / self.rate.source_ticks as u128)
            .map_err(|_| ScheduleError::ClockOverflow)?;
        Ok((ticks, (numerator % self.rate.source_ticks as u128) as u32))
    }

    fn stopped(&self, vmcb: &Vmcb) -> Result<(), ScheduleError> {
        if vmcb.exit_snapshot().code == u64::MAX {
            return Err(ScheduleError::PendingState(
                ExternalInterruptError::InvalidEntry,
            ));
        }
        vmcb.validate_external_interrupt_conflicts()
            .map_err(ScheduleError::PendingState)?;
        vmcb.validate_virtual_interrupt_controls()
            .map_err(ScheduleError::PendingState)?;
        if vmcb.virtual_interrupt_control() & 0xf
            != (self.apic.controller().task_priority() >> 4) as u64
        {
            return Err(ScheduleError::Controller(ApicError::TaskPriorityMismatch));
        }
        if self.apic.controller().delivery_armed() {
            return Err(ScheduleError::Armed);
        }
        if vmcb.virtual_interrupt_control() & (1 << 8) != 0 {
            return Err(ScheduleError::PendingState(
                ExternalInterruptError::PendingVirtualInterrupt,
            ));
        }
        Ok(())
    }

    /// Atomically account for elapsed source time under the OLD configuration.
    /// On a clock/event/control/armed refusal neither timer nor clock is changed.
    /// In particular, armed refusals never discard elapsed time: the unchanged
    /// epoch is charged after a real consumption is observed. Disabled APIC_BASE
    /// retains/freeze-counts the timer and conversion phase, updating only epoch.
    pub fn service(
        &mut self,
        vmcb: &Vmcb,
        sample: ClockSample,
    ) -> Result<TickOutcome, ScheduleError> {
        let converted = self.conversion(sample)?;
        self.stopped(vmcb)?;
        self.commit_time(sample, converted)
    }

    fn commit_time(
        &mut self,
        sample: ClockSample,
        converted: (u64, u32),
    ) -> Result<TickOutcome, ScheduleError> {
        let outcome = if self.apic.mode() == ApicMode::Disabled {
            TickOutcome::Stopped
        } else {
            // The caller preflights idle ownership. Advancing a valid admitted
            // timer has no remaining fallible arithmetic or vector validation.
            let outcome = self
                .apic
                .advance_timer(converted.0)
                .map_err(|error| match error {
                    x2apic::QueueError::Controller(error) => ScheduleError::Controller(error),
                    x2apic::QueueError::Disabled => unreachable!("mode checked above"),
                })?;
            self.phase = converted.1;
            outcome
        };
        self.last = sample;
        Ok(outcome)
    }

    fn instruction_boundary(&self, vmcb: &Vmcb) -> Result<(), ScheduleError> {
        if self.halted.is_some() {
            return Err(ScheduleError::AlreadyHalted);
        }
        self.stopped(vmcb)
    }

    /// Call after service at this stopped dispatch. A refusal preserves the
    /// post-service APIC/VMCB/frame. Initial-count writes reset both conversion
    /// and divider phases at that sampled boundary, including cancellation.
    pub fn handle_msr(
        &mut self,
        vmcb: &mut Vmcb,
        frame: &mut GuestRegisters,
        instruction: &[u8],
    ) -> Result<(), ScheduleError> {
        self.handle_msr_inner(vmcb, frame, instruction, None)
    }

    /// Checked ICR publication through the same instruction and timer boundary.
    /// Every accepted publication requires the caller's unconditional host kick.
    pub fn handle_msr_with_mailbox(
        &mut self,
        vmcb: &mut Vmcb,
        frame: &mut GuestRegisters,
        instruction: &[u8],
        target: &mut super::ipi::MailboxTarget<'_>,
    ) -> Result<(), ScheduleError> {
        self.handle_msr_inner(vmcb, frame, instruction, Some(target))
    }

    fn handle_msr_inner(
        &mut self,
        vmcb: &mut Vmcb,
        frame: &mut GuestRegisters,
        instruction: &[u8],
        target: Option<&mut dyn super::ipi::IpiRoute>,
    ) -> Result<(), ScheduleError> {
        self.instruction_boundary(vmcb)?;
        let resets_phase = vmcb.exit_snapshot().info1 == 1 && frame.rcx as u32 == 0x838;
        x2apic::handle_fixture_msr_inner(&mut self.apic, vmcb, frame, instruction, target)
            .map_err(ScheduleError::Msr)?;
        if resets_phase {
            self.phase = 0;
        }
        Ok(())
    }

    pub fn handle_mmio(
        &mut self,
        vmcb: &mut Vmcb,
        frame: &GuestRegisters,
        instruction: &[u8],
        mapping: &FixtureMmioMapping,
    ) -> Result<(), ScheduleError> {
        self.handle_mmio_inner(vmcb, frame, instruction, mapping, None)
    }

    pub fn handle_mmio_with_mailbox(
        &mut self,
        vmcb: &mut Vmcb,
        frame: &GuestRegisters,
        instruction: &[u8],
        mapping: &FixtureMmioMapping,
        target: &mut super::ipi::MailboxTarget<'_>,
    ) -> Result<(), ScheduleError> {
        self.handle_mmio_inner(vmcb, frame, instruction, mapping, Some(target))
    }

    fn handle_mmio_inner(
        &mut self,
        vmcb: &mut Vmcb,
        frame: &GuestRegisters,
        instruction: &[u8],
        mapping: &FixtureMmioMapping,
        target: Option<&mut dyn super::ipi::IpiRoute>,
    ) -> Result<(), ScheduleError> {
        self.instruction_boundary(vmcb)?;
        let resets_phase = instruction == [0x89, 0x03]
            && vmcb.exit_snapshot().info2 == xapic::FIXTURE_MMIO_BASE + 0x380;
        xapic::handle_fixture_mmio_inner(&mut self.apic, vmcb, frame, instruction, mapping, target)
            .map_err(ScheduleError::Mmio)?;
        if resets_phase {
            self.phase = 0;
        }
        Ok(())
    }

    /// Drain only into this CPU's sole controller, including while HLT is
    /// parked. No HLT completion occurs here: poll_halted owns readiness and
    /// RIP retirement. Preflight failures retain every transport request.
    /// Clock service remains a separate explicit caller-owned boundary.
    pub fn drain_mailbox(
        &mut self,
        vmcb: &Vmcb,
        mailbox: &super::ipi::IpiMailbox,
    ) -> Result<usize, ScheduleError> {
        self.stopped(vmcb)?;
        if self.halted.is_some() {
            self.validate_halted(vmcb)?;
        }
        mailbox
            .drain_into(&mut self.apic, vmcb)
            .map_err(ScheduleError::Ipi)
    }

    /// Observe ONLY after a real entry/exit of this owned VMCB. Clock errors
    /// preflight before observation; consumed delivery is never rolled back.
    /// A still-armed result keeps the old epoch/remainder, retaining all elapsed
    /// time for eventual service. EOI remains a separate guest instruction.
    /// Elapsed expirations are processed AFTER observed consumption at this
    /// stopped boundary and can requeue its vector. No sub-exit arrival/dispatch
    /// timestamp, physical latency, or asynchronous coalescence fidelity is known.
    pub fn observe_after_exit(
        &mut self,
        vmcb: &Vmcb,
        sample: ClockSample,
    ) -> Result<Option<u8>, ScheduleError> {
        if self.halted.is_some() {
            return Err(ScheduleError::AlreadyHalted);
        }
        let converted = self.conversion(sample)?;
        if !self.apic.controller().delivery_armed() {
            self.service(vmcb, sample)?;
            return Ok(None);
        }
        let consumed = self.apic.observe(vmcb).map_err(ScheduleError::Controller)?;
        if consumed.is_some() {
            self.commit_time(sample, converted)?;
        }
        Ok(consumed)
    }

    /// Settle the previous entry before reflecting a fault or emulating a guest
    /// instruction. Unlike observe_after_exit, a still-pending V_IRQ is safely
    /// withdrawn to the existing IRR so it cannot obstruct synchronous EVENTINJ
    /// or APIC register access. A cleared V_IRQ instead transfers IRR to ISR.
    /// APM2 15.7/15.20/15.21.4: valid EXITINTINFO and invalid entry always stop;
    /// this does not recover interrupted delivery or synthesize nested events.
    ///
    /// Caller establishes an actual completed entry/exit and retires any prior
    /// EVENTINJ with clear_event_injection_after_exit BEFORE this call. Never
    /// retire a newly queued injection before entry. After settlement, dispatch
    /// the synchronous exit first, then arm_pending only when no injection is
    /// queued. Clearing EVENTINJ proves neither handler completion nor IRETQ.
    ///
    /// Clock/control refusals preserve VMCB and owner. Settled consumption or
    /// deferral precedes elapsed-time service under the old configuration;
    /// elapsed periodic edges can therefore coalesce in IRR or requeue a vector
    /// now in ISR. No sub-exit arrival order or physical latency is inferred.
    pub fn settle_after_exit(
        &mut self,
        vmcb: &mut Vmcb,
        sample: ClockSample,
    ) -> Result<SettlementOutcome, ScheduleError> {
        if self.halted.is_some() {
            return Err(ScheduleError::AlreadyHalted);
        }
        let converted = self.conversion(sample)?;
        let mut outcome = SettlementOutcome {
            consumed: None,
            deferred: None,
        };
        if self.apic.controller().delivery_armed() {
            outcome.consumed = self.apic.observe(vmcb).map_err(ScheduleError::Controller)?;
            if outcome.consumed.is_none() {
                outcome.deferred = Some(
                    self.apic
                        .defer_after_exit(vmcb)
                        .map_err(ScheduleError::Controller)?,
                );
            }
        } else {
            self.stopped(vmcb)?;
        }
        self.commit_time(sample, converted)?;
        Ok(outcome)
    }

    /// Select an owned IRR request after synchronous dispatch. BASE/SVR/PPR/TPR
    /// gate selection in the sole controller; V_IRQ waits in hardware for guest
    /// IF/GIF/shadow readiness. Thus blocked delivery may deliberately be armed.
    /// EVENTINJ and interrupted delivery refuse: finish fault injection first.
    /// Caller owns the next actual entry/exit and must settle or observe it.
    pub fn arm_pending(&mut self, vmcb: &mut Vmcb) -> Result<Option<u8>, ScheduleError> {
        self.instruction_boundary(vmcb)?;
        self.apic.arm(vmcb).map_err(ScheduleError::Controller)
    }

    /// Service a real physical INTR exit without completing any instruction.
    /// APM2 rev3.44 15.13.1/15.21.1: exit60 leaves the physical interrupt
    /// pending. BEFORE calling, the caller must establish the owned source's
    /// identity and acknowledge it through its host interrupt path, with all
    /// other physical sources excluded. EXITINFO is not a vector receipt.
    /// It also owns the actual same-CPU entry/exit, host IF/GIF/TPR, immutable
    /// guest mappings and complete saved frame. This pure helper proves none
    /// of those facts and never manufactures an exit or a timer callback.
    ///
    /// RIP/RFLAGS/GPRs and shadow are preserved; nRIP/EXITINFO are unused.
    /// Clock/control refusals precede observation, preserving owner and VMCB.
    /// Actual consumed delivery is then accounted before elapsed time, as in
    /// observe_after_exit. A still-armed flight retains its clock epoch.
    /// IF/shadow-blocked new interrupts stay in IRR, allowing clock service
    /// and guest register writes on subsequent exits. APIC priority/SVR/BASE
    /// gates stay in the sole controller. Host-source rearming and a finite
    /// exit/time budget remain the caller's responsibility.
    pub fn handle_preemption(
        &mut self,
        vmcb: &mut Vmcb,
        sample: ClockSample,
    ) -> Result<PreemptionOutcome, ScheduleError> {
        if self.halted.is_some() {
            return Err(ScheduleError::AlreadyHalted);
        }
        if vmcb.exit_snapshot().code != 0x60 {
            return Err(ScheduleError::NotPhysicalInterruptExit);
        }
        if !vmcb.event_intercept(EventIntercept::PhysicalInterrupt)
            || vmcb.virtual_interrupt_control() & (1 << 24) == 0
        {
            return Err(ScheduleError::PhysicalInterruptControlsMissing);
        }
        if !vmcb.guest_in_64_bit_code() {
            return Err(ScheduleError::UnsupportedGuestMode);
        }
        if !crate::address::is_canonical_48(vmcb.guest_rip()) {
            return Err(ScheduleError::Continuation(ResumeError::NonCanonicalRip));
        }
        let consumed = self.observe_after_exit(vmcb, sample)?;
        let armed = if self.apic.controller().delivery_armed()
            || vmcb.guest_rflags() & (1 << 9) == 0
            || vmcb.interrupt_shadow()
        {
            None
        } else {
            self.arm_pending(vmcb)?
        };
        Ok(PreemptionOutcome { consumed, armed })
    }

    fn hlt_mode(vmcb: &Vmcb) -> Result<(), ScheduleError> {
        if !vmcb.guest_in_64_bit_code() {
            return Err(ScheduleError::UnsupportedGuestMode);
        }
        if vmcb.bytes()[0x4cb] != 0 {
            return Err(ScheduleError::UnsupportedPrivilege);
        }
        // APM3 HLT/debug exceptions: this bounded retirement does not implement
        // debug stepping/breakpoints or resume-flag completion semantics.
        let dr7 = u64::from_le_bytes(vmcb.bytes()[0x560..0x568].try_into().unwrap());
        if vmcb.guest_rflags() & ((1 << 8) | (1 << 16)) != 0 || dr7 & 0xff != 0 {
            return Err(ScheduleError::UnsupportedDebugState);
        }
        if !vmcb.instruction_intercept(InstructionIntercept::Hlt) {
            return Err(ScheduleError::HltInterceptMissing);
        }
        Ok(())
    }

    /// Complete one exact installed F4 at a real exit78, after clock service.
    /// Caller owns immutable instruction bytes fetched at this RIP and their
    /// executable mapping/continuation for the session (as for the MSR adapter).
    /// The saved RIP stays at HLT while parked. Consuming the preceding STI
    /// shadow models HLT retirement; flags/GPRs stay unchanged. Only a deliverable
    /// owned maskable interrupt can authorize the after-HLT continuation.
    pub fn park_hlt(&mut self, vmcb: &mut Vmcb, instruction: &[u8]) -> Result<(), ScheduleError> {
        self.instruction_boundary(vmcb)?;
        Self::hlt_mode(vmcb)?;
        let next = vmcb
            .exit_snapshot()
            .hlt_continuation(instruction)
            .map_err(ScheduleError::Continuation)?;
        self.halted = Some(Halted {
            rip: vmcb.guest_rip(),
            flags: vmcb.guest_rflags(),
            next,
        });
        vmcb.complete_hlt_shadow();
        Ok(())
    }

    /// One bounded stopped-guest poll, never an asynchronous host callback.
    /// Caller supplies an actual source sample each time and enforces its finite
    /// polling budget. Exhausting that budget means incomplete execution.
    pub fn poll_halted(
        &mut self,
        vmcb: &mut Vmcb,
        sample: ClockSample,
    ) -> Result<WaitOutcome, ScheduleError> {
        self.validate_halted(vmcb)?;
        self.service(vmcb, sample)?;
        if vmcb.guest_rflags() & (1 << 9) == 0 {
            return Ok(WaitOutcome::Parked);
        }
        // APIC arm enforces BASE/SVR, PPR (including ISR), matching TPR and
        // event/control ownership. VMRUN supplies GIF in this classic profile.
        let Some(vector) = self.apic.arm(vmcb).map_err(ScheduleError::Controller)? else {
            return Ok(WaitOutcome::Parked);
        };
        let halted = self.halted.take().expect("validated halted owner");
        vmcb.commit_emulated_instruction(vmcb.guest_rax(), halted.next);
        Ok(WaitOutcome::Ready { vector })
    }

    fn validate_halted(&self, vmcb: &Vmcb) -> Result<(), ScheduleError> {
        let halted = self.halted.as_ref().ok_or(ScheduleError::NotHalted)?;
        Self::hlt_mode(vmcb)?;
        if vmcb.guest_rip() != halted.rip
            || vmcb.guest_rflags() != halted.flags
            || vmcb.interrupt_shadow()
            || vmcb.exit_snapshot().code != 0x78
        {
            return Err(ScheduleError::HaltedStateChanged);
        }
        Ok(())
    }
}
