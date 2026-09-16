//! Inert four-level guest paging for the synthetic ring-0 profile.
//!
//! AMD APM volume 2 revision 3.44 chapter 5: ordinary 4 KiB leaf entries,
//! supervisor-only at every level. One aligned canonical-48 2 MiB virtual
//! window is supported (by default [0, 2 MiB)). EFER.NXE is clear in the
//! synthetic profile, so no entry sets
//! NX; guest paging does not enforce execute restrictions. NPT must do so.
//! PWT/PCD/PAT remain zero; this does not establish actual cache attributes.
//!
//! Caller-owned storage is not physically installed by this builder. Assigned
//! addresses are GPAs checked against the numeric AddressPolicy, not evidence
//! of allocation or NPT membership. Never modify tables used by a running CPU.

use crate::memory::address::{AddressError, AddressPolicy, PhysicalRange};

pub const PAGE_BYTES: usize = 4096;
pub const TABLE_COUNT: usize = 4;
pub const WINDOW_BYTES: u64 = 2 * 1024 * 1024;
const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

#[repr(C, align(4096))]
pub struct TableStorage(pub [[u8; PAGE_BYTES]; TABLE_COUNT]);

/// Both variants permit instruction fetch at the guest paging level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePermissions {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestPagesError {
    VirtualWindowOverflow,
    VirtualWindowMisaligned,
    VirtualWindowNonCanonical,
    VirtualAddressOutsideWindow,
    VirtualAddressMisaligned,
    Address(AddressError),
    TableArenaOverlap,
    AlreadyMapped,
    GuestPageAlias,
    /// Internal table/entry bounds failed; no unchecked table access is used.
    StorageBounds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub guest_address: u64,
    pub permissions: PagePermissions,
}

pub struct TableView<'a> {
    pub guest_address: u64,
    pub bytes: &'a [u8; PAGE_BYTES],
}

pub struct GuestPages<'a> {
    storage: &'a mut TableStorage,
    arena: PhysicalRange,
    policy: AddressPolicy,
    virtual_window_base: u64,
}

impl<'a> GuestPages<'a> {
    /// Validate the whole assigned GPA arena before clearing caller storage.
    pub fn new(
        storage: &'a mut TableStorage,
        arena_base: u64,
        policy: AddressPolicy,
    ) -> Result<Self, GuestPagesError> {
        Self::new_in_window(storage, arena_base, policy, 0)
    }

    /// Build one 2 MiB-aligned, canonical-48 window using four fixed tables.
    /// Validate the complete virtual window and assigned GPA arena before
    /// clearing caller storage. Overflow is checked before alignment.
    /// The highest window may end at `u64::MAX`; no exclusive end is needed.
    ///
    /// AMD APM vol. 2 rev. 3.44 sections 5.3.1 and 5.3.3: with CR4.LA57
    /// clear, bits 47:39, 38:30, and 29:21 select the three parent entries.
    /// This assigns GPAs only; the caller must provide separately owned NPT
    /// backing for tables and leaves before installing the root on a CPU.
    pub fn new_in_window(
        storage: &'a mut TableStorage,
        arena_base: u64,
        policy: AddressPolicy,
        virtual_window_base: u64,
    ) -> Result<Self, GuestPagesError> {
        let last = virtual_window_base
            .checked_add(WINDOW_BYTES - 1)
            .ok_or(GuestPagesError::VirtualWindowOverflow)?;
        if virtual_window_base & (WINDOW_BYTES - 1) != 0 {
            return Err(GuestPagesError::VirtualWindowMisaligned);
        }
        let canonical = |address: u64| ((address << 16) as i64 >> 16) as u64 == address;
        if !canonical(virtual_window_base) || !canonical(last) {
            return Err(GuestPagesError::VirtualWindowNonCanonical);
        }
        let arena = policy
            .validate(
                arena_base,
                (TABLE_COUNT * PAGE_BYTES) as u64,
                PAGE_BYTES as u64,
            )
            .map_err(GuestPagesError::Address)?;
        for page in &mut storage.0 {
            page.fill(0);
        }
        let mut pages = Self {
            storage,
            arena,
            policy,
            virtual_window_base,
        };
        for (parent, shift) in [39, 30, 21].into_iter().enumerate() {
            pages.write_entry(
                parent,
                ((virtual_window_base >> shift) & 511) as usize,
                arena_base + ((parent + 1) * PAGE_BYTES) as u64 | PRESENT | WRITE,
            )?;
        }
        Ok(pages)
    }

    pub const fn root_address(&self) -> u64 {
        self.arena.base()
    }

    pub const fn virtual_window_base(&self) -> u64 {
        self.virtual_window_base
    }

    /// All four tables are always present; out-of-range indices return None.
    pub fn table(&self, index: usize) -> Option<TableView<'_>> {
        self.storage.0.get(index).map(|bytes| TableView {
            guest_address: self.arena.base() + (index * PAGE_BYTES) as u64,
            bytes,
        })
    }

    /// Add one page atomically: errors leave every table byte unchanged.
    /// The fixed window bounds capacity to 512 leaves without allocation.
    /// No leaf may alias another leaf or any table page in this builder.
    pub fn map_page(
        &mut self,
        virtual_address: u64,
        guest_address: u64,
        permissions: PagePermissions,
    ) -> Result<(), GuestPagesError> {
        let offset = self.virtual_offset(virtual_address)?;
        if virtual_address & (PAGE_BYTES as u64 - 1) != 0 {
            return Err(GuestPagesError::VirtualAddressMisaligned);
        }
        let leaf = self
            .policy
            .validate(guest_address, PAGE_BYTES as u64, PAGE_BYTES as u64)
            .map_err(GuestPagesError::Address)?;
        if leaf.base() <= self.arena.last_byte() && self.arena.base() <= leaf.last_byte() {
            return Err(GuestPagesError::TableArenaOverlap);
        }
        let index = (offset / PAGE_BYTES as u64) as usize;
        if self.read_entry(3, index)? & PRESENT != 0 {
            return Err(GuestPagesError::AlreadyMapped);
        }
        for other in 0..512 {
            let entry = self.read_entry(3, other)?;
            if entry & PRESENT != 0 && entry & ADDRESS_MASK == guest_address {
                return Err(GuestPagesError::GuestPageAlias);
            }
        }
        let flags = PRESENT
            | if permissions == PagePermissions::ReadWrite {
                WRITE
            } else {
                0
            };
        self.write_entry(3, index, guest_address | flags)
    }

    pub fn translate(&self, virtual_address: u64) -> Result<Option<Translation>, GuestPagesError> {
        let offset = self.virtual_offset(virtual_address)?;
        let entry = self.read_entry(3, (offset / PAGE_BYTES as u64) as usize)?;
        if entry & PRESENT == 0 {
            return Ok(None);
        }
        Ok(Some(Translation {
            guest_address: (entry & ADDRESS_MASK) | (virtual_address & (PAGE_BYTES as u64 - 1)),
            permissions: if entry & WRITE != 0 {
                PagePermissions::ReadWrite
            } else {
                PagePermissions::ReadOnly
            },
        }))
    }

    fn virtual_offset(&self, address: u64) -> Result<u64, GuestPagesError> {
        address
            .checked_sub(self.virtual_window_base)
            .filter(|&offset| offset < WINDOW_BYTES)
            .ok_or(GuestPagesError::VirtualAddressOutsideWindow)
    }

    fn read_entry(&self, table: usize, index: usize) -> Result<u64, GuestPagesError> {
        let offset = index.checked_mul(8).ok_or(GuestPagesError::StorageBounds)?;
        let bytes = self
            .storage
            .0
            .get(table)
            .and_then(|page| page.get(offset..))
            .and_then(|tail| tail.first_chunk::<8>())
            .ok_or(GuestPagesError::StorageBounds)?;
        Ok(u64::from_le_bytes(*bytes))
    }

    fn write_entry(
        &mut self,
        table: usize,
        index: usize,
        value: u64,
    ) -> Result<(), GuestPagesError> {
        let offset = index.checked_mul(8).ok_or(GuestPagesError::StorageBounds)?;
        let bytes = self
            .storage
            .0
            .get_mut(table)
            .and_then(|page| page.get_mut(offset..))
            .and_then(|tail| tail.first_chunk_mut::<8>())
            .ok_or(GuestPagesError::StorageBounds)?;
        *bytes = value.to_le_bytes();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::address::EncryptionState;

    #[test]
    fn invalid_internal_entry_indices_refuse_without_mutating_storage() {
        let policy = AddressPolicy::new(
            48,
            EncryptionState::Unencrypted {
                encryption_bit: None,
            },
        )
        .unwrap();
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let mut pages = GuestPages::new(&mut storage, 0x100000, policy).unwrap();
        let before = pages.storage.0;
        for (table, entry) in [(TABLE_COUNT, 0), (usize::MAX, 0), (3, 512), (3, usize::MAX)] {
            assert_eq!(
                pages.read_entry(table, entry),
                Err(GuestPagesError::StorageBounds)
            );
            assert_eq!(
                pages.write_entry(table, entry, 0x1234),
                Err(GuestPagesError::StorageBounds)
            );
            assert_eq!(pages.storage.0, before);
        }
        pages.write_entry(3, 511, 0xfedc_ba98_7654_3210).unwrap();
        assert_eq!(pages.read_entry(3, 511), Ok(0xfedc_ba98_7654_3210));
    }
}
