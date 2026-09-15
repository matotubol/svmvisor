//! Partial guest APIC mode/MSR fixture over the existing controller.
//!
//! AMD APM vol.2 rev.3.44 16.9–16.11/Table16-6, 16.6.4 and 15.11.
//! The caller explicitly admits BSP0 or AP1 with base FEE00000h and a valid mode.
//! This is not a general APIC/reset implementation. Default CPUID remains
//! unchanged and does not advertise complete APIC/x2APIC support. All accesses
//! must be intercepted; no host MSR is read/written. The separate bounded
//! xAPIC MMIO adapter shares this owner and its admitted timer-only LVT.
//! Deterministic supplied-source timer ticks are not physical scheduling;
//! general CR emulation and base relocation remain unsupported. The opt-in ICR
//! path supports cold AP INIT/SIPI and fixed unicast through `ipi::IpiTarget`;
//! running-CPU INIT and physical RESET remain unsupported.
//! One separate bounded MOV CR8 write handler synchronizes the same TPR owner.
//! GeneralProtectionRequired is a stopped outcome,
//! not evidence that #GP was injected into a guest.
use super::{
    events::ExternalInterruptError,
    exit::ResumeError,
    local_apic::{Error, LocalApic, TickOutcome},
    vmcb::Vmcb,
};
use crate::arch::x86_64::registers::GuestRegisters;

pub const FIXTURE_APIC_BASE: u64 = 0xfee0_0d00;
const FIXTURE_BASE_ADDRESS: u64 = 0xfee0_0000;
const BASE_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const BASE_BSC: u64 = 1 << 8;
const BASE_MODE_MASK: u64 = 3 << 10;
const BASE_ALLOWED_MASK: u64 = BASE_ADDRESS_MASK | BASE_BSC | BASE_MODE_MASK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApicMode {
    Disabled,
    XApic,
    X2Apic,
}

impl ApicMode {
    fn from_base(base: u64) -> Option<Self> {
        match (base & BASE_MODE_MASK) >> 10 {
            0 => Some(Self::Disabled),
            2 => Some(Self::XApic),
            3 => Some(Self::X2Apic),
            _ => None,
        }
    }
    const fn bits(self) -> u64 {
        match self {
            Self::Disabled => 0,
            Self::XApic => 1 << 11,
            Self::X2Apic => BASE_MODE_MASK,
        }
    }
}

/// Admission errors describe supplied fixture state, not a guest exception.
#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionError {
    InvalidBase,
    UnsupportedBaseAddress,
    UnsupportedBootstrapCpu,
    ArmedController,
}

#[derive(Debug, PartialEq, Eq)]
pub enum QueueError {
    /// The bounded monitor source cannot inject while APIC is disabled.
    /// This is a policy refusal, not an architectural guest fault.
    Disabled,
    Controller(Error),
}

/// Owns mode metadata and the existing, sole interrupt-state owner together.
/// No mutable controller escape is exposed: delivery always passes mode gating.
#[derive(Debug, PartialEq, Eq)]
pub struct FixtureApic {
    controller: LocalApic,
    mode: ApicMode,
    identity: u8,
    icr: u64,
}

impl FixtureApic {
    /// Admit the full guest APIC_BASE value, never a sampled host MSR.
    /// Caller supplies the matching stopped controller with no armed delivery.
    /// `FIXTURE_APIC_BASE` admits a BSP already in x2APIC mode; clearing its
    /// AE/EXTD bits admits Disabled, and clearing only EXTD admits xAPIC.
    /// This transfers retained state; it does not perform RESET or INIT.
    /// Rejected admission returns the unchanged controller to its caller.
    pub fn admit_fixed_bsp(
        controller: LocalApic,
        base: u64,
    ) -> Result<Self, (AdmissionError, LocalApic)> {
        Self::admit_fixed_cpu(controller, base, 0)
    }
    /// Admit the fixture's BSP0 or AP1; identity and BSC must agree.
    /// The controller remains the only IRR/ISR/timer owner for this vCPU.
    pub fn admit_fixed_cpu(
        controller: LocalApic,
        base: u64,
        identity: u8,
    ) -> Result<Self, (AdmissionError, LocalApic)> {
        let Some(mode) = ApicMode::from_base(base) else {
            return Err((AdmissionError::InvalidBase, controller));
        };
        if base & !BASE_ALLOWED_MASK != 0 {
            return Err((AdmissionError::InvalidBase, controller));
        }
        if base & BASE_ADDRESS_MASK != FIXTURE_BASE_ADDRESS {
            return Err((AdmissionError::UnsupportedBaseAddress, controller));
        }
        if identity > 1 || (base & BASE_BSC != 0) != (identity == 0) {
            return Err((AdmissionError::UnsupportedBootstrapCpu, controller));
        }
        if controller.delivery_armed() {
            return Err((AdmissionError::ArmedController, controller));
        }
        Ok(Self {
            controller,
            mode,
            identity,
            icr: 0,
        })
    }
    pub fn controller(&self) -> &LocalApic {
        &self.controller
    }
    pub fn mode(&self) -> ApicMode {
        self.mode
    }
    pub fn apic_base(&self) -> u64 {
        FIXTURE_BASE_ADDRESS | if self.identity == 0 { BASE_BSC } else { 0 } | self.mode.bits()
    }
    /// Admitted guest identity, independent of host topology and APIC mode.
    pub const fn identity(&self) -> u32 {
        self.identity as u32
    }
    pub fn icr(&self) -> u64 {
        self.icr
    }
    pub(crate) fn set_icr(&mut self, value: u64) {
        self.icr = value;
    }
    /// Only the cold, never-entered AP startup path may reset modeled APIC state.
    /// APM2 Table16-2/16.10: retain APIC mode/identity, clear ICR, SVR=ffh.
    pub(crate) fn initialize_cold_ap(&mut self) {
        self.controller = LocalApic::admit_enabled();
        self.controller.write_spurious_vector(0xff).unwrap();
        self.icr = 0;
    }
    /// Shared register semantics only: adapters own bus validation and ID encoding.
    /// AMD APM vol.2 rev.3.44 16.6.3–4, Table16-2. Reject holes and bound
    /// bitmap indexes here before reaching the controller's word accessors.
    pub(crate) fn read_register(&self, offset: u16) -> Option<u32> {
        match offset {
            // APM2 16.3.4: one admitted LVT => MLE=0; no extended registers.
            // This explicit partial fixture is not a physical AMD APIC model.
            0x30 => Some(0x10),
            0xf0 => Some(self.controller.spurious_vector_register()),
            0x320 => Some(self.controller.timer_lvt()),
            0x380 => Some(self.controller.timer_initial()),
            0x390 => Some(self.controller.timer_remaining()),
            0x3e0 => Some(self.controller.timer_divide()),
            0x80 => Some(self.controller.task_priority() as u32),
            0xa0 => Some(self.controller.processor_priority() as u32),
            0x100..=0x170 if offset & 15 == 0 => Some(
                self.controller
                    .service_word(((offset - 0x100) / 16) as usize),
            ),
            0x200..=0x270 if offset & 15 == 0 => Some(
                self.controller
                    .pending_word(((offset - 0x200) / 16) as usize),
            ),
            _ => None,
        }
    }
    pub(crate) fn write_tpr(&mut self, vmcb: &mut Vmcb, tpr: u8) -> Result<(), Error> {
        self.controller.write_guest_tpr(vmcb, tpr)
    }
    pub(crate) fn write_eoi(&mut self) -> Result<Option<u8>, Error> {
        self.controller.eoi()
    }
    /// Shared SVR/timer operations; each bus owns operand and fault taxonomy.
    pub(crate) fn write_register(&mut self, offset: u16, value: u32) -> Result<(), Error> {
        match offset {
            0xf0 => self.controller.write_spurious_vector(value),
            0x320 => self.controller.write_timer_lvt(value),
            0x380 => self.controller.write_timer_initial(value),
            0x3e0 => self.controller.write_timer_divide(value),
            _ => unreachable!("adapters select admitted writable registers"),
        }
    }
    /// Disabled APIC_BASE freezes the admitted timer as fixture retention policy.
    /// ASE clear instead lets it count while forced masking suppresses inputs.
    pub fn advance_timer(&mut self, source_ticks: u64) -> Result<TickOutcome, QueueError> {
        if self.mode == ApicMode::Disabled {
            return Err(QueueError::Disabled);
        }
        self.controller
            .advance_timer(source_ticks)
            .map_err(QueueError::Controller)
    }
    pub fn queue(&mut self, vector: u8) -> Result<bool, QueueError> {
        if self.mode == ApicMode::Disabled {
            return Err(QueueError::Disabled);
        }
        self.controller
            .queue(vector)
            .map_err(QueueError::Controller)
    }
    /// Retain pending state while disabled, without arming V_IRQ.
    pub fn arm(&mut self, vmcb: &mut Vmcb) -> Result<Option<u8>, Error> {
        if self.mode == ApicMode::Disabled {
            return Ok(None);
        }
        self.controller.arm(vmcb)
    }
    /// Account for the same real entry/exit required by `LocalApic::observe`.
    pub fn observe(&mut self, vmcb: &Vmcb) -> Result<Option<u8>, Error> {
        self.controller.observe(vmcb)
    }
    /// Return an armed but unconsumed V_IRQ to this controller's IRR after a
    /// real exit. Call only after `observe` returned None for that same exit;
    /// never use this to cancel an entry which has not executed. This reuses
    /// the scheduler's existing deferral before local APIC/mailbox mutation.
    pub fn defer_after_exit(&mut self, vmcb: &mut Vmcb) -> Result<u8, Error> {
        self.controller.defer_after_exit(vmcb)
    }
    fn prepare_base(&self, value: u64) -> Result<ApicMode, MsrError> {
        // APM vol.3 rev.3.37 WRMSR p512: writing MBZ bits requires #GP(0).
        let next = ApicMode::from_base(value).ok_or(MsrError::GeneralProtectionRequired)?;
        if value & !BASE_ALLOWED_MASK != 0
            || matches!(
                (self.mode, next),
                (ApicMode::Disabled, ApicMode::X2Apic) | (ApicMode::X2Apic, ApicMode::XApic)
            )
        {
            return Err(MsrError::GeneralProtectionRequired);
        }
        // Figure 16-31 declares ABA R/W and BSC RO. Relocation and changing
        // the supplied BSC bit are outside this fixed-identity policy; do not
        // fabricate #GP for either. Physical-address-width policy is absent.
        if value & BASE_ADDRESS_MASK != FIXTURE_BASE_ADDRESS
            || (value & BASE_BSC != 0) != (self.identity == 0)
        {
            return Err(MsrError::Unsupported {
                index: 0x1b,
                write: true,
            });
        }
        if self.controller.delivery_armed() {
            return Err(MsrError::Controller(Error::Armed));
        }
        // 16.9.1 retains the modeled registers when enabling x2APIC. A mode
        // write is not RESET/INIT (16.10). Disable/re-enable retention is a
        // bounded fixture policy: the cited AMD text does not specify every
        // register's behavior on AE disable. It is not hardware fidelity proof.
        // Keep the sole owner intact and gate delivery while disabled.
        Ok(next)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MsrError {
    Ipi(super::ipi::IpiError),
    Continuation(ResumeError),
    PendingState(ExternalInterruptError),
    Controller(Error),
    /// Architectural #GP(0) is required. Stop without mutation; reflection is
    /// not implemented by this partial MSR surface.
    GeneralProtectionRequired,
    Unsupported {
        index: u32,
        write: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Cr8Error {
    Continuation(ResumeError),
    PendingState(ExternalInterruptError),
    Controller(Error),
    UnsupportedGuestMode,
    /// Normal privilege/operand exceptions precede the CR8 intercept (APM
    /// vol.2 rev.3.44 Table15-7). These inputs contradict a completed exit18;
    /// refuse them rather than synthesizing a fault from inconsistent evidence.
    UnsupportedPrivilege,
    InvalidOperand {
        value: u64,
    },
}

/// Emulate the fixture's exact REX.R MOV CR8, r64 at a real exit18.
/// The caller owns matching immutable bytes fetched at this stopped guest's
/// RIP, the VMCB, the complete saved GPR frame and the admitted APIC owner.
/// Observe any armed delivery first. No host register access is performed.
///
/// APM vol.3 rev.3.37 MOV(CRn), pp426–427, and vol.2 rev.3.44 16.6.4:
/// source is the full 64-bit register, valid values are 0..15, flags and all
/// GPRs are unchanged, and TPR becomes source << 4 (including zero subclass).
/// All checks precede mutation; unsupported state retains RIP for diagnosis.
/// Actual invalid-operand/CPL instructions fault before exit18 (Table15-7)
/// and belong to the existing checked exception-reflection path instead.
pub fn handle_fixture_cr8_write(
    apic: &mut FixtureApic,
    vmcb: &mut Vmcb,
    frame: &GuestRegisters,
    instruction: &[u8],
) -> Result<(), Cr8Error> {
    let snapshot = vmcb.exit_snapshot();
    let source = snapshot
        .cr8_write_source(instruction)
        .map_err(Cr8Error::Continuation)?;
    if !vmcb.guest_in_64_bit_code() {
        return Err(Cr8Error::UnsupportedGuestMode);
    }
    vmcb.validate_external_interrupt_conflicts()
        .map_err(Cr8Error::PendingState)?;
    vmcb.validate_virtual_interrupt_controls()
        .map_err(Cr8Error::PendingState)?;
    // The fixture leaves reads unintercepted: V_INTR_MASKING must select
    // the owned V_TPR for those reads rather than the physical APIC TPR.
    if vmcb.virtual_interrupt_control() & (1 << 24) == 0 {
        return Err(Cr8Error::PendingState(
            ExternalInterruptError::UnsupportedControl {
                control: vmcb.virtual_interrupt_control(),
            },
        ));
    }
    if vmcb.virtual_interrupt_control() & (1 << 8) != 0 {
        return Err(Cr8Error::PendingState(
            ExternalInterruptError::PendingVirtualInterrupt,
        ));
    }
    if apic.controller.delivery_armed() {
        return Err(Cr8Error::Controller(Error::Armed));
    }
    if vmcb.virtual_interrupt_control() & 0xf != (apic.controller.task_priority() >> 4) as u64 {
        return Err(Cr8Error::Controller(Error::TaskPriorityMismatch));
    }
    if vmcb.bytes()[0x4cb] != 0 {
        return Err(Cr8Error::UnsupportedPrivilege);
    }
    // VMRUN/VMEXIT own RAX and RSP in the VMCB; there are deliberately no
    // competing copies in GuestRegisters. The remaining fourteen use the frame.
    let value = match source {
        0 => vmcb.guest_rax(),
        1 => frame.rcx,
        2 => frame.rdx,
        3 => frame.rbx,
        4 => vmcb.guest_rsp(),
        5 => frame.rbp,
        6 => frame.rsi,
        7 => frame.rdi,
        8 => frame.r8,
        9 => frame.r9,
        10 => frame.r10,
        11 => frame.r11,
        12 => frame.r12,
        13 => frame.r13,
        14 => frame.r14,
        15 => frame.r15,
        _ => unreachable!(),
    };
    if value > 15 {
        return Err(Cr8Error::InvalidOperand { value });
    }
    let next = snapshot
        .cr8_write_continuation(instruction)
        .map_err(Cr8Error::Continuation)?;
    apic.controller
        .write_guest_tpr(vmcb, (value as u8) << 4)
        .map_err(Cr8Error::Controller)?;
    vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
    Ok(())
}

/// Emulate one exact unprefixed RDMSR/WRMSR at a caller-established real exit.
/// The same stopped guest owns `apic`, `vmcb`, saved GPRs and instruction bytes.
/// Observe any armed delivery before calling. Successful writes preserve GPRs;
/// reads zero-extend EDX:EAX. All fallible checks precede state changes.
pub fn handle_fixture_msr(
    apic: &mut FixtureApic,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
) -> Result<(), MsrError> {
    handle_fixture_msr_inner(apic, vmcb, frame, instruction, None)
}

/// The same checked MSR adapter with one exclusively borrowed remote vCPU.
/// Delivery and source ICR/RIP commit are atomic on refusal. No self/broadcast
/// routing is inferred from the supplied target. See `ipi::IpiTarget`.
pub fn handle_fixture_msr_with_target(
    apic: &mut FixtureApic,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    target: &mut super::ipi::IpiTarget<'_>,
) -> Result<(), MsrError> {
    handle_fixture_msr_inner(apic, vmcb, frame, instruction, Some(target))
}

/// Same checked adapter, publishing fixed IPIs or admitted cold INIT/SIPI to
/// the target's transport. After success with target.published(), the caller must
/// unconditionally kick the target host CPU. No remote VMCB/APIC is borrowed.
pub fn handle_fixture_msr_with_mailbox(
    apic: &mut FixtureApic,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    target: &mut super::ipi::MailboxTarget<'_>,
) -> Result<(), MsrError> {
    handle_fixture_msr_inner(apic, vmcb, frame, instruction, Some(target))
}

pub(crate) fn handle_fixture_msr_inner(
    apic: &mut FixtureApic,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    target: Option<&mut dyn super::ipi::IpiRoute>,
) -> Result<(), MsrError> {
    let snapshot = vmcb.exit_snapshot();
    snapshot
        .validate_msr_instruction(instruction)
        .map_err(MsrError::Continuation)?;
    vmcb.validate_external_interrupt_conflicts()
        .map_err(MsrError::PendingState)?;
    vmcb.validate_virtual_interrupt_controls()
        .map_err(MsrError::PendingState)?;
    // Appendix B CPL field; an intercepted privileged instruction still needs
    // its architectural privilege check before emulation.
    if vmcb.bytes()[0x4cb] != 0 {
        return Err(MsrError::GeneralProtectionRequired);
    }
    if vmcb.virtual_interrupt_control() & 0xf != (apic.controller.task_priority() >> 4) as u64 {
        return Err(MsrError::Controller(Error::TaskPriorityMismatch));
    }
    let index = frame.rcx as u32;
    let write = snapshot.info1 == 1;
    let input = ((frame.rdx as u32 as u64) << 32) | vmcb.guest_rax() as u32 as u64;
    let unsupported = || MsrError::Unsupported { index, write };
    // 16.11 gates the entire architectural range, including registers whose
    // semantics this fixture does not implement, while outside x2APIC mode.
    if (0x800..=0x8ff).contains(&index)
        && (apic.mode != ApicMode::X2Apic || !architectural_x2apic_register(index))
    {
        return Err(MsrError::GeneralProtectionRequired);
    }
    let mut rax = vmcb.guest_rax();
    let mut rdx = frame.rdx;
    if write && vmcb.virtual_interrupt_control() & (1 << 8) != 0 {
        return Err(MsrError::PendingState(
            ExternalInterruptError::PendingVirtualInterrupt,
        ));
    }
    enum WriteAction {
        Icr(u64),
        Base(ApicMode),
        Tpr(u8),
        Eoi,
        Register(u16, u32),
    }
    let action = if write {
        Some(match index {
            0x830 => {
                super::ipi::validate_x2apic_bits(input)
                    .map_err(|_| MsrError::GeneralProtectionRequired)?;
                WriteAction::Icr(input)
            }
            0x1b => WriteAction::Base(apic.prepare_base(input)?),
            0x808 if input <= 0xff => WriteAction::Tpr(input as u8),
            0x80b if input == 0 => WriteAction::Eoi,
            // APM2 16.11.3 requires #GP for high32 or MBZ bits, not for a
            // legal low vector or a live reconfiguration outside this fixture.
            0x80f if input & !0x3ff == 0 => WriteAction::Register(0xf0, input as u32),
            0x832 if input & !0x310ff == 0 => {
                if input & (1 << 12) != 0 {
                    return Err(unsupported());
                }
                WriteAction::Register(0x320, input as u32)
            }
            0x838 if input <= u32::MAX as u64 => WriteAction::Register(0x380, input as u32),
            0x83e if input & !0xb == 0 => WriteAction::Register(0x3e0, input as u32),
            0x803 | 0x80f | 0x832 | 0x838 | 0x839 | 0x83e => {
                return Err(MsrError::GeneralProtectionRequired);
            }
            0x802 | 0x808 | 0x80a | 0x80b | 0x810..=0x817 | 0x820..=0x827 => {
                return Err(MsrError::GeneralProtectionRequired);
            }
            _ => return Err(unsupported()),
        })
    } else {
        None
    };
    if !write {
        let value = match index {
            0x830 => apic.icr(),
            0x1b => apic.apic_base(),
            0x802 => apic.identity() as u64,
            0x803
            | 0x808
            | 0x80a
            | 0x80f
            | 0x810..=0x817
            | 0x820..=0x827
            | 0x832
            | 0x838
            | 0x839
            | 0x83e => apic
                .read_register(((index - 0x800) << 4) as u16)
                .ok_or_else(unsupported)? as u64,
            0x80b => return Err(MsrError::GeneralProtectionRequired),
            _ => return Err(unsupported()),
        };
        rax = value as u32 as u64;
        rdx = (value >> 32) as u32 as u64;
    }
    // A fault retains RIP and does not require a sequential continuation.
    // Successful emulation must validate it before mutating either owner.
    let next = snapshot
        .msr_continuation(instruction)
        .map_err(MsrError::Continuation)?;
    match action {
        Some(WriteAction::Icr(value)) => {
            let target = target.ok_or_else(unsupported)?;
            target.deliver(apic, value).map_err(MsrError::Ipi)?;
            apic.set_icr(value);
        }
        Some(WriteAction::Base(mode)) => {
            // APM2 16.9.1 does not preserve ICR high when entering x2APIC.
            if apic.mode == ApicMode::XApic && mode == ApicMode::X2Apic {
                apic.icr &= 0xffff_ffff;
            }
            apic.mode = mode;
        }
        Some(WriteAction::Tpr(tpr)) => {
            // The helper updates V_TPR before assigning controller TPR, with
            // all fallible checks preceding either mutation.
            apic.write_tpr(vmcb, tpr).map_err(MsrError::Controller)?;
        }
        Some(WriteAction::Eoi) => {
            apic.write_eoi().map_err(MsrError::Controller)?;
        }
        Some(WriteAction::Register(offset, value)) => {
            apic.write_register(offset, value)
                .map_err(|error| match error {
                    Error::UnsupportedTimerVector
                    | Error::TimerRunning
                    | Error::ReadOnlyTimerStatus => unsupported(),
                    other => MsrError::Controller(other),
                })?;
        }
        None => (),
    }
    vmcb.commit_emulated_instruction(rax, next);
    frame.rdx = rdx;
    Ok(())
}

/// Table 16-6 / 16.11.1: the remaining slots are reserved and require #GP.
/// A listed register can still be unsupported by this partial fixture.
fn architectural_x2apic_register(index: u32) -> bool {
    matches!(
        index,
        0x802..=0x803
            | 0x808..=0x80b
            | 0x80d
            | 0x80f..=0x828
            | 0x830
            | 0x832..=0x839
            | 0x83e..=0x842
            | 0x848..=0x853
    )
}
