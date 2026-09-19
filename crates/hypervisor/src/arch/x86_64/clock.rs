//! Bounded clock ownership for the diagnostic guest, with no privileged I/O.
//!
//! AMD APM vol.2 rev.3.44 §15.30.5/Figure15-29 defines an 8.32 ratio and
//! Appendix B defines VMCB.TSC_OFFSET. AMD PPR 57896 rev.3.00 pp.88,101,
//! 188–189 supplies CPUID gates and AUX/ratio layouts. Zero ratio is refused
//! by our positive-rate restoration policy, not claimed to be an invalid MSR.

pub const IDENTITY_TSC_RATIO: u64 = 1 << 32;
const RATIO_MASK: u64 = (1 << 40) - 1;

/// Values to install and restore on one exclusively owned CPU. Admission is
/// pure: the runtime must capture host values before modifying them, prohibit
/// migration/re-entry, and restore them before executing ordinary host code.
/// `None` means that the unsupported MSR must not be read or written.
#[derive(Debug, PartialEq, Eq)]
pub struct ClockPlan {
    capabilities: ClockCapabilities,
    host_aux: Option<u64>,
    host_ratio: Option<u64>,
    guest_aux: Option<u64>,
}

impl ClockPlan {
    pub fn admit(
        capabilities: ClockCapabilities,
        host_aux: Option<u64>,
        host_ratio: Option<u64>,
        guest_aux: u32,
    ) -> Result<Self, ClockError> {
        if host_aux.is_some() != capabilities.rdtscp() {
            return Err(ClockError::AuxiliaryEvidenceMismatch);
        }
        if host_ratio.is_some() != capabilities.scaling() {
            return Err(ClockError::RatioEvidenceMismatch);
        }
        if let Some(aux) = host_aux
            && aux >> 32 != 0
        {
            return Err(ClockError::AuxiliaryReservedBits);
        }
        if let Some(ratio) = host_ratio {
            if ratio & !RATIO_MASK != 0 {
                return Err(ClockError::RatioReservedBits);
            }
            if ratio == 0 {
                return Err(ClockError::ZeroHostRatio);
            }
        }
        Ok(Self {
            capabilities,
            host_aux,
            host_ratio,
            guest_aux: if capabilities.rdtscp() { Some(guest_aux as u64) } else { None },
        })
    }
    pub(crate) const fn capabilities(&self) -> ClockCapabilities {
        self.capabilities
    }
    pub const fn host_aux(&self) -> Option<u64> {
        self.host_aux
    }
    pub const fn host_ratio(&self) -> Option<u64> {
        self.host_ratio
    }
    pub const fn guest_aux(&self) -> Option<u64> {
        self.guest_aux
    }
    pub const fn guest_ratio(&self) -> Option<u64> {
        if self.capabilities.scaling() { Some(IDENTITY_TSC_RATIO) } else { None }
    }
    pub const fn tsc_offset(&self) -> u64 {
        0
    }
    /// Compare separately captured post-restoration values, including absence.
    /// Equality checks supplied evidence, not that privileged writes occurred.
    pub fn validate_restored(
        &self,
        aux: Option<u64>,
        ratio: Option<u64>,
    ) -> Result<(), ClockError> {
        if aux != self.host_aux || ratio != self.host_ratio {
            return Err(ClockError::RestorationMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockCapabilities {
    rdtscp: bool,
    scaling: bool,
}

impl ClockCapabilities {
    /// Caller supplies observations from supported CPUID leaves on the admitted
    /// AMD SVM CPU; use zero for an absent optional leaf. This does not replace
    /// the separate SVM/platform admission or establish invariant TSC support.
    pub fn detect(leaf1_edx: u32, extended1_edx: u32, svm_edx: u32) -> Result<Self, ClockError> {
        if leaf1_edx & ((1 << 4) | (1 << 5)) != (1 << 4) | (1 << 5) {
            return Err(ClockError::MissingTscOrMsr);
        }
        Ok(Self { rdtscp: extended1_edx & (1 << 27) != 0, scaling: svm_edx & (1 << 4) != 0 })
    }
    pub const fn rdtscp(self) -> bool {
        self.rdtscp
    }
    pub const fn scaling(self) -> bool {
        self.scaling
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    MissingTscOrMsr,
    AuxiliaryEvidenceMismatch,
    RatioEvidenceMismatch,
    AuxiliaryReservedBits,
    RatioReservedBits,
    ZeroHostRatio,
    RestorationMismatch,
}
