use svmvisor_hypervisor::address::EncryptionState;
use svmvisor_hypervisor::capabilities::{
    CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
};
use svmvisor_hypervisor::exit::{ExitAction, ExitSnapshot, ResumeError, TranslationStage};

fn capabilities(nrip_save: bool) -> ValidatedCapabilities {
    CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(1),
        asid_count: Some(32768),
        physical_address_bits: Some(48),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption: EncryptionState::Unencrypted {
            encryption_bit: None,
        },
        optional: OptionalFeatures {
            nrip_save,
            ..OptionalFeatures::default()
        },
    }
    .validate()
    .unwrap()
}

fn snapshot(code: u64) -> ExitSnapshot {
    ExitSnapshot {
        code,
        info1: u64::MAX,
        info2: u64::MAX,
        rip: 0x1000,
        nrip: 0x1002,
    }
}

#[test]
fn snapshot_decodes_exact_appendix_b_offsets_and_full_little_endian_fields() {
    let mut bytes = [0xa5; 4096];
    bytes[0x070..0x078].copy_from_slice(&[0xff; 8]);
    bytes[0x078..0x080].copy_from_slice(&[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe]);
    bytes[0x080..0x088].copy_from_slice(&[0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12]);
    bytes[0x578..0x580].copy_from_slice(&[0xf0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    bytes[0x0c8..0x0d0].copy_from_slice(&[0xf3, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    let retained = bytes;
    assert_eq!(
        ExitSnapshot::from_vmcb_bytes(&bytes),
        ExitSnapshot {
            code: u64::MAX,
            info1: 0xfedc_ba98_7654_3210,
            info2: 0x1234_5678_9abc_def0,
            rip: 0xffff_ffff_ffff_fff0,
            nrip: 0xffff_ffff_ffff_fff3,
        }
    );
    assert_eq!(bytes, retained);
}

#[test]
fn codes_use_all_64_bits_and_never_interpret_undefined_information() {
    for (code, action) in [
        (0x72, ExitAction::CpuidPolicyRequired),
        (0x78, ExitAction::StopOnHlt),
        (0x7f, ExitAction::Shutdown),
        (0x81, ExitAction::HypercallHandlerRequired),
        (u64::MAX, ExitAction::InvalidVmcb),
    ] {
        assert_eq!(snapshot(code).action(), action);
    }
    for code in [
        0,
        0x80,
        0x7c,
        0xa6,
        0xffff_ffff,
        0x1_0000_0072,
        u64::MAX - 1,
    ] {
        assert_eq!(snapshot(code).action(), ExitAction::Unsupported { code });
    }
}

#[test]
fn nested_fault_preserves_raw_bits_and_distinguishes_walk_from_final_translation() {
    for (stage_bits, stage) in [
        (0, TranslationStage::Unspecified),
        (1, TranslationStage::FinalGuestPhysical),
        (2, TranslationStage::GuestPageTable),
        (3, TranslationStage::Ambiguous),
    ] {
        let raw = (stage_bits << 32) | (1 << 63) | 0x1f;
        let value = ExitSnapshot {
            info1: raw,
            info2: 0xfedc_ba98_7654_3210,
            ..snapshot(0x400)
        };
        let ExitAction::NestedPageFault(fault) = value.action() else {
            panic!("expected NPF");
        };
        assert_eq!(fault.raw_info(), raw);
        assert_eq!(fault.guest_physical_address(), value.info2);
        assert_eq!(fault.stage(), stage);
        assert!(fault.present());
        assert!(fault.write());
        assert!(fault.user());
        assert!(fault.reserved_bit_violation());
        assert!(fault.instruction_fetch());
    }
    let ExitAction::NestedPageFault(fault) = (ExitSnapshot {
        info1: 0,
        ..snapshot(0x400)
    })
    .action() else {
        panic!("expected NPF");
    };
    assert!(!fault.present());
    assert!(!fault.write());
    assert!(!fault.user());
    assert!(!fault.reserved_bit_violation());
    assert!(!fault.instruction_fetch());
}

#[test]
fn only_supported_nonterminal_instruction_exits_can_yield_an_nrip_candidate() {
    let available = capabilities(true);
    for code in [0x72, 0x81] {
        assert_eq!(
            snapshot(code).resume_candidate(&capabilities(false)),
            Err(ResumeError::NripNotEstablished)
        );
        for length in [1, 2, 3, 15] {
            let value = ExitSnapshot {
                nrip: 0x1000 + length,
                ..snapshot(code)
            };
            let candidate = value.resume_candidate(&available).unwrap();
            assert_eq!(candidate.address(), value.nrip);
            assert_eq!(candidate.instruction_bytes(), length as u8);
            // Validation is pure: it does not consume or mutate the snapshot.
            assert_eq!(value.rip, 0x1000);
        }
    }
    for code in [0x78, 0x400, u64::MAX, 0x7b, 0x7c, 0x1_0000_0072] {
        assert_eq!(
            snapshot(code).resume_candidate(&available),
            Err(ResumeError::ExitDoesNotPermitCandidate)
        );
    }
}

#[test]
fn nrip_never_wraps_skips_large_distances_or_accepts_noncanonical_pointers() {
    let available = capabilities(true);
    for (rip, nrip) in [
        (0x1000, 0x1000),
        (0x1000, 0x0fff),
        (0x1000, 0x1010),
        (u64::MAX, 0),
    ] {
        assert_eq!(
            (ExitSnapshot {
                rip,
                nrip,
                ..snapshot(0x72)
            })
            .resume_candidate(&available),
            Err(ResumeError::InvalidInstructionLength)
        );
    }
    let noncanonical = 0x0000_8000_0000_0000;
    assert_eq!(
        (ExitSnapshot {
            rip: noncanonical,
            ..snapshot(0x72)
        })
        .resume_candidate(&available),
        Err(ResumeError::NonCanonicalRip)
    );
    assert_eq!(
        (ExitSnapshot {
            rip: noncanonical - 1,
            nrip: noncanonical,
            ..snapshot(0x72)
        })
        .resume_candidate(&available),
        Err(ResumeError::NonCanonicalNrip)
    );
    let high = ExitSnapshot {
        rip: 0xffff_8000_0000_0000,
        nrip: 0xffff_8000_0000_000f,
        ..snapshot(0x81)
    };
    assert_eq!(
        high.resume_candidate(&available)
            .unwrap()
            .instruction_bytes(),
        15
    );
}
