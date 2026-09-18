//! AVIC exit decoding, APM2 3.44 15.29.9 (Tables 15-25..29) and Table C-1.
//!
//! Reserved EXITINFO bits are ignored rather than checked: "Software must not
//! depend on the state of a reserved field (unless qualified as RAZ)" (APM2
//! rev3.44 Definitions, p.lvi). These fields are not qualified as RAZ.

use crate::{arch::x86_64::apic, svm::x2avic::Error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvicExit {
    /// AVIC_INCOMPLETE_IPI (401h). Table 15-25: EXITINFO1 bits 63:32 and 31:0
    /// are the values written to ICRH and ICRL. Table 15-26/15-27: `reason`
    /// is the ID (EXITINFO2 bits 63:32); `index` (bits 11:0) is defined only
    /// for IDs 1-3.
    IncompleteIpi { icr: u64, reason: u32, index: Option<u16> },
    /// AVIC_NOACCEL (402h). Table 15-28: R/W is bit 32 and APIC_Offset[11:4]
    /// is bits 11:4, with APIC_Offset[3:0] = 0. Table 15-29: for a write to
    /// EOI (offset B0h), EXITINFO2[7:0] is the highest in-service vector;
    /// otherwise EXITINFO2 is undefined and not decoded.
    NoAcceleration { offset: u16, write: bool, eoi_vector: Option<u8> },
}
impl AvicExit {
    /// Decode only, without inferring completion or replaying partial IPIs.
    /// ID 5 (Secure AVIC only) and reserved IDs above 5 are refused here.
    pub fn decode(code: u64, info1: u64, info2: u64) -> Result<Self, Error> {
        match code {
            0x401 => {
                let reason = (info2 >> 32) as u32;
                if reason > 4 {
                    return Err(Error::InvalidExit);
                }
                let index = (1..=3).contains(&reason).then_some((info2 & 0xfff) as u16);
                Ok(Self::IncompleteIpi { icr: info1, reason, index })
            }
            0x402 => {
                let offset = (info1 & 0xff0) as u16;
                let write = info1 & (1 << 32) != 0;
                let eoi_vector = if write && offset == apic::EOI {
                    // ISR bits 15:0 are reserved (16.6.3 p647): no in-service
                    // vector is below 16.
                    let vector = info2 as u8;
                    if vector < 16 {
                        return Err(Error::InvalidExit);
                    }
                    Some(vector)
                } else {
                    None
                };
                Ok(Self::NoAcceleration { offset, write, eoi_vector })
            }
            _ => Err(Error::InvalidExit),
        }
    }
}
