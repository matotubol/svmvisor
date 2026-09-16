//! Read-only CPUID policy before designing a native returning adapter.
//!
//! This evaluates supplied observations and executes no instructions. Passing
//! means only that the enumerated CPU features fit the initial contract. It is
//! neither permission to read MSRs nor native entry admission. DXE callbacks do
//! not satisfy firmware_probe's application/CPU-lease/event requirements.
use crate::{
    arch::x86_64::capabilities::{CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures},
    memory::address::EncryptionState,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuidRegisters {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// None means not collected, never an all-zero supported leaf. A collector must
/// query leaf0/80000000 first and honor their maximum supported leaf numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuidEvidence {
    pub basic: CpuidRegisters,
    pub extended: CpuidRegisters,
    pub features: Option<CpuidRegisters>,
    pub extended_features: Option<CpuidRegisters>,
    pub address_width: Option<CpuidRegisters>,
    pub svm: Option<CpuidRegisters>,
}

/// Stable diagnostic values; zero is reserved for CPUID preflight success only.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightError {
    UnsupportedVendor = 1,
    MissingLeaves = 2,
    HypervisorReported = 3,
    MsrUnsupported = 4,
    SvmUnsupported = 5,
    NxUnsupported = 6,
    NptUnsupported = 7,
    UnsupportedRevision = 8,
    InsufficientAsids = 9,
    UnsupportedPhysicalWidth = 10,
    LongModeUnsupported = 11,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuidPreflight {
    capabilities: CapabilityEvidence,
}
impl CpuidPreflight {
    /// VM_CR policy and memory encryption deliberately remain unknown. Calling
    /// validate on this evidence must fail until independent evidence exists.
    pub const fn incomplete_capabilities(self) -> CapabilityEvidence {
        self.capabilities
    }
}

impl CpuidEvidence {
    pub fn evaluate(self) -> Result<CpuidPreflight, PreflightError> {
        use PreflightError::*;
        if [self.basic.ebx, self.basic.edx, self.basic.ecx] != [0x68747541, 0x69746e65, 0x444d4163]
        {
            return Err(UnsupportedVendor);
        }
        if self.basic.eax < 1 || self.extended.eax < 0x8000000a {
            return Err(MissingLeaves);
        }
        let (Some(one), Some(ext), Some(width), Some(svm)) = (
            self.features,
            self.extended_features,
            self.address_width,
            self.svm,
        ) else {
            return Err(MissingLeaves);
        };
        if one.ecx & (1 << 31) != 0 {
            return Err(HypervisorReported);
        }
        if one.edx & (1 << 5) == 0 {
            return Err(MsrUnsupported);
        }
        if ext.ecx & (1 << 2) == 0 {
            return Err(SvmUnsupported);
        }
        if ext.edx & (1 << 20) == 0 {
            return Err(NxUnsupported);
        }
        if ext.edx & (1 << 29) == 0 {
            return Err(LongModeUnsupported);
        }
        if svm.edx & 1 == 0 {
            return Err(NptUnsupported);
        }
        if svm.eax & 0xff != 1 {
            return Err(UnsupportedRevision);
        }
        if svm.ebx < 2 {
            return Err(InsufficientAsids);
        }
        let bits = (width.eax & 0xff) as u8;
        if !(32..=52).contains(&bits) {
            return Err(UnsupportedPhysicalWidth);
        }
        Ok(CpuidPreflight {
            capabilities: CapabilityEvidence {
                vendor: CpuVendor::Amd,
                svm: EvidenceFlag::Set,
                nested_paging: EvidenceFlag::Set,
                svm_revision: Some(1),
                asid_count: Some(svm.ebx),
                physical_address_bits: Some(bits),
                vm_cr_svmdis: EvidenceFlag::Unknown,
                hypervisor_present: EvidenceFlag::Clear,
                encryption: EncryptionState::Unknown,
                optional: OptionalFeatures {
                    nrip_save: svm.edx & (1 << 3) != 0,
                    vmcb_clean: svm.edx & (1 << 5) != 0,
                    flush_by_asid: svm.edx & (1 << 6) != 0,
                    decode_assists: svm.edx & (1 << 7) != 0,
                },
            },
        })
    }
}
