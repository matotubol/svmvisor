//! Immediate, independently seeded canaries around the actual transition.
//!
//! The default assembly admits the explicit TCG fixture. `native-returning`
//! selects a separate gate requiring AuthenticAMD, signature 00B40F40h, and no
//! reported hypervisor before its control/MSR reads. The native caller must also
//! establish live CPU/resource admission; this shim is not a recovery boundary.
//! The shim reads EFER but contains no firmware calls or CR/MSR/XCR0 writes.

use crate::native::transition::state::NativeTransition;

pub const CANARY_BYTES: usize = 3328;
pub const CANARY_ALIGNMENT: usize = 64;
pub const MAX_SHIM_STACK_BYTES: usize = 320;

pub mod failure {
    pub const GPRS: u64 = 1 << 0;
    pub const RFLAGS: u64 = 1 << 1;
    pub const RSP: u64 = 1 << 2;
    pub const X87_ENVIRONMENT: u64 = 1 << 3;
    pub const X87_PAYLOAD: u64 = 1 << 4;
    pub const MXCSR: u64 = 1 << 5;
    pub const XMM: u64 = 1 << 6;
    pub const YMM_UPPER: u64 = 1 << 7;
    pub const XSTATE_CONTROLS: u64 = 1 << 8;
    pub const SETUP: u64 = 1 << 63;
}

unsafe extern "efiapi" {
    /// Save the actual Rust caller state, seed canaries, call the exact
    /// `svmvisor_native_transition`, capture/compare before any Rust executes,
    /// restore the actual caller state, and return the failure mask in RAX.
    ///
    /// # Safety
    /// The caller must already own the synchronous, callback-free BSP interval
    /// at HIGH TPL in either the explicitly admitted pinned QEMU TCG fixture or
    /// the native-returning caller's separately admitted live native interval.
    /// Context, its immutable NativeBoundary and every transition operand remain
    /// valid for the entire call. `canary` is a distinct writable WB allocation of
    /// 3,328 bytes, aligned to 64 bytes, disjoint from context, its operands, and
    /// all active stacks.
    /// Entry has IF/DF/TF/NT/AC clear, CR0.EM/TS clear, CR4.OSFXSR set, FFXSR clear,
    /// and the qualified profile 0/3/7 already active. This shim never enables an
    /// instruction or normalizes a mode. A returning transition must restore
    /// instruction-enabling controls before the immediate save can execute;
    /// unexpected faults or a corrupted return stack have no recovery here.
    ///
    /// All 15 GPRs are checked, including the context-valued RCX and seeded RAX;
    /// returning from this shim replaces only its caller's RAX with the mask.
    /// PUSHFQ does not independently expose RF/VM. All eight x87 slots are seeded
    /// nonempty and checked; conditional no-#MF FIP/FDP/FOP are not compared or
    /// promised restored if the callee changed them. The original caller uses
    /// XRSTOR(profile & !1) then FXRSTOR, matching that conditional-pointer
    /// qualification. Disabled/dormant xstate components are outside the claim.
    pub fn svmvisor_native_transition_canary(
        context: *mut NativeTransition,
        canary: *mut TransitionCanary,
    ) -> u64;
}

/// The assembly initializes every byte before using it. Allocate this in
/// separate writable WB RAM; no constructor or large Rust stack value is needed.
/// Images use standard XSAVE layout, with the qualified AVX offset exactly 576.
/// `observed_gprs` order is RAX, RCX, RDX, RBX, RBP, RSI, RDI, R8..R15.
/// RCX is checked against the context pointer; the other 14 have distinct seeds.
#[repr(C, align(64))]
pub struct TransitionCanary {
    pub abi_version: u64,
    pub buffer_bytes: u64,
    pub failures: u64,
    pub profile: u64,
    pub expected_rflags: u64,
    pub observed_rflags: u64,
    pub expected_rsp: u64,
    pub observed_rsp: u64,
    pub observed_gprs: [u64; 15],
    pub context_address: u64,
    pub original_cr0: u64,
    pub original_cr4: u64,
    pub original_xcr0: u64,
    pub observed_cr0: u64,
    pub observed_cr4: u64,
    pub observed_xcr0: u64,
    /// One only after the immediate hardware xstate capture completed.
    pub observed_complete: u64,
    /// One immediately before the single exact transition CALL.
    pub transition_called: u64,
    pub original_xstate: [u8; 1024],
    pub seeded_xstate: [u8; 1024],
    pub observed_xstate: [u8; 1024],
}

const _: () = assert!(core::mem::size_of::<TransitionCanary>() == CANARY_BYTES);
const _: () = assert!(core::mem::align_of::<TransitionCanary>() == CANARY_ALIGNMENT);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, observed_gprs) == 64);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, context_address) == 184);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, original_cr0) == 192);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, original_cr4) == 200);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, original_xcr0) == 208);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, observed_cr0) == 216);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, observed_cr4) == 224);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, observed_xcr0) == 232);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, observed_complete) == 240);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, transition_called) == 248);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, original_xstate) == 256);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, seeded_xstate) == 1280);
const _: () = assert!(core::mem::offset_of!(TransitionCanary, observed_xstate) == 2304);
