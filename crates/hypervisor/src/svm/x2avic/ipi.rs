//! Guest IPIs that x2AVIC hardware did not complete, and their software
//! fixed-IPI fan-out.
//!
//! AMD APM2 rev3.44: AVIC_INCOMPLETE_IPI (401h) is 15.29.9.1, Tables
//! 15-25..15-27 pp580-581, with the hardware step order of 15.29.6.1
//! pp576-577. The x2APIC ICR is 16.13 Figure 16-34 p661 (field meanings in
//! Figure 16-18 pp642-644, valid combinations in Table 16-4 p644), logical
//! destinations are 16.14 p662, and broadcast is DEST FFFF_FFFFh (p660).
//!
//! The ICR write that caused the exit has completed (Table 15-22 p567 lists
//! the ICRL write as a trap), so a refusal can never become #GP; RIP has
//! already advanced and the exit's nRIP is not used.
use super::{BackingPage, Error, logical_x2apic_id};
use crate::arch::x86_64::apic::{
    DoorbellTarget, ICR_RESERVED, SELF_IPI_MSR, MESSAGE_EXTERNAL, MESSAGE_FIXED, MESSAGE_INIT,
    MESSAGE_LOWEST_PRIORITY, MESSAGE_NMI, MESSAGE_REMOTE_READ, MESSAGE_SMI, MESSAGE_STARTUP,
};

/// Trigger mode (TGM, bit 15): 1 is level-sensitive (Figure 16-18 p643).
const ICR_LEVEL_TRIGGERED: u64 = 1 << 15;
/// Destination mode (DM, bit 11): 1 is logical (Figure 16-18 p643).
const ICR_LOGICAL: u64 = 1 << 11;
/// x2APIC broadcast destination (16.13 p660, 16.14 p662).
const BROADCAST: u32 = u32::MAX;
/// Maximum admitted CPUs; slot sets are 32-bit masks.
const MAX_SLOTS: usize = 32;
/// Destination shorthand 01b, self (bits 19:18, Figure 16-18 p644).
const ICR_SHORTHAND_SELF: u64 = 1 << 18;

/// The ICR command of an incomplete IPI whose guest write was `msr` with
/// EXITINFO1 `written`. A SELF IPI write (MSR 83Fh) is a to-self, fixed,
/// edge-triggered ICR write of its vector (16.15 p663), and 15.29.10 p583
/// handles it "the same way as ICR MSR acceleration" without saying whether
/// EXITINFO1 then carries that ICR or only the written vector. Both become
/// the same command here, so a bare vector is never read as a physical IPI
/// to x2APIC ID 0. Any other MSR is the ICR itself (Table 15-25 p580).
pub const fn written_command(msr: u32, written: u64) -> u64 {
    if msr == SELF_IPI_MSR { (written & 0xff) | ICR_SHORTHAND_SELF } else { written }
}

/// Stopped refusal of an incomplete IPI. The caller records the ICR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IpiRefusal {
    /// ID 1 for an ICR that hardware cannot have published: only a fixed,
    /// edge-triggered IPI with a legal vector reaches the IsRunning check
    /// (15.29.6.1 steps 5-6 p577). A consistent ID 1 is
    /// `IpiAction::Published`, never a refusal.
    TargetNotRunning = 1,
    /// ID 3: the physical-ID table named an invalid backing page.
    InvalidBackingPage = 2,
    /// ID 5 (Secure AVIC only) or a reserved ID.
    UnknownReason = 3,
    /// An x2APIC-reserved ICR bit is set.
    ReservedBits = 4,
    /// Message type 1 (lowest priority), 3 (remote read) or 7 (ExtINT),
    /// eliminated and reserved in x2APIC mode (16.13 p661).
    ReservedMessageType = 5,
    /// Fixed message type with level trigger.
    LevelTriggered = 6,
    /// SMI IPI.
    Smi = 7,
    /// An NMI IPI whose destination form Table 16-4 p644 does not admit: the
    /// self (01) or all-including-self (10) shorthand, or a bare broadcast
    /// destination (FFFF_FFFFh, 16.13 p660). NMI is valid only with
    /// "Destination or all excluding self", like INIT/STARTUP.
    Nmi = 8,
    /// ID 4 for an ICR that is not a fixed IPI with a vector below 16.
    InconsistentVectorExit = 9,
}

/// Incomplete IPI delivered to nobody, with no modeled APIC error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpiDrop {
    /// Fixed vector below 16: an illegal-vector APIC error (Table 15-27 ID 4;
    /// Figure 16-7 p635). Virtual ESR error generation is not modeled.
    /// Informative only: Linux KVM avic.c also drops these IPIs.
    IllegalVector,
    /// No admitted CPU matches the destination. The send-accept error
    /// (Figure 16-16 p640) is not modeled.
    NoTarget,
}

/// A validated fixed, edge-triggered IPI and its admitted target slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedIpi {
    vector: u8,
    targets: u32,
}

impl FixedIpi {
    pub const fn vector(self) -> u8 { self.vector }
    /// Bit `s` selects inventory slot `s`.
    pub const fn targets(self) -> u32 { self.targets }
}

/// A validated NMI IPI and its admitted target slots. The ICR vector is
/// ignored for NMI (Figure 16-18 p642, Table 16-4 p644); delivery sets each
/// target's V_NMI (15.21.10 p536), so no vector is carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NmiIpi {
    targets: u32,
}

impl NmiIpi {
    /// Bit `s` selects inventory slot `s`.
    pub const fn targets(self) -> u32 { self.targets }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpiAction {
    /// INIT or STARTUP: route through `NativeIcr::route_x2avic_startup`,
    /// which admits only destination or all-excluding-self shorthand
    /// (Table 16-4 p644).
    Startup,
    /// Fixed edge IPI: publish with `Inventory::deliver_fixed`.
    Fixed(FixedIpi),
    /// NMI IPI: set V_NMI on each target (the sender directly, remote targets
    /// through the startup mailbox's NMI command and the private kick).
    Nmi(NmiIpi),
    /// Nothing to deliver; the caller records the drop and resumes.
    Dropped(IpiDrop),
    /// ID 1: hardware already set IRR in every valid target and doorbelled
    /// the running ones (15.29.6.1 step 5 p577); step 6 only reports a
    /// target whose IsRunning is clear. IsRunning is never cleared here, so
    /// such a target has not finished its own arm (AP guests run before the
    /// BSP arms); its backing page is already published and its first VMRUN
    /// evaluates IRR (15.29.8.3 p579). The caller resumes and publishes
    /// nothing.
    Published,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanOutError {
    /// A remote target's host APIC ID cannot be doorbelled. Nothing was
    /// published.
    DoorbellTarget { slot: usize, id: u32 },
    /// The target backing page refused the edge publication. Lower target
    /// slots were already published and doorbelled.
    Publication { slot: usize, error: Error },
}

/// Immutable admitted native CPU inventory: slot `s` has x2APIC ID
/// `ids[s]`, and guest APIC IDs equal host x2APIC IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inventory {
    ids: [u32; MAX_SLOTS],
    count: usize,
    source: usize,
}

impl Inventory {
    /// Nonempty, at most 32 distinct IDs, containing `source`, without the
    /// broadcast value. Firmware ordinals are never substituted for IDs.
    pub(super) fn admit(source: u32, ids: &[u32]) -> Option<Self> {
        if ids.is_empty() || ids.len() > MAX_SLOTS || ids.contains(&BROADCAST) {
            return None;
        }
        let mut admitted = [0; MAX_SLOTS];
        for (index, id) in ids.iter().enumerate() {
            if ids[..index].contains(id) {
                return None;
            }
            admitted[index] = *id;
        }
        let source = ids.iter().position(|id| *id == source)?;
        Some(Self { ids: admitted, count: ids.len(), source })
    }

    pub(super) fn ids(&self) -> &[u32] {
        &self.ids[..self.count]
    }

    pub(super) fn source_id(&self) -> u32 {
        self.ids[self.source]
    }

    /// Slot of the sending CPU.
    pub(crate) const fn source_slot(&self) -> usize {
        self.source
    }

    /// AVIC_INCOMPLETE_IPI policy for EXITINFO1 `icr` and EXITINFO2 ID
    /// `reason` (D5). IDs 0, 2 and 4 delivered nothing (15.29.6.1 steps 3-4
    /// precede the IRR writes of step 5), so their IPI is routed by its
    /// message type. ID 1 follows the IRR writes and needs no software
    /// delivery (`IpiAction::Published`). Table 15-27 does not name the ID
    /// reported for INIT or STARTUP (U8): INIT's vector is 0 and a STARTUP vector below 10h names a
    /// low trampoline page, so an ID 4 exit for either goes to the startup
    /// router, which checks the vector itself (decision). Informative only:
    /// Linux KVM avic.c emulates ID 0 and ID 2 fully, as hardware "falls over
    /// if _any_ targets are invalid".
    pub fn classify(&self, icr: u64, reason: u32) -> Result<IpiAction, IpiRefusal> {
        use IpiRefusal as R;
        match reason {
            0 | 2 | 4 => {}
            1 => {
                let fixed_edge = icr & (ICR_RESERVED | ICR_LEVEL_TRIGGERED | (7 << 8)) == 0;
                return if fixed_edge && icr as u8 >= 16 {
                    Ok(IpiAction::Published)
                } else {
                    Err(R::TargetNotRunning)
                };
            }
            3 => return Err(R::InvalidBackingPage),
            _ => return Err(R::UnknownReason),
        }
        if icr & ICR_RESERVED != 0 {
            return Err(R::ReservedBits);
        }
        let message = ((icr >> 8) & 7) as u8;
        let vector = icr as u8;
        if matches!(message, MESSAGE_LOWEST_PRIORITY | MESSAGE_REMOTE_READ | MESSAGE_EXTERNAL) {
            return Err(R::ReservedMessageType);
        }
        if matches!(message, MESSAGE_INIT | MESSAGE_STARTUP) {
            return Ok(IpiAction::Startup);
        }
        if reason == 4 {
            // Table 15-27: ID 4 is "VEC < 16", which names a delivered vector
            // only for the fixed type (Figure 16-18 p642).
            return if message == MESSAGE_FIXED && vector < 16 {
                Ok(IpiAction::Dropped(IpiDrop::IllegalVector))
            } else {
                Err(R::InconsistentVectorExit)
            };
        }
        match message {
            MESSAGE_SMI => Err(R::Smi),
            MESSAGE_NMI => {
                // Table 16-4 p644: NMI is valid only with the "Destination or
                // all excluding self" shorthand and any trigger mode (the
                // vector is ignored). Self (01) and all-including-self (10),
                // and a bare broadcast destination (16.13 p660), are not
                // admitted, like INIT/STARTUP; refuse them the way the startup
                // router does. A destination that reaches nobody is a drop, as
                // real hardware ignores an IPI to an absent APIC.
                let shorthand = (icr >> 18) & 3;
                if !matches!(shorthand, 0 | 3)
                    || (shorthand == 0 && (icr >> 32) as u32 == BROADCAST)
                {
                    return Err(R::Nmi);
                }
                match self.targets(icr) {
                    0 => Ok(IpiAction::Dropped(IpiDrop::NoTarget)),
                    targets => Ok(IpiAction::Nmi(NmiIpi { targets })),
                }
            }
            _ if icr & ICR_LEVEL_TRIGGERED != 0 => Err(R::LevelTriggered),
            _ if vector < 16 => Ok(IpiAction::Dropped(IpiDrop::IllegalVector)),
            _ => match self.targets(icr) {
                0 => Ok(IpiAction::Dropped(IpiDrop::NoTarget)),
                targets => Ok(IpiAction::Fixed(FixedIpi { vector, targets })),
            },
        }
    }

    /// Destination slots of a fixed IPI. Shorthand (bits 19:18, Figure 16-18
    /// p644) comes first and ignores DEST and DM: 01 self, 10 all including
    /// self, 11 all excluding self. Without shorthand, DEST FFFF_FFFFh is
    /// broadcast (16.13 p660); physical mode matches the 32-bit x2APIC ID;
    /// logical mode matches a cluster ID (bits 31:16) exactly and any common
    /// logical ID bit (15:0) of the derived LDR (16.14 p662). DEST FFh is not
    /// a broadcast: chapter 16 names only FFFF_FFFFh for x2APIC mode
    /// (decision). The startup router uses the same computation.
    pub(super) fn targets(&self, icr: u64) -> u32 {
        let all = if self.count == MAX_SLOTS { u32::MAX } else { (1u32 << self.count) - 1 };
        let own = 1u32 << self.source;
        match (icr >> 18) & 3 {
            1 => own,
            2 => all,
            3 => all & !own,
            _ => {
                let destination = (icr >> 32) as u32;
                if destination == BROADCAST {
                    return all;
                }
                let logical = icr & ICR_LOGICAL != 0;
                let mut targets = 0;
                for (slot, &id) in self.ids().iter().enumerate() {
                    let matched = if logical {
                        let ldr = logical_x2apic_id(id);
                        destination >> 16 == ldr >> 16 && destination & ldr & 0xffff != 0
                    } else {
                        destination == id
                    };
                    if matched {
                        targets |= 1 << slot;
                    }
                }
                targets
            }
        }
    }

    /// Software fan-out of one fixed edge IPI (D5), in ascending slot order.
    /// Each target gets an atomic IRR set with its TMR bit cleared
    /// (`BackingPage::enqueue`), as a native edge interrupt would (16.6.3
    /// p648); `backing(slot)` resolves the slot's page. Every remote target
    /// is then doorbelled (15.29.8.2 p579); the source is not, because its
    /// next VMRUN evaluates IRR (15.29.8.3 p579). A software-disabled target
    /// accepts no further fixed interrupts (16.3.1 p629), so it is skipped
    /// and not doorbelled; its SVR is sampled once, and a target changing SVR
    /// concurrently is stopped in its own SVR write, which accounts for
    /// interrupts arriving around that point (`GuestX2Apic`). Doorbell
    /// targets are validated before any publication. Target bits outside
    /// this inventory are ignored.
    pub fn deliver_fixed<'a>(
        &self,
        ipi: FixedIpi,
        mut backing: impl FnMut(usize) -> &'a BackingPage,
        mut ring: impl FnMut(DoorbellTarget),
    ) -> Result<(), FanOutError> {
        let selected = |slot: &usize| ipi.targets & (1 << *slot) != 0;
        for slot in (0..self.count).filter(selected) {
            let id = self.ids[slot];
            if slot != self.source && DoorbellTarget::new(id).is_none() {
                return Err(FanOutError::DoorbellTarget { slot, id });
            }
        }
        for slot in (0..self.count).filter(selected) {
            let page = backing(slot);
            if !page.software_enabled() {
                continue;
            }
            page.enqueue(ipi.vector, false)
                .map_err(|error| FanOutError::Publication { slot, error })?;
            if slot != self.source
                && let Some(target) = DoorbellTarget::new(self.ids[slot])
            {
                ring(target);
            }
        }
        Ok(())
    }
}
