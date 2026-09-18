//! Inert, byte-exact VMCB storage for the unencrypted SVM foundation.
//!
//! Offsets and widths follow AMD APM volume 2 revision 3.44, Appendix B,
//! tables B-1 and B-2. State-save offsets include the 0x400 area base.
//! Zero initialization preserves reserved bytes; it does not create a guest
//! that can run. This module provides no launch, allocation, physical-address
//! translation, guest-state completeness check, or hardware access.

use crate::{
    arch::x86_64::descriptors::SegmentState,
    svm::{
        events::{self, ExternalInterruptError, ReflectedException},
        exit::ExitSnapshot,
    },
};

pub use crate::svm::vmcb::reflection::ReinjectOutcome;

mod continuation;
mod control;
mod interrupt;
mod reflection;
#[cfg(test)]
mod tests;

pub const VMCB_BYTES: usize = 4096;

const INTERCEPT_MISC1: usize = 0x00c;
const INTERCEPT_MISC2: usize = 0x010;
const IOPM_BASE: usize = 0x040;
const MSRPM_BASE: usize = 0x048;
const TSC_OFFSET: usize = 0x050;
const GUEST_ASID: usize = 0x058;
const VIRTUAL_INTERRUPT_CONTROL: usize = 0x060;
const V_IRQ: u64 = 1 << 8;
/// V_NMI (offset 60h bit 11): a virtual NMI is pending (APM2 rev3.44 Table
/// B-1 p740, 15.21.10 p536). V_NMI_MASK (bit 12), which blocks a second one
/// until the guest IRETs, and V_NMI_ENABLE (bit 26,
/// `super::x2avic::V_NMI_ENABLE`) are cleared/preserved by bit position in
/// `initialize_ap_after_init`.
const V_NMI: u64 = 1 << 11;
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

/// CPU layout alignment only: the object's address is not a physical address.
/// No writable byte view is exposed, so callers cannot modify reserved fields.
#[repr(C, align(4096))]
pub struct Vmcb {
    bytes: [u8; VMCB_BYTES],
}

// Storage, retained-state accessors and raw field access.
impl Vmcb {
    pub const fn new() -> Self {
        Self { bytes: [0; VMCB_BYTES] }
    }

    pub const fn bytes(&self) -> &[u8; VMCB_BYTES] {
        &self.bytes
    }

    /// Read retained exit fields. Meaningful only after a separately established
    /// exit; this neither synchronizes with hardware nor captures other GPRs.
    pub fn exit_snapshot(&self) -> ExitSnapshot {
        ExitSnapshot::from_vmcb_bytes(&self.bytes)
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

    /// Stopped guest page-table root from the architectural state-save area.
    pub fn guest_cr3(&self) -> u64 {
        self.read_u64::<GUEST_CR3>()
    }

    pub fn guest_cr2(&self) -> u64 {
        self.read_u64::<0x640>()
    }

    pub(crate) fn guest_rflags(&self) -> u64 {
        self.read_u64::<GUEST_RFLAGS>()
    }

    /// Retained EVENTINJ bytes, not evidence of pending or completed delivery.
    pub fn event_injection(&self) -> u64 {
        self.read_u64::<0x0a8>()
    }

    /// Retained classic virtual interrupt controls (APM Appendix B, 60h).
    pub fn virtual_interrupt_control(&self) -> u64 {
        self.read_u64::<VIRTUAL_INTERRUPT_CONTROL>()
    }

    /// CR8's REX encoding requires 64-bit code, not compatibility mode.
    /// APM vol.2 rev.3.44 Appendix B: EFER.LMA and saved CS attribute L.
    pub(crate) fn guest_in_64_bit_code(&self) -> bool {
        self.read_u64::<GUEST_EFER>() & (1 << 10) != 0 && self.bytes[0x413] & 2 != 0
    }

    /// Retained interrupt-shadow state; not a GIF observation. Ordinary VMRUN
    /// sets GIF; this bounded path rejects virtual-GIF and encrypted state.
    pub fn interrupt_shadow(&self) -> bool {
        self.read_u64::<0x068>() & 1 != 0
    }

    /// Conservatively declare all cached fields dirty. No clean-bit setter is
    /// exposed until CPU-local reuse and state-cache ownership are implemented.
    pub fn invalidate_all(&mut self) {
        self.write_u32::<CLEAN_BITS>(0);
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
        self.write_u32::<OFFSET>(if enabled { previous | mask } else { previous & !mask });
    }
}

// x2AVIC fields.
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
        // NATIVE_CONTROL sets V_NMI_ENABLE (bit 26). APM2 15.21.10 p536:
        // enabling NMI virtualization requires the NMI intercept, or VMRUN
        // exits with VMEXIT_INVALID; with it set the intercept then applies
        // only to physical NMIs, which the host re-presents as V_NMI.
        self.set_event_intercept(EventIntercept::Nmi, true);
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
        // Mask out V_TPR (3:0) and the hardware-owned V_NMI/V_NMI_MASK bits
        // (11/12, APM2 15.21.10 p536): the processor sets V_NMI_MASK and clears
        // V_NMI as it delivers a virtual NMI, and the runtime sets V_NMI to
        // re-present one, so any of those pending states is valid on VMRUN.
        // V_IRQ (bit 8) is hardware-owned too: #VMEXIT writes it back (15.6
        // p507, Table B-1 p740) and VMRUN ignores it while AVIC is enabled
        // (Table B-1 p740, 15.29.4.1 p570), so a guest holding an undelivered
        // IRR bit (IF=0) exits with it set. Software never sets it here.
        // V_INTR_PRIO (19:16), V_IGN_TPR (20) and V_INTR_VECTOR (39:32)
        // describe that same interrupt and are likewise ignored on VMRUN
        // under AVIC (Table B-1 p740-741). The manual does not say whether
        // plain AVIC writes them back (15.36.21.2 p619 has hardware update
        // them from the backing page), so they are not compared either.
        const AVIC_IGNORED: u64 = V_IRQ | (0xf << 16) | (1 << 20) | (0xff << 32);
        if control & !(0xf | AVIC_IGNORED | (1 << 11) | (1 << 12)) != super::x2avic::NATIVE_CONTROL
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
            // V_NMI_ENABLE in NATIVE_CONTROL requires the NMI intercept set
            // (APM2 15.21.10 p536), else VMRUN exits VMEXIT_INVALID.
            || !self.event_intercept(EventIntercept::Nmi)
            || self.event_intercept(EventIntercept::VirtualInterrupt)
        {
            return Err(ExternalInterruptError::ControlMismatch);
        }
        Ok(())
    }

    /// Set V_NMI (offset 60h bit 11): a virtual NMI pending in the guest.
    /// APM2 rev3.44 15.21.10 p536-537 and Table B-1 p740. The armed x2AVIC
    /// profile has V_NMI_ENABLE set, so VMRUN loads V_NMI and the processor
    /// takes the virtual NMI once virtual NMIs are unmasked (V_NMI_MASK clear),
    /// GIF/VGIF allow it and no interrupt shadow is active; a second virtual
    /// NMI is blocked by V_NMI_MASK until the guest completes an IRET. Virtual
    /// NMIs coalesce, so setting it while one is already pending is idempotent.
    /// The caller owns this stopped per-core VMCB; the profile recheck refuses
    /// a VMCB that is not the armed x2AVIC one (V_NMI without V_NMI_ENABLE has
    /// no effect on VMRUN, Table B-1 p740). RIP, GPRs and EVENTINJ are unchanged.
    pub fn set_guest_v_nmi_pending(
        &mut self,
        profile: &super::x2avic::NativeX2AvicProfile,
    ) -> Result<(), ExternalInterruptError> {
        if self.read_u64::<0x070>() == 0x7f {
            return Err(ExternalInterruptError::GuestShutdown);
        }
        self.validate_native_x2avic(profile)?;
        self.write_u64::<VIRTUAL_INTERRUPT_CONTROL>(self.virtual_interrupt_control() | V_NMI);
        self.invalidate_all();
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
}

// #GP(0) queueing for instruction owners.
impl Vmcb {
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
        &mut self,
        instruction: super::exit::MsrInstruction<'_>,
    ) -> Result<(), events::MsrFaultError> {
        use events::MsrFaultError;
        instruction.validate(self.exit_snapshot()).map_err(MsrFaultError::Instruction)?;
        self.queue_native_general_protection().map_err(MsrFaultError::State)
    }

    /// Queue #GP(0) after a native instruction owner has validated this actual
    /// stopped exit and established the architectural fault condition. This
    /// does not validate an opcode or invent a fault for unsupported policy.
    /// APM2 15.20: fault injection preserves the faulting RIP and all GPRs.
    pub(crate) fn queue_native_general_protection(&mut self) -> Result<(), ExternalInterruptError> {
        self.validate_external_interrupt_conflicts()?;
        if self.virtual_interrupt_control() & super::x2avic::ENABLE_BITS != 0 {
            // Under AVIC a written-back V_IRQ is hardware's IRR evaluation,
            // ignored on VMRUN (Table B-1 p740): not a competing injection.
            self.validate_native_x2avic_controls()?;
        } else {
            self.validate_virtual_interrupt_controls()?;
            if self.virtual_interrupt_control() & V_IRQ != 0 {
                return Err(ExternalInterruptError::PendingVirtualInterrupt);
            }
        }
        self.write_u64::<0x0a8>(ReflectedException::GeneralProtection { error_code: 0 }.encoding());
        self.invalidate_all();
        Ok(())
    }
}

// AP INIT and SIPI state.
impl Vmcb {
    /// Target-owned INIT processor state. APM2 rev3.44 Table14-1/2, printed
    /// 481-483. DR6/7 reset here; the caller resets its guest-owned live DR0-3
    /// at the same stopped commit. PAT, auxiliary MSRs, live xstate, XCR0,
    /// MTRRs and other INIT-retained resources stay with the caller.
    /// This does not implement LAPIC initialization or permit guest entry.
    pub(crate) fn initialize_ap_after_init(&mut self) {
        let data = SegmentState { selector: 0, attributes: 0x92, limit: 0xffff, base: 0 };
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
        self.write_segment::<0x460>(SegmentState { attributes: 0, ..data });
        self.write_segment::<0x480>(SegmentState { attributes: 0, ..data });
        self.write_segment::<0x470>(SegmentState { attributes: 0x82, ..data });
        self.write_segment::<0x490>(SegmentState { attributes: 0x83, ..data });
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
            self.virtual_interrupt_control()
                & ((1 << 24) | super::x2avic::ENABLE_BITS | super::x2avic::V_NMI_ENABLE),
        );
        // INIT clears any pending/masked virtual NMI (V_NMI, V_NMI_MASK) but
        // keeps V_NMI_ENABLE armed, matching Table 14-1's NMI reset: a fresh AP
        // takes NMIs once started (APM2 15.21.10 p537).
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
}

impl Default for Vmcb {
    fn default() -> Self {
        Self::new()
    }
}

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
