//! Classic virtual-interrupt TPR and control validation.

use crate::svm::{
    events::ExternalInterruptError,
    vmcb::{SUPPORTED_VIRTUAL_INTERRUPT_CONTROL, VIRTUAL_INTERRUPT_CONTROL, Vmcb},
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
