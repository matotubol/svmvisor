//! Ordering policy for a future returning, single-CPU firmware SVM probe.
//!
//! All observations are caller supplied. This module executes no instruction,
//! authenticates no ownership, and certifies neither physical readiness nor
//! safe firmware return. In particular an acknowledged STGI is not a readable
//! GIF measurement. The current terminal emulator entry cannot implement this
//! contract. A future wrapper must preserve VMLOAD/VMSAVE state, DR7, ABI and
//! extended state, contain faults, and establish NMI/SMM/AP constraints.
use crate::{
    arch::x86_64::{
        capabilities::{CapabilityError, CapabilityEvidence},
        xstate::XstateArea,
    },
    boot::xstate::{FirmwareXstateControls, FirmwareXstateError, FirmwareXstatePlan},
    memory::address::AddressError,
    svm::exit::ExitSnapshot,
};

pub const EFER_SVME: u64 = 1 << 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    FirmwareApplication,
    FirmwareCallback,
    AfterExitBootServices,
    Unknown,
}

/// A caller's externally maintained CPU lease; a nonzero generation prevents
/// accidental mixing of reports from different attempts, not forged reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuLease {
    pub cpu_id: u32,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    Unknown,
    Exclusive(CpuLease),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GifEvidence {
    Unknown,
    /// Caller protocol establishes GIF=1 before the attempt. RFLAGS.IF is not
    /// GIF evidence, and CPUID cannot establish this condition.
    EstablishedSet,
}

/// Values the future wrapper must obtain on the leased CPU. This subset is
/// deliberately insufficient to represent all AMD64/firmware state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostObservation {
    pub lease: CpuLease,
    pub efer: u64,
    pub vm_hsave_pa: u64,
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub rflags: u64,
    pub dr7: u64,
    pub xcr0: Option<u64>,
    pub xss: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionEvidence {
    pub capabilities: CapabilityEvidence,
    pub context: Context,
    pub tpl: u32,
    pub privilege_level: u8,
    pub active_processors: u32,
    pub ownership: Ownership,
    pub gif: GifEvidence,
    pub original: HostObservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Xstate(FirmwareXstateError),
    Capabilities(CapabilityError),
    Context,
    Ownership,
    ExistingSvmState,
    GifUnknown,
    Address(AddressError),
    MemoryOwnership,
    SavedImage,
    Order,
    Observation,
}

/// Numeric allocation evidence. It does not prove WB caching, mappings, or
/// isolation from firmware, devices, APs, or SMM. The allocation must remain
/// owned through the release transition, including on failed arming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryEvidence {
    pub lease: CpuLease,
    pub allocation_base: u64,
    pub allocation_bytes: u64,
    pub hsave_pa: u64,
    pub vmcb_pa: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Prepared,
    EnablingSvm,
    SvmEnabled,
    InstallingHsave,
    Armed,
    EntryAttempted,
    Returned,
    RestoringHsave,
    HsaveRestored,
    RestoringPreGifHost,
    PreGifHostRestored,
    GifRestoreAttempted,
    GifRestored,
    RestoringHost,
    HostRestored,
    Released,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Cleanup completed without any recorded entry attempt.
    AbortedBeforeEntry,
    /// Caller observed the exact expected VMMCALL exit and restored the
    /// recorded state. This is an evidence/order result, not launch approval.
    ExpectedExitAndRestorationObserved,
    UnexpectedExitAndRestorationObserved {
        code: u64,
    },
}

/// Explicit external adapter report, not proof produced by this state machine.
/// It covers VMLOAD/VMSAVE state, descriptors, stack, ABI/debug state, and
/// event/fault containment omitted from HostObservation. Reporting it without
/// an audited assembly implementation does not make STGI safe.
pub struct AdapterRestoreAcknowledgement {
    pub lease: CpuLease,
}

pub struct Transaction<'a> {
    original: HostObservation,
    captured: HostObservation,
    saved_image: &'a [u8],
    memory: MemoryEvidence,
    expected_exit_rip: u64,
    stage: Stage,
    outcome: Outcome,
}

impl<'a> Transaction<'a> {
    /// Rejection here does not create a transaction or imply CPU mutations.
    /// `captured` describes control state after the separately validated
    /// firmware_xstate capture plan; its full restoration remains obligatory.
    /// `saved_image` must be the actual retained original state image, not a
    /// caller-generated success flag. Later byte equality is deliberately a
    /// conservative opaque-evidence comparison, not architectural XSAVE state
    /// equivalence: padding/absent components and conditional x87 pointers can
    /// differ across captures. Passing the saved buffer again proves no CPU
    /// restoration; independently qualified assembly observations are required.
    pub fn prepare(
        evidence: AdmissionEvidence,
        memory: MemoryEvidence,
        captured: HostObservation,
        xstate_plan: FirmwareXstatePlan,
        saved_image: &'a XstateArea,
        observed_mxcsr_mask: u32,
        expected_exit_rip: u64,
    ) -> Result<Self, Error> {
        let caps = evidence.capabilities.validate().map_err(Error::Capabilities)?;
        if evidence.context != Context::FirmwareApplication
            || evidence.tpl != 4
            || evidence.privilege_level != 0
            || evidence.active_processors != 1
        {
            return Err(Error::Context);
        }
        let lease = match evidence.ownership {
            Ownership::Exclusive(lease) if lease.generation != 0 => lease,
            _ => return Err(Error::Ownership),
        };
        if lease != evidence.original.lease || lease != memory.lease || lease != captured.lease {
            return Err(Error::Ownership);
        }
        if evidence.original.efer & EFER_SVME != 0 || evidence.original.vm_hsave_pa != 0 {
            return Err(Error::ExistingSvmState);
        }
        if evidence.gif != GifEvidence::EstablishedSet {
            return Err(Error::GifUnknown);
        }
        // Only the temporary capture controls described by firmware_xstate may
        // differ: CR0.EM/TS and EFER.FFXSR cleared. Interrupt masking is deferred
        // to the future entry adapter and must also be restored before release.
        if controls(evidence.original) != xstate_plan.original_controls() {
            return Err(Error::Observation);
        }
        xstate_plan
            .validate_original_image(saved_image, observed_mxcsr_mask)
            .map_err(Error::Xstate)?;
        let capture_controls = xstate_plan.capture_controls();
        let mut permitted = evidence.original;
        permitted.cr0 = capture_controls.cr0;
        permitted.efer = capture_controls.efer;
        if captured != permitted {
            return Err(Error::Observation);
        }
        let policy = caps.address_policy();
        let arena = policy
            .validate(memory.allocation_base, memory.allocation_bytes, 4096)
            .map_err(Error::Address)?;
        for page in [memory.hsave_pa, memory.vmcb_pa] {
            let range = policy.validate(page, 4096, 4096).map_err(Error::Address)?;
            if page == 0 || page < arena.base() || range.last_byte() > arena.last_byte() {
                return Err(Error::MemoryOwnership);
            }
        }
        if memory.hsave_pa == memory.vmcb_pa {
            return Err(Error::MemoryOwnership);
        }
        if expected_exit_rip == 0 || !crate::memory::address::is_canonical_48(expected_exit_rip) {
            return Err(Error::Observation);
        }
        Ok(Self {
            original: evidence.original,
            captured,
            saved_image: saved_image.bytes(),
            memory,
            expected_exit_rip,
            stage: Stage::Prepared,
            outcome: Outcome::AbortedBeforeEntry,
        })
    }

    pub const fn stage(&self) -> Stage {
        self.stage
    }

    fn advance(&mut self, from: Stage, to: Stage) -> Result<(), Error> {
        if self.stage != from {
            return Err(Error::Order);
        }
        self.stage = to;
        Ok(())
    }

    fn armed_observation(&self, hsave: u64) -> HostObservation {
        HostObservation {
            efer: self.captured.efer | EFER_SVME,
            vm_hsave_pa: hsave,
            ..self.captured
        }
    }

    // Record each possibly mutating operation BEFORE executing it. A failed
    // operation/readback leaves the stage in-flight and never authorizes free.
    pub fn begin_enable(&mut self) -> Result<(), Error> {
        self.advance(Stage::Prepared, Stage::EnablingSvm)
    }
    pub fn observe_enabled(&mut self, actual: HostObservation) -> Result<(), Error> {
        if self.stage != Stage::EnablingSvm {
            return Err(Error::Order);
        }
        if actual != self.armed_observation(self.original.vm_hsave_pa) {
            return Err(Error::Observation);
        }
        self.stage = Stage::SvmEnabled;
        Ok(())
    }
    pub fn begin_install_hsave(&mut self) -> Result<(), Error> {
        self.advance(Stage::SvmEnabled, Stage::InstallingHsave)
    }
    pub fn observe_armed(&mut self, actual: HostObservation) -> Result<(), Error> {
        if self.stage != Stage::InstallingHsave {
            return Err(Error::Order);
        }
        if actual != self.armed_observation(self.memory.hsave_pa) {
            return Err(Error::Observation);
        }
        self.stage = Stage::Armed;
        Ok(())
    }
    pub fn begin_entry(&mut self) -> Result<(), Error> {
        // The adapter must mark this BEFORE CLI/CLGI or other entry mutations.
        self.advance(Stage::Armed, Stage::EntryAttempted)
    }
    pub fn observe_exit(&mut self, lease: CpuLease, exit: ExitSnapshot) -> Result<(), Error> {
        if self.stage != Stage::EntryAttempted {
            return Err(Error::Order);
        }
        if lease != self.original.lease {
            return Err(Error::Ownership);
        }
        self.outcome = if exit.code == 0x81 && exit.rip == self.expected_exit_rip {
            Outcome::ExpectedExitAndRestorationObserved
        } else {
            Outcome::UnexpectedExitAndRestorationObserved { code: exit.code }
        };
        self.stage = Stage::Returned;
        Ok(())
    }
    pub fn begin_restore_hsave(&mut self) -> Result<(), Error> {
        self.advance(Stage::Returned, Stage::RestoringHsave)
    }
    pub fn observe_hsave_restored(&mut self, actual: HostObservation) -> Result<(), Error> {
        if self.stage != Stage::RestoringHsave {
            return Err(Error::Order);
        }
        let expected = HostObservation {
            rflags: self.captured.rflags & !(1 << 9),
            ..self.armed_observation(self.original.vm_hsave_pa)
        };
        if actual != expected {
            return Err(Error::Observation);
        }
        self.stage = Stage::HsaveRestored;
        Ok(())
    }
    pub fn begin_restore_pre_gif_host(&mut self) -> Result<(), Error> {
        self.advance(Stage::HsaveRestored, Stage::RestoringPreGifHost)
    }
    pub fn observe_pre_gif_host_restored(
        &mut self,
        actual: HostObservation,
        restored_image: &[u8],
        adapter: AdapterRestoreAcknowledgement,
    ) -> Result<(), Error> {
        if self.stage != Stage::RestoringPreGifHost {
            return Err(Error::Order);
        }
        if adapter.lease != self.original.lease {
            return Err(Error::Ownership);
        }
        if actual != self.pre_gif_observation() {
            return Err(Error::Observation);
        }
        if restored_image != self.saved_image {
            return Err(Error::SavedImage);
        }
        self.stage = Stage::PreGifHostRestored;
        Ok(())
    }
    fn pre_gif_observation(&self) -> HostObservation {
        HostObservation {
            efer: self.original.efer | EFER_SVME,
            rflags: self.original.rflags & !(1 << 9),
            ..self.original
        }
    }
    pub fn begin_restore_gif(&mut self) -> Result<(), Error> {
        self.advance(Stage::PreGifHostRestored, Stage::GifRestoreAttempted)
    }
    /// Caller reports STGI retirement on this CPU while SVME is still set.
    /// There is no architectural GIF readback here and no NMI/SMM guarantee.
    pub fn acknowledge_stgi(&mut self, actual: HostObservation) -> Result<(), Error> {
        if self.stage != Stage::GifRestoreAttempted {
            return Err(Error::Order);
        }
        if actual != self.pre_gif_observation() {
            return Err(Error::Observation);
        }
        self.stage = Stage::GifRestored;
        Ok(())
    }
    pub fn begin_restore_host(&mut self) -> Result<(), Error> {
        // Only original EFER.SVME and RFLAGS.IF remain after the pre-GIF stage.
        self.advance(Stage::GifRestored, Stage::RestoringHost)
    }
    pub fn observe_host_restored(
        &mut self,
        actual: HostObservation,
        restored_image: &[u8],
    ) -> Result<(), Error> {
        if self.stage != Stage::RestoringHost {
            return Err(Error::Order);
        }
        self.check_restoration(actual, restored_image)?;
        self.stage = Stage::HostRestored;
        Ok(())
    }
    /// Failed preparation/arming can be rolled back without entry or GIF
    /// changes. Once entry is attempted, only observed-return cleanup applies.
    pub fn observe_abort_restored(
        &mut self,
        actual: HostObservation,
        restored_image: &[u8],
    ) -> Result<(), Error> {
        if !matches!(
            self.stage,
            Stage::Prepared
                | Stage::EnablingSvm
                | Stage::SvmEnabled
                | Stage::InstallingHsave
                | Stage::Armed
        ) {
            return Err(Error::Order);
        }
        self.check_restoration(actual, restored_image)?;
        self.stage = Stage::HostRestored;
        Ok(())
    }
    fn check_restoration(
        &self,
        actual: HostObservation,
        restored_image: &[u8],
    ) -> Result<(), Error> {
        if actual != self.original {
            return Err(Error::Observation);
        }
        if restored_image != self.saved_image {
            return Err(Error::SavedImage);
        }
        Ok(())
    }
    pub fn release_memory(&mut self, lease: CpuLease) -> Result<Outcome, Error> {
        if lease != self.original.lease {
            return Err(Error::Ownership);
        }
        self.advance(Stage::HostRestored, Stage::Released)?;
        Ok(self.outcome)
    }
}

fn controls(observed: HostObservation) -> FirmwareXstateControls {
    FirmwareXstateControls {
        cr0: observed.cr0,
        cr4: observed.cr4,
        efer: observed.efer,
        xcr0: observed.xcr0,
        xss: observed.xss,
    }
}
