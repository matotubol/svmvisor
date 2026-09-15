//! Explicit register-frame contract for future guest-entry/exit assembly.
//!
//! This is ordinary inert Rust data: no code here captures or restores CPU
//! registers. AMD SVM stores guest RAX, RSP and RIP in the VMCB; they must not
//! have competing copies in this frame (AMD APM vol. 2 rev. 3.44, section 15.7).
//! The ordering below is project ABI policy, not an AMD hardware save format.
//! Future assembly must save every field before calling Rust and honor these
//! exact offsets. SIMD/FPU state and processor flags are outside this frame.

/// Fourteen GPRs, each eight bytes, with a total frame size of 112 bytes.
/// The named offsets are from this structure's base, not from a stack pointer
/// with an assumed prologue. Its eight-byte alignment does not establish the
/// platform call ABI's stack alignment or shadow-space requirements.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuestRegisters {
    /// Offset 0x00.
    pub rcx: u64,
    /// Offset 0x08.
    pub rdx: u64,
    /// Offset 0x10.
    pub rbx: u64,
    /// Offset 0x18.
    pub rbp: u64,
    /// Offset 0x20.
    pub rsi: u64,
    /// Offset 0x28.
    pub rdi: u64,
    /// Offset 0x30.
    pub r8: u64,
    /// Offset 0x38.
    pub r9: u64,
    /// Offset 0x40.
    pub r10: u64,
    /// Offset 0x48.
    pub r11: u64,
    /// Offset 0x50.
    pub r12: u64,
    /// Offset 0x58.
    pub r13: u64,
    /// Offset 0x60.
    pub r14: u64,
    /// Offset 0x68.
    pub r15: u64,
}

const _: () = assert!(core::mem::size_of::<GuestRegisters>() == 112);
const _: () = assert!(core::mem::align_of::<GuestRegisters>() == 8);

impl GuestRegisters {
    /// CPUID's leaf and subleaf from the caller's VMCB RAX and saved RCX.
    /// Upper halves are ignored; no host CPUID instruction is executed.
    pub const fn cpuid_inputs(&self, guest_rax: u64) -> [u32; 2] {
        [guest_rax as u32, self.rcx as u32]
    }

    /// Apply a policy-generated [EAX, EBX, ECX, EDX] result. CPUID's 32-bit
    /// outputs clear each corresponding upper half in the 64-bit guest.
    /// Only RBX, RCX and RDX change here; the returned zero-extended RAX must
    /// be stored in the VMCB by the caller. No instruction pointer is changed.
    pub fn apply_cpuid(&mut self, result: [u32; 4]) -> u64 {
        self.rbx = u64::from(result[1]);
        self.rcx = u64::from(result[2]);
        self.rdx = u64::from(result[3]);
        u64::from(result[0])
    }
}
