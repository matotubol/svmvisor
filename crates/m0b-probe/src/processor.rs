//! Host-testable model for the cardinality-bounded per-processor inventory
//! slice.

use core::time::Duration;

use crate::cpuid::{CpuInventory, capability_cpuid_equal, cpuid_identity_matches_processor_id};
use crate::evidence::VmCrEvidence;
use crate::msr::SystemRegistersEvidence;

/// The single reviewed wait policy for one AP measurement dispatch.
///
/// F7's finite-timeout recovery can reset an AP with INIT/SIPI. Keeping the raw
/// UEFI argument and its evidence value behind this one policy prevents them
/// from drifting independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApMeasurementTimeoutPolicy {
    NoMpServicesTimeout,
}

impl ApMeasurementTimeoutPolicy {
    #[must_use]
    pub const fn timeout_microseconds(self) -> u64 {
        match self {
            Self::NoMpServicesTimeout => 0,
        }
    }

    #[must_use]
    pub const fn uefi_timeout(self) -> Option<Duration> {
        match self {
            Self::NoMpServicesTimeout => None,
        }
    }
}

pub const AP_MEASUREMENT_TIMEOUT_POLICY: ApMeasurementTimeoutPolicy =
    ApMeasurementTimeoutPolicy::NoMpServicesTimeout;

/// How one processor reached the read-only measurement callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessorDispatch {
    /// The BSP executed the same measurement body directly.
    BspDirect,
    /// A blocking, sequential UEFI `StartupThisAP` call returned success.
    StartupThisApSuccess,
}

/// CPUID, conditional VM_CR, and allowlisted system-register observation made
/// on one enabled processor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessorObservation {
    pub processor_number: usize,
    pub processor_id: u64,
    /// Actual processor number returned by `WhoAmI` in the measurement path.
    pub who_am_i_processor_number: usize,
    pub is_bsp: bool,
    pub dispatch: ProcessorDispatch,
    pub cpu: CpuInventory,
    pub vm_cr: VmCrEvidence<'static>,
    pub system_registers: SystemRegistersEvidence<'static>,
}

impl ProcessorObservation {
    #[must_use]
    pub fn identity_matches_mp_services(&self) -> bool {
        cpuid_identity_matches_processor_id(&self.cpu, self.processor_id)
    }

    #[must_use]
    pub fn cpuid_matches(&self, reference: &Self) -> bool {
        capability_cpuid_equal(&reference.cpu, &self.cpu)
    }

    #[must_use]
    pub fn vm_cr_matches(&self, reference: &Self) -> bool {
        self.vm_cr == reference.vm_cr
    }

    #[must_use]
    pub fn system_registers_matches(&self, reference: &Self) -> bool {
        self.system_registers
            .matches_cross_processor_policy(&reference.system_registers)
    }
}

/// Borrowed complete result for every enabled, healthy processor between
/// matching pre- and post-dispatch MP Services enumerations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessorConsistencyEvidence<'a> {
    pub bsp_processor_number: usize,
    /// Raw UEFI timeout value; zero means an infinite MP Services wait.
    pub timeout_microseconds_per_ap: u64,
    pub observations: &'a [ProcessorObservation],
}

impl ProcessorConsistencyEvidence<'_> {
    #[must_use]
    pub fn bsp_observation(&self) -> Option<&ProcessorObservation> {
        self.observations.iter().find(|observation| {
            observation.is_bsp && observation.processor_number == self.bsp_processor_number
        })
    }

    #[must_use]
    pub fn identity_consistent(&self) -> bool {
        self.observations
            .iter()
            .all(ProcessorObservation::identity_matches_mp_services)
    }

    #[must_use]
    pub fn cpuid_consistent(&self) -> bool {
        let Some(reference) = self.bsp_observation() else {
            return false;
        };
        self.observations
            .iter()
            .all(|observation| observation.cpuid_matches(reference))
    }

    #[must_use]
    pub fn vm_cr_consistent(&self) -> bool {
        let Some(reference) = self.bsp_observation() else {
            return false;
        };
        self.observations
            .iter()
            .all(|observation| observation.vm_cr_matches(reference))
    }

    #[must_use]
    pub fn system_registers_consistent(&self) -> bool {
        let Some(reference) = self.bsp_observation() else {
            return false;
        };
        self.observations
            .iter()
            .all(|observation| observation.system_registers_matches(reference))
    }

    #[must_use]
    pub fn consistent(&self) -> bool {
        self.identity_consistent()
            && self.cpuid_consistent()
            && self.vm_cr_consistent()
            && self.system_registers_consistent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_timeout_policy_has_one_uefi_and_evidence_encoding() {
        assert_eq!(AP_MEASUREMENT_TIMEOUT_POLICY.timeout_microseconds(), 0);
        assert_eq!(AP_MEASUREMENT_TIMEOUT_POLICY.uefi_timeout(), None);
    }
    use crate::cpuid::CpuidRegisters;
    use crate::msr::{
        FIXED_MTRR_COUNT, IORR_RANGE_COUNT, MAX_VARIABLE_MTRR_PAIRS, SystemRegisterInventory,
        SystemRegistersEvidence, VariableMtrrPair,
    };

    const SYS_REGS_NOT_ATTEMPTED: SystemRegistersEvidence<'static> =
        SystemRegistersEvidence::NotAttempted {
            reason: "cpu-is-not-authentic-amd-or-svm-is-not-enumerated",
        };

    fn system_registers(smm_base: u64, hwcr: u64) -> SystemRegistersEvidence<'static> {
        SystemRegistersEvidence::Observed(SystemRegisterInventory {
            mtrr_cap: 0,
            mtrr_def_type: 0,
            pat: 0,
            variable_mtrr_pairs: [VariableMtrrPair { base: 0, mask: 0 }; MAX_VARIABLE_MTRR_PAIRS],
            variable_mtrr_pair_count: 0,
            fixed_mtrr: [0; FIXED_MTRR_COUNT],
            fixed_mtrr_observed: false,
            sys_cfg: 0,
            hwcr,
            top_mem: 0,
            tom2: 0,
            smm_base,
            smm_addr: 0,
            smm_mask: 0,
            iorr_base: [0; IORR_RANGE_COUNT],
            iorr_mask: [0; IORR_RANGE_COUNT],
            iorr_not_attempted_reason: Some("cpu-family-model-not-documented-by-pinned-ppr"),
        })
    }

    fn cpu(apic_id: u32, feature_bits: u32) -> CpuInventory {
        CpuInventory {
            leaf_0000_0000: CpuidRegisters {
                eax: 7,
                ebx: u32::from_le_bytes(*b"Auth"),
                edx: u32::from_le_bytes(*b"enti"),
                ecx: u32::from_le_bytes(*b"cAMD"),
            },
            leaf_0000_0001: Some(CpuidRegisters {
                ebx: apic_id << 24,
                ..Default::default()
            }),
            leaf_0000_0007_subleaf_0: Some(CpuidRegisters::default()),
            leaf_8000_0000: CpuidRegisters {
                eax: 0x8000_001e,
                ..Default::default()
            },
            leaf_8000_0001: Some(CpuidRegisters {
                ecx: (1 << 2) | (1 << 22),
                ..Default::default()
            }),
            leaf_8000_0002_to_0004: Some([CpuidRegisters::default(); 3]),
            leaf_8000_0008: Some(CpuidRegisters {
                eax: 52,
                ..Default::default()
            }),
            leaf_8000_000a: Some(CpuidRegisters {
                edx: feature_bits,
                ..Default::default()
            }),
            leaf_8000_001e: Some(CpuidRegisters {
                eax: apic_id,
                ebx: 0x0000_0100 | apic_id,
                ecx: apic_id,
                edx: 0,
            }),
            leaf_8000_001f: None,
            brand: Some([0; 48]),
        }
    }

    #[test]
    fn consistency_accepts_identity_differences_but_not_capability_differences() {
        let mut observations = [
            ProcessorObservation {
                processor_number: 0,
                processor_id: 0,
                who_am_i_processor_number: 0,
                is_bsp: true,
                dispatch: ProcessorDispatch::BspDirect,
                cpu: cpu(0, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: SYS_REGS_NOT_ATTEMPTED,
            },
            ProcessorObservation {
                processor_number: 1,
                processor_id: 1,
                who_am_i_processor_number: 1,
                is_bsp: false,
                dispatch: ProcessorDispatch::StartupThisApSuccess,
                cpu: cpu(1, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: SYS_REGS_NOT_ATTEMPTED,
            },
        ];
        let evidence = ProcessorConsistencyEvidence {
            bsp_processor_number: 0,
            timeout_microseconds_per_ap: 0,
            observations: &observations,
        };
        assert!(evidence.consistent());

        observations[1].cpu.leaf_8000_000a.as_mut().unwrap().edx = 3;
        let evidence = ProcessorConsistencyEvidence {
            bsp_processor_number: 0,
            timeout_microseconds_per_ap: 0,
            observations: &observations,
        };
        assert!(!evidence.cpuid_consistent());
        assert!(!evidence.consistent());
    }

    #[test]
    fn vm_cr_and_identity_mismatches_are_independent() {
        let observations = [
            ProcessorObservation {
                processor_number: 0,
                processor_id: 0,
                who_am_i_processor_number: 0,
                is_bsp: true,
                dispatch: ProcessorDispatch::BspDirect,
                cpu: cpu(0, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: SYS_REGS_NOT_ATTEMPTED,
            },
            ProcessorObservation {
                processor_number: 1,
                processor_id: 2,
                who_am_i_processor_number: 1,
                is_bsp: false,
                dispatch: ProcessorDispatch::StartupThisApSuccess,
                cpu: cpu(1, 1),
                vm_cr: VmCrEvidence::Observed(0),
                system_registers: SYS_REGS_NOT_ATTEMPTED,
            },
        ];
        let evidence = ProcessorConsistencyEvidence {
            bsp_processor_number: 0,
            timeout_microseconds_per_ap: 0,
            observations: &observations,
        };
        assert!(!evidence.identity_consistent());
        assert!(evidence.cpuid_consistent());
        assert!(!evidence.vm_cr_consistent());
        assert!(evidence.system_registers_consistent());
        assert!(!evidence.consistent());
    }

    #[test]
    fn system_register_mismatch_independently_breaks_consistency() {
        let observations = [
            ProcessorObservation {
                processor_number: 0,
                processor_id: 0,
                who_am_i_processor_number: 0,
                is_bsp: true,
                dispatch: ProcessorDispatch::BspDirect,
                cpu: cpu(0, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: SYS_REGS_NOT_ATTEMPTED,
            },
            ProcessorObservation {
                processor_number: 1,
                processor_id: 1,
                who_am_i_processor_number: 1,
                is_bsp: false,
                dispatch: ProcessorDispatch::StartupThisApSuccess,
                cpu: cpu(1, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: SystemRegistersEvidence::NotAttempted {
                    reason: "cpu-family-model-not-documented-by-pinned-ppr",
                },
            },
        ];
        let evidence = ProcessorConsistencyEvidence {
            bsp_processor_number: 0,
            timeout_microseconds_per_ap: 0,
            observations: &observations,
        };
        assert!(evidence.identity_consistent());
        assert!(evidence.cpuid_consistent());
        assert!(evidence.vm_cr_consistent());
        assert!(!evidence.system_registers_consistent());
        assert!(!evidence.consistent());
    }

    #[test]
    fn thread_scoped_smm_base_difference_preserves_consistency() {
        let observations = [
            ProcessorObservation {
                processor_number: 0,
                processor_id: 0,
                who_am_i_processor_number: 0,
                is_bsp: true,
                dispatch: ProcessorDispatch::BspDirect,
                cpu: cpu(0, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: system_registers(0x0003_0000, 1),
            },
            ProcessorObservation {
                processor_number: 1,
                processor_id: 1,
                who_am_i_processor_number: 1,
                is_bsp: false,
                dispatch: ProcessorDispatch::StartupThisApSuccess,
                cpu: cpu(1, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: system_registers(0x0005_0000, 1),
            },
        ];
        let evidence = ProcessorConsistencyEvidence {
            bsp_processor_number: 0,
            timeout_microseconds_per_ap: 0,
            observations: &observations,
        };

        assert!(observations[1].system_registers_matches(&observations[0]));
        assert!(evidence.system_registers_consistent());
        assert!(evidence.consistent());
    }

    #[test]
    fn hwcr_difference_remains_a_system_register_mismatch() {
        let observations = [
            ProcessorObservation {
                processor_number: 0,
                processor_id: 0,
                who_am_i_processor_number: 0,
                is_bsp: true,
                dispatch: ProcessorDispatch::BspDirect,
                cpu: cpu(0, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: system_registers(0x0003_0000, 1),
            },
            ProcessorObservation {
                processor_number: 1,
                processor_id: 1,
                who_am_i_processor_number: 1,
                is_bsp: false,
                dispatch: ProcessorDispatch::StartupThisApSuccess,
                cpu: cpu(1, 1),
                vm_cr: VmCrEvidence::Observed(8),
                system_registers: system_registers(0x0005_0000, 0),
            },
        ];
        let evidence = ProcessorConsistencyEvidence {
            bsp_processor_number: 0,
            timeout_microseconds_per_ap: 0,
            observations: &observations,
        };

        assert!(!observations[1].system_registers_matches(&observations[0]));
        assert!(!evidence.system_registers_consistent());
        assert!(!evidence.consistent());
    }
}
