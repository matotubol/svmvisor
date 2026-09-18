//! Native transition ABI storage, version 1; assembly is not linked by this module.
//!
//! This module deliberately exports no admission token, launch function, or
//! success predicate. Populating inputs does not prove CPU ownership, accessible
//! memory, WB caching, debugger cooperation, or a restorable firmware profile.
//! See docs/native-transition-contract.md before linking its assembly peer.

pub const ABI_VERSION: u64 = 1;
pub const CONTEXT_BYTES: usize = 1088;
pub const XSTATE_CAPACITY: usize = 1024;

/// Literal sites still need the architecture/CPUID guards in the contract.
/// Listing an address here is not permission to probe its availability.
pub mod msr {
    pub use svmvisor_hypervisor::arch::x86_64::{encryption::SEV_STATUS, msr::HWCR};
    pub const EFER: u32 = 0xc000_0080;
    pub const VM_CR: u32 = 0xc001_0114;
    pub const VM_HSAVE_PA: u32 = 0xc001_0117;
    pub const DEBUGCTL: u32 = 0x0000_01d9;
    pub const DEBUG_EXTN_CTL: u32 = 0xc000_010f;
    pub const FS_BASE: u32 = 0xc000_0100;
    pub const GS_BASE: u32 = 0xc000_0101;
    pub const KERNEL_GS_BASE: u32 = 0xc000_0102;
    pub const XSS: u32 = 0x0000_0da0;
}

/// Output validity distinguishes skipped optional MSRs from an observed zero.
pub mod capture {
    pub const CORE: u64 = 1 << 0;
    pub const DEBUG_REGISTERS: u64 = 1 << 1;
    pub const SVM_MSRS: u64 = 1 << 2;
    pub const HWCR: u64 = 1 << 3;
    pub const DEBUGCTL: u64 = 1 << 4;
    pub const DEBUG_EXTN: u64 = 1 << 5;
    pub const SEV_STATUS: u64 = 1 << 6;
    pub const XCR0: u64 = 1 << 7;
    pub const XSS: u64 = 1 << 8;
    pub const SEGMENT_BASES: u64 = 1 << 9;
    pub const ALL: u64 = (1 << 10) - 1;
}

pub mod guest_capture {
    pub const EXIT_FIELDS: u64 = 1 << 0;
    pub const GPRS: u64 = 1 << 1;
    pub const EXTRA: u64 = 1 << 2;
    pub const XSTATE: u64 = 1 << 3;
    pub const ALL: u64 = (1 << 4) - 1;
}

/// Raw integers cross assembly boundaries; invalid enum values cannot cause UB.
/// Stages describe completed instructions, never promises to perform cleanup.
pub mod progress {
    pub const NOT_ENTERED: u64 = 0;
    pub const ORIGINAL_CAPTURED: u64 = 1;
    pub const SVME_ENABLED: u64 = 2;
    pub const GIF_CLEARED: u64 = 3;
    pub const HOST_EXTRA_SAVED: u64 = 4;
    pub const HSAVE_BOUND: u64 = 5;
    pub const GUEST_EXTRA_LOADED: u64 = 6;
    pub const VMRUN_ATTEMPTED: u64 = 7;
    pub const VMEXIT_CAPTURED: u64 = 8;
    pub const HOST_EXTRA_RESTORED: u64 = 9;
    pub const HOST_SCALARS_RESTORED: u64 = 10;
    pub const EVENTS_RELEASED: u64 = 11;
    pub const EFER_RESTORED: u64 = 12;
    pub const RESTORED_OBSERVED: u64 = 13;
    pub const RETURNING: u64 = 14;
}

pub mod outcome {
    pub const NOT_RUN: u64 = 0;
    pub const REFUSED: u64 = 1;
    pub const VMMCALL: u64 = 2;
    pub const NMI: u64 = 3;
    /// INIT remains pending: release may reset the CPU and never return here.
    pub const INIT: u64 = 4;
    pub const GUEST_EXCEPTION: u64 = 5;
    pub const INVALID_GUEST: u64 = 6;
    pub const UNEXPECTED_EXIT: u64 = 7;
    pub const GUARDED_HOST_FAULT: u64 = 8;
    /// Completed host state round trip with zero attempted guest entries.
    pub const ROUND_TRIP: u64 = 9;
    pub const INTR: u64 = 10;
    /// Terminal mismatch: this result must never be interpreted as a safe return.
    pub const RESTORATION_FAILED: u64 = 11;
    /// Exactly 32 CPUID/query pairs and one terminal stop passed the fixed guest
    /// protocol; restoration and outer adapter checks remain separate evidence.
    pub const MULTI_EXIT: u64 = 12;
}

pub mod mode {
    pub const ONE_ENTRY: u64 = 0;
    pub const BIND_ONLY: u64 = 1;
    pub const MULTI_EXIT: u64 = 2;
}

/// The fixed integer multi-exit protocol uses existing observation storage;
/// context size and all pre-existing field offsets remain ABI version 1.
pub mod multi {
    pub const ROUNDS: u64 = 32;
    pub const EXPECTED_EXITS: u64 = 2 * ROUNDS + 1;
    pub const CPUID_RIP: u64 = 0x1086;
    pub const QUERY_RIP: u64 = 0x10bf;
    pub const STOP_RIP: u64 = 0x10ff;
    pub const FAIL_UD2_RIP: u64 = 0x1102;
    pub const GUEST_RSP: u64 = 0x9000;
    pub const CPUID_INPUT_RAX_HIGH: u64 = 0xaabb_ccdd_0000_0000;
    pub const CPUID_INPUT_RCX: u64 = 0x1122_3344_0000_0000;
    pub const CPUID_INPUT_RBX: u64 = 0x7788_99aa_0000_0303;
    pub const CPUID_LEAVES: [u32; 8] =
        [0, 1, 0x4000_0000, 0x4000_0001, 0x8000_0000, 0x8000_0001, 0xdead_beef, 0x4000_0002];
    /// Exact `svmvisor_hypervisor::svm::emulation::cpuid` results, in AX/BX/CX/DX order.
    pub const CPUID_OUTPUTS: [[u64; 4]; 8] = [
        [1, 0x566d_7653, 0x7473_6554, 0x726f_7369],
        [0, 0, 0x8000_0000, 0x60],
        [0x4000_0001, 0x566d_7653, 0x726f_7369, 0x7473_6554],
        [1, 0, 0, 0],
        [0x8000_0001, 0, 0, 0],
        [0, 0, 0, 0x2000_0000],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ];
    /// RBP, RSI, RDI, R8..R14; R15 is the independently checked round number.
    pub const GPR_SENTINELS: [u64; 10] = [
        0x6d75_0000_0000_0304,
        0x6d75_0000_0000_0305,
        0x6d75_0000_0000_0306,
        0x6d75_0000_0000_0308,
        0x6d75_0000_0000_0309,
        0x6d75_0000_0000_030a,
        0x6d75_0000_0000_030b,
        0x6d75_0000_0000_030c,
        0x6d75_0000_0000_030d,
        0x6d75_0000_0000_030e,
    ];
    pub const CPUID_COUNT: usize = 0;
    pub const QUERY_COUNT: usize = 1;
    pub const RESUME_COUNT: usize = 2;
    pub const FAILURE: usize = 3;
    pub const NRIP: usize = 4;
    pub const LAST_PHASE: usize = 5;
    /// The current CPUID/VMMCALL nRIP was captured under actual NRIPS support.
    /// Equality was checked only if control reached the site check; errors may
    /// precede it, and this flag alone never means continuation was accepted.
    pub const NRIP_CHECKED: usize = 6;
    pub const PHASE_CPUID: u64 = 1;
    pub const PHASE_QUERY: u64 = 2;
    pub const PHASE_STOP: u64 = 3;
    pub mod failure {
        pub const COUNTS: u64 = 1;
        pub const SITE: u64 = 2;
        pub const OPERAND: u64 = 3;
        pub const GPR: u64 = 4;
        pub const STACK_FLAGS: u64 = 5;
        pub const PENDING_EVENT: u64 = 6;
        pub const NRIP: u64 = 7;
        pub const EXIT: u64 = 8;
        pub const HARD_CAP: u64 = 9;
    }
}

/// Constant offsets can be passed to global_asm! rather than silently duplicating
/// Rust layout in assembly. The numeric assertions below independently pin v1.
pub mod offset {
    use crate::native::transition::state::NativeTransition;
    pub const INPUTS: usize = core::mem::offset_of!(NativeTransition, inputs);
    pub const ORIGINAL: usize = core::mem::offset_of!(NativeTransition, original);
    pub const RESTORED: usize = core::mem::offset_of!(NativeTransition, restored);
    pub const GUEST: usize = core::mem::offset_of!(NativeTransition, guest);
    pub const JOURNAL: usize = core::mem::offset_of!(NativeTransition, journal);
}

macro_rules! pin_field {
    ($ty:ty, $field:ident, $offset:expr) => {
        const _: () = assert!(core::mem::offset_of!($ty, $field) == $offset);
    };
}

/// Storage only. Native assembly is deliberately not linked or declared here;
/// the adapter must own the unsafe preparation, scope and linking boundary.
#[repr(C, align(64))]
pub struct NativeTransition {
    pub inputs: TransitionInputs,
    pub original: ScalarState,
    pub restored: ScalarState,
    pub guest: GuestObservation,
    pub journal: TransitionJournal,
}

const _: () = assert!(core::mem::size_of::<NativeTransition>() == CONTEXT_BYTES);
const _: () = assert!(core::mem::align_of::<NativeTransition>() == 64);
pin_field!(NativeTransition, inputs, 0);
pin_field!(NativeTransition, original, 192);
pin_field!(NativeTransition, restored, 448);
pin_field!(NativeTransition, guest, 704);
pin_field!(NativeTransition, journal, 960);

/// Physical operands and linear operands are separate even for an identity map.
/// All buffers remain exclusively owned for the complete synchronous call.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TransitionInputs {
    pub abi_version: u64,
    pub context_bytes: u64,
    /// Immutable, still-live NativeBoundary; never use as a writable scratch area.
    pub image_boundary_va: u64,
    pub guest_vmcb_pa: u64,
    pub guest_vmcb_va: u64,
    pub host_extra_pa: u64,
    pub host_extra_va: u64,
    pub restored_extra_pa: u64,
    pub restored_extra_va: u64,
    pub guest_extra_pa: u64,
    pub guest_extra_va: u64,
    /// A distinct 4-KiB opaque hardware save area, not a software VMCB image.
    pub hsave_pa: u64,
    pub original_xstate_va: u64,
    pub restored_xstate_va: u64,
    pub guest_xstate_va: u64,
    /// 0 = FXSAVE64, 3/7 = exact enabled standard XSAVE64 mask.
    pub xstate_profile: u64,
    pub xstate_bytes: u64,
    pub expected_vmmcall_rip: u64,
    pub expected_vmmcall_rax: u64,
    pub expected_bsp_apic_id: u64,
    /// Prepared expected ScalarState; a comparison source, not an admission flag.
    pub expected_state_va: u64,
    /// Exact previously qualified active GDT bytes for final in-lease comparison.
    pub host_gdt_copy_va: u64,
    pub host_gdt_bytes: u64,
    /// See `mode`; the fixed multi-exit profile is distinct from one-entry mode.
    pub mode: u64,
}

const _: () = assert!(core::mem::size_of::<TransitionInputs>() == 192);
pin_field!(TransitionInputs, guest_vmcb_pa, 24);
pin_field!(TransitionInputs, host_extra_pa, 40);
pin_field!(TransitionInputs, restored_extra_pa, 56);
pin_field!(TransitionInputs, guest_extra_pa, 72);
pin_field!(TransitionInputs, hsave_pa, 88);
pin_field!(TransitionInputs, original_xstate_va, 96);
pin_field!(TransitionInputs, xstate_profile, 120);
pin_field!(TransitionInputs, expected_state_va, 160);
pin_field!(TransitionInputs, mode, 184);

/// Captured immediately at the transition call, then independently after repair.
/// This is later than original EFI image entry. Hidden FS/GS/TR/LDTR and
/// SYSCALL/SYSENTER state live in the separately owned VMSAVE pages.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScalarState {
    pub captured_fields: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub cr8: u64,
    pub efer: u64,
    pub vm_hsave_pa: u64,
    /// PUSHFQ encoding. RF/VM are not claimed as independently captured state.
    pub rflags: u64,
    pub dr0: u64,
    pub dr1: u64,
    pub dr2: u64,
    pub dr3: u64,
    pub dr6: u64,
    pub dr7: u64,
    pub vm_cr: u64,
    pub hwcr: u64,
    pub debugctl: u64,
    pub debug_extn_ctl: u64,
    pub sev_status: u64,
    pub xcr0: u64,
    pub xss: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub kernel_gs_base: u64,
    pub gdtr: DescriptorTableImage,
    pub idtr: DescriptorTableImage,
    /// CS, SS, DS, ES, FS, GS, LDTR, TR in this order.
    pub selectors: [u16; 8],
    pub bsp_apic_id: u32,
    pub reserved: u32,
}

const _: () = assert!(core::mem::size_of::<ScalarState>() == 256);
pin_field!(ScalarState, efer, 48);
pin_field!(ScalarState, rflags, 64);
pin_field!(ScalarState, dr6, 104);
pin_field!(ScalarState, dr7, 112);
pin_field!(ScalarState, vm_cr, 120);
pin_field!(ScalarState, xcr0, 160);
pin_field!(ScalarState, fs_base, 176);
pin_field!(ScalarState, gdtr, 200);
pin_field!(ScalarState, idtr, 216);
pin_field!(ScalarState, selectors, 232);
pin_field!(ScalarState, bsp_apic_id, 248);

/// Exact SGDT/SIDT ten-byte encoding followed by six initialized reserved bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescriptorTableImage {
    pub bytes: [u8; 10],
    pub reserved: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<DescriptorTableImage>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GuestObservation {
    pub exit_code: u64,
    /// Defined only for exit reasons assigning meaning to these fields.
    pub exit_info1: u64,
    pub exit_info2: u64,
    pub exit_int_info: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub rax: u64,
    /// RCX, RDX, RBX, RBP, RSI, RDI, R8..R15; saved before any are scratch.
    pub gprs: [u64; 14],
    pub captured_fields: u64,
    /// In MULTI_EXIT mode the named `multi` indices hold its bounded journal.
    /// In the original modes every word remains reserved and zero.
    pub reserved: [u64; 9],
}

const _: () = assert!(core::mem::size_of::<GuestObservation>() == 256);
pin_field!(GuestObservation, gprs, 64);
pin_field!(GuestObservation, captured_fields, 176);
pin_field!(GuestObservation, reserved, 184);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TransitionJournal {
    pub progress: u64,
    pub outcome: u64,
    pub refusal_code: u64,
    pub fault_vector: u64,
    pub fault_rip: u64,
    pub fault_error: u64,
    /// A future exact-site handler must bind this to instruction PC and stage.
    pub fault_site: u64,
    pub fault_context_flags: u64,
    pub vmrun_attempts: u64,
    pub completed_exits: u64,
    /// Set after STGI completes; no claim of direct GIF readback.
    pub event_release_completed: u64,
    /// Set only after comparisons and final repair; not supplied by the caller.
    pub restoration_complete: u64,
    /// Number of originally-clear host CS/SS/DS/ES GDT A bits restored (0..4).
    pub gdt_accessed_restores: u64,
    pub reserved: [u64; 3],
}

const _: () = assert!(core::mem::size_of::<TransitionJournal>() == 128);
pin_field!(TransitionJournal, vmrun_attempts, 64);
pin_field!(TransitionJournal, event_release_completed, 80);
pin_field!(TransitionJournal, restoration_complete, 88);
pin_field!(TransitionJournal, gdt_accessed_restores, 96);
