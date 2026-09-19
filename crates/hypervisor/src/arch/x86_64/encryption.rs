//! Native unencrypted-address admission, shared by DXE and resident capture.
//!
//! AMD APM2 rev. 3.44 (March 2026), sections 7.10.1-2, 7.10.9 and
//! 15.34.10 distinguish advertised capabilities from enabled modes. Nonzero
//! encryption leaves are admitted only for PPR 57896 rev. 3.00 (28 Aug 2024),
//! Family 1Ah Model 44h B0 / CPUID 00B40F40: CPUID 8000001F pp.111-112 and
//! SYS_CFG pp.202. Other product register profiles require their own review.

use crate::{
    arch::x86_64::msr::{
        SEV_STATUS, SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, TARGET_PHYSICAL_BITS,
        TARGET_SIGNATURE,
    },
    memory::address::EncryptionState,
};

/// A pure CPUID check authorizing only the MSR observations named by this plan.
/// Callers must first establish native AMD CPL0 execution without a hypervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEncryptionPlan {
    encryption_bit: Option<u8>,
    sev_supported: bool,
}

impl NativeEncryptionPlan {
    /// `leaf` is absent only when the maximum extended leaf is below 8000001F.
    /// The supplied width is CPUID 80000008.EAX[7:0], before any reduction.
    pub fn new(
        signature: u32,
        physical_bits: u8,
        leaf: Option<[u32; 4]>,
    ) -> Result<Self, EncryptionError> {
        if !(32..=52).contains(&physical_bits) {
            return Err(EncryptionError::UnsupportedPhysicalWidth);
        }
        let [eax, ebx, ecx, edx] = leaf.unwrap_or([0; 4]);
        if eax | ebx | ecx | edx == 0 {
            // PPR's target always enumerates SME and C-bit 51. An absent leaf
            // on that signature is contradictory evidence, not disabled SME.
            if signature == TARGET_SIGNATURE {
                return Err(EncryptionError::UnsupportedProfile);
            }
            return Ok(Self { encryption_bit: None, sev_supported: false });
        }
        if signature != TARGET_SIGNATURE
            || physical_bits != TARGET_PHYSICAL_BITS
            || eax & 1 == 0
            || eax & !0x41ff_ffff != 0
            || ebx & !0xffff != 0
            || ebx & 63 != 51
            || (ebx >> 6) & 63 > 6
        {
            return Err(EncryptionError::UnsupportedProfile);
        }
        Ok(Self { encryption_bit: Some(51), sev_supported: eax & 2 != 0 })
    }

    pub const fn sys_cfg_msr(self) -> Option<u32> {
        if self.encryption_bit.is_some() { Some(SYS_CFG) } else { None }
    }

    /// APM2 15.34.10: SEV_STATUS exists only when CPUID advertises SEV.
    pub const fn sev_status_msr(self) -> Option<u32> {
        if self.sev_supported { Some(SEV_STATUS) } else { None }
    }

    /// Accept only controls actually observed on the CPU being admitted.
    /// No register is modified. Width reduction applies only to enabled modes;
    /// those modes are refused, so callers retain the original physical width.
    pub fn validate(
        self,
        sys_cfg: Option<u64>,
        sev_status: Option<u64>,
    ) -> Result<EncryptionState, EncryptionError> {
        if self.sys_cfg_msr().is_some() && sys_cfg.is_none()
            || self.sev_supported && sev_status.is_none()
        {
            return Err(EncryptionError::MissingControlEvidence);
        }
        if self.sys_cfg_msr().is_none() && sys_cfg.is_some()
            || !self.sev_supported && sev_status.is_some()
        {
            return Err(EncryptionError::UnexpectedControlEvidence);
        }
        if let Some(value) = sys_cfg {
            if value & !SYS_CFG_DEFINED != 0 {
                return Err(EncryptionError::ReservedControlBits);
            }
            if value & SYS_CFG_ENCRYPTION != 0 {
                return Err(EncryptionError::ActiveEncryptionUnsupported);
            }
        }
        if sev_status.is_some_and(|value| value != 0) {
            return Err(EncryptionError::ActiveEncryptionUnsupported);
        }
        Ok(EncryptionState::Unencrypted { encryption_bit: self.encryption_bit })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptionError {
    UnsupportedPhysicalWidth,
    UnsupportedProfile,
    MissingControlEvidence,
    UnexpectedControlEvidence,
    ReservedControlBits,
    ActiveEncryptionUnsupported,
}
