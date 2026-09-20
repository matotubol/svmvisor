//! The diagnostic hypercall protocol: VMMCALL with the opcode in RAX.
//!
//! This is a project test protocol. No guest memory access, allocation,
//! register mutation or resume occurs here.

pub const ABI_VERSION: u32 = 1;
pub const HYPERCALL_QUERY: u64 = 0;
pub const HYPERCALL_STOP: u64 = 1;

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
        HYPERCALL_QUERY => HypercallAction::Query { abi_version: ABI_VERSION as u64 },
        HYPERCALL_STOP => HypercallAction::Stop,
        _ => HypercallAction::Unsupported { opcode },
    }
}
