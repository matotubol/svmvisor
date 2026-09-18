//! Physical SYSCFG writes for the admitted native, unencrypted target.
//!
//! PPR57896 rev3.00 p202; APM2 rev3.44 3.2.1/Fig3-11 and 7.9.1:
//! bits18/19 control fixed-range DRAM attributes and their visibility below
//! 1MiB. This owner admits only those changes, preserving TOM and encryption
//! controls. The caller retains all monitor memory above1MiB and validates
//! current fixed-range routing before any low guest-memory alias access.
//! Fixed MTRR accesses and guest cache serialization remain native; this is
//! not an independent memory-type model or general SYSCFG implementation.

use crate::{
    arch::x86_64::{
        capabilities::ValidatedCapabilities,
        msr::{
            SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN,
            SYS_CFG_MTRR_FIX_DRAM_MOD_EN,
        },
        registers::GuestRegisters,
    },
    svm::{
        dispatch::{self, NativeEferError, NativeMsrOutcome},
        exit::{MsrInstruction, ResumeCandidate},
        vmcb::Vmcb,
    },
};

pub const FIXED_DRAM_CONTROL_MASK: u64 = SYS_CFG_MTRR_FIX_DRAM_EN | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;

/// All fallible guest-state checks precede construction. Holding this token
/// prevents mutation of its VMCB between preparation and completion. Dropping
/// it leaves the VMCB unchanged; it cannot roll back a caller's hardware write.
pub struct PreparedWrite<'a> {
    vmcb: &'a mut Vmcb,
    next: ResumeCandidate,
    requested: u64,
    current: u64,
}

impl PreparedWrite<'_> {
    pub const fn requested(&self) -> u64 {
        self.requested
    }
    pub const fn current(&self) -> u64 {
        self.current
    }
    pub const fn delta(&self) -> u64 {
        self.requested ^ self.current
    }
    pub fn write_value(&self) -> Option<u64> {
        (self.requested != self.current).then_some(self.requested)
    }
    /// Complete only after the caller successfully applies the requested
    /// physical write, or after an admitted no-op. No hardware access occurs.
    pub fn commit(self) -> NativeMsrOutcome {
        self.vmcb.commit_emulated_instruction(self.vmcb.guest_rax(), self.next);
        self.vmcb.complete_native_instruction_state();
        NativeMsrOutcome::Completed
    }
}

pub enum SyscfgPreparation<'a> {
    Write(PreparedWrite<'a>),
    GeneralProtectionPrepared,
}

/// Hardware evidence requires the actual same-CPU native MSR exit. Bytes
/// require an owned instruction read from this stopped guest's current RIP.
pub enum SyscfgInstruction<'a> {
    Bytes(&'a [u8]),
    Hardware(&'a ValidatedCapabilities),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscfgError {
    Boundary(NativeEferError),
    UnsupportedProfile { signature: u32, physical_bits: u8 },
    CurrentReserved { requested: u64, current: u64 },
    CurrentEncryption { requested: u64, current: u64 },
    RequestedReserved { requested: u64, current: u64 },
    UnsupportedChange { requested: u64, current: u64 },
}

/// Prepare a target-native WRMSR without changing hardware or completing it.
/// `read_current` must read SYSCFG on this owning CPU without fault/allocation;
/// it is called only after target and instruction admission. All unsupported
/// values preserve stopped state, including legal controls outside this owner.
/// Reserved target values remain explicit refusals rather than invented #GP.
/// The byte path supports the existing unpaged startup modes; the hardware
/// path consumes NRIP in long64 and needs no WB instruction-memory access.
#[allow(clippy::too_many_arguments)]
pub fn prepare<'a>(
    vmcb: &'a mut Vmcb,
    frame: &GuestRegisters,
    instruction: SyscfgInstruction<'_>,
    startup_owned: bool,
    signature: u32,
    physical_bits: u8,
    read_current: impl FnOnce() -> u64,
) -> Result<SyscfgPreparation<'a>, SyscfgError> {
    use NativeEferError as B;
    use SyscfgError as E;
    if !crate::memory::mtrrs::Tom2Default::supported_profile(signature, physical_bits) {
        return Err(E::UnsupportedProfile { signature, physical_bits });
    }
    let instruction = match instruction {
        SyscfgInstruction::Bytes(bytes) => MsrInstruction::Bytes(bytes),
        SyscfgInstruction::Hardware(caps) => {
            dispatch::hardware_msr_instruction(vmcb, caps).map_err(E::Boundary)?
        }
    };
    dispatch::validate_native_msr_boundary(vmcb, instruction, startup_owned)
        .map_err(E::Boundary)?;
    let snapshot = vmcb.exit_snapshot();
    let index = frame.rcx as u32;
    let write = snapshot.info1 == 1;
    if index != SYS_CFG || !write {
        return Err(E::Boundary(B::UnsupportedMsr { index, write }));
    }
    // APM3 WRMSR / APM2 15.11: CPL violation faults before MSR execution.
    // Native hardware evidence already excludes CPL!=0; bytes model that fault.
    if vmcb.bytes()[0x4cb] != 0 {
        vmcb.queue_validated_msr_general_protection(instruction)
            .map_err(|e| E::Boundary(B::Fault(e)))?;
        return Ok(SyscfgPreparation::GeneralProtectionPrepared);
    }
    if (!startup_owned && !vmcb.guest_in_64_bit_code())
        || (startup_owned && !dispatch::native_startup_instruction_mode(vmcb, instruction.length()))
    {
        return Err(E::Boundary(B::UnsupportedMode));
    }
    if vmcb.guest_rflags() & (1 << 8) != 0 {
        return Err(E::Boundary(B::UnsupportedDebugState));
    }
    let next = instruction.continuation(snapshot).map_err(|e| E::Boundary(B::Instruction(e)))?;
    let requested = ((frame.rdx as u32 as u64) << 32) | vmcb.guest_rax() as u32 as u64;
    let current = read_current();
    if current & !SYS_CFG_DEFINED != 0 {
        return Err(E::CurrentReserved { requested, current });
    }
    if current & SYS_CFG_ENCRYPTION != 0 {
        return Err(E::CurrentEncryption { requested, current });
    }
    if requested & !SYS_CFG_DEFINED != 0 {
        return Err(E::RequestedReserved { requested, current });
    }
    if (requested ^ current) & !FIXED_DRAM_CONTROL_MASK != 0 {
        return Err(E::UnsupportedChange { requested, current });
    }
    Ok(SyscfgPreparation::Write(PreparedWrite { vmcb, next, requested, current }))
}
