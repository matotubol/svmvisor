//! Classic virtual external-interrupt arming and post-exit accounting.

use crate::svm::{
    events::{ExternalInterruptError, ExternalInterruptState, PendingExternalInterrupt},
    vmcb::{
        EventIntercept, SUPPORTED_VIRTUAL_INTERRUPT_CONTROL, V_IRQ, VIRTUAL_INTERRUPT_CONTROL, Vmcb,
    },
};

// External interrupts.
impl Vmcb {
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
}
