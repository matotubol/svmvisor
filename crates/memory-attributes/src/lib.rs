//! UEFI Memory Attribute Protocol semantics over an independently safe backend.
//!
//! This crate does not establish safe physical-memory access. It contains no
//! native pointer dereferences, CR3 writes, protocol publication, or activation
//! path. A backend must supply safe reads and transactional, synchronized edits.
#![no_std]

pub mod x86;

pub const READ_PROTECT: u64 = 0x2000;
pub const EXECUTE_PROTECT: u64 = 0x4000;
pub const READ_ONLY: u64 = 0x20000;
pub const ACCESS_MASK: u64 = READ_PROTECT | EXECUTE_PROTECT | READ_ONLY;
pub const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidParameter,
    Unsupported,
    NoMapping,
    OutOfResources,
    AccessDenied,
    DeviceError,
}

impl Error {
    /// Numeric EFI status code without the architecture-sized error bit.
    pub const fn code(self) -> usize {
        match self {
            Self::InvalidParameter => 2,
            Self::Unsupported => 3,
            Self::DeviceError => 7,
            Self::OutOfResources => 9,
            Self::NoMapping => 17,
            Self::AccessDenied => 15,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    /// Original four-level, unencrypted page-table root. No PCID/control bits.
    pub root: u64,
    pub physical_bits: u8,
    pub nxe: bool,
    pub page1gb: bool,
}

/// Backend contract, including requirements that a native implementation must
/// substantiate independently of this protocol's own permission queries.
///
/// Every `read_entry` must safely access the specified physical table entry or
/// return an error. A backend must retain a stable root and stable mapping,
/// permission and cache-control fields for the entire operation, and serialize
/// queries/updates against other users.
/// Identity aliases, RAM/cache/encryption provenance and table ownership are
/// backend responsibilities, not inferred from successful parsing. A read-only
/// backend may borrow safely readable, stable tables and reject every attempted
/// update with `Unsupported`; it need not own the tables it only observes.
/// For a backend that permits updates, existing tables must form an owned,
/// non-shared tree: no cycles, reused child tables,
/// aliases outside the managed domain, or table pages also used as data. A
/// backend with shared tables must first provide equivalent isolated copies.
///
/// For native use the backend must independently establish active PG/PAE/LMA,
/// LA57 disabled, CR0.WP enabled for supervisor RO enforcement, stable CR3 and
/// matching physical-width/NXE/1-GiB-page capabilities. This profile supports no
/// memory-encryption/shared-address bits or protection keys. Pure Get ignores
/// accessed/dirty bits, so hardware may monotonically set those bits during a
/// read-only observation. Updates must also synchronize hardware A/D changes
/// to avoid losing them when publishing staged entries. Other firmware/SMM
/// changes to mapping or permission fields remain excluded; holding only a
/// Rust mutable reference or the adapter's callback lock is insufficient.
///
/// `begin_update` starts an isolated transaction. Writes and allocations are
/// staged; reads in that transaction see staged values. Allocated tables are
/// fresh, zeroed, 4-KiB-aligned, WB-backed, nonaliasing, and within physical width.
/// The backend enforces ownership and firmware-controlled-region policy before
/// publication, returning `AccessDenied` for changes the caller cannot make.
/// `commit_update` atomically publishes the transaction to all affected CPUs,
/// including the required TLB synchronization. An error leaves it abortable
/// without externally visible changes. `abort_update` discards all staged writes
/// and allocations. Aborting is valid after any attempted `begin_update`, even
/// when it returned an error. A native backend must not implement these as
/// unchecked live stores and a local-only TLB flush while other CPUs can run.
pub trait Memory {
    fn read_entry(&mut self, physical_address: u64) -> Result<u64, Error>;
    fn begin_update(&mut self) -> Result<(), Error>;
    fn write_entry(&mut self, physical_address: u64, value: u64) -> Result<(), Error>;
    fn allocate_table(&mut self) -> Result<u64, Error>;
    fn commit_update(&mut self) -> Result<(), Error>;
    fn abort_update(&mut self);
}

/// Full Get/Set/Clear operation surface used by the thin ABI adapter.
pub trait Attributes {
    fn get(&mut self, base: u64, length: u64) -> Result<u64, Error>;
    fn set(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error>;
    fn clear(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error>;
}

pub struct Provider<M> {
    pub config: Config,
    pub memory: M,
}

impl<M: Memory> Attributes for Provider<M> {
    fn get(&mut self, base: u64, length: u64) -> Result<u64, Error> {
        x86::get(&mut self.memory, self.config, base, length)
    }
    fn set(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error> {
        x86::set(&mut self.memory, self.config, base, length, attributes)
    }
    fn clear(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error> {
        x86::clear(&mut self.memory, self.config, base, length, attributes)
    }
}
