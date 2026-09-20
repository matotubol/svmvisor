//! EVENTINJ clearing after an exit and interrupted-delivery reinjection.

use crate::svm::{events::ReflectionError, vmcb::Vmcb};

// Exception reflection and reinjection.
impl Vmcb {
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

    /// Complete an interrupted event delivery by re-injecting EXITINTINFO
    /// through EVENTINJ before the next VMRUN.
    ///
    /// APM2 rev3.44 15.7.2 p509-510, 15.7.3 p510-511 and 15.20 p531: when an
    /// intercept fires while the guest is delivering an event through the IDT,
    /// EXITINTINFO (offset 088h) records that event and the VMM completes
    /// delivery by re-injecting it through EVENTINJ (offset 0A8h), whose
    /// encoding matches EXITINTINFO. This runtime intercepts no exceptions
    /// (`configure_native_boot_intercepts` leaves offset 008h zero), so an
    /// intercept never fires *because of* the delivered event and EXITINTINFO
    /// records a single event: it is re-injected verbatim, with no x86
    /// combining or #DF logic (unlike `resolve_exception_delivery_after_exit`,
    /// which the returning path uses for intercepted-exception delivery).
    ///
    /// Only TYPE 0 (external/virtual interrupt), 2 (NMI) and 3 (exception) are
    /// re-injected. EXITINTINFO records INT3/INTO as TYPE 3 (15.7.2 p510), and
    /// 15.20 p531 makes an injected TYPE 3 with vector 3 or 4 behave as the
    /// corresponding trap, so those complete correctly. TYPE 4 (INTn software
    /// interrupt) needs the nRIP-based injection emulation of 15.20 p531-532
    /// that this bounded path does not implement, and reserved types are
    /// undefined; both are refused as `Unsupported`. A re-injected NMI sets
    /// V_NMI_MASK on the next VMRUN under NMI virtualization (15.21.10 p537).
    /// RIP, RSP, RFLAGS, RAX and EXITINTINFO are retained (they are uncached,
    /// 15.15.3 p528); success writes only EVENTINJ and the clean bits.
    pub fn reinject_interrupted_delivery(&mut self) -> ReinjectOutcome {
        let interrupted = self.read_u64::<0x088>();
        if interrupted & (1 << 31) == 0 {
            return ReinjectOutcome::NoEvent;
        }
        // Shutdown/invalid entry cannot describe a recoverable delivery
        // (15.14.3 p528); the caller already stopped on those, so this is a
        // defensive guard.
        let code = self.read_u64::<0x070>();
        if code == 0x7f || code == u64::MAX {
            return ReinjectOutcome::Conflict;
        }
        let kind = ((interrupted >> 8) & 7) as u8;
        if !matches!(kind, 0 | 2 | 3) {
            return ReinjectOutcome::Unsupported { interrupted };
        }
        // Rebuild a clean EVENTINJ. EV=0 leaves the error code (63:32) and the
        // reserved bits (30:12) undefined (15.7.2 p510), so copy them only when
        // EV=1.
        let mut event = (1u64 << 31) | (u64::from(kind) << 8) | (interrupted & 0xff);
        if interrupted & (1 << 11) != 0 {
            event |= (1 << 11) | (interrupted & (0xffff_ffffu64 << 32));
        }
        // Never overwrite a different event. #VMEXIT clears EVENTINJ (APM3
        // rev3.37 VMRUN p505), so one found here was queued after this exit.
        let pending = self.event_injection();
        if pending & (1 << 31) != 0 && pending != event {
            return ReinjectOutcome::Conflict;
        }
        self.write_u64::<0x0a8>(event);
        self.invalidate_all();
        ReinjectOutcome::Reinjected { kind, vector: interrupted as u8 }
    }
}

/// Outcome of `Vmcb::reinject_interrupted_delivery`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReinjectOutcome {
    /// EXITINTINFO.V=0: no interrupted delivery to complete.
    NoEvent,
    /// EXITINTINFO copied to EVENTINJ; the next VMRUN re-delivers `vector`
    /// with the recorded TYPE (`kind`).
    Reinjected { kind: u8, vector: u8 },
    /// EXITINTINFO records a TYPE this bounded path cannot re-inject (TYPE 4
    /// software interrupt or a reserved TYPE). Terminal.
    Unsupported { interrupted: u64 },
    /// EVENTINJ already holds a different pending event, or the exit is
    /// shutdown/invalid entry. Terminal.
    Conflict,
}
