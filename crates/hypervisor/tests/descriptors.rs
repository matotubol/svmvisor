use svmvisor_hypervisor::descriptors::{DescriptorError, GuestDescriptorRequest};

fn request() -> GuestDescriptorRequest {
    GuestDescriptorRequest {
        gdt_base: 0x2000,
        tss_base: 0xffff_8765_4321_0000,
        rsp0: 0xffff_9876_5432_1000,
        ist1: 0x1234_5678_9000,
    }
}

#[test]
fn gdt_bytes_and_expanded_segments_match_architectural_layout() {
    let image = request().validate().unwrap();
    assert_eq!(
        image.gdt(),
        &[
            0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0, 0xff, 0xff, 0, 0, 0, 0x93,
            0xcf, 0, 0x67, 0, 0, 0, 0x21, 0x8b, 0, 0x43, 0x65, 0x87, 0xff, 0xff, 0, 0, 0, 0,
        ]
    );
    assert_eq!(
        (
            image.cs().selector,
            image.cs().attributes,
            image.cs().limit,
            image.cs().base
        ),
        (8, 0xa9b, u32::MAX, 0)
    );
    assert_eq!(
        (
            image.data().selector,
            image.data().attributes,
            image.data().limit
        ),
        (16, 0xc93, u32::MAX)
    );
    assert_eq!(
        (
            image.tr().selector,
            image.tr().attributes,
            image.tr().limit,
            image.tr().base
        ),
        (24, 0x8b, 103, request().tss_base)
    );
    assert_eq!(
        (
            image.gdtr().selector,
            image.gdtr().attributes,
            image.gdtr().limit,
            image.gdtr().base
        ),
        (0, 0, 39, request().gdt_base)
    );
}

#[test]
fn tss_has_exact_stack_offsets_and_zero_reserved_and_unused_fields() {
    let image = request().validate().unwrap();
    let mut expected = [0_u8; 104];
    expected[4..12].copy_from_slice(&[0, 0x10, 0x32, 0x54, 0x76, 0x98, 0xff, 0xff]);
    expected[36..44].copy_from_slice(&[0, 0x90, 0x78, 0x56, 0x34, 0x12, 0, 0]);
    expected[102] = 104;
    assert_eq!(image.tss(), &expected);
    assert!(
        u16::from_le_bytes(image.tss()[102..104].try_into().unwrap()) as u32 > image.tr().limit
    );
}

#[test]
fn rejects_noncanonical_ranges_overflow_overlap_and_stack_pointers() {
    for base in [
        0x8000_0000_0000,
        0xffff_7fff_ffff_ffff,
        0x7fff_ffff_fff0,
        u64::MAX - 10,
    ] {
        assert_eq!(
            GuestDescriptorRequest {
                gdt_base: base,
                ..request()
            }
            .validate(),
            Err(DescriptorError::InvalidGdtRange)
        );
        assert_eq!(
            GuestDescriptorRequest {
                tss_base: base,
                ..request()
            }
            .validate(),
            Err(DescriptorError::InvalidTssRange)
        );
    }
    for tss_base in [0x2000, 0x1f99, 0x2027] {
        assert_eq!(
            GuestDescriptorRequest {
                tss_base,
                ..request()
            }
            .validate(),
            Err(DescriptorError::OverlappingTables)
        );
    }
    assert_eq!(
        GuestDescriptorRequest {
            rsp0: 0x8000_0000_0000,
            ..request()
        }
        .validate(),
        Err(DescriptorError::NonCanonicalRsp0)
    );
    assert_eq!(
        GuestDescriptorRequest {
            ist1: 0x8000_0000_0000,
            ..request()
        }
        .validate(),
        Err(DescriptorError::NonCanonicalIst1)
    );
    for tss_base in [0x1f98, 0x2028, 0x7fff_ffff_ff98, u64::MAX - 103] {
        assert!(
            GuestDescriptorRequest {
                tss_base,
                ..request()
            }
            .validate()
            .is_ok()
        );
    }
}
