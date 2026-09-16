//! AMD IOMMU observation and DMA-preserving command mediation.
//!
//! AMD 48882 rev3.11 §§2.2.2, 2.4, 3.2, 3.4. This module never enables an
//! IOMMU or changes DMA policy implicitly. In particular, command completion
//! does not constitute a proof that direct GA backing-page writers are drained.
use crate::memory::address::{AddressPolicy, PhysicalRange};

pub const MAX_IOMMUS: usize = 8;
pub const MAX_COMMANDS_PER_SERVICE: usize = 64;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    pub segment: u16,
    pub device_id: u16,
    pub capability: u16,
    pub mmio: PhysicalRange,
    /// Authoritative IVHD11/40 image; hardware does not override firmware.
    pub firmware_efr: u64,
    pub firmware_efr2: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Address,
    MissingExtendedFeatures,
    MissingGuestApic,
    MissingX2Apic,
    UnsupportedGuestApicMode,
    PciCapability,
    PciBase,
    RegisterRead,
    UnsupportedDmaMode,
    InvalidCommand,
    UnsupportedCommand,
    InvalidQueue,
    QueueBusy,
    GuestMemory,
    DeviceOutsideTable,
    Hardware,
    CompletionTimeout,
    Poisoned,
}

impl Unit {
    pub fn admit_x2avic(self) -> Result<(), Error> {
        if self.firmware_efr & (1 << 7) == 0 {
            return Err(Error::MissingGuestApic);
        }
        if self.firmware_efr & (1 << 2) == 0 {
            return Err(Error::MissingX2Apic);
        }
        if (self.firmware_efr >> 21) & 7 != 1 {
            return Err(Error::UnsupportedGuestApicMode);
        }
        Ok(())
    }
}

/// Read-only register access. Implementations must not manufacture values for
/// unavailable configuration paths, disabled apertures, or failed MMIO reads.
pub trait RegisterReader {
    fn pci_u32(&mut self, unit: Unit, offset: u16) -> Result<u32, Error>;
    fn mmio_u64(&mut self, unit: Unit, offset: u16) -> Result<u64, Error>;
}

/// Ordered raw observations, not a coherent snapshot or ownership token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegisterSnapshot {
    pub capability_header: u32,
    pub pci_base: u64,
    pub control_before: u64,
    pub device_table: u64,
    pub command_base: u64,
    pub event_base: u64,
    pub exclusion_base: u64,
    pub exclusion_limit: u64,
    pub efr: u64,
    pub efr2: u64,
    /// Segment0 duplicates device_table; remaining active roots are captured.
    pub device_segments: [u64; 8],
    pub command_head: u64,
    pub command_tail: u64,
    pub event_head: u64,
    pub event_tail: u64,
    pub status: u64,
    pub ga_base: u64,
    pub ga_tail_address: u64,
    pub ga_head: u64,
    pub ga_tail: u64,
    pub control_after: u64,
}

pub fn capture(unit: Unit, reader: &mut impl RegisterReader) -> Result<RegisterSnapshot, Error> {
    if !(0x40..=0xe8).contains(&unit.capability)
        || unit.capability & 3 != 0
        || unit.mmio.base() == 0
        || unit.mmio.base() & 0x3fff != 0
        || unit.mmio.len() < 0x4000
    {
        return Err(Error::Address);
    }
    let header = reader.pci_u32(unit, unit.capability)?;
    // CapID=0fh, CapType=011b, CapRev=00001b; EFRSup must be implemented.
    if header & 0x00ff_00ff != 0x000b_000f || header & (1 << 27) == 0 {
        return Err(Error::PciCapability);
    }
    let low = reader.pci_u32(unit, unit.capability + 4)?;
    let high = reader.pci_u32(unit, unit.capability + 8)?;
    let base = (high as u64) << 32 | u64::from(low & !0x3fff);
    if low & 1 == 0 || base != unit.mmio.base() {
        return Err(Error::PciBase);
    }
    let mut r = |offset| reader.mmio_u64(unit, offset);
    let control_before = r(0x18)?;
    let mut snapshot = RegisterSnapshot {
        capability_header: header,
        pci_base: base,
        control_before,
        device_table: r(0)?,
        command_base: r(8)?,
        event_base: r(0x10)?,
        exclusion_base: r(0x20)?,
        exclusion_limit: r(0x28)?,
        efr: r(0x30)?,
        efr2: r(0x1a0)?,
        command_head: r(0x2000)?,
        command_tail: r(0x2008)?,
        event_head: r(0x2010)?,
        event_tail: r(0x2018)?,
        status: r(0x2020)?,
        ..RegisterSnapshot::default()
    };
    snapshot.device_segments[0] = snapshot.device_table;
    let segments = (control_before >> 34) & 7;
    if segments <= 3 {
        for segment in 1..(1usize << segments) {
            snapshot.device_segments[segment] = r(0x100 + (segment as u16 - 1) * 8)?;
        }
    }
    if snapshot.efr & (1 << 7) != 0 {
        snapshot.ga_base = r(0xe0)?;
        snapshot.ga_tail_address = r(0xe8)?;
        snapshot.ga_head = r(0x2040)?;
        snapshot.ga_tail = r(0x2048)?;
    }
    snapshot.control_after = r(0x18)?;
    Ok(snapshot)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct DeviceTableEntry(pub [u64; 4]);

/// Validate the implemented ordinary host-translation profile and merge the
/// guest's DMA policy with the monitor's interrupt policy. This is not hardware
/// publication. Words0/1 are copied exactly, including V, TV, permissions,
/// DomainID, EX, fault controls, root and mode. Never clear V or force Mode000.
/// Guest translation, ATS/PASID/PPR, dirty updates, CXL and vIOMMU are refused.
/// AMD Table7 pp63-74: the independent interrupt portion occupies word2.
pub fn merge_dma(
    guest: DeviceTableEntry,
    owned: DeviceTableEntry,
    policy: &AddressPolicy,
    max_levels: u8,
) -> Result<DeviceTableEntry, Error> {
    let w = guest.0;
    // Unsupported fields are refused even if dormant, avoiding latent activation.
    if w[0] & (0x1ffu64 << 52 | 0x1fc) != 0
        || w[1] & (0xffffu64 << 16 | 1 << 32 | 1 << 42 | (!0u64 << 43)) != 0
        || w[2] & (3 << 54 | 1 << 59) != 0
        || w[3] != 0
        || w[0] & (1 << 63) != 0
        || (w[1] >> 35) & 3 == 3
    {
        return Err(Error::UnsupportedDmaMode);
    }
    if w[0] & 3 == 3 {
        let levels = ((w[0] >> 9) & 7) as u8;
        if levels > max_levels || levels > 6 {
            return Err(Error::UnsupportedDmaMode);
        }
        if levels != 0 {
            policy
                .validate(w[0] & ADDRESS_MASK, 4096, 4096)
                .map_err(|_| Error::Address)?;
        }
    }
    Ok(DeviceTableEntry([w[0], w[1], owned.0[2], 0]))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct Command(pub [u64; 2]);

impl Command {
    pub const fn invalidate_device(device: u16) -> Self {
        Self([2 << 60 | device as u64, 0])
    }
    pub const fn invalidate_interrupt(device: u16) -> Self {
        Self([5 << 60 | device as u64, 0])
    }
    pub fn completion(address: PhysicalRange, value: u64) -> Result<Self, Error> {
        if address.base() & 7 != 0 || address.len() < 8 || address.last_byte() >> 52 != 0 {
            return Err(Error::Address);
        }
        Ok(Self([1 << 60 | address.base() | 5, value])) // store + flush
    }
}

/// Runtime implementation owns all guest/host memory and physical command
/// publication. Success of `execute` requires CompletionWait *store*, not only
/// head advance; errors must retain hardware state and stop the guest.
pub trait CommandBackend {
    fn read_command(&mut self, address: u64) -> Result<Command, Error>;
    fn read_guest_dte(&mut self, device: u16) -> Result<DeviceTableEntry, Error>;
    fn read_owned_dte(&mut self, device: u16) -> Result<DeviceTableEntry, Error>;
    /// Atomically replace only words0/1 using aligned CMPXCHG16B, preserving
    /// words2/3. Plain paired 64-bit stores are not a valid implementation.
    fn publish_dma(&mut self, device: u16, entry: DeviceTableEntry) -> Result<(), Error>;
    fn execute(&mut self, command: Command) -> Result<(), Error>;
    /// Translate guest interrupt policy through the source owner before issuing
    /// physical INVALIDATE_INTERRUPT_TABLE; never copy a guest IRTE pointer.
    fn update_interrupts(&mut self, device: u16) -> Result<(), Error>;
    fn store_completion(&mut self, address: PhysicalRange, value: u64) -> Result<(), Error>;
}

/// Owned physical command-ring transport. Implementations access an exclusively
/// retained coherent ring and completion word and the admitted IOMMU aperture.
/// All methods must be bounded; publication must order table/ring writes before
/// MMIO tail. This interface cannot authorize takeover of a firmware-owned ring.
pub trait RingIo {
    fn head(&mut self) -> Result<u32, Error>;
    fn status(&mut self) -> Result<u64, Error>;
    fn write(&mut self, offset: u32, command: Command) -> Result<(), Error>;
    fn clear_completion(&mut self) -> Result<(), Error>;
    fn completion(&mut self) -> Result<u64, Error>;
    /// Must include a release fence and native store ordering before MMIO.
    fn publish_tail(&mut self, tail: u32) -> Result<(), Error>;
}

/// Physical queue cursor, used by the runtime backend's execute operation.
/// It reserves both the requested command and a distinct CompletionWait before
/// publication. It never uses head movement as completion evidence.
pub struct CommandRing {
    bytes: u32,
    tail: u32,
    completion: PhysicalRange,
    sequence: u64,
    poisoned: bool,
}

impl CommandRing {
    pub fn new(bytes: u32, tail: u32, completion: PhysicalRange) -> Result<Self, Error> {
        if !bytes.is_power_of_two()
            || !(4096..=524288).contains(&bytes)
            || tail >= bytes
            || tail & 15 != 0
        {
            return Err(Error::InvalidQueue);
        }
        Command::completion(completion, 1)?;
        Ok(Self {
            bytes,
            tail,
            completion,
            sequence: 0,
            poisoned: false,
        })
    }
    pub const fn tail(&self) -> u32 {
        self.tail
    }
    pub const fn poisoned(&self) -> bool {
        self.poisoned
    }

    /// `poll_budget` is capped so bad hardware cannot spin indefinitely. A
    /// timeout or transport error poisons the queue: the caller retains storage
    /// and stops, and must not replay a possibly executed command.
    pub fn execute(
        &mut self,
        command: Command,
        io: &mut impl RingIo,
        poll_budget: usize,
    ) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let result = self.execute_inner(command, io, poll_budget.min(100_000));
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn execute_inner(
        &mut self,
        command: Command,
        io: &mut impl RingIo,
        poll_budget: usize,
    ) -> Result<(), Error> {
        let head = io.head()?;
        if head >= self.bytes || head & 15 != 0 {
            return Err(Error::InvalidQueue);
        }
        if head.wrapping_sub(self.tail).wrapping_sub(16) & (self.bytes - 1) < 32 {
            return Err(Error::QueueBusy);
        }
        check_status(io.status()?)?;
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Poisoned)?;
        io.clear_completion()?;
        io.write(self.tail, command)?;
        let completion_offset = (self.tail + 16) & (self.bytes - 1);
        io.write(
            completion_offset,
            Command::completion(self.completion, self.sequence)?,
        )?;
        let tail = (self.tail + 32) & (self.bytes - 1);
        // Once this can have published, even an error must never reclaim/replay.
        self.tail = tail;
        io.publish_tail(tail)?;
        for _ in 0..poll_budget {
            check_status(io.status()?)?;
            if io.completion()? == self.sequence {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(Error::CompletionTimeout)
    }
}

fn check_status(status: u64) -> Result<(), Error> {
    // §3.4.16 pp254-256. Pending event/overflow is observed and never cleared
    // here. Event relay/diagnosis must handle it before another queue owner can
    // operate. CmdBufRun=0 includes fatal command errors and explicit halt.
    if status & (1 << 4) == 0
        || status & ((1 << 0) | (1 << 1) | (1 << 5) | (1 << 9) | (1 << 11) | (1 << 15)) != 0
    {
        return Err(Error::Hardware);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestCommandQueue {
    range: PhysicalRange,
    head: u32,
    tail: u32,
    poisoned: bool,
}

impl GuestCommandQueue {
    pub fn new(range: PhysicalRange, head: u32, tail: u32) -> Result<Self, Error> {
        if range.base() & 4095 != 0
            || !range.len().is_power_of_two()
            || !(4096..=524288).contains(&range.len())
            || u64::from(head) >= range.len()
            || u64::from(tail) >= range.len()
            || (head | tail) & 15 != 0
        {
            return Err(Error::InvalidQueue);
        }
        Ok(Self {
            range,
            head,
            tail,
            poisoned: false,
        })
    }
    pub const fn head(&self) -> u32 {
        self.head
    }
    pub const fn tail(&self) -> u32 {
        self.tail
    }
    pub const fn poisoned(&self) -> bool {
        self.poisoned
    }

    /// Called for an intercepted guest tail write under the unit's shared lock.
    /// A guest cannot reuse space before observing the mediated head advance.
    pub fn submit_tail(&mut self, tail: u64) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if tail >= self.range.len() || tail & 15 != 0 {
            return Err(Error::InvalidQueue);
        }
        let mask = self.range.len() as u32 - 1;
        let advance = (tail as u32).wrapping_sub(self.tail) & mask;
        let free = self.head.wrapping_sub(self.tail).wrapping_sub(16) & mask;
        if advance > free {
            return Err(Error::QueueBusy);
        }
        self.tail = tail as u32;
        Ok(())
    }

    /// Service at most64 commands, retaining the next unconsumed head. `false`
    /// means more work remains and the runtime must schedule another service;
    /// it must not expose a completed head/tail or discard the remaining work.
    /// On failure the head stays at the failed command and this queue is poisoned
    /// so a partially published hardware transaction cannot be blindly retried.
    pub fn service(
        &mut self,
        backend: &mut impl CommandBackend,
        policy: &AddressPolicy,
        max_levels: u8,
        budget: usize,
    ) -> Result<bool, Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        for _ in 0..budget.min(MAX_COMMANDS_PER_SERVICE) {
            if self.head == self.tail {
                return Ok(true);
            }
            let result = backend
                .read_command(self.range.base() + u64::from(self.head))
                .and_then(|command| service_command(command, backend, policy, max_levels));
            if let Err(error) = result {
                self.poisoned = true;
                return Err(error);
            }
            self.head = (self.head + 16) & (self.range.len() as u32 - 1);
        }
        Ok(self.head == self.tail)
    }
}

fn service_command(
    command: Command,
    backend: &mut impl CommandBackend,
    policy: &AddressPolicy,
    max_levels: u8,
) -> Result<(), Error> {
    let [a, b] = command.0;
    match a >> 60 {
        1 => {
            if a & 0x0ff0_0000_0000_0000 != 0 {
                return Err(Error::InvalidCommand);
            }
            // Completion interrupts require virtual event/interrupt relay.
            if a & 2 != 0 {
                return Err(Error::UnsupportedCommand);
            }
            let store = if a & 1 != 0 {
                Some(
                    policy
                        .validate(a & 0x000f_ffff_ffff_fff8, 8, 8)
                        .map_err(|_| Error::Address)?,
                )
            } else {
                None
            };
            // Every earlier hardware operation already waited for completion.
            // This explicit barrier also covers a first command after takeover.
            backend.execute(Command([1 << 60 | 4, 0]))?;
            if let Some(address) = store {
                backend.store_completion(address, b)?;
            }
            Ok(())
        }
        2 | 5 => {
            if a & 0x0fff_ffff_ffff_0000 != 0 || b != 0 {
                return Err(Error::InvalidCommand);
            }
            let device = a as u16;
            if a >> 60 == 5 {
                return backend.update_interrupts(device);
            }
            let guest = backend.read_guest_dte(device)?;
            let owned = backend.read_owned_dte(device)?;
            let merged = merge_dma(guest, owned, policy, max_levels)?;
            // A DTE invalidation also changes the guest interrupt root/IV/
            // IntCtl; route mediation must observe it, not only opcode5.
            backend.update_interrupts(device)?;
            backend.publish_dma(device, merged)?;
            backend.execute(command)
        }
        3 => {
            // Table36: no PASID/GN for the supported ordinary DMA profile.
            if a & 0x0fff_0000_ffff_ffff != 0
                || b & 0xffc != 0
                || (b & 1 != 0 && b & !0xfff == !0xfff)
            {
                return Err(Error::UnsupportedCommand);
            }
            backend.execute(command)
        }
        _ => Err(Error::UnsupportedCommand),
    }
}

const _: () = {
    assert!(core::mem::size_of::<DeviceTableEntry>() == 32);
    assert!(core::mem::size_of::<Command>() == 16);
};
