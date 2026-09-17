//! Host IRQ bridge policy: pinned physical IRQ ownership behind x2AVIC.
//!
//! APM2 rev3.44 15.13.1, 15.21.1-2, 15.29.3.1/Table15-22,
//! 15.29.9.2/Tables15-28/29 and 16.6.3/16.6.4. Physical EOI ordering
//! differs from guest EOI ordering; never acknowledge a lower physical ISR
//! while a higher physical source remains in service. No allocations or locks.
//! Physical registers are reached only through `apic::PhysicalX2Apic`.
//!
//! Invariant after every bridge operation on the owning CPU: the physical ISR
//! equals the set of held level sources. Edge sources are acknowledged at
//! capture; a level source is acknowledged after its guest EOI, in physical
//! ISR order.
use super::BackingPage;
use crate::arch::x86_64::apic::{self, PhysicalX2Apic, highest_vector};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqError {
    ReservedVector(u8),
    PhysicalIsrMismatch { vector: u8, highest: Option<u8> },
    DuplicatePhysicalSource(u8),
    /// Retired (`PhysicalIrqLedger::prepare_capture`).
    AmbiguousLevelSource(u8),
    UnownedLevelCompletion(u8),
    CompletionNotReady(u8),
    UnexpectedPhysicalIsr(u8),
    /// The backing page refused the captured vector (`BackingPage::enqueue`).
    VirtualPublication(u8),
    /// A level-EOI exit named an in-service vector that is not the highest.
    VirtualIsrMismatch { vector: u8, highest: Option<u8> },
    /// The bounded physical EOI drain did not reach an empty ledger.
    DrainIncomplete,
    /// The host accepted this vector, but it has no physical ISR bit and is
    /// not the host spurious vector: the signature of an ExtINT (8259
    /// virtual-wire) acknowledgement through an unmasked ExtINT LINT, which
    /// goes "directly to the CPU core" without local APIC in-service state
    /// (APM2 rev3.44 16.6.3 p647). The bridge cannot own such a source.
    NotInService(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    /// The caller owns a virtual IRR publication, then one physical EOI.
    Edge,
    /// The physical EOI remains held until the corresponding virtual EOI.
    Level,
    /// `capture` only: an edge source acknowledged without publication,
    /// because the guest APIC is software-disabled (APM2 16.3.1 p629).
    Discarded,
}

/// One per permanently pinned physical CPU; modified only with host IRQ
/// acceptance closed. Guest fixed edge IPIs do not pass through this ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalIrqLedger {
    held: [u32; 8],
    completed: [u32; 8],
    /// Guest RIP of a level-EOI AVIC_NOACCEL exit whose EOI write may still
    /// be re-executed (`level_eoi_exit`).
    eoi_replay: Option<u64>,
}

impl PhysicalIrqLedger {
    pub const fn new() -> Self {
        Self { held: [0; 8], completed: [0; 8], eoi_replay: None }
    }

    /// True when no level source is held.
    pub fn is_empty(&self) -> bool { self.held.iter().all(|word| *word == 0) }

    /// Whether the vCPU's MSRPM must intercept guest EOI writes
    /// (`Msrpm::update_x2apic_eoi_intercept`): a level source is held, or an
    /// EOI write may still be re-executed (`level_eoi_exit`).
    pub fn intercepts_eoi(&self) -> bool { !self.is_empty() || self.eoi_replay.is_some() }

    /// An intercepted guest EOI write at `rip`. True when it is the
    /// re-execution of the write `level_eoi_exit` already completed, which
    /// the caller completes without another EOI. The first intercepted EOI
    /// write disarms the record either way: a write at another RIP is a
    /// nested handler's own EOI, and the record must not outlive the code at
    /// its address.
    pub fn take_eoi_replay(&mut self, rip: u64) -> bool {
        self.eoi_replay.take() == Some(rip)
    }

    pub fn holds(&self, vector: u8) -> bool {
        self.held[usize::from(vector / 32)] & (1 << (vector % 32)) != 0
    }

    /// The held level sources as an eight-bank vector bitmap.
    pub(crate) const fn held(&self) -> [u32; 8] {
        self.held
    }

    fn is_completed(&self, vector: u8) -> bool {
        self.completed[usize::from(vector / 32)] & (1 << (vector % 32)) != 0
    }

    /// Validate before any virtual bitmap publication or physical EOI; the
    /// result is `Edge` or `Level`.
    ///
    /// A level source whose vector is already pending or in service
    /// virtually is held like any other. A local APIC merges the request
    /// into the vector's IRR bit and TMR takes the level type (APM2 16.6.3
    /// p648), so the next EOI of that vector sends the I/O APIC EOI even when
    /// it ends the earlier interrupt; a line that is still asserted is then
    /// delivered again. The bridge does the same: that guest EOI completes
    /// the held source, and a still-asserted line is captured again.
    /// `IrqError::AmbiguousLevelSource` is retired; its wire code stays
    /// reserved.
    pub fn prepare_capture(
        &self, vector: u8, level: bool, physical_highest: Option<u8>,
    ) -> Result<Capture, IrqError> {
        if vector < 32 { return Err(IrqError::ReservedVector(vector)); }
        if physical_highest != Some(vector) {
            return Err(IrqError::PhysicalIsrMismatch { vector, highest: physical_highest });
        }
        if self.holds(vector) { return Err(IrqError::DuplicatePhysicalSource(vector)); }
        Ok(if level { Capture::Level } else { Capture::Edge })
    }

    /// Commit only after successful prepare_capture and virtual IRR ownership
    /// publication, without opening host acceptance between prepare and commit.
    pub fn commit_level_capture(&mut self, vector: u8) -> Result<(), IrqError> {
        if vector < 32 { return Err(IrqError::ReservedVector(vector)); }
        if self.holds(vector) { return Err(IrqError::DuplicatePhysicalSource(vector)); }
        self.held[usize::from(vector / 32)] |= 1 << (vector % 32);
        Ok(())
    }

    /// Mark a held level source guest-completed. The argument is the vector
    /// whose virtual ISR bit the guest's EOI cleared.
    pub fn complete_level(&mut self, vector: u8) -> Result<(), IrqError> {
        if !self.holds(vector) { return Err(IrqError::UnownedLevelCompletion(vector)); }
        let word = usize::from(vector / 32);
        let mask = 1 << (vector % 32);
        if self.completed[word] & mask != 0 {
            return Err(IrqError::UnownedLevelCompletion(vector));
        }
        self.completed[word] |= mask;
        Ok(())
    }

    /// Guest INIT discards every virtual in-service and pending interrupt
    /// (APM2 Table 16-2 p631), so every held source becomes completed and is
    /// then acknowledged in physical ISR order.
    pub fn retire_all(&mut self) {
        self.completed = self.held;
        self.eoi_replay = None;
    }

    /// Read-only INIT precondition: the physical ISR (banks of MSRs
    /// 810h-817h) holds exactly the held sources, so the retirement drain
    /// cannot fail on a foreign or missing physical in-service bit.
    pub fn check_retirement(&self, physical_isr: &[u32; 8]) -> Result<(), IrqError> {
        let mut foreign = [0; 8];
        let mut missing = [0; 8];
        for index in 0..8 {
            foreign[index] = physical_isr[index] & !self.held[index];
            missing[index] = self.held[index] & !physical_isr[index];
        }
        if let Some(vector) = highest_vector(&foreign) {
            return Err(IrqError::UnexpectedPhysicalIsr(vector));
        }
        if let Some(vector) = highest_vector(&missing) {
            return Err(IrqError::PhysicalIsrMismatch { vector, highest: highest_vector(physical_isr) });
        }
        Ok(())
    }

    /// Read the actual physical ISR anew after each EOI. A higher held source
    /// that the guest has not completed postpones lower completed sources.
    pub fn next_eoi(&self, physical_highest: Option<u8>) -> Result<Option<u8>, IrqError> {
        let owned_highest = highest_vector(&self.held);
        let Some(vector) = physical_highest else {
            return match owned_highest {
                None => Ok(None),
                Some(vector) => Err(IrqError::PhysicalIsrMismatch { vector, highest: None }),
            };
        };
        if !self.holds(vector) { return Err(IrqError::UnexpectedPhysicalIsr(vector)); }
        if let Some(owned) = owned_highest
            && owned != vector
        {
            return Err(IrqError::PhysicalIsrMismatch { vector: owned, highest: physical_highest });
        }
        Ok(self.is_completed(vector).then_some(vector))
    }

    /// Call immediately after the physical EOI chosen by next_eoi succeeds.
    pub fn commit_eoi(&mut self, vector: u8) -> Result<(), IrqError> {
        let word = usize::from(vector / 32);
        let mask = 1 << (vector % 32);
        if self.held[word] & self.completed[word] & mask == 0 {
            return Err(IrqError::CompletionNotReady(vector));
        }
        self.held[word] &= !mask;
        self.completed[word] &= !mask;
        Ok(())
    }
}

impl Default for PhysicalIrqLedger { fn default() -> Self { Self::new() } }

/// Physical EOI (MSR 80Bh write of zero, Table 16-6 p658).
fn physical_eoi(physical: &mut impl PhysicalX2Apic) {
    physical.write(apic::msr(apic::EOI), 0);
}

/// Acknowledge completed held sources while each is the highest physical
/// in-service vector (APM2 16.6.4 p652: EOI resets the highest ISR bit).
/// Bounded: at most 224 sources (vectors 32-255) are held and each round
/// releases one, so 225 rounds reach the terminating `next_eoi` result.
pub(crate) fn drain(ledger: &mut PhysicalIrqLedger, physical: &mut impl PhysicalX2Apic)
    -> Result<(), IrqError>
{
    for _ in 0..=224 {
        let Some(vector) = ledger.next_eoi(apic::highest_in_service(physical))? else {
            return Ok(());
        };
        physical_eoi(physical);
        ledger.commit_eoi(vector)?;
    }
    Err(IrqError::DrainIncomplete)
}

/// Bridge one physical interrupt that the host gate accepted on this CPU,
/// with host acceptance closed and the guest stopped. `Ok(None)` is a
/// physical spurious interrupt: it leaves the ISR unaffected and needs no EOI
/// (APM2 16.4.7 p640); the host-owned physical SVR vector identifies it. Any
/// other vector without a physical ISR bit is refused as `NotInService`. An
/// edge source is published and acknowledged; a level source is published
/// with TMR set and held until its guest EOI (16.6.3 p648). Publication
/// precedes any physical EOI. Errors are terminal for the caller; a refusal
/// before publication changes nothing.
///
/// A software-disabled guest APIC accepts no further fixed interrupts
/// (16.3.1 p629): an edge source is only acknowledged (`Discarded`). A level
/// source is still published and held (documented deviation): acknowledging
/// it without a guest EOI would re-deliver the still-asserted line at once
/// and livelock this CPU. x2AVIC does not consult the virtual SVR, so the
/// guest may take that interrupt while software-disabled.
pub fn capture(
    vector: u8,
    backing: &BackingPage,
    ledger: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
) -> Result<Option<Capture>, IrqError> {
    let in_service = apic::in_service_banks(physical);
    if in_service[usize::from(vector / 32)] & (1 << (vector % 32)) == 0 {
        return if physical.read(apic::msr(apic::SVR)) as u8 == vector {
            Ok(None)
        } else {
            Err(IrqError::NotInService(vector))
        };
    }
    let highest = highest_vector(&in_service);
    let level = apic::level_triggered(physical, vector);
    let capture = ledger.prepare_capture(vector, level, highest)?;
    if capture == Capture::Edge && !backing.software_enabled() {
        physical_eoi(physical);
        drain(ledger, physical)?;
        return Ok(Some(Capture::Discarded));
    }
    if backing.enqueue(vector, level).is_err() {
        return Err(IrqError::VirtualPublication(vector));
    }
    match capture {
        Capture::Level => ledger.commit_level_capture(vector)?,
        Capture::Edge | Capture::Discarded => physical_eoi(physical),
    }
    drain(ledger, physical)?;
    Ok(Some(capture))
}

/// Complete the host side of a guest EOI whose virtual ISR bit is already
/// clear (D6): a held, not yet completed level source is completed and the
/// physical EOIs are drained; a TMR bit that no longer describes a pending
/// interrupt is cleared. Level completion is decided by the ledger only.
fn complete_guest_eoi(
    vector: u8,
    backing: &BackingPage,
    ledger: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
) -> Result<(), IrqError> {
    if ledger.holds(vector) && !ledger.is_completed(vector) {
        ledger.complete_level(vector)?;
        drain(ledger, physical)?;
    }
    backing.clear_trigger_unless_pending(vector);
    Ok(())
}

/// Intercepted guest EOI (MSR 80Bh write, value already checked zero; D6).
/// Clears the highest virtual ISR bit and recomputes PPR, then completes the
/// host side. The APM gives no effect for an EOI with no in-service vector;
/// it completes as a no-op (decision). Returns the vector.
pub(crate) fn software_eoi(
    backing: &BackingPage,
    ledger: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
) -> Result<Option<u8>, IrqError> {
    let Some(vector) = backing.eoi_stopped() else { return Ok(None) };
    complete_guest_eoi(vector, backing, ledger, physical)?;
    Ok(Some(vector))
}

/// AVIC_NOACCEL level-EOI exit (EXITINFO2[7:0] = `vector`, Table 15-29
/// p582) at guest `rip`, the fallback when the EOI write was not intercepted.
/// The manual contradicts itself: Table 15-22 p566 makes the level-triggered
/// EOI write "#VMEXIT (trap)" (after the access, 15.29.3.1 p566), 15.29.9.2
/// p581 calls the same exit a fault (before the access), and neither says
/// whether a trapped write has cleared the ISR bit. The EOI is completed
/// here under every reading, and the caller never advances RIP:
/// - ISR bit clear: hardware applied the write, so the exit was a trap and
///   RIP is past the WRMSR. Only the host side is completed.
/// - ISR bit set (it must be the highest): it is cleared here. If the exit
///   was a fault, RIP still names the WRMSR and the guest executes it again;
///   that second EOI would retire a lower in-service vector early. `rip` is
///   therefore recorded and guest EOI writes are intercepted
///   (`intercepts_eoi`) until the next one, which `take_eoi_replay`
///   recognizes at the same RIP and the caller completes without effect.
///   Under the trap reading `rip` follows the WRMSR and only a second EOI
///   WRMSR directly behind the first could match (accepted; decision).
pub fn level_eoi_exit(
    vector: u8,
    rip: u64,
    backing: &BackingPage,
    ledger: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
) -> Result<(), IrqError> {
    if backing.is_in_service(vector) {
        let highest = backing.highest_in_service();
        if highest != Some(vector) {
            return Err(IrqError::VirtualIsrMismatch { vector, highest });
        }
        backing.eoi_stopped();
        ledger.eoi_replay = Some(rip);
    }
    complete_guest_eoi(vector, backing, ledger, physical)
}

/// Guest INIT (D9 commit step 2): complete every held source and drain.
/// Precondition: `check_retirement` succeeded for the current physical ISR.
pub(crate) fn retire(ledger: &mut PhysicalIrqLedger, physical: &mut impl PhysicalX2Apic)
    -> Result<(), IrqError>
{
    ledger.retire_all();
    drain(ledger, physical)?;
    if ledger.is_empty() { Ok(()) } else { Err(IrqError::DrainIncomplete) }
}
