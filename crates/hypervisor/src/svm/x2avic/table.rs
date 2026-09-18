//! Shared x2AVIC physical-ID table, APM2 3.44 15.29.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::{
    memory::address::{AddressError, AddressPolicy},
    svm::x2avic::{Error, MAX_ID, PAGE_BYTES},
};

/// One shared table per VM; one-to-one guest/host APIC IDs, no migration.
#[repr(C, align(4096))]
pub struct PhysicalIdTable {
    entries: [AtomicU64; 512],
}

impl PhysicalIdTable {
    pub const fn new() -> Self {
        Self { entries: [const { AtomicU64::new(0) }; 512] }
    }

    pub fn entry(&self, id: u16) -> Result<u64, Error> {
        self.entries.get(id as usize).map(|e| e.load(Ordering::Acquire)).ok_or(Error::InvalidId)
    }
    /// Whether entry `id` is exactly what `insert_stopped` publishes for
    /// `backing`: V set, IsRunning and reserved bits 61:52 clear, the backing
    /// page in 51:12 and host physical APIC ID `id` in 11:0 (APM2 rev3.44
    /// Figure 15-17 and Table 15-23 p572). Read-only; arm checks its own
    /// entry with this before any irreversible change.
    pub fn is_stopped_entry(&self, id: u16, backing: u64) -> bool {
        id <= MAX_ID
            && backing & !0x000f_ffff_ffff_f000 == 0
            && self.entry(id) == Ok((1 << 63) | backing | u64::from(id))
    }

    /// Populate only before table publication. Each backing page must be
    /// initialized and pinned WB before the valid bit becomes visible.
    pub fn insert_stopped(
        &mut self,
        id: u16,
        backing: u64,
        policy: &AddressPolicy,
    ) -> Result<(), Error> {
        if id > MAX_ID {
            return Err(Error::InvalidId);
        }
        if backing == 0 {
            return Err(Error::Address(AddressError::EmptyRange));
        }
        policy.validate(backing, PAGE_BYTES as u64, PAGE_BYTES as u64).map_err(Error::Address)?;
        if self.entries[id as usize].load(Ordering::Acquire) != 0 {
            return Err(Error::Occupied);
        }
        for entry in &self.entries {
            if entry.load(Ordering::Acquire) & 0x000f_ffff_ffff_f000 == backing {
                return Err(Error::AliasedPages);
            }
        }
        self.entries[id as usize].store((1 << 63) | backing | id as u64, Ordering::Release);
        Ok(())
    }
    /// Assigned-to-core status includes host VM-exit service; clearing this
    /// bit does not itself drain in-flight IPI references.
    pub fn set_running(&self, id: u16, running: bool) -> Result<(), Error> {
        if id > MAX_ID || self.entries[id as usize].load(Ordering::Acquire) & (1 << 63) == 0 {
            return Err(Error::InvalidId);
        }
        if running {
            self.entries[id as usize].fetch_or(1 << 62, Ordering::AcqRel);
        } else {
            self.entries[id as usize].fetch_and(!(1 << 62), Ordering::AcqRel);
        }
        Ok(())
    }
}

impl Default for PhysicalIdTable {
    fn default() -> Self {
        Self::new()
    }
}
