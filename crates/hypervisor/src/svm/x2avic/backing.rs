//! Per-vCPU x2AVIC backing page, APM2 3.44 15.29.3-15.29.4 and Table 15-22.
//! Register offsets and reset values follow Table 16-2; INIT follows 16.10.
use super::{Error, GUEST_APIC_VERSION, MAX_ID, PAGE_BYTES, logical_x2apic_id};
use crate::arch::x86_64::apic;
use core::sync::atomic::{AtomicU32, Ordering};

/// Word index of a constant, aligned register offset.
const fn word(offset: u16) -> usize {
    offset as usize / 4
}

/// Word index of the bank holding `vector` in an eight-bank register.
const fn bank(base: u16, vector: u8) -> usize {
    word(base) + (vector as usize / 32) * 4
}

/// Hardware sees 32-bit register slots at 16-byte strides. Atomic accesses
/// preserve hardware IRR updates; WB mapping and hardware lifetime are external.
#[repr(C, align(4096))]
pub struct BackingPage {
    words: [AtomicU32; PAGE_BYTES / 4],
}
impl Default for BackingPage {
    fn default() -> Self {
        Self::new()
    }
}
impl BackingPage {
    pub const fn new() -> Self {
        Self { words: [const { AtomicU32::new(0) }; PAGE_BYTES / 4] }
    }

    fn register(offset: u16) -> Result<usize, Error> {
        if offset as usize >= PAGE_BYTES || offset & 15 != 0 {
            return Err(Error::InvalidOffset);
        }
        Ok(word(offset))
    }
    pub fn read_register(&self, offset: u16) -> Result<u32, Error> {
        Ok(self.words[Self::register(offset)?].load(Ordering::Acquire))
    }
    /// Caller excludes guest execution and all writers of this register. Other
    /// CPUs may still publish distinct IRR words through this shared page; do
    /// not borrow the entire hardware-visible page mutably for a local store.
    /// This does not validate a guest architectural register access.
    pub fn write_register_stopped(&self, offset: u16, value: u32) -> Result<(), Error> {
        self.words[Self::register(offset)?].store(value, Ordering::Release);
        Ok(())
    }
    /// Six standard LVTs, no AMD extension exposure. APM Table16-2/16.10:
    /// INIT preserves APIC_BASE mode (owned outside this backing page), ID and
    /// version; x2APIC LDR is derived, not reset to legacy flat-mode state.
    /// Offset E0h (DFR, FFFFFFFFh in Table 16-2) has no x2APIC register
    /// (16.11.1 p657) and stays zero. Caller quiesces every producer/table
    /// reference before acquiring &mut.
    pub fn reset_stopped(&mut self, id: u32, version: u32) -> Result<(), Error> {
        if id > MAX_ID as u32 {
            return Err(Error::InvalidId);
        }
        if version != GUEST_APIC_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        for slot in &mut self.words {
            *slot.get_mut() = 0;
        }
        *self.words[word(apic::ID)].get_mut() = id;
        *self.words[word(apic::VERSION)].get_mut() = version;
        *self.words[word(apic::LDR)].get_mut() = logical_x2apic_id(id);
        *self.words[word(apic::SVR)].get_mut() = 0xff;
        for offset in apic::LVTS {
            *self.words[word(offset)].get_mut() = apic::LVT_MASKED;
        }
        Ok(())
    }
    /// Read-only precondition of `reset_after_init_stopped`: the identity the
    /// reset preserves is an admitted one. No register changes.
    pub(crate) fn check_init_identity(&self) -> Result<(), Error> {
        let id = self.words[word(apic::ID)].load(Ordering::Acquire);
        let version = self.words[word(apic::VERSION)].load(Ordering::Acquire);
        if id > MAX_ID as u32 {
            return Err(Error::InvalidId);
        }
        if version != GUEST_APIC_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        Ok(())
    }
    /// Apply architectural INIT to an already-bound x2APIC backing page.
    /// APM2 rev3.44 Table 16-2 p631 ("after reset and INIT") and 16.10 p657:
    /// TPR, APR, PPR, ESR, ICR, ISR, TMR, IRR, counts and divide become 0,
    /// SVR FFh and the six LVTs 10000h. ID and version are preserved (PPR
    /// 57896 rev3.00 p55: INIT leaves ApicId unaffected). LDR takes the
    /// derived logical x2APIC ID, not Table 16-2's xAPIC value 0: 16.14 p661
    /// initializes the x2APIC LDR whenever x2APIC mode is enabled and INIT
    /// keeps that mode, while 16.10 cites a "Reset in x2APIC mode" section
    /// that does not exist (decision). APIC_BASE mode is owned outside this
    /// page.
    ///
    /// The target guest is stopped. The caller has already reset the mirrored
    /// physical timer/LVTs and retired every physical level source this page
    /// held (`registers::commit_init`), so no local source metadata survives.
    /// Page/table identity remains stable. Remote IRR publication may race the
    /// atomic per-bank clears: a publication preceding its bank's clear is
    /// discarded by INIT; one following it remains pending. This is not a
    /// simultaneous snapshot, fabric drain, retarget, or permission to reclaim
    /// the backing page. A physical interrupt still pending in physical IRR is
    /// captured after the next VMRUN into the reset page, where a real INIT
    /// would have discarded it (recorded deviation). No guest register reader
    /// or local dispatch/EOI may run during this operation. Validation is
    /// read-only; after it succeeds all stores are infallible and bounded.
    pub fn reset_after_init_stopped(&self) -> Result<(), Error> {
        self.check_init_identity()?;
        let id = self.words[word(apic::ID)].load(Ordering::Acquire);
        self.words[word(apic::SVR)].store(0xff, Ordering::Release);
        for offset in apic::LVTS {
            self.words[word(offset)].store(apic::LVT_MASKED, Ordering::Release);
        }
        for offset in [
            apic::TPR,
            apic::APR,
            apic::PPR,
            apic::EOI,
            apic::RRR,
            apic::ESR,
            apic::ICR,
            apic::ICR_HIGH,
            apic::TIMER_INITIAL_COUNT,
            apic::TIMER_CURRENT_COUNT,
            apic::TIMER_DIVIDE,
        ] {
            self.words[word(offset)].store(0, Ordering::Release);
        }
        // x2APIC LDR is an identity-derived read-only register, not legacy zero.
        self.words[word(apic::LDR)].store(logical_x2apic_id(id), Ordering::Release);
        for base in [apic::ISR, apic::TMR, apic::IRR] {
            for index in 0..8 {
                self.words[word(base) + index * 4].store(0, Ordering::Release);
            }
        }
        Ok(())
    }

    fn bit(&self, base: u16, vector: u8) -> bool {
        self.words[bank(base, vector)].load(Ordering::Acquire) & (1 << (vector % 32)) != 0
    }
    pub fn is_pending(&self, vector: u8) -> bool {
        self.bit(apic::IRR, vector)
    }
    pub fn is_in_service(&self, vector: u8) -> bool {
        self.bit(apic::ISR, vector)
    }
    pub fn is_level(&self, vector: u8) -> bool {
        self.bit(apic::TMR, vector)
    }
    /// SVR bit 8 (ASE, Figure 16-17 p641). While it is clear, the virtual
    /// APIC accepts no further fixed interrupts (16.3.1 p629).
    pub fn software_enabled(&self) -> bool {
        self.words[word(apic::SVR)].load(Ordering::Acquire) & apic::SVR_SOFTWARE_ENABLE != 0
    }
    /// The eight IRR banks. Remote publishers may add bits during the scan;
    /// the result is a stopped-guest observation, not a snapshot.
    pub(crate) fn pending_banks(&self) -> [u32; 8] {
        core::array::from_fn(|index| {
            self.words[word(apic::IRR) + index * 4].load(Ordering::Acquire)
        })
    }
    /// Withdraw the pending `vectors` (an eight-bank bitmap) that this APIC
    /// never accepted: clear each IRR bit, then its TMR bit. The caller
    /// excludes level sources it still owns; remote publishers set edge
    /// vectors only, which keep TMR clear themselves (`set_trigger`), so the
    /// atomic per-bank updates cannot leave a stale trigger bit. Bits 15:0 of
    /// the first bank are reserved (16.6.3 p647) and never touched.
    pub(crate) fn discard_pending(&self, vectors: &[u32; 8]) {
        for (index, bits) in vectors.iter().enumerate() {
            let bits = if index == 0 { bits & !0xffff } else { *bits };
            if bits != 0 {
                self.words[word(apic::IRR) + index * 4].fetch_and(!bits, Ordering::AcqRel);
                self.words[word(apic::TMR) + index * 4].fetch_and(!bits, Ordering::AcqRel);
            }
        }
    }
    /// Atomic publication to AVIC. A vector that is already pending or in
    /// service takes the trigger type of this acceptance, as a local APIC's
    /// TMR does (`set_trigger`). INIT may discard a racing publication at
    /// its bank clear; level-source metadata needs the reset owner's separate
    /// coordination. No delivery or EOI is claimed: return true only when IRR
    /// was newly set. Ringing a doorbell or
    /// returning through VMRUN is the runtime's separate responsibility.
    pub fn enqueue(&self, vector: u8, level: bool) -> Result<bool, Error> {
        self.set_trigger(vector, level)?;
        let bit = 1 << (vector % 32);
        Ok(self.words[bank(apic::IRR, vector)].fetch_or(bit, Ordering::AcqRel) & bit == 0)
    }
    /// Record TMR before the IRR publication; the atomic update preserves
    /// unrelated vectors and never enqueues. APM2 16.6.3 p648: "When the
    /// interrupt is accepted by the local APIC and the IRR bit is set, the
    /// associated TMR bit is set for level-sensitive interrupts or reset for
    /// edge-triggered interrupts", also for a vector that is pending or in
    /// service with the other trigger type (a second request of an
    /// in-service vector sets IRR, same page). Hardware-accelerated IPIs
    /// reach such a vector without any software check. The bridge never
    /// reads TMR to complete a level source (`irq::PhysicalIrqLedger`), so a
    /// racing remote edge publication cannot lose a physical EOI.
    /// `Error::MixedTrigger` is retired; its wire code stays reserved.
    fn set_trigger(&self, vector: u8, level: bool) -> Result<(), Error> {
        if vector < 16 {
            return Err(Error::InvalidVector);
        }
        let bit = 1 << (vector % 32);
        let tmr = &self.words[bank(apic::TMR, vector)];
        if level {
            tmr.fetch_or(bit, Ordering::Release);
        } else {
            tmr.fetch_and(!bit, Ordering::Release);
        }
        Ok(())
    }
    /// Clear a TMR bit left by a completed level source unless the vector is
    /// pending again (APM2 16.6.3 p648: TMR describes an accepted interrupt).
    /// Only the owning CPU publishes level sources, and it calls this while
    /// its guest is stopped. A remote edge publication racing this clear
    /// clears the same bit itself (`set_trigger`), so the result is correct.
    pub(crate) fn clear_trigger_unless_pending(&self, vector: u8) {
        if vector >= 16 && !self.is_pending(vector) {
            self.words[bank(apic::TMR, vector)].fetch_and(!(1 << (vector % 32)), Ordering::AcqRel);
        }
    }
    /// Highest set vector of an eight-bank register. Bits 15:0 of the first
    /// bank are reserved (16.6.3 p647) and never name a vector.
    fn highest(&self, base: u16) -> Option<u8> {
        for index in (0..8).rev() {
            let mut bits = self.words[word(base) + index * 4].load(Ordering::Acquire);
            if index == 0 {
                bits &= !0xffff;
            }
            if bits != 0 {
                return Some((index * 32 + 31 - bits.leading_zeros() as usize) as u8);
            }
        }
        None
    }
    /// Stopped guest inspection; hardware may still add IRR, but no other CPU
    /// may dispatch into or reset this page during this bounded ISR scan.
    pub fn highest_in_service(&self) -> Option<u8> {
        self.highest(apic::ISR)
    }
    /// Highest pending vector. Remote publishers may add IRR bits during the
    /// scan; the result is a stopped-guest observation, not a snapshot.
    pub(crate) fn highest_pending(&self) -> Option<u8> {
        self.highest(apic::IRR)
    }
    /// Software completion of an EOI whose ISR effect has not happened yet,
    /// never a physical EOI: clear the highest in-service bit and recompute
    /// PPR (APM2 15.29.3.1 p569; 16.6.4 p651: PP is the higher of the ISR
    /// class and TP, and PPS equals TPS when PP equals TP; PPS is otherwise
    /// zero, a decision for the unstated case). The guest is stopped and no
    /// other local dispatch runs; remote CPUs change only IRR and TMR. Level
    /// completion of the returned vector is decided by the host IRQ ledger,
    /// never by TMR, which remote publishers may change.
    pub fn eoi_stopped(&self) -> Option<u8> {
        let vector = self.highest_in_service()?;
        self.words[bank(apic::ISR, vector)].fetch_and(!(1 << (vector % 32)), Ordering::AcqRel);
        let tpr = self.words[word(apic::TPR)].load(Ordering::Acquire) & 0xff;
        let service = self.highest_in_service().unwrap_or(0) as u32 & 0xf0;
        let ppr = if tpr & 0xf0 >= service { tpr } else { service };
        self.words[word(apic::PPR)].store(ppr, Ordering::Release);
        Some(vector)
    }
}
