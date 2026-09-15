use svmvisor_hypervisor::host_descriptors::{HostDescriptorError as Error, HostDescriptorRequest};

fn request() -> HostDescriptorRequest {
    HostDescriptorRequest {
        gdt_base: 0x1000,
        tss_base: 0x2000,
        idt_base: 0x3000,
        rsp0: 0x8000,
        ist1: 0x9000,
        handlers: core::array::from_fn(|vector| 0xffff_9876_5432_0000 + vector as u64 * 16),
    }
}

#[test]
fn every_gate_has_exact_target_selector_privilege_and_reserved_bits() {
    let request = request();
    let image = request.clone().validate().unwrap();
    assert_eq!(image.idt().len(), 4096);
    for (vector, gate) in image.idt().chunks_exact(16).enumerate() {
        let target = u16::from_le_bytes(gate[0..2].try_into().unwrap()) as u64
            | ((u16::from_le_bytes(gate[6..8].try_into().unwrap()) as u64) << 16)
            | ((u32::from_le_bytes(gate[8..12].try_into().unwrap()) as u64) << 32);
        assert_eq!(target, request.handlers[vector]);
        assert_eq!(&gate[2..4], &[8, 0]);
        assert_eq!(gate[4], if vector == 8 { 1 } else { 0 });
        assert_eq!(gate[5], 0x8e);
        assert_eq!(&gate[12..16], &[0; 4]);
    }
    assert_eq!(
        &image.idt()[128..144],
        &[
            0x80, 0, 8, 0, 1, 0x8e, 0x32, 0x54, 0x76, 0x98, 0xff, 0xff, 0, 0, 0, 0
        ]
    );
    assert_eq!((image.idtr().base, image.idtr().limit), (0x3000, 4095));
}

#[test]
fn available_tss_and_flat_segments_have_exact_image_layout() {
    let image = request().validate().unwrap();
    assert_eq!(
        image.gdt(),
        &[
            0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0, 0xff, 0xff, 0, 0, 0, 0x93,
            0xcf, 0, 103, 0, 0, 0x20, 0, 0x89, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    );
    let mut expected = [0; 104];
    expected[5] = 0x80;
    expected[37] = 0x90;
    expected[102] = 104;
    assert_eq!(image.tss(), &expected);
    assert_eq!((image.gdtr().base, image.gdtr().limit), (0x1000, 39));
    assert_eq!(
        (
            image.code_selector(),
            image.data_selector(),
            image.tss_selector()
        ),
        (8, 16, 24)
    );
}

#[test]
fn rejects_invalid_ranges_and_all_table_overlap_pairs() {
    for base in [
        0x8000_0000_0000,
        0xffff_7fff_ffff_ffff,
        0x7fff_ffff_fff0,
        u64::MAX - 1,
    ] {
        assert_eq!(
            HostDescriptorRequest {
                gdt_base: base,
                ..request()
            }
            .validate(),
            Err(Error::InvalidGdtRange)
        );
        assert_eq!(
            HostDescriptorRequest {
                tss_base: base,
                ..request()
            }
            .validate(),
            Err(Error::InvalidTssRange)
        );
        assert_eq!(
            HostDescriptorRequest {
                idt_base: base,
                ..request()
            }
            .validate(),
            Err(Error::InvalidIdtRange)
        );
    }
    assert_eq!(
        HostDescriptorRequest {
            tss_base: 0x1027,
            ..request()
        }
        .validate(),
        Err(Error::OverlappingTables)
    );
    assert_eq!(
        HostDescriptorRequest {
            idt_base: 0x1027,
            ..request()
        }
        .validate(),
        Err(Error::OverlappingTables)
    );
    assert_eq!(
        HostDescriptorRequest {
            idt_base: 0x2067,
            ..request()
        }
        .validate(),
        Err(Error::OverlappingTables)
    );
    assert!(
        HostDescriptorRequest {
            tss_base: 0x1028,
            idt_base: 0x1090,
            ..request()
        }
        .validate()
        .is_ok()
    );
    assert!(
        HostDescriptorRequest {
            idt_base: u64::MAX - 4095,
            ..request()
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn rejects_null_and_noncanonical_stacks_or_any_handler_including_last_vector() {
    for pointer in [0, 0x8000_0000_0000, 0xffff_7fff_ffff_ffff] {
        assert_eq!(
            HostDescriptorRequest {
                rsp0: pointer,
                ..request()
            }
            .validate(),
            Err(Error::InvalidRsp0)
        );
        assert_eq!(
            HostDescriptorRequest {
                ist1: pointer,
                ..request()
            }
            .validate(),
            Err(Error::InvalidIst1)
        );
        for vector in [0, 8, 255] {
            let mut request = request();
            request.handlers[vector] = pointer;
            assert_eq!(
                request.validate(),
                Err(Error::InvalidHandler {
                    vector: vector as u8
                })
            );
        }
    }
}

#[test]
fn native_terminal_profile_uses_ist_without_changing_returning_sx_or_targets() {
    let expected = request().validate().unwrap();
    let image = request().validate_terminal_ist().unwrap();
    assert_eq!(image.tss(), expected.tss());
    for vector in 0..256 {
        let mut expected_gate = expected.idt()[vector * 16..(vector + 1) * 16].to_vec();
        expected_gate[4] = u8::from(vector != 30);
        assert_eq!(&image.idt()[vector * 16..(vector + 1) * 16], expected_gate);
    }
}
