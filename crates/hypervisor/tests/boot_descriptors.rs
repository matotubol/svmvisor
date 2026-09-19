use svmvisor_hypervisor::{boot::descriptors::*, host::descriptors::HostTablePointer};

fn fixture() -> ([u8; 32], FirmwareSelectors, HostTablePointer) {
    let mut bytes = [0; 32];
    bytes[8..16].copy_from_slice(&0x00af_9b00_0000_ffffu64.to_le_bytes());
    bytes[16..24].copy_from_slice(&0x00cf_9300_0000_ffffu64.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x00cf_f300_0000_ffffu64.to_le_bytes());
    (
        bytes,
        FirmwareSelectors { cs: 8, ss: 16, ds: 16, es: 16 },
        HostTablePointer { base: 0x1000, limit: 31 },
    )
}

#[test]
fn decoded_fields_and_original_capture_are_retained() {
    let (bytes, selectors, table) = fixture();
    let parsed = parse_firmware_gdt(table, selectors, &bytes).unwrap();
    assert_eq!(parsed.original_bytes(), &bytes);
    assert_eq!(parsed.table(), table);
    assert_eq!(parsed.selectors(), selectors);
    match parsed.segments()[0] {
        CapturedSegment::Descriptor { raw, decoded } => {
            assert_eq!(raw, 0x00af_9b00_0000_ffff);
            assert_eq!(decoded.attributes, 0xa9b);
            assert_eq!(decoded.base, 0);
            assert_eq!(decoded.limit, u32::MAX);
        }
        _ => panic!("code descriptor missing"),
    }
}

#[test]
fn permitted_null_segments_do_not_read_entry_zero() {
    let (mut bytes, mut selectors, table) = fixture();
    bytes[..8].fill(0xff);
    selectors.ss = 0;
    selectors.ds = 3;
    selectors.es = 0;
    let parsed = parse_firmware_gdt(table, selectors, &bytes).unwrap();
    assert_eq!(parsed.segments()[1], CapturedSegment::Null { selector: 0 });
    assert_eq!(parsed.segments()[2], CapturedSegment::Null { selector: 3 });
    selectors.cs = 0;
    assert_eq!(
        parse_firmware_gdt(table, selectors, &bytes).unwrap_err(),
        FirmwareDescriptorError::NullCode
    );
}

#[test]
fn bounds_and_mapping_evidence_fail_closed() {
    let (bytes, selectors, mut table) = fixture();
    table.base = 0x1ff8;
    let parsed = parse_firmware_gdt(table, selectors, &bytes).unwrap();
    assert_eq!(parsed.required_mapping(), GdtRange { first: 0x1ff8, last: 0x2017 });
    let mut pages = [
        CapturedGdtPage { linear_page: 0x1000, present: true, writable: true },
        CapturedGdtPage { linear_page: 0x2000, present: true, writable: true },
    ];
    assert_eq!(parsed.validate_mapping_capture(&pages), Ok(()));
    assert_eq!(
        parsed.validate_mapping_capture(&pages[..1]),
        Err(FirmwareDescriptorError::MappingCoverage)
    );
    pages[1].linear_page = 0x1000;
    assert_eq!(
        parsed.validate_mapping_capture(&pages),
        Err(FirmwareDescriptorError::MappingCoverage)
    );
    pages[1].linear_page = 0x2000;
    pages[1].present = false;
    assert_eq!(
        parsed.validate_mapping_capture(&pages),
        Err(FirmwareDescriptorError::MappingNotPresent)
    );
    pages[1].present = true;
    pages[1].writable = false;
    assert_eq!(
        parsed.validate_mapping_capture(&pages),
        Err(FirmwareDescriptorError::MappingNotWritable)
    );
    for base in [0x8000_0000_0000, u64::MAX - 15, 0x7fff_ffff_fff8] {
        table.base = base;
        assert_eq!(
            parse_firmware_gdt(table, selectors, &bytes).unwrap_err(),
            FirmwareDescriptorError::NonCanonicalTable
        );
    }
    table.base = 0xffff_8000_0000_1000;
    assert!(parse_firmware_gdt(table, selectors, &bytes).is_ok());
    assert_eq!(
        parse_firmware_gdt(table, selectors, &bytes[..31]).unwrap_err(),
        FirmwareDescriptorError::WrongCaptureLength
    );
    table.limit = 30;
    assert_eq!(
        parse_firmware_gdt(table, selectors, &bytes).unwrap_err(),
        FirmwareDescriptorError::WrongCaptureLength
    );
}

#[test]
fn selector_tables_extents_and_privilege_are_validated() {
    let (bytes, mut s, table) = fixture();
    s.cs = 12;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::LdtSelector(FirmwareSegment::Cs)
    );
    s.cs = 8;
    s.ds = 4;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::LdtSelector(FirmwareSegment::Ds)
    );
    s.ds = 32;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::SelectorOutsideTable(FirmwareSegment::Ds)
    );
    s.ds = 27; // Ring3 data is legal for CPL0 with RPL3 <= DPL3.
    assert!(parse_firmware_gdt(table, s, &bytes).is_ok());
    s.ds = 19;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::InvalidData(FirmwareSegment::Ds)
    );
    s.ds = 16;
    s.ss = 27;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::InvalidStack
    );
    s.ss = 3;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::InvalidStack
    );
    s.ss = 16;
    s.cs = 11;
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::InvalidCode
    );
}

#[test]
fn malformed_descriptor_classes_are_rejected() {
    let (original, s, table) = fixture();
    for (offset, bit, error) in [
        (13, 0x80, FirmwareDescriptorError::NotPresent(FirmwareSegment::Cs)),
        (13, 0x10, FirmwareDescriptorError::SystemDescriptor(FirmwareSegment::Cs)),
        (14, 0x20, FirmwareDescriptorError::InvalidCode),
        (14, 0x40, FirmwareDescriptorError::InvalidCode),
        (21, 0x02, FirmwareDescriptorError::InvalidStack),
    ] {
        let mut bytes = original;
        bytes[offset] ^= bit;
        assert_eq!(parse_firmware_gdt(table, s, &bytes).unwrap_err(), error);
    }
    let mut bytes = original;
    bytes[24..32].copy_from_slice(&0x00af_9900_0000_ffffu64.to_le_bytes());
    let s = FirmwareSelectors { ds: 24, ..s };
    assert_eq!(
        parse_firmware_gdt(table, s, &bytes).unwrap_err(),
        FirmwareDescriptorError::InvalidData(FirmwareSegment::Ds)
    );
}

#[test]
fn data_register_rejects_reserved_code_size_encoding() {
    let (mut bytes, mut selectors, table) = fixture();
    bytes[24..32].copy_from_slice(&0x00ef_9b00_0000_ffffu64.to_le_bytes());
    selectors.ds = 24;
    assert_eq!(
        parse_firmware_gdt(table, selectors, &bytes).unwrap_err(),
        FirmwareDescriptorError::InvalidData(FirmwareSegment::Ds)
    );
}
