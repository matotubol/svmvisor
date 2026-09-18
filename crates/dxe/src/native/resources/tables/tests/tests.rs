use super::super::*;

use svmvisor_hypervisor::{
    boot::memory::{MemoryDescriptor, ValidatedMemoryMap},
    host::paging::PagingConfig,
};
use uefi_raw::table::boot::MemoryAttribute;

use super::super::{
    attribute::{attributes_allow, internal_access},
    mapping::{
        read_checked_entry, reject_owned_overlap, retain_borrowed_mappings, retain_owned_mappings,
        validate_borrowed_spans, validate_owned_ranges,
    },
};

fn empty() -> RetainedWalks {
    RetainedWalks {
        entries: [EntryObservation { address: 0, value: 0, allowed_set_bits: 0 };
            MAX_RETAINED_ENTRIES],
        entry_count: 0,
        table_pages: [TablePageObservation::default(); MAX_TABLE_PAGES],
        page_count: 0,
    }
}
fn config() -> PagingConfig {
    PagingConfig {
        cr3: 0x1000,
        physical_bits: 48,
        la57: false,
        nxe: true,
        pcid: false,
        page1gb: true,
    }
}
fn read_large(address: u64) -> Result<u64, TableError> {
    match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3000 => Ok(0x83),
        _ => Err(TableError::ReadPermission),
    }
}

fn empty_storage() -> TableStorage {
    TableStorage {
        gdt: [0; MAX_GDT_BYTES],
        gdt_mappings: [LeafObservation::default(); MAX_GDT_PAGES],
        gdt_page_count: 0,
        walks: empty(),
        owned_ranges: [OwnedRange::default(); MAX_OWNED_RANGES],
        owned_range_count: 0,
        owned_mappings: [LeafObservation::default(); MAX_OWNED_PAGES],
        owned_page_count: 0,
        borrowed_spans: [BorrowedSpan::default(); MAX_BORROWED_SPANS + INTERNAL_SPANS],
        borrowed_span_count: 0,
        caller_borrowed_span_count: 0,
        borrowed_mappings: [LeafObservation::default(); MAX_BORROWED_PAGES + MAX_INTERNAL_PAGES],
        borrowed_page_count: 0,
    }
}

fn ram(base: u64, pages: u64) -> MemoryDescriptor {
    MemoryDescriptor { memory_type: 4, physical_start: base, page_count: pages, attributes: 8 }
}

fn span(base: u64, bytes: u64, access: BorrowedAccess) -> BorrowedSpan {
    BorrowedSpan { base, bytes, access }
}

#[test]
fn borrowed_exact_extents_rounding_overlap_and_separate_bounds() {
    use BorrowedAccess::*;
    let unaligned = span(0x1fff, 2, Read);
    assert_eq!(covering_pages(unaligned, 48), Ok((0x1000, 2)));
    assert_eq!(validate_borrowed_spans(&[unaligned, unaligned], 48, &[], false), Ok(4));
    for invalid in [
        span(0x1000, 0, Read),
        span(u64::MAX, 1, Read),
        span(1 << 47, 1, Read),
        span((1 << 47) - 1, 2, Read),
        span(1 << 48, 4096, Read),
    ] {
        assert_eq!(covering_pages(invalid, 48), Err(TableError::BorrowedSpan));
    }
    assert_eq!(covering_pages(unaligned, 53), Err(TableError::BorrowedSpan));
    assert_eq!(
        validate_borrowed_spans(&[unaligned; MAX_BORROWED_SPANS + 1], 48, &[], false),
        Err(TableError::Bounds)
    );
    let page_limit = span(0x1000, MAX_BORROWED_PAGES as u64 * 4096, ReadWrite);
    assert_eq!(validate_borrowed_spans(&[page_limit], 48, &[], false), Ok(MAX_BORROWED_PAGES));
    assert_eq!(
        validate_borrowed_spans(
            &[BorrowedSpan { bytes: page_limit.bytes + 1, ..page_limit }],
            48,
            &[],
            false
        ),
        Err(TableError::Bounds)
    );
    let maximum_map = span(0x1001, (1024 * 1024 + 128 * 1024) as u64, ReadWrite);
    let table_pool = span(0x20_0001, size_of::<TableStorage>() as u64, ReadWrite);
    let internal_pages =
        validate_borrowed_spans(&[table_pool, maximum_map], 48, &[], true).unwrap();
    assert!(internal_pages > MAX_OWNED_PAGES && internal_pages <= MAX_INTERNAL_PAGES);
    assert_eq!(
        validate_borrowed_spans(
            &[span(0x1000, (MAX_INTERNAL_PAGES as u64 + 1) * 4096, Read)],
            48,
            &[],
            true
        ),
        Err(TableError::Bounds)
    );
    assert_eq!(validate_borrowed_spans(&[unaligned; 3], 48, &[], true), Err(TableError::Bounds));
    let owned = [OwnedRange { base: 0x2000, bytes: 4096 }];
    assert_eq!(
        validate_borrowed_spans(&[unaligned], 48, &owned, false),
        Err(TableError::OwnedRange)
    );
    assert_eq!(
        validate_borrowed_spans(&[span(0x1fff, 1, Read), span(0x3000, 1, Read)], 48, &owned, false),
        Ok(2)
    );
}

#[test]
fn current_attribute_permissions_match_each_required_access() {
    use BorrowedAccess::*;
    for access in [Read, ReadWrite, ReadExecute] {
        assert!(attributes_allow(Status::SUCCESS, MemoryAttribute::empty(), access));
        assert!(!attributes_allow(Status::UNSUPPORTED, MemoryAttribute::empty(), access));
        assert!(!attributes_allow(Status::SUCCESS, MemoryAttribute::READ_PROTECT, access));
    }
    let ro = MemoryAttribute::from_bits_retain(0x20000);
    assert!(attributes_allow(Status::SUCCESS, ro, Read));
    assert!(!attributes_allow(Status::SUCCESS, ro, ReadWrite));
    assert!(attributes_allow(Status::SUCCESS, ro, ReadExecute));
    let xp = MemoryAttribute::EXECUTE_PROTECT;
    assert!(attributes_allow(Status::SUCCESS, xp, Read));
    assert!(attributes_allow(Status::SUCCESS, xp, ReadWrite));
    assert!(!attributes_allow(Status::SUCCESS, xp, ReadExecute));
}

#[test]
fn internal_query_failures_and_real_denials_remain_distinct_and_block_source_loads() {
    let descriptors = [ram(0x1000, 1)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let failed = core::cell::Cell::new(false);
    for query in [Ok(0x2000u64), Err(())] {
        assert_eq!(
            read_checked_entry(
                &memory,
                0x1000,
                |_| internal_access(query, BorrowedAccess::Read, &failed),
                |_| panic!("failed internal permission must precede direct read"),
            ),
            Err(TableError::ReadPermission),
        );
        assert_eq!(failed.get(), query.is_err());
    }
    let failed = core::cell::Cell::new(false);
    assert!(internal_access(Ok::<u64, ()>(0x20000), BorrowedAccess::Read, &failed));
    assert!(!internal_access(Ok::<u64, ()>(0x20000), BorrowedAccess::ReadWrite, &failed));
    assert!(!internal_access(Ok::<u64, ()>(0x4000), BorrowedAccess::ReadExecute, &failed));
    assert!(!failed.get());
}

#[test]
fn borrowed_metadata_and_current_denial_precede_all_operand_walks() {
    use BorrowedAccess::*;
    let descriptors = [ram(0x1000, 1)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let mut storage = empty_storage();
    assert_eq!(
        retain_borrowed_mappings(
            &mut storage,
            &[span(0x1fff, 2, Read)],
            &[],
            &memory,
            config(),
            false,
            false,
            &mut |_, _| panic!("metadata denial must precede current query"),
            &mut |_| panic!("metadata denial must precede table read")
        ),
        Err(TableError::Metadata)
    );
    assert_eq!(
        retain_borrowed_mappings(
            &mut storage,
            &[span(0x1001, 1, Read)],
            &[],
            &memory,
            config(),
            false,
            false,
            &mut |page, access| {
                assert_eq!((page, access), (0x1000, Read));
                false
            },
            &mut |_| panic!("current denial must precede table read")
        ),
        Err(TableError::ReadPermission)
    );
    assert_eq!(storage.walks.entry_count, 0);
}

#[test]
fn borrowed_access_requires_effective_rw_nx_and_actual_smep_user_compatibility() {
    use BorrowedAccess::*;
    let descriptors = [ram(0, 512)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    for (access, smep, ro, nx, user, expected) in [
        (Read, true, true, true, true, Ok(())),
        (ReadWrite, false, true, false, false, Err(TableError::NotWritable)),
        (ReadExecute, false, true, false, false, Ok(())),
        (ReadExecute, false, false, true, false, Err(TableError::NotExecutable)),
        (ReadExecute, true, false, false, true, Err(TableError::NotExecutable)),
        (ReadExecute, false, false, false, true, Ok(())),
        (ReadExecute, true, false, false, false, Ok(())),
    ] {
        let mut storage = empty_storage();
        assert_eq!(
            retain_borrowed_mappings(
                &mut storage,
                &[span(0x9001, 4096, access)],
                &[],
                &memory,
                config(),
                smep,
                false,
                &mut |_, required| {
                    assert_eq!(required, access);
                    true
                },
                &mut |address| read_large(address).map(|mut value| {
                    if ro && address == 0x1000 {
                        value &= !2;
                    }
                    if nx && address == 0x2000 {
                        value |= 1 << 63;
                    }
                    if user {
                        value |= 4;
                    }
                    value
                })
            ),
            expected
        );
    }
    let mut storage = empty_storage();
    assert_eq!(
        retain_borrowed_mappings(
            &mut storage,
            &[span(0x9001, 1, Read)],
            &[],
            &memory,
            config(),
            false,
            false,
            &mut |_, _| true,
            &mut |address| read_large(address).map(|value| if address == 0x3000 {
                value | 0x20_0000
            } else {
                value
            })
        ),
        Err(TableError::NonIdentity)
    );
}

#[test]
fn overlapping_spans_retain_exact_bytes_access_and_duplicate_actual_leaves() {
    use BorrowedAccess::*;
    let descriptors = [ram(0, 512)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let requested =
        [span(0x8fff, 2, Read), span(0x9000, 2, ReadExecute), span(0x9001, 1, ReadWrite)];
    let mut queried = 0;
    let mut storage = empty_storage();
    retain_borrowed_mappings(
        &mut storage,
        &requested,
        &[],
        &memory,
        config(),
        true,
        false,
        &mut |_, _| {
            queried += 1;
            true
        },
        &mut read_large,
    )
    .unwrap();
    assert_eq!(queried, 4);
    assert_eq!(&storage.borrowed_spans[..3], &requested);
    assert_eq!(storage.caller_borrowed_span_count, 3);
    assert_eq!(storage.borrowed_page_count, 4);
    for (index, page) in [0x8000, 0x9000, 0x9000, 0x9000].iter().enumerate() {
        assert_eq!(
            storage.borrowed_mappings[index],
            LeafObservation {
                physical_page: *page,
                leaf_physical_base: 0,
                leaf_bytes: 0x20_0000,
                pat_index: 0,
            }
        );
    }
    storage.walks.close_dependencies(config(), &mut read_large).unwrap();
    assert_eq!(storage.walks.compare(|address| read_large(address).unwrap()), Ok(()));
    assert_eq!(
        storage.walks.compare(
            |address| read_large(address).unwrap() ^ if address == 0x3000 { 8 } else { 0 }
        ),
        Err(TableError::Changed)
    );
}

#[test]
fn internal_pool_pages_have_separate_capacity_and_legacy_pcid_compatibility() {
    use BorrowedAccess::*;
    let descriptors = [ram(0, 2048)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let mut storage = empty_storage();
    let requested = [span(0x1000, MAX_BORROWED_PAGES as u64 * 4096, Read)];
    let internal = [
        span(0x50_0001, (1024 * 1024 + 128 * 1024) as u64, ReadWrite),
        span(0x70_0001, size_of::<TableStorage>() as u64, ReadWrite),
    ];
    let mut reader = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x83),
        _ => Err(TableError::ReadPermission),
    };
    retain_borrowed_mappings(
        &mut storage,
        &requested,
        &[],
        &memory,
        config(),
        false,
        false,
        &mut |_, _| true,
        &mut reader,
    )
    .unwrap();
    retain_borrowed_mappings(
        &mut storage,
        &internal,
        &[],
        &memory,
        config(),
        false,
        true,
        &mut |_, _| true,
        &mut reader,
    )
    .unwrap();
    assert_eq!(storage.borrowed_span_count, 3);
    assert_eq!(storage.caller_borrowed_span_count, 1);
    assert!(storage.borrowed_page_count > MAX_BORROWED_PAGES + MAX_OWNED_PAGES);
    assert_eq!(&storage.borrowed_spans[1..3], &internal);
    let mut storage = empty_storage();
    let pcid = PagingConfig { pcid: true, cr3: 0x1007, ..config() };
    assert_eq!(
        retain_borrowed_mappings(
            &mut storage,
            &requested,
            &[],
            &memory,
            pcid,
            false,
            false,
            &mut |_, _| panic!("PCID refuses before query"),
            &mut |_| panic!("PCID refuses before read")
        ),
        Err(TableError::TableFetch)
    );
    retain_borrowed_mappings(
        &mut storage,
        &internal,
        &[],
        &memory,
        pcid,
        false,
        true,
        &mut |_, _| true,
        &mut reader,
    )
    .unwrap();
    assert_eq!(storage.walks.table_pages[0].fetch_pat_indices, 0);
}

#[test]
fn borrowed_fragmented_layout_refuses_entry_exhaustion_without_truncating_success() {
    let descriptors = [ram(0, 1024)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let mut storage = empty_storage();
    let mut reads = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3008 => Ok(0x4003),
        0x4000..=0x47f8 if address & 7 == 0 => Ok(0x20_0003 + ((address - 0x4000) / 8) * 4096),
        _ => Err(TableError::ReadPermission),
    };
    assert_eq!(
        retain_borrowed_mappings(
            &mut storage,
            &[span(0x20_0000, 256 * 4096, BorrowedAccess::Read)],
            &[],
            &memory,
            config(),
            false,
            false,
            &mut |_, _| true,
            &mut reads
        ),
        Err(TableError::Bounds)
    );
    assert_eq!(storage.walks.entry_count, MAX_RETAINED_ENTRIES);
    assert_eq!(storage.borrowed_span_count, 0);
    assert_eq!(storage.borrowed_page_count, 0);
}

#[test]
fn settlement_rejects_dirty_on_nonleaf_slots_and_accepts_valid_leaf_sizes() {
    for nonleaf in [0x1000, 0x2000] {
        let mut retained = empty();
        retained.translate(config(), 0, &mut read_large).unwrap();
        assert_eq!(
            retained
                .settle_accessed_dirty(|address| read_large(address).unwrap()
                    | if address == nonleaf { 0x40 } else { 0 }),
            Err(TableError::Changed)
        );
    }
    for leaf_level in [1u8, 2, 3] {
        let leaf_address = (5 - u64::from(leaf_level)) * 4096;
        let mut reader = |address| {
            if address == leaf_address {
                Ok(0x83)
            } else if address < leaf_address {
                Ok(address + 4096 + 3)
            } else {
                Err(TableError::ReadPermission)
            }
        };
        let mut retained = empty();
        let translated = retained.translate(config(), 0, &mut reader).unwrap();
        assert_eq!(translated.page_bytes, 1u64 << (12 + 9 * (leaf_level - 1)));
        assert_eq!(translated.pat_index, if leaf_level == 1 { 4 } else { 0 });
        retained
            .settle_accessed_dirty(|address| {
                reader(address).unwrap() | if address == leaf_address { 0x60 } else { 0x20 }
            })
            .unwrap();
    }
}

#[test]
fn settlement_unions_validated_roles_per_slot_and_rejects_unvalidated_roles() {
    let mut retained = empty();
    // The same slot serves every level of this recursive supplied walk.
    retained.translate(config(), 0, &mut |_| Ok(0x1003)).unwrap();
    assert_eq!(retained.entry_count, 1);
    assert_eq!(retained.entries[0].allowed_set_bits, 0x60);
    retained.settle_accessed_dirty(|_| 0x1063).unwrap();
    assert_eq!(retained.compare(|_| 0x1023), Err(TableError::Changed));
    let mut invalid = empty();
    assert_eq!(invalid.translate(config(), 0, &mut |_| Ok(0x1083)), Err(TableError::Translation));
    assert_eq!(invalid.entries[0].allowed_set_bits, 0);
    assert_eq!(invalid.settle_accessed_dirty(|_| 0x10a3), Err(TableError::Changed));
}

#[test]
fn owned_extent_shape_bounds_and_overlap_are_checked_without_reads() {
    assert_eq!(validate_owned_ranges(&[], 0), Ok(0)); // Legacy path unchanged.
    let page = OwnedRange { base: 0x20_0000, bytes: 4096 };
    assert_eq!(validate_owned_ranges(&[page], 48), Ok(1));
    assert_eq!(validate_owned_ranges(&[page; 9], 48), Err(TableError::Bounds));
    assert_eq!(validate_owned_ranges(&[page; 2], 48), Err(TableError::OwnedRange));
    for invalid in [
        OwnedRange { bytes: 0, ..page },
        OwnedRange { base: page.base + 1, ..page },
        OwnedRange { bytes: 4097, ..page },
        OwnedRange { base: u64::MAX & !4095, ..page },
        OwnedRange { base: 1 << 48, ..page },
        OwnedRange { base: 1 << 47, ..page },
    ] {
        assert_eq!(validate_owned_ranges(&[invalid], 48), Err(TableError::OwnedRange));
    }
    assert_eq!(validate_owned_ranges(&[page], 53), Err(TableError::OwnedRange));
    assert_eq!(
        validate_owned_ranges(&[OwnedRange { bytes: 257 * 4096, ..page }], 48),
        Err(TableError::Bounds)
    );
    assert_eq!(
        validate_owned_ranges(
            &[
                OwnedRange { bytes: 255 * 4096, ..page },
                OwnedRange { base: 0x40_0000, bytes: 4096 },
            ],
            48
        ),
        Ok(256)
    );
    assert_eq!(
        validate_owned_ranges(
            &[
                OwnedRange { bytes: 256 * 4096, ..page },
                OwnedRange { base: 0x40_0000, bytes: 4096 },
            ],
            48
        ),
        Err(TableError::Bounds)
    );
    assert_eq!(
        validate_owned_ranges(&[page, OwnedRange { base: page.base + 4096, ..page }], 48),
        Ok(2)
    );
}

#[test]
fn owned_metadata_holes_and_unallocated_or_non_wb_ram_never_reach_readers() {
    let range = OwnedRange { base: 0x20_0000, bytes: 8192 };
    for descriptor in [
        ram(range.base, 1),
        MemoryDescriptor { memory_type: 7, ..ram(range.base, 2) },
        MemoryDescriptor { memory_type: 11, ..ram(range.base, 2) },
        MemoryDescriptor { attributes: 0x2008, ..ram(range.base, 2) },
        MemoryDescriptor { attributes: 1, ..ram(range.base, 2) },
    ] {
        let descriptors = [descriptor];
        let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        let mut storage = empty_storage();
        assert_eq!(
            retain_owned_mappings(
                &mut storage,
                &[range],
                &memory,
                config(),
                &mut |_| panic!("metadata rejection must precede current permission"),
                &mut |_| panic!("metadata rejection must precede table reads")
            ),
            Err(TableError::Metadata)
        );
        assert_eq!(storage.walks.entry_count, 0);
    }
}

#[test]
fn entry_metadata_and_real_query_failure_stop_before_the_source_load() {
    let descriptors = [ram(0x1000, 1)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    for address in [0, 0x1001, 0x2000, 1 << 47] {
        assert_eq!(
            read_checked_entry(
                &memory,
                address,
                |_| panic!("bad metadata reached permission query"),
                |_| panic!("bad metadata reached source load")
            ),
            Err(TableError::ReadPermission)
        );
    }
    assert_eq!(
        read_checked_entry(&memory, 0x1000, |_| false, |_| panic!("denied source was read")),
        Err(TableError::ReadPermission)
    );
    assert_eq!(read_checked_entry(&memory, 0x1ff8, |_| true, |_| 0x2003), Ok(0x2003));
}

#[test]
fn owned_pages_require_current_write_permission_identity_and_every_ancestor_rw() {
    let descriptors = [ram(0, 2048)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let ranges = [OwnedRange { base: 0x20_0000, bytes: 4096 }];
    assert_eq!(
        retain_owned_mappings(
            &mut empty_storage(),
            &ranges,
            &memory,
            config(),
            &mut |_| false,
            &mut |_| panic!("denied page reached walker")
        ),
        Err(TableError::ReadPermission)
    );
    for (root, leaf, error) in [
        (0x2001, 0x20_0083, TableError::NotWritable),
        (0x2003, 0x20_0081, TableError::NotWritable),
        (0x2003, 0x40_0083, TableError::NonIdentity),
    ] {
        let mut read = |address| match address {
            0x1000 => Ok(root),
            0x2000 => Ok(0x3003),
            0x3008 => Ok(leaf),
            _ => Err(TableError::ReadPermission),
        };
        assert_eq!(
            retain_owned_mappings(
                &mut empty_storage(),
                &ranges,
                &memory,
                config(),
                &mut |_| true,
                &mut read
            ),
            Err(error)
        );
    }
    assert_eq!(
        retain_owned_mappings(
            &mut empty_storage(),
            &ranges,
            &memory,
            PagingConfig { pcid: true, ..config() },
            &mut |_| panic!("PCID refusal must precede query"),
            &mut |_| panic!("PCID refusal must precede walk")
        ),
        Err(TableError::TableFetch)
    );
}

#[test]
fn retains_33_owned_pages_and_separates_parent_fetch_indices_from_leaf_pat() {
    let descriptors = [ram(0, 2048)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let ranges = [OwnedRange { base: 0x20_0000, bytes: 33 * 4096 }];
    let config = PagingConfig { cr3: 0x1018, ..config() };
    let mut read = |address| match address {
        0x1000 => Ok(0x200b),    // PDPT fetched with PAT1.
        0x2000 => Ok(0x3013),    // PD fetched with PAT2.
        0x3000 => Ok(0x1083),    // Software aliases of tables select PAT4.
        0x3008 => Ok(0x20_109b), // Arena leaf selects PAT7, not table-fetch PAT.
        _ => Err(TableError::ReadPermission),
    };
    let mut storage = empty_storage();
    storage.walks.close_dependencies(config, &mut read).unwrap();
    assert_eq!(storage.walks.entry_count, 3);
    retain_owned_mappings(&mut storage, &ranges, &memory, config, &mut |_| true, &mut read)
        .unwrap();
    storage.walks.close_dependencies(config, &mut read).unwrap();
    assert_eq!(storage.owned_page_count, 33);
    assert_eq!(storage.walks.entry_count, 4);
    assert_eq!(storage.walks.page_count, 3);
    for (index, page) in storage.owned_mappings[..33].iter().enumerate() {
        assert_eq!(
            *page,
            LeafObservation {
                physical_page: ranges[0].base + index as u64 * 4096,
                leaf_physical_base: 0x20_0000,
                leaf_bytes: 0x20_0000,
                pat_index: 7
            }
        );
    }
    for (page, expected_fetch, expected_level) in
        [(0x1000, 1 << 3, 1 << 3), (0x2000, 1 << 1, 1 << 2), (0x3000, 1 << 2, 1 << 1)]
    {
        let observed = storage
            .walks
            .pages()
            .unwrap()
            .iter()
            .find(|entry| entry.physical_page == page)
            .unwrap();
        assert_eq!(observed.fetch_pat_indices, expected_fetch);
        assert_eq!(observed.levels, expected_level);
        assert_eq!(
            observed.alias,
            LeafObservation {
                physical_page: page,
                leaf_physical_base: 0,
                leaf_bytes: 0x20_0000,
                pat_index: 4
            }
        );
    }
    assert_eq!(
        storage
            .walks
            .compare(|address| read(address).unwrap() ^ if address == 0x3008 { 8 } else { 0 }),
        Err(TableError::Changed)
    );
}

#[test]
fn owned_4k_walk_discovers_new_table_page_and_closes_its_software_alias() {
    let descriptors = [ram(0, 2048)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    let ranges = [OwnedRange { base: 0x20_0000, bytes: 8192 }];
    let mut read = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3000 => Ok(0x83),
        0x3008 => Ok(0x401b),    // PT fetch index3, RW inherited.
        0x4000 => Ok(0x20_008b), // Actual 4K leaf PAT5.
        0x4008 => Ok(0x20_1003), // Actual next 4K leaf PAT0.
        _ => Err(TableError::ReadPermission),
    };
    let mut storage = empty_storage();
    storage.walks.close_dependencies(config(), &mut read).unwrap();
    retain_owned_mappings(&mut storage, &ranges, &memory, config(), &mut |_| true, &mut read)
        .unwrap();
    storage.walks.close_dependencies(config(), &mut read).unwrap();
    assert_eq!(storage.walks.entry_count, 6);
    assert_eq!(storage.walks.page_count, 4);
    assert_eq!(
        storage.owned_mappings[0],
        LeafObservation {
            physical_page: 0x20_0000,
            leaf_physical_base: 0x20_0000,
            leaf_bytes: 4096,
            pat_index: 5
        }
    );
    assert_eq!(
        storage.owned_mappings[1],
        LeafObservation {
            physical_page: 0x20_1000,
            leaf_physical_base: 0x20_1000,
            leaf_bytes: 4096,
            pat_index: 0
        }
    );
    let pt =
        storage.walks.pages().unwrap().iter().find(|entry| entry.physical_page == 0x4000).unwrap();
    assert_eq!(pt.alias.physical_page, 0x4000);
    assert_eq!(pt.alias.leaf_bytes, 0x20_0000);
    assert_eq!(pt.fetch_pat_indices, 1 << 3);
    assert_eq!(pt.levels, 1);
    assert_eq!(
        storage
            .walks
            .compare(|address| read(address).unwrap() ^ if address == 0x4008 { 0x1000 } else { 0 }),
        Err(TableError::Changed)
    );
}

#[test]
fn retains_full_capacity_across_disjoint_ranges_in_actual_one_gib_leaves() {
    let descriptors = [ram(0, 0xc_0000)];
    let memory = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    // Deliberately supplied out of physical order. Each allocation's own
    // view remains ascending, with full 1 GiB leaf bases preserved.
    let ranges = [
        OwnedRange { base: 0x8000_3000, bytes: 128 * 4096 },
        OwnedRange { base: 0x4000_5000, bytes: 128 * 4096 },
    ];
    let mut read = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3000 => Ok(0x83),
        0x2008 => Ok(0x4000_1083), // 1 GiB leaf, PAT4.
        0x2010 => Ok(0x8000_0093), // 1 GiB leaf, PAT2.
        _ => Err(TableError::ReadPermission),
    };
    let mut storage = empty_storage();
    retain_owned_mappings(&mut storage, &ranges, &memory, config(), &mut |_| true, &mut read)
        .unwrap();
    storage.walks.close_dependencies(config(), &mut read).unwrap();
    assert_eq!(storage.owned_page_count, MAX_OWNED_PAGES);
    assert_eq!(storage.owned_ranges[..2], ranges);
    assert_eq!(
        storage.owned_mappings[127],
        LeafObservation {
            physical_page: ranges[0].base + 127 * 4096,
            leaf_physical_base: 0x8000_0000,
            leaf_bytes: 0x4000_0000,
            pat_index: 2
        }
    );
    assert_eq!(
        storage.owned_mappings[255],
        LeafObservation {
            physical_page: ranges[1].base + 127 * 4096,
            leaf_physical_base: 0x4000_0000,
            leaf_bytes: 0x4000_0000,
            pat_index: 4
        }
    );
    assert_eq!(storage.walks.entry_count, 5);
}

#[test]
fn observed_fetch_alias_roles_are_accumulated_and_invalid_roles_refuse() {
    let mut walks = empty();
    walks.remember_fetch(0x1000, 4, Some(0)).unwrap();
    walks.remember_fetch(0x1000, 1, Some(3)).unwrap();
    assert_eq!(walks.pages().unwrap()[0].fetch_pat_indices, 9);
    assert_eq!(walks.pages().unwrap()[0].levels, 9);
    for (level, index) in [(0, 0), (5, 0), (4, 4), (4, 255)] {
        assert_eq!(walks.remember_fetch(0x2000, level, Some(index)), Err(TableError::Bounds));
    }
    assert_eq!(walks.page_count, 1);
}

#[test]
fn owned_ranges_must_not_overlap_any_borrowed_gdt_byte_or_table_page() {
    let ranges = [OwnedRange { base: 0x20_0000, bytes: 8192 }];
    for (base, bytes) in [(0x1f_ffff, 2), (0x20_1000, 4096), (0x20_1fff, 1)] {
        assert_eq!(reject_owned_overlap(&ranges, base, bytes), Err(TableError::OwnedRange));
    }
    assert_eq!(reject_owned_overlap(&ranges, 0x1f_f000, 4096), Ok(()));
    assert_eq!(reject_owned_overlap(&ranges, 0x20_2000, 4096), Ok(()));
}

#[test]
fn closes_root_and_self_mapping_without_recursion_or_duplicate_entries() {
    let mut retained = empty();
    assert_eq!(
        retained.translate(config(), 0x8000, &mut read_large).unwrap().physical_address,
        0x8000
    );
    assert_eq!(retained.entry_count, 3);
    retained.close_dependencies(config(), &mut read_large).unwrap();
    assert_eq!(retained.entry_count, 3);
    assert_eq!(retained.page_count, 3);
    retained.compare(|address| read_large(address).unwrap()).unwrap();
}

#[test]
fn closes_dependency_pages_newly_discovered_by_root_mapping() {
    // A 4 KiB mapping of the GDT at 0x9000 uses one PT entry. Checking
    // identity access to the root and table-source pages adds four others.
    let mut retained = empty();
    let mut read = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3000 => Ok(0x4003),
        0x4008 => Ok(0x1003),
        0x4010 => Ok(0x2003),
        0x4018 => Ok(0x3003),
        0x4020 => Ok(0x4003),
        0x4048 => Ok(0x9003),
        _ => Err(TableError::ReadPermission),
    };
    retained.translate(config(), 0x9000, &mut read).unwrap();
    assert_eq!(retained.entry_count, 4);
    retained.close_dependencies(config(), &mut read).unwrap();
    assert_eq!(retained.entry_count, 8);
    assert_eq!(retained.page_count, 4);
    assert!(retained.entries().unwrap().iter().any(|entry| entry.address == 0x4008));
}

#[test]
fn refuses_unmapped_root_even_when_gdt_mapping_is_present() {
    let mut retained = empty();
    let mut read = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3000 => Ok(0x4003),
        0x4048 => Ok(0x9003),
        0x4008 => Ok(0),
        _ => Err(TableError::ReadPermission),
    };
    retained.translate(config(), 0x9000, &mut read).unwrap();
    assert_eq!(retained.close_dependencies(config(), &mut read), Err(TableError::Translation));
}

#[test]
fn refuses_nonidentity_root_even_when_gdt_mapping_is_identity() {
    let mut retained = empty();
    let mut read = |address| match address {
        0x1000 => Ok(0x2003),
        0x2000 => Ok(0x3003),
        0x3000 => Ok(0x4003),
        0x4048 => Ok(0x9003),
        0x4008 => Ok(0x5003),
        _ => Err(TableError::ReadPermission),
    };
    retained.translate(config(), 0x9000, &mut read).unwrap();
    assert_eq!(retained.close_dependencies(config(), &mut read), Err(TableError::NonIdentity));
}

#[test]
fn unavailable_current_permission_stops_before_recording_an_entry() {
    let mut retained = empty();
    assert_eq!(
        retained.translate(config(), 0x9000, &mut |_| Err(TableError::ReadPermission)),
        Err(TableError::ReadPermission)
    );
    assert_eq!(retained.entry_count, 0);
}

#[test]
fn rejects_drift_on_repeated_walk_including_accessed_and_dirty_bits() {
    for bit in [0, 1, 5, 6, 12, 63] {
        let mut retained = empty();
        retained.translate(config(), 0x9000, &mut read_large).unwrap();
        assert_eq!(
            retained.translate(config(), 0x9000, &mut |address| read_large(address)
                .map(|entry| if address == 0x1000 { entry ^ (1 << bit) } else { entry })),
            Err(TableError::Changed)
        );
    }
}

#[test]
fn comparison_never_follows_a_changed_entry() {
    let mut retained = empty();
    retained.translate(config(), 0x9000, &mut read_large).unwrap();
    let mut reads = 0;
    assert_eq!(
        retained.compare(|address| {
            reads += 1;
            assert_eq!(address, 0x1000);
            0xdead_0003
        }),
        Err(TableError::Changed)
    );
    assert_eq!(reads, 1);
}

#[test]
fn final_baseline_allows_only_setting_accessed_dirty_then_requires_exact_equality() {
    let mut retained = empty();
    retained.translate(config(), 0x9000, &mut read_large).unwrap();
    retained
        .settle_accessed_dirty(|address| {
            read_large(address).unwrap() | if address == 0x3000 { 0x60 } else { 0x20 }
        })
        .unwrap();
    retained
        .compare(|address| {
            read_large(address).unwrap() | if address == 0x3000 { 0x60 } else { 0x20 }
        })
        .unwrap();
    assert_eq!(retained.compare(|address| read_large(address).unwrap()), Err(TableError::Changed));
    assert_eq!(
        retained.settle_accessed_dirty(|address| read_large(address).unwrap()),
        Err(TableError::Changed)
    );
    for changed in [1, 2, 4, 0x1000, 1 << 63] {
        let mut retained = empty();
        retained.translate(config(), 0x9000, &mut read_large).unwrap();
        assert_eq!(
            retained.settle_accessed_dirty(|address| read_large(address).unwrap() ^ changed),
            Err(TableError::Changed)
        );
    }
}

#[test]
fn bounds_exhaustion_and_noncanonical_pages_are_explicit_refusals() {
    let mut entries = empty();
    for index in 0..MAX_RETAINED_ENTRIES {
        entries.remember_entry(0x1000 + index as u64 * 8, 1).unwrap();
    }
    assert_eq!(
        entries.remember_entry(0x1000 + MAX_RETAINED_ENTRIES as u64 * 8, 1),
        Err(TableError::Bounds)
    );
    let mut pages = empty();
    for index in 0..MAX_TABLE_PAGES {
        pages.remember_page(index as u64 * 4096).unwrap();
    }
    assert_eq!(pages.remember_page(MAX_TABLE_PAGES as u64 * 4096), Err(TableError::Bounds));
    assert_eq!(empty().remember_page(0x0000_8000_0000_0000), Err(TableError::Context));
    assert_eq!(empty().remember_entry(0x1001, 1), Err(TableError::Metadata));
}

#[test]
fn high_tpl_requires_if_clear_but_allows_the_expected_if_transition() {
    let before = NativeSnapshot { rflags: 0x202, ..NativeSnapshot::default() };
    let mut after = before;
    after.rflags = 2;
    assert_eq!(context_unchanged(&before, 0x500, &after, 0x500, true), Ok(()));
    assert_eq!(context_unchanged(&before, 0x500, &after, 0x500, false), Err(TableError::Changed));
    assert_eq!(context_unchanged(&before, 0x500, &before, 0x500, true), Err(TableError::Changed));
    after.rflags |= 0x400;
    assert_eq!(context_unchanged(&before, 0x500, &after, 0x500, true), Err(TableError::Changed));
}

#[test]
fn detects_control_table_selector_and_efer_drift_before_live_reads() {
    let before = NativeSnapshot::default();
    for field in 0..10 {
        let mut after = before;
        match field {
            0 => after.cr0 = 1,
            1 => after.cr3 = 1,
            2 => after.cr4 = 1,
            3 => after.gdtr.bytes[0] = 1,
            4 => after.idtr.bytes[2] = 1,
            5 => after.cs = 1,
            6 => after.ss = 1,
            7 => after.ds = 1,
            8 => after.es = 1,
            _ => after.rflags = 0x400,
        }
        assert_eq!(
            context_unchanged(&before, 0x500, &after, 0x500, true),
            Err(TableError::Changed)
        );
    }
    assert_eq!(context_unchanged(&before, 0x500, &before, 0xd00, true), Err(TableError::Changed));
}
