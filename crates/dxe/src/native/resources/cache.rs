//! Join actual retained leaf/fetch observations to the reviewed WB classifier.
//! This adds no ownership, global-alias, TLB, DMA or native-entry authority.
use crate::native_tables::{LeafObservation, PreparedTables, TableError, TablePageObservation};
use svmvisor_dxe::native_cache::{self, CacheError, CacheSnapshot, PageMapping};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Owned,
    Borrowed,
    Gdt,
    TableAlias,
    TableFetch,
    ArenaFetch,
}

/// The fixed constructed guest/NPT page-walk paths use actual PWT/PCD zero
/// and guest PAT0=WB. Qualify host PAT0 for every arena frame separately from
/// the already checked full software aliases; this supplies no construction
/// or ownership authority by itself.
#[cfg(feature = "native-returning")]
pub fn qualify_arena_fetches(
    snapshot: &CacheSnapshot,
    base: u64,
    bytes: u64,
) -> Result<(), ResourceCacheError> {
    if base & 4095 != 0
        || bytes == 0
        || bytes & 4095 != 0
        || bytes / 4096 > native_cache::MAX_PAGES as u64
        || base.checked_add(bytes).is_none()
    {
        return Err(ResourceCacheError::Shape);
    }
    for offset in 0..bytes / 4096 {
        let page = base + offset * 4096;
        qualify_leaf(
            snapshot,
            LeafObservation {
                physical_page: page,
                leaf_physical_base: page,
                leaf_bytes: 4096,
                pat_index: 0,
            },
            ResourceKind::ArenaFetch,
        )?;
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceCacheError {
    Tables(TableError),
    Shape,
    Cache {
        kind: ResourceKind,
        page: u64,
        pat_index: u8,
        error: CacheError,
    },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceCacheReport {
    pub owned_pages: usize,
    /// Summed span coverages; shared pages may be checked several times.
    pub borrowed_pages: usize,
    pub gdt_pages: usize,
    pub table_alias_pages: usize,
    pub table_fetch_encodings: usize,
}

fn qualify_leaf(
    snapshot: &CacheSnapshot,
    leaf: LeafObservation,
    kind: ResourceKind,
) -> Result<(), ResourceCacheError> {
    // One page at a time avoids a second large mapping array on the firmware
    // stack. The classifier still validates the full actual containing leaf.
    native_cache::classify_write_back(
        snapshot,
        leaf.physical_page,
        4096,
        &[PageMapping {
            physical_page: leaf.physical_page,
            leaf_physical_base: leaf.leaf_physical_base,
            leaf_bytes: leaf.leaf_bytes,
            pat_index: leaf.pat_index,
        }],
    )
    .map(|_| ())
    .map_err(|error| ResourceCacheError::Cache {
        kind,
        page: leaf.physical_page,
        pat_index: leaf.pat_index,
        error,
    })
}

fn qualify_table(
    snapshot: &CacheSnapshot,
    page: &TablePageObservation,
) -> Result<usize, ResourceCacheError> {
    if page.physical_page != page.alias.physical_page
        || page.fetch_pat_indices == 0
        || page.fetch_pat_indices & !15 != 0
        || page.levels == 0
        || page.levels & !15 != 0
    {
        return Err(ResourceCacheError::Shape);
    }
    qualify_leaf(snapshot, page.alias, ResourceKind::TableAlias)?;
    let mut count = 0;
    for index in 0..4 {
        if page.fetch_pat_indices & (1 << index) == 0 {
            continue;
        }
        // Hardware fetch uses its observed parent/CR3 encoding on this 4 KiB
        // physical frame. This is not a fabricated software leaf or ownership.
        qualify_leaf(
            snapshot,
            LeafObservation {
                physical_page: page.physical_page,
                leaf_physical_base: page.physical_page,
                leaf_bytes: 4096,
                pat_index: index,
            },
            ResourceKind::TableFetch,
        )?;
        count += 1;
    }
    Ok(count)
}

pub fn qualify(
    snapshot: &CacheSnapshot,
    tables: &PreparedTables<'_>,
) -> Result<ResourceCacheReport, ResourceCacheError> {
    use ResourceCacheError::Tables;
    let mut report = ResourceCacheReport::default();
    for index in 0..tables.owned_ranges().map_err(Tables)?.len() {
        for leaf in tables.owned_range_mappings(index).map_err(Tables)? {
            qualify_leaf(snapshot, *leaf, ResourceKind::Owned)?;
            report.owned_pages += 1;
        }
    }
    for index in 0..tables.borrowed_spans().map_err(Tables)?.len() {
        for leaf in tables.borrowed_span_mappings(index).map_err(Tables)? {
            qualify_leaf(snapshot, *leaf, ResourceKind::Borrowed)?;
            report.borrowed_pages += 1;
        }
    }
    for leaf in tables.gdt_mappings().map_err(Tables)? {
        qualify_leaf(snapshot, *leaf, ResourceKind::Gdt)?;
        report.gdt_pages += 1;
    }
    for page in tables.table_page_observations().map_err(Tables)? {
        report.table_fetch_encodings += qualify_table(snapshot, page)?;
        report.table_alias_pages += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> CacheSnapshot {
        CacheSnapshot {
            abi_version: 1,
            captured_fields: native_cache::captured::REQUIRED,
            msr_reads: 30,
            signature: native_cache::TARGET_SIGNATURE,
            max_basic: 0x10,
            max_extended: 0x80000026,
            leaf1_edx: 0x11020,
            physical_bits: 48,
            encryption_eax: 1,
            encryption_ebx: 51 | (5 << 6),
            cr0: 0x80010033,
            cr4: 0x620,
            efer: 0xd00,
            sys_cfg: 1 << 20,
            pat: 0x0007040600070406,
            mtrr_cap: 0x508,
            mtrr_default: 0x806,
            top_mem: 0x08000000,
            apic_base: 0xfee00900,
            ..CacheSnapshot::default()
        }
    }
    fn table() -> TablePageObservation {
        TablePageObservation {
            physical_page: 0x201000,
            alias: LeafObservation {
                physical_page: 0x201000,
                leaf_physical_base: 0x200000,
                leaf_bytes: 0x200000,
                pat_index: 4,
            },
            fetch_pat_indices: 1,
            levels: 8,
        }
    }
    #[test]
    fn every_parent_fetch_encoding_is_qualified_separately_from_the_software_alias() {
        let s = snapshot();
        let mut page = table();
        assert_eq!(qualify_table(&s, &page), Ok(1));
        page.fetch_pat_indices |= 2; // Actual PAT[1] is WT, while alias PAT[4] is WB.
        assert_eq!(
            qualify_table(&s, &page),
            Err(ResourceCacheError::Cache {
                kind: ResourceKind::TableFetch,
                page: page.physical_page,
                pat_index: 1,
                error: CacheError::PatNotWriteBack,
            })
        );
        let mut wb = s;
        wb.pat = (wb.pat & !0xff00) | 0x0600;
        assert_eq!(qualify_table(&wb, &page), Ok(2));
        for mask in [0, 16, 0xff] {
            page.fetch_pat_indices = mask;
            assert_eq!(qualify_table(&wb, &page), Err(ResourceCacheError::Shape));
        }
    }
    #[test]
    fn a_small_fetch_frame_never_hides_a_conflict_in_its_containing_software_leaf() {
        let mut s = snapshot();
        s.variable[0] = native_cache::RegisterPair {
            base: 0x300000,
            mask: (((1u64 << 48) - 1) & !(0x100000 - 1)) | 0x800,
        };
        assert_eq!(
            qualify_table(&s, &table()),
            Err(ResourceCacheError::Cache {
                kind: ResourceKind::TableAlias,
                page: 0x201000,
                pat_index: 4,
                error: CacheError::MtrrNotWriteBack,
            })
        );
    }
    #[cfg(feature = "native-returning")]
    #[test]
    fn constructed_arena_fetches_require_actual_pat_zero_even_with_a_wb_alias() {
        let mut s = snapshot();
        let page = table();
        assert_eq!(qualify_arena_fetches(&s, 0x200000, 33 * 4096), Ok(()));
        s.pat = (s.pat & !255) | 4; // PAT[0]=WT; software alias PAT[4] stays WB.
        assert_eq!(
            qualify_leaf(&s, page.alias, ResourceKind::TableAlias),
            Ok(())
        );
        assert_eq!(
            qualify_arena_fetches(&s, 0x200000, 33 * 4096),
            Err(ResourceCacheError::Cache {
                kind: ResourceKind::ArenaFetch,
                page: 0x200000,
                pat_index: 0,
                error: CacheError::PatNotWriteBack
            })
        );
    }
}
