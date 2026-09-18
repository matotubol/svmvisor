//! Stops: the stop record, the terminal barrier and export, and the host fault entry.

use core::{arch::asm, ptr};

use crate::{
    arch::x86_64::{
        apic,
        msr::{SYS_CFG, VM_CR, VM_CR_R_INIT},
    },
    host::resident::{
        runtime::{
            ASSIGNED_APIC_ID, EXIT_HISTORY, EXIT_HISTORY_COUNT, EXIT_HISTORY_NEXT, FRAME,
            RawVmexitCapture, State, VMCB,
            debug::{debug, diagnostic_record, hex},
            diagnostics, image_start,
            msr::read_msr,
            startup::{mailboxes, send_native_notification},
            svmvisor_resident_raw_vmexit,
        },
        terminal::{self, TerminalControl},
    },
};

impl RawVmexitCapture {
    pub(super) fn stop_record(
        self,
        expected_vmcb: u64,
        assigned_apic_id: u32,
        reason: u64,
        detail: u64,
    ) -> Option<(u8, [u64; 6])> {
        // Assembly captures GPR/VMCB provenance and physical APIC identity on
        // every MSR exit. Only the optional physical cache sample is SYS_CFG-
        // specific; other owned registers must not depend on that sample.
        let index = self.guest_rcx as u32;
        if self.code != 0x7c
            || !crate::svm::native_cache::owned_msr(index)
            || index == SYS_CFG && self.physical_cache_valid != 1
        {
            return None;
        }
        if self.entry_sequence == 0
            || self.entry_sequence != self.exit_sequence
            || self.entry_vmcb_pa != expected_vmcb
            || self.exit_vmcb_pa != expected_vmcb
            || self.context_vmcb_pa != expected_vmcb
            || self.physical_apic_id != u64::from(assigned_apic_id)
        {
            return Some((
                11,
                [
                    self.exit_vmcb_pa,
                    expected_vmcb,
                    (self.physical_apic_id << 32) | u64::from(assigned_apic_id),
                    self.exit_rip,
                    self.nrip,
                    self.entry_rip,
                ],
            ));
        }
        if index == SYS_CFG {
            Some((
                10,
                [
                    self.exit_rip,
                    self.nrip,
                    self.entry_rip,
                    self.guest_cr0,
                    self.mtrr_def_type,
                    self.host_cr0,
                ],
            ))
        } else {
            // EDX:EAX uses the low DWORD of each register. Preserve raw RCX
            // separately from its architectural low-DWORD MSR index. These
            // are boundary operands, not a claim the access was completed.
            let operand = (self.guest_rax as u32 as u64) | ((self.guest_rdx as u32 as u64) << 32);
            Some((13, [self.exit_rip, self.guest_rcx, operand, self.nrip, reason, detail]))
        }
    }
}

pub(super) unsafe fn terminal_requested(state: &State) -> bool {
    state.armed
        && terminal_enabled(state)
        && unsafe { terminal_control() }.ready(state.count)
        && unsafe { terminal_control() }.requested()
}

/// Terminal-only: no route guard is held, no resume follows. APM2 Table15-10
/// holds external SMI/NMI/INIT while GIF=0. All CPUs acknowledge only in that
/// state and never reopen GIF afterward, excluding their firmware/config writes.
/// Reset/machine-check or a nonparticipating CPU can lose evidence; no write is
/// permitted for the legacy aggregate without the complete ack mask. The live
/// guarded per-CPU transport exports failure evidence independently of that mask.
/// Iteration caps are not time bounds.
pub(super) unsafe fn terminal_finish(state: &mut State) {
    if !state.armed
        || !terminal_enabled(state)
        || state.terminal_endpoint.is_some_and(|endpoint| !endpoint.valid())
    {
        return;
    }
    unsafe {
        diagnostics::flush_fault();
    }
    let shared = unsafe { terminal_control() };
    if !shared.ready(state.count) {
        return;
    }
    let winner = state.stopped_valid && shared.claim(state.slot, state.count);
    if !shared.requested() {
        return;
    }
    if !shared.acknowledge(state.slot, state.count) {
        return;
    }
    unsafe {
        record_barrier(shared);
    }
    if !winner {
        return;
    }
    // The terminal request establishes sole spare-bank ownership; export before
    // notification/preflight can fail and without waiting for another CPU.
    unsafe {
        export_stop_context(state);
    }
    // The published ready gate follows every target's armed/guest ACK. The
    // dedicated terminal request is authoritative; no guest INIT is enqueued.
    let base = unsafe { read_msr(apic::APIC_BASE) };
    // x2APIC has no software-polled ICR delivery status to wait for.
    if base != state.host_apic_base
        || base & apic::APIC_BASE_X2APIC != apic::APIC_BASE_X2APIC
        || unsafe { read_msr(VM_CR) } & VM_CR_R_INIT == 0
        || unsafe { mailboxes(state.count) }.iter().any(|m| !m.is_ready())
    {
        shared.finish(2);
        unsafe {
            record_barrier(shared);
        }
        return;
    }
    // Same already admitted INIT-to-#SX wire operation as startup notification,
    // with a separate irreversible terminal publication instead of a queue.
    unsafe { send_native_notification() };
    for _ in 0..20_000_000 {
        if shared.all_acknowledged(state.count) {
            #[cfg(feature = "resident-runtime-test")]
            if state.terminal_endpoint.is_none() {
                debug(b"resident-terminal barrier=complete owner=");
                hex(state.slot as u64);
                debug(b" count=");
                hex(state.count as u64);
                debug(b" card=disabled\n");
                shared.finish(1);
                return;
            }
            let words = terminal::stop_words(
                state.slot,
                state.stopped,
                state.stopped_rip,
                state.stopped_info1,
                state.stopped_info2,
            );
            let result = words.is_some_and(|words| unsafe { diagnostics::export_terminal(words) });
            shared.finish(if result { 1 } else { 4 });
            unsafe {
                record_barrier(shared);
            }
            return;
        }
        core::hint::spin_loop();
    }
    shared.finish(3);
    unsafe {
        record_barrier(shared);
    }
    debug(b"resident-terminal barrier=incomplete\n");
}

/// All accesses use the existing pool-excluded shared alias, initialized before
/// any arm call. No guest reference or original DXE pointer survives here.
pub(super) unsafe fn terminal_control() -> &'static TerminalControl {
    unsafe {
        &*((ptr::addr_of!(image_start) as u64
            + super::STARTUP_PAGE_OFFSET
            + terminal::CONTROL_OFFSET) as *const TerminalControl)
    }
}

pub(super) fn terminal_enabled(state: &State) -> bool {
    state.startup_owned
        && state.count >= 2
        && (state.terminal_endpoint.is_some() || cfg!(feature = "resident-runtime-test"))
}

pub(super) fn stop(state: &mut State, code: u64, rip: u64, info1: u64, info2: u64) -> bool {
    // Retain the exact terminal reason even in production, where the diagnostic
    // port is absent. The dedicated stopped owner never resumes after this.
    unsafe {
        ptr::write_volatile(&mut state.stopped_rip, rip);
        ptr::write_volatile(&mut state.stopped_info1, info1);
        ptr::write_volatile(&mut state.stopped_info2, info2);
        ptr::write_volatile(&mut state.stopped, code);
        ptr::write_volatile(&mut state.stopped_valid, true);
        if code == 0x7c {
            let raw = ptr::read_volatile(ptr::addr_of!(svmvisor_resident_raw_vmexit));
            if let Some((event, context)) =
                raw.stop_record(ptr::addr_of!(VMCB) as u64, ASSIGNED_APIC_ID, info1, info2)
            {
                // Win the sticky first-fault bank with the immutable boundary;
                // keep event3 as the ordinary latest stopped-state record.
                diagnostic_record(event, true, context, info1 as u32);
            }
        }
        diagnostic_record(3, true, [rip, code, info1, info2, state.exits, stop_counters(state)], 0);
    }
    debug(b"resident-stop code=");
    hex(code);
    debug(b" rip=");
    hex(rip);
    debug(b" info1=");
    hex(info1);
    debug(b" info2=");
    hex(info2);
    debug(b"\n");
    false
}

/// Last context word of a stop record: incomplete-IPI drops in bits 31:0 and
/// software-disabled edge discards in bits 63:32, each saturated.
pub(super) fn stop_counters(state: &State) -> u64 {
    state.ipi_drops.min(u64::from(u32::MAX)) | (state.irq_discards.min(u64::from(u32::MAX)) << 32)
}

/// Terminal assembly has copied the normalized first host exception frame to
/// private retained storage and blocked recursive callback entry. No STATE
/// reference may be formed here: a host fault can interrupt a live mutable
/// runtime borrow. Export is a bounded best effort, then this CPU stays stopped.
#[unsafe(no_mangle)]
unsafe extern "C" fn svmvisor_resident_host_fault(frame: *const u64, cr2: u64, cr3: u64) -> ! {
    let vector = unsafe { ptr::read(frame) } as u32;
    let error = unsafe { ptr::read(frame.add(1)) };
    let rip = unsafe { ptr::read(frame.add(2)) };
    let flags = unsafe { ptr::read(frame.add(4)) };
    let rsp = unsafe { ptr::read(frame.add(5)) };
    unsafe {
        diagnostic_record(4, true, [rip, error, cr2, cr3, rsp, flags], vector);
        diagnostics::flush_fault();
    }
    unsafe {
        asm!("cli", "2: hlt", "jmp 2b", options(noreturn, nostack));
    }
}

unsafe fn record_barrier(shared: &TerminalControl) {
    let mut context = shared.diagnostic_snapshot();
    context[5] = diagnostics::fault_status();
    unsafe {
        diagnostic_record(7, false, context, 0);
    }
}

/// Stable raw stopped state; reading bytes does not change VMCB/GPRs. These are
/// software observations, not a claim that invalid-entry save fields are valid.
unsafe fn export_stop_context(state: &State) {
    let aux = (state.slot as u32) << 8 | (state.count as u32) << 24;
    unsafe {
        diagnostics::export_context(
            0,
            [
                state.stopped_rip,
                state.stopped,
                state.stopped_info1,
                state.stopped_info2,
                state.exits,
                diagnostics::fault_status(),
            ],
            aux,
        );
        let bytes = ptr::addr_of!(VMCB).cast::<u8>();
        let read = |offset: usize| ptr::read_volatile(bytes.add(offset).cast::<u64>());
        diagnostics::export_context(
            1,
            [read(0x78), read(0x80), read(0x88), read(0xa8), read(0xc8), read(0x550)],
            aux | 1,
        );
        let frame = ptr::read_volatile(ptr::addr_of!(FRAME));
        diagnostics::export_context(
            2,
            [read(0x5f8), frame.rcx, frame.rdx, read(0x558), read(0x4d0), read(0x410)],
            aux | 2 | ((ptr::read_volatile(bytes.add(0x4cb)) as u32) << 13),
        );
        let count = EXIT_HISTORY_COUNT;
        for n in 0..count {
            let index = (EXIT_HISTORY_NEXT + 5 - count + n) % 5;
            let context = ptr::addr_of!(EXIT_HISTORY).cast::<[u64; 6]>().add(index).read();
            diagnostics::export_context(3 + n, context, aux | 3 | ((n as u32) << 16));
        }
    }
}
