//! Four-level, unencrypted x86 page-table interpretation and transactional edits.
//!
//! This identity-map profile accepts physical widths 32..=52 and addresses below
//! 2^47. Protection keys, five-level tables and reserved encodings are refused.
//! At most [`MAX_ENTRY_OPERATIONS`] backend entry accesses/allocations are made
//! per call, including validation and editing. Large leaves are visited once;
//! work is therefore bounded without forbidding whole 1-GiB leaf operations.
//! An exhausted editing budget aborts the isolated transaction.
//! An edit that would encode an existing nonleaf reference as all-zero is
//! refused: zero parents denote absence, even when physical table zero exists.

use crate::{
    ACCESS_MASK, Config, EXECUTE_PROTECT, Error, Memory, PAGE_SIZE, READ_ONLY, READ_PROTECT,
};

/// Bound on reads, writes and allocations, shared by preflight and transaction.
pub const MAX_ENTRY_OPERATIONS: usize = 131_072;

// Standalone copies of PRESENT, WRITE, USER, NX and ADDRESS_MASK in hypervisor `memory::address`.
const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const LARGE_OR_PAT: u64 = 1 << 7;
const LARGE_PAT: u64 = 1 << 12;
const NX: u64 = 1 << 63;
const ADDRESS_FIELD: u64 = 0x000f_ffff_ffff_f000;
const HIGH_SOFTWARE: u64 = 0x07f0_0000_0000_0000;
const CANONICAL_LIMIT: u64 = 1 << 47;

struct Access<'a, M> {
    memory: &'a mut M,
    config: Config,
    remaining: usize,
}

impl<M: Memory> Access<'_, M> {
    fn read(&mut self, location: u64) -> Result<u64, Error> {
        self.charge()?;
        self.memory.read_entry(location)
    }

    fn charge(&mut self) -> Result<(), Error> {
        if self.remaining == 0 {
            return Err(Error::OutOfResources);
        }
        self.remaining -= 1;
        Ok(())
    }

    fn write(&mut self, location: u64, value: u64) -> Result<(), Error> {
        self.charge()?;
        self.memory.write_entry(location, value)
    }

    fn allocate(&mut self) -> Result<u64, Error> {
        self.charge()?;
        let table = self.memory.allocate_table()?;
        if table & (PAGE_SIZE - 1) != 0 || table & !address_mask(self.config) != 0 {
            return Err(Error::Unsupported);
        }
        Ok(table)
    }
}

#[derive(Clone, Copy)]
enum Entry {
    Empty,
    Table(u64),
    Leaf(u64),
}

#[derive(Clone, Copy)]
struct Edit {
    attributes: u64,
    set: bool,
}

impl Edit {
    fn needed(self, effective: u64) -> bool {
        if self.set {
            effective & self.attributes != self.attributes
        } else {
            effective & self.attributes != 0
        }
    }
}

#[derive(Default)]
struct Scan {
    first: Option<u64>,
    heterogeneous: bool,
    change_needed: bool,
}

#[derive(Clone, Copy)]
struct Walk {
    table: u64,
    level: u8,
    table_base: u64,
    start: u64,
    end: u64,
    inherited: u64,
}

impl Walk {
    fn root(table: u64, start: u64, end: u64) -> Self {
        Self { table, level: 4, table_base: 0, start, end, inherited: 0 }
    }

    fn child(self, table: u64, base: u64, inherited: u64) -> Self {
        Self {
            table,
            level: self.level - 1,
            table_base: base,
            start: self.start.max(base),
            end: self.end.min(base + span(self.level)),
            inherited,
        }
    }
}

/// Read effective P/RW/NX protection across the entire identity-mapped range.
/// Mixed effective permissions produce `NoMapping`; no table is allocated.
pub fn get<M: Memory>(
    memory: &mut M,
    config: Config,
    base: u64,
    length: u64,
) -> Result<u64, Error> {
    let end = validate_get(base, length, config)?;
    let mut access = Access { memory, config, remaining: MAX_ENTRY_OPERATIONS };
    let mut result = Scan::default();
    scan(&mut access, Walk::root(config.root, base, end), None, &mut result)?;
    if result.heterogeneous {
        Err(Error::NoMapping)
    } else {
        result.first.ok_or(Error::Unsupported)
    }
}

/// Add every requested access restriction without replacing unrelated bits.
pub fn set<M: Memory>(
    memory: &mut M,
    config: Config,
    base: u64,
    length: u64,
    attributes: u64,
) -> Result<(), Error> {
    update(memory, config, base, length, Edit { attributes, set: true })
}

/// Remove every requested access restriction while preserving the effective
/// permissions of addresses outside the range, including parent restrictions.
pub fn clear<M: Memory>(
    memory: &mut M,
    config: Config,
    base: u64,
    length: u64,
    attributes: u64,
) -> Result<(), Error> {
    update(memory, config, base, length, Edit { attributes, set: false })
}

fn update<M: Memory>(
    memory: &mut M,
    config: Config,
    base: u64,
    length: u64,
    edit: Edit,
) -> Result<(), Error> {
    let end = validate_edit(base, length, edit, config)?;
    let mut access = Access { memory, config, remaining: MAX_ENTRY_OPERATIONS };
    let mut result = Scan::default();
    scan(&mut access, Walk::root(config.root, base, end), Some(edit), &mut result)?;
    if !result.change_needed {
        return Ok(());
    }
    // A failed begin may have staged internal work; the contract makes it
    // abortable just like every later failure. No live writes occur here.
    if let Err(error) = access.memory.begin_update() {
        access.memory.abort_update();
        return Err(error);
    }
    let result = edit_table(&mut access, Walk::root(config.root, base, end), edit)
        .and_then(|()| access.memory.commit_update());
    if result.is_err() {
        access.memory.abort_update();
    }
    result
}

fn validate_get(base: u64, length: u64, config: Config) -> Result<u64, Error> {
    if base & (PAGE_SIZE - 1) != 0 {
        return Err(Error::Unsupported);
    }
    if length & (PAGE_SIZE - 1) != 0 {
        return Err(Error::Unsupported);
    }
    if length == 0 {
        return Err(Error::InvalidParameter);
    }
    range_end(base, length, config)
}

fn validate_edit(base: u64, length: u64, edit: Edit, config: Config) -> Result<u64, Error> {
    // Keep the public protocol's validation order, including combined failures.
    if edit.attributes == 0 || edit.attributes & !ACCESS_MASK != 0 {
        return Err(Error::InvalidParameter);
    }
    if length == 0 {
        return Err(Error::InvalidParameter);
    }
    if base & (PAGE_SIZE - 1) != 0 {
        return Err(Error::Unsupported);
    }
    if length & (PAGE_SIZE - 1) != 0 {
        return Err(Error::Unsupported);
    }
    let end = range_end(base, length, config)?;
    if !config.nxe && edit.attributes & EXECUTE_PROTECT != 0 {
        return Err(Error::Unsupported);
    }
    Ok(end)
}

fn range_end(base: u64, length: u64, config: Config) -> Result<u64, Error> {
    validate_config(config)?;
    let end = base.checked_add(length).ok_or(Error::InvalidParameter)?;
    if end > CANONICAL_LIMIT || end > (1u64 << config.physical_bits) {
        return Err(Error::Unsupported);
    }
    Ok(end)
}

fn validate_config(config: Config) -> Result<(), Error> {
    if !(32..=52).contains(&config.physical_bits) {
        return Err(Error::Unsupported);
    }
    if config.root & (PAGE_SIZE - 1) != 0 || config.root & !address_mask(config) != 0 {
        return Err(Error::Unsupported);
    }
    Ok(())
}

fn scan<M: Memory>(
    access: &mut Access<'_, M>,
    walk: Walk,
    edit: Option<Edit>,
    result: &mut Scan,
) -> Result<(), Error> {
    let Walk { table, level, table_base, start, end, inherited } = walk;
    let size = span(level);
    let first = (start - table_base) / size;
    let last = (end - 1 - table_base) / size;
    for index in first..=last {
        let node_base = table_base + index * size;
        let value = access.read(table + index * 8)?;
        let effective = inherited | attributes(value);
        match queried_entry(value, level, node_base, access.config)? {
            Entry::Table(child) => {
                scan(access, walk.child(child, node_base, effective), edit, result)?
            }
            Entry::Leaf(physical) => {
                if physical != node_base {
                    return Err(Error::Unsupported);
                }
                if let Some(first) = result.first {
                    result.heterogeneous |= first != effective;
                } else {
                    result.first = Some(effective);
                }
                if let Some(edit) = edit {
                    result.change_needed |= edit.needed(effective);
                }
            }
            Entry::Empty => return Err(Error::Unsupported),
        }
    }
    Ok(())
}

fn edit_table<M: Memory>(access: &mut Access<'_, M>, walk: Walk, edit: Edit) -> Result<(), Error> {
    let Walk { table, level, table_base, start, end, inherited } = walk;
    let size = span(level);
    let first = (start - table_base) / size;
    let last = (end - 1 - table_base) / size;
    for index in first..=last {
        let node_base = table_base + index * size;
        let node_end = node_base + size;
        let location = table + index * 8;
        let value = access.read(location)?;
        let own_attributes = attributes(value);
        let effective = inherited | own_attributes;
        let entry = queried_entry(value, level, node_base, access.config)?;
        // A restriction already imposed above a subtree satisfies Set for it.
        if edit.set && !edit.needed(effective) {
            continue;
        }
        let full = start <= node_base && end >= node_end;
        let child;
        let child_inherited;
        match entry {
            Entry::Table(next) => {
                child = next;
                if !edit.set {
                    let lifted = own_attributes & edit.attributes;
                    if lifted != 0 {
                        if !full {
                            push_restrictions(access, child, level - 1, node_base, lifted)?;
                        }
                        let updated = remove_restrictions(value, lifted);
                        if updated == 0 {
                            return Err(Error::Unsupported);
                        }
                        access.write(location, updated)?;
                    }
                    child_inherited = inherited | (own_attributes & !lifted);
                } else {
                    child_inherited = effective;
                }
            }
            Entry::Leaf(physical) => {
                // Preflight verified this; check again against transaction reads.
                if physical != node_base {
                    return Err(Error::Unsupported);
                }
                if !edit.needed(effective) {
                    continue;
                }
                if full || level == 1 {
                    let updated = if edit.set {
                        add_restrictions(value, edit.attributes)
                    } else {
                        remove_restrictions(value, edit.attributes)
                    };
                    if updated != value {
                        access.write(location, updated)?;
                    }
                    continue;
                }
                child = split(access, value, level, physical)?;
                // The new table is WB memory, independent of the data's cache
                // mode. Children hold every original permission (including U/S).
                access.write(location, child | PRESENT | WRITABLE | USER)?;
                child_inherited = inherited;
            }
            Entry::Empty => return Err(Error::Unsupported),
        }
        edit_table(access, walk.child(child, node_base, child_inherited), edit)?;
    }
    Ok(())
}

fn split<M: Memory>(
    access: &mut Access<'_, M>,
    value: u64,
    level: u8,
    physical: u64,
) -> Result<u64, Error> {
    let table = access.allocate()?;
    let child_level = level - 1;
    let child_size = span(child_level);
    // Data cache flags, A/D, global, access and software bits survive in leaves.
    let mut flags = value & !ADDRESS_FIELD & !LARGE_OR_PAT;
    if child_level > 1 {
        flags |= LARGE_OR_PAT;
    }
    if value & LARGE_PAT != 0 {
        flags |= if child_level == 1 { LARGE_OR_PAT } else { LARGE_PAT };
    }
    for index in 0..512 {
        access.write(table + index * 8, (physical + index * child_size) | flags)?;
    }
    Ok(table)
}

/// Before relaxing a partial parent, transfer only its lifted restrictions to
/// every existing child. All-zero absent entries remain absent. The PA-zero
/// final PTE exception remains a stored mapping and receives the restrictions.
fn push_restrictions<M: Memory>(
    access: &mut Access<'_, M>,
    table: u64,
    child_level: u8,
    table_base: u64,
    restrictions: u64,
) -> Result<(), Error> {
    for index in 0..512 {
        let location = table + index * 8;
        let value = access.read(location)?;
        let parsed = parse(value, child_level, access.config)?;
        let base = table_base + index * span(child_level);
        if matches!(parsed, Entry::Empty) && !(child_level == 1 && base == 0) {
            continue;
        }
        let updated = add_restrictions(value, restrictions);
        if updated == 0 && child_level > 1 {
            return Err(Error::Unsupported);
        }
        if updated != value {
            access.write(location, updated)?;
        }
    }
    Ok(())
}

/// An all-zero final PTE at address zero is the reference PA-zero exception.
/// Zero intermediate entries and zero PTEs elsewhere are absent, not mappings.
fn queried_entry(value: u64, level: u8, base: u64, config: Config) -> Result<Entry, Error> {
    match parse(value, level, config)? {
        Entry::Empty if level == 1 && base == 0 => Ok(Entry::Leaf(0)),
        Entry::Empty => Err(Error::Unsupported),
        entry => Ok(entry),
    }
}

fn parse(value: u64, level: u8, config: Config) -> Result<Entry, Error> {
    if value == 0 {
        return Ok(Entry::Empty);
    }
    let allowed = address_mask(config) | 0xfff | HIGH_SOFTWARE | if config.nxe { NX } else { 0 };
    // This also rejects physical-address bits above MAXPHYADDR and all keys.
    if value & !allowed != 0 || (level == 4 && value & LARGE_OR_PAT != 0) {
        return Err(Error::Unsupported);
    }
    let large = level > 1 && value & LARGE_OR_PAT != 0;
    if large && level == 3 && !config.page1gb {
        return Err(Error::Unsupported);
    }
    if level == 1 || large {
        let size = span(level);
        if large && value & ((size - 1) & ADDRESS_FIELD & !LARGE_PAT) != 0 {
            return Err(Error::Unsupported);
        }
        Ok(Entry::Leaf(value & address_mask(config) & !(size - 1)))
    } else {
        Ok(Entry::Table(value & address_mask(config)))
    }
}

fn attributes(value: u64) -> u64 {
    let mut result = 0;
    if value & PRESENT == 0 {
        result |= READ_PROTECT;
    }
    if value & WRITABLE == 0 {
        result |= READ_ONLY;
    }
    if value & NX != 0 {
        result |= EXECUTE_PROTECT;
    }
    result
}

fn add_restrictions(mut value: u64, mask: u64) -> u64 {
    if mask & READ_PROTECT != 0 {
        value &= !PRESENT;
    }
    if mask & READ_ONLY != 0 {
        value &= !WRITABLE;
    }
    if mask & EXECUTE_PROTECT != 0 {
        value |= NX;
    }
    value
}

fn remove_restrictions(mut value: u64, mask: u64) -> u64 {
    if mask & READ_PROTECT != 0 {
        value |= PRESENT;
    }
    if mask & READ_ONLY != 0 {
        value |= WRITABLE;
    }
    if mask & EXECUTE_PROTECT != 0 {
        value &= !NX;
    }
    value
}

fn span(level: u8) -> u64 {
    1u64 << (12 + 9 * (level - 1))
}

fn address_mask(config: Config) -> u64 {
    ((1u64 << config.physical_bits) - 1) & ADDRESS_FIELD
}
