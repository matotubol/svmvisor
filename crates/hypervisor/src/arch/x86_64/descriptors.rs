//! Fixed ring-0 long-mode guest GDT and TSS images, without loading hardware.
//!
//! Layouts follow pinned AMD APM vol. 2 rev. 3.44 sections 4.7, 4.8, 12.2.2, 12.2.5
//! and 15.5.1; Appendix B defines expanded VMCB segment fields. The 104-byte
//! TSS assumes CET is disabled. Only RSP0 and IST1 are populated; ring changes
//! to rings 1/2 and IDT gates selecting IST2..7 are outside this profile.
//! Addresses are guest virtual addresses. Validation establishes neither
//! backing memory nor mappings, ownership, stack space or launch readiness.
//! The image represents an already loaded guest TR: both GDT and saved TR
//! use busy TSS type 0xb, matching post-LTR state. It is not an LTR input.

use crate::memory::address::is_canonical_48;

pub const CODE_SELECTOR: u16 = 8;
pub const DATA_SELECTOR: u16 = 16;
pub const TSS_SELECTOR: u16 = 24;
pub const GDT_BYTES: usize = 40;
pub const TSS_BYTES: usize = 104;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestDescriptorRequest {
    pub gdt_base: u64,
    pub tss_base: u64,
    pub rsp0: u64,
    pub ist1: u64,
}

impl GuestDescriptorRequest {
    pub fn validate(self) -> Result<ValidatedGuestDescriptors, DescriptorError> {
        let gdt_last =
            canonical_last(self.gdt_base, GDT_BYTES).ok_or(DescriptorError::InvalidGdtRange)?;
        let tss_last =
            canonical_last(self.tss_base, TSS_BYTES).ok_or(DescriptorError::InvalidTssRange)?;
        if self.gdt_base <= tss_last && self.tss_base <= gdt_last {
            return Err(DescriptorError::OverlappingTables);
        }
        if !is_canonical_48(self.rsp0) {
            return Err(DescriptorError::NonCanonicalRsp0);
        }
        if !is_canonical_48(self.ist1) {
            return Err(DescriptorError::NonCanonicalIst1);
        }
        let mut gdt = [0; GDT_BYTES];
        // Accessed bits are set in advance. Limit is 0xfffff with G=1.
        put::<8, _, _>(&mut gdt, 0x00af_9b00_0000_ffff_u64.to_le_bytes());
        put::<16, _, _>(&mut gdt, 0x00cf_9300_0000_ffff_u64.to_le_bytes());
        // Busy 64-bit TSS: byte-granular limit 103, upper reserved bits 0.
        let base = self.tss_base;
        let low = 103
            | ((base & 0xffff) << 16)
            | (((base >> 16) & 0xff) << 32)
            | (0x8b_u64 << 40)
            | (((base >> 24) & 0xff) << 56);
        put::<24, _, _>(&mut gdt, low.to_le_bytes());
        put::<32, _, _>(&mut gdt, ((base >> 32) as u32).to_le_bytes());
        let mut tss = [0; TSS_BYTES];
        put::<4, _, _>(&mut tss, self.rsp0.to_le_bytes());
        put::<36, _, _>(&mut tss, self.ist1.to_le_bytes());
        // Offset is beyond the inclusive TSS limit: no TSS I/O permission map.
        // This does not restrict ring-0 I/O; the SVM IOPM is separate.
        put::<102, _, _>(&mut tss, (TSS_BYTES as u16).to_le_bytes());
        Ok(ValidatedGuestDescriptors { request: self, gdt, tss })
    }
}

/// Immutable image and matching semantic fields, with no unchecked constructor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedGuestDescriptors {
    request: GuestDescriptorRequest,
    gdt: [u8; GDT_BYTES],
    tss: [u8; TSS_BYTES],
}

impl ValidatedGuestDescriptors {
    pub const fn gdt(&self) -> &[u8; GDT_BYTES] {
        &self.gdt
    }
    pub const fn tss(&self) -> &[u8; TSS_BYTES] {
        &self.tss
    }
    pub const fn cs(&self) -> SegmentState {
        SegmentState { selector: CODE_SELECTOR, attributes: 0xa9b, limit: u32::MAX, base: 0 }
    }
    pub const fn data(&self) -> SegmentState {
        SegmentState { selector: DATA_SELECTOR, attributes: 0xc93, limit: u32::MAX, base: 0 }
    }
    pub const fn tr(&self) -> SegmentState {
        SegmentState {
            selector: TSS_SELECTOR,
            attributes: 0x8b,
            limit: 103,
            base: self.request.tss_base,
        }
    }
    pub const fn gdtr(&self) -> SegmentState {
        SegmentState { selector: 0, attributes: 0, limit: 39, base: self.request.gdt_base }
    }
}

/// Expanded segment representation, not itself an unchecked VMCB input token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentState {
    pub selector: u16,
    pub attributes: u16,
    pub limit: u32,
    pub base: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    InvalidGdtRange,
    InvalidTssRange,
    OverlappingTables,
    NonCanonicalRsp0,
    NonCanonicalIst1,
}

/// Compile-time field bounds avoid retaining a runtime panic path in DXE.
#[inline(always)]
fn put<const OFFSET: usize, const N: usize, const M: usize>(dst: &mut [u8; N], src: [u8; M]) {
    const {
        assert!(M <= N && OFFSET <= N - M);
    }
    for (out, byte) in dst.iter_mut().skip(OFFSET).zip(src) {
        *out = byte;
    }
}

fn canonical_last(base: u64, len: usize) -> Option<u64> {
    let last = base.checked_add(len as u64 - 1)?;
    (is_canonical_48(base) && is_canonical_48(last)).then_some(last)
}
