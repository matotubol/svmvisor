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
    DoorbellTarget, ICR_RESERVED, MESSAGE_EXTERNAL, MESSAGE_FIXED, MESSAGE_INIT,
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

/// Stopped refusal of an incomplete IPI. The caller records the ICR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IpiRefusal {
    /// ID 1. Hardware already set IRR in every valid target and doorbelled
    /// the running ones (15.29.6.1 step 5); republishing would deliver twice.
    /// IsRunning is never cleared, so a target is not running only before its
    /// own arm has finished (AP guests run before the BSP arms).
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
    /// NMI IPI.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpiAction {
    /// INIT or STARTUP: route through `NativeIcr::route_x2avic_startup`,
    /// which admits only destination or all-excluding-self shorthand
    /// (Table 16-4 p644).
    Startup,
    /// Fixed edge IPI: publish with `Inventory::deliver_fixed`.
    Fixed(FixedIpi),
    /// Nothing to deliver; the caller records the drop and resumes.
    Dropped(IpiDrop),
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
    pub(super) const fn source_slot(&self) -> usize {
        self.source
    }

    /// AVIC_INCOMPLETE_IPI policy for EXITINFO1 `icr` and EXITINFO2 ID
    /// `reason` (D5). IDs 0, 2 and 4 delivered nothing (15.29.6.1 steps 3-4
    /// precede the IRR writes of step 5), so their IPI is routed by its
    /// message type. Table 15-27 does not name the ID reported for INIT or
    /// STARTUP (U8): INIT's vector is 0 and a STARTUP vector below 10h names a
    /// low trampoline page, so an ID 4 exit for either goes to the startup
    /// router, which checks the vector itself (decision). Informative only:
    /// Linux KVM avic.c emulates ID 0 and ID 2 fully, as hardware "falls over
    /// if _any_ targets are invalid".
    pub fn classify(&self, icr: u64, reason: u32) -> Result<IpiAction, IpiRefusal> {
        use IpiRefusal as R;
        match reason {
            0 | 2 | 4 => {}
            1 => return Err(R::TargetNotRunning),
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
            MESSAGE_NMI => Err(R::Nmi),
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
