//! Independent hardware-permission observations over a transactional host model.
//!
//! This suite never reads CR3, uses a physical pointer, or changes native tables.
//! The observation walker is deliberately separate from the provider under test.

use std::collections::BTreeMap;

use svmvisor_memory_attributes::{
    ACCESS_MASK, Attributes, Config, EXECUTE_PROTECT, Error, Memory, PAGE_SIZE, Provider,
    READ_ONLY, READ_PROTECT,
};

const PRESENT: u64 = 1;
const WRITE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const HUGE: u64 = 1 << 7;
const GLOBAL: u64 = 1 << 8;
const LARGE_PAT: u64 = 1 << 12;
const NX: u64 = 1 << 63;
const SOFTWARE: u64 = (7 << 9) | (1 << 52) | (1 << 58);
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
// Table storage is separate from the managed ranges used in these tests.
const ROOT: u64 = 0x1_0000_0000;
const PDPT: u64 = ROOT + 0x1000;
const PD: u64 = ROOT + 0x2000;
const PT: u64 = ROOT + 0x3000;
const TWO_MIB: u64 = 1 << 21;
const ONE_GIB: u64 = 1 << 30;
const NORMAL: u64 = PRESENT | WRITE | USER;
const DECORATIONS: u64 = PWT | PCD | ACCESSED | DIRTY | GLOBAL | SOFTWARE;

#[derive(Debug, Default)]
struct Counts {
    reads: usize,
    begins: usize,
    writes: usize,
    allocations: usize,
    commits: usize,
    aborts: usize,
}

#[derive(Default)]
struct Faults {
    read_at: Option<usize>,
    write_at: Option<usize>,
    allocation_at: Option<usize>,
    begin: bool,
    commit: bool,
}

struct Transaction {
    entries: BTreeMap<u64, u64>,
    next_table: u64,
}

struct HostMemory {
    live: BTreeMap<u64, u64>,
    transaction: Option<Transaction>,
    next_table: u64,
    counts: Counts,
    faults: Faults,
}

impl HostMemory {
    fn new() -> Self {
        Self {
            live: BTreeMap::new(),
            transaction: None,
            next_table: 0x2_0000_0000,
            counts: Counts::default(),
            faults: Faults::default(),
        }
    }

    fn table(&mut self, address: u64) {
        for index in 0..512 {
            assert!(self.live.insert(address + 8 * index, 0).is_none());
        }
    }

    fn put(&mut self, address: u64, value: u64) {
        *self.live.get_mut(&address).expect("table must be allocated") = value;
    }

    fn no_backend_access(&self) {
        assert_eq!(self.counts.reads, 0);
        assert_eq!(self.counts.begins, 0);
        assert_eq!(self.counts.writes, 0);
        assert_eq!(self.counts.allocations, 0);
        assert_eq!(self.counts.commits, 0);
        assert_eq!(self.counts.aborts, 0);
    }
}

impl Memory for HostMemory {
    fn read_entry(&mut self, address: u64) -> Result<u64, Error> {
        self.counts.reads += 1;
        if self.faults.read_at == Some(self.counts.reads) {
            return Err(Error::DeviceError);
        }
        let entries = self.transaction.as_ref().map_or(&self.live, |t| &t.entries);
        entries.get(&address).copied().ok_or(Error::AccessDenied)
    }

    fn begin_update(&mut self) -> Result<(), Error> {
        self.counts.begins += 1;
        assert!(self.transaction.is_none(), "transactions cannot nest");
        self.transaction =
            Some(Transaction { entries: self.live.clone(), next_table: self.next_table });
        // A partially initialized failed begin is explicitly abortable.
        if self.faults.begin {
            return Err(Error::AccessDenied);
        }
        Ok(())
    }

    fn write_entry(&mut self, address: u64, value: u64) -> Result<(), Error> {
        self.counts.writes += 1;
        if self.faults.write_at == Some(self.counts.writes) {
            return Err(Error::DeviceError);
        }
        let transaction = self.transaction.as_mut().expect("writes must be staged");
        *transaction.entries.get_mut(&address).ok_or(Error::AccessDenied)? = value;
        Ok(())
    }

    fn allocate_table(&mut self) -> Result<u64, Error> {
        self.counts.allocations += 1;
        if self.faults.allocation_at == Some(self.counts.allocations) {
            return Err(Error::OutOfResources);
        }
        let transaction = self.transaction.as_mut().expect("allocations must be staged");
        let address = transaction.next_table;
        transaction.next_table += PAGE_SIZE;
        for index in 0..512 {
            assert!(transaction.entries.insert(address + index * 8, 0).is_none());
        }
        Ok(address)
    }

    fn commit_update(&mut self) -> Result<(), Error> {
        self.counts.commits += 1;
        assert!(self.transaction.is_some());
        if self.faults.commit {
            return Err(Error::DeviceError);
        }
        let transaction = self.transaction.take().unwrap();
        self.live = transaction.entries;
        self.next_table = transaction.next_table;
        Ok(())
    }

    fn abort_update(&mut self) {
        self.counts.aborts += 1;
        assert!(self.transaction.take().is_some(), "abort needs an active transaction");
    }
}

fn provider(memory: HostMemory) -> Provider<HostMemory> {
    Provider { config: Config { root: ROOT, physical_bits: 48, nxe: true, page1gb: true }, memory }
}

fn flat(pages: u64) -> Provider<HostMemory> {
    assert!(pages <= 512);
    let mut memory = HostMemory::new();
    for address in [ROOT, PDPT, PD, PT] {
        memory.table(address);
    }
    memory.put(ROOT, PDPT | NORMAL);
    memory.put(PDPT, PD | NORMAL);
    memory.put(PD, PT | NORMAL);
    for page in 0..pages {
        memory.put(PT + page * 8, (page * PAGE_SIZE) | NORMAL);
    }
    provider(memory)
}

fn large(page_size: u64, decoration: u64) -> Provider<HostMemory> {
    let mut memory = HostMemory::new();
    memory.table(ROOT);
    memory.table(PDPT);
    memory.put(ROOT, PDPT | NORMAL);
    if page_size == ONE_GIB {
        memory.put(PDPT, NORMAL | HUGE | decoration);
    } else {
        assert_eq!(page_size, TWO_MIB);
        memory.table(PD);
        memory.put(PDPT, PD | NORMAL);
        memory.put(PD, NORMAL | HUGE | decoration);
    }
    provider(memory)
}

fn attribute_mask(index: u64) -> u64 {
    (if index & 1 != 0 { READ_PROTECT } else { 0 })
        | (if index & 2 != 0 { READ_ONLY } else { 0 })
        | (if index & 4 != 0 { EXECUTE_PROTECT } else { 0 })
}

fn protect(mut entry: u64, attributes: u64) -> u64 {
    if attributes & READ_PROTECT != 0 {
        entry &= !PRESENT;
    }
    if attributes & READ_ONLY != 0 {
        entry &= !WRITE;
    }
    if attributes & EXECUTE_PROTECT != 0 {
        entry |= NX;
    }
    entry
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    physical: u64,
    attributes: u64,
    user: bool,
    pwt: bool,
    pcd: bool,
    pat: bool,
    global: bool,
    accessed: bool,
    dirty: bool,
    software: u64,
}

/// Independently derive hardware permissions from published entries only.
/// Retained non-present entries remain observable for testing RP restoration.
fn observe(memory: &HostMemory, virtual_address: u64) -> (Observation, u64) {
    let mut table = ROOT;
    let mut present = true;
    let mut writable = true;
    let mut user = true;
    let mut executable = true;
    for shift in [39, 30, 21, 12] {
        let entry = memory.live[&(table + ((virtual_address >> shift) & 511) * 8)];
        present &= entry & PRESENT != 0;
        writable &= entry & WRITE != 0;
        user &= entry & USER != 0;
        executable &= entry & NX == 0;
        if shift == 12 || (shift != 39 && entry & HUGE != 0) {
            let size = 1 << shift;
            let physical = (entry & ADDRESS & !(size - 1)) | (virtual_address & (size - 1));
            return (
                Observation {
                    physical,
                    attributes: (if !present { READ_PROTECT } else { 0 })
                        | (if !writable { READ_ONLY } else { 0 })
                        | (if !executable { EXECUTE_PROTECT } else { 0 }),
                    user,
                    pwt: entry & PWT != 0,
                    pcd: entry & PCD != 0,
                    pat: entry & (if shift == 12 { HUGE } else { LARGE_PAT }) != 0,
                    global: entry & GLOBAL != 0,
                    accessed: entry & ACCESSED != 0,
                    dirty: entry & DIRTY != 0,
                    software: entry & SOFTWARE,
                },
                size,
            );
        }
        assert_ne!(entry, 0, "missing table while independently observing {virtual_address:#x}");
        table = entry & ADDRESS;
    }
    unreachable!()
}

fn assert_rolled_back(map: &Provider<HostMemory>, before: &BTreeMap<u64, u64>, next_table: u64) {
    assert_eq!(
        &map.memory.live, before,
        "failed operation published entry changes or leaked pages"
    );
    assert_eq!(map.memory.next_table, next_table, "failed operation consumed allocation state");
    assert!(map.memory.transaction.is_none(), "failed operation left its transaction active");
    assert!(map.memory.counts.begins <= 1);
    assert_eq!(map.memory.counts.aborts, map.memory.counts.begins);
}

fn sparse(addresses: &[u64]) -> Provider<HostMemory> {
    let mut memory = HostMemory::new();
    memory.table(ROOT);
    let mut next = ROOT + PAGE_SIZE;
    for &address in addresses {
        let mut table = ROOT;
        for shift in [39, 30, 21] {
            let location = table + ((address >> shift) & 511) * 8;
            if memory.live[&location] == 0 {
                memory.table(next);
                memory.put(location, next | NORMAL);
                next += PAGE_SIZE;
            }
            table = memory.live[&location] & ADDRESS;
        }
        memory.put(table + ((address >> 12) & 511) * 8, address | NORMAL | DECORATIONS);
    }
    provider(memory)
}

fn many_small_pages(table_count: u64) -> Provider<HostMemory> {
    assert!(table_count <= 512);
    let mut memory = HostMemory::new();
    for address in [ROOT, PDPT, PD] {
        memory.table(address);
    }
    memory.put(ROOT, PDPT | NORMAL);
    memory.put(PDPT, PD | NORMAL);
    for directory_index in 0..table_count {
        let table = ROOT + 0x100000 + directory_index * PAGE_SIZE;
        memory.table(table);
        memory.put(PD + directory_index * 8, table | NORMAL);
        for index in 0..512 {
            let physical = directory_index * TWO_MIB + index * PAGE_SIZE;
            memory.put(table + index * 8, physical | NORMAL);
        }
    }
    provider(memory)
}

#[test]
fn readonly_get_tolerates_hardware_accessed_dirty_updates() {
    struct HardwareAd(HostMemory);
    impl Memory for HardwareAd {
        fn read_entry(&mut self, address: u64) -> Result<u64, Error> {
            // Hardware may mark aliases accessed while the observer walks,
            // and another cooperating CPU may dirty an unchanged data leaf.
            for (location, value) in &mut self.0.live {
                if *value & PRESENT != 0 {
                    *value |= ACCESSED;
                    if *location >= PT && *location < PT + PAGE_SIZE {
                        *value |= DIRTY;
                    }
                }
            }
            self.0.read_entry(address)
        }
        fn begin_update(&mut self) -> Result<(), Error> {
            panic!("Get started an update")
        }
        fn write_entry(&mut self, _: u64, _: u64) -> Result<(), Error> {
            panic!("Get wrote a table")
        }
        fn allocate_table(&mut self) -> Result<u64, Error> {
            panic!("Get allocated a table")
        }
        fn commit_update(&mut self) -> Result<(), Error> {
            panic!("Get committed an update")
        }
        fn abort_update(&mut self) {
            panic!("Get aborted an update")
        }
    }
    for mask in [0, READ_ONLY, EXECUTE_PROTECT, READ_ONLY | EXECUTE_PROTECT] {
        let mut original = flat(512);
        for page in 0..512 {
            original.memory.put(PT + page * 8, protect((page * PAGE_SIZE) | NORMAL, mask));
        }
        let mut actual = Provider { config: original.config, memory: HardwareAd(original.memory) };
        assert_eq!(actual.get(0, 512 * PAGE_SIZE), Ok(mask));
        assert_eq!(actual.memory.0.live[&PT] & (ACCESSED | DIRTY), ACCESSED | DIRTY);
        assert!(actual.memory.0.counts.reads > 0);
    }
}

#[test]
fn every_mask_is_incremental_from_every_existing_mask() {
    for initial in 0..8 {
        for requested in 1..8 {
            for clear in [false, true] {
                let mut map = flat(32);
                let address = 16 * PAGE_SIZE;
                let initial_mask = attribute_mask(initial);
                let requested_mask = attribute_mask(requested);
                map.memory
                    .put(PT + 16 * 8, protect(address | NORMAL | DECORATIONS | HUGE, initial_mask));
                let (before, _) = observe(&map.memory, address);
                let expected = if clear {
                    initial_mask & !requested_mask
                } else {
                    initial_mask | requested_mask
                };
                let result = if clear {
                    map.clear(address, PAGE_SIZE, requested_mask)
                } else {
                    map.set(address, PAGE_SIZE, requested_mask)
                };
                assert_eq!(
                    result,
                    Ok(()),
                    "initial={initial}, requested={requested}, clear={clear}"
                );
                let (after, size) = observe(&map.memory, address);
                let mut expected_observation = before;
                expected_observation.attributes = expected;
                assert_eq!(after, expected_observation);
                assert_eq!(size, PAGE_SIZE);
                assert_eq!(map.get(address, PAGE_SIZE), Ok(expected));
                assert_eq!(observe(&map.memory, address - PAGE_SIZE).0.attributes, 0);
                assert_eq!(observe(&map.memory, address + PAGE_SIZE).0.attributes, 0);
                assert!(map.memory.transaction.is_none());
                assert_eq!(map.memory.counts.allocations, 0);
            }
        }
    }
}

#[test]
fn get_distinguishes_uniform_and_mixed_ranges_without_mutating() {
    let mut map = flat(4);
    for page in 1..3 {
        map.memory.put(PT + 8 * page, protect((page * PAGE_SIZE) | NORMAL, READ_ONLY));
    }
    let before = map.memory.live.clone();
    assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE * 2), Ok(READ_ONLY));
    assert_eq!(map.get(0, PAGE_SIZE * 3), Err(Error::NoMapping));
    assert_eq!(map.memory.live, before);
    assert_eq!(map.memory.counts.begins, 0);
    assert_eq!(map.memory.counts.writes, 0);
    assert_eq!(map.memory.counts.allocations, 0);
    assert_eq!(map.memory.counts.commits, 0);
}

#[test]
fn invalid_parameters_follow_pinned_edk2_precedence_before_backend_access() {
    for (base, length, expected) in [
        (1, 0, Error::Unsupported),
        (0, 1, Error::Unsupported),
        (1, 1, Error::Unsupported),
        (0, 0, Error::InvalidParameter),
        (PAGE_SIZE - 1, PAGE_SIZE, Error::Unsupported),
        (PAGE_SIZE + 1, PAGE_SIZE, Error::Unsupported),
        (0, PAGE_SIZE - 1, Error::Unsupported),
        (0, PAGE_SIZE + 1, Error::Unsupported),
    ] {
        let mut map = flat(2);
        assert_eq!(map.get(base, length), Err(expected));
        map.memory.no_backend_access();
    }
    for (base, length, attributes, expected) in [
        (0, PAGE_SIZE, 0, Error::InvalidParameter),
        (1, 0, 0, Error::InvalidParameter),
        (1, 0, READ_ONLY, Error::InvalidParameter),
        (1, PAGE_SIZE, READ_ONLY, Error::Unsupported),
        (0, 1, READ_ONLY, Error::Unsupported),
        (0, 0, READ_ONLY, Error::InvalidParameter),
        (1, PAGE_SIZE, READ_ONLY | 1, Error::InvalidParameter),
        (0, PAGE_SIZE, u64::MAX, Error::InvalidParameter),
    ] {
        for clear in [false, true] {
            let mut map = flat(2);
            let result = if clear {
                map.clear(base, length, attributes)
            } else {
                map.set(base, length, attributes)
            };
            assert_eq!(result, Err(expected));
            map.memory.no_backend_access();
        }
    }
}

#[test]
fn invalid_root_width_and_out_of_domain_ranges_are_rejected_without_access() {
    for (root, physical_bits) in [(ROOT + 1, 48), (ROOT, 31), (ROOT, 53), (1 << 49, 48)] {
        let mut map = flat(2);
        map.config.root = root;
        map.config.physical_bits = physical_bits;
        assert!(map.get(0, PAGE_SIZE).is_err());
        map.memory.no_backend_access();
    }
    for (base, length) in [
        (!(PAGE_SIZE - 1), PAGE_SIZE),
        (0, 1 << 63),
        (1 << 47, PAGE_SIZE),
        ((1 << 47) - PAGE_SIZE, 2 * PAGE_SIZE),
        (1 << 48, PAGE_SIZE),
    ] {
        for operation in 0..3 {
            let mut map = flat(2);
            let result = match operation {
                0 => map.get(base, length).map(|_| ()),
                1 => map.set(base, length, READ_ONLY),
                _ => map.clear(base, length, READ_ONLY),
            };
            assert!(result.is_err());
            map.memory.no_backend_access();
        }
    }
}

#[test]
fn nx_inactive_rejects_xp_instead_of_silently_succeeding() {
    for clear in [false, true] {
        let mut map = flat(4);
        map.config.nxe = false;
        assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE), Ok(0));
        let before = map.memory.live.clone();
        let result = if clear {
            map.clear(PAGE_SIZE, PAGE_SIZE, EXECUTE_PROTECT)
        } else {
            map.set(PAGE_SIZE, PAGE_SIZE, EXECUTE_PROTECT)
        };
        assert_eq!(result, Err(Error::Unsupported));
        assert_eq!(map.memory.live, before);
        assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Ok(()));
        assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE), Ok(READ_ONLY));
    }
    let mut map = flat(4);
    map.config.nxe = false;
    map.memory.put(PT + 8, PAGE_SIZE | NORMAL | NX);
    assert!(
        map.get(PAGE_SIZE, PAGE_SIZE).is_err(),
        "reserved NX cannot be presented as a valid mapping"
    );
}

#[test]
fn nonidentity_leaf_is_rejected_by_all_operations() {
    for operation in 0..3 {
        let mut map = flat(4);
        map.memory.put(PT + 8, (3 * PAGE_SIZE) | NORMAL);
        let before = map.memory.live.clone();
        let result = match operation {
            0 => map.get(PAGE_SIZE, PAGE_SIZE).map(|_| ()),
            1 => map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY),
            _ => map.clear(PAGE_SIZE, PAGE_SIZE, READ_ONLY),
        };
        assert!(result.is_err());
        assert_eq!(map.memory.live, before);
        assert!(map.memory.transaction.is_none());
    }
}

#[test]
fn missing_mappings_and_reserved_paging_encodings_are_errors() {
    let corruptions = [
        (ROOT, 0),
        (ROOT, PDPT | NORMAL | HUGE),
        (PDPT, 0),
        (PD, 0),
        (PT + 8, 0),
        (PT + 8, PAGE_SIZE | NORMAL | (1 << 49)),
        (PT + 8, PAGE_SIZE | NORMAL | (1 << 59)),
    ];
    for (address, value) in corruptions {
        let mut map = flat(4);
        map.memory.put(address, value);
        assert!(map.get(PAGE_SIZE, PAGE_SIZE).is_err(), "corruption at {address:#x}: {value:#x}");
    }
    let mut map = large(TWO_MIB, 1 << 13);
    assert!(map.get(PAGE_SIZE, PAGE_SIZE).is_err(), "reserved large-page address bit");
    let mut map = large(ONE_GIB, 1 << 21);
    assert!(map.get(PAGE_SIZE, PAGE_SIZE).is_err(), "reserved 1-GiB address bit");
    let mut map = large(ONE_GIB, 0);
    map.config.page1gb = false;
    assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE), Err(Error::Unsupported));
}

#[test]
fn retained_nonpresent_mapping_can_be_queried_and_restored() {
    let mut map = flat(4);
    let original = PAGE_SIZE | NORMAL | DECORATIONS | HUGE;
    map.memory.put(PT + 8, original);
    assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_PROTECT), Ok(()));
    assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE), Ok(READ_PROTECT));
    assert_eq!(map.memory.live[&(PT + 8)], original & !PRESENT);
    assert_eq!(map.clear(PAGE_SIZE, PAGE_SIZE, READ_PROTECT), Ok(()));
    assert_eq!(map.memory.live[&(PT + 8)], original);
    assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE), Ok(0));
}

#[test]
fn whole_large_leaves_edit_without_splitting() {
    for size in [TWO_MIB, ONE_GIB] {
        let mut map = large(size, DECORATIONS | LARGE_PAT);
        let before = observe(&map.memory, 7 * PAGE_SIZE).0;
        assert_eq!(map.set(0, size, ACCESS_MASK), Ok(()));
        assert_eq!(map.get(0, size), Ok(ACCESS_MASK));
        let (mut after, after_size) = observe(&map.memory, 7 * PAGE_SIZE);
        assert_eq!(after_size, size);
        after.attributes = before.attributes;
        assert_eq!(after, before);
        assert_eq!(map.memory.counts.allocations, 0);
        assert_eq!(map.clear(0, size, ACCESS_MASK), Ok(()));
        assert_eq!(observe(&map.memory, 7 * PAGE_SIZE).0, before);
    }
}

#[test]
fn partial_two_mib_split_preserves_every_neighbor_and_leaf_flag() {
    let mut map = large(TWO_MIB, DECORATIONS | LARGE_PAT);
    let before: Vec<_> = (0..512).map(|page| observe(&map.memory, page * PAGE_SIZE).0).collect();
    let target = 173;
    assert_eq!(map.set(target * PAGE_SIZE, PAGE_SIZE, READ_ONLY | EXECUTE_PROTECT), Ok(()));
    assert_eq!(map.memory.counts.allocations, 1);
    for page in 0..512 {
        let (after, size) = observe(&map.memory, page * PAGE_SIZE);
        let mut expected = before[page as usize].clone();
        if page == target {
            expected.attributes = READ_ONLY | EXECUTE_PROTECT;
        }
        assert_eq!(after, expected, "4-KiB page {page}");
        assert_eq!(size, PAGE_SIZE);
    }
    assert_eq!(map.get(0, TWO_MIB), Err(Error::NoMapping));
}

#[test]
fn partial_one_gib_split_preserves_offsets_and_both_pat_encodings() {
    let mut map = large(ONE_GIB, DECORATIONS | LARGE_PAT);
    let target = 257 * TWO_MIB + 173 * PAGE_SIZE;
    let samples = [
        0,
        PAGE_SIZE,
        256 * TWO_MIB,
        target - PAGE_SIZE,
        target,
        target + PAGE_SIZE,
        258 * TWO_MIB,
        ONE_GIB - PAGE_SIZE,
    ];
    let before: Vec<_> = samples.iter().map(|&address| observe(&map.memory, address).0).collect();
    assert_eq!(map.set(target, PAGE_SIZE, ACCESS_MASK), Ok(()));
    assert_eq!(map.memory.counts.allocations, 2);
    for (&address, original) in samples.iter().zip(before.iter()) {
        let (after, size) = observe(&map.memory, address);
        let mut expected = original.clone();
        if address == target {
            expected.attributes = ACCESS_MASK;
        }
        assert_eq!(after, expected, "address {address:#x}");
        assert_eq!(size, if address / TWO_MIB == target / TWO_MIB { PAGE_SIZE } else { TWO_MIB });
    }
    assert_eq!(map.clear(target, PAGE_SIZE, ACCESS_MASK), Ok(()));
    for (&address, original) in samples.iter().zip(before.iter()) {
        assert_eq!(&observe(&map.memory, address).0, original);
    }
}

#[test]
fn idempotent_subrange_update_keeps_a_large_mapping_intact() {
    for clear in [false, true] {
        let mut map = large(ONE_GIB, if clear { 0 } else { NX });
        let before = map.memory.live.clone();
        let result = if clear {
            map.clear(PAGE_SIZE, PAGE_SIZE, EXECUTE_PROTECT)
        } else {
            map.set(PAGE_SIZE, PAGE_SIZE, EXECUTE_PROTECT)
        };
        assert_eq!(result, Ok(()));
        assert_eq!(map.memory.live, before);
        assert_eq!(map.memory.counts.allocations, 0);
        assert_eq!(observe(&map.memory, PAGE_SIZE).1, ONE_GIB);
    }
}

#[test]
fn clearing_inherited_restrictions_preserves_all_neighbors() {
    for parent in [ROOT, PDPT, PD] {
        for mask in 1..8 {
            let mut map = flat(512);
            let attributes = attribute_mask(mask);
            let parent_entry = map.memory.live[&parent];
            map.memory.put(parent, protect(parent_entry, attributes));
            let before: Vec<_> =
                (0..512).map(|page| observe(&map.memory, page * PAGE_SIZE).0).collect();
            let target = 173;
            assert_eq!(map.get(target * PAGE_SIZE, PAGE_SIZE), Ok(attributes));
            assert_eq!(
                map.clear(target * PAGE_SIZE, PAGE_SIZE, attributes),
                Ok(()),
                "parent={parent:#x}, mask={mask}"
            );
            assert_eq!(map.get(target * PAGE_SIZE, PAGE_SIZE), Ok(0));
            for page in 0..512 {
                let mut expected = before[page as usize].clone();
                if page == target {
                    expected.attributes = 0;
                }
                assert_eq!(
                    observe(&map.memory, page * PAGE_SIZE).0,
                    expected,
                    "parent={parent:#x}, mask={mask}, page={page}"
                );
            }
            // Relaxing an ancestor cannot invent mappings in formerly zero slots.
            assert_eq!(map.memory.live[&(ROOT + 8)], 0);
            assert_eq!(map.memory.live[&(PDPT + 8)], 0);
            assert_eq!(map.memory.live[&(PD + 8)], 0);
        }
    }
}

#[test]
fn clearing_one_inherited_bit_retains_other_inherited_and_leaf_protections() {
    let mut map = flat(512);
    map.memory.put(ROOT, protect(PDPT | NORMAL, ACCESS_MASK));
    for page in 0..512 {
        map.memory.put(
            PT + page * 8,
            protect((page * PAGE_SIZE) | NORMAL | DECORATIONS, attribute_mask(page % 8)),
        );
    }
    let before: Vec<_> = (0..512).map(|page| observe(&map.memory, page * PAGE_SIZE).0).collect();
    let target = 172;
    assert_eq!(map.clear(target * PAGE_SIZE, 3 * PAGE_SIZE, READ_ONLY), Ok(()));
    for page in 0..512 {
        let mut expected = before[page as usize].clone();
        if (target..target + 3).contains(&page) {
            expected.attributes &= !READ_ONLY;
        }
        assert_eq!(observe(&map.memory, page * PAGE_SIZE).0, expected, "page={page}");
    }
}

#[test]
fn missing_later_leaf_is_found_in_preflight_before_any_edit() {
    let mut map = flat(2);
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert!(map.set(0, PAGE_SIZE * 3, READ_ONLY).is_err());
    assert_rolled_back(&map, &before, next_table);
    assert_eq!(map.memory.counts.begins, 0);
    assert_eq!(map.memory.counts.writes, 0);
    assert_eq!(map.memory.counts.commits, 0);
}

#[test]
fn second_split_allocation_failure_rolls_back_first_split() {
    let mut map = large(ONE_GIB, DECORATIONS | LARGE_PAT);
    map.memory.faults.allocation_at = Some(2);
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Err(Error::OutOfResources));
    assert_eq!(map.memory.counts.allocations, 2);
    assert_rolled_back(&map, &before, next_table);
    assert_eq!(map.memory.counts.commits, 0);
}

#[test]
fn late_split_write_failure_rolls_back_allocations_and_entries() {
    let mut map = large(ONE_GIB, DECORATIONS | LARGE_PAT);
    map.memory.faults.write_at = Some(600);
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Err(Error::DeviceError));
    assert_eq!(map.memory.counts.writes, 600);
    assert_rolled_back(&map, &before, next_table);
    assert_eq!(map.memory.counts.commits, 0);
}

#[test]
fn read_failures_in_preflight_and_transaction_leave_live_state_unchanged() {
    for (failure_at, expected_begins) in [(2, 0), (6, 1)] {
        let mut map = flat(4);
        map.memory.faults.read_at = Some(failure_at);
        let before = map.memory.live.clone();
        let next_table = map.memory.next_table;
        assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Err(Error::DeviceError));
        assert_rolled_back(&map, &before, next_table);
        assert_eq!(map.memory.counts.begins, expected_begins);
        assert_eq!(map.memory.counts.commits, 0);
    }
}

#[test]
fn commit_failure_is_abortable_without_visible_changes() {
    let mut map = large(TWO_MIB, DECORATIONS | LARGE_PAT);
    map.memory.faults.commit = true;
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Err(Error::DeviceError));
    assert_eq!(map.memory.counts.commits, 1);
    assert_rolled_back(&map, &before, next_table);
}

#[test]
fn denied_begin_aborts_partial_setup_without_publishing_changes() {
    let mut map = flat(4);
    map.memory.faults.begin = true;
    let before = map.memory.live.clone();
    assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Err(Error::AccessDenied));
    assert_eq!(map.memory.live, before);
    assert_eq!(map.memory.counts.begins, 1);
    assert_eq!(map.memory.counts.writes, 0);
    assert_eq!(map.memory.counts.allocations, 0);
    assert_eq!(map.memory.counts.commits, 0);
    assert_eq!(map.memory.counts.aborts, 1);
    assert!(map.memory.transaction.is_none());
}

#[test]
fn clearing_inherited_permissions_is_atomic_on_child_write_failure() {
    let mut map = flat(512);
    map.memory.put(ROOT, protect(PDPT | NORMAL, ACCESS_MASK));
    map.memory.faults.write_at = Some(5);
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert_eq!(map.clear(17 * PAGE_SIZE, PAGE_SIZE, ACCESS_MASK), Err(Error::DeviceError));
    assert_rolled_back(&map, &before, next_table);
    assert_eq!(map.memory.counts.commits, 0);
}

#[test]
fn an_owned_root_at_physical_zero_is_supported() {
    let mut memory = HostMemory::new();
    for address in [0, PDPT, PD, PT] {
        memory.table(address);
    }
    memory.put(0, PDPT | NORMAL);
    memory.put(PDPT + 8, PD | NORMAL);
    memory.put(PD, PT | NORMAL);
    memory.put(PT, ONE_GIB | NORMAL);
    let mut map = provider(memory);
    map.config.root = 0;
    assert_eq!(map.get(ONE_GIB, PAGE_SIZE), Ok(0));
    assert_eq!(map.set(ONE_GIB, PAGE_SIZE, READ_ONLY), Ok(()));
    assert_eq!(map.get(ONE_GIB, PAGE_SIZE), Ok(READ_ONLY));
    assert_eq!(map.memory.live[&PT], ONE_GIB | NORMAL & !WRITE);
}

#[test]
fn all_zero_final_pte_at_address_zero_can_be_queried_and_restored() {
    let mut map = flat(2);
    map.memory.put(PT, 0);
    assert_eq!(map.get(0, PAGE_SIZE), Ok(READ_PROTECT | READ_ONLY));
    assert_eq!(map.clear(0, PAGE_SIZE, READ_PROTECT), Ok(()));
    assert_eq!(map.memory.live[&PT], PRESENT);
    assert_eq!(observe(&map.memory, 0).0.attributes, READ_ONLY);
    assert_eq!(map.clear(0, PAGE_SIZE, READ_ONLY), Ok(()));
    assert_eq!(map.memory.live[&PT], PRESENT | WRITE);
    assert_eq!(map.get(0, PAGE_SIZE), Ok(0));
}

#[test]
fn parent_propagation_refuses_to_erase_a_reference_to_table_zero() {
    let mut map = flat(4);
    map.memory.table(0);
    map.memory.put(0, PT | NORMAL);
    map.memory.put(PDPT, PRESENT | WRITE); // PD physically at zero, with no extra metadata.
    map.memory.put(ROOT, protect(PDPT | NORMAL, READ_PROTECT | READ_ONLY));
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert_eq!(map.get(PAGE_SIZE, PAGE_SIZE), Ok(READ_PROTECT | READ_ONLY));
    assert_eq!(map.clear(PAGE_SIZE, PAGE_SIZE, READ_PROTECT | READ_ONLY), Err(Error::Unsupported));
    assert_rolled_back(&map, &before, next_table);
}

#[test]
fn invalid_split_allocation_addresses_abort_without_leaking() {
    for allocated_address in [0x2_0000_0001, 1 << 49] {
        let mut map = large(TWO_MIB, LARGE_PAT);
        map.memory.next_table = allocated_address;
        let before = map.memory.live.clone();
        assert_eq!(map.set(PAGE_SIZE, PAGE_SIZE, READ_ONLY), Err(Error::Unsupported));
        assert_rolled_back(&map, &before, allocated_address);
    }
}

#[test]
fn updates_cross_all_three_page_table_boundaries() {
    for boundary in [TWO_MIB, ONE_GIB, 1 << 39] {
        let addresses =
            [boundary - 2 * PAGE_SIZE, boundary - PAGE_SIZE, boundary, boundary + PAGE_SIZE];
        let mut map = sparse(&addresses);
        let before: Vec<_> =
            addresses.iter().map(|&address| observe(&map.memory, address).0).collect();
        assert_eq!(map.get(boundary - PAGE_SIZE, 2 * PAGE_SIZE), Ok(0));
        assert_eq!(map.set(boundary - PAGE_SIZE, 2 * PAGE_SIZE, ACCESS_MASK), Ok(()));
        assert_eq!(map.get(boundary - PAGE_SIZE, 2 * PAGE_SIZE), Ok(ACCESS_MASK));
        assert_eq!(map.clear(boundary, PAGE_SIZE, READ_ONLY), Ok(()));
        assert_eq!(map.get(boundary - PAGE_SIZE, 2 * PAGE_SIZE), Err(Error::NoMapping));
        for (index, &address) in addresses.iter().enumerate() {
            let mut expected = before[index].clone();
            if index == 1 {
                expected.attributes = ACCESS_MASK;
            }
            if index == 2 {
                expected.attributes = ACCESS_MASK & !READ_ONLY;
            }
            assert_eq!(observe(&map.memory, address).0, expected);
        }
    }
}

#[test]
fn query_work_budget_is_bounded_without_allocations() {
    let mut map = many_small_pages(256);
    assert_eq!(map.get(0, 256 * TWO_MIB), Err(Error::OutOfResources));
    assert_eq!(map.memory.counts.reads, svmvisor_memory_attributes::x86::MAX_ENTRY_OPERATIONS);
    assert_eq!(map.memory.counts.begins, 0);
    assert_eq!(map.memory.counts.writes, 0);
    assert_eq!(map.memory.counts.allocations, 0);
}

#[test]
fn edit_work_budget_exhaustion_aborts_already_staged_writes() {
    let mut map = many_small_pages(128);
    let before = map.memory.live.clone();
    let next_table = map.memory.next_table;
    assert_eq!(map.set(0, 128 * TWO_MIB, READ_ONLY), Err(Error::OutOfResources));
    assert!(map.memory.counts.writes > 0, "exercise transaction exhaustion, not only preflight");
    assert_eq!(
        map.memory.counts.reads + map.memory.counts.writes + map.memory.counts.allocations,
        svmvisor_memory_attributes::x86::MAX_ENTRY_OPERATIONS
    );
    assert_rolled_back(&map, &before, next_table);
    assert_eq!(map.memory.counts.commits, 0);
}
