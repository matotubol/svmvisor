//! Pure offset planning for a single-CPU reversible synthetic foundation.
//!
//! These are requested final permissions, not installed mappings. The caller
//! must separately prove physical-address limits, WB memory, encryption state,
//! ownership, and executable-memory policy before realizing this plan. No
//! allocation, page-table construction, or CPU operation happens here.
//!
//! Sizes follow the pinned AMD APM volume 2 revision 3.44: section 15.5.1
//! (page-aligned VMCB), I/O intercept IOPM paragraph (12 KiB), table 15-8
//! (MSRPM offsets through 0x1fff), and section 15.30.4 (4 KiB HSAVE).
//! See docs/amd64_apm_vol2_markdown/24593_3.44_APM_Vol2_standalone.md.
//! The three distinct VMCBs and two stack guards are project policy. This
//! deliberately omits the later emergency stack, page tables, and SMP objects.

pub const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Code,
    Data,
    StackGuardLow,
    Stack,
    StackGuardHigh,
    ExecutionVmcb,
    HostAuxVmcb,
    NativeReturnVmcb,
    Hsave,
    Iopm,
    Msrpm,
}

/// Final host mapping policy; writable executable mappings are unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permissions {
    ReadExecute,
    ReadWriteNoExecute,
    Unmapped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    kind: RegionKind,
    offset: u64,
    len: u64,
    permissions: Permissions,
}

impl Region {
    pub const fn kind(&self) -> RegionKind {
        self.kind
    }
    pub const fn offset(&self) -> u64 {
        self.offset
    }
    pub const fn len(&self) -> u64 {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub const fn permissions(&self) -> Permissions {
        self.permissions
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutRequest {
    pub code_bytes: u64,
    pub data_bytes: u64,
    pub stack_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    InvalidSize,
    Overflow,
    InsufficientArena,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    regions: [Region; 11],
    used_bytes: u64,
}

impl Layout {
    /// Plan disjoint ranges starting at offset zero. Arena length must be a
    /// nonzero page multiple. Requested sizes are nonzero and rounded up.
    /// The eventual arena base must separately be page aligned.
    pub fn plan(arena_bytes: u64, request: LayoutRequest) -> Result<Self, LayoutError> {
        if arena_bytes == 0 || !arena_bytes.is_multiple_of(PAGE_SIZE) {
            return Err(LayoutError::InvalidSize);
        }
        let code = round_pages(request.code_bytes)?;
        let data = round_pages(request.data_bytes)?;
        let stack = round_pages(request.stack_bytes)?;
        use Permissions::{ReadExecute, ReadWriteNoExecute, Unmapped};
        use RegionKind::*;
        let specifications = [
            (Code, code, ReadExecute),
            (Data, data, ReadWriteNoExecute),
            (StackGuardLow, PAGE_SIZE, Unmapped),
            (Stack, stack, ReadWriteNoExecute),
            (StackGuardHigh, PAGE_SIZE, Unmapped),
            (ExecutionVmcb, PAGE_SIZE, ReadWriteNoExecute),
            (HostAuxVmcb, PAGE_SIZE, ReadWriteNoExecute),
            (NativeReturnVmcb, PAGE_SIZE, ReadWriteNoExecute),
            (Hsave, PAGE_SIZE, ReadWriteNoExecute),
            (Iopm, 3 * PAGE_SIZE, ReadWriteNoExecute),
            (Msrpm, 2 * PAGE_SIZE, ReadWriteNoExecute),
        ];
        let mut regions = [Region { kind: Code, offset: 0, len: 0, permissions: Unmapped }; 11];
        let mut offset = 0u64;
        for (slot, (kind, len, permissions)) in regions.iter_mut().zip(specifications) {
            *slot = Region { kind, offset, len, permissions };
            offset = offset.checked_add(len).ok_or(LayoutError::Overflow)?;
        }
        if offset > arena_bytes {
            return Err(LayoutError::InsufficientArena);
        }
        Ok(Self { regions, used_bytes: offset })
    }

    pub const fn regions(&self) -> &[Region; 11] {
        &self.regions
    }
    pub const fn used_bytes(&self) -> u64 {
        self.used_bytes
    }
}

fn round_pages(bytes: u64) -> Result<u64, LayoutError> {
    if bytes == 0 {
        return Err(LayoutError::InvalidSize);
    }
    bytes
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .ok_or(LayoutError::Overflow)
}
