//! Deterministic PCI 3.0 option-ROM packaging for one uncompressed UEFI image.

use std::error::Error;
use std::fmt::{self, Display, Formatter, Write};

const EFI_ROM_HEADER_SIZE: usize = 26;
const PCI_DATA_STRUCTURE_SIZE: usize = 28;
const PCI_DATA_STRUCTURE_OFFSET: usize = 28;
const HEADER_SIZE: usize = EFI_ROM_HEADER_SIZE + 2 + PCI_DATA_STRUCTURE_SIZE;
const IMAGE_UNIT: usize = 512;
const MAX_ROM_SIZE: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RomConfig {
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RomError {
    ClassCodeTooLarge(u32),
    ImageTooLarge(usize),
    InvalidMemorySize(usize),
    InvalidPe(&'static str),
    RomDoesNotFit { rom_size: usize, memory_size: usize },
}

impl Display for RomError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClassCodeTooLarge(value) => {
                write!(formatter, "PCI class code exceeds 24 bits: 0x{value:x}")
            }
            Self::ImageTooLarge(size) => {
                write!(
                    formatter,
                    "option ROM exceeds the 16 MiB limit: {size} bytes"
                )
            }
            Self::InvalidMemorySize(size) => {
                write!(
                    formatter,
                    "ROM memory size must be a non-zero multiple of four bytes: {size}"
                )
            }
            Self::InvalidPe(reason) => write!(formatter, "invalid EFI PE image: {reason}"),
            Self::RomDoesNotFit {
                rom_size,
                memory_size,
            } => write!(
                formatter,
                "option ROM is {rom_size} bytes and does not fit in the {memory_size}-byte Expansion ROM BAR"
            ),
        }
    }
}

impl Error for RomError {}

/// Wraps one PE32/PE32+ EFI image in a PCI Firmware 3.0 option-ROM image.
///
/// The layout intentionally matches EDK2 `EfiRom` for its default, uncompressed,
/// single-device, single-image mode.
pub fn build_uefi_option_rom(efi_image: &[u8], config: RomConfig) -> Result<Vec<u8>, RomError> {
    if config.class_code > 0x00ff_ffff {
        return Err(RomError::ClassCodeTooLarge(config.class_code));
    }

    let pe = PeMetadata::parse(efi_image)?;
    let unaligned_size = HEADER_SIZE
        .checked_add(efi_image.len())
        .ok_or(RomError::ImageTooLarge(usize::MAX))?;
    let total_size = unaligned_size
        .checked_add(IMAGE_UNIT - 1)
        .map(|size| size & !(IMAGE_UNIT - 1))
        .ok_or(RomError::ImageTooLarge(usize::MAX))?;

    if total_size > MAX_ROM_SIZE || total_size / IMAGE_UNIT > u16::MAX as usize {
        return Err(RomError::ImageTooLarge(total_size));
    }

    let image_units = (total_size / IMAGE_UNIT) as u16;
    let image_offset = total_size - efi_image.len();
    let image_offset_u16 =
        u16::try_from(image_offset).map_err(|_| RomError::ImageTooLarge(total_size))?;

    let mut rom = vec![0xff; total_size];

    // EFI PCI expansion ROM header.
    write_u16(&mut rom, 0, 0xaa55);
    write_u16(&mut rom, 2, image_units);
    write_u32(&mut rom, 4, 0x0000_0ef1);
    write_u16(&mut rom, 8, pe.subsystem);
    write_u16(&mut rom, 10, pe.machine);
    write_u16(&mut rom, 12, 0); // Uncompressed.
    rom[14..22].fill(0);
    write_u16(&mut rom, 22, image_offset_u16);
    write_u16(&mut rom, 24, PCI_DATA_STRUCTURE_OFFSET as u16);

    // Two zero bytes align the PCI data structure to a four-byte boundary.
    rom[EFI_ROM_HEADER_SIZE..PCI_DATA_STRUCTURE_OFFSET].fill(0);

    // PCI 3.0 data structure.
    let pcir = PCI_DATA_STRUCTURE_OFFSET;
    rom[pcir..pcir + 4].copy_from_slice(b"PCIR");
    write_u16(&mut rom, pcir + 4, config.vendor_id);
    write_u16(&mut rom, pcir + 6, config.device_id);
    write_u16(&mut rom, pcir + 8, 0); // No device-ID list.
    write_u16(&mut rom, pcir + 10, PCI_DATA_STRUCTURE_SIZE as u16);
    rom[pcir + 12] = 3; // PCI Firmware 3.0 structure revision.
    rom[pcir + 13] = config.class_code as u8;
    rom[pcir + 14] = (config.class_code >> 8) as u8;
    rom[pcir + 15] = (config.class_code >> 16) as u8;
    write_u16(&mut rom, pcir + 16, image_units);
    write_u16(&mut rom, pcir + 18, 0); // Code revision.
    rom[pcir + 20] = 3; // EFI image.
    rom[pcir + 21] = 0x80; // Last image.
    rom[pcir + 22..pcir + PCI_DATA_STRUCTURE_SIZE].fill(0);

    rom[image_offset..].copy_from_slice(efi_image);
    Ok(rom)
}

/// Converts an option-ROM image to one 32-bit hexadecimal word per line for
/// `$readmemh`. Each word is encoded as a little-endian integer because the
/// PCILeech completion formatter converts BAR words to PCIe wire byte order.
/// Unused space is filled with the erased-ROM value, `0xff`.
pub fn build_readmemh(rom: &[u8], memory_size: usize) -> Result<String, RomError> {
    if memory_size == 0 || !memory_size.is_multiple_of(4) {
        return Err(RomError::InvalidMemorySize(memory_size));
    }
    if rom.len() > memory_size {
        return Err(RomError::RomDoesNotFit {
            rom_size: rom.len(),
            memory_size,
        });
    }

    let mut contents = String::with_capacity(memory_size / 4 * 9);
    for offset in (0..memory_size).step_by(4) {
        for byte_offset in (0..4).rev() {
            let byte = rom.get(offset + byte_offset).copied().unwrap_or(0xff);
            write!(&mut contents, "{byte:02x}").expect("writing to a String cannot fail");
        }
        contents.push('\n');
    }

    Ok(contents)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PeMetadata {
    machine: u16,
    subsystem: u16,
}

impl PeMetadata {
    fn parse(image: &[u8]) -> Result<Self, RomError> {
        if read_u16(image, 0) != Some(0x5a4d) {
            return Err(RomError::InvalidPe("missing DOS MZ signature"));
        }

        let pe_offset = read_u32(image, 0x3c)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(RomError::InvalidPe("missing PE header offset"))?;

        if image.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
            return Err(RomError::InvalidPe("missing PE signature"));
        }

        let machine =
            read_u16(image, pe_offset + 4).ok_or(RomError::InvalidPe("truncated COFF header"))?;
        let optional_header_size = read_u16(image, pe_offset + 20)
            .ok_or(RomError::InvalidPe("truncated COFF header"))?
            as usize;
        if optional_header_size < 70 {
            return Err(RomError::InvalidPe("optional header is too small"));
        }

        let optional_header = pe_offset
            .checked_add(24)
            .ok_or(RomError::InvalidPe("PE header offset overflow"))?;
        let optional_magic = read_u16(image, optional_header)
            .ok_or(RomError::InvalidPe("truncated optional header"))?;
        if !matches!(optional_magic, 0x010b | 0x020b) {
            return Err(RomError::InvalidPe("unsupported optional-header magic"));
        }

        let subsystem = read_u16(image, optional_header + 68)
            .ok_or(RomError::InvalidPe("truncated optional header"))?;

        Ok(Self { machine, subsystem })
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_a_pci_3_uefi_image() {
        let efi = synthetic_pe32_plus(3072, 0x8664, 11);
        let config = RomConfig {
            vendor_id: 0x10ee,
            device_id: 0x0666,
            class_code: 0x020000,
        };

        let rom = build_uefi_option_rom(&efi, config).unwrap();

        assert_eq!(rom.len(), 3584);
        assert_eq!(&rom[0..2], &[0x55, 0xaa]);
        assert_eq!(read_u16(&rom, 2), Some(7));
        assert_eq!(read_u32(&rom, 4), Some(0x0ef1));
        assert_eq!(read_u16(&rom, 8), Some(11));
        assert_eq!(read_u16(&rom, 10), Some(0x8664));
        assert_eq!(read_u16(&rom, 22), Some(512));
        assert_eq!(read_u16(&rom, 24), Some(28));
        assert_eq!(&rom[28..32], b"PCIR");
        assert_eq!(read_u16(&rom, 32), Some(0x10ee));
        assert_eq!(read_u16(&rom, 34), Some(0x0666));
        assert_eq!(&rom[41..44], &[0x00, 0x00, 0x02]);
        assert_eq!(rom[48], 3);
        assert_eq!(rom[49], 0x80);
        assert!(rom[56..512].iter().all(|byte| *byte == 0xff));
        assert_eq!(&rom[512..], efi.as_slice());
    }

    #[test]
    fn rejects_a_non_pe_image() {
        let error = build_uefi_option_rom(
            &[0; 128],
            RomConfig {
                vendor_id: 0x10ee,
                device_id: 0x0666,
                class_code: 0x020000,
            },
        )
        .unwrap_err();

        assert_eq!(error, RomError::InvalidPe("missing DOS MZ signature"));
    }

    #[test]
    fn creates_a_little_endian_padded_4k_readmemh_image() {
        let memory = build_readmemh(&[0x55, 0xaa, 0x07, 0x00, 0x12], 4096).unwrap();
        let words: Vec<_> = memory.lines().collect();

        assert_eq!(words.len(), 1024);
        assert_eq!(words[0], "0007aa55");
        assert_eq!(words[1], "ffffff12");
        assert!(words[2..].iter().all(|word| *word == "ffffffff"));
    }

    #[test]
    fn rejects_a_rom_larger_than_its_bar() {
        let error = build_readmemh(&[0; 4097], 4096).unwrap_err();

        assert_eq!(
            error,
            RomError::RomDoesNotFit {
                rom_size: 4097,
                memory_size: 4096,
            }
        );
    }

    fn synthetic_pe32_plus(size: usize, machine: u16, subsystem: u16) -> Vec<u8> {
        let mut image = vec![0; size];
        let pe_offset = 0x80;
        write_u16(&mut image, 0, 0x5a4d);
        write_u32(&mut image, 0x3c, pe_offset as u32);
        image[pe_offset..pe_offset + 4].copy_from_slice(b"PE\0\0");
        write_u16(&mut image, pe_offset + 4, machine);
        write_u16(&mut image, pe_offset + 20, 0x00f0);
        write_u16(&mut image, pe_offset + 24, 0x020b);
        write_u16(&mut image, pe_offset + 24 + 68, subsystem);
        image
    }
}
