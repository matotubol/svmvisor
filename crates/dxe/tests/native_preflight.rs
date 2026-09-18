#![cfg(feature = "native-preflight")]

use svmvisor_dxe::native::admission::preflight::{Outcome, collect};
use svmvisor_hypervisor::{
    boot::preflight::{CpuidRegisters as R, PreflightError},
    svm::cpu_model::CpuIdentityError,
};

fn raw_brand() -> [u8; 48] {
    // Deliberately includes NUL and non-UTF8 bytes: this is CPUID evidence,
    // not a human-readable string reconstruction.
    core::array::from_fn(|i| (i as u8).wrapping_mul(37))
}

fn amd(leaf: u32) -> R {
    match leaf {
        0 => R { eax: 1, ebx: 0x68747541, edx: 0x69746e65, ecx: 0x444d4163 },
        0x80000000 => R { eax: 0x8000000a, ..amd(0) },
        1 => R { eax: 0x00b40f12, edx: 1 << 5, ..R::default() },
        0x80000001 => R { eax: 0x00b40f12, ecx: 4, edx: (1 << 20) | (1 << 29), ..R::default() },
        0x80000002..=0x80000004 => {
            let bytes = raw_brand();
            let offset = (leaf - 0x80000002) as usize * 16;
            let word = |index| {
                u32::from_le_bytes(bytes[offset + index..offset + index + 4].try_into().unwrap())
            };
            R { eax: word(0), ebx: word(4), ecx: word(8), edx: word(12) }
        }
        0x80000008 => R { eax: 48, ..R::default() },
        0x8000000a => R { eax: 1, ebx: 32, edx: 1, ..R::default() },
        _ => panic!("unsupported query"),
    }
}

#[test]
fn bounded_collection_reaches_explicit_boundary_refusal() {
    let mut calls = Vec::new();
    let report = collect(|leaf| {
        calls.push(leaf);
        amd(leaf)
    });
    assert_eq!(
        calls,
        [0, 0x80000000, 1, 0x80000001, 0x80000008, 0x8000000a, 0x80000002, 0x80000003, 0x80000004]
    );
    assert_eq!(report.outcome, Outcome::NativeBoundaryUnavailable);
    assert_eq!(report.outcome.diagnostic_code(), 0x100);
    let identity = report.identity.unwrap();
    assert_eq!(identity.vendor(), *b"AuthenticAMD");
    assert_eq!(identity.signature(), 0x00b40f12);
    assert_eq!(identity.extended_signature(), 0x00b40f12);
    assert_eq!(identity.brand(), raw_brand());
}

#[test]
fn unsupported_leaf_ranges_are_never_queried() {
    let mut calls = Vec::new();
    let report = collect(|leaf| {
        calls.push(leaf);
        match leaf {
            0 => R { eax: 0, ..amd(0) },
            0x80000000 => R { eax: 0x80000000, ..R::default() },
            _ => panic!("queried missing leaf"),
        }
    });
    assert_eq!(calls, [0, 0x80000000]);
    assert_eq!(report.outcome, Outcome::CpuidRejected(PreflightError::MissingLeaves));
    assert_eq!(report.evidence.features, None);
    assert_eq!(report.evidence.extended_features, None);
    assert_eq!(report.identity, Err(CpuIdentityError::InvalidMaxima));
}

#[test]
fn reported_hypervisor_reaches_core_refusal_unchanged() {
    let report = collect(|leaf| {
        let mut r = amd(leaf);
        if leaf == 1 {
            r.ecx |= 1 << 31;
        }
        r
    });
    assert_eq!(report.outcome, Outcome::CpuidRejected(PreflightError::HypervisorReported));
    assert_eq!(report.outcome.diagnostic_code(), 3);
    assert_ne!(report.evidence.features.unwrap().ecx & (1 << 31), 0);
    assert!(report.identity.is_ok(), "identity does not override SVM refusal");
}

#[test]
fn complete_brand_group_is_collected_only_when_all_three_leaves_are_enumerated() {
    for maximum in 0x80000000..=0x8000000a {
        let mut calls = Vec::new();
        let report = collect(|leaf| {
            calls.push(leaf);
            assert!(leaf <= 1 || leaf <= maximum, "unsupported extended leaf {leaf:x}");
            if leaf == 0x80000000 { R { eax: maximum, ..amd(leaf) } } else { amd(leaf) }
        });
        let brand_calls: Vec<_> =
            calls.iter().copied().filter(|leaf| (0x80000002..=0x80000004).contains(leaf)).collect();
        if maximum < 0x80000004 {
            assert!(brand_calls.is_empty());
            assert!(report.identity.is_err());
        } else {
            assert_eq!(brand_calls, [0x80000002, 0x80000003, 0x80000004]);
            assert_eq!(report.identity.unwrap().brand(), raw_brand());
        }
        assert!(calls.len() <= 9);
        // Feature observations are reused, never re-read for identity.
        for leaf in calls.iter() {
            assert_eq!(calls.iter().filter(|other| *other == leaf).count(), 1);
        }
        assert_eq!(
            report.outcome,
            if maximum < 0x8000000a {
                Outcome::CpuidRejected(PreflightError::MissingLeaves)
            } else {
                Outcome::NativeBoundaryUnavailable
            }
        );
    }
}

#[test]
fn invalid_identity_is_reported_without_changing_existing_preflight_outcome() {
    for (leaf, altered, identity_error) in [
        (1, R { eax: 0, ..amd(1) }, CpuIdentityError::InvalidSignature),
        (
            0x80000001,
            R { eax: 0x00b40f13, ..amd(0x80000001) },
            CpuIdentityError::InconsistentSignature,
        ),
        (0x80000000, R { ebx: 0, ..amd(0x80000000) }, CpuIdentityError::InconsistentVendor),
    ] {
        let report = collect(|query| if query == leaf { altered } else { amd(query) });
        assert_eq!(report.identity, Err(identity_error));
        assert_eq!(report.outcome, Outcome::NativeBoundaryUnavailable);
        assert_eq!(report.outcome.diagnostic_code(), 0x100);
    }
}

#[test]
fn missing_basic_signature_and_invalid_extended_maximum_never_create_identity() {
    let mut calls = Vec::new();
    let report = collect(|leaf| {
        calls.push(leaf);
        if leaf == 0 { R { eax: 0, ..amd(0) } } else { amd(leaf) }
    });
    assert!(!calls.contains(&1));
    assert_eq!(report.evidence.features, None);
    assert_eq!(report.identity, Err(CpuIdentityError::InvalidMaxima));
    let mut calls = Vec::new();
    let report = collect(|leaf| {
        calls.push(leaf);
        if leaf == 0x80000000 { R { eax: u32::MAX, ..amd(leaf) } } else { amd(leaf) }
    });
    assert!(calls.iter().all(|leaf| !(0x80000002..=0x80000004).contains(leaf)));
    assert_eq!(report.identity, Err(CpuIdentityError::InvalidMaxima));
    // Existing feature preflight semantics remain distinct from identity's
    // stricter namespace validation; no new SVM admission rule is introduced.
    assert_eq!(report.outcome, Outcome::NativeBoundaryUnavailable);
}
