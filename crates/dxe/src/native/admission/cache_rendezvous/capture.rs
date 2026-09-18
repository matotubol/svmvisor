//! Live capture into the owned slots: the AP observer and the BSP capture-and-compare round.

#[cfg(any(target_os = "uefi", test))]
use core::sync::atomic::Ordering;

#[cfg(any(target_os = "uefi", test))]
use super::super::cache::CacheSnapshot;
use super::PreparedCacheRendezvous;
#[cfg(any(target_os = "uefi", test))]
use super::{
    ARITHMETIC_FLAGS, COMPLETE, CacheConsistencyReport, ConfigurationField, EMPTY, ENABLED,
    PagingRootError, RendezvousError, Slot, UNSUPPORTED_FLAGS, WRITING, compare_ap_configuration,
    svmvisor_native_cache_read, svmvisor_native_snapshot,
};
// The live capture adapters exist only for firmware and host-test builds.
#[cfg(any(target_os = "uefi", test))]
use super::super::cpu::{ApObservation, DispatchedAp, QuiescentBsp};

impl PreparedCacheRendezvous<'_> {
    /// Capture the actual BSP directly into its owned slot and compare every
    /// enabled AP's actual final-round observations. May be repeated with &mut
    /// self before/after the transition; AP records remain immutable.
    ///
    /// # Safety
    /// The guard must cover this BSP at HIGH, outside SMM, with IF/DF/TF/NT/AC
    /// clear. This object must have been selected as that round's final observer.
    /// The pool, code and stack must remain accessible/coherent throughout this
    /// service-free call, under the native cache reader's ordinary firmware
    /// contract. No fault containment is provided. Finish/release must run only
    /// after conforming blocking dispatch terminated every callback, at NOTIFY.
    #[cfg(any(target_os = "uefi", test))]
    pub unsafe fn capture_bsp_and_compare(
        &mut self,
        guard: &QuiescentBsp<'_>,
    ) -> Result<CacheConsistencyReport, RendezvousError> {
        self.cr4_mismatch_processor = None;
        let current = guard.report();
        if current.total_processors != self.report.total_processors
            || current.enabled_processors != self.report.enabled_processors
            || current.enabled_aps != self.report.enabled_aps
            || current.bsp_number != self.report.bsp_number
            || current.bsp_processor_id != self.report.bsp_processor_id
            || current.completed_ap_callbacks != self.report.enabled_aps
        {
            return Err(RendezvousError::Inventory);
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err(RendezvousError::ReusedOrInvalidCallback);
        }
        // Bind even a single-BSP machine to one scope. Repeated BSP captures
        // are allowed only inside that scope; no old object crosses a new one.
        bind_bsp_round(&mut self.bsp_rendezvous, guard.rendezvous(), current.bsp_number)?;
        let slot = self.slot(current.bsp_number)?;
        slot.state.store(WRITING, Ordering::Relaxed);
        // &mut self excludes all BSP snapshot references while it is replaced.
        unsafe {
            *slot.rendezvous.get() = guard.rendezvous();
            capture_slot(slot);
        }
        slot.state.store(COMPLETE, Ordering::Release);
        let bsp = self.completed_snapshot(current.bsp_number)?;
        let bsp_cr3 = self.completed_cr3(current.bsp_number)?;
        for number in 0..current.total_processors {
            let slot = self.slot(number)?;
            if slot.information.status_flag & ENABLED == 0 || number == current.bsp_number {
                continue;
            }
            let ap = self.completed_snapshot(number)?;
            // COMPLETE's acquire covers rendezvous and snapshot together.
            if unsafe { *slot.rendezvous.get() } != guard.rendezvous() {
                return Err(RendezvousError::Stale { processor: number });
            }
            if let Err(field) = compare_ap_configuration(bsp, ap) {
                if field == ConfigurationField::Cr4 {
                    self.cr4_mismatch_processor = Some(number);
                }
                return Err(RendezvousError::Mismatch { processor: number, field });
            }
            if self.completed_cr3(number)? != bsp_cr3 {
                return Err(RendezvousError::Mismatch {
                    processor: number,
                    field: ConfigurationField::Cr3,
                });
            }
        }
        Ok(CacheConsistencyReport {
            enabled_processors: current.enabled_processors,
            completed_ap_captures: current.enabled_aps,
            bsp_number: current.bsp_number,
            rendezvous: guard.rendezvous(),
        })
    }
}

// This is the legitimate AP adapter for the unchanged reader. It receives no
// mutable P or BSP guard and does not normalize flags. Unsupported IF and other
// incoming flags are recorded as reader refusal before any RDMSR.
#[cfg(any(target_os = "uefi", test))]
unsafe impl ApObservation for PreparedCacheRendezvous<'_> {
    unsafe fn observe(&self, ap: &DispatchedAp<'_>) {
        let Ok(slot) = self.slot(ap.number()) else {
            self.invalid.store(true, Ordering::Release);
            return;
        };
        if ap.number() == self.report.bsp_number
            || slot.information != ap.information()
            || slot.information.status_flag & ENABLED == 0
            || slot
                .state
                .compare_exchange(EMPTY, WRITING, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            self.invalid.store(true, Ordering::Release);
            return;
        }
        unsafe {
            *slot.rendezvous.get() = ap.rendezvous();
            capture_slot(slot);
        }
        slot.state.store(COMPLETE, Ordering::Release);
    }
}

/// The caller exclusively owns this WRITING slot and supplies a legitimate AP
/// callback or current HIGH BSP guard. COMPLETE is published only on return.
#[cfg(any(target_os = "uefi", test))]
unsafe fn capture_slot(slot: &Slot) {
    let status = unsafe { svmvisor_native_cache_read(slot.snapshot.get()) };
    unsafe {
        *slot.status.get() = status;
        // Never retain the previous BSP root after any refused recapture.
        *slot.paging_root.get() = if status == 0 {
            capture_paging_root(&*slot.snapshot.get())
        } else {
            Err(PagingRootError::NotCaptured)
        };
    }
}

/// Called only after the real cache reader's CPL/flags/target guards succeed.
/// The unchanged helper independently checks CPL before SGDT/SIDT/CR reads;
/// it does not read MSRs, dereference tables or write privileged state.
#[cfg(any(target_os = "uefi", test))]
unsafe fn capture_paging_root(cache: &CacheSnapshot) -> Result<u64, PagingRootError> {
    // Nine u64 words give the helper's exact 72-byte size and 8-byte alignment.
    // CR0/CR3/CR4/RFLAGS occupy byte offsets 40/48/56/64; CS starts at 32.
    let mut words = [0u64; 9];
    match unsafe { svmvisor_native_snapshot(words.as_mut_ptr().cast()) } {
        0 => {}
        1 => return Err(PagingRootError::PrivilegeLevel),
        _ => return Err(PagingRootError::UnexpectedStatus),
    }
    if words[4] & 3 != 0 {
        return Err(PagingRootError::PrivilegeLevel);
    }
    if words[5] != cache.cr0 {
        return Err(PagingRootError::InconsistentCr0);
    }
    if words[7] != cache.cr4 {
        return Err(PagingRootError::InconsistentCr4);
    }
    if words[8] & UNSUPPORTED_FLAGS != 0 {
        return Err(PagingRootError::UnsupportedFlags);
    }
    if words[8] & !ARITHMETIC_FLAGS != cache.rflags & !ARITHMETIC_FLAGS {
        return Err(PagingRootError::InconsistentFlags);
    }
    // Restricted four-level, 48-bit target with PCIDE clear: CR3[4:3] retain
    // the root fetch's PCD/PWT, not PCID bits. Do not normalize any control.
    if cache.physical_bits != 48
        || cache.cr0 & 0x8000_0001 != 0x8000_0001 // PG and PE.
        || cache.cr4 & (1 << 5) == 0 // PAE.
        || cache.cr4 & ((1 << 12) | (1 << 17)) != 0 // LA57 or PCIDE.
        || cache.efer & 0x500 != 0x500
    // LME and LMA.
    {
        return Err(PagingRootError::UnsupportedPagingMode);
    }
    const ROOT_ADDRESS: u64 = 0x0000_ffff_ffff_f000;
    let cr3 = words[6];
    if cr3 & !(ROOT_ADDRESS | 0x18) != 0 || cr3 & ROOT_ADDRESS == 0 {
        return Err(PagingRootError::InvalidCr3);
    }
    Ok(cr3)
}

#[cfg(any(target_os = "uefi", test))]
fn bind_bsp_round(
    bound: &mut usize,
    current: usize,
    processor: usize,
) -> Result<(), RendezvousError> {
    if current == 0 || (*bound != 0 && *bound != current) {
        return Err(RendezvousError::Stale { processor });
    }
    *bound = current;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bsp_binding_rejects_a_new_round_even_without_any_ap_slot() {
        let mut bound = 0;
        assert_eq!(bind_bsp_round(&mut bound, 0, 0), Err(RendezvousError::Stale { processor: 0 }));
        assert_eq!(bind_bsp_round(&mut bound, 7, 0), Ok(()));
        assert_eq!(bind_bsp_round(&mut bound, 7, 0), Ok(()));
        assert_eq!(bind_bsp_round(&mut bound, 8, 0), Err(RendezvousError::Stale { processor: 0 }));
        assert_eq!(bound, 7);
    }
}
