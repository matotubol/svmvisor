//! The card image envelope: the 128-byte header at the start of the card's 1 MiB payload slot.
//!
//! All integers are little-endian.
//!
//! | Offset | Width | Field |
//! | --- | --- | --- |
//! | `0x000` | 8 | magic: `SVMBPE01` |
//! | `0x008` | 4 | envelope version, 1 |
//! | `0x00c` | 4 | header bytes, 128 |
//! | `0x010` | 8 | payload bytes |
//! | `0x018` | 8 | slot bytes, 0x10_0000 |
//! | `0x020` | 8 | payload offset within the slot, 128 |
//! | `0x028` | 8 | flags: exactly `1 << 2` |
//! | `0x030` | 32 | SHA-256 of the payload bytes |
//!
//! The payload is a PE32+ child that stays resident, subsystem 12 (runtime driver). Flag `1 << 0`
//! belonged to the retired `SVMCRD01` package envelope, flag `1 << 1` and the magic `SVMPE001` to
//! the retired envelope of a child that returned.
//!
//! The header continues with metadata that must equal the payload's own PE headers:
//!
//! | Offset | Width | Field |
//! | --- | --- | --- |
//! | `0x050` | 2 | COFF machine, 0x8664 |
//! | `0x052` | 2 | PE subsystem, 12 |
//! | `0x054` | 2 | optional header magic, 0x020b |
//! | `0x056` | 2 | reserved, zero |
//! | `0x058` | 4 | entry point RVA |
//! | `0x05c` | 4 | image bytes |
//! | `0x060` | 4 | headers bytes |
//! | `0x064` | 4 | section alignment, 4096 |
//! | `0x068` | 4 | file alignment, 512 |
//! | `0x06c` | 4 | section count |
//! | `0x070` | 16 | reserved, zero |
//!
//! The payload follows the header; the rest of the slot is erased flash (`0xff`).
//!
//! The envelope is written by `firmware/card/package-payload.py --resident` (Python, `struct`
//! format `<8sII4Q32s4H6I16s`). The `svmvisor-card-loader` test
//! `optional_python_actual_slot_matches_rust_parser` cross-checks a slot that script produced
//! against this parser.
//!
//! `Envelope::parse` accepts the header, `parse_pe` applies the same narrow policy to the
//! payload's own PE headers; a loader or packager compares the two `PeMetadata` values and
//! the SHA-256.

pub const HEADER_BYTES: usize = 128;
pub const SLOT_BYTES: usize = 0x10_0000;
pub const DIGEST_BYTES: usize = 32;

pub const RESIDENT_BOOT_MAGIC: [u8; 8] = *b"SVMBPE01";

pub const VERSION: u32 = 1;

pub const FLAGS_RESIDENT_BOOT: u64 = 1 << 2;

pub const VERSION_OFFSET: usize = 0x008;
pub const HEADER_BYTES_OFFSET: usize = 0x00c;
pub const PAYLOAD_BYTES_OFFSET: usize = 0x010;
pub const SLOT_BYTES_OFFSET: usize = 0x018;
pub const PAYLOAD_OFFSET_OFFSET: usize = 0x020;
pub const FLAGS_OFFSET: usize = 0x028;
pub const DIGEST_OFFSET: usize = 0x030;
pub const MACHINE_OFFSET: usize = 0x050;
pub const SUBSYSTEM_OFFSET: usize = 0x052;
pub const OPTIONAL_MAGIC_OFFSET: usize = 0x054;
pub const RESERVED_WORD_OFFSET: usize = 0x056;
pub const ENTRY_RVA_OFFSET: usize = 0x058;
pub const IMAGE_BYTES_OFFSET: usize = 0x05c;
pub const HEADERS_BYTES_OFFSET: usize = 0x060;
pub const SECTION_ALIGNMENT_OFFSET: usize = 0x064;
pub const FILE_ALIGNMENT_OFFSET: usize = 0x068;
pub const SECTIONS_OFFSET: usize = 0x06c;
pub const RESERVED_OFFSET: usize = 0x070;

pub const MACHINE_AMD64: u16 = 0x8664;
pub const OPTIONAL_MAGIC_PE32_PLUS: u16 = 0x020b;
pub const SUBSYSTEM_RUNTIME_DRIVER: u16 = 12;

// The narrow PE policy a payload has to meet before firmware ever sees it.
pub const SECTION_ALIGNMENT: u32 = 4096;
pub const FILE_ALIGNMENT: u32 = 512;
pub const MAX_IMAGE_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_SECTIONS: u32 = 16;
pub const MIN_PE_BYTES: usize = 512;

const _: () = {
    assert!(RESERVED_OFFSET + 16 == HEADER_BYTES);
    assert!(DIGEST_OFFSET + DIGEST_BYTES == MACHINE_OFFSET);
    assert!(HEADER_BYTES + MIN_PE_BYTES <= SLOT_BYTES);
};

/// The structurally valid content of one PE envelope header. Nothing here says the payload
/// matches it: the digest and the PE metadata still have to be compared with the payload bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub payload_bytes: usize,
    pub digest: [u8; DIGEST_BYTES],
    pub metadata: PeMetadata,
}

impl Envelope {
    /// Accept exactly the 128-byte `SVMBPE01` header.
    pub fn parse(header: &[u8]) -> Result<Self, EnvelopeError> {
        if header.len() != HEADER_BYTES
            || header.get(..8) != Some(&RESIDENT_BOOT_MAGIC)
            || read_u32(header, VERSION_OFFSET)? != VERSION
            || read_u32(header, HEADER_BYTES_OFFSET)? != HEADER_BYTES as u32
            || read_u64(header, SLOT_BYTES_OFFSET)? != SLOT_BYTES as u64
            || read_u64(header, PAYLOAD_OFFSET_OFFSET)? != HEADER_BYTES as u64
            || read_u64(header, FLAGS_OFFSET)? != FLAGS_RESIDENT_BOOT
            || read_u16(header, MACHINE_OFFSET)? != MACHINE_AMD64
            || read_u16(header, SUBSYSTEM_OFFSET)? != SUBSYSTEM_RUNTIME_DRIVER
            || read_u16(header, OPTIONAL_MAGIC_OFFSET)? != OPTIONAL_MAGIC_PE32_PLUS
            || read_u16(header, RESERVED_WORD_OFFSET)? != 0
            || header.get(RESERVED_OFFSET..).ok_or(EnvelopeError::Header)?.iter().any(|b| *b != 0)
        {
            return Err(EnvelopeError::Header);
        }
        let bytes = read_u64(header, PAYLOAD_BYTES_OFFSET)?;
        if !(MIN_PE_BYTES as u64..=(SLOT_BYTES - HEADER_BYTES) as u64).contains(&bytes) {
            return Err(EnvelopeError::PayloadBounds);
        }
        let mut digest = [0; DIGEST_BYTES];
        for (d, s) in digest.iter_mut().zip(
            header.get(DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_BYTES).ok_or(EnvelopeError::Header)?,
        ) {
            *d = *s;
        }
        let metadata = PeMetadata {
            entry_rva: read_u32(header, ENTRY_RVA_OFFSET)?,
            image_bytes: read_u32(header, IMAGE_BYTES_OFFSET)?,
            headers_bytes: read_u32(header, HEADERS_BYTES_OFFSET)?,
            section_alignment: read_u32(header, SECTION_ALIGNMENT_OFFSET)?,
            file_alignment: read_u32(header, FILE_ALIGNMENT_OFFSET)?,
            sections: read_u32(header, SECTIONS_OFFSET)?,
        };
        metadata.validate(bytes as usize)?;
        Ok(Self { payload_bytes: bytes as usize, digest, metadata })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeMetadata {
    pub entry_rva: u32,
    pub image_bytes: u32,
    pub headers_bytes: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub sections: u32,
}

impl PeMetadata {
    fn validate(&self, bytes: usize) -> Result<(), EnvelopeError> {
        if self.section_alignment != SECTION_ALIGNMENT
            || self.file_alignment != FILE_ALIGNMENT
            || self.image_bytes == 0
            || self.image_bytes > MAX_IMAGE_BYTES
            || self.image_bytes & (SECTION_ALIGNMENT - 1) != 0
            || self.headers_bytes == 0
            || self.headers_bytes as usize > bytes
            || self.headers_bytes & (FILE_ALIGNMENT - 1) != 0
            || self.headers_bytes > self.image_bytes
            || self.entry_rva < self.headers_bytes
            || self.entry_rva >= self.image_bytes
            || self.sections == 0
            || self.sections > MAX_SECTIONS
        {
            return Err(EnvelopeError::PeGeometry);
        }
        Ok(())
    }
}

/// What `Envelope::parse` or `parse_pe` found wrong, precise enough for a packager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// Length, magic, version, header bytes, slot bytes, payload offset, flags, machine,
    /// subsystem, optional header magic or a reserved field of the 128-byte header.
    Header,
    /// Payload bytes outside `MIN_PE_BYTES..=SLOT_BYTES - HEADER_BYTES`.
    PayloadBounds,
    /// A field lies outside the bytes it is read from.
    FieldBounds,
    /// Alignment, image bytes, headers bytes, entry RVA or section count outside the policy.
    PeGeometry,
    /// PE file length or the `MZ` signature.
    PeDosHeader,
    /// `e_lfanew`, the `PE` signature, machine, optional header size or characteristics.
    PeFileHeader,
    /// Optional header magic, subsystem or the number of data directories.
    PeOptionalHeader,
    /// The section table does not fit in the headers.
    PeSectionTable,
    /// An import, TLS, delay-import or CLR directory is present.
    PeDirectory,
    /// The base relocation directory is half-present, too small or outside the image.
    PeRelocation,
    /// A section is empty, misaligned, out of order, out of bounds or writable and executable.
    PeSection,
    /// The entry point's section is not initialized, executable, read-only code.
    PeEntry,
    /// No section backs the entry point, or none backs the relocation directory.
    PeBacking,
}

/// Deliberately narrow AMD64 PE32+ policy. Firmware performs final PE/COFF and
/// security validation; digest binding covers every file byte including overlays.
pub fn parse_pe(pe: &[u8]) -> Result<PeMetadata, EnvelopeError> {
    if pe.len() < MIN_PE_BYTES || pe.len() > SLOT_BYTES - HEADER_BYTES || read_u16(pe, 0)? != 0x5a4d
    {
        return Err(EnvelopeError::PeDosHeader);
    }
    let base = read_u32(pe, 0x3c)? as usize;
    if base < 64
        || base > pe.len().saturating_sub(24)
        || pe.get(base..base + 4) != Some(b"PE\0\0")
        || read_u16(pe, base + 4)? != MACHINE_AMD64
        || read_u16(pe, base + 20)? != 240
        || read_u16(pe, base + 22)? & 3 != 2
    {
        return Err(EnvelopeError::PeFileHeader);
    }
    let opt = base + 24;
    if read_u16(pe, opt)? != OPTIONAL_MAGIC_PE32_PLUS
        || read_u16(pe, opt + 68)? != SUBSYSTEM_RUNTIME_DRIVER
        || read_u32(pe, opt + 108)? != 16
    {
        return Err(EnvelopeError::PeOptionalHeader);
    }
    let meta = PeMetadata {
        entry_rva: read_u32(pe, opt + 16)?,
        image_bytes: read_u32(pe, opt + 56)?,
        headers_bytes: read_u32(pe, opt + 60)?,
        section_alignment: read_u32(pe, opt + 32)?,
        file_alignment: read_u32(pe, opt + 36)?,
        sections: u32::from(read_u16(pe, base + 6)?),
    };
    meta.validate(pe.len())?;
    let table = opt + 240;
    if table + meta.sections as usize * 40 > meta.headers_bytes as usize {
        return Err(EnvelopeError::PeSectionTable);
    }
    // No imports, TLS callbacks, delay imports or CLR initialization in the
    // child. A position-independent image may have no base fixups;
    // if a directory is present it must be wholly backed by initialized data.
    for directory in [1, 9, 13, 14] {
        if read_u64(pe, opt + 112 + directory * 8)? != 0 {
            return Err(EnvelopeError::PeDirectory);
        }
    }
    let reloc = read_u32(pe, opt + 112 + 5 * 8)?;
    let reloc_size = read_u32(pe, opt + 116 + 5 * 8)?;
    if (reloc == 0) != (reloc_size == 0)
        || (reloc != 0 && reloc_size < 8)
        || reloc.checked_add(reloc_size).is_none_or(|e| e > meta.image_bytes)
    {
        return Err(EnvelopeError::PeRelocation);
    }
    let mut previous_virtual = meta.headers_bytes;
    let mut previous_raw = meta.headers_bytes;
    let mut entry = false;
    let mut relocation = reloc == 0 && reloc_size == 0;
    for i in 0..meta.sections as usize {
        let s = table + i * 40;
        let virtual_size = read_u32(pe, s + 8)?;
        let va = read_u32(pe, s + 12)?;
        let raw_size = read_u32(pe, s + 16)?;
        let raw = read_u32(pe, s + 20)?;
        let flags = read_u32(pe, s + 36)?;
        let extent = virtual_size.max(raw_size);
        let end = va.checked_add(extent).ok_or(EnvelopeError::PeSection)?;
        if extent == 0
            || va & (SECTION_ALIGNMENT - 1) != 0
            || va < previous_virtual
            || end > meta.image_bytes
            || raw_size & (FILE_ALIGNMENT - 1) != 0
            || (raw_size != 0
                && (raw & (FILE_ALIGNMENT - 1) != 0
                    || raw < previous_raw
                    || raw.checked_add(raw_size).is_none_or(|e| e as usize > pe.len())))
            || flags & 0xa0000000 == 0xa0000000
        {
            return Err(EnvelopeError::PeSection);
        }
        previous_virtual = end;
        if raw_size != 0 {
            previous_raw = raw + raw_size;
        }
        if meta.entry_rva >= va && meta.entry_rva < end {
            if flags & 0xe0000020 != 0x60000020
                || meta.entry_rva - va >= raw_size
                || meta.entry_rva - va >= virtual_size
            {
                return Err(EnvelopeError::PeEntry);
            }
            entry = true;
        }
        if reloc >= va && reloc.checked_add(reloc_size).is_some_and(|e| e <= va + raw_size) {
            relocation = true;
        }
    }
    if !entry || !relocation {
        return Err(EnvelopeError::PeBacking);
    }
    Ok(meta)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, EnvelopeError> {
    let Some(&[a, b]) = bytes.get(offset..offset.checked_add(2).ok_or(EnvelopeError::FieldBounds)?)
    else {
        return Err(EnvelopeError::FieldBounds);
    };
    Ok(u16::from_le_bytes([a, b]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, EnvelopeError> {
    let Some(&[a, b, c, d]) =
        bytes.get(offset..offset.checked_add(4).ok_or(EnvelopeError::FieldBounds)?)
    else {
        return Err(EnvelopeError::FieldBounds);
    };
    Ok(u32::from_le_bytes([a, b, c, d]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, EnvelopeError> {
    let Some(&[a, b, c, d, e, f, g, h]) =
        bytes.get(offset..offset.checked_add(8).ok_or(EnvelopeError::FieldBounds)?)
    else {
        return Err(EnvelopeError::FieldBounds);
    };
    Ok(u64::from_le_bytes([a, b, c, d, e, f, g, h]))
}
