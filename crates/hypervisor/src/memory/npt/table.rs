//! Table storage, entry bits and caller evidence shared by both nested page-table builders.

use crate::{arch::x86_64::capabilities::EvidenceFlag, memory::address::PAGE_BYTES};

/// Storage pages of both the synthetic `Npt` and the resident `IdentityNpt`.
/// The native returning guest reserves exactly this many pages in its fixed
/// arena (dxe `native/resources/guest.rs`); a larger value moves that layout.
pub const TABLE_COUNT: usize = 8;

/// Caller-owned backing storage. Its virtual address does not establish its
/// assigned physical address. Construction clears it only after validation.
#[repr(C, align(4096))]
pub struct TableStorage(pub [[u8; PAGE_BYTES]; TABLE_COUNT]);

pub struct TableView<'a> {
    pub physical_address: u64,
    pub bytes: &'a [u8; PAGE_BYTES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NptEvidence {
    pub nx_supported: EvidenceFlag,
    pub host_nxe: EvidenceFlag,
    /// Long mode with four-level paging (LA57 clear), established by caller.
    pub host_four_level: EvidenceFlag,
}

pub(super) fn indices(gpa: u64) -> [usize; 4] {
    [
        ((gpa >> 39) & 511) as usize,
        ((gpa >> 30) & 511) as usize,
        ((gpa >> 21) & 511) as usize,
        ((gpa >> 12) & 511) as usize,
    ]
}
