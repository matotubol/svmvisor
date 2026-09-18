//! Bounded synchronous fault reflection and virtual maskable IRQ ownership.
//!
//! AMD APM vol.2 rev.3.44 sections 15.7, 15.12, 15.20, 15.21 and Appendix B.
//! Interrupted exception delivery has a separate opt-in, bounded policy. Physical
//! interrupt routing, interrupted INTR/NMI delivery and arbitrary restart are absent.

/// One owned synthetic interrupt request; this is not an APIC IRR/ISR or EOI model.
///
/// The caller exclusively owns the stopped VMCB and must observe each actual
/// entry/exit before reusing it. This record cannot prove hardware execution.
/// It intentionally is not Copy/Clone: copying a request could duplicate delivery.
#[derive(Debug, PartialEq, Eq)]
pub struct PendingExternalInterrupt {
    vector: u8,
    pub(crate) state: ExternalInterruptState,
}

impl PendingExternalInterrupt {
    pub const fn new(vector: u8) -> Result<Self, ExternalInterruptError> {
        if vector < 32 {
            return Err(ExternalInterruptError::ReservedVector { vector });
        }
        Ok(Self { vector, state: ExternalInterruptState::Queued })
    }

    pub const fn vector(&self) -> u8 {
        self.vector
    }

    pub const fn state(&self) -> ExternalInterruptState {
        self.state
    }

    pub(crate) const fn control(&self) -> u64 {
        // AMD APM vol.2 rev.3.44 15.21.4, Appendix B offset 60h:
        // use vector priority class, virtual IF/TPR masking, no V_IGN_TPR.
        ((self.vector as u64) << 32) | (((self.vector >> 4) as u64) << 16) | (1 << 24)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalInterruptState {
    Queued,
    Armed,
    /// Hardware cleared V_IRQ without interrupted delivery. This means dispatch,
    /// not guest handler completion, IRETQ completion, or APIC acknowledgement.
    Consumed,
}

/// An exception queued without advancing guest CS:RIP. #DF is an abort and its
/// retained instruction pointer is undefined, not a restart address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReflectedException {
    InvalidOpcode,
    SegmentNotPresent {
        error_code: u32,
    },
    StackFault {
        error_code: u32,
    },
    GeneralProtection {
        error_code: u32,
    },
    PageFault {
        error_code: u32,
        address: u64,
    },
    /// Abort; the saved instruction pointer is undefined, not a retry address.
    DoubleFault,
}

impl ReflectedException {
    pub const fn vector(self) -> u8 {
        match self {
            Self::InvalidOpcode => 6,
            Self::DoubleFault => 8,
            Self::SegmentNotPresent { .. } => 11,
            Self::StackFault { .. } => 12,
            Self::GeneralProtection { .. } => 13,
            Self::PageFault { .. } => 14,
        }
    }

    pub(crate) const fn encoding(self) -> u64 {
        let error = match self {
            Self::InvalidOpcode => 0,
            Self::DoubleFault => 1 << 11,
            Self::SegmentNotPresent { error_code }
            | Self::StackFault { error_code }
            | Self::GeneralProtection { error_code }
            | Self::PageFault { error_code, .. } => (1 << 11) | ((error_code as u64) << 32),
        };
        (1 << 31) | (3 << 8) | self.vector() as u64 | error
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Delivery request only. #DF is nonrestartable; no handler ran in this call.
    Injected(ReflectedException),
    /// Caller must terminate this execution and retain the stopped evidence.
    Shutdown(GuestShutdown),
}

/// A terminal guest outcome, never a resume authorization or a host shutdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestShutdown {
    /// VMEXIT_SHUTDOWN; all other saved guest fields are architecturally undefined.
    Intercepted,
    /// Reflecting a checked fault during #DF delivery causes guest shutdown.
    ExceptionDelivery { interrupted_vector: u8, fault_vector: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalInterruptError {
    /// The bounded policy excludes the architectural exception vector range.
    ReservedVector {
        vector: u8,
    },
    InvalidTaskPriority {
        priority: u8,
    },
    RequestNotQueued,
    RequestNotArmed,
    /// A dispatched request cannot be withdrawn back to pending ownership.
    RequestAlreadyConsumed,
    PendingInjection,
    NestedDeliveryUnsupported,
    PendingVirtualInterrupt,
    UnsupportedControl {
        control: u64,
    },
    UnsupportedNestedControl {
        control: u64,
    },
    ControlMismatch,
    InvalidEntry,
    /// APM 15.14.3: saved guest state is undefined; never settle an IRQ from it.
    GuestShutdown,
    InconsistentVirtualInterruptExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReflectionError {
    InvalidEntry,
    GuestShutdown,
    PendingInjection,
    PendingVirtualInterrupt,
    NestedDeliveryUnsupported,
    UnsupportedExit { code: u64 },
    InvalidGeneralProtectionError { error_code: u64 },
    UnsupportedPageFaultError { error_code: u64 },
    InvalidSelectorError { vector: u8, error_code: u64 },
    NoInterruptedDelivery,
    UnsupportedInterruptedEvent { event: u64 },
    InvalidInterruptedEvent { event: u64 },
    PriorInjectionMismatch,
    Control(ExternalInterruptError),
}

/// Failure to queue a policy-required #GP(0) for a retained MSR instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsrFaultError {
    Instruction(super::exit::ResumeError),
    State(ExternalInterruptError),
}

/// APM2 8.2.9/Table8-3 and 15.7.2–3. Deliberately only interrupted #UD/#GP/#PF/#DF
/// and secondary #NP/#SS/#GP/#PF; no repair/dismiss, INTR/NMI or trap reinjection.
pub(crate) fn prepare_interrupted_delivery(
    code: u64,
    info1: u64,
    info2: u64,
    interrupted: u64,
    injection: u64,
) -> Result<DeliveryOutcome, ReflectionError> {
    if interrupted & (1 << 31) == 0 {
        return Err(ReflectionError::NoInterruptedDelivery);
    }
    if interrupted & 0x7fff_f000 != 0 {
        return Err(ReflectionError::InvalidInterruptedEvent { event: interrupted });
    }
    let prior = interrupted as u8;
    if (interrupted >> 8) & 7 != 3 || !matches!(prior, 6 | 8 | 13 | 14) {
        return Err(ReflectionError::UnsupportedInterruptedEvent { event: interrupted });
    }
    let has_error = interrupted & (1 << 11) != 0;
    let error = interrupted >> 32;
    if has_error != (prior != 6)
        || (prior == 8 && error != 0)
        || (prior == 13 && error & !0xffff != 0)
        || (prior == 14 && error & !0x7f != 0)
    {
        return Err(ReflectionError::InvalidInterruptedEvent { event: interrupted });
    }
    // EVENTINJ may retain the prior input or have V cleared by hardware. If it
    // still carries a valid request, never overwrite a different queued event.
    // Error-code bits with EV=0 are undefined in EXITINTINFO (APM15.7.2).
    let meaningful = if has_error { u64::MAX } else { u32::MAX as u64 };
    if injection & (1 << 31) != 0 && injection & meaningful != interrupted & meaningful {
        return Err(ReflectionError::PriorInjectionMismatch);
    }
    let fault = match code {
        0x4b | 0x4c if info1 & !0xffff != 0 => {
            return Err(ReflectionError::InvalidSelectorError {
                vector: (code - 0x40) as u8,
                error_code: info1,
            });
        }
        0x4b => ReflectedException::SegmentNotPresent { error_code: info1 as u32 },
        0x4c => ReflectedException::StackFault { error_code: info1 as u32 },
        0x4d | 0x4e => prepare(code, info1, info2, 0, 0)?,
        _ => return Err(ReflectionError::UnsupportedExit { code }),
    };
    if prior == 8 {
        return Ok(DeliveryOutcome::Shutdown(GuestShutdown::ExceptionDelivery {
            interrupted_vector: prior,
            fault_vector: fault.vector(),
        }));
    }
    let combined = if prior == 14 || (prior == 13 && fault.vector() != 14) {
        ReflectedException::DoubleFault
    } else {
        fault
    };
    Ok(DeliveryOutcome::Injected(combined))
}

pub(crate) fn prepare(
    code: u64,
    info1: u64,
    info2: u64,
    exit_interrupt_info: u64,
    injection: u64,
) -> Result<ReflectedException, ReflectionError> {
    if code == u64::MAX {
        return Err(ReflectionError::InvalidEntry);
    }
    if code == 0x7f {
        return Err(ReflectionError::GuestShutdown);
    }
    if injection & (1 << 31) != 0 {
        return Err(ReflectionError::PendingInjection);
    }
    // APM 15.7.2: reflecting here requires combining exceptions by x86 rules.
    if exit_interrupt_info & (1 << 31) != 0 {
        return Err(ReflectionError::NestedDeliveryUnsupported);
    }
    match code {
        // #UD has no error code; EXITINFO1/2 are undefined and must be ignored.
        0x46 => Ok(ReflectedException::InvalidOpcode),
        // APM 8.4.1: selector error codes occupy bits 15:0.
        0x4d if info1 & !0xffff == 0 => {
            Ok(ReflectedException::GeneralProtection { error_code: info1 as u32 })
        }
        0x4d => Err(ReflectionError::InvalidGeneralProtectionError { error_code: info1 }),
        // APM 8.4.2: P, R/W, U/S, RSV, I/D, PK, SS. RMP faults require
        // a separate SNP policy and are excluded from this classic-SVM path.
        0x4e if info1 & !0x7f == 0 => {
            Ok(ReflectedException::PageFault { error_code: info1 as u32, address: info2 })
        }
        0x4e => Err(ReflectionError::UnsupportedPageFaultError { error_code: info1 }),
        _ => Err(ReflectionError::UnsupportedExit { code }),
    }
}
