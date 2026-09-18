//! Policy for preserving a firmware caller's original extended-state profile.
//!
//! AMD APM vol.2 rev.3.44 §§2.5.12,11.3,11.4.4.3,11.5 and18.13 specify
//! enablement, FFXSR, XCR0 and standard save images. Unlike the terminal
//! synthetic policy, this policy NEVER chooses a smaller firmware XCR0.
//! No instructions are executed, and a validated plan is not launch authority.
//! Image validation does not prove FIP/FDP/FOP preservation, pending exception
//! fidelity, full native CPU state, or an ABI-safe last-instruction restore.

use crate::arch::x86_64::xstate::{XstateArea, XstateCapabilities, XstateError, XstateLayout};

const XSAVE: u32 = 1 << 26;
const OSXSAVE: u64 = 1 << 18;
const CPUID_OSXSAVE: u32 = 1 << 27;
const AVX: u32 = 1 << 28;
const FFXSR: u64 = 1 << 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareXstatePlan {
    original: FirmwareXstateControls,
    capture: FirmwareXstateControls,
    layout: XstateLayout,
}

impl FirmwareXstatePlan {
    pub fn validate(evidence: FirmwareXstateEvidence) -> Result<Self, FirmwareXstateError> {
        let original = evidence.original;
        let mut caps = evidence.capabilities;
        if evidence.max_basic_leaf < 1 {
            return Err(FirmwareXstateError::MissingCpuidEvidence);
        }
        // This future probe requires firmware already in 64-bit paged mode
        // with legacy SSE management. General mode transitions are not modeled.
        if original.cr0 & ((1 << 31) | 1) != (1 << 31) | 1
            || original.cr4 & ((1 << 5) | (1 << 9)) != (1 << 5) | (1 << 9)
            || original.efer & ((1 << 8) | (1 << 10)) != (1 << 8) | (1 << 10)
        {
            return Err(FirmwareXstateError::UnsupportedControlState);
        }
        // Reject unmodeled CPUID D.1 capabilities (including dynamic XFD),
        // and CR4's protection-key/CET and later controls. No absent MSR
        // observation may be treated as evidence that such state is zero.
        if evidence.leaf_d1_eax & !0xf != 0 || original.cr4 >> 22 != 0 {
            return Err(FirmwareXstateError::UnsupportedExtendedControls);
        }
        let xsave = caps.leaf1_ecx & XSAVE != 0;
        if xsave {
            if evidence.max_basic_leaf < 0xd {
                return Err(FirmwareXstateError::MissingCpuidEvidence);
            }
            // Turning OSXSAVE on merely to discover an unknown original XCR0
            // requires another audited capture path. Fail closed here.
            if original.cr4 & OSXSAVE == 0 || caps.leaf1_ecx & CPUID_OSXSAVE == 0 {
                return Err(FirmwareXstateError::InconsistentEnablement);
            }
            let original_mask = original.xcr0.ok_or(FirmwareXstateError::MissingOriginalXcr0)?;
            if !matches!(original_mask, 3 | 7) || original_mask & !caps.supported_xcr0 != 0 {
                return Err(FirmwareXstateError::UnsupportedOriginalXcr0);
            }
            if original_mask == 7 && caps.leaf1_ecx & AVX == 0 {
                return Err(FirmwareXstateError::InconsistentEnablement);
            }
            let has_xss = evidence.leaf_d1_eax & (1 << 3) != 0;
            if !has_xss && (evidence.supported_xss != 0 || original.xss.is_some()) {
                return Err(FirmwareXstateError::InconsistentSupervisorEvidence);
            }
            if has_xss && original.xss.is_none() {
                return Err(FirmwareXstateError::MissingOriginalXss);
            }
            if original.xss.is_some_and(|value| value != 0) {
                return Err(FirmwareXstateError::SupervisorStateEnabled);
            }
            // Existing layout detection chooses AVX from CPUID. Suppress that
            // selection ONLY when it is absent from the captured ORIGINAL
            // mask; this never writes or truncates any firmware register.
            if original_mask == 3 {
                caps.leaf1_ecx &= !AVX;
            }
        } else if original.cr4 & OSXSAVE != 0
            || caps.leaf1_ecx & CPUID_OSXSAVE != 0
            || original.xcr0.is_some()
            || original.xss.is_some()
            || evidence.leaf_d1_eax != 0
            || evidence.supported_xss != 0
            || caps.supported_xcr0 != 0
            || caps.enabled_size != 0
            || caps.max_size != 0
            || caps.leaf1_ecx & AVX != 0
        {
            return Err(FirmwareXstateError::InconsistentEnablement);
        }
        let layout = XstateLayout::detect(caps).map_err(FirmwareXstateError::Layout)?;
        layout.validate_enabled_size(caps.enabled_size).map_err(FirmwareXstateError::Layout)?;
        let capture = FirmwareXstateControls {
            cr0: original.cr0 & !((1 << 2) | (1 << 3)), // clear EM/TS only temporarily
            efer: original.efer & !FFXSR,               // never permit omission of XMM registers
            ..original
        };
        Ok(Self { original, capture, layout })
    }

    pub const fn original_controls(self) -> FirmwareXstateControls {
        self.original
    }
    pub const fn capture_controls(self) -> FirmwareXstateControls {
        self.capture
    }
    pub const fn layout(self) -> XstateLayout {
        self.layout
    }
    pub const fn obligations(self) -> FirmwareXstateObligations {
        FirmwareXstateObligations {
            capture_before_simd_or_fpu_use: true,
            initialize_save_header_before_first_capture: true,
            separate_exclusively_owned_aligned_areas: true,
            retain_original_xcr0_and_xss: true,
            eager_guest_and_host_switch: true,
            block_guest_control_mutation: true,
            restore_image_before_original_controls: true,
            no_simd_between_final_restore_and_abi_return: true,
            verify_original_control_readback: true,
            qualify_x87_exception_pointer_preservation: true,
        }
    }

    /// Validate a separately captured ORIGINAL image without initializing or
    /// rewriting it. Valid bytes alone cannot prove an actual CPU capture.
    pub fn validate_original_image(
        self,
        image: &XstateArea,
        observed_mxcsr_mask: u32,
    ) -> Result<(), FirmwareXstateError> {
        image.validate(self.layout, observed_mxcsr_mask).map_err(FirmwareXstateError::Layout)
    }

    /// Final readback after the original image and control values were restored.
    /// Exact equality includes unrelated retained control bits and Option state.
    /// Equality is not proof of register payload restoration or CPU affinity.
    pub fn validate_restored_controls(
        self,
        actual: FirmwareXstateControls,
    ) -> Result<(), FirmwareXstateError> {
        if actual != self.original {
            return Err(FirmwareXstateError::OriginalControlsNotRestored);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FirmwareXstateEvidence {
    pub max_basic_leaf: u32,
    pub capabilities: XstateCapabilities,
    pub leaf_d1_eax: u32,
    /// CPUID D.1 EDX:ECX, supervisor component support.
    pub supported_xss: u64,
    pub original: FirmwareXstateControls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareXstateControls {
    pub cr0: u64,
    pub cr4: u64,
    pub efer: u64,
    /// None only when CPUID establishes that XCR0 does not exist.
    pub xcr0: Option<u64>,
    /// Some(value) required when XSAVES/XSS is enumerated.
    pub xss: Option<u64>,
}

/// Obligations an eventual assembly implementation must discharge separately.
/// The values describe work REQUIRED, never work already completed by Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareXstateObligations {
    pub capture_before_simd_or_fpu_use: bool,
    pub initialize_save_header_before_first_capture: bool,
    pub separate_exclusively_owned_aligned_areas: bool,
    pub retain_original_xcr0_and_xss: bool,
    pub eager_guest_and_host_switch: bool,
    pub block_guest_control_mutation: bool,
    pub restore_image_before_original_controls: bool,
    pub no_simd_between_final_restore_and_abi_return: bool,
    pub verify_original_control_readback: bool,
    /// AMD legacy exception pointers may not be saved/restored when FSW.ES=0.
    /// The terminal emulator's sentinel tests do not establish this property.
    pub qualify_x87_exception_pointer_preservation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareXstateError {
    MissingCpuidEvidence,
    InconsistentEnablement,
    MissingOriginalXcr0,
    UnsupportedOriginalXcr0,
    MissingOriginalXss,
    SupervisorStateEnabled,
    InconsistentSupervisorEvidence,
    UnsupportedExtendedControls,
    UnsupportedControlState,
    Layout(XstateError),
    OriginalControlsNotRestored,
}
