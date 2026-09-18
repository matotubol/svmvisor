//! Transactional synthetic instruction emulation over retained state.
//! Caller must supply the snapshot and frame from the same stopped guest.
//! RIP equality detects a stale instruction, not hardware provenance. No CPU
//! entry, interrupt delivery, state capture or physical teardown happens here.

use crate::{
    arch::x86_64::{capabilities::ValidatedCapabilities, registers::GuestRegisters},
    svm::{
        emulation::{self, HypercallAction},
        exit::{ExitAction, ExitSnapshot, ResumeCandidate, ResumeError},
        vmcb::Vmcb,
    },
};

const EFER_SVME: u64 = 1 << 12;
const NATIVE_EFER_MASK: u64 = 0xd01;

/// Native CPUID hides SVM and SVM-Lock. APM2 15.31 defines SVMDIS as
/// read-only without SVM-Lock; target LOCK reads zero and ignores writes
/// (PPR57896 pp215 and27 Table8).
/// This is the virtual unavailable-SVM profile, not the physical reset value.
/// It is invariant across guest INIT and has no mutable or live host backing.
pub const NATIVE_VM_CR_VALUE: u64 = crate::arch::x86_64::msr::VM_CR_SVMDIS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Stored state is prepared; host/guest entry prerequisites still apply.
    ResumePrepared,
    /// Instruction did not complete; #GP(0) is queued at the original RIP.
    /// Caller must account for EVENTINJ after the actual guest entry.
    GeneralProtectionPrepared,
    Stop(StopReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Exit(ExitAction),
    Requested,
    UnsupportedHypercall { opcode: u64 },
}

/// Logical native EFER, separate from the VMCB's mandatory SVME backing.
/// Admission matches the native long-mode continuation, not a general CPU
/// model. APM2 rev3.44 3.1.7, 14.6.2/Table14-5, 15.5 and 15.11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEfer {
    logical: u64,
    native_features: u64,
    unsupported_features: u64,
    nx_supported: bool,
    startup_owned: bool,
}

impl NativeEfer {
    pub fn admit(original_efer: u64, nx_supported: bool) -> Result<Self, NativeEferError> {
        Self::admit_mask(original_efer, nx_supported, 0, 0)
    }

    /// Same-CPU native CPUID evidence: Fn80000001 ECX/EDX, Fn80000008 EBX and optional
    /// Fn80000021 EAX, read only when that leaf is enumerated. The caller owns
    /// direct guest FXSR/TLB execution and VMRUN/VMEXIT EFER switching; this
    /// constructor does not establish that execution contract itself.
    /// PPR57896 rev3.00 pp87-88/117/186; APM2 rev3.44 pp57-58/68/502/507.
    pub fn admit_native(
        original_efer: u64,
        extended_ecx: u32,
        extended_edx: u32,
        extended8_ebx: u32,
        extended21_eax: Option<u32>,
    ) -> Result<Self, NativeEferError> {
        let features = if extended_edx & (1 << 25) != 0 { 1 << 14 } else { 0 }
            | if extended_ecx & (1 << 17) != 0 { 1 << 15 } else { 0 }
            | if extended8_ebx & (1 << 13) != 0 { 1 << 18 } else { 0 }
            | if extended21_eax.is_some_and(|eax| eax & (1 << 7) != 0) { 1 << 20 } else { 0 }
            | if extended21_eax.is_some_and(|eax| eax & (1 << 8) != 0) { 1 << 21 } else { 0 };
        // APM2 p55: enabling a processor-absent feature faults. Keep that
        // distinct from a supported feature outside our execution coverage.
        // PPR99: LMSLE unsupported is explicit MBZ; MCOMMIT has its own gate.
        let available = features
            | if extended_edx & (1 << 11) != 0 { 1 } else { 0 }
            | if extended_edx & (1 << 29) != 0 { 1 << 8 } else { 0 }
            | if extended_edx & (1 << 20) != 0 { 1 << 11 } else { 0 }
            | if extended8_ebx & (1 << 8) != 0 { 1 << 17 } else { 0 };
        let unsupported = ((1 | (1 << 8) | (1 << 11) | (1 << 14) | (1 << 15)
            | (1 << 17) | (1 << 18) | (1 << 20) | (1 << 21)) & !available)
            | (1 << 12) // Native virtual CPUID does not expose nested SVM.
            | if extended8_ebx & (1 << 20) != 0 { 1 << 13 } else { 0 };
        Self::admit_mask(original_efer, extended_edx & (1 << 20) != 0, features, unsupported)
    }

    fn admit_mask(
        original_efer: u64,
        nx_supported: bool,
        native_features: u64,
        unsupported_features: u64,
    ) -> Result<Self, NativeEferError> {
        if original_efer & !(NATIVE_EFER_MASK | native_features) != 0
            || original_efer & unsupported_features != 0
            || original_efer & 0x500 != 0x500
            || (!nx_supported && original_efer & 0x800 != 0)
        {
            return Err(NativeEferError::UnsupportedInitialState);
        }
        Ok(Self {
            logical: original_efer,
            native_features,
            unsupported_features,
            nx_supported,
            startup_owned: false,
        })
    }

    pub const fn logical(&self) -> u64 {
        self.logical
    }

    /// Opt in only with target-owned INIT and the native startup IRQ owner.
    /// This permits legacy unpaged execution; paging/segment fetch admission
    /// remains the caller's responsibility. APM2 14.6 and 15.13.1.
    pub fn enable_guest_startup(&mut self) {
        self.startup_owned = true;
    }

    /// Target owner calls after committing the matching VMCB INIT state.
    /// INIT clears logical EFER; mandatory SVM backing remains private.
    pub fn reset_after_init(&mut self) -> Result<(), NativeEferError> {
        if !self.startup_owned {
            return Err(NativeEferError::UnsupportedMode);
        }
        self.logical = 0;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMsrOutcome {
    Completed,
    /// Fault retry at the original RIP. Delivery has not yet executed.
    GeneralProtectionPrepared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    SnapshotRipMismatch,
    CpuStateMismatch,
    Resume(ResumeError),
    CpuModel(super::cpu_model::CpuModelError),
    PendingState(super::events::ExternalInterruptError),
    UnsupportedDebugState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeEferError {
    UnsupportedInitialState,
    BackingMismatch,
    UnsupportedMode,
    UnsupportedDebugState,
    UnsupportedMsr {
        index: u32,
        write: bool,
    },
    /// Legal or feature-dependent settings outside the current owner are a
    /// terminal refusal, never an invented architectural #GP.
    UnsupportedValue {
        value: u64,
    },
    Instruction(ResumeError),
    PendingState(super::events::ExternalInterruptError),
    Fault(super::events::MsrFaultError),
}

pub type NativeVmCrError = NativeEferError;

/// Exact unprefixed EFER RDMSR/WRMSR, using this stopped guest's owned bytes.
/// No hardware MSR access occurs. Writes preserve all GPRs; reads zero-extend
/// EDX:EAX. Refusal preserves the owner, VMCB and frame. Guest faults only
/// queue #GP(0), preserving RIP and registers; caller must account for EVENTINJ
/// after actual entry before another dispatch. Other intercepted MSRs stop.
pub fn handle_native_efer(
    owner: &mut NativeEfer,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
) -> Result<NativeMsrOutcome, NativeEferError> {
    native_efer_inner(owner, vmcb, frame, super::exit::MsrInstruction::Bytes(instruction))
}

/// Complete a hardware-decoded EFER MSR using same-CPU NRIPS evidence.
/// Caller supplies the actual exclusively stopped native VMCB/frame and its
/// CPU capabilities. Common opcode/CPL faults precede MSR interception; EFER
/// value faults still belong to the shared semantic owner (APM2 15.11).
/// No instruction memory is read and no opcode bytes are fabricated.
pub fn handle_native_efer_with_nrip(
    owner: &mut NativeEfer,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    caps: &ValidatedCapabilities,
) -> Result<NativeMsrOutcome, NativeEferError> {
    let evidence = hardware_msr_instruction(vmcb, caps)?;
    native_efer_inner(owner, vmcb, frame, evidence)
}

pub fn handle_native_vmcr(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    startup_owned: bool,
) -> Result<NativeMsrOutcome, NativeVmCrError> {
    native_vmcr_inner(vmcb, frame, super::exit::MsrInstruction::Bytes(instruction), startup_owned)
}

/// Caller supplies the actual exclusively stopped native VMCB/frame and
/// same-CPU validated capabilities. Hardware NRIP requires long64/CPL0;
/// rejected hardware evidence never falls back to fabricated opcode bytes.
pub fn handle_native_vmcr_with_nrip(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    caps: &ValidatedCapabilities,
    startup_owned: bool,
) -> Result<NativeMsrOutcome, NativeVmCrError> {
    let evidence = hardware_msr_instruction(vmcb, caps)?;
    native_vmcr_inner(vmcb, frame, evidence, startup_owned)
}

/// Complete one native CPUID with a response sampled on the owning CPU while
/// stopped. The host leaves guest XCR0 live and owns no FP state. Topology and
/// native capabilities must have been admitted separately; this is not SMP
/// admission or a host CPUID query. Pending/interrupted delivery stays stopped.
pub fn handle_native_cpuid(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    native_response: [u32; 4],
) -> Result<DispatchOutcome, DispatchError> {
    let snapshot = vmcb.exit_snapshot();
    native_cpuid_inner(vmcb, frame, native_response, false, false, || {
        snapshot.resume_candidate_from_instruction(instruction)
    })
}

/// Opt-in startup owner: exact CPUID in long64 or legacy unpaged code, with
/// physical INTR masking owned separately. No generic legacy paging admission.
pub fn handle_native_startup_cpuid(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    native_response: [u32; 4],
) -> Result<DispatchOutcome, DispatchError> {
    if !native_startup_instruction_mode(vmcb, instruction.len()) {
        return Err(DispatchError::CpuStateMismatch);
    }
    let snapshot = vmcb.exit_snapshot();
    native_cpuid_inner(vmcb, frame, native_response, true, false, || {
        snapshot.resume_candidate_from_instruction(instruction)
    })
}

/// Complete native CPUID using hardware instruction provenance, including
/// legal prefixes and CPL0-3 long64/compatibility code. Legacy unpaged code
/// additionally requires startup ownership. Segment/IP wrap remains unsupported.
/// The VMCB must be the exclusively stopped hardware exit from the same CPU
/// whose admitted capabilities are supplied here. APM2 rev.3.44 15.7.1,
/// 15.9/Table15-7 and Appendix C identify CPUID and its sequential nRIP.
/// APM3 rev.3.37 CPUID pp.171-173 defines the two-byte opcode and HWCR bit35
/// user fault. `cpuid_user_disabled` must be the same CPU's live HWCR bit35;
/// CPUID interception alone does not prove the control is clear. APM3 1.1/1.2
/// bounds decoded instruction length to 15. For lengths above two the caller
/// must supply exact coherently fetched stopped instruction bytes: APM2 15.7
/// does not unambiguously establish illegal LOCK priority for CPUID. The
/// reviewed prefixes exclude LOCK, REP, and non-final/non-64-bit REX. Two-byte
/// CPUID needs no guest-memory read. Unsupported modes and
/// invalid/missing nRIP remain stopped; callers must
/// not substitute instruction bytes after this hardware path rejects an exit.
/// `startup_owned` requires the same separately admitted physical interrupt
/// masking ownership as `handle_native_startup_cpuid`.
pub fn handle_native_cpuid_with_nrip(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    capabilities: &ValidatedCapabilities,
    native_response: [u32; 4],
    startup_owned: bool,
    cpuid_user_disabled: bool,
    prefixed_instruction: Option<&[u8]>,
) -> Result<DispatchOutcome, DispatchError> {
    let snapshot = vmcb.exit_snapshot();
    let next = snapshot.resume_candidate(capabilities).map_err(DispatchError::Resume)?;
    if next.instruction_bytes() < 2 {
        return Err(DispatchError::Resume(ResumeError::InvalidInstructionLength));
    }
    if !native_cpuid_mode(vmcb, next.address(), startup_owned) {
        return Err(DispatchError::CpuStateMismatch);
    }
    if next.instruction_bytes() > 2 || prefixed_instruction.is_some() {
        let Some(bytes) = prefixed_instruction else {
            return Err(DispatchError::Resume(ResumeError::UnsupportedInstructionBytes));
        };
        let prefix_len = bytes.len().saturating_sub(2);
        if bytes.len() != next.instruction_bytes() as usize
            || bytes.get(prefix_len..) != Some(&[0x0f, 0xa2][..])
            || !bytes[..prefix_len].iter().enumerate().all(|(index, byte)| {
                matches!(byte, 0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67)
                    || (vmcb.guest_in_64_bit_code()
                        && index + 1 == prefix_len
                        && (0x40..=0x4f).contains(byte))
            })
        {
            return Err(DispatchError::Resume(ResumeError::UnsupportedInstructionBytes));
        }
    }
    native_cpuid_inner(vmcb, frame, native_response, startup_owned, cpuid_user_disabled, || {
        Ok(next)
    })
}

/// Every fallible check occurs before either the frame or VMCB is changed.
/// Terminal exits and rejected calls leave both inputs unchanged.
pub fn handle_exit(
    snapshot: ExitSnapshot,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    capabilities: &ValidatedCapabilities,
) -> Result<DispatchOutcome, DispatchError> {
    dispatch(
        snapshot,
        vmcb,
        frame,
        |leaf, subleaf| Ok(emulation::cpuid(leaf, subleaf)),
        || snapshot.resume_candidate(capabilities),
    )
}

/// Dispatch with exact unprefixed instruction bytes instead of CPU nRIP.
/// Caller must fetch the bytes from this stopped guest's RIP through an owned
/// mapping and keep them unchanged until entry. No guest-memory reads occur
/// here. Terminal actions leave state unchanged and need no continuation.
pub fn handle_exit_with_instruction(
    snapshot: ExitSnapshot,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
) -> Result<DispatchOutcome, DispatchError> {
    dispatch(
        snapshot,
        vmcb,
        frame,
        |leaf, subleaf| Ok(emulation::cpuid(leaf, subleaf)),
        || snapshot.resume_candidate_from_instruction(instruction),
    )
}

/// Opt-in clock CPUID policy using the same transactional stopped-state path.
/// The runtime must install this plan's clock state before every guest entry.
pub fn handle_exit_with_instruction_and_clock(
    snapshot: ExitSnapshot,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    clock: &crate::arch::x86_64::clock::ClockPlan,
) -> Result<DispatchOutcome, DispatchError> {
    dispatch(
        snapshot,
        vmcb,
        frame,
        |leaf, subleaf| Ok(emulation::cpuid_with_clock(leaf, subleaf, clock)),
        || snapshot.resume_candidate_from_instruction(instruction),
    )
}

/// Execute the admitted AMD policy through the same transactional dispatcher.
/// The caller owns this CPU's stopped state and installs the matching XCR0,
/// clock, backing and xstate contract before every entry. A model answer alone
/// does not grant entry or establish guest-memory instruction provenance.
pub fn handle_exit_with_cpu_model(
    snapshot: ExitSnapshot,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    model: &super::cpu_model::AmdCpuModel,
    state: super::cpu_model::GuestCpuState,
) -> Result<DispatchOutcome, DispatchError> {
    // CR4 is hardware-saved VMCB state, not a caller-selected feature switch.
    if snapshot.action() == ExitAction::Shutdown {
        return Ok(DispatchOutcome::Stop(StopReason::Exit(ExitAction::Shutdown)));
    }
    let cr4 = u64::from_le_bytes(vmcb.bytes()[0x548..0x550].try_into().unwrap());
    if state.cr4 != cr4 {
        return Err(DispatchError::CpuStateMismatch);
    }
    dispatch(
        snapshot,
        vmcb,
        frame,
        |leaf, subleaf| model.cpuid(leaf, subleaf, state).map_err(DispatchError::CpuModel),
        || snapshot.resume_candidate_from_instruction(instruction),
    )
}

pub(crate) fn hardware_msr_instruction(
    vmcb: &Vmcb,
    caps: &ValidatedCapabilities,
) -> Result<super::exit::MsrInstruction<'static>, NativeEferError> {
    let b = vmcb.bytes();
    let cr0 = u64::from_le_bytes(b[0x558..0x560].try_into().unwrap());
    let cr4 = u64::from_le_bytes(b[0x548..0x550].try_into().unwrap());
    let cs = u16::from_le_bytes(b[0x412..0x414].try_into().unwrap());
    if !vmcb.guest_in_64_bit_code()
        || b[0x4cb] != 0
        || cs & 0x600 != 0x200
        || cr0 & 0x8000_0001 != 0x8000_0001
        || cr4 & (1 << 5) == 0
        || cr4 & (1 << 12) != 0
        || vmcb.guest_rflags() & (1 << 17) != 0
    {
        return Err(NativeEferError::UnsupportedMode);
    }
    let evidence = super::exit::MsrInstruction::hardware(vmcb.exit_snapshot(), caps)
        .map_err(NativeEferError::Instruction)?;
    Ok(evidence)
}

pub(crate) fn validate_native_msr_boundary(
    vmcb: &Vmcb,
    instruction: super::exit::MsrInstruction<'_>,
    startup_owned: bool,
) -> Result<(), NativeEferError> {
    use NativeEferError as E;
    let snapshot = vmcb.exit_snapshot();
    instruction.validate(snapshot).map_err(E::Instruction)?;
    vmcb.validate_external_interrupt_conflicts().map_err(E::PendingState)?;
    validate_native_interrupt_profile(vmcb, startup_owned).map_err(E::PendingState)?;
    Ok(())
}

/// nRIP is an instruction offset, not CS.base + offset (APM2 15.7.1).
/// Retain the existing 48-bit native profile and checked, nonwrapping legacy
/// continuation. Compat segmentation remains enabled (APM2 1.3/Table1-1, 2.3).
pub(crate) fn native_cpuid_mode(vmcb: &Vmcb, next: u64, startup_owned: bool) -> bool {
    let b = vmcb.bytes();
    let cr0 = u64::from_le_bytes(b[0x558..0x560].try_into().unwrap());
    let cr4 = u64::from_le_bytes(b[0x548..0x550].try_into().unwrap());
    let efer = u64::from_le_bytes(b[0x4d0..0x4d8].try_into().unwrap());
    let cs = u16::from_le_bytes(b[0x412..0x414].try_into().unwrap());
    let cpl = b[0x4cb];
    if cpl > 3 || cs & 0x98 != 0x98 || vmcb.guest_rflags() & (1 << 17) != 0 {
        return false;
    }
    if efer & (1 << 10) != 0 {
        if efer & (1 << 8) == 0
            || cr0 & 0x8000_0001 != 0x8000_0001
            || cr4 & (1 << 5) == 0
            || cr4 & (1 << 12) != 0
        {
            return false;
        }
        if cs & 0x200 != 0 {
            return cs & 0x400 == 0;
        }
    } else if !startup_owned || cr0 & (1 << 31) != 0 || cs & 0x200 != 0 || cr0 & 1 == 0 && cpl != 0
    {
        return false;
    }
    let limit = u32::from_le_bytes(b[0x414..0x418].try_into().unwrap()) as u64;
    let ip_limit = if cs & 0x400 != 0 { u32::MAX as u64 } else { u16::MAX as u64 };
    next <= limit.min(ip_limit)
}

/// Bounded sequential legacy completion: refuse segment/IP wrap rather than
/// applying long64 RIP arithmetic to a 16/32-bit instruction. Actual opcode
/// provenance and physical backing are checked by the caller's fetch owner.
pub(crate) fn native_startup_instruction_mode(vmcb: &Vmcb, length: usize) -> bool {
    let b = vmcb.bytes();
    let cr0 = u64::from_le_bytes(b[0x558..0x560].try_into().unwrap());
    let efer = u64::from_le_bytes(b[0x4d0..0x4d8].try_into().unwrap());
    let cs = u16::from_le_bytes(b[0x412..0x414].try_into().unwrap());
    if vmcb.guest_in_64_bit_code() {
        return true;
    }
    if cr0 & (1 << 31) != 0
        || efer & (1 << 10) != 0
        || cs & 0x200 != 0
        || cs & 0x98 != 0x98
        || vmcb.guest_rflags() & (1 << 17) != 0
    {
        return false;
    }
    let limit = u32::from_le_bytes(b[0x414..0x418].try_into().unwrap()) as u64;
    let ip_limit = if cs & 0x400 != 0 { u32::MAX as u64 } else { u16::MAX as u64 };
    vmcb.guest_rip().checked_add(length as u64).is_some_and(|next| next <= limit.min(ip_limit))
}

fn native_efer_inner(
    owner: &mut NativeEfer,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: super::exit::MsrInstruction<'_>,
) -> Result<NativeMsrOutcome, NativeEferError> {
    use NativeEferError as E;
    let snapshot = vmcb.exit_snapshot();
    validate_native_msr_boundary(vmcb, instruction, owner.startup_owned)?;
    let backing = u64::from_le_bytes(vmcb.bytes()[0x4d0..0x4d8].try_into().unwrap());
    let cr0 = u64::from_le_bytes(vmcb.bytes()[0x558..0x560].try_into().unwrap());
    // A direct guest MOV CR0 derives LMA in hardware. Only that read-only bit
    // may differ from the last owned EFER write; validate it before committing
    // a new logical observation. Failed emulation leaves the owner unchanged.
    let logical = if owner.startup_owned {
        let expected_lma =
            if cr0 & (1 << 31) != 0 && owner.logical & (1 << 8) != 0 { 1 << 10 } else { 0 };
        (owner.logical & !(1 << 10)) | expected_lma
    } else {
        owner.logical
    };
    if backing != (logical | EFER_SVME) {
        return Err(E::BackingMismatch);
    }
    let index = frame.rcx as u32;
    let write = snapshot.info1 == 1;
    if index != 0xc000_0080 {
        return Err(E::UnsupportedMsr { index, write });
    }
    let input = ((frame.rdx as u32 as u64) << 32) | vmcb.guest_rax() as u32 as u64;
    // Fig3-9 MBZ is distinct from RAZ7:1. LMA writes must preserve hardware
    // state; changing LME while PG=1 faults (Table14-5). Unadmitted defined
    // controls, SVME and RAZ writes are explicitly outside this owner.
    const MBZ: u64 = !0x0036_fdff;
    let fault = vmcb.bytes()[0x4cb] != 0
        || (write
            && (input & (MBZ | owner.unsupported_features) != 0
                || (input ^ logical) & (1 << 10) != 0
                || ((input ^ logical) & (1 << 8) != 0 && cr0 & (1 << 31) != 0)));
    if fault {
        vmcb.queue_validated_msr_general_protection(instruction).map_err(E::Fault)?;
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    }
    if (!owner.startup_owned && (cr0 & (1 << 31) == 0 || backing & 0x500 != 0x500))
        || (owner.startup_owned && !native_startup_instruction_mode(vmcb, instruction.length()))
    {
        return Err(E::UnsupportedMode);
    }
    // Target PPR57896 p186 permits OS enable of FFXSE once, then preservation.
    // Refuse clearing an enabled bit outside target-owned INIT; no invented #GP.
    // Other admitted controls use hardware's guest EFER; VMEXIT restores host
    // EFER (APM2 pp502/507). Direct guest INVLPG retains hardware TCE semantics.
    if write
        && (input & !(NATIVE_EFER_MASK | owner.native_features) != 0
            || (!owner.nx_supported && input & 0x800 != 0)
            || (logical & (1 << 14) != 0 && input & (1 << 14) == 0))
    {
        return Err(E::UnsupportedValue { value: input });
    }
    if vmcb.guest_rflags() & (1 << 8) != 0 {
        return Err(E::UnsupportedDebugState);
    }
    let next = instruction.continuation(snapshot).map_err(E::Instruction)?;
    if write {
        if input != logical {
            vmcb.commit_native_efer(input);
        }
        owner.logical = input;
        vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
    } else {
        vmcb.commit_emulated_instruction(logical as u32 as u64, next);
        frame.rdx = logical >> 32;
        owner.logical = logical;
    }
    vmcb.complete_native_instruction_state();
    Ok(NativeMsrOutcome::Completed)
}

fn native_vmcr_inner(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: super::exit::MsrInstruction<'_>,
    startup_owned: bool,
) -> Result<NativeMsrOutcome, NativeVmCrError> {
    use NativeEferError as E;
    validate_native_msr_boundary(vmcb, instruction, startup_owned)?;
    let snapshot = vmcb.exit_snapshot();
    let write = snapshot.info1 == 1;
    let index = frame.rcx as u32;
    if index != crate::arch::x86_64::msr::VM_CR {
        return Err(E::UnsupportedMsr { index, write });
    }
    let input = ((frame.rdx as u32 as u64) << 32) | vmcb.guest_rax() as u32 as u64;
    // APM2 Fig15-27 MBZ63:5; APM3 WRMSR faults on MBZ writes. Faults
    // precede unsupported target Reserved0/2 or unowned guest R_INIT1.
    if vmcb.bytes()[0x4cb] != 0 || (write && input & !0x1f != 0) {
        vmcb.queue_validated_msr_general_protection(instruction).map_err(E::Fault)?;
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    }
    if (!startup_owned && !vmcb.guest_in_64_bit_code())
        || (startup_owned && !native_startup_instruction_mode(vmcb, instruction.length()))
    {
        return Err(E::UnsupportedMode);
    }
    // PPR57896 p215: bits0/2 Reserved (write-as-read), R_INIT controls #SX.
    // Guest #SX redirection is not owned; never pass this write to host VM_CR.
    if write && input & 7 != 0 {
        return Err(E::UnsupportedValue { value: input });
    }
    if vmcb.guest_rflags() & (1 << 8) != 0 {
        return Err(E::UnsupportedDebugState);
    }
    let next = instruction.continuation(snapshot).map_err(E::Instruction)?;
    if write {
        vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
    } else {
        vmcb.commit_emulated_instruction(NATIVE_VM_CR_VALUE, next);
        frame.rdx = 0;
    }
    vmcb.complete_native_instruction_state();
    Ok(NativeMsrOutcome::Completed)
}

fn native_cpuid_inner(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    native_response: [u32; 4],
    startup_owned: bool,
    cpuid_user_disabled: bool,
    continuation: impl FnOnce() -> Result<ResumeCandidate, ResumeError>,
) -> Result<DispatchOutcome, DispatchError> {
    vmcb.validate_external_interrupt_conflicts().map_err(DispatchError::PendingState)?;
    validate_native_interrupt_profile(vmcb, startup_owned).map_err(DispatchError::PendingState)?;
    let snapshot = vmcb.exit_snapshot();
    if snapshot.code != 0x72 {
        return Err(DispatchError::Resume(ResumeError::ExitDoesNotPermitCandidate));
    }
    if vmcb.guest_rflags() & (1 << 8) != 0 {
        return Err(DispatchError::UnsupportedDebugState);
    }
    let next = continuation().map_err(DispatchError::Resume)?;
    if cpuid_user_disabled && vmcb.bytes()[0x4cb] != 0 {
        vmcb.queue_native_general_protection().map_err(DispatchError::PendingState)?;
        return Ok(DispatchOutcome::GeneralProtectionPrepared);
    }
    let cr4 = u64::from_le_bytes(vmcb.bytes()[0x548..0x550].try_into().unwrap());
    let outcome = dispatch(
        snapshot,
        vmcb,
        frame,
        |leaf, _| Ok(super::cpu_model::native_boot_cpuid(leaf, native_response, cr4)),
        || Ok(next),
    )?;
    vmcb.complete_native_instruction_state();
    Ok(outcome)
}

fn validate_native_interrupt_profile(
    vmcb: &Vmcb,
    startup_owned: bool,
) -> Result<(), super::events::ExternalInterruptError> {
    let control = vmcb.virtual_interrupt_control();
    if control & super::x2avic::ENABLE_BITS != 0 {
        if !startup_owned {
            return Err(super::events::ExternalInterruptError::ControlMismatch);
        }
        // Native entry separately checks the exact retained profile addresses.
        // This strict encoded validator does not broaden synthetic IRQ profiles.
        return vmcb.validate_native_x2avic_controls();
    }
    vmcb.validate_virtual_interrupt_controls()?;
    let permitted = 0xf | if startup_owned { 1 << 24 } else { 0 };
    if control & !permitted != 0 {
        return Err(super::events::ExternalInterruptError::ControlMismatch);
    }
    Ok(())
}

fn dispatch(
    snapshot: ExitSnapshot,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    policy: impl FnOnce(u32, u32) -> Result<[u32; 4], DispatchError>,
    continuation: impl FnOnce() -> Result<ResumeCandidate, ResumeError>,
) -> Result<DispatchOutcome, DispatchError> {
    // APM15.14.3 makes the saved guest state undefined on shutdown. Do not
    // interpret RIP or request instruction bytes before returning this stop.
    if snapshot.action() == ExitAction::Shutdown {
        return Ok(DispatchOutcome::Stop(StopReason::Exit(ExitAction::Shutdown)));
    }
    if snapshot.rip != vmcb.guest_rip() {
        return Err(DispatchError::SnapshotRipMismatch);
    }
    let mut updated = *frame;
    let rax = match snapshot.action() {
        ExitAction::CpuidPolicyRequired => {
            let [leaf, subleaf] = frame.cpuid_inputs(vmcb.guest_rax());
            updated.apply_cpuid(policy(leaf, subleaf)?)
        }
        ExitAction::HypercallHandlerRequired => match emulation::hypercall(vmcb.guest_rax()) {
            HypercallAction::Query { abi_version } => abi_version,
            HypercallAction::Stop => return Ok(DispatchOutcome::Stop(StopReason::Requested)),
            HypercallAction::Unsupported { opcode } => {
                return Ok(DispatchOutcome::Stop(StopReason::UnsupportedHypercall { opcode }));
            }
        },
        action => return Ok(DispatchOutcome::Stop(StopReason::Exit(action))),
    };
    let next = continuation().map_err(DispatchError::Resume)?;
    vmcb.commit_emulated_instruction(rax, next);
    *frame = updated;
    Ok(DispatchOutcome::ResumePrepared)
}
