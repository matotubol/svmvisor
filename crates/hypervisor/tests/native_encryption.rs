use svmvisor_hypervisor::{
    arch::x86_64::{
        encryption::{EncryptionError, NativeEncryptionPlan, SEV_STATUS},
        msr::SYS_CFG,
    },
    memory::address::{AddressPolicy, EncryptionState},
};

const TARGET: u32 = 0x00b4_0f40;
fn ryzen(eax: u32, reduction: u32) -> NativeEncryptionPlan {
    NativeEncryptionPlan::new(TARGET, 48, Some([eax, 51 | (reduction << 6), 0, 0])).unwrap()
}

#[test]
fn advertised_sme_requires_controls_but_does_not_reduce_disabled_width() {
    for reduction in 0..=6 {
        let plan = ryzen(0x20_0001, reduction);
        assert_eq!(plan.sys_cfg_msr(), Some(SYS_CFG));
        assert_eq!(plan.sev_status_msr(), None);
        assert_eq!(plan.validate(None, None), Err(EncryptionError::MissingControlEvidence));
        let encryption = plan.validate(Some(0x74_0000), None).unwrap();
        assert_eq!(encryption, EncryptionState::Unencrypted { encryption_bit: Some(51) });
        let policy = AddressPolicy::new(48, encryption).unwrap();
        // Reducing by the advertised count while disabled would reject this.
        assert!(policy.validate(1 << 47, 4096, 4096).is_ok());
        assert!(policy.validate(1 << 51, 4096, 4096).is_err());
    }
}

#[test]
fn every_enabled_memory_mode_and_reserved_control_refuses() {
    let plan = ryzen(1, 5);
    for bit in 23..=26 {
        assert_eq!(
            plan.validate(Some(1 << bit), None),
            Err(EncryptionError::ActiveEncryptionUnsupported)
        );
    }
    for bit in [0, 17, 27, 63] {
        assert_eq!(plan.validate(Some(1 << bit), None), Err(EncryptionError::ReservedControlBits));
    }
}

#[test]
fn sev_status_is_read_only_when_enumerated_and_never_defaulted() {
    let sme = ryzen(1, 5);
    assert_eq!(sme.validate(Some(0), Some(0)), Err(EncryptionError::UnexpectedControlEvidence));
    let sev = ryzen(3, 5);
    assert_eq!(sev.sev_status_msr(), Some(SEV_STATUS));
    assert_eq!(sev.validate(Some(0), None), Err(EncryptionError::MissingControlEvidence));
    assert!(sev.validate(Some(0), Some(0)).is_ok());
    for bit in 0..64 {
        assert_eq!(
            sev.validate(Some(0), Some(1 << bit)),
            Err(EncryptionError::ActiveEncryptionUnsupported)
        );
    }
}

#[test]
fn absent_legacy_leaf_needs_no_speculative_msrs() {
    for leaf in [None, Some([0; 4])] {
        let plan = NativeEncryptionPlan::new(0x800f12, 48, leaf).unwrap();
        assert_eq!(plan.sys_cfg_msr(), None);
        assert_eq!(plan.sev_status_msr(), None);
        assert_eq!(
            plan.validate(None, None),
            Ok(EncryptionState::Unencrypted { encryption_bit: None })
        );
        assert_eq!(plan.validate(Some(0), None), Err(EncryptionError::UnexpectedControlEvidence));
        assert_eq!(
            NativeEncryptionPlan::new(TARGET, 48, leaf),
            Err(EncryptionError::UnsupportedProfile)
        );
    }
}

#[test]
fn malformed_and_unreviewed_profiles_refuse_before_msr_reads() {
    for (signature, width, leaf) in [
        (0x800f12, 48, [1, 51, 0, 0]),
        (TARGET, 47, [1, 51, 0, 0]),
        (TARGET, 48, [0, 51, 0, 0]),
        (TARGET, 48, [1 << 31 | 1, 51, 0, 0]),
        (TARGET, 48, [1, 47, 0, 0]),
        (TARGET, 48, [1, 51 | (7 << 6), 0, 0]),
        (TARGET, 48, [1, 51 | (1 << 16), 0, 0]),
    ] {
        assert_eq!(
            NativeEncryptionPlan::new(signature, width, Some(leaf)),
            Err(EncryptionError::UnsupportedProfile)
        );
    }
    for width in [0, 31, 53, 255] {
        assert_eq!(
            NativeEncryptionPlan::new(0, width, None),
            Err(EncryptionError::UnsupportedPhysicalWidth)
        );
    }
}
