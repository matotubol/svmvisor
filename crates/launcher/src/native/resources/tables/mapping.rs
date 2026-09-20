//! Validation and retention of owned-range and borrowed-span mappings.

use svmvisor_hypervisor::{
    boot::memory::ValidatedMemoryMap, host::paging::PagingConfig, memory::address::is_canonical_48,
};

use super::{
    BorrowedAccess, BorrowedSpan, INTERNAL_SPANS, LeafObservation, MAX_BORROWED_PAGES,
    MAX_BORROWED_SPANS, MAX_INTERNAL_PAGES, MAX_OWNED_PAGES, MAX_OWNED_RANGES, OwnedRange,
    TableError, TableStorage, covering_pages,
};

pub(super) fn retain_owned_mappings(
    storage: &mut TableStorage,
    ranges: &[OwnedRange],
    memory: &ValidatedMemoryMap<'_>,
    config: PagingConfig,
    current_writable: &mut impl FnMut(u64) -> bool,
    read: &mut impl FnMut(u64) -> Result<u64, TableError>,
) -> Result<(), TableError> {
    let page_count = validate_owned_ranges(ranges, config.physical_bits)?;
    if !ranges.is_empty() && config.pcid {
        return Err(TableError::TableFetch);
    }
    // Reject any hole/unallocated/non-RAM page across the entire request before
    // invoking current-permission queries or any page-table reader for it.
    for range in ranges {
        for index in 0..range.bytes / 4096 {
            let page = range.base + index * 4096; // Shape/extent validated above.
            memory.permit_gdt_copy(page, 4096).map_err(|_| TableError::Metadata)?;
        }
    }
    let mut next = 0usize;
    for range in ranges {
        for index in 0..range.bytes / 4096 {
            let page = range.base + index * 4096;
            if !current_writable(page) {
                return Err(TableError::ReadPermission);
            }
            let translation = storage.walks.translate(config, page, read)?;
            let observed = LeafObservation::identity(page, translation)?;
            if !translation.writable {
                return Err(TableError::NotWritable);
            }
            *storage.owned_mappings.get_mut(next).ok_or(TableError::Bounds)? = observed;
            next += 1;
        }
    }
    if next != page_count {
        return Err(TableError::Bounds);
    }
    for (destination, source) in storage
        .owned_ranges
        .get_mut(..ranges.len())
        .ok_or(TableError::Bounds)?
        .iter_mut()
        .zip(ranges)
    {
        *destination = *source;
    }
    storage.owned_range_count = ranges.len();
    storage.owned_page_count = page_count;
    Ok(())
}

pub(super) fn retain_borrowed_mappings(
    storage: &mut TableStorage,
    spans: &[BorrowedSpan],
    owned: &[OwnedRange],
    memory: &ValidatedMemoryMap<'_>,
    config: PagingConfig,
    smep: bool,
    internal: bool,
    current: &mut impl FnMut(u64, BorrowedAccess) -> bool,
    read: &mut impl FnMut(u64) -> Result<u64, TableError>,
) -> Result<(), TableError> {
    let page_count = validate_borrowed_spans(spans, config.physical_bits, owned, internal)?;
    if !spans.is_empty() && !internal && config.pcid {
        return Err(TableError::TableFetch);
    }
    let first_span = storage.borrowed_span_count;
    let end_span = first_span.checked_add(spans.len()).ok_or(TableError::Bounds)?;
    let first_mapping = storage.borrowed_page_count;
    let end_mapping = first_mapping.checked_add(page_count).ok_or(TableError::Bounds)?;
    storage.borrowed_spans.get(first_span..end_span).ok_or(TableError::Bounds)?;
    storage.borrowed_mappings.get(first_mapping..end_mapping).ok_or(TableError::Bounds)?;
    // Metadata for this complete request precedes its permission queries and
    // walks. These callbacks never load or execute the borrowed operand bytes.
    for span in spans {
        let (first, count) = covering_pages(*span, config.physical_bits)?;
        for index in 0..count {
            memory
                .permit_gdt_copy(first + index as u64 * 4096, 4096)
                .map_err(|_| TableError::Metadata)?;
        }
    }
    let mut next = first_mapping;
    for span in spans {
        let (first, count) = covering_pages(*span, config.physical_bits)?;
        for index in 0..count {
            let page = first + index as u64 * 4096;
            if !current(page, span.access) {
                return Err(TableError::ReadPermission);
            }
            let translation = storage.walks.translate(config, page, read)?;
            let observed = LeafObservation::identity(page, translation)?;
            if span.access == BorrowedAccess::ReadWrite && !translation.writable {
                return Err(TableError::NotWritable);
            }
            if span.access == BorrowedAccess::ReadExecute
                && (!translation.executable || (smep && translation.user))
            {
                return Err(TableError::NotExecutable);
            }
            *storage.borrowed_mappings.get_mut(next).ok_or(TableError::Bounds)? = observed;
            next += 1;
        }
    }
    if next != end_mapping {
        return Err(TableError::Bounds);
    }
    for (destination, source) in storage
        .borrowed_spans
        .get_mut(first_span..end_span)
        .ok_or(TableError::Bounds)?
        .iter_mut()
        .zip(spans)
    {
        *destination = *source;
    }
    storage.borrowed_span_count = end_span;
    storage.borrowed_page_count = end_mapping;
    if !internal {
        storage.caller_borrowed_span_count = end_span;
    }
    Ok(())
}

pub(super) fn validate_owned_ranges(
    ranges: &[OwnedRange],
    physical_bits: u8,
) -> Result<usize, TableError> {
    if ranges.len() > MAX_OWNED_RANGES {
        return Err(TableError::Bounds);
    }
    if ranges.is_empty() {
        return Ok(0);
    }
    if !(32..=52).contains(&physical_bits) {
        return Err(TableError::OwnedRange);
    }
    let mut pages = 0u64;
    for (index, range) in ranges.iter().enumerate() {
        let end = range.base.checked_add(range.bytes).ok_or(TableError::OwnedRange)?;
        if range.bytes == 0
            || (range.base | range.bytes) & 4095 != 0
            || end > (1u64 << physical_bits)
            || !is_canonical_48(range.base)
            || !is_canonical_48(end - 1)
        {
            return Err(TableError::OwnedRange);
        }
        pages = pages.checked_add(range.bytes / 4096).ok_or(TableError::Bounds)?;
        if pages > MAX_OWNED_PAGES as u64 {
            return Err(TableError::Bounds);
        }
        for previous in ranges.get(..index).ok_or(TableError::Bounds)? {
            let previous_end =
                previous.base.checked_add(previous.bytes).ok_or(TableError::OwnedRange)?;
            if range.base < previous_end && previous.base < end {
                return Err(TableError::OwnedRange);
            }
        }
    }
    Ok(pages as usize)
}

pub(super) fn validate_borrowed_spans(
    spans: &[BorrowedSpan],
    physical_bits: u8,
    owned: &[OwnedRange],
    internal: bool,
) -> Result<usize, TableError> {
    let span_limit = if internal { INTERNAL_SPANS } else { MAX_BORROWED_SPANS };
    let page_limit = if internal { MAX_INTERNAL_PAGES } else { MAX_BORROWED_PAGES };
    if spans.len() > span_limit {
        return Err(TableError::Bounds);
    }
    let mut pages = 0usize;
    for span in spans {
        let (_, count) = covering_pages(*span, physical_bits)?;
        pages = pages.checked_add(count).ok_or(TableError::Bounds)?;
        if pages > page_limit {
            return Err(TableError::Bounds);
        }
        // Only the actual byte extents assert conflicting provenance. Borrowed
        // spans can share covering pages and overlap one another freely.
        reject_owned_overlap(owned, span.base, span.bytes)?;
    }
    Ok(pages)
}

pub(super) fn reject_owned_overlap(
    ranges: &[OwnedRange],
    borrowed_base: u64,
    borrowed_bytes: u64,
) -> Result<(), TableError> {
    let borrowed_end = borrowed_base.checked_add(borrowed_bytes).ok_or(TableError::Bounds)?;
    for range in ranges {
        let end = range.base.checked_add(range.bytes).ok_or(TableError::OwnedRange)?;
        if range.base < borrowed_end && borrowed_base < end {
            return Err(TableError::OwnedRange);
        }
    }
    Ok(())
}

/// Metadata and the actual current-permission query precede every source load.
/// The final callback is the only operation permitted to dereference a source.
pub(super) fn read_checked_entry(
    memory: &ValidatedMemoryMap<'_>,
    address: u64,
    current: impl FnOnce(u64) -> bool,
    read: impl FnOnce(u64) -> u64,
) -> Result<u64, TableError> {
    if !is_canonical_48(address)
        || !is_canonical_48(address | 4095)
        || memory.permit_table_entry(address).is_err()
        || memory.permit_gdt_copy(address & !4095, 4096).is_err()
        || !current(address)
    {
        return Err(TableError::ReadPermission);
    }
    Ok(read(address))
}
