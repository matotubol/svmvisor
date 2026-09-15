//! Bounded, inert four-level nested page tables for a synthetic single CPU.
//!
//! AMD APM vol. 2 rev. 3.44 sections 5.4 and 15.25: every present entry has
//! U/S=1, parent entries permit writes/execution, leaf entries restrict them.
//! NX requires supported NX and host EFER.NXE=1. The caller supplies evidence
//! of host four-level long mode; GMET, SSS and encrypted modes must be disabled.
//! PWT/PCD/PAT are zero (PAT index zero), which does not establish WB memory.
//! Allocation, actual copying to assigned physical addresses, ownership, PAT/
//! MTRR validation, guest state, TLB invalidation and enabling NPT are external.
//! This builder must never edit tables used by a running CPU.

use crate::address::{AddressError, AddressPolicy, PhysicalRange};
use crate::capabilities::EvidenceFlag;

pub const PAGE_BYTES: usize = 4096;
pub const TABLE_COUNT: usize = 8;
const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const USER: u64 = 4;
const NX: u64 = 1 << 63;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Caller-owned backing storage. Its virtual address does not establish its
/// assigned physical address. Construction clears it only after validation.
#[repr(C, align(4096))]
pub struct TableStorage(pub [[u8; PAGE_BYTES]; TABLE_COUNT]);

/// Private stopped-CPU copy of the identity root, with one extra low PT.
/// The original eight pages keep their indices; only child pointers relocate.
#[repr(C, align(4096))]
pub struct LowMemoryNptStorage(pub [[u8; PAGE_BYTES]; TABLE_COUNT + 1]);

impl LowMemoryNptStorage {
    pub const fn empty() -> Self { Self([[0; PAGE_BYTES]; TABLE_COUNT + 1]) }

    /// APM2 5.4/15.25: remove permissions without changing any cache type.
    /// Both roots must be private to this stopped CPU. The caller owns the
    /// subsequent NCR3 switch and full TLB invalidation. Rebuild from the
    /// current root each time, preserving dynamic ECAM permissions and holes.
    /// On failure this destination is unusable; the source is never changed.
    pub fn prepare(&mut self, source: &TableStorage, source_base: u64,
        destination_base: u64) -> Result<(), IdentityNptError>
    {
        use IdentityNptError as E;
        if (source_base | destination_base) & 4095 != 0
            || source_base == destination_base { return Err(E::StorageBounds); }
        for index in 0..TABLE_COUNT { self.0[index].copy_from_slice(&source.0[index]); }
        self.0[TABLE_COUNT].fill(0);
        let mut seen = 0u16;
        self.relocate(0, 4, source_base, destination_base, &mut seen)?;
        let child = |entry: u64| -> Result<usize, IdentityNptError> {
            let address = entry & ADDRESS_MASK;
            let offset = address.checked_sub(destination_base).ok_or(E::StorageBounds)?;
            if entry & 0x87 != 7 || offset / 4096 >= TABLE_COUNT as u64 {
                return Err(E::StorageBounds);
            }
            Ok((offset / 4096) as usize)
        };
        let pdpt = child(self.entry(0, 0))?;
        let pd = child(self.entry(pdpt, 0))?;
        let low = self.entry(pd, 0);
        if low == 0 { return Ok(()); }
        if low & 0x80 == 0 {
            let pt = child(low)?;
            for index in 0..256 { self.put(pt, index, 0); }
            return Ok(());
        }
        if low & ADDRESS_MASK != 0 || low & !0xe7 != 0 || low & 0x87 != 0x87 {
            return Err(E::StorageBounds);
        }
        // First MiB is absent. The adjacent MiB retains original RWX and A/D.
        for index in 256..512 {
            self.put(TABLE_COUNT, index, ((index as u64) << 12) | (low & !0x80));
        }
        self.put(pd, 0, destination_base + (TABLE_COUNT * PAGE_BYTES) as u64 | 7);
        Ok(())
    }

    fn entry(&self, table: usize, index: usize) -> u64 {
        u64::from_le_bytes(self.0[table][index*8..index*8+8].try_into().unwrap())
    }
    fn put(&mut self, table: usize, index: usize, value: u64) {
        self.0[table][index*8..index*8+8].copy_from_slice(&value.to_le_bytes());
    }
    fn relocate(&mut self, table: usize, level: u8, old: u64, new: u64,
        seen: &mut u16) -> Result<(), IdentityNptError>
    {
        use IdentityNptError as E;
        if table >= TABLE_COUNT || *seen & (1 << table) != 0 { return Err(E::StorageBounds); }
        *seen |= 1 << table;
        for index in 0..512 {
            let entry = self.entry(table, index);
            if entry == 0 { continue; }
            if entry & 5 != 5 || entry & !(ADDRESS_MASK | 0xe7) != 0
                || (level == 4 || level == 1) && entry & 0x80 != 0 {
                return Err(E::StorageBounds);
            }
            if level == 1 || entry & 0x80 != 0 { continue; }
            let offset = (entry & ADDRESS_MASK).checked_sub(old).ok_or(E::StorageBounds)?;
            if offset / 4096 >= TABLE_COUNT as u64 { return Err(E::StorageBounds); }
            self.relocate((offset / 4096) as usize, level - 1, old, new, seen)?;
            self.put(table, index, new + offset | (entry & !ADDRESS_MASK));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NptEvidence {
    pub nx_supported: EvidenceFlag,
    pub host_nxe: EvidenceFlag,
    /// Long mode with four-level paging (LA57 clear), established by caller.
    pub host_four_level: EvidenceFlag,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub host_address: u64,
    pub permissions: PagePermissions,
}

pub struct TableView<'a> {
    pub physical_address: u64,
    pub bytes: &'a [u8; PAGE_BYTES],
}

pub struct Npt<'a> {
    storage: &'a mut TableStorage,
    arena: PhysicalRange,
    policy: AddressPolicy,
    guest_bits: u8,
    used: usize,
    levels: [u8; TABLE_COUNT],
}

#[derive(Clone, Copy)]
struct EntryUpdate {
    index: usize,
    value: u64,
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
            .validate(
                arena_base,
                (TABLE_COUNT * PAGE_BYTES) as u64,
                PAGE_BYTES as u64,
            )
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
            if *self
                .levels
                .get(existing_table)
                .ok_or(NptError::StorageBounds)?
                != 3
            {
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
        *update = Some(EntryUpdate {
            index: leaf_index,
            value: hpa | flags,
        });

        // Resolve all mutable destinations before committing any bytes. Each
        // slot comes from a separate page borrow; no aliasing or unchecked
        // access is needed, and no fallible operation remains after this pass.
        let mut destinations: [Option<&mut [u8; 8]>; TABLE_COUNT] = [const { None }; TABLE_COUNT];
        for ((page, update), destination) in self
            .storage
            .0
            .iter_mut()
            .zip(updates.iter())
            .zip(destinations.iter_mut())
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
        Ok(Some(Translation {
            host_address: (entry & ADDRESS_MASK) | (gpa & 4095),
            permissions,
        }))
    }

    fn check_guest(&self, gpa: u64) -> Result<(), NptError> {
        if gpa >> self.guest_bits != 0 {
            Err(NptError::GuestAddressOutsideWidth)
        } else {
            Ok(())
        }
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
        let offset = (entry & ADDRESS_MASK)
            .checked_sub(self.arena.base())
            .ok_or(NptError::StorageBounds)?;
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

fn entry_mut(page: &mut [u8; PAGE_BYTES], index: usize) -> Result<&mut [u8; 8], NptError> {
    let offset = index.checked_mul(8).ok_or(NptError::StorageBounds)?;
    page.get_mut(offset..)
        .and_then(|tail| tail.first_chunk_mut::<8>())
        .ok_or(NptError::StorageBounds)
}

fn indices(gpa: u64) -> [usize; 4] {
    [
        ((gpa >> 39) & 511) as usize,
        ((gpa >> 30) & 511) as usize,
        ((gpa >> 21) & 511) as usize,
        ((gpa >> 12) & 511) as usize,
    ]
}

/// Separate trusted native-boot profile. It intentionally retains RWX identity
/// access to the admitted physical domain except the monitor's reserved span.
/// It does not weaken the strict, explicitly mapped W^X `Npt` policy above.
/// This is not a RAM/device inventory, DMA boundary or analysis containment.
pub struct IdentityNpt<'a> {
    storage: &'a mut TableStorage,
    arena: PhysicalRange,
    excluded: PhysicalRange,
    guest_bits: u8,
    pdpt_count: usize,
    pt_count: usize,
    trapped_page: Option<u64>,
    protected_range: Option<(u64,u64)>,
    protected_pd: Option<usize>,
    protection_tables: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityNptError {
    RequiredModeNotEstablished,
    OneGiBPagesNotEstablished,
    PatZeroNotWriteBack,
    Address(AddressError),
    InvalidExclusion,
    TableArenaOutsideExclusion,
    GuestAddressOutsideWidth,
    StorageBounds,
}

/// Intended nested translation only; guest paging and actual MTRR/PAT cache
/// composition remain separate. Every present native identity leaf is RWX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityTranslation {
    pub host_address: u64,
    pub page_bytes: u64,
    pub writable: bool,
    pub executable: bool,
    pub pat_index: u8,
}

impl<'a> IdentityNpt<'a> {
    /// Build once while every consumer is stopped. APM2 rev3.44 5.3/5.4,
    /// 15.25 (including nested PAT/MTRR composition). Original guest PAT is
    /// preserved by the continuation owner; NPT PAT index zero must select WB
    /// in the source PAT. This numeric check does not prove WB table backing.
    ///
    /// Map 0..2^min(physical_bits,40). One or two PDPTs use 1GiB leaves;
    /// exactly one leaf is split into 2MiB leaves. Fully excluded 2MiB leaves
    /// remain absent; at most two endpoint leaves need 4KiB tables. The monitor
    /// is a page-aligned interval of at most 32MiB wholly below 1GiB, or the
    /// original <=1MiB interval contained in one 2MiB page anywhere in-domain.
    /// All eight storage pages must be wholly inside the exclusion. Capacity
    /// and the complete address plan are checked before the first table write.
    /// No table is modified on refusal. No hardware operation is performed.
    pub fn new(
        storage: &'a mut TableStorage,
        table_base: u64,
        policy: AddressPolicy,
        excluded: PhysicalRange,
        evidence: NptEvidence,
        one_gib_pages: EvidenceFlag,
        source_pat: u64,
    ) -> Result<Self, IdentityNptError> {
        use IdentityNptError as E;
        if evidence.nx_supported != EvidenceFlag::Set
            || evidence.host_nxe != EvidenceFlag::Set
            || evidence.host_four_level != EvidenceFlag::Set
        {
            return Err(E::RequiredModeNotEstablished);
        }
        if one_gib_pages != EvidenceFlag::Set {
            return Err(E::OneGiBPagesNotEstablished);
        }
        if source_pat & 0xff != 6 {
            return Err(E::PatZeroNotWriteBack);
        }
        let guest_bits = policy.physical_bits().min(40);
        let domain = policy
            .validate(0, 1u64 << guest_bits, 4096)
            .map_err(E::Address)?;
        let excluded = policy
            .validate(excluded.base(), excluded.len(), 4096)
            .map_err(E::Address)?;
        let legacy_exclusion =
            excluded.len() <= 1 << 20 && excluded.base() >> 21 == excluded.last_byte() >> 21;
        if excluded.len() & 4095 != 0
            || excluded.len() > 32 << 20
            || (!legacy_exclusion && excluded.last_byte() >= 1 << 30)
            || excluded.last_byte() > domain.last_byte()
        {
            return Err(E::InvalidExclusion);
        }
        let arena = policy
            .validate(table_base, (TABLE_COUNT * PAGE_BYTES) as u64, 4096)
            .map_err(E::Address)?;
        if arena.base() < excluded.base() || arena.last_byte() > excluded.last_byte() {
            return Err(E::TableArenaOutsideExclusion);
        }
        // Both admitted profiles fit one GiB. Only partial endpoint leaves
        // require PTs; complete leaves stay absent without consuming storage.
        let excluded_end = excluded.last_byte() + 1;
        let gib_base = excluded.base() & !((1 << 30) - 1);
        let mut pt_count = 0;
        for index in 0..512 {
            let start = gib_base + ((index as u64) << 21);
            let end = start + (1 << 21);
            if start < excluded_end
                && excluded.base() < end
                && !(excluded.base() <= start && end <= excluded_end)
            {
                pt_count += 1;
            }
        }
        let pdpt_count = if guest_bits == 40 { 2 } else { 1 };
        let pd_table = 1 + pdpt_count;
        if pt_count > 2 || pd_table + 1 + pt_count > TABLE_COUNT {
            return Err(E::StorageBounds);
        }
        // Domain is at most 1TiB: at most two PDPTs, PML4, PD and two PTs.
        // All validation precedes the first write; no fallible work follows.
        for page in &mut storage.0 {
            page.fill(0);
        }
        for index in 0..pdpt_count {
            identity_put(
                storage,
                0,
                index,
                table_base + ((1 + index) * PAGE_BYTES) as u64 | 7,
            );
        }
        let excluded_gib = excluded.base() >> 30;
        for gib in 0..(1u64 << (guest_bits - 30)) {
            let value = if gib == excluded_gib {
                table_base + (pd_table * PAGE_BYTES) as u64 | 7
            } else {
                (gib << 30) | 0x87
            };
            identity_put(
                storage,
                1 + (gib / 512) as usize,
                (gib % 512) as usize,
                value,
            );
        }
        let mut pt_table = pd_table + 1;
        for index in 0..512 {
            let start = gib_base + ((index as u64) << 21);
            let end = start + (1 << 21);
            let value = if excluded.base() <= start && end <= excluded_end {
                0
            } else if start < excluded_end && excluded.base() < end {
                for page in 0..512 {
                    let address = start + ((page as u64) << 12);
                    if address < excluded.base() || address >= excluded_end {
                        identity_put(storage, pt_table, page, address | 7);
                    }
                }
                let link = table_base + (pt_table * PAGE_BYTES) as u64 | 7;
                pt_table += 1;
                link
            } else {
                start | 0x87
            };
            identity_put(storage, pd_table, index, value);
        }
        Ok(Self {
            storage,
            arena,
            excluded,
            guest_bits,
            pdpt_count,
            pt_count,
            trapped_page: None,
            protected_range: None,
            protected_pd: None,
            protection_tables: 0,
        })
    }

    pub const fn root_address(&self) -> u64 {
        self.arena.base()
    }
    pub const fn guest_bits(&self) -> u8 {
        self.guest_bits
    }
    pub const fn used_tables(&self) -> usize {
        self.pdpt_count + 2 + self.pt_count + if self.trapped_page.is_some() { 2 } else { 0 } + self.protection_tables
    }
    pub const fn excluded(&self) -> PhysicalRange {
        self.excluded
    }

    /// Remove one MMIO page before any CPU uses these tables (APM2 5.4, 15.25).
    /// The native LAPIC caller owns device emulation; this only creates an NPF
    /// hole. Its GiB must differ from the monitor pool's GiB. Two unused tables
    /// split that identity leaf, preserving every neighboring translation.
    /// Refusal leaves all bytes and metadata unchanged. No live TLB is flushed.
    pub fn trap_page(&mut self, gpa: u64) -> Result<(), IdentityNptError> {
        use IdentityNptError as E;
        if self.trapped_page.is_some() || self.protected_range.is_some() || gpa & 4095 != 0 || gpa >> 30 == self.excluded.base() >> 30
        {
            return Err(E::InvalidExclusion);
        }
        let Some(translation) = self.translate(gpa)? else {
            return Err(E::InvalidExclusion);
        };
        let pd = self.used_tables();
        let pt = pd + 1;
        if translation.page_bytes != 1 << 30 || pt >= TABLE_COUNT {
            return Err(E::StorageBounds);
        }
        let gib_base = gpa & !((1 << 30) - 1);
        let mib_base = gpa & !((1 << 21) - 1);
        self.storage.0[pd].fill(0);
        self.storage.0[pt].fill(0);
        for index in 0..512 {
            let address = gib_base + ((index as u64) << 21);
            let value = if address == mib_base {
                self.arena.base() + (pt * PAGE_BYTES) as u64 | 7
            } else {
                address | 0x87
            };
            identity_put(self.storage, pd, index, value);
            let address = mib_base + ((index as u64) << 12);
            if address != gpa {
                identity_put(self.storage, pt, index, address | 7);
            }
        }
        identity_put(
            self.storage,
            1 + (gpa >> 39) as usize,
            ((gpa >> 30) & 511) as usize,
            self.arena.base() + (pd * PAGE_BYTES) as u64 | 7,
        );
        self.trapped_page = Some(gpa);
        Ok(())
    }

    /// Protect writes throughout the complete admitted ECAM aperture before
    /// consumers run, including upstream bridges. Rounds outward to2MiB and
    /// uses one PD at most; native LAPIC leaf holes stay unchanged. Requires
    /// oneGiB-contained aperture distinct from the excluded pool's GiB.
    pub fn protect_write_range(&mut self, base: u64, bytes: u64) -> Result<(), IdentityNptError> {
        use IdentityNptError as E;
        let (start,end)=identity_protection_range(base,bytes)?;
        if self.protected_range.is_some() || start>>30==self.excluded.base()>>30 {
            return Err(E::InvalidExclusion);
        }
        let Some(t)=self.translate(start)? else {return Err(E::InvalidExclusion);};
        // Validate every affected existing translation before writes.
        for a in (start..end).step_by(1<<21) {self.translate(a)?;}
        let pdpt=1+(start>>39)as usize;let pi=((start>>30)&511)as usize;
        let new_pd=t.page_bytes==1<<30;
        let pd=if new_pd {self.used_tables()} else {
            let e=identity_entry(self.storage,pdpt,pi)?;
            ((e&ADDRESS_MASK)-self.arena.base())as usize/PAGE_BYTES
        };
        if new_pd && pd>=TABLE_COUNT {return Err(E::StorageBounds);}
        if new_pd {
            let gib=start&!((1u64<<30)-1);self.storage.0[pd].fill(0);
            for i in 0..512 {identity_put(self.storage,pd,i,gib+((i as u64)<<21)|0x87);}
        }
        for a in (start..end).step_by(1<<21) {
            let i=((a>>21)&511)as usize;
            let e=u64::from_le_bytes(self.storage.0[pd][i*8..i*8+8].try_into().unwrap());
            identity_put(self.storage,pd,i,e&!WRITE);
        }
        if new_pd {identity_put(self.storage,pdpt,pi,self.arena.base()+(pd*PAGE_BYTES)as u64|7);}
        self.protected_range=Some((start,end));self.protected_pd=new_pd.then_some(pd);
        self.protection_tables=usize::from(new_pd);Ok(())
    }

    /// Unused storage pages remain zero and are never exported as reachable.
    pub fn table(&self, index: usize) -> Option<TableView<'_>> {
        if index >= self.used_tables() {
            return None;
        }
        Some(TableView {
            physical_address: self.arena.base() + (index * PAGE_BYTES) as u64,
            bytes: self.storage.0.get(index)?,
        })
    }

    /// Walk actual emitted bytes for audit, without dereferencing a physical
    /// address or consulting a TLB. Checks builder-owned links and leaf identity,
    /// tolerating hardware A/D updates. Corruption never escapes this storage.
    pub fn translate(&self, gpa: u64) -> Result<Option<IdentityTranslation>, IdentityNptError> {
        use IdentityNptError as E;
        if gpa >> self.guest_bits != 0 {
            return Err(E::GuestAddressOutsideWidth);
        }
        let mut table = 0;
        for (depth, index) in indices(gpa).into_iter().enumerate() {
            let level = 4 - depth;
            let bytes = self.storage.0.get(table).ok_or(E::StorageBounds)?;
            let offset = index * 8;
            let entry = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
            if entry & PRESENT == 0 {
                // Only wholly excluded 2MiB or 4KiB leaves may be absent.
                // A missing endpoint PT must not hide its ordinary neighbors.
                let page_bytes = 1u64 << (12 + 9 * (level - 1));
                let start = gpa & !(page_bytes - 1);
                if (((level == 1 || level == 2)
                    && start >= self.excluded.base()
                    && start + page_bytes - 1 <= self.excluded.last_byte())
                    || (level == 1 && self.trapped_page == Some(start)))
                    && entry == 0
                {
                    return Ok(None);
                }
                return Err(E::StorageBounds);
            }
            let protected = level == 2 && self.protected_range.is_some_and(|(s,e)|s<=gpa&&gpa<e);
            if entry & 7 != if protected {5} else {7} || entry & !(ADDRESS_MASK | 0xe7) != 0 {
                return Err(E::StorageBounds);
            }
            let large = entry & 0x80 != 0;
            if level == 4 && large || level == 1 && large {
                return Err(E::StorageBounds);
            }
            if large || level == 1 {
                let page_bytes = 1u64 << (12 + 9 * (level - 1));
                let base = entry & ADDRESS_MASK;
                if base & (page_bytes - 1) != 0 {
                    return Err(E::StorageBounds);
                }
                let host_address = base | (gpa & (page_bytes - 1));
                if host_address != gpa
                    || (base <= self.excluded.last_byte()
                        && self.excluded.base() < base + page_bytes)
                    || self
                        .trapped_page
                        .is_some_and(|page| base <= page && page < base + page_bytes)
                {
                    return Err(E::StorageBounds);
                }
                return Ok(Some(IdentityTranslation {
                    host_address,
                    page_bytes,
                    writable: !self.protected_range.is_some_and(|(s,e)|s<=gpa&&gpa<e),
                    executable: true,
                    pat_index: 0,
                }));
            }
            let offset = (entry & ADDRESS_MASK)
                .checked_sub(self.arena.base())
                .ok_or(E::StorageBounds)?;
            let next = usize::try_from(offset / PAGE_BYTES as u64).map_err(|_| E::StorageBounds)?;
            let ordinary_used = self.pdpt_count + 2 + self.pt_count;
            let expected_level = if (1..=self.pdpt_count).contains(&next) {
                3
            } else if next == self.pdpt_count + 1
                || (self.trapped_page.is_some() && next == ordinary_used)
                || self.protected_pd == Some(next)
            {
                2
            } else if (self.pdpt_count + 2..self.used_tables()).contains(&next) {
                1
            } else {
                return Err(E::StorageBounds);
            };
            if next >= self.used_tables() || expected_level != level - 1 {
                return Err(E::StorageBounds);
            }
            table = next;
        }
        Err(E::StorageBounds)
    }
}

/// Bound and outward-round an ECAM aperture. No physical address is accessed.
pub fn identity_protection_range(base:u64, bytes:u64)->Result<(u64,u64),IdentityNptError> {
    let end=base.checked_add(bytes).filter(|_|bytes!=0).ok_or(IdentityNptError::InvalidExclusion)?;
    let start=base&!((1u64<<21)-1);
    let end=end.checked_add((1<<21)-1).ok_or(IdentityNptError::InvalidExclusion)?&!((1u64<<21)-1);
    if base&((1<<20)-1)!=0 || bytes&((1<<20)-1)!=0 || start>>30!=(end-1)>>30
        || end>1u64<<40 {return Err(IdentityNptError::InvalidExclusion);}
    Ok((start,end))
}
fn identity_entry(storage:&TableStorage,table:usize,index:usize)->Result<u64,IdentityNptError>{
    let p=storage.0.get(table).filter(|_|index<512).ok_or(IdentityNptError::StorageBounds)?;
    Ok(u64::from_le_bytes(p[index*8..index*8+8].try_into().unwrap()))
}
/// Restore only the installed ECAM PD write restrictions after diagnostic
/// revocation. Caller exclusively owns this stopped CPU's installed tables;
/// no other CPU may use or edit them, and the caller must request a nested-TLB
/// flush before resuming the SAME faulting instruction. Every link/entry is
/// checked before the first mutation. Existing leaf holes are never changed.
pub fn restore_identity_write_range(storage:&mut TableStorage,table_base:u64,base:u64,bytes:u64)
    ->Result<(),IdentityNptError>
{
    use IdentityNptError as E;
    let(start,end)=identity_protection_range(base,bytes)?;
    if table_base&4095!=0 {return Err(E::StorageBounds);}
    let child=|e:u64|->Result<usize,E>{
        if e&7!=7 || e&!(ADDRESS_MASK|0x67)!=0 {return Err(E::StorageBounds);}
        let d=(e&ADDRESS_MASK).checked_sub(table_base).ok_or(E::StorageBounds)?;
        let i=usize::try_from(d/PAGE_BYTES as u64).map_err(|_|E::StorageBounds)?;
        if i==0 || i>=TABLE_COUNT {return Err(E::StorageBounds);}Ok(i)
    };
    let pdpt=child(identity_entry(storage,0,(start>>39)as usize)?)?;
    let pd=child(identity_entry(storage,pdpt,((start>>30)&511)as usize)?)?;
    for a in(start..end).step_by(1<<21){
        let e=identity_entry(storage,pd,((a>>21)&511)as usize)?;
        if !matches!(e&7,5|7)||e&!(ADDRESS_MASK|0xe7)!=0{return Err(E::StorageBounds);}
        if e&0x80!=0 {if e&ADDRESS_MASK!=a{return Err(E::StorageBounds);}}
        else {child(e|WRITE)?;}
    }
    for a in(start..end).step_by(1<<21){let i=((a>>21)&511)as usize;
        let e=identity_entry(storage,pd,i).expect("prevalidated PD entry");identity_put(storage,pd,i,e|WRITE);}
    Ok(())
}

fn identity_put(storage: &mut TableStorage, table: usize, index: usize, value: u64) {
    storage.0[table][index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::EncryptionState;

    fn policy() -> AddressPolicy {
        AddressPolicy::new(
            48,
            EncryptionState::Unencrypted {
                encryption_bit: None,
            },
        )
        .unwrap()
    }
    fn evidence() -> NptEvidence {
        NptEvidence {
            nx_supported: EvidenceFlag::Set,
            host_nxe: EvidenceFlag::Set,
            host_four_level: EvidenceFlag::Set,
        }
    }

    fn low_translation(storage: &LowMemoryNptStorage, base: u64, gpa: u64) -> Option<(u64, bool)> {
        let mut table = 0;
        for (depth, index) in indices(gpa).into_iter().enumerate() {
            let entry = storage.entry(table, index);
            if entry & 1 == 0 { return None; }
            if depth == 3 || entry & 0x80 != 0 {
                let size = 1u64 << (12 + 9 * (3-depth));
                return Some(((entry & ADDRESS_MASK) + (gpa & (size-1)), entry & 2 != 0));
            }
            table = ((entry & ADDRESS_MASK) - base) as usize / 4096;
        }
        unreachable!()
    }

    #[test]
    fn cache_root_denies_low_mib_preserves_existing_holes_and_refreshes_permissions() {
        for excluded_base in [0x100000, 0x400000] {
            let policy = policy();
            let mut original = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
            let mut npt = IdentityNpt::new(&mut original, excluded_base, policy,
                policy.validate(excluded_base, 0x100000, 4096).unwrap(), evidence(), EvidenceFlag::Set, 6).unwrap();
            npt.trap_page(0xfee00000).unwrap();
            npt.protect_write_range(0xe0000000, 0x1000000).unwrap();
            let mut copy = LowMemoryNptStorage::empty();
            let root = 0x800000;
            copy.prepare(npt.storage, excluded_base, root).unwrap();
            for address in [0, 0x1000, 0x7ffff, 0xfffff] { assert_eq!(low_translation(&copy, root, address), None); }
            assert_eq!(low_translation(&copy, root, excluded_base), None);
            assert_eq!(low_translation(&copy, root, 0xfee00000), None);
            assert_eq!(low_translation(&copy, root, 0xfee01000), Some((0xfee01000, true)));
            assert_eq!(low_translation(&copy, root, 0xe0000000), Some((0xe0000000, false)));
            assert_eq!(low_translation(&copy, root, 0x200000), Some((0x200000, true)));
            if excluded_base > 0x100000 { assert_eq!(low_translation(&copy, root, 0x100000), Some((0x100000, true))); }
            assert_eq!(npt.translate(0).unwrap().unwrap().host_address, 0);
            restore_identity_write_range(npt.storage, excluded_base, 0xe0000000, 0x1000000).unwrap();
            copy.prepare(npt.storage, excluded_base, root).unwrap();
            assert_eq!(low_translation(&copy, root, 0xe0000000), Some((0xe0000000, true)));
        }
    }

    #[test]
    fn identity_mmio_hole_preserves_neighbors_and_monitor_exclusion() {
        let policy = policy();
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let mut npt = IdentityNpt::new(
            &mut storage,
            0x201000,
            policy,
            policy.validate(0x201000, 0x600000, 4096).unwrap(),
            evidence(),
            EvidenceFlag::Set,
            6,
        )
        .unwrap();
        assert_eq!(npt.used_tables(), 6);
        npt.trap_page(0xfee00000).unwrap();
        assert_eq!(npt.used_tables(), TABLE_COUNT);
        assert_eq!(npt.translate(0xfee00000), Ok(None));
        assert_eq!(npt.translate(0xfee00fff), Ok(None));
        assert_eq!(npt.translate(0x400000), Ok(None));
        for address in [
            0,
            0x200fff,
            0x801000,
            0xfedfffff,
            0xfee01000,
            0xfeffffff,
            0xffffffffff,
        ] {
            assert_eq!(
                npt.translate(address).unwrap().unwrap().host_address,
                address
            );
        }
        let before = npt.storage.0;
        assert_eq!(
            npt.trap_page(0xfec00000),
            Err(IdentityNptError::InvalidExclusion)
        );
        assert_eq!(npt.storage.0, before);
        // A lost parent cannot silently expand the permitted MMIO hole.
        identity_put(npt.storage, 6, ((0xfee00000u64 >> 21) & 511) as usize, 0);
        assert_eq!(
            npt.translate(0xfee00000),
            Err(IdentityNptError::StorageBounds)
        );
    }

    #[test]
    fn identity_mmio_hole_refuses_bad_plan_without_mutation() {
        let policy = policy();
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let mut npt = IdentityNpt::new(
            &mut storage,
            0x200000,
            policy,
            policy.validate(0x200000, 0x200000, 4096).unwrap(),
            evidence(),
            EvidenceFlag::Set,
            6,
        )
        .unwrap();
        let before = npt.storage.0;
        for address in [0xfee00001, 0x800000, 1u64 << 40] {
            assert!(npt.trap_page(address).is_err());
            assert_eq!(npt.storage.0, before);
            assert_eq!(npt.trapped_page, None);
        }
    }

    #[test]
    fn identity_pool_audit_distinguishes_full_holes_from_missing_endpoint_tables() {
        let policy = policy();
        let excluded = policy.validate(0x201000, 0x600000, 4096).unwrap();
        for (index, value, address) in [
            // First/last partial PD entries cannot become wholesale holes,
            // even when this particular GPA lies inside the monitor.
            (1, 0, 0x201000),
            (4, 0, 0x800000),
            // A wholly excluded PD leaf cannot regain a present identity map.
            (2, 0x400087, 0x400000),
            // Nor can an endpoint become a large leaf that exposes the monitor
            // while an audit queries one of its ordinary adjacent addresses.
            (1, 0x200087, 0x200000),
            // Linking the endpoint to an unused page is not an admitted hole.
            (1, 0x207007, 0x201000),
        ] {
            let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
            let npt = IdentityNpt::new(
                &mut storage,
                0x201000,
                policy,
                excluded,
                evidence(),
                EvidenceFlag::Set,
                6,
            )
            .unwrap();
            assert_eq!(npt.translate(0x400000), Ok(None));
            identity_put(npt.storage, 3, index, value);
            let before = npt.storage.0;
            assert_eq!(npt.translate(address), Err(IdentityNptError::StorageBounds));
            assert_eq!(npt.storage.0, before);
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
            npt.map_page(0, 0x200000, PagePermissions::ReadOnly)
                .unwrap();
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
