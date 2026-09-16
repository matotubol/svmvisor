//! Inert, byte-exact VMCB storage for the unencrypted SVM foundation.
//!
//! Offsets and widths follow AMD APM volume 2 revision 3.44, Appendix B,
//! tables B-1 and B-2. State-save offsets include the 0x400 area base.
//! Zero initialization preserves reserved bytes; it does not create a guest
//! that can run. This module provides no launch, allocation, physical-address
//! translation, guest-state completeness check, or hardware access.

use super::events::{
    self, DeliveryOutcome, ExternalInterruptError, ExternalInterruptState, GuestShutdown,
    PendingExternalInterrupt, ReflectedException, ReflectionError,
};
use crate::address::{AddressError, AddressPolicy};
use crate::capabilities::{CapabilityError, ValidatedCapabilities};
use crate::descriptors::{SegmentState, ValidatedGuestDescriptors};
use crate::exit::{ExitSnapshot, ResumeCandidate};
use crate::guest_state::ValidatedGuestState;
use crate::permission_maps::{IOPM_BYTES, MSRPM_BYTES};

pub const VMCB_BYTES: usize = 4096;

const INTERCEPT_MISC1: usize = 0x00c;
const INTERCEPT_MISC2: usize = 0x010;
const IOPM_BASE: usize = 0x040;
const MSRPM_BASE: usize = 0x048;
const TSC_OFFSET: usize = 0x050;
const GUEST_ASID: usize = 0x058;
const VIRTUAL_INTERRUPT_CONTROL: usize = 0x060;
const V_IRQ: u64 = 1 << 8;
const SUPPORTED_VIRTUAL_INTERRUPT_CONTROL: u64 =
    0xf | V_IRQ | (0xf << 16) | (1 << 24) | (0xff << 32);
const NESTED_CR3: usize = 0x0b0;
const CLEAN_BITS: usize = 0x0c0;
const GUEST_EFER: usize = 0x4d0;
const GUEST_CR4: usize = 0x548;
const GUEST_CR3: usize = 0x550;
const GUEST_CR0: usize = 0x558;
const GUEST_RFLAGS: usize = 0x570;
const GUEST_RIP: usize = 0x578;
const GUEST_RSP: usize = 0x5d8;
const GUEST_S_CET: usize = 0x5e0;
const GUEST_ISST_ADDR: usize = 0x5f0;
const GUEST_RAX: usize = 0x5f8;

/// Reviewed baseline instruction intercepts; arbitrary bit masks are excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstructionIntercept {
    Rdtsc,
    Rdtscp,
    Cpuid,
    Hlt,
    Vmrun,
    Vmmcall,
    Vmload,
    Vmsave,
    Stgi,
    Clgi,
    Skinit,
    Invlpga,
    Ioio,
    Msr,
    Xsetbv,
}

impl InstructionIntercept {
    const fn location(self) -> (usize, u32) {
        match self {
            Self::Rdtsc => (INTERCEPT_MISC1, 1 << 14),
            Self::Rdtscp => (INTERCEPT_MISC2, 1 << 7),
            Self::Cpuid => (INTERCEPT_MISC1, 1 << 18),
            Self::Hlt => (INTERCEPT_MISC1, 1 << 24),
            Self::Vmrun => (INTERCEPT_MISC2, 1),
            Self::Vmmcall => (INTERCEPT_MISC2, 1 << 1),
            Self::Vmload => (INTERCEPT_MISC2, 1 << 2),
            Self::Vmsave => (INTERCEPT_MISC2, 1 << 3),
            Self::Stgi => (INTERCEPT_MISC2, 1 << 4),
            Self::Clgi => (INTERCEPT_MISC2, 1 << 5),
            Self::Skinit => (INTERCEPT_MISC2, 1 << 6),
            Self::Invlpga => (INTERCEPT_MISC1, 1 << 26),
            Self::Ioio => (INTERCEPT_MISC1, 1 << 27),
            Self::Msr => (INTERCEPT_MISC1, 1 << 28),
            Self::Xsetbv => (INTERCEPT_MISC2, 1 << 13),
        }
    }
}

/// Event intercept controls from APM vol.2 rev.3.44 Appendix B,
/// Table B-1, offset 00Ch bits 0/1/2/3/4/31. These are distinct from instruction
/// intercepts and virtual event injection. Setting a bit does not establish
/// platform support, pending-event handling or a safe firmware return path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventIntercept {
    /// Physical maskable INTR, exit60h. APM 15.13.1: the interrupt remains
    /// pending for host acknowledgement; this does not inject a guest IRQ.
    PhysicalInterrupt,
    Nmi,
    /// APM section 15.13.3: hardware ignores this bit when HWCR.SMMLOCK is set.
    /// Internal and external SMIs have different pending-event semantics.
    Smi,
    Init,
    /// Exit just before a virtual IRQ is dispatched; V_IRQ remains pending.
    /// APM 15.13.5. This is not a physical interrupt intercept.
    VirtualInterrupt,
    /// APM15.14.3: terminal exit7fh; saved guest state is undefined.
    Shutdown,
}

impl EventIntercept {
    const fn mask(self) -> u32 {
        match self {
            Self::PhysicalInterrupt => 1,
            Self::Nmi => 1 << 1,
            Self::Smi => 1 << 2,
            Self::Init => 1 << 3,
            Self::VirtualInterrupt => 1 << 4,
            Self::Shutdown => 1 << 31,
        }
    }
}

/// CPU layout alignment only: the object's address is not a physical address.
/// No writable byte view is exposed, so callers cannot modify reserved fields.
#[repr(C, align(4096))]
pub struct Vmcb {
    bytes: [u8; VMCB_BYTES],
}

impl Default for Vmcb {
    fn default() -> Self {
        Self::new()
    }
}

impl Vmcb {
    /// Install the explicitly admitted native x2AVIC profile before first
    /// entry. APM2 3.44 15.29.4/.10. Caller owns pinned WB backing/table pages,
    /// one-to-one routing, host IRQ acknowledgement and an x2APIC continuation.
    /// This does not change the generic/synthetic interrupt profile.
    pub fn enable_native_x2avic(
        &mut self,
        profile: &super::x2avic::NativeX2AvicProfile,
    ) -> Result<(), ExternalInterruptError> {
        self.validate_external_interrupt_conflicts()?;
        self.validate_virtual_interrupt_controls()?;
        if self.virtual_interrupt_control() & !(0xf | (1 << 24)) != 0
            || self.read_u64::<0x090>() != 1
            || self.read_u64::<0x0b8>() != 0
            || self.read_u64::<0x098>() != 0
            || self.read_u64::<0x0e0>() != 0
            || self.read_u64::<0x0e8>() != 0
            || self.read_u64::<0x0f0>() != 0
            || self.read_u64::<0x0f8>() != 0
        {
            return Err(ExternalInterruptError::ControlMismatch);
        }
        self.write_u64::<0x0e0>(profile.backing_address());
        self.write_u64::<0x0f8>(profile.table_control());
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(
            (self.virtual_interrupt_control() & 0xf) | super::x2avic::NATIVE_CONTROL,
        );
        self.set_event_intercept(EventIntercept::PhysicalInterrupt, true);
        self.set_event_intercept(EventIntercept::Init, true);
        self.set_event_intercept(EventIntercept::VirtualInterrupt, false);
        self.invalidate_all();
        Ok(())
    }

    /// Recheck exact immutable page bindings before a native VMRUN. The
    /// runtime separately checks pending-event ownership and stopped state.
    pub fn validate_native_x2avic(
        &self,
        profile: &super::x2avic::NativeX2AvicProfile,
    ) -> Result<(), ExternalInterruptError> {
        self.validate_native_x2avic_controls()?;
        if self.read_u64::<0x0e0>() != profile.backing_address()
            || self.read_u64::<0x0f8>() != profile.table_control()
        {
            return Err(ExternalInterruptError::ControlMismatch);
        }
        Ok(())
    }

    /// Only the capability/address-checked native setup can establish this
    /// encoding through the safe API. Used by native fault owners; generic
    /// external interrupt injection continues to reject AVIC controls.
    pub(crate) fn validate_native_x2avic_controls(&self) -> Result<(), ExternalInterruptError> {
        let control = self.virtual_interrupt_control();
        let backing = self.read_u64::<0x0e0>();
        let table = self.read_u64::<0x0f8>();
        if control & !0xf != super::x2avic::NATIVE_CONTROL
            || self.read_u64::<0x090>() != 1
            || self.read_u64::<0x0b8>() != 0
            || self.read_u64::<0x098>() != 0
            || self.read_u64::<0x0e8>() != 0
            || self.read_u64::<0x0f0>() != 0
            || backing == 0 || backing & !0x000f_ffff_ffff_f000 != 0
            || table & 0x000f_ffff_ffff_f000 == 0
            || table >> 52 != 0 || table & 0xfff > super::x2avic::MAX_ID as u64
            || backing == table & !0xfff
            || !self.event_intercept(EventIntercept::PhysicalInterrupt)
            || !self.event_intercept(EventIntercept::Init)
            || self.event_intercept(EventIntercept::VirtualInterrupt)
        {
            return Err(ExternalInterruptError::ControlMismatch);
        }
        Ok(())
    }

    /// Native instruction owner has established #GP(0) before side effects.
    /// The explicit profile is checked without advancing RIP or changing GPRs.
    pub fn queue_native_x2avic_general_protection(
        &mut self,
        profile: &super::x2avic::NativeX2AvicProfile,
    ) -> Result<(), ExternalInterruptError> {
        self.validate_native_x2avic(profile)?;
        self.queue_native_general_protection()
    }
    /// Trusted single-CPU native continuation, APM2 rev3.44 15.5–15.7,
    /// 15.11, 15.21 and Appendix B. Install only before first entry, with the
    /// native MSRPM and an admitted unencrypted native platform. Hardware owns
    /// I/O, HLT, APIC/CR8, interrupts, DRs and XSETBV. The host must never use
    /// or switch the live guest xstate/XCR0 or enable its own breakpoints.
    /// SVM instructions remain stopped despite the mandatory backing SVME.
    /// This deliberately replaces synthetic intercepts; it is not containment.
    pub fn configure_native_boot_intercepts(&mut self) -> Result<(), ExternalInterruptError> {
        self.validate_external_interrupt_conflicts()?;
        self.validate_virtual_interrupt_controls()?;
        if self.virtual_interrupt_control() != 0
            || self.read_u64::<0x0b8>() != 0
            || self.read_u64::<TSC_OFFSET>() != 0
        {
            return Err(ExternalInterruptError::ControlMismatch);
        }
        self.write_u32::<0x000>(0); // CR reads/writes execute natively.
        self.write_u32::<0x004>(0); // DR6/DR7 are switched by VMRUN/VMEXIT.
        self.write_u32::<0x008>(0); // Guest exceptions use its native IDT.
        self.write_u32::<INTERCEPT_MISC1>((1 << 18) | (1 << 26) | (1 << 28) | (1 << 31));
        self.write_u32::<INTERCEPT_MISC2>(0x7f);
        self.write_u32::<0x014>(0);
        self.invalidate_all();
        Ok(())
    }

    pub const fn new() -> Self {
        Self {
            bytes: [0; VMCB_BYTES],
        }
    }

    pub const fn bytes(&self) -> &[u8; VMCB_BYTES] {
        &self.bytes
    }

    /// Bounded identity clock profile; APM vol.2 rev.3.44 Appendix B offset50h.
    /// Caller must separately own the optional global ratio and AUX MSRs.
    pub fn set_tsc_offset_zero(&mut self) {
        self.write_u64::<TSC_OFFSET>(0);
        self.invalidate_all();
    }

    pub fn tsc_offset(&self) -> u64 {
        self.read_u64::<TSC_OFFSET>()
    }

    pub fn guest_asid(&self) -> u32 {
        self.read_u32::<GUEST_ASID>()
    }

    /// Read retained exit fields. Meaningful only after a separately established
    /// exit; this neither synchronizes with hardware nor captures other GPRs.
    pub fn exit_snapshot(&self) -> ExitSnapshot {
        ExitSnapshot::from_vmcb_bytes(&self.bytes)
    }

    pub fn set_guest_asid(
        &mut self,
        asid: u32,
        capabilities: &ValidatedCapabilities,
    ) -> Result<(), CapabilityError> {
        capabilities.validate_asid(asid)?;
        self.write_u32::<GUEST_ASID>(asid);
        self.request_full_tlb_flush();
        Ok(())
    }

    /// Validate both complete map extents before changing either field.
    /// The caller still owns initialization, WB mapping, and lifetime proofs.
    /// This does not enable I/O or MSR intercepts.
    pub fn set_permission_maps(
        &mut self,
        iopm: u64,
        msrpm: u64,
        policy: &AddressPolicy,
    ) -> Result<(), AddressError> {
        policy.validate(iopm, IOPM_BYTES as u64, 4096)?;
        policy.validate(msrpm, MSRPM_BYTES as u64, 4096)?;
        self.write_u64::<IOPM_BASE>(iopm);
        self.write_u64::<MSRPM_BASE>(msrpm);
        self.invalidate_all();
        Ok(())
    }

    pub fn permission_maps(&self) -> (u64, u64) {
        (self.read_u64::<IOPM_BASE>(), self.read_u64::<MSRPM_BASE>())
    }

    /// Store an aligned root page address, with low CR3 control bits zero.
    /// This neither enables nested paging nor validates page-table contents.
    pub fn set_nested_root(
        &mut self,
        root: u64,
        policy: &AddressPolicy,
    ) -> Result<(), AddressError> {
        policy.validate(root, 4096, 4096)?;
        self.write_u64::<NESTED_CR3>(root);
        self.request_full_tlb_flush();
        Ok(())
    }

    pub fn nested_root(&self) -> u64 {
        self.read_u64::<NESTED_CR3>()
    }

    /// APM2 Appendix B, offset000h bit16: pre-execution CR0 write intercept.
    /// Used only while the native shared cache replay requires CD to stay set.
    pub fn set_cache_cr0_guard(&mut self, enabled: bool) {
        self.update_intercept::<0x000>(1 << 16, enabled);
    }

    /// Enable the already-built native NPT only before first entry. Caller
    /// proves complete table ownership and mappings; this validates root
    /// encoding, nonzero ASID and absence of extra nested controls. APM2
    /// rev3.44 15.25/Appendix B: NP_ENABLE and TLB_CONTROL=1.
    pub fn enable_native_nested_paging(
        &mut self,
        policy: &AddressPolicy,
    ) -> Result<(), crate::guest::continuation::NativeContinuationError> {
        use crate::guest::continuation::NativeContinuationError as E;
        if self.nested_root() == 0
            || self.read_u32::<GUEST_ASID>() == 0
            || self.read_u64::<0x090>() != 0
            || self.read_u64::<0x070>() != 0
            || self.read_u64::<0x0b8>() != 0
        {
            return Err(E::DestinationEventState);
        }
        policy
            .validate(self.nested_root(), 4096, 4096)
            .map_err(E::Address)?;
        self.write_u64::<0x090>(1);
        self.request_full_tlb_flush();
        Ok(())
    }

    /// Request a full flush on the next VMRUN. APM2 rev3.44 15.16.1,
    /// Table15-9: encoding1 includes all ASIDs and global translations.
    /// Call after monitor changes to live NPT mappings/permissions as well as
    /// emulated paging controls. This is a request, not a completed shootdown.
    pub fn request_full_tlb_flush(&mut self) {
        self.bytes[0x05c] = 1;
        self.invalidate_all();
    }

    /// Consume only the request submitted to the preceding successful entry.
    /// VMRUN reads but does not clear TLB_CONTROL (APM2 15.16.1 p529).
    /// Invalid entry leaves the pending request intact.
    ///
    /// # Safety
    /// The caller must establish an actual VMRUN/VMEXIT on this exclusively
    /// owned VMCB, on its owning CPU, and call before any post-exit operation
    /// can request another flush. Inert exit bytes alone are not that proof.
    pub unsafe fn consume_tlb_flush_after_exit(&mut self) {
        if self.exit_snapshot().code == u64::MAX || self.exit_snapshot().code == u32::MAX as u64 {
            return;
        }
        if self.bytes[0x05c] == 1 {
            self.bytes[0x05c] = 0;
            self.invalidate_all();
        }
    }

    /// Same-CPU CPUID Fn8000000A.EDX evidence; native prepare only. APM2
    /// 15.14.4/TableB-1: nonzero count and zero threshold enable count-only
    /// filtering. Unsupported CPUs remain unchanged, avoiding exit-per-PAUSE.
    pub fn configure_native_pause_filter(&mut self, svm_features: u32) -> bool {
        if svm_features & (1 << 10) == 0 {
            return false;
        }
        self.bytes[0x03c..0x03e].copy_from_slice(&0u16.to_le_bytes());
        self.bytes[0x03e..0x040].copy_from_slice(&4096u16.to_le_bytes());
        self.update_intercept::<INTERCEPT_MISC1>(1 << 23, true);
        self.invalidate_all();
        true
    }

    pub fn set_instruction_intercept(&mut self, intercept: InstructionIntercept, enabled: bool) {
        let (offset, mask) = intercept.location();
        if offset == INTERCEPT_MISC1 {
            self.update_intercept::<INTERCEPT_MISC1>(mask, enabled);
        } else {
            self.update_intercept::<INTERCEPT_MISC2>(mask, enabled);
        }
        self.invalidate_all();
    }

    pub fn instruction_intercept(&self, intercept: InstructionIntercept) -> bool {
        let (offset, mask) = intercept.location();
        let word = if offset == INTERCEPT_MISC1 {
            self.read_u32::<INTERCEPT_MISC1>()
        } else {
            self.read_u32::<INTERCEPT_MISC2>()
        };
        word & mask != 0
    }

    /// Set only the selected event intercept; preserve all other
    /// controls and invalidate cached VMCB fields. This is an inert byte edit,
    /// not activation or evidence that hardware will honor the SMI intercept.
    pub fn set_event_intercept(&mut self, intercept: EventIntercept, enabled: bool) {
        let mask = intercept.mask();
        self.update_intercept::<INTERCEPT_MISC1>(mask, enabled);
        self.invalidate_all();
    }

    pub fn event_intercept(&self, intercept: EventIntercept) -> bool {
        self.read_u32::<INTERCEPT_MISC1>() & intercept.mask() != 0
    }

    /// Prepare classic physical INTR interception independent of guest IF/CR8.
    /// APM2 rev3.44 15.13.1, 15.21.1-2, Appendix B: INTR intercept plus
    /// V_INTR_MASKING lets the host IF saved at VMRUN gate physical interrupts.
    /// The caller must separately own the physical source, host IF/GIF/TPR,
    /// acknowledgement, host IDT, entry/exit assembly and source cleanup.
    /// This inert setup does not establish any of that execution evidence.
    /// Pending delivery and unsupported controls refuse without byte changes.
    pub fn enable_physical_interrupt_virtualization(
        &mut self,
    ) -> Result<(), ExternalInterruptError> {
        self.validate_external_interrupt_conflicts()?;
        self.validate_virtual_interrupt_controls()?;
        if self.virtual_interrupt_control() & V_IRQ != 0 {
            return Err(ExternalInterruptError::PendingVirtualInterrupt);
        }
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(self.virtual_interrupt_control() | (1 << 24));
        self.set_event_intercept(EventIntercept::PhysicalInterrupt, true);
        Ok(())
    }

    /// Conservatively declare all cached fields dirty. No clean-bit setter is
    /// exposed until CPU-local reuse and state-cache ownership are implemented.
    pub fn invalidate_all(&mut self) {
        self.write_u32::<CLEAN_BITS>(0);
    }

    pub fn guest_rip(&self) -> u64 {
        self.read_u64::<GUEST_RIP>()
    }

    pub fn guest_rsp(&self) -> u64 {
        self.read_u64::<GUEST_RSP>()
    }

    pub fn guest_rax(&self) -> u64 {
        self.read_u64::<GUEST_RAX>()
    }

    /// CR8's REX encoding requires 64-bit code, not compatibility mode.
    /// APM vol.2 rev.3.44 Appendix B: EFER.LMA and saved CS attribute L.
    pub(crate) fn guest_in_64_bit_code(&self) -> bool {
        self.read_u64::<GUEST_EFER>() & (1 << 10) != 0 && self.bytes[0x413] & 2 != 0
    }

    /// Stopped guest page-table root from the architectural state-save area.
    pub fn guest_cr3(&self) -> u64 { self.read_u64::<GUEST_CR3>() }

    pub fn guest_cr2(&self) -> u64 {
        self.read_u64::<0x640>()
    }

    /// Retained EVENTINJ bytes, not evidence of pending or completed delivery.
    pub fn event_injection(&self) -> u64 {
        self.read_u64::<0x0a8>()
    }

    /// Retained classic virtual interrupt controls (APM Appendix B, 60h).
    pub fn virtual_interrupt_control(&self) -> u64 {
        self.read_u64::<VIRTUAL_INTERRUPT_CONTROL>()
    }

    /// Retained interrupt-shadow state; not a GIF observation. Ordinary VMRUN
    /// sets GIF; this bounded path rejects virtual-GIF and encrypted state.
    pub fn interrupt_shadow(&self) -> bool {
        self.read_u64::<0x068>() & 1 != 0
    }

    pub(crate) fn guest_rflags(&self) -> u64 {
        self.read_u64::<GUEST_RFLAGS>()
    }

    /// Change the classic virtual TPR priority class, preserving an armed IRQ.
    /// This does not change a physical APIC TPR or implement APIC MMIO/EOI.
    pub fn set_virtual_interrupt_tpr(
        &mut self,
        priority: u8,
    ) -> Result<(), ExternalInterruptError> {
        if priority > 15 {
            return Err(ExternalInterruptError::InvalidTaskPriority { priority });
        }
        self.validate_virtual_interrupt_controls()?;
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(
            (self.virtual_interrupt_control() & !0xf) | priority as u64,
        );
        self.invalidate_all();
        Ok(())
    }

    /// Arm one new maskable IRQ without changing guest registers or RIP.
    ///
    /// APM 15.21.4: V_IRQ waits for guest IF=1, GIF=1, no interrupt shadow,
    /// and priority strictly above V_TPR. EVENTINJ would bypass these gates and
    /// is deliberately not used. V_INTR_MASKING is enabled, V_IGN_TPR remains
    /// clear. The caller must separately own host interrupt masking and the
    /// physical APIC; this method does not establish a safe entry boundary.
    /// Refusals preserve both VMCB and request. Resume an armed request without
    /// calling this method again; observe the actual exit before retiring it.
    pub fn arm_external_interrupt(
        &mut self,
        request: &mut PendingExternalInterrupt,
    ) -> Result<(), ExternalInterruptError> {
        if request.state != ExternalInterruptState::Queued {
            return Err(ExternalInterruptError::RequestNotQueued);
        }
        self.validate_external_interrupt_conflicts()?;
        self.validate_virtual_interrupt_controls()?;
        if self.virtual_interrupt_control() & V_IRQ != 0 {
            return Err(ExternalInterruptError::PendingVirtualInterrupt);
        }
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(
            (self.virtual_interrupt_control() & 0xf) | request.control() | V_IRQ,
        );
        self.invalidate_all();
        request.state = ExternalInterruptState::Armed;
        Ok(())
    }

    /// Account for this armed request after a caller-established real VM exit.
    ///
    /// APM 15.21.4 clears V_IRQ before IDT access, so clearing alone cannot
    /// prove delivery: failed entry and valid EXITINTINFO must be refused.
    /// An Armed result retains the request for the next entry. Consumed retires
    /// it exactly once but does not prove handler completion or EOI. Neither
    /// success nor refusal edits the VMCB. Never call against pre-entry fields.
    pub fn observe_external_interrupt_after_exit(
        &self,
        request: &mut PendingExternalInterrupt,
    ) -> Result<ExternalInterruptState, ExternalInterruptError> {
        let state = self.external_interrupt_state_after_exit(request)?;
        request.state = state;
        Ok(state)
    }

    fn external_interrupt_state_after_exit(
        &self,
        request: &PendingExternalInterrupt,
    ) -> Result<ExternalInterruptState, ExternalInterruptError> {
        if request.state != ExternalInterruptState::Armed {
            return Err(ExternalInterruptError::RequestNotArmed);
        }
        if self.read_u64::<0x070>() == u64::MAX {
            return Err(ExternalInterruptError::InvalidEntry);
        }
        self.validate_external_interrupt_conflicts()?;
        self.validate_virtual_interrupt_controls()?;
        let control = self.virtual_interrupt_control();
        if control & !(0xf | V_IRQ) != request.control() {
            return Err(ExternalInterruptError::ControlMismatch);
        }
        if control & V_IRQ == 0 {
            if self.read_u64::<0x070>() == 0x64 {
                return Err(ExternalInterruptError::InconsistentVirtualInterruptExit);
            }
            return Ok(ExternalInterruptState::Consumed);
        }
        Ok(ExternalInterruptState::Armed)
    }

    pub(crate) fn validate_external_interrupt_conflicts(
        &self,
    ) -> Result<(), ExternalInterruptError> {
        // No saved IRQ/control field can establish ownership after shutdown.
        if self.read_u64::<0x070>() == 0x7f {
            return Err(ExternalInterruptError::GuestShutdown);
        }
        if self.event_injection() & (1 << 31) != 0 {
            return Err(ExternalInterruptError::PendingInjection);
        }
        if self.read_u64::<0x088>() & (1 << 31) != 0 {
            return Err(ExternalInterruptError::NestedDeliveryUnsupported);
        }
        Ok(())
    }

    pub(crate) fn validate_virtual_interrupt_controls(&self) -> Result<(), ExternalInterruptError> {
        let control = self.virtual_interrupt_control();
        // Explicitly excludes AVIC, virtual GIF/NMI, V_IGN_TPR and reserved bits.
        if control & !SUPPORTED_VIRTUAL_INTERRUPT_CONTROL != 0 {
            return Err(ExternalInterruptError::UnsupportedControl { control });
        }
        let control = self.read_u64::<0x090>();
        // Classic unencrypted SVM only; NP enable is the only admitted field.
        // SEV/ES/SNP and alternate injection require different state owners.
        if control & !1 != 0 {
            return Err(ExternalInterruptError::UnsupportedNestedControl { control });
        }
        Ok(())
    }

    /// Queue a supported fault from this stopped VMCB's actual exit fields.
    ///
    /// The caller owns stop/entry synchronization and must establish that these
    /// fields describe a completed exit. Refusal preserves every byte. Success
    /// preserves RIP and writes only EVENTINJ, clean bits, and CR2 for #PF.
    /// No guest entry, nested-delivery resolution, or OS readiness is implied.
    /// AMD APM vol.2 rev.3.44 sections 15.7, 15.12.15, 15.20, Appendix B.
    pub fn reflect_exception(&mut self) -> Result<ReflectedException, ReflectionError> {
        if self.read_u64::<0x070>() == 0x7f {
            return Err(ReflectionError::GuestShutdown);
        }
        if self.virtual_interrupt_control() & V_IRQ != 0 {
            return Err(ReflectionError::PendingVirtualInterrupt);
        }
        let reflected = events::prepare(
            self.read_u64::<0x070>(),
            self.read_u64::<0x078>(),
            self.read_u64::<0x080>(),
            self.read_u64::<0x088>(),
            self.event_injection(),
        )?;
        if let ReflectedException::PageFault { address, .. } = reflected {
            // An intercepted #PF has not updated CR2 (APM 15.12.15).
            self.write_u64::<0x640>(address);
        }
        self.write_u64::<0x0a8>(reflected.encoding());
        self.invalidate_all();
        Ok(reflected)
    }

    /// Resolve a fault intercepted DURING IDT exception delivery after a real exit.
    ///
    /// The caller exclusively owns the stopped VMCB and establishes that any
    /// retained EVENTINJ belongs to the immediately preceding entry, not a newly
    /// queued event. Call before clearing that request, and do not settle an APIC
    /// flight from this interrupted exit. Only type-3 #UD/#GP/#PF/#DF interrupted
    /// by #NP/#SS/#GP/#PF is admitted. No fault repair or original-event replay.
    /// A still-valid prior request must match EXITINTINFO; a cleared V is ignored.
    ///
    /// Success replaces EVENTINJ and updates CR2 for an intercepted #PF, including
    /// one combined into #DF. RIP, RSP, flags and EXITINTINFO evidence are retained.
    /// #DF's saved RIP is undefined and cannot authorize restart or IRETQ retry.
    /// Shutdown is terminal and preserves EVERY byte; intercepted shutdown does
    /// not interpret any saved state. Refusal likewise preserves every byte.
    /// The next actual exit must precede any retirement of the new request.
    /// APM2 rev3.44 8.2.9/Table8-3, 15.7.2–3, 15.12.15, 15.14.3, 15.20, App.B/C.
    pub fn resolve_exception_delivery_after_exit(
        &mut self,
    ) -> Result<DeliveryOutcome, ReflectionError> {
        let code = self.read_u64::<0x070>();
        if code == 0x7f {
            return Ok(DeliveryOutcome::Shutdown(GuestShutdown::Intercepted));
        }
        if code == u64::MAX {
            return Err(ReflectionError::InvalidEntry);
        }
        self.validate_virtual_interrupt_controls()
            .map_err(ReflectionError::Control)?;
        if self.virtual_interrupt_control() & V_IRQ != 0 {
            return Err(ReflectionError::PendingVirtualInterrupt);
        }
        let outcome = events::prepare_interrupted_delivery(
            code,
            self.read_u64::<0x078>(),
            self.read_u64::<0x080>(),
            self.read_u64::<0x088>(),
            self.event_injection(),
        )?;
        if let DeliveryOutcome::Injected(event) = outcome {
            if code == 0x4e {
                // APM15.12.15: interception did not write CR2. Reflection must,
                // even when this #PF contributes to an injected double fault.
                self.write_u64::<0x640>(self.read_u64::<0x080>());
            }
            self.write_u64::<0x0a8>(event.encoding());
            self.invalidate_all();
        }
        Ok(outcome)
    }

    /// Queue #GP(0) only after the MSR policy requires that architectural fault.
    /// Caller establishes a real stopped MSR exit and immutable instruction bytes
    /// from the same guest. This validates the faulting instruction, not a resume
    /// address: RIP, GPRs and CR2 remain unchanged. EVENTINJ is a request, not
    /// delivery proof; observe actual exit before clearing it. Nested recovery
    /// and competing injection/V_IRQ are deliberately refused.
    /// AMD APM vol.2 rev.3.44 sections 15.11, 15.20 and Appendix B.
    pub fn queue_msr_general_protection(
        &mut self,
        instruction: &[u8],
    ) -> Result<(), events::MsrFaultError> {
        self.queue_validated_msr_general_protection(super::exit::MsrInstruction::Bytes(instruction))
    }

    pub(crate) fn queue_validated_msr_general_protection(
        &mut self, instruction: super::exit::MsrInstruction<'_>,
    ) -> Result<(), events::MsrFaultError> {
        use events::MsrFaultError;
        instruction.validate(self.exit_snapshot()).map_err(MsrFaultError::Instruction)?;
        self.queue_native_general_protection().map_err(MsrFaultError::State)
    }

    /// Queue #GP(0) after a native instruction owner has validated this actual
    /// stopped exit and established the architectural fault condition. This
    /// does not validate an opcode or invent a fault for unsupported policy.
    /// APM2 15.20: fault injection preserves the faulting RIP and all GPRs.
    pub(crate) fn queue_native_general_protection(
        &mut self,
    ) -> Result<(), ExternalInterruptError> {
        self.validate_external_interrupt_conflicts()?;
        if self.virtual_interrupt_control() & super::x2avic::ENABLE_BITS != 0 {
            self.validate_native_x2avic_controls()?;
        } else {
            self.validate_virtual_interrupt_controls()?;
        }
        if self.virtual_interrupt_control() & V_IRQ != 0 {
            return Err(ExternalInterruptError::PendingVirtualInterrupt);
        }
        self.write_u64::<0x0a8>(ReflectedException::GeneralProtection { error_code: 0 }.encoding());
        self.invalidate_all();
        Ok(())
    }

    /// Clear the previous entry's injection request after a completed exit.
    ///
    /// EVENTINJ is an input request (APM 15.20), not a delivery acknowledgement.
    /// The caller must account for failed entry and EXITINTINFO before clearing;
    /// valid EXITINTINFO requires delivery recovery outside this bounded API.
    /// Never clear a newly queued request before the intended VMRUN.
    pub fn clear_event_injection_after_exit(&mut self) -> Result<(), ReflectionError> {
        if self.read_u64::<0x070>() == 0x7f {
            return Err(ReflectionError::GuestShutdown);
        }
        if self.read_u64::<0x070>() == u64::MAX {
            return Err(ReflectionError::InvalidEntry);
        }
        if self.read_u64::<0x088>() & (1 << 31) != 0 {
            return Err(ReflectionError::NestedDeliveryUnsupported);
        }
        self.write_u64::<0x0a8>(0);
        self.invalidate_all();
        Ok(())
    }

    /// Write only the validated synthetic register tuple. Segment/descriptor
    /// state, page contents, NPT translation and launch readiness remain absent.
    pub fn set_synthetic_state(&mut self, state: &ValidatedGuestState) {
        self.write_u64::<GUEST_EFER>(state.efer());
        self.write_u64::<GUEST_CR4>(state.cr4());
        self.write_u64::<GUEST_CR3>(state.cr3());
        self.write_u64::<GUEST_CR0>(state.cr0());
        self.write_u64::<GUEST_RFLAGS>(state.rflags());
        self.write_u64::<GUEST_RIP>(state.rip());
        self.write_u64::<GUEST_RSP>(state.rsp());
        self.write_u64::<GUEST_RAX>(state.rax());
        self.request_full_tlb_flush();
    }

    /// Core-only application of the native adapter's prepared bootstrap state.
    /// All destination checks precede the first write. APM2 rev3.44 15.5.1/2,
    /// 15.7 and Appendix B; only VMSAVE-defined auxiliary fields are imported.
    /// Source control/reserved/exit bytes are never interpreted as CPU state.
    pub(crate) fn apply_native_continuation(
        &mut self,
        prepared: &crate::guest::continuation::PreparedNativeContinuation<'_>,
    ) -> Result<(), crate::guest::continuation::NativeContinuationError> {
        use crate::guest::continuation::NativeContinuationError as E;
        if self.event_injection() != 0
            || self.read_u64::<0x088>() != 0
            || self.read_u64::<0x068>() != 0
            || self.read_u64::<0x070>() != 0
            || self.virtual_interrupt_control() & !(0xf | (1 << 24)) != 0
            || self.read_u64::<0x090>() & !1 != 0
            || self.read_u64::<0x0b8>() != 0
        {
            return Err(E::DestinationEventState);
        }
        let r = &prepared.request;
        let s = r.entry;
        self.write_segment::<0x410>(prepared.segments[0]);
        self.write_segment::<0x420>(prepared.segments[1]);
        self.write_segment::<0x430>(prepared.segments[2]);
        self.write_segment::<0x400>(prepared.segments[3]);
        let gdtr = r.gdt.table();
        self.write_segment::<0x460>(SegmentState {
            selector: 0,
            attributes: 0,
            limit: u32::from(gdtr.limit),
            base: gdtr.base,
        });
        self.write_segment::<0x480>(SegmentState {
            selector: 0,
            attributes: 0,
            limit: u32::from(r.idtr.limit),
            base: r.idtr.base,
        });
        for (start, end) in [
            (0x440, 0x460),
            (0x470, 0x480),
            (0x490, 0x4a0),
            (0x600, 0x640),
        ] {
            self.bytes[start..end].copy_from_slice(&r.auxiliary.bytes()[start..end]);
        }
        self.bytes[0x4cb] = 0;
        self.write_u64::<GUEST_EFER>(s.efer | (1 << 12));
        self.write_u64::<GUEST_CR0>(s.cr0);
        self.write_u64::<GUEST_CR3>(s.cr3);
        self.write_u64::<GUEST_CR4>(s.cr4);
        self.write_u64::<GUEST_RFLAGS>(s.rflags);
        self.write_u64::<GUEST_RIP>(s.rip);
        self.write_u64::<GUEST_RSP>(s.rsp);
        self.write_u64::<GUEST_RAX>(s.rax);
        self.write_u64::<0x640>(r.cr2);
        self.write_u64::<0x560>(r.dr7);
        self.write_u64::<0x568>(r.dr6);
        self.write_u64::<0x668>(r.pat);
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(
            (self.virtual_interrupt_control() & !0xf) | r.cr8,
        );
        self.request_full_tlb_flush();
        Ok(())
    }

    /// RAX is uninterpreted register data.
    pub fn set_guest_rax(&mut self, rax: u64) {
        self.write_u64::<GUEST_RAX>(rax);
        self.invalidate_all();
    }

    /// Import the separately sampled native CET MSRs before the first entry.
    /// VMSAVE does not capture these fields (APM3 rev3.37 VMSAVE); their VMCB
    /// locations are APM2 rev3.44 Table B-2. CR4.CET must still be disabled at
    /// this initial boundary. This does not capture a dormant hardware SSP or
    /// authorize an already active shadow-stack continuation. U_CET, PLn_SSP
    /// and XSS remain live on the same physical CPU, unused by the monitor.
    /// The caller must have enumerated CET_SS before reading these MSRs.
    pub fn initialize_native_cet_msrs(&mut self, s_cet: u64, isst_addr: u64)
        -> Result<(), crate::guest::continuation::NativeContinuationError>
    {
        use crate::guest::continuation::NativeContinuationError as E;
        if self.read_u64::<GUEST_CR4>() & (1 << 23) != 0 || s_cet & !3 != 0 {
            return Err(E::UnsupportedCr4);
        }
        if !crate::memory::address::is_canonical_48(isst_addr) {
            return Err(E::NoncanonicalAddress);
        }
        self.write_u64::<GUEST_S_CET>(s_cet);
        self.write_u64::<GUEST_ISST_ADDR>(isst_addr);
        self.invalidate_all();
        Ok(())
    }

    /// Target-owned INIT processor state. APM2 rev3.44 Table14-1/2, printed
    /// 481-483. DR6/7 reset here; the caller resets its guest-owned live DR0-3
    /// at the same stopped commit. PAT, auxiliary MSRs, live xstate, XCR0,
    /// MTRRs and other INIT-retained resources stay with the caller.
    /// This does not implement LAPIC initialization or permit guest entry.
    pub(crate) fn initialize_ap_after_init(&mut self) {
        let data = SegmentState {
            selector: 0,
            attributes: 0x92,
            limit: 0xffff,
            base: 0,
        };
        self.write_segment::<0x400>(data);
        self.write_segment::<0x420>(data);
        self.write_segment::<0x430>(data);
        self.write_segment::<0x440>(data);
        self.write_segment::<0x450>(data);
        self.write_segment::<0x410>(SegmentState {
            selector: 0xf000,
            attributes: 0x9a,
            limit: 0xffff,
            base: 0xffff_0000,
        });
        self.write_segment::<0x460>(SegmentState {
            attributes: 0,
            ..data
        });
        self.write_segment::<0x480>(SegmentState {
            attributes: 0,
            ..data
        });
        self.write_segment::<0x470>(SegmentState {
            attributes: 0x82,
            ..data
        });
        self.write_segment::<0x490>(SegmentState {
            attributes: 0x83,
            ..data
        });
        self.bytes[0x4cb] = 0;
        self.write_u64::<GUEST_EFER>(0x1000);
        self.write_u64::<GUEST_CR0>((self.read_u64::<GUEST_CR0>() & 0x6000_0000) | 0x10);
        self.write_u64::<GUEST_CR3>(0);
        self.write_u64::<GUEST_CR4>(0);
        self.write_u64::<0x560>(0x400); // DR7, APM2 Table14-1 p482.
        self.write_u64::<0x568>(0xffff_0ff0); // DR6, same INIT value as RESET.
        self.write_u64::<0x640>(0); // CR2
        self.write_u64::<GUEST_RFLAGS>(2);
        self.write_u64::<GUEST_RIP>(0xfff0);
        self.write_u64::<GUEST_RSP>(0);
        self.write_u64::<GUEST_RAX>(0);
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(
            self.virtual_interrupt_control() & ((1 << 24) | super::x2avic::ENABLE_BITS),
        );
        self.write_u64::<0x068>(0); // interrupt shadow
        self.request_full_tlb_flush();
    }

    /// APM2 15.27.8: vector gives upper8 of20-bit address,16-bit real mode,
    /// page offset0; ordinary real-mode CS formation (14.1.5) gives vector<<8.
    pub(crate) fn start_ap_from_sipi(&mut self, vector: u8) {
        self.write_segment::<0x410>(SegmentState {
            selector: (vector as u16) << 8,
            attributes: 0x9a,
            limit: 0xffff,
            base: (vector as u64) << 12,
        });
        self.write_u64::<GUEST_RIP>(0);
        self.invalidate_all();
    }

    /// Only the crate's dispatcher may commit a checked instruction outcome.
    /// This changes stored state; it does not run the guest or flush a TLB.
    pub(crate) fn commit_emulated_instruction(&mut self, rax: u64, next: ResumeCandidate) {
        self.write_u64::<GUEST_RAX>(rax);
        self.write_u64::<GUEST_RIP>(next.address());
        self.invalidate_all();
    }

    /// EFER policy has checked the logical value and its architectural faults.
    /// Preserve mandatory hardware SVME and invalidate translations after a
    /// paging-permission change (APM2 15.16, Appendix B TLB_CONTROL=1).
    pub(crate) fn commit_native_efer(&mut self, logical: u64) {
        self.write_u64::<GUEST_EFER>(logical | (1 << 12));
        self.request_full_tlb_flush();
    }

    /// The native CPUID/MSR owner completed the instruction rather than
    /// retrying a fault. Consume STI/MOV-SS shadow and RF (APM2 15.21.5,
    /// 3.1.6). TF requires a separately owned post-instruction #DB and is
    /// refused by these native handlers before they commit anything.
    pub(crate) fn complete_native_instruction_state(&mut self) {
        self.write_u64::<0x068>(self.read_u64::<0x068>() & !1);
        self.write_u64::<GUEST_RFLAGS>(self.guest_rflags() & !(1 << 16));
        self.invalidate_all();
    }

    /// Install the fixed synthetic segment state from validated guest images.
    /// GDT/TSS bytes must separately be copied and mapped at their guest VAs.
    /// No IDT, auxiliary state capture or hardware loading is performed here.
    pub fn set_guest_descriptors(&mut self, descriptors: &ValidatedGuestDescriptors) {
        self.write_segment::<0x400>(descriptors.data());
        self.write_segment::<0x420>(descriptors.data());
        self.write_segment::<0x430>(descriptors.data());
        self.write_segment::<0x440>(descriptors.data());
        self.write_segment::<0x450>(descriptors.data());
        self.write_segment::<0x410>(descriptors.cs());
        self.write_segment::<0x460>(descriptors.gdtr());
        self.write_segment::<0x470>(SegmentState {
            selector: 0,
            attributes: 0,
            limit: 0,
            base: 0,
        });
        self.write_segment::<0x490>(descriptors.tr());
        self.bytes[0x4cb] = 0; // CPL, independent of descriptor DPL.
        self.invalidate_all();
    }

    #[inline(always)]
    fn write_segment<const OFFSET: usize>(&mut self, segment: SegmentState) {
        const {
            assert!(OFFSET <= VMCB_BYTES - 16);
        }
        let bytes = segment
            .selector
            .to_le_bytes()
            .into_iter()
            .chain(segment.attributes.to_le_bytes())
            .chain(segment.limit.to_le_bytes())
            .chain(segment.base.to_le_bytes());
        for (dst, src) in self.bytes.iter_mut().skip(OFFSET).zip(bytes) {
            *dst = src;
        }
    }

    #[inline(always)]
    fn read_u32<const OFFSET: usize>(&self) -> u32 {
        const {
            assert!(OFFSET <= VMCB_BYTES - 4);
        }
        let mut bytes = [0; 4];
        for (dst, src) in bytes.iter_mut().zip(self.bytes.iter().skip(OFFSET)) {
            *dst = *src;
        }
        u32::from_le_bytes(bytes)
    }

    #[inline(always)]
    fn read_u64<const OFFSET: usize>(&self) -> u64 {
        const {
            assert!(OFFSET <= VMCB_BYTES - 8);
        }
        let mut bytes = [0; 8];
        for (dst, src) in bytes.iter_mut().zip(self.bytes.iter().skip(OFFSET)) {
            *dst = *src;
        }
        u64::from_le_bytes(bytes)
    }

    #[inline(always)]
    fn write_u32<const OFFSET: usize>(&mut self, value: u32) {
        const {
            assert!(OFFSET <= VMCB_BYTES - 4);
        }
        for (dst, src) in self.bytes.iter_mut().skip(OFFSET).zip(value.to_le_bytes()) {
            *dst = src;
        }
    }

    #[inline(always)]
    fn write_u64<const OFFSET: usize>(&mut self, value: u64) {
        const {
            assert!(OFFSET <= VMCB_BYTES - 8);
        }
        for (dst, src) in self.bytes.iter_mut().skip(OFFSET).zip(value.to_le_bytes()) {
            *dst = src;
        }
    }

    fn update_intercept<const OFFSET: usize>(&mut self, mask: u32, enabled: bool) {
        let previous = self.read_u32::<OFFSET>();
        self.write_u32::<OFFSET>(if enabled {
            previous | mask
        } else {
            previous & !mask
        });
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;

    #[test]
    fn identity_offset_clears_stale_offset_and_invalidates_clean_bits_only() {
        let mut vmcb = Vmcb::new();
        vmcb.set_instruction_intercept(InstructionIntercept::Rdtscp, true);
        vmcb.write_u64::<TSC_OFFSET>(u64::MAX);
        vmcb.write_u32::<CLEAN_BITS>(u32::MAX);
        let mut expected = *vmcb.bytes();
        expected[TSC_OFFSET..TSC_OFFSET + 8].fill(0);
        expected[CLEAN_BITS..CLEAN_BITS + 4].fill(0);
        vmcb.set_tsc_offset_zero();
        assert_eq!(vmcb.tsc_offset(), 0);
        assert_eq!(vmcb.bytes(), &expected);
    }
}

#[cfg(test)]
mod tlb_lifecycle_tests {
    use super::*;

    #[test]
    fn successful_entry_consumes_once_and_invalid_entry_keeps_request() {
        let mut vmcb = Vmcb::new();
        vmcb.request_full_tlb_flush();
        for invalid in [u64::MAX, u32::MAX as u64] {
            vmcb.write_u64::<0x070>(invalid);
            let before = vmcb.bytes;
            // Synthetic model of the assembly's post-entry call, not hardware proof.
            unsafe { vmcb.consume_tlb_flush_after_exit(); }
            assert_eq!(vmcb.bytes, before);
        }
        vmcb.write_u64::<0x070>(0x72);
        unsafe { vmcb.consume_tlb_flush_after_exit(); }
        assert_eq!(vmcb.bytes[0x05c], 0);
        vmcb.set_guest_rax(42);
        unsafe { vmcb.consume_tlb_flush_after_exit(); }
        assert_eq!(vmcb.bytes[0x05c], 0);
        vmcb.commit_native_efer(0xd01);
        assert_eq!(vmcb.bytes[0x05c], 1);
        unsafe { vmcb.consume_tlb_flush_after_exit(); }
        vmcb.initialize_ap_after_init();
        assert_eq!(vmcb.bytes[0x05c], 1);
        vmcb.start_ap_from_sipi(8);
        assert_eq!(vmcb.bytes[0x05c], 1);
        unsafe { vmcb.consume_tlb_flush_after_exit(); }
        assert_eq!(vmcb.bytes[0x05c], 0);
    }

    #[test]
    fn nested_root_change_requests_flush_but_rejected_root_keeps_state() {
        let policy = AddressPolicy::new(48, crate::memory::address::EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
        let mut vmcb = Vmcb::new();
        vmcb.set_nested_root(0x1000, &policy).unwrap();
        assert_eq!(vmcb.bytes[0x05c], 1);
        vmcb.write_u64::<0x070>(0x400);
        unsafe { vmcb.consume_tlb_flush_after_exit(); }
        let before = vmcb.bytes;
        assert!(vmcb.set_nested_root(0x2001, &policy).is_err());
        assert_eq!(vmcb.bytes, before);
        vmcb.set_nested_root(0x2000, &policy).unwrap();
        assert_eq!(vmcb.bytes[0x05c], 1);
        assert_eq!(vmcb.nested_root(), 0x2000);
    }
}
