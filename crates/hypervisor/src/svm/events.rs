//! Event types of the VMCB paths: the reflected exception and the errors of
//! event injection, virtual interrupt control and MSR fault queuing.
//!
//! AMD APM vol.2 rev.3.44 sections 15.7, 15.12, 15.20, 15.21 and Appendix B.

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
