// The transition state module exists only in these two profiles.
#![cfg(any(feature = "native-transition-test", feature = "native-returning"))]

use svmvisor_hypervisor::svm::emulation::{self, HypercallAction};
use svmvisor_launcher::native::transition::state::{
    self as native_transition, GuestObservation, NativeTransition, mode, multi, outcome,
};

#[test]
fn all_fixed_cpuid_cases_match_core_semantics_and_zero_extend() {
    for round in 0..multi::ROUNDS {
        let case = (round % 8) as usize;
        let poisoned_rax = multi::CPUID_INPUT_RAX_HIGH | u64::from(multi::CPUID_LEAVES[case]);
        assert_ne!(poisoned_rax >> 32, 0);
        assert_ne!(multi::CPUID_INPUT_RCX >> 32, 0);
        let actual = emulation::cpuid(poisoned_rax as u32, multi::CPUID_INPUT_RCX as u32);
        assert_eq!(actual.map(u64::from), multi::CPUID_OUTPUTS[case]);
        assert!(multi::CPUID_OUTPUTS[case].into_iter().all(|value| value >> 32 == 0));
    }
    assert_eq!(emulation::HYPERCALL_QUERY, 0);
    assert_eq!(emulation::hypercall(0), HypercallAction::Query { abi_version: 1 });
    assert_eq!(emulation::HYPERCALL_STOP, 1);
    assert_eq!(emulation::hypercall(1), HypercallAction::Stop);
    for bit in 32..64 {
        assert!(matches!(emulation::hypercall(1u64 << bit), HypercallAction::Unsupported { .. }));
        assert!(matches!(
            emulation::hypercall((1u64 << bit) | 1),
            HypercallAction::Unsupported { .. }
        ));
    }
}

#[test]
fn new_profile_keeps_old_mode_outcome_and_context_layout_meanings() {
    assert_eq!((mode::ONE_ENTRY, mode::BIND_ONLY, mode::MULTI_EXIT), (0, 1, 2));
    assert_eq!((outcome::VMMCALL, outcome::ROUND_TRIP, outcome::MULTI_EXIT), (2, 9, 12));
    assert_eq!(native_transition::ABI_VERSION, 1);
    assert_eq!(core::mem::size_of::<NativeTransition>(), 1088);
    assert_eq!(core::mem::offset_of!(NativeTransition, guest), 704);
    assert_eq!(core::mem::offset_of!(NativeTransition, journal), 960);
    assert_eq!(core::mem::size_of::<GuestObservation>(), 256);
    assert_eq!(core::mem::offset_of!(GuestObservation, reserved), 184);
    assert_eq!(
        [
            multi::CPUID_COUNT,
            multi::QUERY_COUNT,
            multi::RESUME_COUNT,
            multi::FAILURE,
            multi::NRIP,
            multi::LAST_PHASE,
            multi::NRIP_CHECKED
        ],
        [0, 1, 2, 3, 4, 5, 6]
    );
    assert_eq!(multi::EXPECTED_EXITS, 65);
    assert_eq!(multi::EXPECTED_EXITS, 2 * multi::ROUNDS + 1);
    assert!(multi::GPR_SENTINELS.into_iter().all(|value| value >> 32 != 0));
}
