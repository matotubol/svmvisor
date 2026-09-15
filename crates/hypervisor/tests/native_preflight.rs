use svmvisor_hypervisor::{
    address::EncryptionState,
    capabilities::{CapabilityError, EvidenceFlag},
    native_preflight::*,
};
fn evidence() -> CpuidEvidence {
    CpuidEvidence {
        basic: CpuidRegisters {
            eax: 0x10,
            ebx: 0x68747541,
            edx: 0x69746e65,
            ecx: 0x444d4163,
        },
        extended: CpuidRegisters {
            eax: 0x8000001f,
            ..Default::default()
        },
        features: Some(CpuidRegisters {
            edx: 1 << 5,
            ..Default::default()
        }),
        extended_features: Some(CpuidRegisters {
            ecx: 4,
            edx: (1 << 20) | (1 << 29),
            ..Default::default()
        }),
        address_width: Some(CpuidRegisters {
            eax: 48,
            ..Default::default()
        }),
        svm: Some(CpuidRegisters {
            eax: 1,
            ebx: 32768,
            edx: 1,
            ..Default::default()
        }),
    }
}
#[test]
fn cpuid_success_never_fabricates_native_admission() {
    let caps = evidence().evaluate().unwrap().incomplete_capabilities();
    assert_eq!(caps.vm_cr_svmdis, EvidenceFlag::Unknown);
    assert_eq!(caps.encryption, EncryptionState::Unknown);
    assert_eq!(caps.validate(), Err(CapabilityError::SvmDisabledOrUnknown));
    assert_eq!(caps.physical_address_bits, Some(48));
}
#[test]
fn missing_leaves_and_reported_hypervisor_refuse() {
    let mut e = evidence();
    e.features = None;
    assert_eq!(e.evaluate(), Err(PreflightError::MissingLeaves));
    e = evidence();
    e.extended.eax = 0x80000008;
    assert_eq!(e.evaluate(), Err(PreflightError::MissingLeaves));
    e = evidence();
    e.basic.ebx = 0;
    assert_eq!(e.evaluate(), Err(PreflightError::UnsupportedVendor));
    e = evidence();
    e.features.as_mut().unwrap().ecx |= 1 << 31;
    assert_eq!(e.evaluate(), Err(PreflightError::HypervisorReported));
}
#[test]
fn required_cpu_features_and_boundaries_fail_closed() {
    for (mutation, expected) in [
        (0, PreflightError::MsrUnsupported),
        (1, PreflightError::SvmUnsupported),
        (2, PreflightError::NxUnsupported),
        (3, PreflightError::NptUnsupported),
        (4, PreflightError::UnsupportedRevision),
        (5, PreflightError::InsufficientAsids),
        (6, PreflightError::UnsupportedPhysicalWidth),
    ] {
        let mut e = evidence();
        match mutation {
            0 => e.features.as_mut().unwrap().edx = 0,
            1 => e.extended_features.as_mut().unwrap().ecx = 0,
            2 => e.extended_features.as_mut().unwrap().edx = 0,
            3 => e.svm.as_mut().unwrap().edx = 0,
            4 => e.svm.as_mut().unwrap().eax = 2,
            5 => e.svm.as_mut().unwrap().ebx = 1,
            _ => e.address_width.as_mut().unwrap().eax = 53,
        }
        assert_eq!(e.evaluate(), Err(expected));
    }
    for bits in [32, 52] {
        let mut e = evidence();
        e.address_width.as_mut().unwrap().eax = bits;
        e.svm.as_mut().unwrap().ebx = 2;
        assert!(e.evaluate().is_ok());
    }
}

#[test]
fn long_mode_must_be_enumerated_separately_from_nx() {
    let mut e = evidence();
    e.extended_features.as_mut().unwrap().edx &= !(1 << 29);
    assert_eq!(e.evaluate(), Err(PreflightError::LongModeUnsupported));
}

#[test]
fn optional_features_decode_independent_bits_without_neighbor_aliases() {
    for bit in 1..32 {
        let mut e = evidence();
        e.svm.as_mut().unwrap().edx = 1 | (1 << bit);
        let optional = e.evaluate().unwrap().incomplete_capabilities().optional;
        assert_eq!(optional.nrip_save, bit == 3);
        assert_eq!(optional.vmcb_clean, bit == 5);
        assert_eq!(optional.flush_by_asid, bit == 6);
        assert_eq!(optional.decode_assists, bit == 7);
    }
    assert_eq!(
        evidence()
            .evaluate()
            .unwrap()
            .incomplete_capabilities()
            .optional,
        svmvisor_hypervisor::capabilities::OptionalFeatures::default()
    );
}
