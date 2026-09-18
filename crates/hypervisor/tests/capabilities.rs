use svmvisor_hypervisor::arch::x86_64::capabilities::{
    CapabilityError as E, CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures,
};
use svmvisor_hypervisor::memory::address::{AddressError, EncryptionState};

fn evidence() -> CapabilityEvidence {
    CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(1),
        asid_count: Some(32768),
        physical_address_bits: Some(48),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption: EncryptionState::Unencrypted { encryption_bit: Some(47) },
        optional: OptionalFeatures::default(),
    }
}

#[test]
fn required_contract_and_asid_boundaries() {
    let caps = evidence().validate().unwrap();
    assert_eq!(caps.svm_revision(), 1);
    assert_eq!(caps.address_policy().physical_bits(), 48);
    assert_eq!(caps.asid_count(), 32768);
    assert_eq!(caps.validate_asid(0), Err(E::InvalidAsid));
    assert_eq!(caps.validate_asid(1), Ok(()));
    assert_eq!(caps.validate_asid(32767), Ok(()));
    assert_eq!(caps.validate_asid(32768), Err(E::InvalidAsid));
    assert_eq!(caps.validate_asid(u64::MAX as u32), Err(E::InvalidAsid));
    let mut e = evidence();
    e.asid_count = Some(2);
    assert_eq!(e.validate().unwrap().validate_asid(1), Ok(()));
}

#[test]
fn missing_and_unsupported_required_evidence_rejected() {
    for vendor in [CpuVendor::Unknown, CpuVendor::Other] {
        assert_eq!(
            CapabilityEvidence { vendor, ..evidence() }.validate(),
            Err(E::UnsupportedVendor)
        );
    }
    for flag in [EvidenceFlag::Unknown, EvidenceFlag::Clear] {
        assert_eq!(
            CapabilityEvidence { svm: flag, ..evidence() }.validate(),
            Err(E::SvmNotEstablished)
        );
        assert_eq!(
            CapabilityEvidence { nested_paging: flag, ..evidence() }.validate(),
            Err(E::NestedPagingNotEstablished)
        );
    }
    for flag in [EvidenceFlag::Unknown, EvidenceFlag::Set] {
        assert_eq!(
            CapabilityEvidence { vm_cr_svmdis: flag, ..evidence() }.validate(),
            Err(E::SvmDisabledOrUnknown)
        );
        assert_eq!(
            CapabilityEvidence { hypervisor_present: flag, ..evidence() }.validate(),
            Err(E::HypervisorPresentOrUnknown)
        );
    }
    for svm_revision in [None, Some(0), Some(2), Some(255)] {
        assert_eq!(
            CapabilityEvidence { svm_revision, ..evidence() }.validate(),
            Err(E::UnsupportedRevision)
        );
    }
    for asid_count in [None, Some(0), Some(1)] {
        assert_eq!(
            CapabilityEvidence { asid_count, ..evidence() }.validate(),
            Err(E::InsufficientAsids)
        );
    }
    assert_eq!(
        CapabilityEvidence { physical_address_bits: None, ..evidence() }.validate(),
        Err(E::MissingPhysicalWidth)
    );
    assert_eq!(
        CapabilityEvidence { physical_address_bits: Some(64), ..evidence() }.validate(),
        Err(E::Address(AddressError::UnsupportedPhysicalWidth))
    );
    assert_eq!(
        CapabilityEvidence { encryption: EncryptionState::Unknown, ..evidence() }.validate(),
        Err(E::Address(AddressError::UnknownEncryption))
    );
    assert_eq!(
        CapabilityEvidence { encryption: EncryptionState::Active, ..evidence() }.validate(),
        Err(E::Address(AddressError::ActiveEncryptionUnsupported))
    );
}

#[test]
fn optional_accelerators_are_preserved_and_not_required() {
    assert_eq!(evidence().validate().unwrap().optional_features(), OptionalFeatures::default());
    for mask in 0..16 {
        let optional = OptionalFeatures {
            nrip_save: mask & 1 != 0,
            decode_assists: mask & 2 != 0,
            vmcb_clean: mask & 4 != 0,
            flush_by_asid: mask & 8 != 0,
        };
        assert_eq!(
            CapabilityEvidence { optional, ..evidence() }.validate().unwrap().optional_features(),
            optional
        );
    }
}
