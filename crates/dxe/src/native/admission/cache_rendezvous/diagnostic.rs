//! Stable diagnostic encoding of a rendezvous error for the caller's refusal word.

use core::sync::atomic::Ordering;

use super::super::{
    cache::CaptureError,
    cpu::{ENABLED, MAX_PROCESSORS},
};
use super::{
    AP_CR4_DIFFERENCE, COMPLETE, ConfigurationField, PagingRootError, PreparedCacheRendezvous,
    RendezvousError, UNSUPPORTED_FLAGS, compare_ap_configuration,
};

impl PreparedCacheRendezvous<'_> {
    /// Stable diagnostic bits for the parent's existing refusal word. Bits
    /// 31:24 hold a processor number for processor-specific errors; bits 23:16
    /// hold the explicit detail code. The low 16 bits remain the caller's stage.
    /// Global errors use processor byte zero, which is not a BSP observation.
    ///
    /// PrivilegeOrFlags consults only an acquired COMPLETE slot. Its
    /// 0x80..=0x9f detail records observed TF/IF/DF/NT/AC in bits 0..4;
    /// zero observed flags does not identify CPL or the unclassified reason.
    /// An associated current-round AP CR4 mismatch may use 0xc0..=0xff for
    /// exactly one remaining rejected bit 0..63 after excluding AP CR4.DE.
    /// This records a bit index, not the full XOR or a normalized register.
    /// This does not recapture, normalize, or admit any observation.
    /// The caller must supply this object's actual error from the current round,
    /// before another capture. An encoded error alone authenticates no evidence.
    pub fn diagnostic_bits(&self, error: RendezvousError) -> u64 {
        let processor = diagnostic_processor(&error);
        // Exactly one disjoint category supplies a nonzero detail code.
        let mut detail = diagnostic_global(&error)
            | diagnostic_capture(&error)
            | diagnostic_record(&error)
            | diagnostic_paging(&error)
            | diagnostic_mismatch(&error);
        let Some(processor) = processor else {
            return u64::from(detail) << 16;
        };
        const _: () = assert!(MAX_PROCESSORS == 256);
        if processor >= MAX_PROCESSORS {
            // Do not truncate an unrepresentable processor to a different CPU.
            return 0x007f_0000;
        }
        if detail == 0x52 {
            detail = self.diagnostic_cr4_mismatch(processor);
        }
        if detail == 0x11
            && let Ok(slot) = self.slot(processor)
            && slot.state.load(Ordering::Acquire) == COMPLETE
        {
            // COMPLETE publishes the immutable AP record. &self also excludes
            // BSP replacement and release while status and flags are read.
            let status = unsafe { *slot.status.get() };
            let flags = unsafe { (*slot.snapshot.get()).rflags };
            if status == 2 || (status == 0 && flags & UNSUPPORTED_FLAGS != 0) {
                detail = 0x80
                    | ((flags >> 8) & 1) as u8
                    | (((flags >> 9) & 1) as u8) << 1
                    | (((flags >> 10) & 1) as u8) << 2
                    | (((flags >> 14) & 1) as u8) << 3
                    | (((flags >> 18) & 1) as u8) << 4;
            }
        }
        ((processor as u64) << 24) | (u64::from(detail) << 16)
    }

    #[inline(never)]
    fn diagnostic_cr4_mismatch(&self, processor: usize) -> u8 {
        if self.cr4_mismatch_processor != Some(processor)
            || processor == self.report.bsp_number
            || self.bsp_rendezvous == 0
        {
            return 0x52;
        }
        let Ok(bsp_slot) = self.slot(self.report.bsp_number) else {
            return 0x52;
        };
        let Ok(ap_slot) = self.slot(processor) else {
            return 0x52;
        };
        // COMPLETE is acquired before any published record or round is read.
        let Ok(bsp) = self.completed_snapshot(self.report.bsp_number) else {
            return 0x52;
        };
        let Ok(ap) = self.completed_snapshot(processor) else {
            return 0x52;
        };
        if ap_slot.information.status_flag & ENABLED == 0
            || unsafe { *bsp_slot.rendezvous.get() } != self.bsp_rendezvous
            || unsafe { *ap_slot.rendezvous.get() } != self.bsp_rendezvous
            || compare_ap_configuration(bsp, ap) != Err(ConfigurationField::Cr4)
        {
            return 0x52;
        }
        let rejected = (bsp.cr4 ^ ap.cr4) & !AP_CR4_DIFFERENCE;
        if rejected.is_power_of_two() { 0xc0 | rejected.trailing_zeros() as u8 } else { 0x52 }
    }
}

// Keep these typed scalar encoders separate from processor selection and flag
// enrichment. The combined match lowered to computed jumps in the HIGH scope;
// the mandatory linked audit requires direct control flow. No enum layout or
// numeric Rust discriminant is part of the diagnostic wire contract.
#[inline(never)]
fn diagnostic_global(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::Cpu(_) => 0x01,
        RendezvousError::Allocation(_) => 0x02,
        RendezvousError::Layout => 0x03,
        RendezvousError::Released => 0x04,
        RendezvousError::Bounds => 0x05,
        RendezvousError::Inventory => 0x06,
        RendezvousError::ReusedOrInvalidCallback => 0x07,
        RendezvousError::Cleanup(_) => 0x08,
        RendezvousError::Capture { .. }
        | RendezvousError::CaptureShape { .. }
        | RendezvousError::Incomplete { .. }
        | RendezvousError::Stale { .. }
        | RendezvousError::PagingRoot { .. }
        | RendezvousError::Mismatch { .. } => 0,
    }
}

#[inline(never)]
fn diagnostic_capture(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::Capture { error, .. } => match error {
            CaptureError::OutputAddress => 0x10,
            CaptureError::PrivilegeOrFlags => 0x11,
            CaptureError::UnsupportedCpu => 0x12,
            CaptureError::UnsupportedFeatures => 0x13,
            CaptureError::AddressEncryptionActive => 0x14,
            CaptureError::UnsupportedMtrrCount => 0x15,
            CaptureError::UnexpectedStatus => 0x16,
        },
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_record(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::CaptureShape { .. } => 0x20,
        RendezvousError::Incomplete { .. } => 0x21,
        RendezvousError::Stale { .. } => 0x22,
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_paging(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::PagingRoot { error, .. } => match error {
            PagingRootError::NotCaptured => 0x30,
            PagingRootError::PrivilegeLevel => 0x31,
            PagingRootError::UnexpectedStatus => 0x32,
            PagingRootError::InconsistentCr0 => 0x33,
            PagingRootError::InconsistentCr4 => 0x34,
            PagingRootError::UnsupportedFlags => 0x35,
            PagingRootError::InconsistentFlags => 0x36,
            PagingRootError::UnsupportedPagingMode => 0x37,
            PagingRootError::InvalidCr3 => 0x38,
        },
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_mismatch(error: &RendezvousError) -> u8 {
    match error {
        RendezvousError::Mismatch { field, .. } => match field {
            ConfigurationField::AbiVersion => 0x40,
            ConfigurationField::CapturedFields => 0x41,
            ConfigurationField::Refusal => 0x42,
            ConfigurationField::MsrReads => 0x43,
            ConfigurationField::Signature => 0x44,
            ConfigurationField::MaximumBasicLeaf => 0x45,
            ConfigurationField::MaximumExtendedLeaf => 0x46,
            ConfigurationField::Leaf1Ecx => 0x47,
            ConfigurationField::Leaf1Edx => 0x48,
            ConfigurationField::PhysicalBits => 0x49,
            ConfigurationField::EncryptionEax => 0x4a,
            ConfigurationField::EncryptionEbx => 0x4b,
            ConfigurationField::MultiKeyEax => 0x4c,
            ConfigurationField::MultiKeyEbx => 0x4d,
            ConfigurationField::Reserved => 0x4e,
            ConfigurationField::Rflags => 0x4f,
            ConfigurationField::Cr0 => 0x50,
            ConfigurationField::Cr3 => 0x51,
            ConfigurationField::Cr4 => 0x52,
            ConfigurationField::Efer => 0x53,
            ConfigurationField::SysCfg => 0x54,
            ConfigurationField::SevStatus => 0x55,
            ConfigurationField::Pat => 0x56,
            ConfigurationField::MtrrCap => 0x57,
            ConfigurationField::MtrrDefault => 0x58,
            ConfigurationField::TopMem => 0x59,
            ConfigurationField::SmmAddress => 0x5a,
            ConfigurationField::SmmMask => 0x5b,
            ConfigurationField::ApicBase => 0x5c,
            ConfigurationField::MmioConfig => 0x5d,
            ConfigurationField::Iorr => 0x5e,
            ConfigurationField::VariableMtrrs => 0x5f,
        },
        _ => 0,
    }
}

#[inline(never)]
fn diagnostic_processor(error: &RendezvousError) -> Option<usize> {
    match error {
        RendezvousError::Capture { processor, .. }
        | RendezvousError::CaptureShape { processor }
        | RendezvousError::Incomplete { processor }
        | RendezvousError::Stale { processor }
        | RendezvousError::PagingRoot { processor, .. }
        | RendezvousError::Mismatch { processor, .. } => Some(*processor),
        _ => None,
    }
}
