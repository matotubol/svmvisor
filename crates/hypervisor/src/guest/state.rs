//! The register tuple of a guest entry, carried by
//! `guest::continuation::NativeContinuationRequest`. Validation belongs to
//! `guest::continuation::prepare_native_with_efer`. No hardware access occurs here.

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
