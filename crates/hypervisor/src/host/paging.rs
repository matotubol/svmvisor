//! Bounded parsing of caller-supplied, unencrypted four-level host paging.
//!
//! The callback reads a u64 at a physical address through an independently safe
//! mechanism. This module never dereferences physical memory. A parsed mapping
//! proves neither trustworthy/WB backing nor ownership, coherent tables, TLB
//! agreement, SME/SEV state, or safe native access. R/W is the paging permission
//! intersection; CR0.WP, SMEP/SMAP, protection keys and access privilege are not
//! modeled, nor is CET. Unsupported upper software/protection-key bits fail
//! conservatively.

use crate::memory::address::{ADDRESS_MASK, NX, is_canonical_48};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagingConfig {
    pub cr3: u64,
    pub physical_bits: u8,
    pub la57: bool,
    pub nxe: bool,
    pub pcid: bool,
    pub page1gb: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub physical_address: u64,
    pub page_bytes: u64,
    /// Index selected by this leaf's PAT/PCD/PWT bits, not a memory-type proof.
    /// Parent PCD/PWT controls apply to paging-structure accesses, not this leaf.
    pub pat_index: u8,
    pub writable: bool,
    pub user: bool,
    pub executable: bool,
    /// Bytes left in this leaf, allowing a caller to bound a range check.
    pub remaining_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkError {
    UnsupportedPhysicalWidth,
    FiveLevelUnsupported,
    NoncanonicalAddress,
    InvalidCr3,
    UnreadableTable { level: u8, address: u64 },
    NotPresent { level: u8 },
    ReservedEntry { level: u8 },
    UnsupportedEntryBits { level: u8 },
    OneGiBUnsupported,
    IncompleteWalk,
}

/// Read at most four eight-byte entries. A missing mapping is an explicit
/// NotPresent error; permissions are accumulated only across present entries.
pub fn translate(
    config: PagingConfig,
    virtual_address: u64,
    mut read: impl FnMut(u64) -> Option<u64>,
) -> Result<Translation, WalkError> {
    use WalkError::*;
    if !(32..=52).contains(&config.physical_bits) {
        return Err(UnsupportedPhysicalWidth);
    }
    if config.la57 {
        return Err(FiveLevelUnsupported);
    }
    if !is_canonical_48(virtual_address) {
        return Err(NoncanonicalAddress);
    }
    let physical_mask = (1u64 << config.physical_bits) - 1;
    let low_cr3 = if config.pcid { 0xfff } else { (1 << 3) | (1 << 4) };
    if config.cr3 & !((physical_mask & ADDRESS_MASK) | low_cr3) != 0 {
        return Err(InvalidCr3);
    }
    let mut table = config.cr3 & ADDRESS_MASK;
    let mut writable = true;
    let mut user = true;
    let mut executable = true;
    for level in [4u8, 3, 2, 1] {
        let shift = 12 + 9 * u32::from(level - 1);
        let index = (virtual_address >> shift) & 511;
        let address = table + index * 8;
        let entry = read(address).ok_or(UnreadableTable { level, address })?;
        if entry & 1 == 0 {
            return Err(NotPresent { level });
        }
        if entry & ADDRESS_MASK & !physical_mask != 0 || (!config.nxe && entry & NX != 0) {
            return Err(ReservedEntry { level });
        }
        // Bits52..62 are not all architecturally reserved. This initial parser
        // does not interpret software use or protection keys in this region.
        if entry & 0x7ff0_0000_0000_0000 != 0 {
            return Err(UnsupportedEntryBits { level });
        }
        writable &= entry & 2 != 0;
        user &= entry & 4 != 0;
        executable &= entry & NX == 0;
        let large = level > 1 && entry & (1 << 7) != 0;
        if level == 4 && large {
            return Err(ReservedEntry { level });
        }
        if level == 3 && large && !config.page1gb {
            return Err(OneGiBUnsupported);
        }
        if level == 1 || large {
            let page_bytes = 1u64 << shift;
            // Large-page PAT occupies bit12, while bits13..shift-1 must be zero.
            if large && entry & ((page_bytes - 1) & !0x1fff) != 0 {
                return Err(ReservedEntry { level });
            }
            let offset = virtual_address & (page_bytes - 1);
            let pat_bit = if large { 12 } else { 7 };
            return Ok(Translation {
                physical_address: (entry & ADDRESS_MASK & !(page_bytes - 1)) + offset,
                page_bytes,
                pat_index: (((entry >> 3) & 3) | (((entry >> pat_bit) & 1) << 2)) as u8,
                writable,
                user,
                executable,
                remaining_bytes: page_bytes - offset,
            });
        }
        table = entry & ADDRESS_MASK;
    }
    Err(IncompleteWalk)
}
