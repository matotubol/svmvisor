//! CPUID-only collection for the opt-in DXE image-entry adapter.
//! No result authorizes SVM. Image entry remains a boot-service-driver context,
//! not the firmware-application context required by firmware_probe today.
use svmvisor_hypervisor::{
    boot::preflight::{CpuidEvidence, CpuidRegisters, PreflightError},
    svm::cpu_model::{CpuIdentity, CpuIdentityError},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    CpuidRejected(PreflightError),
    /// CPU enumeration passed, but no native capture/restore entry exists.
    NativeBoundaryUnavailable,
}
impl Outcome {
    pub const fn diagnostic_code(self) -> u32 {
        match self {
            Self::CpuidRejected(error) => error as u32,
            Self::NativeBoundaryUnavailable => 0x100,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub evidence: CpuidEvidence,
    pub outcome: Outcome,
    /// Read-only BSP boot identity, independent of SVM admission. Failure is
    /// retained explicitly; missing leaves never become fabricated zero data.
    pub identity: Result<CpuIdentity, CpuIdentityError>,
}

/// At most nine CPUID calls, with optional leaves gated by reported maxima and
/// no privileged state reads. The callback contract is CPUID(leaf, subleaf=0)
/// only. Native image entry uses the BSP's actual instructions before any guest
/// entry; this does not enumerate AP identities or install a native guest model.
/// AMD PPR 57896 rev.3.00 section2.1.12 defines vendor, signature and brand leaves.
pub fn collect(mut cpuid: impl FnMut(u32) -> CpuidRegisters) -> Report {
    let basic = cpuid(0);
    let extended = cpuid(0x80000000);
    let evidence = CpuidEvidence {
        basic,
        extended,
        features: (basic.eax >= 1).then(|| cpuid(1)),
        extended_features: (extended.eax >= 0x80000001).then(|| cpuid(0x80000001)),
        address_width: (extended.eax >= 0x80000008).then(|| cpuid(0x80000008)),
        svm: (extended.eax >= 0x8000000a).then(|| cpuid(0x8000000a)),
    };
    let outcome = match evidence.evaluate() {
        Ok(_) => Outcome::NativeBoundaryUnavailable,
        Err(error) => Outcome::CpuidRejected(error),
    };
    let registers = |r: CpuidRegisters| [r.eax, r.ebx, r.ecx, r.edx];
    let brand = (0x80000004..=0xbfffffff).contains(&extended.eax).then(|| {
        [registers(cpuid(0x80000002)), registers(cpuid(0x80000003)), registers(cpuid(0x80000004))]
    });
    let identity = CpuIdentity::from_leaves(
        registers(basic),
        evidence.features.map(registers),
        registers(extended),
        evidence.extended_features.map(registers),
        brand,
    );
    Report { evidence, outcome, identity }
}
