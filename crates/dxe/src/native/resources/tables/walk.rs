//! Retained page-walk entries: translation, dependency closure and comparison.

use svmvisor_hypervisor::{
    host::paging::{self as host_paging, PagingConfig},
    memory::address::{ADDRESS_MASK, is_canonical_48},
};

use super::{EntryObservation, LeafObservation, RetainedWalks, TableError, TablePageObservation};

impl RetainedWalks {
    pub(super) fn entries(&self) -> Result<&[EntryObservation], TableError> {
        self.entries.get(..self.entry_count).ok_or(TableError::Bounds)
    }

    pub(super) fn pages(&self) -> Result<&[TablePageObservation], TableError> {
        self.table_pages.get(..self.page_count).ok_or(TableError::Bounds)
    }

    pub(super) fn translate(
        &mut self,
        config: PagingConfig,
        linear: u64,
        read: &mut impl FnMut(u64) -> Result<u64, TableError>,
    ) -> Result<host_paging::Translation, TableError> {
        let mut failure = None;
        let mut level = 4u8;
        let mut trace = [0u64; 4];
        let mut trace_count = 0usize;
        // With PCIDE set these CR3 bits are PCID bits, not fetch selectors.
        // Compatibility prepare still supports that mode, but owned-resource
        // preparation and the cache-facing table view conservatively refuse it.
        let mut fetch_index = if config.pcid { None } else { Some(((config.cr3 >> 3) & 3) as u8) };
        let translation = host_paging::translate(config, linear, |address| {
            let result = read(address).and_then(|value| {
                self.remember_entry(address, value)?;
                self.remember_fetch(address & !4095, level, fetch_index)?;
                *trace.get_mut(trace_count).ok_or(TableError::Bounds)? = address;
                trace_count += 1;
                level = level.checked_sub(1).ok_or(TableError::Bounds)?;
                // The walker will call again only for a validated non-leaf.
                // Leaf PCD/PWT therefore never become a table-fetch encoding.
                fetch_index = Some(((value >> 3) & 3) as u8);
                Ok(value)
            });
            match result {
                Ok(value) => Some(value),
                Err(error) => {
                    failure = Some(error);
                    None
                }
            }
        })
        .map_err(|_| failure.unwrap_or(TableError::Translation))?;
        // A successful complete walk validates every source role. Its final
        // entry is the leaf; preceding entries are non-leaves. Do not infer a
        // leaf from bit 7 alone (that bit is PAT in a 4 KiB PTE). Accumulate per
        // physical slot if recursive/shared tables use it in multiple roles.
        for (index, address) in
            trace.get(..trace_count).ok_or(TableError::Bounds)?.iter().enumerate()
        {
            let entry = self
                .entries
                .get_mut(..self.entry_count)
                .ok_or(TableError::Bounds)?
                .iter_mut()
                .find(|entry| entry.address == *address)
                .ok_or(TableError::Bounds)?;
            entry.allowed_set_bits |= if index + 1 == trace_count { 0x60 } else { 0x20 };
        }
        Ok(translation)
    }

    pub(super) fn remember_entry(&mut self, address: u64, value: u64) -> Result<(), TableError> {
        if address & 7 != 0 {
            return Err(TableError::Metadata);
        }
        if let Some(previous) = self.entries()?.iter().find(|entry| entry.address == address) {
            return if previous.value == value { Ok(()) } else { Err(TableError::Changed) };
        }
        self.remember_page(address & !4095)?;
        *self.entries.get_mut(self.entry_count).ok_or(TableError::Bounds)? =
            EntryObservation { address, value, allowed_set_bits: 0 };
        self.entry_count += 1;
        Ok(())
    }

    pub(super) fn remember_page(&mut self, page: u64) -> Result<(), TableError> {
        if !is_canonical_48(page) || !is_canonical_48(page | 4095) || page & 4095 != 0 {
            return Err(TableError::Context);
        }
        let pages = self.table_pages.get(..self.page_count).ok_or(TableError::Bounds)?;
        if pages.iter().any(|entry| entry.physical_page == page) {
            return Ok(());
        }
        *self.table_pages.get_mut(self.page_count).ok_or(TableError::Bounds)? =
            TablePageObservation { physical_page: page, ..TablePageObservation::default() };
        self.page_count += 1;
        Ok(())
    }

    pub(super) fn remember_fetch(
        &mut self,
        page: u64,
        level: u8,
        pat_index: Option<u8>,
    ) -> Result<(), TableError> {
        if !(1..=4).contains(&level) || pat_index.is_some_and(|index| index > 3) {
            return Err(TableError::Bounds);
        }
        self.remember_page(page)?;
        let observed = self
            .table_pages
            .get_mut(..self.page_count)
            .ok_or(TableError::Bounds)?
            .iter_mut()
            .find(|entry| entry.physical_page == page)
            .ok_or(TableError::Bounds)?;
        observed.levels |= 1u8
            .checked_shl(u32::from(level.checked_sub(1).ok_or(TableError::Bounds)?))
            .ok_or(TableError::Bounds)?;
        if let Some(index) = pat_index {
            observed.fetch_pat_indices |=
                1u8.checked_shl(u32::from(index)).ok_or(TableError::Bounds)?;
        }
        Ok(())
    }

    /// Walk the identity mapping of the root and EVERY entry-source page,
    /// including any new dependency pages those walks discover. This closes
    /// self/root mapping dependencies without recursion or an unchecked root.
    pub(super) fn close_dependencies(
        &mut self,
        config: PagingConfig,
        read: &mut impl FnMut(u64) -> Result<u64, TableError>,
    ) -> Result<(), TableError> {
        self.remember_page(config.cr3 & ADDRESS_MASK)?;
        let mut index = 0;
        while index < self.page_count {
            let page = self.table_pages.get(index).ok_or(TableError::Bounds)?.physical_page;
            let translation = self.translate(config, page, read)?;
            self.table_pages.get_mut(index).ok_or(TableError::Bounds)?.alias =
                LeafObservation::identity(page, translation)?;
            // Table sources need read access only. Effective RW is separately
            // required for GDT pages, which the eventual VMEXIT may update.
            index += 1;
        }
        Ok(())
    }

    pub(super) fn compare(&self, mut read: impl FnMut(u64) -> u64) -> Result<(), TableError> {
        for entry in self.entries()? {
            if read(entry.address) != entry.value {
                return Err(TableError::Changed);
            }
        }
        Ok(())
    }

    /// Preparation itself may set A/D while touching newly observed pages.
    /// Establish the final exact baseline only after all pages/GDT were read.
    /// Admit only 0->1 A on validated non-leaves or A/D on validated leaves;
    /// this is architectural compatibility, not proof of the change's cause.
    /// Address/permissions/presence and clearing any
    /// bit remain refusal. Subsequent comparisons permit no changes at all.
    pub(super) fn settle_accessed_dirty(
        &mut self,
        mut read: impl FnMut(u64) -> u64,
    ) -> Result<(), TableError> {
        let entries = self.entries.get_mut(..self.entry_count).ok_or(TableError::Bounds)?;
        for entry in entries {
            let current = read(entry.address);
            if (entry.value ^ current) & !entry.allowed_set_bits != 0 || entry.value & !current != 0
            {
                return Err(TableError::Changed);
            }
            entry.value = current;
        }
        Ok(())
    }
}
