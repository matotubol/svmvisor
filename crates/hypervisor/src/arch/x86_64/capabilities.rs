//! Validate caller-supplied evidence for the initial classic-SVM/NPT contract.
//! No CPUID/MSR reads occur here. Acceptance is not launch readiness: ownership,
//! recovery, SMM, mappings, per-CPU state and memory attributes remain unproven.
//! See pinned AMD APM vol. 2 rev. 3.44 sections 15.4, 15.5, 15.16 and 15.25.

use crate::memory::address::{AddressError, AddressPolicy, EncryptionState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuVendor {
    Amd,
    Other,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceFlag {
    Unknown,
    Clear,
    Set,
}

/// Optional accelerators: absence is accepted, not silently enabled. Any later
/// code path relying on one must check its corresponding flag first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptionalFeatures {
    pub nrip_save: bool,
    pub decode_assists: bool,
    pub vmcb_clean: bool,
    pub flush_by_asid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityEvidence {
    pub vendor: CpuVendor,
    pub svm: EvidenceFlag,
    pub nested_paging: EvidenceFlag,
    /// Decoded SVM revision; missing evidence is distinct from revision zero.
    pub svm_revision: Option<u8>,
    /// CPUID-reported count, including reserved host ASID zero.
    pub asid_count: Option<u32>,
    pub physical_address_bits: Option<u8>,
    pub vm_cr_svmdis: EvidenceFlag,
    /// Caller evidence of an existing hypervisor, not a claim of detectability.
    pub hypervisor_present: EvidenceFlag,
    pub encryption: EncryptionState,
    pub optional: OptionalFeatures,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    UnsupportedVendor,
    SvmNotEstablished,
    NestedPagingNotEstablished,
    UnsupportedRevision,
    InsufficientAsids,
    MissingPhysicalWidth,
    SvmDisabledOrUnknown,
    HypervisorPresentOrUnknown,
    Address(AddressError),
    InvalidAsid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedCapabilities {
    svm_revision: u8,
    asid_count: u32,
    address_policy: AddressPolicy,
    optional: OptionalFeatures,
}

impl CapabilityEvidence {
    /// Require AMD, SVM, NPT, revision 1, at least guest ASID 1, known enabled
    /// SVM policy and no reported existing hypervisor. Revision 1 is this
    /// implementation's reviewed contract; future revisions require review.
    pub fn validate(self) -> Result<ValidatedCapabilities, CapabilityError> {
        use CapabilityError::*;
        if self.vendor != CpuVendor::Amd {
            return Err(UnsupportedVendor);
        }
        if self.svm != EvidenceFlag::Set {
            return Err(SvmNotEstablished);
        }
        if self.nested_paging != EvidenceFlag::Set {
            return Err(NestedPagingNotEstablished);
        }
        let svm_revision = match self.svm_revision {
            Some(1) => 1,
            _ => return Err(UnsupportedRevision),
        };
        let asid_count = match self.asid_count {
            Some(count) if count >= 2 => count,
            _ => return Err(InsufficientAsids),
        };
        if self.vm_cr_svmdis != EvidenceFlag::Clear {
            return Err(SvmDisabledOrUnknown);
        }
        if self.hypervisor_present != EvidenceFlag::Clear {
            return Err(HypervisorPresentOrUnknown);
        }
        let bits = self.physical_address_bits.ok_or(MissingPhysicalWidth)?;
        let address_policy = AddressPolicy::new(bits, self.encryption).map_err(Address)?;
        Ok(ValidatedCapabilities {
            svm_revision,
            asid_count,
            address_policy,
            optional: self.optional,
        })
    }
}

impl ValidatedCapabilities {
    pub const fn svm_revision(self) -> u8 {
        self.svm_revision
    }
    pub const fn asid_count(self) -> u32 {
        self.asid_count
    }
    pub const fn address_policy(self) -> AddressPolicy {
        self.address_policy
    }
    pub const fn optional_features(self) -> OptionalFeatures {
        self.optional
    }
    pub fn validate_asid(self, asid: u32) -> Result<(), CapabilityError> {
        if asid == 0 || asid >= self.asid_count {
            Err(CapabilityError::InvalidAsid)
        } else {
            Ok(())
        }
    }
}
