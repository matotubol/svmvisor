//! Stop reasons and their wire encodings: fetch, MSR, SYS_CFG, x2AVIC and startup
//! refusals, and the exported stop words. No encoder reads guest memory or hardware.

/// Stable software diagnostic codes; no additional guest reads are performed.
#[repr(u16)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FetchReadFailure {
    MemoryMap = 0x30,
    AddressPolicy = 0x31,
    MonitorRange = 0x32,
    HostPat = 0x33,
    MtrrCapture = 0x34,
    ReadShape = 0x35,
    RamAdmission = 0x36,
    FixedMtrrControl = 0x37,
    FixedMtrrRange = 0x38,
    PhysicalMemoryNotWb = 0x39,
    ScratchAliasOccupied = 0x3a,
    MemoryControlBusy = 0x3b,
}

/// Resident x2AVIC stop reasons: the low 16 bits of a stopped `info1`. A
/// reason with a code keeps that code in its low nibble; bits 63:16 carry
/// the detail listed per reason. The ordinary stop record (event 3) and the
/// terminal context export carry both words; `stop_words` exports these
/// stops as an unhandled exit with the guest RIP, except on an NPF exit
/// (400h), where it exports `info2` in the GPA field (kind 3), so a
/// dispatch-entry `ProfileMismatch` on an NPF reads as GPA 2 there.
///
/// Retired, never reused: F501h-F504h (capture steps), F505h (shared route
/// table), F511h (APIC_BASE change), F522h (level completion), F523h/F524h
/// (shared-route level completion), F530h/F531h (register backend). Images
/// built before this batch (HEAD eafe33a and earlier) also emitted F510h
/// with an MSR index in `info2` (now F541h) and F520h with the raw
/// EXITINFO2 of an unhandled AVIC exit (now F580h/F581h); decode those tags
/// by image.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X2AvicStop {
    /// The bounded acceptance helper returned a value above 255.
    /// `info2` = that value.
    AcceptedVector = 0xf500,
    /// Intercepted x2APIC or APIC_BASE MSR boundary. `info2`: 0 guest APIC
    /// owner missing, 1 x2AVIC profile or pending event, 2 instruction
    /// evidence, 3 instruction mode or TF, 4 #GP(0) could not be queued.
    MsrBoundary = 0xf510,
    /// Exit without the armed x2AVIC VMCB profile. `info2`: 0 profile or IPI
    /// owner missing, 1 AVIC exit with a changed profile, 2 dispatch entry
    /// with a changed profile, 3 a self-targeted NMI IPI could not set V_NMI.
    /// Detail (`info2` 2 only): VMCB offset 60h bits 47:0 as the exit left it
    /// (`profile_mismatch_at_entry`).
    ProfileMismatch = 0xf520,
    /// The startup router refused an INIT/SIPI IPI (`startup_route_refusal`).
    /// `info2` = EXITINFO1 (ICR).
    StartupRoute = 0xf521,
    /// Register-owner refusal; code = `registers::Refusal` (1-7). Detail: MSR
    /// index, bit 48 set for WRMSR. `info2` = the WRMSR EDX:EAX (0 for RDMSR).
    Register = 0xf540,
    /// AVIC_INCOMPLETE_IPI refusal; code = `ipi::IpiRefusal`. Detail:
    /// EXITINFO2 index (bits 27:16) and ID (bits 59:28). `info2` = EXITINFO1.
    Ipi = 0xf550,
    /// Fixed-IPI fan-out stopped before any publication: a remote target ID
    /// is not a doorbell target. Detail: slot | ID << 8. `info2` = EXITINFO1.
    FanOutDoorbell = 0xf560,
    /// Fixed-IPI fan-out publication refused; lower slots were published and
    /// doorbelled. Detail: slot | `x2avic_error_code` << 8. `info2` = EXITINFO1.
    FanOutPublication = 0xf561,
    /// Host IRQ bridge failure; code = `irq_error_code` variant (11 is the
    /// ExtINT signature). Detail: `IrqSite`. `info2` = `irq_error_code`.
    Irq = 0xf570,
    /// AVIC_NOACCEL outside the D1 interception profile (anything but a
    /// level-triggered EOI write). Detail: EXITINFO2 bits 31:0.
    /// `info2` = EXITINFO1.
    NoAcceleration = 0xf580,
    /// AVIC exit with an ID or EOI vector the decoder refuses. Detail:
    /// EXITINFO2 bits 31:0. `info2` = EXITINFO1.
    UndecodableAvicExit = 0xf581,
}

/// Where the host IRQ bridge failed.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqSite {
    /// Physical INTR (exit 60h) capture.
    Capture = 0,
    /// Intercepted guest EOI write (exit 7Ch).
    SoftwareEoi = 1,
    /// AVIC_NOACCEL level-triggered EOI (exit 402h).
    LevelEoiExit = 2,
}

/// Startup service stages (reason 7 of `startup_failure`). Stages 10 and 11
/// carry `init_error_code`; every other stage carries 1 for AwaitSipi and 0
/// for Running. Stage 3 (current APIC mode) and 7 (ICR INIT reset) are
/// retired xAPIC-era values; 14 was the retired guest-INIT refusal. Stages
/// 2, 4 and 8 can follow an applied command (terminal: the command stays
/// queued).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupStage {
    InitAcknowledgment = 1,
    /// Route table unusable, or its lease not acquired within
    /// `ROUTE_WAIT_ATTEMPTS` after the command was applied.
    RouteTable = 2,
    ModeCommitPreparation = 4,
    TargetApplication = 5,
    EferReset = 6,
    MailboxCompletion = 8,
    /// The core lease of cache replay stayed busy for the bounded retry.
    WaitExhausted = 9,
    /// Read-only guest INIT LAPIC preparation refused; nothing changed.
    InitPreparation = 10,
    /// Guest INIT LAPIC commit failed after its physical reset (terminal).
    InitLapicCommit = 11,
    /// Guest INIT CPU-state commit refused after the LAPIC commit (terminal).
    InitCpuCommit = 12,
    CacheReplay = 13,
    /// An owner that arm always installs is missing.
    OwnerMissing = 15,
}

pub fn fetch_failure_code(error: crate::host::resident::fetch::FetchError) -> u16 {
    use super::fetch::FetchError as F;
    use crate::host::paging::WalkError as W;
    match error {
        F::UnsupportedExit => 1,
        F::UnsupportedMode => 2,
        F::AddressOverflow => 3,
        F::UnsupportedCacheControl => 4,
        F::NotExecutable => 5,
        F::PrivilegeMismatch => 6,
        F::NonWriteBackInstruction => 7,
        F::UnreadableInstruction { .. } => 8,
        F::SegmentLimit => 9,
        F::Walk(w) => match w {
            W::UnsupportedPhysicalWidth => 0x10,
            W::FiveLevelUnsupported => 0x11,
            W::NoncanonicalAddress => 0x12,
            W::InvalidCr3 => 0x13,
            W::UnreadableTable { level, .. } => 0x14 | ((level as u16) << 8),
            W::NotPresent { level } => 0x15 | ((level as u16) << 8),
            W::ReservedEntry { level } => 0x16 | ((level as u16) << 8),
            W::UnsupportedEntryBits { level } => 0x17 | ((level as u16) << 8),
            W::OneGiBUnsupported => 0x18,
            W::IncompleteWalk => 0x19,
        },
    }
}

/// Stable EFER diagnostic, derived only from the already stopped state.
/// The payload is one full-width operand; it does not export the whole VMCB.
pub fn efer_failure(
    error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::svm::vmcb::Vmcb,
    logical: u64,
    instruction_bytes: [u8; 2],
) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, Some(instruction_bytes), 0xf108)
}

/// Hardware NRIP diagnostics contain actual continuation evidence, never invented bytes.
pub fn efer_nrip_failure(
    error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::svm::vmcb::Vmcb,
    logical: u64,
) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, None, 0xf108)
}

/// VM_CR uses the same stopped-instruction errors but a distinct register identity.
pub fn vmcr_failure(
    error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::svm::vmcb::Vmcb,
    logical: u64,
    instruction_bytes: [u8; 2],
) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, Some(instruction_bytes), 0xf109)
}

pub fn vmcr_nrip_failure(
    error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::svm::vmcb::Vmcb,
    logical: u64,
) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, None, 0xf109)
}

/// SYSCFG stage85 carries both DWORD operands when possible, otherwise one
/// explicitly typed full-width value. No diagnostic path reads guest memory.
pub fn syscfg_failure(
    error: crate::svm::syscfg::SyscfgError,
    vmcb: &crate::svm::vmcb::Vmcb,
    instruction: Option<[u8; 2]>,
) -> (u64, u64) {
    use crate::svm::syscfg::SyscfgError as E;
    let write = vmcb.exit_snapshot().info1 == 1;
    let (reason, requested, current) = match error {
        E::Boundary(error) => {
            let (tag, value) = msr_failure_context(error, vmcb, 0, instruction, 0);
            return syscfg_context((tag >> 16) & 255, 3, write, value);
        }
        E::UnsupportedProfile { signature, physical_bits } => {
            return syscfg_context(
                0x80,
                3,
                write,
                u64::from(signature) | (u64::from(physical_bits) << 32),
            );
        }
        E::CurrentReserved { requested, current } => (0x81, requested, current),
        E::CurrentEncryption { requested, current } => (0x82, requested, current),
        E::RequestedReserved { requested, current } => (0x83, requested, current),
        E::UnsupportedChange { requested, current } => (0x84, requested, current),
    };
    syscfg_operands(reason, write, requested, current)
}

pub fn syscfg_operands(reason: u64, write: bool, requested: u64, current: u64) -> (u64, u64) {
    if current > u32::MAX as u64 {
        syscfg_context(reason, 2, write, current)
    } else if requested > u32::MAX as u64 {
        syscfg_context(reason, 1, write, requested)
    } else {
        syscfg_context(reason, 0, write, requested | (current << 32))
    }
}

/// Guest-register refusal: the MSR, the direction and the refused value
/// (`registers::Emulation::Refused`) survive.
pub fn register_refusal(
    reason: crate::svm::x2avic::registers::Refusal,
    msr: u32,
    write: bool,
    value: u64,
) -> (u64, u64) {
    let detail = u64::from(msr) | (u64::from(write) << 32);
    (x2avic_stop(X2AvicStop::Register, reason as u8, detail), value)
}

/// Incomplete-IPI refusal with the raw EXITINFO1/EXITINFO2 (Tables
/// 15-25/15-26).
pub fn ipi_refusal(
    refusal: crate::svm::x2avic::ipi::IpiRefusal,
    info1: u64,
    info2: u64,
) -> (u64, u64) {
    let detail = (info2 & 0xfff) | ((info2 >> 32) << 12);
    (x2avic_stop(X2AvicStop::Ipi, refusal as u8, detail), info1)
}

/// Fixed-IPI fan-out failure; `info1` is the exit's EXITINFO1 (ICR).
pub fn fan_out_failure(error: crate::svm::x2avic::ipi::FanOutError, info1: u64) -> (u64, u64) {
    use crate::svm::x2avic::ipi::FanOutError as F;
    let (reason, detail) = match error {
        F::DoorbellTarget { slot, id } => {
            (X2AvicStop::FanOutDoorbell, slot as u64 | (u64::from(id) << 8))
        }
        F::Publication { slot, error } => (
            X2AvicStop::FanOutPublication,
            slot as u64 | (u64::from(x2avic_error_code(error)) << 8),
        ),
    };
    (x2avic_stop(reason, 0, detail), info1)
}

/// Host IRQ bridge failure at `site`.
pub fn irq_failure(site: IrqSite, error: crate::svm::x2avic::irq::IrqError) -> (u64, u64) {
    let code = irq_error_code(error);
    (x2avic_stop(X2AvicStop::Irq, (code >> 17) as u8, site as u64), u64::from(code))
}

/// Startup-router refusal of an incomplete INIT/SIPI (`StartupRoute`).
/// Detail bits 3:0 hold the route predicate (0 when none was recorded);
/// when the router recorded a recipient, bit 4 is set, bits 9:5 hold its
/// INIT count (saturated at 31), bits 11:10 the cause of its last
/// destination record (0 observed, 1 guest control, 2 guest INIT), bits
/// 15:12 its destination mode (0 not ready, 4 x2APIC) and bits 47:16 its
/// x2APIC ID. `info2` = EXITINFO1.
pub fn startup_route_refusal(
    failure: Option<crate::svm::x2avic::startup::NativeRouteFailure>,
    info1: u64,
) -> (u64, u64) {
    let mut detail = failure.map_or(0, |failure| failure.predicate as u64);
    if let Some(recipient) = failure.and_then(|failure| failure.recipient) {
        detail |= 1 << 4
            | u64::from(recipient.init_count.min(31)) << 5
            | (recipient.cause as u64) << 10
            | recipient.mode.map_or(0, |mode| mode as u64) << 12
            | u64::from(recipient.identity) << 16;
    }
    (x2avic_stop(X2AvicStop::StartupRoute, 0, detail), info1)
}

/// AVIC exit refused with its raw exit information (`NoAcceleration` or
/// `UndecodableAvicExit`).
/// Dispatch-entry `ProfileMismatch`: the processor writes offset 60h back on
/// #VMEXIT (APM2 rev3.44 15.6 p507), so the record names what it left there.
/// Bits 63:40 of that word are reserved (Table B-1 p741) and do not survive.
pub fn profile_mismatch_at_entry(control: u64) -> (u64, u64) {
    (x2avic_stop(X2AvicStop::ProfileMismatch, 0, control & 0xffff_ffff_ffff), 2)
}

pub fn avic_exit_refusal(reason: X2AvicStop, info1: u64, info2: u64) -> (u64, u64) {
    (x2avic_stop(reason, 0, info2 & 0xffff_ffff), info1)
}

/// Guest INIT LAPIC failure as the 32-bit value of startup service stages 10
/// and 11: bits 31:28 = 1 with `x2avic_error_code` in bits 7:0 (backing page
/// identity), or 2 with `irq_error_code` in bits 20:0 (physical sources).
pub fn init_error_code(error: crate::svm::x2avic::registers::InitError) -> u32 {
    use crate::svm::x2avic::registers::InitError as E;
    match error {
        E::Backing(error) => (1 << 28) | x2avic_error_code(error),
        E::Irq(error) => (2 << 28) | irq_error_code(error),
    }
}

/// One 32-bit operand for a host IRQ bridge error: bits 20:17 the variant
/// (1 reserved vector, 2 physical ISR mismatch, 3 duplicate source,
/// 4 ambiguous level source, 5 unowned completion, 6 completion not ready,
/// 7 unexpected physical ISR, 8 virtual publication, 9 virtual ISR mismatch,
/// 10 drain incomplete, 11 accepted vector not in service), bits 16:8 the
/// second vector (100h = none), bits 7:0 the vector (0 for variant 10).
pub fn irq_error_code(error: crate::svm::x2avic::irq::IrqError) -> u32 {
    use crate::svm::x2avic::irq::IrqError as E;
    let (variant, vector, other) = match error {
        E::ReservedVector(vector) => (1, vector, None),
        E::PhysicalIsrMismatch { vector, highest } => (2, vector, highest),
        E::DuplicatePhysicalSource(vector) => (3, vector, None),
        E::AmbiguousLevelSource(vector) => (4, vector, None),
        E::UnownedLevelCompletion(vector) => (5, vector, None),
        E::CompletionNotReady(vector) => (6, vector, None),
        E::UnexpectedPhysicalIsr(vector) => (7, vector, None),
        E::VirtualPublication(vector) => (8, vector, None),
        E::VirtualIsrMismatch { vector, highest } => (9, vector, highest),
        E::DrainIncomplete => (10, 0, None),
        E::NotInService(vector) => (11, vector, None),
    };
    (variant << 17) | (other.map_or(0x100, u32::from) << 8) | u32::from(vector)
}

/// `svm::x2avic::Error` as a small code (1-11 in declaration order).
pub fn x2avic_error_code(error: crate::svm::x2avic::Error) -> u32 {
    use crate::svm::x2avic::Error as E;
    match error {
        E::MissingCapability => 1,
        E::Address(_) => 2,
        E::InvalidId => 3,
        E::AliasedPages => 4,
        E::Occupied => 5,
        E::InvalidOffset => 6,
        E::InvalidVector => 7,
        E::MixedTrigger => 8,
        E::UnsupportedVersion => 9,
        E::InvalidExit => 10,
        E::UnsupportedApicBase => 11,
    }
}

pub fn startup_pending_failure(
    error: crate::svm::events::ExternalInterruptError,
    identity: u32,
) -> (u64, u64) {
    let (reason, value) = pending(error);
    startup_failure(6, reason as u8, value, identity)
}

/// Target-local startup evidence. Wide observations retain all 64 bits and
/// explicitly omit CPU identity. No diagnostic performs another hardware read.
pub fn startup_failure(reason: u8, detail: u8, value: u64, identity: u32) -> (u64, u64) {
    let wide = value > u32::MAX as u64;
    let code = reason as u64 | ((detail as u64) << 3) | (u64::from(wide) << 10);
    (0xf10c | (code << 16), if wide { value } else { value | ((identity as u64) << 32) })
}

/// Select one explicitly typed full-width context; full stop state remains local.
/// Kinds 8 and 13 belonged to retired xAPIC MMIO stops and kind 14 to the
/// retired startup-route record; they are never exported now, and only the
/// offline snapshot decoder still reads them from old images.
pub fn stop_words(slot: usize, code: u64, rip: u64, info1: u64, info2: u64) -> Option<[u32; 3]> {
    if slot >= 32 {
        return None;
    }
    if code == 0x7c && info1 & 0xffff == 0xf10d {
        let d = info1 >> 16;
        let reason = d & 255;
        let mode = (d >> 8) & 3;
        if d > 0x7ff
            || !is_valid_syscfg_reason(reason)
            || (mode == 3 && reason > 0x80)
            || (mode != 3 && (!(0x81..=0x85).contains(&reason) || d & 0x400 == 0))
        {
            return None;
        }
        return Some([
            0x1000_0085 | ((slot as u32) << 8) | ((d as u32) << 13),
            info2 as u32,
            (info2 >> 32) as u32,
        ]);
    }
    let (exit, kind, value) = if info1 & 0xffff == 0xf10c {
        let d = info1 >> 16;
        if d > 0x7ff || d & 7 == 0 {
            return None;
        }
        (d as u32, 15, info2)
    } else if code == 0x7c && matches!(info1 & 0xffff, 0xf108 | 0xf109) {
        let diagnostic = info1 >> 16;
        let reason = diagnostic & 0x1ff;
        let vmcr = info1 & 0xffff == 0xf109;
        // The fixed VM_CR profile has no initial-state or backing-mismatch error.
        if vmcr && matches!(reason, 1 | 2) {
            return None;
        }
        if diagnostic >> 9 > 2
            || !((1..=6).contains(&reason)
                || (0x10..=0x15).contains(&reason)
                || (0x20..=0x2d).contains(&reason)
                || (0x30..=0x35).contains(&reason)
                || (0x40..=0x4d).contains(&reason)
                || (0x50..=0x54).contains(&reason)
                || (0x60..=0x64).contains(&reason))
        {
            return None;
        }
        (diagnostic as u32, if vmcr { 12 } else { 11 }, info2)
    } else if code > 0x7ff {
        (0x7ff, 4, code)
    } else {
        let (kind, value) = match info1 {
            0xf001 => detailed_fetch(rip, info2).map_or((1, rip), |v| (10, v)),
            0xf102 => (5, rip),
            0xf103 => (6, rip),
            0xf104 => (2, info2),
            0xf105 => (7, rip),
            0xf107 => (9, info2),
            // A runtime context mismatch carries a host pointer, never a GPA.
            0xf10a => (0, rip),
            _ if code == 0x400 => (3, info2),
            _ => (0, rip),
        };
        (code as u32, kind, value)
    };
    Some([
        0x1000_0083 | ((slot as u32) << 8) | (exit << 13) | (kind << 24),
        value as u32,
        (value >> 32) as u32,
    ])
}

/// kind10 preserves canonical48 RIP and exact software predicate in 64 bits.
/// Noncanonical RIP falls back to the original full-width RIP-only record.
fn detailed_fetch(rip: u64, failure: u64) -> Option<u64> {
    let low = rip & 0xffff_ffff_ffff;
    let canonical = ((low << 16) as i64 >> 16) as u64;
    let base = failure & 255;
    let level = failure >> 8;
    let valid = if (0x14..=0x17).contains(&base) {
        (1..=4).contains(&level)
    } else {
        level == 0
            && ((1..=9).contains(&base)
                || (0x10..=0x13).contains(&base)
                || (0x18..=0x19).contains(&base)
                || (0x30..=0x3a).contains(&base))
    };
    (canonical == rip && valid).then_some(low | (failure << 48))
}

fn msr_failure_context(
    error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::svm::vmcb::Vmcb,
    logical: u64,
    instruction_bytes: Option<[u8; 2]>,
    tag: u64,
) -> (u64, u64) {
    use crate::svm::{
        dispatch::NativeEferError as E, events::MsrFaultError as M, exit::ResumeError as R,
    };
    let info = vmcb.exit_snapshot().info1;
    let instruction_value = |e| match e {
        R::UnsupportedInstructionBytes => {
            instruction_bytes.map(u16::from_le_bytes).map(u64::from).unwrap_or(0)
        }
        R::NonCanonicalRip => vmcb.exit_snapshot().rip,
        // EFER checked_instruction already proved addition does not overflow;
        // this error describes RIP+2, never the unrelated hardware NRIP field.
        R::NonCanonicalNrip => {
            if instruction_bytes.is_some() {
                vmcb.exit_snapshot().rip.wrapping_add(2)
            } else {
                vmcb.exit_snapshot().nrip
            }
        }
        R::NripNotEstablished => vmcb.exit_snapshot().nrip,
        R::InvalidInstructionLength => {
            if instruction_bytes.is_some() {
                2
            } else {
                vmcb.exit_snapshot().nrip
            }
        }
        R::ExitDoesNotPermitCandidate => info,
    };
    let (reason, value) = match error {
        E::UnsupportedInitialState => (1, logical),
        E::BackingMismatch => {
            (2, u64::from_le_bytes(vmcb.bytes()[0x4d0..0x4d8].try_into().unwrap()))
        }
        E::UnsupportedMode => {
            (3, u64::from_le_bytes(vmcb.bytes()[0x558..0x560].try_into().unwrap()))
        }
        E::UnsupportedDebugState => (4, vmcb.guest_rflags()),
        E::UnsupportedMsr { index, .. } => (5, index.into()),
        E::UnsupportedValue { value } => (6, value),
        E::Instruction(e) => (
            (if instruction_bytes.is_some() { 0x10 } else { 0x50 }) + instruction(e),
            instruction_value(e),
        ),
        E::PendingState(e) => {
            let (r, v) = pending(e);
            (0x20 + r, v)
        }
        E::Fault(M::Instruction(e)) => (
            (if instruction_bytes.is_some() { 0x30 } else { 0x60 }) + instruction(e),
            instruction_value(e),
        ),
        E::Fault(M::State(e)) => {
            let (r, v) = pending(e);
            (0x40 + r, v)
        }
    };
    let direction = match error {
        E::UnsupportedMsr { write, .. } => u64::from(write),
        _ => match info {
            0 => 0,
            1 => 1,
            _ => 2,
        },
    };
    (tag | ((reason | (direction << 9)) << 16), value)
}

fn instruction(e: crate::svm::exit::ResumeError) -> u64 {
    use crate::svm::exit::ResumeError as R;
    match e {
        R::ExitDoesNotPermitCandidate => 0,
        R::NripNotEstablished => 1,
        R::NonCanonicalRip => 2,
        R::NonCanonicalNrip => 3,
        R::InvalidInstructionLength => 4,
        R::UnsupportedInstructionBytes => 5,
    }
}

fn pending(e: crate::svm::events::ExternalInterruptError) -> (u64, u64) {
    use crate::svm::events::ExternalInterruptError as P;
    match e {
        P::ReservedVector { vector } => (0, vector.into()),
        P::InvalidTaskPriority { priority } => (1, priority.into()),
        P::RequestNotQueued => (2, 0),
        P::RequestNotArmed => (3, 0),
        P::RequestAlreadyConsumed => (4, 0),
        P::PendingInjection => (5, 0),
        P::NestedDeliveryUnsupported => (6, 0),
        P::PendingVirtualInterrupt => (7, 0),
        P::UnsupportedControl { control } => (8, control),
        P::UnsupportedNestedControl { control } => (9, control),
        P::ControlMismatch => (10, 0),
        P::InvalidEntry => (11, 0),
        P::GuestShutdown => (12, 0),
        P::InconsistentVirtualInterruptExit => (13, 0),
    }
}

fn syscfg_context(reason: u64, mode: u64, write: bool, value: u64) -> (u64, u64) {
    (0xf10d | ((reason | (mode << 8) | (u64::from(write) << 10)) << 16), value)
}

fn is_valid_syscfg_reason(reason: u64) -> bool {
    (1..=6).contains(&reason)
        || (0x10..=0x15).contains(&reason)
        || (0x20..=0x2d).contains(&reason)
        || (0x30..=0x35).contains(&reason)
        || (0x40..=0x4d).contains(&reason)
        || (0x50..=0x55).contains(&reason)
        || (0x60..=0x65).contains(&reason)
        || (0x80..=0x85).contains(&reason)
}

fn x2avic_stop(reason: X2AvicStop, code: u8, detail: u64) -> u64 {
    reason as u64 | u64::from(code) | (detail << 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_xapic_kinds_are_never_exported() {
        // Former kind8 is an ordinary NPF GPA observation.
        let npf = stop_words(0, 0x400, 0x1234, 0xf106, 0xfee00030).unwrap();
        assert_eq!((npf[0] >> 24) & 15, 3);
        assert_eq!(npf[1], 0xfee00030);
        // A context mismatch keeps its RIP on every exit, including NPF.
        for code in [0x400, 0x7c] {
            let words = stop_words(0, code, 0x5678, 0xf10a, 0xdead_beef).unwrap();
            assert_eq!((words[0] >> 24) & 15, 0);
            assert_eq!(words[1], 0x5678);
        }
        // Former kind13 detail tags no longer select a typed context.
        assert_eq!((stop_words(0, 0x400, 0, 0xf10a | (1 << 16), 9).unwrap()[0] >> 24) & 15, 3);
        // Nor does the retired kind-14 route record: F10Bh (the pre-arm stop,
        // or an old route detail) is an unhandled exit with its RIP.
        for info1 in [0xf10b, 0xf10b | (0x13 << 16)] {
            let words = stop_words(5, 0x7c, 0x5678, info1, 0x10_0000_c500).unwrap();
            assert_eq!(((words[0] >> 24) & 15, words[1]), (0, 0x5678));
        }
    }
    #[test]
    fn vmcr_diagnostics_preserve_distinct_identity_and_full_operand() {
        use crate::svm::{dispatch::NativeEferError as E, exit::ResumeError as R};
        let vmcb = crate::svm::vmcb::Vmcb::new();
        let before = *vmcb.bytes();
        let (tag, value) =
            vmcr_failure(E::UnsupportedValue { value: u64::MAX }, &vmcb, 0x10, [15, 48]);
        let words = stop_words(0, 0x7c, 0, tag, value).unwrap();
        assert_eq!((words[0] >> 24) & 15, 12);
        assert_eq!((words[0] >> 13) & 0x7ff, 6);
        assert_eq!([words[1], words[2]], [u32::MAX; 2]);
        let (tag, value) =
            vmcr_nrip_failure(E::Instruction(R::InvalidInstructionLength), &vmcb, 0x10);
        let words = stop_words(0, 0x7c, 0, tag, value).unwrap();
        assert_eq!((words[0] >> 13) & 0x7ff, 0x54);
        for reason in [1u64, 2, 0x55, 0x65, 0x600] {
            assert!(stop_words(0, 0x7c, 0, 0xf109 | (reason << 16), 0).is_none());
        }
        assert_eq!(vmcb.bytes(), &before);
    }
    #[test]
    fn efer_error_payload_is_full_width_and_instruction_is_explicit() {
        use crate::svm::{
            dispatch::NativeEferError as E, events::ExternalInterruptError as P,
            exit::ResumeError as R,
        };
        let vmcb = crate::svm::vmcb::Vmcb::new();
        let before = *vmcb.bytes();
        for (error, reason, value) in [
            (E::UnsupportedValue { value: 0xfedc_ba98_7654_3210 }, 6, 0xfedc_ba98_7654_3210),
            (E::Instruction(R::UnsupportedInstructionBytes), 0x15, 0x320f),
            (E::PendingState(P::UnsupportedControl { control: u64::MAX }), 0x28, u64::MAX),
            (E::PendingState(P::GuestShutdown), 0x2c, 0),
        ] {
            let (tag, payload) = efer_failure(error, &vmcb, 0xd01, [0x0f, 0x32]);
            assert_eq!(payload, value);
            let words = stop_words(23, 0x7c, 0xffff_ffff_ffff_ffff, tag, payload).unwrap();
            assert_eq!((words[0] >> 24) & 15, 11);
            assert_eq!((words[0] >> 13) & 0x7ff, reason);
            assert_eq!(words[1] as u64 | ((words[2] as u64) << 32), value);
        }
        assert_eq!(vmcb.bytes(), &before);
        let mut boundary = crate::svm::vmcb::Vmcb::new();
        // Deliberately stale hardware NRIP must not replace proposed RIP+2.
        unsafe {
            let bytes = (&mut boundary as *mut crate::svm::vmcb::Vmcb).cast::<u8>();
            core::ptr::copy_nonoverlapping(
                0x7fff_ffff_ffffu64.to_le_bytes().as_ptr(),
                bytes.add(0x578),
                8,
            );
            core::ptr::copy_nonoverlapping(0x1234u64.to_le_bytes().as_ptr(), bytes.add(0xc8), 8);
        }
        assert_eq!(
            efer_failure(E::Instruction(R::NonCanonicalNrip), &boundary, 0x500, [15, 50]).1,
            0x8000_0000_0001
        );
        for diagnostic in [0, 7, 0x16, 0x2e, 0x36, 0x4e, 0x600, 0x10000] {
            assert!(stop_words(0, 0x7c, 0, 0xf108 | (diagnostic << 16), 0).is_none());
        }
    }
    #[test]
    fn detailed_fetch_retains_rip_and_qualified_failure_only() {
        use super::super::fetch::FetchError;
        use crate::host::paging::WalkError;
        assert_eq!(
            fetch_failure_code(FetchError::Walk(WalkError::UnreadableTable {
                level: 3,
                address: 123
            })),
            0x314
        );
        for rip in [0, 0x7fff_ffff_ffff, 0xffff_8000_0000_0000, 0xffff_f800_b363_797a] {
            let words = stop_words(23, 0x7c, rip, 0xf001, 0x314).unwrap();
            assert_eq!((words[0] >> 24) & 15, 10);
            let context = words[1] as u64 | ((words[2] as u64) << 32);
            assert_eq!(context >> 48, 0x314);
            assert_eq!(((context << 16) as i64 >> 16) as u64, rip);
        }
        for failure in [0, 0x14, 0x514, 0x130, 0x3b, u64::MAX] {
            let words = stop_words(0, 0x7c, 0x1234, 0xf001, failure).unwrap();
            assert_eq!((words[0] >> 24) & 15, 1);
            assert_eq!(words[1], 0x1234);
        }
        let words = stop_words(0, 0x72, 0x8000_0000_0000, 0xf001, 4).unwrap();
        assert_eq!((words[0] >> 24) & 15, 1);
        assert_eq!(words[2], 0x8000);
    }
    #[test]
    fn x2avic_stop_reasons_are_distinct_typed_and_never_reuse_retired_tags() {
        use crate::svm::x2avic::{
            Error,
            ipi::{FanOutError, IpiRefusal as I},
            irq::IrqError as Q,
            registers::{InitError, Refusal as R},
        };
        let irq = [
            Q::ReservedVector(0x1f),
            Q::PhysicalIsrMismatch { vector: 0x43, highest: Some(0x50) },
            Q::DuplicatePhysicalSource(0x44),
            Q::AmbiguousLevelSource(0x45),
            Q::UnownedLevelCompletion(0x46),
            Q::CompletionNotReady(0x47),
            Q::UnexpectedPhysicalIsr(0x48),
            Q::VirtualPublication(0x49),
            Q::VirtualIsrMismatch { vector: 0x4a, highest: None },
            Q::DrainIncomplete,
            Q::NotInService(0x30),
        ];
        let (mut tags, mut count) = ([0u64; 48], 0);
        let mut push = |tag: u64| {
            tags[count] = tag & 0xffff;
            count += 1;
        };
        for (index, error) in irq.into_iter().enumerate() {
            let code = irq_error_code(error);
            assert_eq!(code >> 17, index as u32 + 1);
            let (tag, value) = irq_failure(IrqSite::LevelEoiExit, error);
            assert_eq!((tag, value), (0xf570 | (index as u64 + 1) | (2 << 16), u64::from(code)));
            push(tag);
        }
        // The ExtINT signature keeps its vector; mismatches keep both vectors.
        assert_eq!(
            irq_failure(IrqSite::Capture, Q::NotInService(0x30)),
            (0xf57b, (11 << 17) | 0x1_0030)
        );
        assert_eq!(irq_error_code(irq[1]), (2 << 17) | 0x5043);
        assert_eq!(irq_error_code(Q::DrainIncomplete), (10 << 17) | 0x1_0000);
        for (code, reason) in [
            R::UnownedAccess,
            R::UnsupportedMessageType,
            R::UnmaskedSmi,
            R::UnmaskedExtInt,
            R::ApicDisable,
            R::ApicRelocation,
            R::ExceptionVector,
        ]
        .into_iter()
        .enumerate()
        {
            let (tag, value) = register_refusal(reason, 0x835, true, 0x700);
            assert_eq!((tag, value), ((0xf541 + code as u64) | (0x1_0000_0835 << 16), 0x700));
            push(tag);
        }
        for (code, refusal) in [
            I::TargetNotRunning,
            I::InvalidBackingPage,
            I::UnknownReason,
            I::ReservedBits,
            I::ReservedMessageType,
            I::LevelTriggered,
            I::Smi,
            I::Nmi,
            I::InconsistentVectorExit,
        ]
        .into_iter()
        .enumerate()
        {
            let (tag, value) = ipi_refusal(refusal, 0x1b_0000_c4ef, (3 << 32) | 0xffff_f01b);
            assert_eq!((tag, value), ((0xf551 + code as u64) | (0x301b << 16), 0x1b_0000_c4ef));
            push(tag);
        }
        let (tag, value) = fan_out_failure(FanOutError::DoorbellTarget { slot: 5, id: 255 }, 7);
        assert_eq!((tag, value), (0xf560 | (0xff05 << 16), 7));
        push(tag);
        let (tag, value) =
            fan_out_failure(FanOutError::Publication { slot: 31, error: Error::MixedTrigger }, 7);
        assert_eq!((tag, value), (0xf561 | (0x81f << 16), 7));
        push(tag);
        for reason in [X2AvicStop::NoAcceleration, X2AvicStop::UndecodableAvicExit] {
            let (tag, value) = avic_exit_refusal(reason, (1 << 32) | 0x320, 0xdead_beef_0000_0061);
            assert_eq!((tag, value), (reason as u64 | (0x61 << 16), (1 << 32) | 0x320));
            push(tag);
        }
        {
            use crate::svm::x2avic::startup::{
                NativeDestinationCause as C, NativeDestinationMode as M, NativeRouteFailure,
                NativeRoutePredicate as P, NativeRouteRecipient,
            };
            assert_eq!(startup_route_refusal(None, 0x10_0000_0500), (0xf521, 0x10_0000_0500));
            let failure = NativeRouteFailure {
                value: 0x10_0000_0500,
                source: 8,
                predicate: P::QueueBusy,
                recipient: Some(NativeRouteRecipient {
                    identity: u32::MAX - 1,
                    mode: Some(M::X2Apic),
                    init_count: 40,
                    cause: C::GuestInit,
                }),
            };
            let (tag, value) = startup_route_refusal(Some(failure), 0x10_0000_0500);
            assert_eq!((tag, value), (0xf521 | (0xffff_fffe_4bfc << 16), 0x10_0000_0500));
            let failure =
                NativeRouteFailure { recipient: None, predicate: P::InitVector, ..failure };
            assert_eq!(startup_route_refusal(Some(failure), 7), (0xf521 | (14 << 16), 7));
            push(tag);
        }
        // Offset 60h with a hardware-written V_IRQ; reserved bits 63:40 drop.
        let control = (0xffu64 << 56) | (1 << 31) | (1 << 30) | (1 << 26) | (1 << 24) | (1 << 8);
        assert_eq!(profile_mismatch_at_entry(control), (0xf520 | (0xc500_0100 << 16), 2));
        for tag in [0xf500, 0xf510, 0xf520] {
            push(tag);
        }
        let tags = &mut tags[..count];
        tags.sort_unstable();
        assert!(tags.windows(2).all(|pair| pair[0] != pair[1]), "each stop reason has its own tag");
        for retired in
            [0xf501, 0xf502, 0xf503, 0xf504, 0xf505, 0xf511, 0xf522, 0xf523, 0xf524, 0xf530, 0xf531]
        {
            assert!(!tags.contains(&retired), "{retired:#x}");
        }
        // These reasons export as an unhandled exit with the guest RIP.
        for code in [0x60, 0x7c, 0x401, 0x402] {
            let words = stop_words(4, code, 0xffff_f800_1234_5678, tags[0], 0).unwrap();
            assert_eq!(((words[0] >> 24) & 15, words[1], words[2]), (0, 0x1234_5678, 0xffff_f800));
        }
        // Guest INIT failures use the exported startup-target record.
        assert_eq!(init_error_code(InitError::Backing(Error::UnsupportedVersion)), 0x1000_0009);
        assert_eq!(init_error_code(InitError::Irq(Q::UnexpectedPhysicalIsr(0x70))), 0x200f_0070);
        for (stage, value) in [
            (StartupStage::InitPreparation, 0x200f_0070u64),
            (StartupStage::InitLapicCommit, 0x1000_0009),
            (StartupStage::OwnerMissing, 1),
        ] {
            let (tag, payload) = startup_failure(7, stage as u8, value, 16);
            assert_eq!(payload, value | (16 << 32));
            let words = stop_words(2, 0x63, 0, tag, payload).unwrap();
            assert_eq!(((words[0] >> 24) & 15, (words[0] >> 16) & 127), (15, stage as u32));
            assert_eq!([words[1], words[2]], [value as u32, 16]);
        }
    }
    #[test]
    fn fixed_record_preserves_wide_values_and_synthetic_context() {
        assert_eq!(stop_words(31, u64::MAX, 5, 0, 0), Some([0x14ff_ff83, u32::MAX, u32::MAX]));
        let words = stop_words(2, 0x7c, 0x1234, 0xf104, 0xc0010114).unwrap();
        assert_eq!((words[0] >> 24) & 15, 2);
        assert_eq!(words[1], 0xc0010114);
        assert!(stop_words(32, 0, 0, 0, 0).is_none());
    }
}
