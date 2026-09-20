//! Guest continuation state and committed instruction outcomes.

use crate::{
    arch::x86_64::descriptors::SegmentState,
    svm::{
        exit::ResumeCandidate,
        vmcb::{
            GUEST_CR0, GUEST_CR3, GUEST_CR4, GUEST_EFER, GUEST_ISST_ADDR, GUEST_RAX, GUEST_RFLAGS,
            GUEST_RIP, GUEST_RSP, GUEST_S_CET, VIRTUAL_INTERRUPT_CONTROL, Vmcb,
        },
    },
};

// Continuation and committed instruction state.
impl Vmcb {
    /// RAX is uninterpreted register data.
    pub fn set_guest_rax(&mut self, rax: u64) {
        self.write_u64::<GUEST_RAX>(rax);
        self.invalidate_all();
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
        for (start, end) in [(0x440, 0x460), (0x470, 0x480), (0x490, 0x4a0), (0x600, 0x640)] {
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

    /// Import the separately sampled native CET MSRs before the first entry.
    /// VMSAVE does not capture these fields (APM3 rev3.37 VMSAVE); their VMCB
    /// locations are APM2 rev3.44 Table B-2. CR4.CET must still be disabled at
    /// this initial boundary. This does not capture a dormant hardware SSP or
    /// authorize an already active shadow-stack continuation. U_CET, PLn_SSP
    /// and XSS remain live on the same physical CPU, unused by the monitor.
    /// The caller must have enumerated CET_SS before reading these MSRs.
    pub fn initialize_native_cet_msrs(
        &mut self,
        s_cet: u64,
        isst_addr: u64,
    ) -> Result<(), crate::guest::continuation::NativeContinuationError> {
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
}
