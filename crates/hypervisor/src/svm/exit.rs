//! Bounded decoding for a future synthetic guest's exit loop.
//!
//! AMD APM volume 2 revision 3.44 Appendix C specifies the 64-bit exit codes;
//! sections 15.7.1 and 15.25.6 define nRIP and nested-fault information.
//! This module reads no hardware or guest memory and does not emulate, resume,
//! change RIP, inject exceptions, or complete interrupt delivery. The caller
//! must retain other GPRs (including CPUID's RCX) outside the VMCB separately.

use crate::arch::x86_64::capabilities::ValidatedCapabilities;

/// Caller-supplied snapshot. Unused EXITINFO fields may be undefined and are
/// interpreted only for the specific decoded exit that defines their meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitSnapshot {
    pub code: u64,
    pub info1: u64,
    pub info2: u64,
    pub rip: u64,
    pub nrip: u64,
}

/// MSR operation evidence retained from one exclusively stopped guest.
/// Hardware construction additionally requires caller-proven native exit provenance;
/// inert VMCB contents do not establish that provenance. APM2 15.7.1/15.11.
#[derive(Clone, Copy)]
pub(crate) enum MsrInstruction<'a> {
    Bytes(&'a [u8]),
    Hardware { exit: ExitSnapshot, next: ResumeCandidate },
}
impl<'a> MsrInstruction<'a> {
    pub(crate) fn hardware(exit: ExitSnapshot, caps: &ValidatedCapabilities) -> Result<Self, ResumeError> {
        if exit.code != 0x7c || exit.info1 > 1 { return Err(ResumeError::ExitDoesNotPermitCandidate); }
        if !caps.optional_features().nrip_save { return Err(ResumeError::NripNotEstablished); }
        if !crate::memory::address::is_canonical_48(exit.rip) { return Err(ResumeError::NonCanonicalRip); }
        if !crate::memory::address::is_canonical_48(exit.nrip) { return Err(ResumeError::NonCanonicalNrip); }
        let length = exit.nrip.checked_sub(exit.rip).filter(|n| (2..=15).contains(n))
            .ok_or(ResumeError::InvalidInstructionLength)?;
        Ok(Self::Hardware { exit, next: ResumeCandidate { address: exit.nrip, instruction_bytes: length as u8 } })
    }
    pub(crate) fn validate(self, stopped: ExitSnapshot) -> Result<(), ResumeError> {
        match self {
            Self::Bytes(bytes) => stopped.validate_msr_instruction(bytes),
            Self::Hardware { exit, .. } if exit == stopped => Ok(()),
            _ => Err(ResumeError::ExitDoesNotPermitCandidate),
        }
    }
    pub(crate) fn length(self) -> usize {
        match self { Self::Bytes(b) => b.len(), Self::Hardware { next, .. } => next.instruction_bytes as usize }
    }
    pub(crate) fn continuation(self, stopped: ExitSnapshot) -> Result<ResumeCandidate, ResumeError> {
        self.validate(stopped)?;
        match self { Self::Bytes(b) => stopped.msr_continuation(b), Self::Hardware { next, .. } => Ok(next) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitAction {
    /// End this synthetic run; this is not architectural HLT emulation.
    StopOnHlt,
    /// A guest-specific CPUID response policy and saved GPR operands are needed.
    CpuidPolicyRequired,
    /// No hypercall ABI is assumed; a handler must validate saved GPR operands.
    HypercallHandlerRequired,
    /// Pre-instruction I/O stop; no port has an emulation or resume owner.
    IoioRefused(IoRefusal),
    NestedPageFault(NestedPageFault),
    InvalidVmcb,
    /// APM2 15.14.3: terminal; saved guest state is undefined and cannot resume.
    Shutdown,
    Unsupported {
        code: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoDirection {
    Out,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoWidth {
    Byte,
    Word,
    Dword,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoDecodeError {
    NotIoioExit,
    ReservedBits,
    InvalidOperandSize,
}

/// Diagnostic refusal, never architectural completion or guest fault injection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoRefusal {
    /// Every scalar port is unknown to this boundary; no device is emulated.
    UnknownPort(IoIntercept),
    StringOrRep(IoIntercept),
    /// The linear byte span exceeds port FFFFh. Do not wrap to port zero.
    PortSpanOverrun(IoIntercept),
    Malformed(IoDecodeError),
}

/// Reviewed IOIO EXITINFO1 fields (APM2 rev.3.44, 15.10.2, Figure 15-2).
/// This describes the stopped attempt and grants no permission to access a port.
/// String address/segment fields are retained raw, not admitted for execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoIntercept {
    info: u64,
    width: IoWidth,
}

impl IoIntercept {
    pub const fn raw_info(self) -> u64 {
        self.info
    }
    pub const fn port(self) -> u16 {
        (self.info >> 16) as u16
    }
    pub const fn direction(self) -> IoDirection {
        if self.input() {
            IoDirection::In
        } else {
            IoDirection::Out
        }
    }
    pub const fn input(self) -> bool {
        self.info & 1 != 0
    }
    pub const fn width(self) -> IoWidth {
        self.width
    }
    pub const fn width_bytes(self) -> u8 {
        match self.width {
            IoWidth::Byte => 1,
            IoWidth::Word => 2,
            IoWidth::Dword => 4,
        }
    }
    pub const fn string(self) -> bool {
        self.info & (1 << 2) != 0
    }
    pub const fn rep(self) -> bool {
        self.info & (1 << 3) != 0
    }
    /// Raw A64/A32/A16 mask; scalar instructions need no memory address size.
    pub const fn address_size_bits(self) -> u8 {
        ((self.info >> 7) & 7) as u8
    }
    /// Raw SEG field, meaningful for string I/O only and never dereferenced.
    pub const fn segment_bits(self) -> u8 {
        ((self.info >> 10) & 7) as u8
    }
    /// APM2 15.10.1–2 checks consecutive IOPM bits, including its high-port
    /// padding. A span crossing FFFFh is retained and refused, never wrapped.
    pub const fn last_port(self) -> Option<u16> {
        self.port().checked_add(self.width_bytes() as u16 - 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeError {
    ExitDoesNotPermitCandidate,
    NripNotEstablished,
    NonCanonicalRip,
    NonCanonicalNrip,
    InvalidInstructionLength,
    UnsupportedInstructionBytes,
}

/// A numerically plausible sequential address, not authorization to resume.
/// Instruction emulation, register updates, event handling and mapping checks
/// remain the caller's responsibility even when this candidate is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeCandidate {
    address: u64,
    instruction_bytes: u8,
}

impl ResumeCandidate {
    pub const fn address(self) -> u64 {
        self.address
    }
    pub const fn instruction_bytes(self) -> u8 {
        self.instruction_bytes
    }
}

impl ExitSnapshot {
    /// Decode an IOIO pre-instruction exit without interpreting continuation.
    /// APM2 rev.3.44 15.7 and 15.10.2-.3: EXITINFO2 reports the following RIP,
    /// but neither that field nor nRIP authorizes instruction completion.
    /// Figure 15-2 explicitly reserves bits 1 and 15:13. It does not define
    /// EXITINFO1[63:32]; preserve those bits without inventing a zero rule.
    /// Address size and SEG are not validated: scalar I/O has no guest-memory
    /// operand, and every string/REP attempt is refused before emulation.
    pub const fn ioio(self) -> Result<IoIntercept, IoDecodeError> {
        if self.code != 0x7b {
            return Err(IoDecodeError::NotIoioExit);
        }
        if self.info1 & ((7 << 13) | (1 << 1)) != 0 {
            return Err(IoDecodeError::ReservedBits);
        }
        let width = match (self.info1 >> 4) & 7 {
            1 => IoWidth::Byte,
            2 => IoWidth::Word,
            4 => IoWidth::Dword,
            _ => return Err(IoDecodeError::InvalidOperandSize),
        };
        Ok(IoIntercept {
            info: self.info1,
            width,
        })
    }

    /// Actual scalar IOIO exit only: APM2 15.10.2 supplies the next RIP in
    /// EXITINFO2 independently of NRIPS. The native config owner must validate
    /// mode, events and operands before executing hardware and committing it.
    pub(crate) fn ioio_continuation(self) -> Result<ResumeCandidate, ResumeError> {
        if self.code != 0x7b { return Err(ResumeError::ExitDoesNotPermitCandidate); }
        if !crate::memory::address::is_canonical_48(self.rip) { return Err(ResumeError::NonCanonicalRip); }
        if !crate::memory::address::is_canonical_48(self.info2) { return Err(ResumeError::NonCanonicalNrip); }
        let length = self.info2.checked_sub(self.rip)
            .filter(|n| (1..=15).contains(n)).ok_or(ResumeError::InvalidInstructionLength)?;
        Ok(ResumeCandidate { address: self.info2, instruction_bytes: length as u8 })
    }

    /// Exact XSETBV completion for an opt-in stopped xstate owner. APM2 15.7
    /// and APM3 XSETBV: validate architectural operands and pending events
    /// separately before committing either XCR0 or this continuation.
    pub fn xsetbv_continuation(self, instruction: &[u8]) -> Result<ResumeCandidate, ResumeError> {
        if self.code != 0x8d {
            return Err(ResumeError::ExitDoesNotPermitCandidate);
        }
        self.checked_instruction(instruction, &[0x0f, 0x01, 0xd1])
    }

    /// Derive continuation from exactly one unprefixed CPUID or VMMCALL.
    /// The caller must fetch these bytes from this stopped guest's RIP using
    /// its owned mapping and preserve their identity through resumption. This
    /// method cannot establish byte provenance, mappings, or launch readiness.
    /// No nRIP feature or instruction-length fallback is inferred.
    pub fn resume_candidate_from_instruction(
        self,
        instruction: &[u8],
    ) -> Result<ResumeCandidate, ResumeError> {
        let expected: &[u8] = match self.code {
            0x72 => &[0x0f, 0xa2],
            0x81 => &[0x0f, 0x01, 0xd9],
            _ => return Err(ResumeError::ExitDoesNotPermitCandidate),
        };
        self.checked_instruction(instruction, expected)
    }

    /// Scoped MSR emulation only; this does not enable generic MSR dispatch.
    pub(crate) fn msr_continuation(
        self,
        instruction: &[u8],
    ) -> Result<ResumeCandidate, ResumeError> {
        self.validate_msr_instruction(instruction)?;
        self.checked_instruction(instruction, instruction)
    }

    /// Validate the faulting MSR instruction without proposing a next RIP.
    pub(crate) fn validate_msr_instruction(self, instruction: &[u8]) -> Result<(), ResumeError> {
        let expected: &[u8] = match (self.code, self.info1) {
            (0x7c, 0) => &[0x0f, 0x32],
            (0x7c, 1) => &[0x0f, 0x30],
            _ => return Err(ResumeError::ExitDoesNotPermitCandidate),
        };
        if instruction != expected {
            return Err(ResumeError::UnsupportedInstructionBytes);
        }
        if !crate::memory::address::is_canonical_48(self.rip) {
            return Err(ResumeError::NonCanonicalRip);
        }
        Ok(())
    }

    fn checked_instruction(
        self,
        instruction: &[u8],
        expected: &[u8],
    ) -> Result<ResumeCandidate, ResumeError> {
        if instruction != expected {
            return Err(ResumeError::UnsupportedInstructionBytes);
        }
        if !crate::memory::address::is_canonical_48(self.rip) {
            return Err(ResumeError::NonCanonicalRip);
        }
        let address = self
            .rip
            .checked_add(expected.len() as u64)
            .ok_or(ResumeError::InvalidInstructionLength)?;
        if !crate::memory::address::is_canonical_48(address) {
            return Err(ResumeError::NonCanonicalNrip);
        }
        Ok(ResumeCandidate {
            address,
            instruction_bytes: expected.len() as u8,
        })
    }

    /// Decode the reviewed Appendix B fields from an inert VMCB page image.
    /// This neither imports a runnable VMCB nor proves hardware provenance.
    pub fn from_vmcb_bytes(bytes: &[u8; 4096]) -> Self {
        fn field(bytes: &[u8; 4096], offset: usize) -> u64 {
            let mut value = [0; 8];
            value.copy_from_slice(&bytes[offset..offset + 8]);
            u64::from_le_bytes(value)
        }
        Self {
            code: field(bytes, 0x070),
            info1: field(bytes, 0x078),
            info2: field(bytes, 0x080),
            rip: field(bytes, 0x578),
            nrip: field(bytes, 0x0c8),
        }
    }

    pub const fn action(self) -> ExitAction {
        match self.code {
            0x72 => ExitAction::CpuidPolicyRequired,
            0x78 => ExitAction::StopOnHlt,
            0x7b => ExitAction::IoioRefused(match self.ioio() {
                Err(error) => IoRefusal::Malformed(error),
                Ok(io) if io.string() || io.rep() => IoRefusal::StringOrRep(io),
                Ok(io) if io.last_port().is_none() => IoRefusal::PortSpanOverrun(io),
                Ok(io) => IoRefusal::UnknownPort(io),
            }),
            0x7f => ExitAction::Shutdown,
            0x81 => ExitAction::HypercallHandlerRequired,
            0x400 => ExitAction::NestedPageFault(NestedPageFault {
                info: self.info1,
                guest_physical_address: self.info2,
            }),
            u64::MAX => ExitAction::InvalidVmcb,
            code => ExitAction::Unsupported { code },
        }
    }

    /// Accept only a bounded forward nRIP for the two instruction intercepts
    /// that this foundation exposes to future handlers. HLT is terminal here;
    /// faults and unknown exits never yield a resume address. This nRIP path
    /// never falls back to instruction bytes when support is not established.
    pub fn resume_candidate(
        self,
        capabilities: &ValidatedCapabilities,
    ) -> Result<ResumeCandidate, ResumeError> {
        if !matches!(self.code, 0x72 | 0x81) {
            return Err(ResumeError::ExitDoesNotPermitCandidate);
        }
        if !capabilities.optional_features().nrip_save {
            return Err(ResumeError::NripNotEstablished);
        }
        if !crate::memory::address::is_canonical_48(self.rip) {
            return Err(ResumeError::NonCanonicalRip);
        }
        if !crate::memory::address::is_canonical_48(self.nrip) {
            return Err(ResumeError::NonCanonicalNrip);
        }
        let bytes = self
            .nrip
            .checked_sub(self.rip)
            .filter(|length| (1..=15).contains(length))
            .ok_or(ResumeError::InvalidInstructionLength)?;
        Ok(ResumeCandidate {
            address: self.nrip,
            instruction_bytes: bytes as u8,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslationStage {
    Unspecified,
    FinalGuestPhysical,
    GuestPageTable,
    /// Both indication bits set: preserve ambiguity instead of picking a stage.
    Ambiguous,
}

/// Limited baseline view of NPF information. Additional feature-specific bits
/// remain available through raw_info; they are neither rejected nor decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedPageFault {
    info: u64,
    guest_physical_address: u64,
}

impl NestedPageFault {
    pub const fn raw_info(self) -> u64 {
        self.info
    }
    /// Raw reported GPA, not a validated, translated or dereferenceable address.
    pub const fn guest_physical_address(self) -> u64 {
        self.guest_physical_address
    }
    pub const fn present(self) -> bool {
        self.info & 1 != 0
    }
    /// Access at the nested-table level, not necessarily the guest instruction.
    /// A guest page-table walk can require writes for accessed/dirty updates.
    pub const fn write(self) -> bool {
        self.info & (1 << 1) != 0
    }
    pub const fn user(self) -> bool {
        self.info & (1 << 2) != 0
    }
    pub const fn reserved_bit_violation(self) -> bool {
        self.info & (1 << 3) != 0
    }
    pub const fn instruction_fetch(self) -> bool {
        self.info & (1 << 4) != 0
    }
    pub const fn stage(self) -> TranslationStage {
        match (self.info >> 32) & 3 {
            0 => TranslationStage::Unspecified,
            1 => TranslationStage::FinalGuestPhysical,
            2 => TranslationStage::GuestPageTable,
            _ => TranslationStage::Ambiguous,
        }
    }
}

#[cfg(test)]
mod msr_evidence_tests {
    use super::*;
    #[test]
    fn retained_hardware_evidence_rejects_stale_exit_before_fault_or_completion() {
        let exit=ExitSnapshot { code:0x7c,info1:1,info2:0,rip:0x2000,nrip:0x2003 };
        let evidence=MsrInstruction::Hardware { exit,next:ResumeCandidate { address:0x2003,instruction_bytes:3 } };
        assert!(evidence.validate(exit).is_ok());
        for changed in [ExitSnapshot { nrip:0x2004,..exit },ExitSnapshot { rip:0x2001,..exit },
            ExitSnapshot { info1:0,..exit },ExitSnapshot { code:0x72,..exit }] {
            assert!(evidence.validate(changed).is_err());
            assert!(evidence.continuation(changed).is_err());
        }
    }
}
