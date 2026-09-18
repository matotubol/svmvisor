//! The strict synthetic `Npt`: explicitly mapped 4 KiB leaves with W^X permissions.

use crate::{
    arch::x86_64::capabilities::EvidenceFlag,
    memory::{
        address::{AddressError, AddressPolicy, PhysicalRange},
        npt::table::{
            ADDRESS_MASK, NX, NptEvidence, PAGE_BYTES, PRESENT, TABLE_COUNT, TableStorage,
            TableView, USER, WRITE, indices,
        },
    },
};

pub struct Npt<'a> {
    storage: &'a mut TableStorage,
    arena: PhysicalRange,
    policy: AddressPolicy,
    guest_bits: u8,
    used: usize,
    levels: [u8; TABLE_COUNT],
}

impl<'a> Npt<'a> {
    pub fn new(
        storage: &'a mut TableStorage,
        arena_base: u64,
        policy: AddressPolicy,
        guest_address_bits: u8,
        evidence: NptEvidence,
    ) -> Result<Self, NptError> {
        if evidence.nx_supported != EvidenceFlag::Set
            || evidence.host_nxe != EvidenceFlag::Set
            || evidence.host_four_level != EvidenceFlag::Set
        {
            return Err(NptError::RequiredModeNotEstablished);
        }
        // This initial policy deliberately limits GPA width to host width too.
        if !(32..=48).contains(&guest_address_bits) || guest_address_bits > policy.physical_bits() {
            return Err(NptError::InvalidGuestWidth);
        }
        let arena = policy
            .validate(arena_base, (TABLE_COUNT * PAGE_BYTES) as u64, PAGE_BYTES as u64)
            .map_err(NptError::Address)?;
        for page in &mut storage.0 {
            page.fill(0);
        }
        Ok(Self {
            storage,
            arena,
            policy,
            guest_bits: guest_address_bits,
            used: 1,
            levels: [0; TABLE_COUNT],
        })
    }

    pub const fn root_address(&self) -> u64 {
        self.arena.base()
    }
    pub const fn used_tables(&self) -> usize {
        self.used
    }

    /// Only assigned, reachable tables are exported. Unused storage is zero.
    pub fn table(&self, index: usize) -> Option<TableView<'_>> {
        if index >= self.used {
            return None;
        }
        Some(TableView {
            physical_address: self.table_address(index).ok()?,
            bytes: self.storage.0.get(index)?,
        })
    }

    /// Map exactly one page. All rejection checks precede any table mutation;
    /// existing mappings cannot be replaced, including identical duplicates.
    pub fn map_page(
        &mut self,
        gpa: u64,
        hpa: u64,
        permissions: PagePermissions,
    ) -> Result<(), NptError> {
        self.check_guest(gpa)?;
        if gpa & 4095 != 0 {
            return Err(NptError::GuestAddressMisaligned);
        }
        let leaf = self
            .policy
            .validate(hpa, PAGE_BYTES as u64, PAGE_BYTES as u64)
            .map_err(NptError::Address)?;
        if leaf.base() <= self.arena.last_byte() && self.arena.base() <= leaf.last_byte() {
            return Err(NptError::TableArenaOverlap);
        }
        if self.used == 0 || self.used > TABLE_COUNT {
            return Err(NptError::StorageBounds);
        }
        let [pml4, pdpt, pd, leaf_index] = indices(gpa);
        let parents = [pml4, pdpt, pd];
        let mut table = 0;
        let mut needed = 0;
        for (depth, index) in parents.iter().enumerate() {
            let entry = self.read(table, *index)?;
            if entry & PRESENT == 0 {
                needed = 3 - depth;
                break;
            }
            table = self.child_index(entry, depth as u8 + 1)?;
        }
        if needed == 0 && self.read(table, leaf_index)? & PRESENT != 0 {
            return Err(NptError::AlreadyMapped);
        }
        // Keep the synthetic profile free of HPA aliases, including separate
        // writable/executable aliases that would bypass per-leaf W^X policy.
        for existing_table in 0..self.used {
            if *self.levels.get(existing_table).ok_or(NptError::StorageBounds)? != 3 {
                continue;
            }
            for index in 0..512 {
                let entry = self.read(existing_table, index)?;
                if entry & PRESENT != 0 && entry & ADDRESS_MASK == hpa {
                    return Err(NptError::HostPageAlias);
                }
            }
        }
        if self.used + needed > TABLE_COUNT {
            return Err(NptError::TablesExhausted);
        }

        // Stage every link and the leaf without changing storage or allocation
        // metadata. A path writes at most one entry in each distinct table.
        let mut updates = [None; TABLE_COUNT];
        let mut levels = self.levels;
        let mut used = self.used;
        table = 0;
        for (depth, index) in parents.iter().enumerate() {
            let entry = self.read(table, *index)?;
            table = if entry & PRESENT != 0 {
                self.child_index(entry, depth as u8 + 1)?
            } else {
                let child = used;
                *levels.get_mut(child).ok_or(NptError::StorageBounds)? = depth as u8 + 1;
                let update = updates.get_mut(table).ok_or(NptError::StorageBounds)?;
                if update.is_some() {
                    return Err(NptError::StorageBounds);
                }
                *update = Some(EntryUpdate {
                    index: *index,
                    value: self.table_address(child)? | PRESENT | WRITE | USER,
                });
                used += 1;
                child
            };
        }
        let flags = PRESENT
            | USER
            | match permissions {
                PagePermissions::ReadOnly => NX,
                PagePermissions::ReadWrite => WRITE | NX,
                PagePermissions::ReadExecute => 0,
            };
        let update = updates.get_mut(table).ok_or(NptError::StorageBounds)?;
        if update.is_some() {
            return Err(NptError::StorageBounds);
        }
        *update = Some(EntryUpdate { index: leaf_index, value: hpa | flags });

        // Resolve all mutable destinations before committing any bytes. Each
        // slot comes from a separate page borrow; no aliasing or unchecked
        // access is needed, and no fallible operation remains after this pass.
        let mut destinations: [Option<&mut [u8; 8]>; TABLE_COUNT] = [const { None }; TABLE_COUNT];
        for ((page, update), destination) in
            self.storage.0.iter_mut().zip(updates.iter()).zip(destinations.iter_mut())
        {
            if let Some(update) = update {
                *destination = Some(entry_mut(page, update.index)?);
            }
        }
        for (destination, update) in destinations.into_iter().zip(updates) {
            if let (Some(destination), Some(update)) = (destination, update) {
                *destination = update.value.to_le_bytes();
            }
        }
        self.used = used;
        self.levels = levels;
        Ok(())
    }

    /// Inspect this builder's intended mappings, without CPU access or a TLB.
    /// Does not evaluate guest page tables or guest-side access restrictions.
    pub fn translate(&self, gpa: u64) -> Result<Option<Translation>, NptError> {
        self.check_guest(gpa)?;
        let [pml4, pdpt, pd, leaf_index] = indices(gpa);
        let mut table = 0;
        for (depth, index) in [pml4, pdpt, pd].into_iter().enumerate() {
            let entry = self.read(table, index)?;
            if entry & PRESENT == 0 {
                return Ok(None);
            }
            table = self.child_index(entry, depth as u8 + 1)?;
        }
        let entry = self.read(table, leaf_index)?;
        if entry & PRESENT == 0 {
            return Ok(None);
        }
        let permissions = if entry & NX == 0 {
            PagePermissions::ReadExecute
        } else if entry & WRITE != 0 {
            PagePermissions::ReadWrite
        } else {
            PagePermissions::ReadOnly
        };
        Ok(Some(Translation { host_address: (entry & ADDRESS_MASK) | (gpa & 4095), permissions }))
    }

    fn check_guest(&self, gpa: u64) -> Result<(), NptError> {
        if gpa >> self.guest_bits != 0 { Err(NptError::GuestAddressOutsideWidth) } else { Ok(()) }
    }

    fn table_address(&self, table: usize) -> Result<u64, NptError> {
        if table >= TABLE_COUNT {
            return Err(NptError::StorageBounds);
        }
        Ok(self.arena.base() + (table * PAGE_BYTES) as u64)
    }
    // Links only originate from this builder; no mutable table view is exposed.
    // Still validate them so a corrupted link cannot become a panic or escape
    // this arena, and depth mismatches/cycles refuse before mapping mutation.
    fn child_index(&self, entry: u64, level: u8) -> Result<usize, NptError> {
        let offset =
            (entry & ADDRESS_MASK).checked_sub(self.arena.base()).ok_or(NptError::StorageBounds)?;
        let index =
            usize::try_from(offset / PAGE_BYTES as u64).map_err(|_| NptError::StorageBounds)?;
        if index >= self.used || self.levels.get(index).copied() != Some(level) {
            return Err(NptError::StorageBounds);
        }
        Ok(index)
    }
    fn read(&self, table: usize, index: usize) -> Result<u64, NptError> {
        let offset = index.checked_mul(8).ok_or(NptError::StorageBounds)?;
        let bytes = self
            .storage
            .0
            .get(table)
            .and_then(|page| page.get(offset..))
            .and_then(|tail| tail.first_chunk::<8>())
            .ok_or(NptError::StorageBounds)?;
        Ok(u64::from_le_bytes(*bytes))
    }
}

#[derive(Clone, Copy)]
struct EntryUpdate {
    index: usize,
    value: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub host_address: u64,
    pub permissions: PagePermissions,
}

/// Read permission is mandatory for any present x86 page. There is no RWX
/// variant; this is per-mapping policy, not proof against aliases elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePermissions {
    ReadOnly,
    ReadWrite,
    ReadExecute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NptError {
    RequiredModeNotEstablished,
    InvalidGuestWidth,
    GuestAddressOutsideWidth,
    GuestAddressMisaligned,
    Address(AddressError),
    TableArenaOverlap,
    AlreadyMapped,
    HostPageAlias,
    TablesExhausted,
    /// A bounded table, entry or builder-owned child link is invalid.
    StorageBounds,
}

fn entry_mut(page: &mut [u8; PAGE_BYTES], index: usize) -> Result<&mut [u8; 8], NptError> {
    let offset = index.checked_mul(8).ok_or(NptError::StorageBounds)?;
    page.get_mut(offset..)
        .and_then(|tail| tail.first_chunk_mut::<8>())
        .ok_or(NptError::StorageBounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::address::EncryptionState;

    fn policy() -> AddressPolicy {
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
    }
    fn evidence() -> NptEvidence {
        NptEvidence {
            nx_supported: EvidenceFlag::Set,
            host_nxe: EvidenceFlag::Set,
            host_four_level: EvidenceFlag::Set,
        }
    }

    #[test]
    fn invalid_internal_entry_indices_refuse_without_partial_write() {
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let npt = Npt::new(&mut storage, 0x100000, policy(), 48, evidence()).unwrap();
        for (table, index) in [(TABLE_COUNT, 0), (usize::MAX, 0), (0, 512), (0, usize::MAX)] {
            assert_eq!(npt.read(table, index), Err(NptError::StorageBounds));
        }
        assert!(npt.table(usize::MAX).is_none());
        assert_eq!(npt.table_address(usize::MAX), Err(NptError::StorageBounds));
        let mut page = [0xa5; PAGE_BYTES];
        for index in [512, usize::MAX] {
            assert_eq!(entry_mut(&mut page, index), Err(NptError::StorageBounds));
            assert!(page.iter().all(|byte| *byte == 0xa5));
        }
        *entry_mut(&mut page, 511).unwrap() = 0xfedc_ba98_7654_3210u64.to_le_bytes();
        assert_eq!(&page[4088..], &0xfedc_ba98_7654_3210u64.to_le_bytes());
    }

    #[test]
    fn invalid_child_links_refuse_translation_and_mapping_without_mutation() {
        for child in [0, 0x100000, 0x104000, 0x108000, ADDRESS_MASK] {
            let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
            let mut npt = Npt::new(&mut storage, 0x100000, policy(), 48, evidence()).unwrap();
            npt.map_page(0, 0x200000, PagePermissions::ReadOnly).unwrap();
            *entry_mut(&mut npt.storage.0[0], 0).unwrap() = (child | 7).to_le_bytes();
            let before = npt.storage.0;
            let levels = npt.levels;
            let used = npt.used;
            assert_eq!(npt.translate(0), Err(NptError::StorageBounds));
            assert_eq!(
                npt.map_page(0x1000, 0x300000, PagePermissions::ReadWrite),
                Err(NptError::StorageBounds)
            );
            assert_eq!(npt.storage.0, before);
            assert_eq!(npt.levels, levels);
            assert_eq!(npt.used, used);
        }
    }

    #[test]
    fn corrupted_allocation_count_refuses_before_any_mapping_change() {
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let mut npt = Npt::new(&mut storage, 0x100000, policy(), 48, evidence()).unwrap();
        for used in [0, TABLE_COUNT + 1, usize::MAX] {
            npt.used = used;
            let before = npt.storage.0;
            assert_eq!(
                npt.map_page(0, 0x200000, PagePermissions::ReadWrite),
                Err(NptError::StorageBounds)
            );
            assert_eq!(npt.storage.0, before);
            assert_eq!(npt.used, used);
        }
    }
}
