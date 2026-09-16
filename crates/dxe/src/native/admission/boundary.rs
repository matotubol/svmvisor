//! Exact assembly image-entry observation and selected xstate return boundary.
//!
//! This is not an SVM transition context. Controls/descriptors are observations;
//! the Rust inner and firmware services must preserve them. See the native
//! boundary contract for the restricted x87 and dormant-state qualifications.

use super::snapshot::TableSnapshot;

pub const ABI_VERSION: u64 = 1;
pub const CAPTURE_XCR0: u64 = 1;
pub const CAPTURE_XSS: u64 = 2;
pub const XSTATE_CAPACITY: usize = 1024;
pub const MAX_BOUNDARY_STACK_BYTES: usize = 1656;

/// Stack-owned, immutable for the lifetime of the synchronous Rust inner call.
/// `gprs` order is RAX, RCX, RDX, RBX, RBP, RSI, RDI, R8 through R15.
/// RAX is captured for evidence but the return value is the inner EFI status.
#[repr(C, align(64))]
pub struct NativeBoundary {
    pub abi_version: u64,
    pub captured_fields: u64,
    /// Zero selects FXSAVE64; 3/7 select standard XSAVE64 with this exact mask.
    pub profile: u64,
    pub xstate_size: u64,
    /// Valid only when CAPTURE_XCR0 is set. Never inferred by enabling OSXSAVE.
    pub xcr0: u64,
    /// Valid only when CAPTURE_XSS is set; supported-but-zero is accepted.
    pub xss: u64,
    /// Reserved for a future qualified XFD capture; XFD capability is refused.
    pub reserved_xfd: u64,
    pub reserved_xfd_err: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub cr8: u64,
    pub efer: u64,
    pub gdtr: TableSnapshot,
    pub idtr: TableSnapshot,
    pub cs: u16,
    pub ss: u16,
    pub ds: u16,
    pub es: u16,
    pub fs: u16,
    pub gs: u16,
    pub ldtr: u16,
    pub tr: u16,
    /// PUSHFQ image; this instruction does not expose RF/VM as live state.
    pub rflags: u64,
    pub entry_rsp: u64,
    pub entry_rip: u64,
    pub gprs: [u64; 15],
    pub leaf_d1_eax: u32,
    pub extended8_ebx: u32,
    pub max_basic_leaf: u32,
    pub leaf1_ecx: u32,
    pub supported_xcr0: u64,
    pub avx_offset: u32,
    pub avx_size: u32,
    pub leaf1_ebx: u32,
    pub leaf1_edx: u32,
    pub reserved: [u8; 40],
    /// Aligned original capture. Do not modify, retain or publish this pointer.
    pub xstate: [u8; XSTATE_CAPACITY],
}

impl NativeBoundary {
    pub fn xstate_address(&self) -> u64 {
        self.xstate.as_ptr() as u64
    }

    /// Verify metadata at the assembly/Rust seam. This checks the shape of the
    /// capture, not CPU ownership, hardware provenance, or pointer fidelity.
    pub fn has_valid_shape(&self) -> bool {
        if self.abi_version != ABI_VERSION
            || self.captured_fields & !(CAPTURE_XCR0 | CAPTURE_XSS) != 0
            || self.xstate_address() & 63 != 0
            || self.xss != 0
            || self.reserved_xfd != 0
            || self.reserved_xfd_err != 0
            || self.leaf_d1_eax & !0xf != 0
            || (self.captured_fields & CAPTURE_XSS != 0) != (self.leaf_d1_eax & (1 << 3) != 0)
        {
            return false;
        }
        match self.profile {
            0 => {
                self.xstate_size == 512
                    && self.captured_fields & CAPTURE_XCR0 == 0
                    && self.xcr0 == 0
                    && self.cr4 & (1 << 18) == 0
                    && self.leaf1_ecx & (1 << 27) == 0
            }
            3 | 7 => {
                self.captured_fields & CAPTURE_XCR0 != 0
                    && self.xcr0 == self.profile
                    && self.xcr0 & !self.supported_xcr0 == 0
                    && self.cr4 & (1 << 18) != 0
                    && self.leaf1_ecx & (3 << 26) == 3 << 26
                    && (576..=XSTATE_CAPACITY as u64).contains(&self.xstate_size)
                    && (self.profile == 3
                        || self.leaf1_ecx & (1 << 28) != 0
                            && self.avx_size == 256
                            && self.avx_offset >= 576
                            && u64::from(self.avx_offset) + 256 <= self.xstate_size)
            }
            _ => false,
        }
    }
}

const _: () = assert!(core::mem::size_of::<NativeBoundary>() == 1408);
const _: () = assert!(core::mem::align_of::<NativeBoundary>() == 64);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, cr0) == 64);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, efer) == 104);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, gdtr) == 112);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, idtr) == 128);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, cs) == 144);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, rflags) == 160);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, gprs) == 184);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, leaf_d1_eax) == 304);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, supported_xcr0) == 320);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, avx_offset) == 328);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, leaf1_ebx) == 336);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, reserved) == 344);
const _: () = assert!(core::mem::offset_of!(NativeBoundary, xstate) == 384);
