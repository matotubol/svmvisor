//! VMCB control-area configuration: TLB, ASID, permission maps and intercepts.

use crate::{
    arch::x86_64::capabilities::{CapabilityError, ValidatedCapabilities},
    memory::address::{AddressError, AddressPolicy},
    svm::{
        events::ExternalInterruptError,
        permission_maps::{IOPM_BYTES, MSRPM_BYTES},
        vmcb::{
            EventIntercept, GUEST_ASID, INTERCEPT_MISC1, INTERCEPT_MISC2, IOPM_BASE,
            InstructionIntercept, MSRPM_BASE, NESTED_CR3, TSC_OFFSET, Vmcb,
        },
    },
};

// TLB, ASID, permission-map and intercept configuration.
impl Vmcb {
    pub fn tsc_offset(&self) -> u64 {
        self.read_u64::<TSC_OFFSET>()
    }

    pub fn guest_asid(&self) -> u32 {
        self.read_u32::<GUEST_ASID>()
    }

    pub fn permission_maps(&self) -> (u64, u64) {
        (self.read_u64::<IOPM_BASE>(), self.read_u64::<MSRPM_BASE>())
    }

    pub fn nested_root(&self) -> u64 {
        self.read_u64::<NESTED_CR3>()
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

    pub fn event_intercept(&self, intercept: EventIntercept) -> bool {
        self.read_u32::<INTERCEPT_MISC1>() & intercept.mask() != 0
    }

    /// Bounded identity clock profile; APM vol.2 rev.3.44 Appendix B offset50h.
    /// Caller must separately own the optional global ratio and AUX MSRs.
    pub fn set_tsc_offset_zero(&mut self) {
        self.write_u64::<TSC_OFFSET>(0);
        self.invalidate_all();
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

    /// APM2 Appendix B, offset000h bit16: pre-execution CR0 write intercept.
    /// Used only while the native shared cache replay requires CD to stay set.
    pub fn set_cache_cr0_guard(&mut self, enabled: bool) {
        self.update_intercept::<0x000>(1 << 16, enabled);
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

    /// Set only the selected event intercept; preserve all other
    /// controls and invalidate cached VMCB fields. This is an inert byte edit,
    /// not activation or evidence that hardware will honor the SMI intercept.
    pub fn set_event_intercept(&mut self, intercept: EventIntercept, enabled: bool) {
        let mask = intercept.mask();
        self.update_intercept::<INTERCEPT_MISC1>(mask, enabled);
        self.invalidate_all();
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
        policy.validate(self.nested_root(), 4096, 4096).map_err(E::Address)?;
        self.write_u64::<0x090>(1);
        self.request_full_tlb_flush();
        Ok(())
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
}

/// Observe a filtered PAUSE without emulating it. APM2 rev3.44 15.14.4:
/// VMRUN reloads the internal count. Reentry at unchanged RIP executes the
/// interrupted PAUSE with a replenished nonzero budget; TF, RF, instruction
/// bytes and real/protected/long-mode behavior remain hardware-owned.
/// This is not a watchdog: a guest with no PAUSE and no other exits is invisible.
///
/// Caller owns the stopped native VMCB and its normal pending-event lifecycle.
/// This function changes no guest state. The configuration must have been
/// admitted on this CPU by `Vmcb::configure_native_pause_filter`.
pub fn native_pause_retry_ready(vmcb: &Vmcb) -> bool {
    let b = vmcb.bytes();
    vmcb.exit_snapshot().code == 0x77
        && u32::from_le_bytes(b[0x00c..0x010].try_into().unwrap()) & (1 << 23) != 0
        && u16::from_le_bytes(b[0x03c..0x03e].try_into().unwrap()) == 0
        && u16::from_le_bytes(b[0x03e..0x040].try_into().unwrap()) == 4096
}
