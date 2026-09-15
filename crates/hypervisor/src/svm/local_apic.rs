//! Bounded single-vCPU, edge-triggered local interrupt controller.
//!
//! AMD APM vol.2 rev.3.44 16.4.1, 16.6.3–4 and 15.21.4. This is a
//! controller behind both guest register adapters. The stopped owner supplies
//! deterministic timer source ticks before the divider; no physical clock rate
//! or native scheduling is assumed. Exactly one LVT (the timer) is admitted.
//! Scheduling mutations are refused while a request is armed. General CR8 decoding,
//! level interrupts, IOAPIC, NMI and interrupted-delivery recovery require
//! other policies. One owner must retain this object and its VMCB across entries.
//! The separate bounded ICR router queues fixed IPIs through this same owner.
use super::{
    events::{ExternalInterruptError, ExternalInterruptState, PendingExternalInterrupt},
    vmcb::Vmcb,
};

#[derive(Debug, PartialEq, Eq)]
pub struct LocalApic {
    irr: [u64; 4],
    isr: [u64; 4],
    tpr: u8,
    flight: Option<PendingExternalInterrupt>,
    svr: u16,
    timer: Timer,
}

#[derive(Debug, PartialEq, Eq)]
struct Timer {
    lvt: u32,
    initial: u32,
    current: u32,
    divide: u8,
    phase: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Armed,
    NoDelivery,
    TaskPriorityMismatch,
    SoftwareDisabled,
    ReservedRegisterBits,
    ReadOnlyTimerStatus,
    UnsupportedTimerVector,
    TimerRunning,
    Interrupt(ExternalInterruptError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    Counting,
    Stopped,
    MaskedExpiration,
    Queued,
    Coalesced,
}

impl LocalApic {
    /// Admit an enabled bounded controller, not hardware RESET/INIT state.
    /// SVR=1ffh; the only LVT is a masked timer with stopped count/divider 2.
    pub const fn admit_enabled() -> Self {
        Self {
            irr: [0; 4],
            isr: [0; 4],
            tpr: 0,
            flight: None,
            svr: 0x1ff,
            timer: Timer {
                lvt: 1 << 16,
                initial: 0,
                current: 0,
                divide: 0,
                phase: 0,
            },
        }
    }
    pub fn software_enabled(&self) -> bool {
        self.svr & (1 << 8) != 0
    }
    pub fn spurious_vector_register(&self) -> u32 {
        self.svr as u32
    }
    /// APM2 16.3.1/16.4.7: hold accepted interrupts, force every admitted LVT
    /// mask, and do not restore forced masks upon reenable. FCC is retained;
    /// lowest-priority routing and spurious-acknowledgment races are not modeled.
    pub fn write_spurious_vector(&mut self, value: u32) -> Result<(), Error> {
        self.idle()?;
        if value & !0x3ff != 0 {
            return Err(Error::ReservedRegisterBits);
        }
        self.svr = value as u16;
        if !self.software_enabled() {
            self.timer.lvt |= 1 << 16;
        }
        Ok(())
    }
    fn idle(&self) -> Result<(), Error> {
        if self.flight.is_some() {
            Err(Error::Armed)
        } else {
            Ok(())
        }
    }
    fn valid(vector: u8) -> Result<(), Error> {
        PendingExternalInterrupt::new(vector)
            .map(|_| ())
            .map_err(Error::Interrupt)
    }
    fn bit(map: &[u64; 4], vector: u8) -> bool {
        map[vector as usize / 64] & (1u64 << (vector % 64)) != 0
    }
    fn set(map: &mut [u64; 4], vector: u8, value: bool) {
        let bit = 1u64 << (vector % 64);
        if value {
            map[vector as usize / 64] |= bit;
        } else {
            map[vector as usize / 64] &= !bit;
        }
    }
    fn highest(map: &[u64; 4]) -> Option<u8> {
        (0..4)
            .rev()
            .find(|&i| map[i] != 0)
            .map(|i| (i * 64 + 63 - map[i].leading_zeros() as usize) as u8)
    }
    pub fn pending(&self, vector: u8) -> bool {
        Self::bit(&self.irr, vector)
    }
    /// The target of a non-specific EOI; reading does not acknowledge it.
    pub fn eoi_target(&self) -> Option<u8> {
        Self::highest(&self.isr)
    }
    pub fn in_service(&self, vector: u8) -> bool {
        Self::bit(&self.isr, vector)
    }
    pub fn delivery_armed(&self) -> bool {
        self.flight.is_some()
    }
    pub fn task_priority(&self) -> u8 {
        self.tpr
    }
    /// Preserve TPR subclass when its class wins (including equality).
    pub fn processor_priority(&self) -> u8 {
        let service = Self::highest(&self.isr).unwrap_or(0) & 0xf0;
        if self.tpr & 0xf0 >= service {
            self.tpr
        } else {
            service
        }
    }
    pub fn set_task_priority(&mut self, tpr: u8) -> Result<(), Error> {
        self.idle()?;
        self.tpr = tpr;
        Ok(())
    }
    /// Synchronize a guest TPR write with the stopped virtual TPR atomically.
    pub(crate) fn write_guest_tpr(&mut self, vmcb: &mut Vmcb, tpr: u8) -> Result<(), Error> {
        self.idle()?;
        if vmcb.virtual_interrupt_control() & 0xf != (self.tpr >> 4) as u64 {
            return Err(Error::TaskPriorityMismatch);
        }
        vmcb.set_virtual_interrupt_tpr(tpr >> 4)
            .map_err(Error::Interrupt)?;
        self.tpr = tpr;
        Ok(())
    }
    pub(crate) fn pending_word(&self, index: usize) -> u32 {
        (self.irr[index / 2] >> ((index % 2) * 32)) as u32
    }
    pub(crate) fn service_word(&self, index: usize) -> u32 {
        (self.isr[index / 2] >> ((index % 2) * 32)) as u32
    }
    /// Returns false for an already pending edge; pending bits are not counters.
    pub fn queue(&mut self, vector: u8) -> Result<bool, Error> {
        self.idle()?;
        Self::valid(vector)?;
        if !self.software_enabled() {
            return Err(Error::SoftwareDisabled);
        }
        let fresh = !self.pending(vector);
        Self::set(&mut self.irr, vector, true);
        Ok(fresh)
    }
    /// Arm only against the matching stopped guest TPR. PPR is never written
    /// into V_TPR. Hardware remains responsible for IF/shadow/VINTR gating.
    pub fn arm(&mut self, vmcb: &mut Vmcb) -> Result<Option<u8>, Error> {
        self.idle()?;
        if vmcb.virtual_interrupt_control() & 0xf != (self.tpr >> 4) as u64 {
            return Err(Error::TaskPriorityMismatch);
        }
        if !self.software_enabled() {
            return Ok(None);
        }
        let Some(vector) = Self::highest(&self.irr) else {
            return Ok(None);
        };
        if vector >> 4 <= self.processor_priority() >> 4 {
            return Ok(None);
        }
        let mut request = PendingExternalInterrupt::new(vector).map_err(Error::Interrupt)?;
        vmcb.arm_external_interrupt(&mut request)
            .map_err(Error::Interrupt)?;
        self.flight = Some(request);
        Ok(Some(vector))
    }
    /// Call only after a real entry/exit under exclusive CPU/VMCB ownership.
    /// Consume before dispatching any guest EOI. Errors preserve controller state;
    /// interrupted delivery remains a stop requiring a separate recovery policy.
    pub fn observe(&mut self, vmcb: &Vmcb) -> Result<Option<u8>, Error> {
        if vmcb.virtual_interrupt_control() & 0xf != (self.tpr >> 4) as u64 {
            return Err(Error::TaskPriorityMismatch);
        }
        let request = self.flight.as_mut().ok_or(Error::NoDelivery)?;
        let vector = request.vector();
        match vmcb
            .observe_external_interrupt_after_exit(request)
            .map_err(Error::Interrupt)?
        {
            ExternalInterruptState::Consumed => {
                Self::set(&mut self.irr, vector, false);
                Self::set(&mut self.isr, vector, true);
                self.flight = None;
                Ok(Some(vector))
            }
            _ => Ok(None),
        }
    }
    /// Withdraw only a still-pending flight after a real entry/exit. IRR/ISR
    /// remain unchanged: the request can be reconsidered after fault injection
    /// or a guest instruction changes masking. Interrupted delivery is refused.
    pub(crate) fn defer_after_exit(&mut self, vmcb: &mut Vmcb) -> Result<u8, Error> {
        if vmcb.virtual_interrupt_control() & 0xf != (self.tpr >> 4) as u64 {
            return Err(Error::TaskPriorityMismatch);
        }
        let request = self.flight.as_mut().ok_or(Error::NoDelivery)?;
        vmcb.defer_external_interrupt_after_exit(request)
            .map_err(Error::Interrupt)?;
        let vector = request.vector();
        self.flight = None;
        Ok(vector)
    }
    /// Non-specific EOI clears the highest ISR vector; empty ISR is a no-op.
    /// This is acknowledgement, independent of guest IRET completion.
    pub fn eoi(&mut self) -> Result<Option<u8>, Error> {
        self.idle()?;
        let vector = self.eoi_target();
        if let Some(v) = vector {
            Self::set(&mut self.isr, v, false);
        }
        Ok(vector)
    }
    pub fn timer_lvt(&self) -> u32 {
        self.timer.lvt
    }
    pub fn timer_initial(&self) -> u32 {
        self.timer.initial
    }
    pub fn timer_remaining(&self) -> u32 {
        self.timer.current
    }
    pub fn timer_divide(&self) -> u32 {
        self.timer.divide as u32
    }

    /// Pre-divider ticks to the next expiration, including the retained phase.
    /// This is a countdown boundary even when its eventual interrupt is masked.
    pub(crate) fn timer_source_remaining(&self) -> Option<u64> {
        if self.timer.current == 0 {
            return None;
        }
        let encoding = (self.timer.divide & 3) | ((self.timer.divide & 8) >> 1);
        let divisor = 1u64 << ((encoding + 1) & 7);
        Some(self.timer.current as u64 * divisor - self.timer.phase as u64)
    }

    /// Admitted APM2 16.4.1 Fig16-8 fixed-only profile; DS is read-only and always
    /// zero because each accepted expiration synchronously reaches the sole IRR.
    /// Masked vectors are retained, including zero. Unmasked vectors below 32
    /// require the missing low-vector/ESR policy, not a fabricated guest #GP.
    /// Reprogram active vector/mode only after cancelling the timer. Mask-only
    /// changes are admitted and preserve count and prescaler phase.
    pub fn write_timer_lvt(&mut self, value: u32) -> Result<(), Error> {
        self.idle()?;
        // Other physical profiles can define MMIO MT bits10:8 (PPR57896
        // APICx320); the MMIO adapter maps this rejection to policy-stop.
        // Its MSR832 layout reserves11:8, as does the selected APM profile.
        if value & !0x310ff != 0 {
            return Err(Error::ReservedRegisterBits);
        }
        if value & (1 << 12) != 0 {
            return Err(Error::ReadOnlyTimerStatus);
        }
        let value = value | if self.software_enabled() { 0 } else { 1 << 16 };
        if value & (1 << 16) == 0 && value & 255 < 32 {
            return Err(Error::UnsupportedTimerVector);
        }
        if self.timer.current != 0 && (value ^ self.timer.lvt) & !(1 << 16) != 0 {
            return Err(Error::TimerRunning);
        }
        self.timer.lvt = value;
        Ok(())
    }
    /// Reload/cancel current count and start a fresh supplied-source phase.
    /// The phase choice is deterministic fixture policy, not hardware-clock proof.
    pub fn write_timer_initial(&mut self, value: u32) -> Result<(), Error> {
        self.idle()?;
        self.timer.initial = value;
        self.timer.current = value;
        self.timer.phase = 0;
        Ok(())
    }
    /// APM2 16.4.1 Table16-3. Live divisor changes are outside admission because
    /// the pinned APM does not specify the prescaler phase of such a write.
    pub fn write_timer_divide(&mut self, value: u32) -> Result<(), Error> {
        self.idle()?;
        if value & !0xb != 0 {
            return Err(Error::ReservedRegisterBits);
        }
        if self.timer.current != 0 && value != self.timer.divide as u32 {
            return Err(Error::TimerRunning);
        }
        self.timer.divide = value as u8;
        Ok(())
    }
    /// Advance by supplied pre-divider source ticks, bounded even for u64::MAX.
    /// Masking/software-disable suppress generation, not counting. Expirations
    /// while masked are lost; an already accepted IRR bit is never cleared.
    /// Multiple periodic expirations coalesce to one pending bit. Outcomes report
    /// whether that bit was newly queued/already pending, not expiration counts.
    pub fn advance_timer(&mut self, ticks: u64) -> Result<TickOutcome, Error> {
        self.idle()?;
        if self.timer.current == 0 {
            return Ok(TickOutcome::Stopped);
        }
        let encoding = (self.timer.divide & 3) | ((self.timer.divide & 8) >> 1);
        let divisor = 1u64 << ((encoding + 1) & 7);
        // Divide first to avoid overflow when ticks is u64::MAX and phase != 0.
        let partial = ticks % divisor + self.timer.phase as u64;
        let decrements = ticks / divisor + partial / divisor;
        self.timer.phase = (partial % divisor) as u8;
        if decrements < self.timer.current as u64 {
            self.timer.current -= decrements as u32;
            return Ok(TickOutcome::Counting);
        }
        if self.timer.lvt & (1 << 17) != 0 {
            let beyond = decrements - self.timer.current as u64;
            self.timer.current = self.timer.initial - (beyond % self.timer.initial as u64) as u32;
        } else {
            self.timer.current = 0;
            self.timer.phase = 0;
        }
        if !self.software_enabled() || self.timer.lvt & (1 << 16) != 0 {
            return Ok(TickOutcome::MaskedExpiration);
        }
        let vector = self.timer.lvt as u8;
        let fresh = !self.pending(vector);
        Self::set(&mut self.irr, vector, true);
        Ok(if fresh {
            TickOutcome::Queued
        } else {
            TickOutcome::Coalesced
        })
    }
}
