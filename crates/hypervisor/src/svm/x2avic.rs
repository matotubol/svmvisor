//! Exclusive ordinary-SVM x2AVIC memory formats and stopped-state operations.
//!
//! AMD APM2 3.44 15.29, Tables15-22..29; 16.10 and Table16-2.
//! This module does not allocate, map, enable physical x2APIC, ring doorbells,
//! acknowledge physical sources or establish a safe loader continuation.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::address::{AddressError, AddressPolicy};

pub const PAGE_BYTES: usize = 4096;
pub const MAX_ID: u16 = 511;
pub const ENABLE_BITS: u64 = 3 << 30;
pub const NATIVE_CONTROL: u64 = ENABLE_BITS | (1 << 24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    MissingCapability,
    Address(AddressError),
    InvalidId,
    AliasedPages,
    Occupied,
    InvalidOffset,
    InvalidVector,
    MixedTrigger,
    UnsupportedVersion,
    InvalidExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X2AvicCapabilities(());
impl X2AvicCapabilities {
    /// Evidence must be sampled on every admitted CPU before activation.
    pub fn admit(cpuid1_ecx: u32, svm_edx: u32) -> Result<Self, Error> {
        let required = 1 | (1 << 13) | (1 << 18); // NPT, AVIC, x2AVIC
        if cpuid1_ecx & (1 << 21) == 0 || svm_edx & required != required {
            return Err(Error::MissingCapability);
        }
        Ok(Self(()))
    }
}

/// Address-checked entry configuration, not allocation or WB/lifetime proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeX2AvicProfile {
    backing: u64,
    table: u64,
    maximum: u16,
}
impl NativeX2AvicProfile {
    pub fn new(
        _capabilities: X2AvicCapabilities,
        backing: u64,
        table: u64,
        maximum: u16,
        policy: &AddressPolicy,
    ) -> Result<Self, Error> {
        if maximum > MAX_ID { return Err(Error::InvalidId); }
        for address in [backing, table] {
            if address == 0 { return Err(Error::Address(AddressError::EmptyRange)); }
            policy.validate(address, PAGE_BYTES as u64, PAGE_BYTES as u64).map_err(Error::Address)?;
        }
        if backing == table { return Err(Error::AliasedPages); }
        Ok(Self { backing, table, maximum })
    }
    pub const fn backing_address(self) -> u64 { self.backing }
    pub const fn table_address(self) -> u64 { self.table }
    pub const fn maximum_id(self) -> u16 { self.maximum }
    pub const fn table_control(self) -> u64 { self.table | self.maximum as u64 }
}

/// Hardware sees 32-bit register slots at 16-byte strides. Atomic accesses
/// preserve hardware IRR updates; WB mapping and hardware lifetime are external.
#[repr(C, align(4096))]
pub struct BackingPage { words: [AtomicU32; PAGE_BYTES / 4] }
impl Default for BackingPage { fn default() -> Self { Self::new() } }
impl BackingPage {
    pub const fn new() -> Self { Self { words: [const { AtomicU32::new(0) }; PAGE_BYTES / 4] } }

    fn register(offset: u16) -> Result<usize, Error> {
        if offset as usize >= PAGE_BYTES || offset & 15 != 0 { return Err(Error::InvalidOffset); }
        Ok(offset as usize / 4)
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
    /// Caller quiesces every producer/table reference before acquiring &mut.
    pub fn reset_stopped(&mut self, id: u32, version: u32) -> Result<(), Error> {
        if id > MAX_ID as u32 { return Err(Error::InvalidId); }
        if version != 0x0005_0010 { return Err(Error::UnsupportedVersion); }
        for word in &mut self.words { *word.get_mut() = 0; }
        *self.words[0x20 / 4].get_mut() = id;
        *self.words[0x30 / 4].get_mut() = version;
        *self.words[0xd0 / 4].get_mut() = ((id >> 4) << 16) | (1 << (id & 15));
        *self.words[0xf0 / 4].get_mut() = 0xff;
        for offset in (0x320..=0x370).step_by(16) { *self.words[offset / 4].get_mut() = 1 << 16; }
        Ok(())
    }
    /// Apply architectural INIT to an already-bound x2APIC backing page.
    /// APM2 rev3.44 16.3.2/Table16-2, 16.10: preserve identity/version and
    /// derived x2APIC LDR; APIC_BASE mode is owned outside this page.
    ///
    /// The target guest is stopped and the caller owns local registers, timer
    /// cancellation and physical/direct-source retirement. Page/table identity
    /// remains stable. Remote IRR publication may race the atomic per-bank
    /// clears: a publication preceding its bank's clear is discarded by INIT;
    /// one following it remains pending. This is not a simultaneous snapshot,
    /// fabric drain, retarget, or permission to reclaim the backing page.
    ///
    /// GA does not write TMR. Before entering this target again the source
    /// owner must restore trigger metadata for retained direct level routes,
    /// including an IRQ arriving after the IRR clear. No guest register reader
    /// or local dispatch/EOI may run during this operation. Validation is
    /// read-only; after it succeeds all stores are infallible and bounded.
    pub fn reset_after_init_stopped(&self) -> Result<(), Error> {
        let id = self.words[0x20 / 4].load(Ordering::Acquire);
        let version = self.words[0x30 / 4].load(Ordering::Acquire);
        if id > MAX_ID as u32 { return Err(Error::InvalidId); }
        if version != 0x0005_0010 { return Err(Error::UnsupportedVersion); }
        self.words[0xf0 / 4].store(0xff, Ordering::Release);
        for offset in (0x320..=0x370).step_by(16) {
            self.words[offset / 4].store(1 << 16, Ordering::Release);
        }
        for offset in [0x80, 0x90, 0xa0, 0xb0, 0xc0, 0x280, 0x300, 0x310,
            0x380, 0x390, 0x3e0] {
            self.words[offset / 4].store(0, Ordering::Release);
        }
        // x2APIC LDR is an identity-derived read-only register, not legacy zero.
        self.words[0xd0 / 4].store(((id >> 4) << 16) | (1 << (id & 15)), Ordering::Release);
        for base in [0x100, 0x180, 0x200] {
            for bank in 0..8 { self.words[base / 4 + bank * 4].store(0, Ordering::Release); }
        }
        Ok(())
    }

    fn bit(&self, base: usize, vector: u8) -> bool {
        self.words[base / 4 + (vector as usize / 32) * 4].load(Ordering::Acquire) & (1 << (vector % 32)) != 0
    }
    pub fn is_pending(&self, vector: u8) -> bool { self.bit(0x200, vector) }
    pub fn is_in_service(&self, vector: u8) -> bool { self.bit(0x100, vector) }
    pub fn is_level(&self, vector: u8) -> bool { self.bit(0x180, vector) }
    /// Atomic publication to AVIC. The source owner must serialize differing
    /// trigger types for a vector. INIT may discard a racing publication at
    /// its bank clear; level-source metadata needs the reset owner's separate
    /// coordination. No delivery or EOI is claimed: return true only when IRR
    /// was newly set. Ringing a doorbell or
    /// returning through VMRUN is the runtime's separate responsibility.
    pub fn enqueue(&self, vector: u8, level: bool) -> Result<bool, Error> {
        self.prepare_trigger_stopped(vector, level)?;
        let lane = (vector as usize / 32) * 4;
        let bit = 1 << (vector % 32);
        Ok(self.words[0x200 / 4 + lane].fetch_or(bit, Ordering::AcqRel) & bit == 0)
    }
    /// Prepare TMR before enabling an IOMMU GA route; GA hardware writes IRR
    /// without updating TMR. Caller excludes all writers of this vector and
    /// guest execution for the whole source publication transaction. This
    /// atomic update preserves unrelated vectors and never enqueues an IRQ.
    /// Neither this check nor an IOMMU CompletionWait proves writer drainage.
    pub fn prepare_trigger_stopped(&self, vector: u8, level: bool) -> Result<(), Error> {
        if vector < 16 { return Err(Error::InvalidVector); }
        if (self.is_pending(vector) || self.is_in_service(vector)) && self.is_level(vector) != level {
            return Err(Error::MixedTrigger);
        }
        let lane = (vector as usize / 32) * 4;
        let bit = 1 << (vector % 32);
        let tmr = &self.words[0x180 / 4 + lane];
        if level { tmr.fetch_or(bit, Ordering::Release); } else { tmr.fetch_and(!bit, Ordering::Release); }
        Ok(())
    }
    /// Stopped guest inspection; hardware may still add IRR, but no other CPU
    /// may dispatch into or reset this page during this bounded ISR scan.
    pub fn highest_in_service(&self) -> Option<u8> {
        for bank in (0..8).rev() {
            let word = self.words[0x100 / 4 + bank * 4].load(Ordering::Acquire);
            if word != 0 { return Some((bank * 32 + 31 - word.leading_zeros() as usize) as u8); }
        }
        None
    }
    /// Software completion of an intercepted EOI, never a physical EOI.
    /// APM15.29.3.1 p569,16.6.4 pp650-651. Call only for an EOI intercepted
    /// before AVIC's side effect; an AVIC_NOACCEL EOI trap already cleared ISR.
    /// The returned trigger bit lets the source owner perform level completion.
    pub fn eoi_stopped(&mut self) -> Option<(u8, bool)> {
        let vector = self.highest_in_service()?;
        let level = self.is_level(vector);
        self.words[0x100 / 4 + (vector as usize / 32) * 4].fetch_and(!(1 << (vector % 32)), Ordering::AcqRel);
        let tpr = self.words[0x80 / 4].load(Ordering::Acquire) & 0xff;
        let service = self.highest_in_service().unwrap_or(0) as u32 & 0xf0;
        let ppr = if tpr & 0xf0 >= service { tpr } else { service };
        self.words[0xa0 / 4].store(ppr, Ordering::Release);
        Some((vector, level))
    }
}

/// One shared table per VM; one-to-one guest/host APIC IDs, no migration.
#[repr(C, align(4096))]
pub struct PhysicalIdTable { entries: [AtomicU64; 512] }
impl Default for PhysicalIdTable { fn default() -> Self { Self::new() } }
impl PhysicalIdTable {
    pub const fn new() -> Self { Self { entries: [const { AtomicU64::new(0) }; 512] } }
    /// Populate only before table publication. Each backing page must be
    /// initialized and pinned WB before the valid bit becomes visible.
    pub fn insert_stopped(&mut self, id: u16, backing: u64, policy: &AddressPolicy) -> Result<(), Error> {
        if id > MAX_ID { return Err(Error::InvalidId); }
        if backing == 0 { return Err(Error::Address(AddressError::EmptyRange)); }
        policy.validate(backing, PAGE_BYTES as u64, PAGE_BYTES as u64).map_err(Error::Address)?;
        if self.entries[id as usize].load(Ordering::Acquire) != 0 { return Err(Error::Occupied); }
        for entry in &self.entries {
            if entry.load(Ordering::Acquire) & 0x000f_ffff_ffff_f000 == backing { return Err(Error::AliasedPages); }
        }
        self.entries[id as usize].store((1 << 63) | backing | id as u64, Ordering::Release);
        Ok(())
    }
    /// Assigned-to-core status includes host VM-exit service; clearing this
    /// bit does not itself drain in-flight IPI or IOMMU references.
    pub fn set_running(&self, id: u16, running: bool) -> Result<(), Error> {
        if id > MAX_ID || self.entries[id as usize].load(Ordering::Acquire) & (1 << 63) == 0 { return Err(Error::InvalidId); }
        if running { self.entries[id as usize].fetch_or(1 << 62, Ordering::AcqRel); }
        else { self.entries[id as usize].fetch_and(!(1 << 62), Ordering::AcqRel); }
        Ok(())
    }
    pub fn entry(&self, id: u16) -> Result<u64, Error> {
        self.entries.get(id as usize).map(|e| e.load(Ordering::Acquire)).ok_or(Error::InvalidId)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvicExit {
    IncompleteIpi { icr: u64, reason: u32, index: Option<u16> },
    NoAcceleration { offset: u16, write: bool, eoi_vector: Option<u8> },
}
impl AvicExit {
    /// Decode only, without inferring completion or replaying partial IPIs.
    /// Secure-AVIC-only reason5 and reserved encodings are outside this profile.
    pub fn decode(code: u64, info1: u64, info2: u64) -> Result<Self, Error> {
        match code {
            0x401 => {
                let reason = (info2 >> 32) as u32;
                if reason > 4 || info2 & 0xffff_f000 != 0 { return Err(Error::InvalidExit); }
                let index = if (1..=3).contains(&reason) { Some((info2 & 0xfff) as u16) } else {
                    if info2 & 0xfff != 0 { return Err(Error::InvalidExit); }
                    None
                };
                Ok(Self::IncompleteIpi { icr: info1, reason, index })
            }
            0x402 => {
                if info1 & !((1 << 32) | 0xff0) != 0 { return Err(Error::InvalidExit); }
                let offset = (info1 & 0xff0) as u16;
                let write = info1 & (1 << 32) != 0;
                let eoi_vector = if write && offset == 0xb0 {
                    if info2 & !0xff != 0 || info2 < 16 { return Err(Error::InvalidExit); }
                    Some(info2 as u8)
                } else { None }; // Undefined EXITINFO2 is not checked.
                Ok(Self::NoAcceleration { offset, write, eoi_vector })
            }
            _ => Err(Error::InvalidExit),
        }
    }
}
