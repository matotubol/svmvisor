//! MXCSR mask admission for a captured legacy extended-state image, and the
//! extended-state error type. AMD APM vol. 2 rev. 3.44 section 11.5.10.

pub const MXCSR_INITIAL: u32 = 0x1f80;
pub const MXCSR_DEFAULT_MASK: u32 = 0xffbf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XstateError {
    MissingLegacyFeatures,
    UnsupportedMask,
    InvalidLayout,
    AreaTooSmall,
    EnabledSizeMismatch,
    InvalidMxcsrMask,
    InvalidMxcsr,
    InvalidHeader,
}

/// A zero hardware MXCSR_MASK denotes the architectural fallback 0000FFBFh.
/// AMD APM1 rev3.24 4.2.2 includes MM (bit17); bit16 and bits31:18 remain
/// reserved. APM2 rev3.44 11.5.10 (printed p363): a nonzero hardware mask
/// identifies supported bits, independently of their current values. The caller
/// supplies its actual processor's observed mask, not an invented capability.
pub fn effective_mxcsr_mask(observed: u32) -> Result<u32, XstateError> {
    let mask = if observed == 0 { MXCSR_DEFAULT_MASK } else { observed };
    if mask & !0x2ffff != 0 || mask & MXCSR_INITIAL != MXCSR_INITIAL {
        return Err(XstateError::InvalidMxcsrMask);
    }
    Ok(mask)
}
