use svmvisor_card_abi::envelope::{Envelope, EnvelopeError, PeMetadata, parse_pe};

const METADATA: PeMetadata = PeMetadata {
    entry_rva: 4096,
    image_bytes: 8192,
    headers_bytes: 512,
    section_alignment: 4096,
    file_alignment: 512,
    sections: 1,
};

fn w16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn w32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn w64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// A 1024-byte runtime driver: one 512-byte code section at RVA 4096.
fn pe() -> Vec<u8> {
    let mut pe = vec![0; 1024];
    pe[..2].copy_from_slice(b"MZ");
    w32(&mut pe, 0x3c, 64);
    pe[64..68].copy_from_slice(b"PE\0\0");
    for (offset, value) in [(68, 0x8664), (70, 1), (84, 240), (86, 2), (88, 0x20b), (156, 12)] {
        w16(&mut pe, offset, value);
    }
    for (offset, value) in [
        (104, 4096),
        (120, 4096),
        (124, 512),
        (144, 8192),
        (148, 512),
        (196, 16),
        (336, 1),
        (340, 4096),
        (344, 512),
        (348, 512),
        (364, 0x60000020),
    ] {
        w32(&mut pe, offset, value);
    }
    pe[512] = 0xc3;
    pe
}

/// The `SVMBPE01` header `package-payload.py --resident` writes for `pe()`, digest left as 0xab.
fn header() -> [u8; 128] {
    let mut header = [0; 128];
    header[..8].copy_from_slice(b"SVMBPE01");
    for (offset, value) in
        [(8, 1), (12, 128), (88, 4096), (92, 8192), (96, 512), (100, 4096), (104, 512), (108, 1)]
    {
        w32(&mut header, offset, value);
    }
    for (offset, value) in [(16, 1024), (24, 0x100000), (32, 128), (40, 4)] {
        w64(&mut header, offset, value);
    }
    for (offset, value) in [(80, 0x8664), (82, 12), (84, 0x20b)] {
        w16(&mut header, offset, value);
    }
    header[48..80].fill(0xab);
    header
}

#[test]
fn header_and_pe_agree_on_the_metadata() {
    let expected = Envelope { payload_bytes: 1024, digest: [0xab; 32], metadata: METADATA };
    assert_eq!(Envelope::parse(&header()), Ok(expected));
    assert_eq!(parse_pe(&pe()), Ok(METADATA));
}

#[test]
fn every_fixed_header_field_is_checked() {
    type Corrupt = fn(&mut [u8; 128]);
    let classes: [(&str, Corrupt); 12] = [
        ("magic", |h| h[0] ^= 1),
        ("version", |h| w32(h, 8, 2)),
        ("header bytes", |h| w32(h, 12, 132)),
        ("slot bytes", |h| w64(h, 24, 0x200000)),
        ("payload offset", |h| w64(h, 32, 132)),
        ("flags", |h| w64(h, 40, 2)),
        ("flags: two bits", |h| w64(h, 40, 6)),
        ("machine", |h| w16(h, 80, 0x14c)),
        ("subsystem", |h| w16(h, 82, 11)),
        ("optional header magic", |h| w16(h, 84, 0x10b)),
        ("reserved word", |h| w16(h, 86, 1)),
        ("reserved tail", |h| h[127] = 1),
    ];
    for (name, corrupt) in classes {
        let mut header = header();
        corrupt(&mut header);
        let actual = Envelope::parse(&header);
        assert_eq!(actual, Err(EnvelopeError::Header), "{name}");
    }
    assert_eq!(Envelope::parse(&header()[..127]), Err(EnvelopeError::Header));
    let mut long = header().to_vec();
    long.push(0);
    assert_eq!(Envelope::parse(&long), Err(EnvelopeError::Header));
}

#[test]
fn payload_bytes_stay_inside_the_slot() {
    for (bytes, expected) in [
        (511, Err(EnvelopeError::PayloadBounds)),
        (512, Ok(512)),
        (0x100000 - 128, Ok(0x100000 - 128)),
        (0x100000 - 127, Err(EnvelopeError::PayloadBounds)),
        (u64::MAX, Err(EnvelopeError::PayloadBounds)),
    ] {
        let mut header = header();
        w64(&mut header, 16, bytes);
        let actual = Envelope::parse(&header).map(|envelope| envelope.payload_bytes);
        assert_eq!(actual, expected, "{bytes}");
    }
}

#[test]
fn header_metadata_meets_the_pe_policy() {
    type Corrupt = fn(&mut [u8; 128]);
    let classes: [(&str, Corrupt); 9] = [
        ("entry below headers", |h| w32(h, 88, 511)),
        ("entry outside image", |h| w32(h, 88, 8192)),
        ("image bytes zero", |h| w32(h, 92, 0)),
        ("image bytes unaligned", |h| w32(h, 92, 8193)),
        ("image bytes above 16 MiB", |h| w32(h, 92, 16 * 1024 * 1024 + 4096)),
        ("headers beyond payload", |h| w32(h, 96, 1536)),
        ("section alignment", |h| w32(h, 100, 8192)),
        ("file alignment", |h| w32(h, 104, 4096)),
        ("seventeen sections", |h| w32(h, 108, 17)),
    ];
    for (name, corrupt) in classes {
        let mut header = header();
        corrupt(&mut header);
        let actual = Envelope::parse(&header);
        assert_eq!(actual, Err(EnvelopeError::PeGeometry), "{name}");
    }
}

#[test]
fn every_pe_refusal_names_its_cause() {
    type Corrupt = fn(&mut Vec<u8>);
    let classes: [(&str, Corrupt, EnvelopeError); 18] = [
        ("shorter than 512 bytes", |pe| pe.truncate(511), EnvelopeError::PeDosHeader),
        ("no MZ", |pe| pe[0] = b'N', EnvelopeError::PeDosHeader),
        ("e_lfanew inside the DOS header", |pe| w32(pe, 0x3c, 60), EnvelopeError::PeFileHeader),
        ("e_lfanew beyond the file", |pe| w32(pe, 0x3c, 0xffff_fff0), EnvelopeError::PeFileHeader),
        ("machine", |pe| w16(pe, 68, 0x14c), EnvelopeError::PeFileHeader),
        ("optional header size", |pe| w16(pe, 84, 224), EnvelopeError::PeFileHeader),
        ("relocations stripped", |pe| w16(pe, 86, 3), EnvelopeError::PeFileHeader),
        ("PE32 magic", |pe| w16(pe, 88, 0x10b), EnvelopeError::PeOptionalHeader),
        ("boot service driver subsystem", |pe| w16(pe, 156, 11), EnvelopeError::PeOptionalHeader),
        ("fifteen directories", |pe| w32(pe, 196, 15), EnvelopeError::PeOptionalHeader),
        ("file alignment", |pe| w32(pe, 124, 4096), EnvelopeError::PeGeometry),
        (
            "sixteen sections in 512 header bytes",
            |pe| w16(pe, 70, 16),
            EnvelopeError::PeSectionTable,
        ),
        ("import directory", |pe| w32(pe, 208, 4096), EnvelopeError::PeDirectory),
        ("relocation address without size", |pe| w32(pe, 240, 4096), EnvelopeError::PeRelocation),
        ("section RVA unaligned", |pe| w32(pe, 340, 4097), EnvelopeError::PeSection),
        (
            "writable and executable section",
            |pe| w32(pe, 364, 0xe0000020),
            EnvelopeError::PeSection,
        ),
        ("entry in a data section", |pe| w32(pe, 364, 0x40000040), EnvelopeError::PeEntry),
        (
            "relocation directory past the section's file bytes",
            |pe| {
                w32(pe, 240, 4096 + 508);
                w32(pe, 244, 8);
            },
            EnvelopeError::PeBacking,
        ),
    ];
    for (name, corrupt, expected) in classes {
        let mut pe = pe();
        corrupt(&mut pe);
        assert_eq!(parse_pe(&pe), Err(expected), "{name}");
    }
}

#[test]
fn pe_header_at_the_end_of_the_file_is_out_of_bounds_not_a_panic() {
    let mut pe = pe();
    w32(&mut pe, 0x3c, 1000);
    pe[1000..1004].copy_from_slice(b"PE\0\0");
    w16(&mut pe, 1004, 0x8664);
    w16(&mut pe, 1020, 240);
    w16(&mut pe, 1022, 2);
    assert_eq!(parse_pe(&pe), Err(EnvelopeError::FieldBounds));
}
