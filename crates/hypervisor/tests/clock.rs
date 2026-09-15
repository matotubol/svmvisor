use svmvisor_hypervisor::arch::x86_64::clock::*;

fn caps(rdtscp: bool, scaling: bool) -> ClockCapabilities {
    ClockCapabilities::detect(
        0x30,
        if rdtscp { 1 << 27 } else { 0 },
        if scaling { 1 << 4 } else { 0 },
    )
    .unwrap()
}
#[test]
fn capability_gates_are_independent() {
    for edx in [0, 0x10, 0x20] {
        assert_eq!(
            ClockCapabilities::detect(edx, u32::MAX, u32::MAX),
            Err(ClockError::MissingTscOrMsr)
        );
    }
    for aux in [false, true] {
        for ratio in [false, true] {
            let c = caps(aux, ratio);
            assert_eq!((c.rdtscp(), c.scaling()), (aux, ratio));
            let a = aux.then_some(0x1234);
            let r = ratio.then_some(3 << 32);
            let p = ClockPlan::admit(c, a, r, 0x5678).unwrap();
            assert_eq!(p.host_aux(), a);
            assert_eq!(p.host_ratio(), r);
            assert_eq!(p.guest_aux(), aux.then_some(0x5678));
            assert_eq!(p.guest_ratio(), ratio.then_some(IDENTITY_TSC_RATIO));
            assert_eq!(p.tsc_offset(), 0);
            assert_eq!(p.validate_restored(a, r), Ok(()));
        }
    }
}
#[test]
fn requires_exact_supported_msr_evidence() {
    for (c, a, r, e) in [
        (
            caps(false, false),
            Some(0),
            None,
            ClockError::AuxiliaryEvidenceMismatch,
        ),
        (
            caps(true, false),
            None,
            None,
            ClockError::AuxiliaryEvidenceMismatch,
        ),
        (
            caps(false, false),
            None,
            Some(1),
            ClockError::RatioEvidenceMismatch,
        ),
        (
            caps(false, true),
            None,
            None,
            ClockError::RatioEvidenceMismatch,
        ),
    ] {
        assert_eq!(ClockPlan::admit(c, a, r, 0), Err(e));
    }
}
#[test]
fn rejects_reserved_bits_and_zero_host_rate() {
    for bit in 32..64 {
        assert_eq!(
            ClockPlan::admit(caps(true, false), Some(1 << bit), None, 0),
            Err(ClockError::AuxiliaryReservedBits)
        );
    }
    for bit in 40..64 {
        assert_eq!(
            ClockPlan::admit(caps(false, true), None, Some((1 << bit) | 1), 0),
            Err(ClockError::RatioReservedBits)
        );
    }
    assert_eq!(
        ClockPlan::admit(caps(false, true), None, Some(0), 0),
        Err(ClockError::ZeroHostRatio)
    );
    for ratio in [1, (1 << 40) - 1] {
        assert!(
            ClockPlan::admit(
                caps(true, true),
                Some(u32::MAX as u64),
                Some(ratio),
                u32::MAX
            )
            .is_ok()
        );
    }
}
#[test]
fn restoration_detects_changed_or_missing_registers() {
    let p = ClockPlan::admit(caps(true, true), Some(123), Some(3 << 32), 456).unwrap();
    for (aux, ratio) in [
        (Some(456), p.host_ratio()),
        (p.host_aux(), p.guest_ratio()),
        (None, p.host_ratio()),
        (p.host_aux(), None),
    ] {
        assert_eq!(
            p.validate_restored(aux, ratio),
            Err(ClockError::RestorationMismatch)
        );
    }
    let p = ClockPlan::admit(caps(false, false), None, None, 0).unwrap();
    assert_eq!(
        p.validate_restored(Some(0), None),
        Err(ClockError::RestorationMismatch)
    );
}

#[test]
fn clock_cpuid_policy_changes_only_admitted_instruction_bits() {
    use svmvisor_hypervisor::svm::emulation::{cpuid, cpuid_with_clock};
    for rdtscp in [false, true] {
        let p = ClockPlan::admit(
            caps(rdtscp, true),
            rdtscp.then_some(0),
            Some(IDENTITY_TSC_RATIO),
            42,
        )
        .unwrap();
        for leaf in [
            0,
            1,
            2,
            0x15,
            0x16,
            0x4000_0000,
            0x4000_0001,
            0x8000_0000,
            0x8000_0001,
            0x8000_0007,
            0x8000_000a,
            u32::MAX,
        ] {
            let mut expected = cpuid(leaf, 0);
            if leaf == 1 {
                expected[3] |= 1 << 4;
            }
            if leaf == 0x8000_0001 && rdtscp {
                expected[3] |= 1 << 27;
            }
            assert_eq!(cpuid_with_clock(leaf, u32::MAX, &p), expected);
        }
        assert_ne!(cpuid_with_clock(1, 0, &p)[2] & (1 << 31), 0);
    }
}
