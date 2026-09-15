//! Bounded Memory Attribute Protocol operations delegated to firmware CPU DXE.
//!
//! Get uses the shared, ancestor-aware identity-map walker. Changes are limited
//! to one complete existing leaf with unrestricted ancestors. Partial huge-page
//! changes are refused so the reference CPU driver cannot lose mapping flags
//! while splitting. Multiple firmware calls are never combined into a pretend
//! transaction: a firmware error is returned and may have firmware side effects.
//!
//! No physical-pointer reader, CPU-context qualifier, protocol lookup,
//! installation, or activation is supplied here. Those require independent
//! native evidence. In particular this provider cannot qualify its own reads.

use core::ptr::NonNull;

use svmvisor_memory_attributes::{
    ACCESS_MASK, Attributes, Config, EXECUTE_PROTECT, Error, Memory, PAGE_SIZE, x86,
};
use uefi_raw::{Guid, Status, guid};

const PRESENT: u64 = 1;
const WRITABLE: u64 = 2;
const LARGE: u64 = 1 << 7;
const NX: u64 = 1 << 63;
const ADDRESS_FIELD: u64 = 0x000f_ffff_ffff_f000;
const HIGH_SOFTWARE: u64 = 0x07f0_0000_0000_0000;
const MAX_POLICY_RANGES: usize = 128;

/// An independently qualified, stable view of the original firmware tables.
///
/// # Safety
/// Implementations must make every read safe or return an error, using an
/// independently established alias/protected-load contract, never this
/// provider's Get. `config` must describe the active original four-level root,
/// physical width, NXE and 1-GiB support. PG/PAE/LMA, CR0.WP, no LA57, no memory
/// encryption or protection keys, and compatible cache provenance must already
/// be established. All entries must remain stable from the start of a bridge
/// operation through its firmware callback, except the callback's intended
/// changes. This includes other CPUs, firmware/SMM and hardware A/D updates.
/// The reader and setter must describe the same root/address space. An owned
/// Rust reference, a cached CR3, or this adapter's lock alone does not prove any
/// of these conditions. `Send` does not permit skipping per-CPU qualification.
pub unsafe trait QualifiedTableReader: Send {
    fn config(&self) -> Config;
    fn read_entry(&mut self, physical_address: u64) -> Result<u64, Error>;
}

/// Assign the complete RP/RO/XP mask; zero means remove all three protections.
/// Implementations must independently validate the safety of every invocation.
pub trait Setter: Send {
    fn assign(&mut self, base: u64, length: u64, absolute_access_mask: u64) -> Result<(), Error>;
}

/// An explicitly declared policy interval, with an exclusive checked end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    pub base: u64,
    pub length: u64,
}

impl Range {
    fn end(self) -> Result<u64, Error> {
        if self.length == 0 || (self.base | self.length) & (PAGE_SIZE - 1) != 0 {
            return Err(Error::InvalidParameter);
        }
        self.base
            .checked_add(self.length)
            .ok_or(Error::InvalidParameter)
    }

    fn overlaps(self, other: Self) -> bool {
        // Both operands have been validated, or were derived from a valid leaf.
        self.base < other.base + other.length && other.base < self.base + self.length
    }
}

/// A qualified reader plus a one-call firmware setter and explicit range policy.
///
/// The policy is an additional restriction, not evidence of ownership. The
/// setter's qualification must substantiate ownership and cover all provider,
/// protocol, stack, callback, table-source and other live critical allocations.
/// The bridge also excludes every table page on the target's own walk.
pub struct FirmwareAttributes<'a, R, S> {
    reader: R,
    setter: S,
    owned: &'a [Range],
    protected: &'a [Range],
    poisoned: bool,
}

impl<'a, R: QualifiedTableReader, S: Setter> FirmwareAttributes<'a, R, S> {
    /// Validate bounded, aligned, non-overlapping intervals within each list.
    /// Protected intervals may overlap owned intervals, to exclude subregions.
    pub fn new(
        reader: R,
        setter: S,
        owned: &'a [Range],
        protected: &'a [Range],
    ) -> Result<Self, Error> {
        validate_policy(owned)?;
        validate_policy(protected)?;
        Ok(Self {
            reader,
            setter,
            owned,
            protected,
            poisoned: false,
        })
    }

    /// Inspect diagnostics such as the complete last raw firmware status.
    pub fn setter(&self) -> &S {
        &self.setter
    }

    /// An attempted firmware change failed or could not be verified. Recovery
    /// requires constructing a fresh instance after independent qualification.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Recover a temporary reader and setter for explicit lifecycle cleanup.
    /// Their own failure/poison state is preserved. No hardware state is reset.
    pub fn into_parts(self) -> (R, S) {
        (self.reader, self.setter)
    }

    fn change(&mut self, base: u64, length: u64, mask: u64, clear: bool) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::AccessDenied);
        }
        if mask == 0 || mask & !ACCESS_MASK != 0 || length == 0 {
            return Err(Error::InvalidParameter);
        }
        if (base | length) & (PAGE_SIZE - 1) != 0 {
            return Err(Error::Unsupported);
        }
        let config = self.reader.config();
        // x86::get performs checked address/configuration/encoding validation,
        // aggregates ancestors, and verifies every queried leaf's identity.
        // Match the core editing profile even when an NX operation is a no-op.
        if !config.nxe && mask & EXECUTE_PROTECT != 0 {
            return Err(Error::Unsupported);
        }
        let old =
            x86::get(&mut ReadOnly(&mut self.reader), config, base, length).map_err(|error| {
                // Get reports heterogeneous permissions as NO_MAPPING. Such a
                // range is outside this setter's single-leaf mutation profile.
                if error == Error::NoMapping {
                    Error::Unsupported
                } else {
                    error
                }
            })?;
        let absolute = if clear { old & !mask } else { old | mask };
        // A proven uniform no-op needs no writable policy grant or firmware call.
        if absolute == old {
            return Ok(());
        }
        let request = Range { base, length };
        let request_end = request.end()?;
        if !self
            .owned
            .iter()
            .any(|range| range.base <= base && request_end <= range.base + range.length)
            || self.protected.iter().any(|range| range.overlaps(request))
        {
            return Err(Error::AccessDenied);
        }
        let original_path = qualify_single_leaf(&mut self.reader, config, request)?;
        // Exactly one absolute access-mask request. No caching attribute is set.
        self.poisoned = true;
        self.setter.assign(base, length, absolute)?;
        let after = self.reader.config();
        if after.root != config.root
            || after.physical_bits != config.physical_bits
            || after.nxe != config.nxe
            || after.page1gb != config.page1gb
        {
            return Err(Error::DeviceError);
        }
        if x86::get(&mut ReadOnly(&mut self.reader), config, base, length)? != absolute {
            return Err(Error::DeviceError);
        }
        let updated_path = qualify_single_leaf(&mut self.reader, config, request)?;
        if !original_path.same_except_leaf_protection(updated_path) {
            return Err(Error::DeviceError);
        }
        self.poisoned = false;
        Ok(())
    }
}

impl<R: QualifiedTableReader, S: Setter> Attributes for FirmwareAttributes<'_, R, S> {
    fn get(&mut self, base: u64, length: u64) -> Result<u64, Error> {
        if self.poisoned {
            return Err(Error::AccessDenied);
        }
        let config = self.reader.config();
        x86::get(&mut ReadOnly(&mut self.reader), config, base, length)
    }

    fn set(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error> {
        self.change(base, length, attributes, false)
    }

    fn clear(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error> {
        self.change(base, length, attributes, true)
    }
}

fn validate_policy(ranges: &[Range]) -> Result<(), Error> {
    if ranges.len() > MAX_POLICY_RANGES {
        return Err(Error::OutOfResources);
    }
    for (index, range) in ranges.iter().enumerate() {
        range.end()?;
        if ranges[..index].iter().any(|prior| prior.overlaps(*range)) {
            return Err(Error::InvalidParameter);
        }
    }
    Ok(())
}

struct ReadOnly<'a, R>(&'a mut R);

impl<R: QualifiedTableReader> Memory for ReadOnly<'_, R> {
    fn read_entry(&mut self, physical_address: u64) -> Result<u64, Error> {
        self.0.read_entry(physical_address)
    }
    fn begin_update(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn write_entry(&mut self, _: u64, _: u64) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn allocate_table(&mut self) -> Result<u64, Error> {
        Err(Error::Unsupported)
    }
    fn commit_update(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn abort_update(&mut self) {}
}

#[derive(Default)]
struct LeafPath {
    slots: [u64; 4],
    values: [u64; 4],
    count: usize,
}

impl LeafPath {
    fn same_except_leaf_protection(&self, other: Self) -> bool {
        if self.count != other.count || self.slots != other.slots {
            return false;
        }
        for index in 0..self.count {
            let mask = if index + 1 == self.count {
                !(PRESENT | WRITABLE | NX)
            } else {
                u64::MAX
            };
            if self.values[index] & mask != other.values[index] & mask {
                return false;
            }
        }
        true
    }
}

fn qualify_single_leaf<R: QualifiedTableReader>(
    reader: &mut R,
    config: Config,
    request: Range,
) -> Result<LeafPath, Error> {
    // The shared walker has already validated configuration and range. Repeat
    // encoding checks on this final path rather than trusting unchecked values.
    let address_mask = ((1u64 << config.physical_bits) - 1) & ADDRESS_FIELD;
    let allowed = address_mask | 0xfff | HIGH_SOFTWARE | if config.nxe { NX } else { 0 };
    let mut table = config.root;
    let mut path = LeafPath::default();
    for level in (1u32..=4).rev() {
        if request.overlaps(Range {
            base: table,
            length: PAGE_SIZE,
        }) {
            return Err(Error::AccessDenied);
        }
        let shift = 12 + 9 * (level - 1);
        let slot = table + ((request.base >> shift) & 511) * 8;
        let value = reader.read_entry(slot)?;
        path.slots[path.count] = slot;
        path.values[path.count] = value;
        path.count += 1;
        if value & !allowed != 0 || (level == 4 && value & LARGE != 0) {
            return Err(Error::Unsupported);
        }
        if value == 0 && !(level == 1 && request.base == 0) {
            return Err(Error::Unsupported);
        }
        let large = level > 1 && value & LARGE != 0;
        if level == 1 || large {
            let size = 1u64 << shift;
            if level == 3 && !config.page1gb {
                return Err(Error::Unsupported);
            }
            if large && value & ((size - 1) & ADDRESS_FIELD & !(1 << 12)) != 0 {
                return Err(Error::Unsupported);
            }
            if request.base & (size - 1) != 0 || request.length != size {
                return Err(Error::Unsupported);
            }
            if value & address_mask & !(size - 1) != request.base {
                return Err(Error::Unsupported);
            }
            return Ok(path);
        }
        // CPU DXE's legacy setter changes leaf permissions. It cannot safely
        // clear inherited restrictions; deny every changed request on that path.
        if value & PRESENT == 0 || value & WRITABLE == 0 || value & NX != 0 {
            return Err(Error::Unsupported);
        }
        table = value & address_mask;
    }
    Err(Error::Unsupported)
}

/// EFI_CPU_ARCH_PROTOCOL GUID, as specified by PI and MdePkg/Protocol/Cpu.h.
pub const CPU_ARCH_PROTOCOL_GUID: Guid = guid!("26baccb1-6f42-11d4-bce7-0080c73c8881");

pub type SetMemoryAttributesFn = unsafe extern "efiapi" fn(
    this: *mut CpuArchProtocol,
    base: u64,
    length: u64,
    attributes: u64,
) -> Status;

/// CPU Architectural Protocol ABI. Unused callable slots are opaque addresses.
#[repr(C)]
pub struct CpuArchProtocol {
    pub opaque_slots: [usize; 7],
    pub set_memory_attributes: SetMemoryAttributesFn,
    pub number_of_timers: u32,
    pub dma_buffer_alignment: u32,
}

/// Per-call independent evidence for using the borrowed firmware CPU interface.
///
/// # Safety
/// A successful `verify` must establish that the requested operation is safe:
/// the caller is the BSP at a firmware-supported TPL, Boot Services and the
/// qualified original root are live, and the complete target leaf is owned and
/// disjoint from all live/protected allocations and table sources. The checked
/// firmware implementation must preserve non-access flags for this whole-leaf
/// operation and synchronize the affected CPUs/TLBs. It must reject unsupported
/// platform state and fail without authorizing a call. These conditions must
/// remain stable until the immediately following synchronous call returns,
/// including on an error. `verify` must recheck CPU/context after any transfer
/// between threads; successful construction on the BSP is not sufficient.
/// Failure handling must not assume firmware rolled back any changes.
pub unsafe trait QualifiedCpuContext: Send {
    fn verify(&mut self, base: u64, length: u64, absolute_access_mask: u64) -> Result<(), Error>;
    /// Recheck original controls, BSP/TPL and system qualification after the
    /// callback. Invoked even when firmware returned a failure or warning.
    fn verify_after(&mut self) -> Result<(), Error>;
}

/// Borrowed, qualified CPU interface with complete last-status diagnostics.
pub struct CpuArchSetter<C> {
    protocol: NonNull<CpuArchProtocol>,
    context: C,
    last_status: Option<Status>,
    last_after_error: Option<Error>,
    poisoned: bool,
}

// SAFETY: The constructor requires a protocol that remains valid across moves;
// every invocation checks a Send context which must requalify the current CPU.
unsafe impl<C: QualifiedCpuContext> Send for CpuArchSetter<C> {}

impl<C: QualifiedCpuContext> CpuArchSetter<C> {
    /// # Safety
    /// `protocol` must point to a valid, aligned, authentic CPU Architectural
    /// Protocol of the layout above. Its storage and callback code must remain
    /// readable/executable for this object's lifetime, including after moves.
    /// The callback and its globals must not be replaced concurrently. The
    /// context must qualify that exact interface and original address space.
    /// Retain the provider's lifetime externally; this type does not own it.
    pub unsafe fn new(protocol: NonNull<CpuArchProtocol>, context: C) -> Self {
        Self {
            protocol,
            context,
            last_status: None,
            last_after_error: None,
            poisoned: false,
        }
    }

    /// Last actual firmware callback status; rejected/preflight requests leave
    /// this unchanged. Unknown errors and warnings retain their exact raw value.
    pub fn last_status(&self) -> Option<Status> {
        self.last_status
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Independent post-call qualification failure, retained even when the
    /// firmware callback itself also failed. Preflight leaves this unchanged.
    pub fn last_after_error(&self) -> Option<Error> {
        self.last_after_error
    }
}

impl<C: QualifiedCpuContext> Setter for CpuArchSetter<C> {
    fn assign(&mut self, base: u64, length: u64, absolute_access_mask: u64) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::AccessDenied);
        }
        if absolute_access_mask & !ACCESS_MASK != 0 || length == 0 {
            return Err(Error::InvalidParameter);
        }
        if (base | length) & (PAGE_SIZE - 1) != 0 {
            return Err(Error::Unsupported);
        }
        base.checked_add(length).ok_or(Error::InvalidParameter)?;
        self.context.verify(base, length, absolute_access_mask)?;
        self.poisoned = true;
        // SAFETY: Constructor lifetime/provenance contract plus the immediately
        // preceding context qualification cover this synchronous firmware call.
        let status = unsafe {
            (self.protocol.as_ref().set_memory_attributes)(
                self.protocol.as_ptr(),
                base,
                length,
                absolute_access_mask,
            )
        };
        self.last_status = Some(status);
        let after = self.context.verify_after();
        self.last_after_error = after.err();
        match status {
            Status::SUCCESS => {
                after?;
                self.poisoned = false;
                Ok(())
            }
            Status::INVALID_PARAMETER => Err(Error::InvalidParameter),
            Status::UNSUPPORTED => Err(Error::Unsupported),
            Status::OUT_OF_RESOURCES => Err(Error::OutOfResources),
            Status::ACCESS_DENIED => Err(Error::AccessDenied),
            Status::NO_MAPPING => Err(Error::NoMapping),
            // Warnings do not prove the exact requested attributes were applied.
            _ => Err(Error::DeviceError),
        }
    }
}
