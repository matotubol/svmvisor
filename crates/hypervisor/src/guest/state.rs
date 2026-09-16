//! A constrained register tuple for a future synthetic ring-0 64-bit guest.
//!
//! Pinned AMD APM volume 2 revision 3.44 sections 3.1.1, 3.1.3, 3.1.6,
//! 3.1.7, and 15.5.1 define the registers and SVM consistency requirements.
//! These exact values are project policy, not the set of all legal AMD states.
//! The baseline omits LA57, PCID, CET, OSXSAVE, and NXE. An explicit validated
//! extended-state profile permits its fixed CR4 bits. NX support is not inferred.
//!
//! Validation covers only this tuple. Segment descriptors (including CS.L=1,
//! CS.D=0 and ring-0 attributes), CPL, debug registers, tables, mappings,
//! intercepts, and remaining VMCB state are still required. A validated tuple
//! does not establish executable memory, stack availability, CPU support for
//! long mode, or launch readiness. No hardware access occurs here.

use crate::memory::address::{AddressError, AddressPolicy};

/// PG, WP, NE, MP and PE; ET is the architectural fixed-one bit.
pub const SYNTHETIC_CR0: u64 = 0x8001_0033;
/// PAE only: use four-level long-mode page tables.
pub const SYNTHETIC_CR4: u64 = 0x20;
/// SVME, LMA and LME. SVM guest consistency requires SVME even without nesting.
pub const SYNTHETIC_EFER: u64 = 0x1500;
/// Architectural fixed-one bit; IF, DF, TF, VM and IOPL are all zero.
pub const SYNTHETIC_RFLAGS: u64 = 0x2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestStateRequest {
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cr0: u64,
    /// Guest-physical root page address, without low CR3 control bits.
    pub cr3: u64,
    pub cr4: u64,
    pub efer: u64,
    pub rax: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestStateError {
    NonCanonicalRip,
    NonCanonicalRsp,
    UnsupportedRflags,
    UnsupportedCr0,
    UnsupportedCr4,
    UnsupportedEfer,
    Cr3(AddressError),
}

/// Immutable validated values; there is no unchecked public constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedGuestState {
    request: GuestStateRequest,
}

impl GuestStateRequest {
    /// Apply the initial profile's numeric address policy to the entire CR3
    /// page. This does not translate the GPA to a host address or establish
    /// nested-page-table membership. Canonical pointers need not be mapped.
    pub fn validate(self, policy: &AddressPolicy) -> Result<ValidatedGuestState, GuestStateError> {
        self.validate_cr4_profile(policy, SYNTHETIC_CR4, SYNTHETIC_RFLAGS)
    }

    /// Explicit bounded extended-state guest profile. The caller must install
    /// the layout's XCR0, intercept XSETBV, and eagerly switch the owned areas.
    /// This method validates values only; it does not perform those operations.
    pub fn validate_with_xstate(
        self,
        policy: &AddressPolicy,
        layout: crate::arch::x86_64::xstate::XstateLayout,
    ) -> Result<ValidatedGuestState, GuestStateError> {
        self.validate_cr4_profile(policy, layout.guest_cr4(), SYNTHETIC_RFLAGS)
    }

    /// Restricted loader ABI migration: retain arithmetic flags captured from
    /// native integer execution, with interrupts/direction/debug flags disabled.
    /// This deliberately installs supplied guest paging/descriptors; it does not
    /// preserve arbitrary firmware CPU state. APM vol.2 rev.3.44 3.1.6/15.5.1.
    pub fn validate_continuation_with_xstate(
        self,
        policy: &AddressPolicy,
        layout: crate::arch::x86_64::xstate::XstateLayout,
    ) -> Result<ValidatedGuestState, GuestStateError> {
        self.validate_cr4_profile(
            policy,
            layout.guest_cr4(),
            super::continuation::CONTINUATION_RFLAGS_MASK,
        )
    }

    fn validate_cr4_profile(
        self,
        policy: &AddressPolicy,
        expected_cr4: u64,
        allowed_flags: u64,
    ) -> Result<ValidatedGuestState, GuestStateError> {
        if !crate::memory::address::is_canonical_48(self.rip) {
            return Err(GuestStateError::NonCanonicalRip);
        }
        if !crate::memory::address::is_canonical_48(self.rsp) {
            return Err(GuestStateError::NonCanonicalRsp);
        }
        if self.rflags & !allowed_flags != 0 || self.rflags & SYNTHETIC_RFLAGS == 0 {
            return Err(GuestStateError::UnsupportedRflags);
        }
        if self.cr0 != SYNTHETIC_CR0 {
            return Err(GuestStateError::UnsupportedCr0);
        }
        if self.cr4 != expected_cr4 {
            return Err(GuestStateError::UnsupportedCr4);
        }
        if self.efer != SYNTHETIC_EFER {
            return Err(GuestStateError::UnsupportedEfer);
        }
        policy
            .validate(self.cr3, 4096, 4096)
            .map_err(GuestStateError::Cr3)?;
        Ok(ValidatedGuestState { request: self })
    }
}

impl ValidatedGuestState {
    pub const fn rip(self) -> u64 {
        self.request.rip
    }
    pub const fn rsp(self) -> u64 {
        self.request.rsp
    }
    pub const fn rflags(self) -> u64 {
        self.request.rflags
    }
    pub const fn cr0(self) -> u64 {
        self.request.cr0
    }
    pub const fn cr3(self) -> u64 {
        self.request.cr3
    }
    pub const fn cr4(self) -> u64 {
        self.request.cr4
    }
    pub const fn efer(self) -> u64 {
        self.request.efer
    }
    pub const fn rax(self) -> u64 {
        self.request.rax
    }
}
