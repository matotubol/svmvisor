//! Bounded firmware IOMMU inventory. AMD48882 rev3.11 chapter5 Tables85-113.
//! Original IVRS bytes remain authoritative and must be retained by the caller;
//! this decoder never rewrites firmware tables or hides DMA protection.
use svmvisor_hypervisor::{
    memory::address::AddressPolicy,
    svm::iommu::{Error as HardwareError, MAX_IOMMUS, Unit},
};

pub const MAX_IVRS_BYTES: usize = 16384;

/// Read through the firmware admission owner's validated physical-memory
/// mapping. A numeric address check alone is not a safe implementation: RAM
/// classification, current paging, mapping lifetime and readable extent must
/// all be established before the backend dereferences a physical address.
pub trait FirmwareReader {
    fn read(&mut self, address: u64, destination: &mut [u8]) -> Result<(), Error>;
}

/// ACPI6.6 §5.2.5.2: prefer the ACPI2 configuration-table GUID. The supplied
/// slice is already validated/readable firmware storage; no vendor pointer is
/// dereferenced here. There is no legacy EBDA/ROM scan in the native profile.
pub fn rsdp_address(
    tables: &[uefi_raw::table::configuration::ConfigurationTable],
) -> Result<u64, Error> {
    use uefi_raw::guid;
    let mut found = None;
    for table in tables {
        if table.vendor_guid == guid!("8868e871-e4f1-11d3-bc22-0080c73c8881") {
            if found.is_some() || table.vendor_table.is_null() {
                return Err(Error::Duplicate);
            }
            found = Some(table.vendor_table as u64);
        }
    }
    found.ok_or(Error::MissingUnit)
}

/// Copy the complete IVRS into retained caller storage. ACPI6.6 Tables5.3/5.4/
/// 5.8: checks both RSDP checksums and the complete XSDT checksum, prefers XSDT,
/// decodes its naturally unaligned 64-bit entries without typed dereferences,
/// and rejects duplicate IVRS pointers. Bounded to16KiB XSDT/IVRS and4KiB RSDP.
/// The native x2AVIC profile requires ACPI2+ with XSDT; it does not silently
/// switch to RSDT after a malformed XSDT. Reader errors propagate unchanged.
pub fn load_ivrs(
    rsdp: u64,
    reader: &mut impl FirmwareReader,
    output: &mut [u8],
) -> Result<usize, Error> {
    let mut root = [0u8; 36];
    reader.read(rsdp, &mut root[..20])?;
    if root[..8] != *b"RSD PTR " || root[15] < 2 {
        return Err(Error::Header);
    }
    if checksum(&root[..20]) != 0 {
        return Err(Error::Checksum);
    }
    reader.read(rsdp.checked_add(20).ok_or(Error::Address)?, &mut root[20..])?;
    let root_bytes = number(&root, 20, 4)? as usize;
    if !(36..=4096).contains(&root_bytes) {
        return Err(Error::Bounds);
    }
    checksum_physical(reader, rsdp, root_bytes)?;
    let xsdt = number(&root, 24, 8)?;
    if xsdt == 0 {
        return Err(Error::Address);
    }
    let mut header = [0u8; 36];
    reader.read(xsdt, &mut header)?;
    let length = number(&header, 4, 4)? as usize;
    if header[..4] != *b"XSDT"
        || header[8] != 1
        || !(36..=MAX_IVRS_BYTES).contains(&length)
        || (length - 36) % 8 != 0
    {
        return Err(Error::Header);
    }
    checksum_physical(reader, xsdt, length)?;
    let mut found = None;
    for offset in (36..length).step_by(8) {
        let mut entry = [0u8; 8];
        reader.read(
            xsdt.checked_add(offset as u64).ok_or(Error::Address)?,
            &mut entry,
        )?;
        let address = u64::from_le_bytes(entry);
        if address == 0 {
            return Err(Error::Address);
        }
        reader.read(address, &mut header)?;
        if header[..4] == *b"IVRS" {
            if found.is_some() {
                return Err(Error::Duplicate);
            }
            let count = number(&header, 4, 4)? as usize;
            if !(48..=MAX_IVRS_BYTES).contains(&count) || count > output.len() {
                return Err(Error::Bounds);
            }
            found = Some((address, count));
        }
    }
    let (address, count) = found.ok_or(Error::MissingUnit)?;
    reader.read(address, &mut output[..count])?;
    // Detect changed headers as well as checksum corruption during discovery.
    if output[..4] != *b"IVRS" || number(output, 4, 4)? != count as u64 {
        return Err(Error::Header);
    }
    if checksum(&output[..count]) != 0 {
        return Err(Error::Checksum);
    }
    Ok(count)
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

fn checksum_physical(
    reader: &mut impl FirmwareReader,
    address: u64,
    bytes: usize,
) -> Result<(), Error> {
    let mut block = [0u8; 64];
    let mut sum = 0u8;
    for offset in (0..bytes).step_by(block.len()) {
        let count = (bytes - offset).min(block.len());
        reader.read(
            address.checked_add(offset as u64).ok_or(Error::Address)?,
            &mut block[..count],
        )?;
        sum = sum.wrapping_add(checksum(&block[..count]));
    }
    if sum != 0 {
        return Err(Error::Checksum);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Header,
    Checksum,
    Bounds,
    Unsupported,
    Reserved,
    Range,
    Duplicate,
    TooManyUnits,
    MissingUnit,
    Address,
    Features(HardwareError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub unit: Unit,
    pub offset: u16,
    pub bytes: u16,
    pub kind: u8,
    pub flags: u8,
}

pub struct Inventory<'a> {
    bytes: &'a [u8],
    units: [Option<Description>; MAX_IOMMUS],
    formats: [u8; MAX_IOMMUS],
    count: usize,
    ivinfo: u32,
}

impl<'a> Inventory<'a> {
    pub fn units(&self) -> impl Iterator<Item = Description> + '_ {
        self.units[..self.count].iter().filter_map(|unit| *unit)
    }
    pub const fn ivinfo(&self) -> u32 {
        self.ivinfo
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub fn admit_x2avic(&self) -> Result<(), Error> {
        for description in self.units() {
            if description.kind == 0x10 {
                return Err(Error::Features(HardwareError::MissingExtendedFeatures));
            }
            description.unit.admit_x2avic().map_err(Error::Features)?;
        }
        Ok(())
    }
    /// Original selected source declarations, including aliases and specials.
    /// No overlap precedence is invented. A source owner must resolve/validate
    /// its full requester inventory before activation, including Type40 HIDs.
    pub fn device_bytes(&self, description: Description) -> Result<&'a [u8], Error> {
        if !self.units().any(|d| d == description) {
            return Err(Error::MissingUnit);
        }
        let start = description.offset as usize;
        let header = if description.kind == 0x10 { 24 } else { 40 };
        self.bytes
            .get(start + header..start + description.bytes as usize)
            .ok_or(Error::Bounds)
    }
}

fn number(bytes: &[u8], offset: usize, length: usize) -> Result<u64, Error> {
    let field = bytes
        .get(offset..offset.checked_add(length).ok_or(Error::Bounds)?)
        .ok_or(Error::Bounds)?;
    let mut value = 0;
    for (i, byte) in field.iter().enumerate() {
        value |= u64::from(*byte) << (i * 8);
    }
    Ok(value)
}

pub fn discover_ivrs<'a>(bytes: &'a [u8], policy: &AddressPolicy) -> Result<Inventory<'a>, Error> {
    if !(48..=MAX_IVRS_BYTES).contains(&bytes.len())
        || bytes.get(..4) != Some(b"IVRS")
        || number(bytes, 4, 4)? != bytes.len() as u64
        || !matches!(bytes[8], 1 | 2)
    {
        return Err(Error::Header);
    }
    if bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
        return Err(Error::Checksum);
    }
    let ivinfo = number(bytes, 36, 4)? as u32;
    if ivinfo & 0xff80_001c != 0 || bytes[40..48].iter().any(|b| *b != 0) {
        return Err(Error::Reserved);
    }
    let mut inventory = Inventory {
        bytes,
        units: [None; MAX_IOMMUS],
        formats: [0; MAX_IOMMUS],
        count: 0,
        ivinfo,
    };
    let mut offset = 48;
    let mut previous_ivhd = false;
    while offset < bytes.len() {
        let length = number(bytes, offset + 2, 2)? as usize;
        if length < 4 {
            return Err(Error::Bounds);
        }
        let block = bytes
            .get(offset..offset.checked_add(length).ok_or(Error::Bounds)?)
            .ok_or(Error::Bounds)?;
        match block[0] {
            kind @ (0x10 | 0x11 | 0x40) => {
                let header = if kind == 0x10 { 24 } else { 40 };
                if length <= header
                    || (kind == 0x40 && bytes[8] != 2)
                    || (kind != 0x10 && ivinfo & 1 == 0)
                {
                    return Err(Error::Header);
                }
                if number(block, 18, 2)? & 0xe0e0 != 0 {
                    return Err(Error::Reserved);
                }
                if kind != 0x10
                    && (block[1] & 0xc0 != 0 || number(block, 20, 4)? & 0xf000_1ffe != 0)
                {
                    return Err(Error::Reserved);
                }
                validate_devices(&block[header..], kind == 0x40)?;
                let efr = if kind != 0x10 {
                    number(block, 24, 8)?
                } else {
                    0
                };
                let efr2 = if kind != 0x10 {
                    number(block, 32, 8)?
                } else {
                    0
                };
                let base = number(block, 8, 8)?;
                let aperture = if efr & (1 << 9) != 0 {
                    512 * 1024
                } else {
                    16 * 1024
                };
                if base == 0 {
                    return Err(Error::Address);
                }
                let capability = number(block, 6, 2)? as u16;
                if !(0x40..=0xe8).contains(&capability) || capability & 3 != 0 {
                    return Err(Error::Address);
                }
                let description = Description {
                    unit: Unit {
                        segment: number(block, 16, 2)? as u16,
                        device_id: number(block, 4, 2)? as u16,
                        capability,
                        mmio: policy
                            .validate(base, aperture, aperture)
                            .map_err(|_| Error::Address)?,
                        firmware_efr: efr,
                        firmware_efr2: efr2,
                    },
                    offset: offset as u16,
                    bytes: length as u16,
                    kind,
                    flags: block[1],
                };
                insert(&mut inventory, description)?;
                previous_ivhd = true;
            }
            0x20..=0x22 => {
                if !previous_ivhd
                    || length != 32
                    || block[1] & 0xf0 != 0
                    || block[10..16].iter().any(|b| *b != 0)
                {
                    return Err(Error::Header);
                }
                let start = number(block, 16, 8)?;
                let size = number(block, 24, 8)?;
                policy
                    .validate(start, size, 1)
                    .map_err(|_| Error::Address)?;
                if (block[0] == 0x20 && number(block, 4, 6)? != 0)
                    || (block[0] == 0x21 && number(block, 6, 2)? != 0)
                    || (block[0] == 0x22 && number(block, 4, 2)? > number(block, 6, 2)?)
                {
                    return Err(Error::Range);
                }
            }
            _ => return Err(Error::Unsupported),
        }
        offset += length;
    }
    if inventory.count == 0 {
        return Err(Error::MissingUnit);
    }
    Ok(inventory)
}

fn insert(inventory: &mut Inventory<'_>, description: Description) -> Result<(), Error> {
    let format = match description.kind {
        0x10 => 1,
        0x11 => 2,
        _ => 4,
    };
    for (index, entry) in inventory.units[..inventory.count].iter_mut().enumerate() {
        let prior = entry.as_ref().unwrap();
        if (
            prior.unit.segment,
            prior.unit.device_id,
            prior.unit.capability,
        ) != (
            description.unit.segment,
            description.unit.device_id,
            description.unit.capability,
        ) {
            if prior.unit.mmio.base() <= description.unit.mmio.last_byte()
                && description.unit.mmio.base() <= prior.unit.mmio.last_byte()
            {
                return Err(Error::Duplicate);
            }
            continue;
        }
        if inventory.formats[index] & format != 0
            || prior.unit.mmio.base() != description.unit.mmio.base()
            || (prior.kind != 0x10
                && description.kind != 0x10
                && (prior.unit.firmware_efr != description.unit.firmware_efr
                    || prior.unit.firmware_efr2 != description.unit.firmware_efr2))
        {
            return Err(Error::Duplicate);
        }
        inventory.formats[index] |= format;
        // Type11 authoritative capability preference. Original Type40/HID bytes
        // remain in the retained complete IVRS for source ownership.
        if description.kind == 0x11 || prior.kind == 0x10 {
            *entry = Some(description);
        }
        return Ok(());
    }
    if inventory.count == MAX_IOMMUS {
        return Err(Error::TooManyUnits);
    }
    inventory.units[inventory.count] = Some(description);
    inventory.formats[inventory.count] = format;
    inventory.count += 1;
    Ok(())
}

fn validate_devices(bytes: &[u8], mixed: bool) -> Result<(), Error> {
    let mut offset = 0;
    let mut pending: Option<u16> = None;
    let mut variable = false;
    while offset < bytes.len() {
        let kind = bytes[offset];
        if pending.is_some() && kind != 4 {
            return Err(Error::Range);
        }
        let length = match kind {
            0xf0 if mixed => {
                variable = true;
                let format = *bytes.get(offset + 20).ok_or(Error::Bounds)?;
                let uid = *bytes.get(offset + 21).ok_or(Error::Bounds)? as usize;
                if format > 2 || (format == 0 && uid != 0) {
                    return Err(Error::Header);
                }
                22 + uid
            }
            0..=0x7f if !variable => {
                if kind < 0x40 {
                    4
                } else {
                    8
                }
            }
            _ => return Err(Error::Unsupported),
        };
        let raw = bytes.get(offset..offset + length).ok_or(Error::Bounds)?;
        let device = number(raw, 1, 2)? as u16;
        match kind {
            0 if raw.iter().any(|b| *b != 0) => return Err(Error::Reserved),
            0 | 1 | 2 | 0x42 | 0x46 | 0x48 | 0xf0 => {}
            3 | 0x43 | 0x47 => pending = Some(device),
            4 => {
                if pending.take().is_none_or(|start| start > device) || raw[3] != 0 {
                    return Err(Error::Range);
                }
            }
            _ => return Err(Error::Unsupported),
        }
        if matches!(kind, 0x42 | 0x43) && (raw[4] != 0 || raw[7] != 0) {
            return Err(Error::Reserved);
        }
        if matches!(kind, 0x46 | 0x47) && number(raw, 4, 4)? & 0x7fff_fff8 != 0 {
            return Err(Error::Reserved);
        }
        if kind == 0x48 && (device != 0 || !matches!(raw[7], 1 | 2)) {
            return Err(Error::Header);
        }
        offset += length;
    }
    if pending.is_some() {
        return Err(Error::Range);
    }
    Ok(())
}
