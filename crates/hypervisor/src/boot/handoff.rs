//! Versioned, pointer-free description of a one-CPU development arena.
//!
//! This is a preparation schema, not the complete persistent handoff described
//! by the roadmap. It proves neither memory ownership/cacheability nor that any
//! page is mapped, copied, executable, or safe for SVM. The encoded header may be
//! stored as bytes; no Rust structure layout or padding is part of its ABI.
//! A checked u64 address span does not qualify the CPU's physical address width,
//! memory encryption state, WB cache type, or ownership of the described pages.
use crate::memory::address::{AddressError, AddressPolicy, PhysicalRange};
use crate::memory::layout::{Layout, LayoutError, LayoutRequest, PAGE_SIZE};

pub const HANDOFF_SIZE: usize = 80;
pub const HANDOFF_VERSION: u16 = 1;
pub const HANDOFF_MAGIC: [u8; 8] = *b"SVMDEV01";

#[derive(Debug, PartialEq, Eq)]
pub enum HandoffError {
    Size,
    Magic,
    Version,
    Reserved,
    CpuCount,
    ArenaBase,
    AddressOverflow,
    Layout(LayoutError),
}

/// Validated in-process value. All physical addresses remain integer metadata.
pub struct Handoff {
    arena_base: u64,
    arena_bytes: u64,
    request: LayoutRequest,
    layout: Layout,
}

impl Handoff {
    pub fn new(
        arena_base: u64,
        arena_bytes: u64,
        request: LayoutRequest,
        cpu_count: u32,
    ) -> Result<Self, HandoffError> {
        if cpu_count != 1 {
            return Err(HandoffError::CpuCount);
        }
        if arena_base == 0 || !arena_base.is_multiple_of(PAGE_SIZE) {
            return Err(HandoffError::ArenaBase);
        }
        arena_base
            .checked_add(arena_bytes)
            .ok_or(HandoffError::AddressOverflow)?;
        let layout = Layout::plan(arena_bytes, request).map_err(HandoffError::Layout)?;
        Ok(Self {
            arena_base,
            arena_bytes,
            request,
            layout,
        })
    }

    /// Decode exactly one header; reject extensions until a new version defines
    /// them. All multi-byte fields are little endian. Bytes 56..80 are reserved.
    pub fn decode(bytes: &[u8]) -> Result<Self, HandoffError> {
        let bytes: &[u8; HANDOFF_SIZE] = bytes.try_into().map_err(|_| HandoffError::Size)?;
        if bytes[..8] != HANDOFF_MAGIC {
            return Err(HandoffError::Magic);
        }
        if u16::from_le_bytes([bytes[8], bytes[9]]) != HANDOFF_VERSION {
            return Err(HandoffError::Version);
        }
        if u16::from_le_bytes([bytes[10], bytes[11]]) != HANDOFF_SIZE as u16 {
            return Err(HandoffError::Size);
        }
        if bytes[56..].iter().any(|value| *value != 0) {
            return Err(HandoffError::Reserved);
        }
        let cpu_count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        Self::new(
            read_u64(bytes, 16),
            read_u64(bytes, 24),
            LayoutRequest {
                code_bytes: read_u64(bytes, 32),
                data_bytes: read_u64(bytes, 40),
                stack_bytes: read_u64(bytes, 48),
            },
            cpu_count,
        )
    }

    pub fn encode(&self) -> [u8; HANDOFF_SIZE] {
        let mut bytes = [0; HANDOFF_SIZE];
        bytes[..8].copy_from_slice(&HANDOFF_MAGIC);
        bytes[8..10].copy_from_slice(&HANDOFF_VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&(HANDOFF_SIZE as u16).to_le_bytes());
        bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
        for (offset, value) in [
            (16, self.arena_base),
            (24, self.arena_bytes),
            (32, self.request.code_bytes),
            (40, self.request.data_bytes),
            (48, self.request.stack_bytes),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    pub fn arena_base(&self) -> u64 {
        self.arena_base
    }
    pub fn arena_bytes(&self) -> u64 {
        self.arena_bytes
    }
    /// Exclusive physical end, proven representable by construction.
    pub fn arena_end(&self) -> u64 {
        self.arena_base + self.arena_bytes
    }
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Apply a separately established CPU address policy to the entire arena.
    /// Every planned region is contained in this span by the layout invariant.
    /// This still does not establish RAM ownership, WB cacheability or mappings.
    pub fn validate_addresses(
        &self,
        policy: &AddressPolicy,
    ) -> Result<PhysicalRange, AddressError> {
        policy.validate(self.arena_base, self.arena_bytes, PAGE_SIZE)
    }
}

fn read_u64(bytes: &[u8; HANDOFF_SIZE], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}
