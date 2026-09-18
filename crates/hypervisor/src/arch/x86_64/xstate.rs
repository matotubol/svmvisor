//! Bounded, standard-format eager extended-state policy for the synthetic host.
//!
//! AMD APM vol. 2 rev. 3.44 sections 11.3, 11.4.4.3 and 11.5 define the
//! enablement, legacy image, XCR0 dependencies and CPUID-enumerated layout.
//! This terminal host deliberately enables only x87/MMX, SSE and optional AVX.
//! It does not preserve a firmware/OS XCR0 profile or supervisor extended state.
//! Callers must disable EFER.FFXSR, clear CR0.EM/TS, enable CR4.OSFXSR and
//! OSXMMEXCPT, and (for XSAVE) enable OSXSAVE and install the selected XCR0.

pub const XSTATE_AREA_BYTES: usize = 4096;
pub const LEGACY_BYTES: usize = 512;
pub const XSAVE_HEADER_OFFSET: usize = 512;
pub const XSAVE_BASE_BYTES: usize = 576;
pub const MXCSR_INITIAL: u32 = 0x1f80;
pub const MXCSR_DEFAULT_MASK: u32 = 0xffbf;

/// Raw CPUID observations. `enabled_size` is leaf D.0 EBX *before* selecting
/// XCR0; call `validate_enabled_size` with a fresh observation afterwards.
#[derive(Clone, Copy, Debug, Default)]
pub struct XstateCapabilities {
    pub leaf1_ecx: u32,
    pub leaf1_edx: u32,
    pub supported_xcr0: u64,
    pub enabled_size: u32,
    pub max_size: u32,
    pub avx_size: u32,
    pub avx_offset: u32,
    pub avx_flags: u32,
}

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

/// Architectural fault for an intercepted, otherwise validly decoded XSETBV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XsetbvFault {
    UndefinedOpcode,
    GeneralProtection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XstateLayout {
    mask: u64,
    size: usize,
    avx_offset: Option<usize>,
    avx_flags: u32,
    xsave: bool,
}

impl XstateLayout {
    pub fn detect(caps: XstateCapabilities) -> Result<Self, XstateError> {
        // FPU, MMX, FXSR, SSE and SSE2 are required by the synthetic profile.
        const LEGACY: u32 = 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26);
        if caps.leaf1_edx & LEGACY != LEGACY {
            return Err(XstateError::MissingLegacyFeatures);
        }
        if caps.leaf1_ecx & (1 << 26) == 0 {
            return Ok(Self {
                mask: 3,
                size: LEGACY_BYTES,
                avx_offset: None,
                avx_flags: 0,
                xsave: false,
            });
        }
        if caps.supported_xcr0 & 3 != 3 {
            return Err(XstateError::UnsupportedMask);
        }
        let mut layout =
            Self { mask: 3, size: XSAVE_BASE_BYTES, avx_offset: None, avx_flags: 0, xsave: true };
        if caps.leaf1_ecx & (1 << 28) != 0 {
            if caps.supported_xcr0 & 4 == 0 {
                return Err(XstateError::UnsupportedMask);
            }
            // Sixteen upper YMM halves occupy exactly 256 bytes. Standard
            // layout offsets must not overlap legacy state/header. ECX bit0
            // marks supervisor state and is forbidden for XCR0 component2.
            if caps.avx_size != 256
                || caps.avx_offset < XSAVE_BASE_BYTES as u32
                || caps.avx_flags & !2 != 0
            {
                return Err(XstateError::InvalidLayout);
            }
            layout.size =
                caps.avx_offset.checked_add(caps.avx_size).ok_or(XstateError::InvalidLayout)?
                    as usize;
            layout.mask = 7;
            layout.avx_offset = Some(caps.avx_offset as usize);
            layout.avx_flags = caps.avx_flags;
        }
        if layout.size > XSTATE_AREA_BYTES {
            return Err(XstateError::AreaTooSmall);
        }
        // Other available components may require larger allocations; they are
        // deliberately not enabled and must not disqualify a supported CPU.
        if caps.max_size < layout.size as u32 {
            return Err(XstateError::InvalidLayout);
        }
        Ok(layout)
    }

    pub const fn mask(self) -> u64 {
        self.mask
    }
    /// Fixed synthetic guest: PAE, OSFXSR, OSXMMEXCPT, optional OSXSAVE.
    pub const fn guest_cr4(self) -> u64 {
        0x620 | if self.xsave { 1 << 18 } else { 0 }
    }
    pub const fn size(self) -> usize {
        self.size
    }
    pub const fn uses_xsave(self) -> bool {
        self.xsave
    }
    pub const fn avx_offset(self) -> Option<usize> {
        self.avx_offset
    }
    /// CPUID D.2 ECX layout attributes retained from the admitted host.
    /// Bit 1 requests 64-byte alignment in compacted format; component 2 is
    /// never supervisor state (bit 0 was rejected during layout admission).
    pub const fn avx_flags(self) -> u32 {
        self.avx_flags
    }

    /// Validate the terminal host's full preservation mask. Guest XCR0 can be
    /// a subset only with a boundary that owns it separately; see
    /// `validate_guest_xcr0`.
    pub fn validate_xcr0(self, mask: u64) -> Result<(), XstateError> {
        if mask != self.mask {
            return Err(XstateError::UnsupportedMask);
        }
        Ok(())
    }

    /// Check an intercepted XSETBV against this admitted x87/SSE/AVX model.
    /// AMD APM vol.2 rev.3.44 3.1.3 (OSXSAVE), 11.5.2 and 11.5.5:
    /// absent XSAVE/OSXSAVE causes #UD; nonzero CPL/index, unsupported bits,
    /// cleared x87 or AVX without SSE causes #GP(0). The 32-bit ECX operand
    /// selects XCR0; EDX:EAX must be combined without discarding high bits.
    /// This pure check never changes XCR0, saved components, RIP or flags.
    /// Prefix/decode faults and event delivery remain the exit owner's job.
    pub fn validate_guest_xcr0(
        self,
        cr4: u64,
        cpl: u8,
        ecx: u32,
        value: u64,
    ) -> Result<(), XsetbvFault> {
        if !self.xsave || cr4 & (1 << 18) == 0 {
            return Err(XsetbvFault::UndefinedOpcode);
        }
        if cpl != 0
            || ecx != 0
            || value & !self.mask != 0
            || value & 1 == 0
            || (value & 4 != 0 && value & 2 == 0)
        {
            return Err(XsetbvFault::GeneralProtection);
        }
        Ok(())
    }

    pub fn validate_enabled_size(self, size: u32) -> Result<(), XstateError> {
        if self.xsave && size as usize != self.size {
            return Err(XstateError::EnabledSizeMismatch);
        }
        Ok(())
    }
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

/// Owned storage: callers retain exclusive access while assembly/hardware uses
/// the pointer. Separate areas are required for host, guest and verification.
#[repr(C, align(64))]
pub struct XstateArea {
    bytes: [u8; XSTATE_AREA_BYTES],
}

impl Default for XstateArea {
    fn default() -> Self {
        Self::new()
    }
}

impl XstateArea {
    pub const fn new() -> Self {
        Self { bytes: [0; XSTATE_AREA_BYTES] }
    }
    pub fn as_ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr()
    }
    pub fn bytes(&self) -> &[u8; XSTATE_AREA_BYTES] {
        &self.bytes
    }
    /// Fixture construction only; validate before handing a modified image to XRSTOR.
    pub fn bytes_mut(&mut self) -> &mut [u8; XSTATE_AREA_BYTES] {
        &mut self.bytes
    }

    pub fn reset(&mut self, layout: XstateLayout, mxcsr_mask: u32) -> Result<(), XstateError> {
        let mask = effective_mxcsr_mask(mxcsr_mask)?;
        self.bytes.fill(0);
        self.bytes[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
        self.bytes[24..28].copy_from_slice(&MXCSR_INITIAL.to_le_bytes());
        self.bytes[28..32].copy_from_slice(&mask.to_le_bytes());
        if layout.xsave {
            // Materialize every selected component, including zero payloads.
            // This also permits sentinels to be written into a reset image.
            self.bytes[512..520].copy_from_slice(&layout.mask.to_le_bytes());
        }
        Ok(())
    }

    pub fn validate(&self, layout: XstateLayout, mxcsr_mask: u32) -> Result<(), XstateError> {
        let mask = effective_mxcsr_mask(mxcsr_mask)?;
        let mxcsr = u32::from_le_bytes(self.bytes[24..28].try_into().unwrap());
        if mxcsr & !mask != 0 {
            return Err(XstateError::InvalidMxcsr);
        }
        if layout.xsave {
            let present = u64::from_le_bytes(self.bytes[512..520].try_into().unwrap());
            if present & !layout.mask != 0 || self.bytes[520..576].iter().any(|&b| b != 0) {
                return Err(XstateError::InvalidHeader);
            }
        }
        Ok(())
    }
}

const _: () = assert!(core::mem::size_of::<XstateArea>() == XSTATE_AREA_BYTES);
const _: () = assert!(core::mem::align_of::<XstateArea>() == 64);
