//! Fixed diagnostic responses for an integer-only synthetic guest.
//!
//! This is a project test protocol, not a complete AMD CPU or real-OS CPUID
//! model. MSR, PAE and long mode describe the intended minimal execution mode;
//! FPU, SSE, NX, SVM and other features are not advertised. Returning a feature
//! flag neither emulates that feature nor establishes host support. No physical
//! CPUID, MSR, guest memory access, allocation, register mutation or resume
//! occurs here. Host identity and features are never forwarded.

pub const ABI_VERSION: u32 = 1;
pub const HYPERCALL_QUERY: u64 = 0;
pub const HYPERCALL_STOP: u64 = 1;

const VENDOR: [u8; 12] = *b"SvmVisorTest";
const MSR: u32 = 1 << 5;
const PAE: u32 = 1 << 6;
const HYPERVISOR_PRESENT: u32 = 1 << 31;
const LONG_MODE: u32 = 1 << 29;

/// Return `[EAX, EBX, ECX, EDX]`. All supported leaves are scalar, so ECX
/// input is ignored, including stale nonzero values. Unknown leaves return
/// zeros by explicit diagnostic policy, not physical-CPU fallback behavior.
///
/// Basic vendor order is EBX/EDX/ECX; the private hypervisor convention uses
/// EBX/ECX/EDX. The private ABI leaf reports only the fixed protocol version.
pub fn cpuid(leaf: u32, _subleaf: u32) -> [u32; 4] {
    let a = u32::from_le_bytes([VENDOR[0], VENDOR[1], VENDOR[2], VENDOR[3]]);
    let b = u32::from_le_bytes([VENDOR[4], VENDOR[5], VENDOR[6], VENDOR[7]]);
    let c = u32::from_le_bytes([VENDOR[8], VENDOR[9], VENDOR[10], VENDOR[11]]);
    match leaf {
        0 => [1, a, c, b],
        1 => [0, 0, HYPERVISOR_PRESENT, MSR | PAE],
        0x4000_0000 => [0x4000_0001, a, b, c],
        0x4000_0001 => [ABI_VERSION, 0, 0, 0],
        0x8000_0000 => [0x8000_0001, 0, 0, 0],
        0x8000_0001 => [0, 0, 0, LONG_MODE],
        _ => [0; 4],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HypercallAction {
    /// Caller may place this value in guest RAX after validating continuation.
    Query { abi_version: u64 },
    /// End the synthetic run without advancing RIP.
    Stop,
    /// End the synthetic run without advancing RIP or modifying registers.
    Unsupported { opcode: u64 },
}

/// The full 64-bit RAX is the opcode; high bits are never truncated. No other
/// operand is interpreted as a pointer, memory operation, or privileged action.
/// This function returns policy only; the caller owns stop/resume handling.
pub const fn hypercall(opcode: u64) -> HypercallAction {
    match opcode {
        HYPERCALL_QUERY => HypercallAction::Query {
            abi_version: ABI_VERSION as u64,
        },
        HYPERCALL_STOP => HypercallAction::Stop,
        _ => HypercallAction::Unsupported { opcode },
    }
}

/// Opt-in diagnostic clock policy; only admitted TSC/RDTSCP instructions are
/// advertised. Identity remains explicit. Invariant TSC, frequency leaves,
/// scaling and nested SVM remain absent. Guest MSR access still needs a separate
/// intercept/refusal policy; this is not a complete OS CPU model.
pub fn cpuid_with_clock(
    leaf: u32,
    subleaf: u32,
    clock: &crate::arch::x86_64::clock::ClockPlan,
) -> [u32; 4] {
    let mut result = cpuid(leaf, subleaf);
    if leaf == 1 {
        result[3] |= 1 << 4;
    }
    if leaf == 0x8000_0001 && clock.capabilities().rdtscp() {
        result[3] |= 1 << 27;
    }
    result
}
