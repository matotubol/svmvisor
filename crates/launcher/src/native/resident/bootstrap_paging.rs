//! Retained AP bootstrap identity tables, distinct from each private host root.
//!
//! AMD APM2 rev3.44 5.3.3/Figure5-17 and 5.4: four-level 4KiB paging,
//! supervisor leaves and PAT selection. This bounded prepublication builder
//! does not allocate or borrow firmware tables. Its caller proves WB runtime
//! backing, initializes it once, and retains it until the AP guest replaces CR3.

use svmvisor_hypervisor::memory::address::{ADDRESS_MASK, PAGE_BYTES};

pub const TABLE_PAGES: usize = 64;
const PAGE: u64 = PAGE_BYTES as u64;

#[repr(C, align(4096))]
pub struct BootstrapPaging {
    tables: [[u64; 512]; TABLE_PAGES],
    base: u64,
    used: usize,
}

const _: () = assert!(core::mem::offset_of!(BootstrapPaging, tables) == 0);

impl BootstrapPaging {
    pub const fn empty() -> Self {
        Self { tables: [[0; 512]; TABLE_PAGES], base: 0, used: 0 }
    }

    /// Root must fit the protected-mode trampoline's 32-bit MOV CR3.
    /// Call only before publication; a failed construction is never runnable.
    pub fn initialize(&mut self, base: u64) -> Result<(), Error> {
        if base == 0
            || base & (PAGE - 1) != 0
            || base.checked_add(core::mem::size_of::<Self>() as u64).is_none_or(|end| end > 1 << 32)
        {
            return Err(Error::Address);
        }
        for table in &mut self.tables {
            table.fill(0);
        }
        self.base = base;
        self.used = 1;
        Ok(())
    }

    /// Admit only the existing native profile's low 40-bit physical aperture.
    /// Every leaf selects PAT entry 0; the bootstrap maps no device pages.
    /// Identical repeated leaves are allowed; conflicting leaves are refused.
    pub fn map_page(&mut self, page: u64, writable: bool, executable: bool) -> Result<(), Error> {
        if self.used == 0 || page >= 1 << 40 || page & (PAGE - 1) != 0 {
            return Err(Error::Address);
        }
        let mut table = 0;
        for shift in [39, 30, 21] {
            let index = ((page >> shift) & 511) as usize;
            let entry = self.tables[table][index];
            table = if entry == 0 {
                if self.used == TABLE_PAGES {
                    return Err(Error::Capacity);
                }
                let next = self.used;
                self.used += 1;
                self.tables[table][index] = (self.base + next as u64 * PAGE) | 3;
                next
            } else {
                ((entry & ADDRESS_MASK) - self.base) as usize / PAGE as usize
            };
        }
        let index = ((page >> 12) & 511) as usize;
        let leaf = page | 1 | if writable { 2 } else { 0 } | if executable { 0 } else { 1 << 63 };
        if self.tables[table][index] != 0 && self.tables[table][index] != leaf {
            return Err(Error::Conflict);
        }
        self.tables[table][index] = leaf;
        Ok(())
    }

    /// Inert reader for the shared paging walker; no arbitrary physical access.
    pub fn read(&self, address: u64) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        if self.used == 0 || offset & 7 != 0 || offset >= self.used as u64 * PAGE {
            return None;
        }
        Some(self.tables[(offset / PAGE) as usize][((offset % PAGE) / 8) as usize])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Address,
    Capacity,
    Conflict,
}
