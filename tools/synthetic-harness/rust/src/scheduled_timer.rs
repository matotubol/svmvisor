//! Actual stopped-host TSC scheduling of guest-programmed APIC timers.
//!
//! One APIC source tick per 1024 sampled TSC ticks is explicit fixture
//! admission, not a measured physical frequency. APM2 3.44 sections 6.5,
//! 15.9, 15.21 and 16.11 govern HLT/intercepts/delivery/timer behavior.
//! PPR 57896 3.00 pp.43,55 concerns Family 1Ah Model 44h B0, whose 2xCLKIN
//! timer frequency is not inferred for this emulator. No running-guest
//! preemption or host APIC programming occurs in this bounded polling loop.
use crate::{clock, hex, interrupts::{enter, initialize}, memory, print, x2apic::admit, xstate};
use core::ptr;
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::{EvidenceFlag, ValidatedCapabilities}, registers::GuestRegisters},
    boot::ownership::OwnershipRecord,
    memory::npt::NptEvidence,
    svm::{
        apic_scheduler::{ClockRate, ScheduledApic, WaitOutcome},
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::FixtureApic,
    },
};

const MAX_POLL: usize = 200_000;
const SOURCE_DIVISOR: u32 = 1024;

/// # Safety
/// The caller owns the stopped single CPU and every fixture backing page,
/// retires all previous Prepared/mapping tokens and installed-code borrows,
/// and permits rebuilding the SAME owned pages. The immutable source is
/// disjoint from destination CODE. Fresh initialize invalidates prior ASIDs.
/// Host mapping, SVM, xstate and clock-restoration contracts remain admitted.
pub unsafe fn run(
    control: *mut Vmcb,
    ownership: Option<&OwnershipRecord<'_>>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_scheduled_start: u8;
        static guest_scheduled_end: u8;
        static guest_scheduled_handler: u8;
        static guest_scheduled_hlt: u8;
        static guest_scheduled_second_hlt: u8;
    }
    let source = ptr::addr_of!(guest_scheduled_start) as usize;
    let length = ptr::addr_of!(guest_scheduled_end) as usize - source;
    let code = unsafe { core::slice::from_raw_parts(source as *const u8, length) };
    let prepared = unsafe {
        memory::prepare(caps.address_policy(), code, ownership, NptEvidence {
            nx_supported: EvidenceFlag::Set,
            host_nxe: EvidenceFlag::Set,
            host_four_level: EvidenceFlag::Set,
        })
    };
    let address = |p: *const u8| 0x1000 + p as u64 - source as u64;
    let handler = address(ptr::addr_of!(guest_scheduled_handler));
    let first_hlt = address(ptr::addr_of!(guest_scheduled_hlt));
    let second_hlt = address(ptr::addr_of!(guest_scheduled_second_hlt));
    assert_eq!(clock.plan().tsc_offset(), 0);
    assert!(clock.plan().guest_ratio().is_none_or(|r| r == 1 << 32));
    let (mut samples, mut polls, mut entries, mut delivered, mut halted) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut min_late, mut max_late, mut before_deadline, mut sti_shadows) = (u64::MAX, 0, 0u64, 0u64);
    for bus in 0..2 {
        for case in 0..8 {
            for round in 0..if case < 2 { 16 } else { 1 } {
                unsafe { initialize(control, &prepared, caps, state, 0x1000, handler, true, round); admit(control); }
                let apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(),
                    if bus == 0 { 0xfee00900 } else { 0xfee00d00 }).unwrap();
                let initial = unsafe { clock.sample() };
                samples += 1;
                let mut scheduler = ScheduledApic::admit(apic, ClockRate::new(1, SOURCE_DIVISOR).unwrap(), initial)
                    .unwrap_or_else(|_| panic!("scheduler admission"));
                let mut frame = GuestRegisters { r10: case, r15: bus, ..GuestRegisters::default() };
                let (mut wakes, mut consumed, mut session_halts) = (0, 0, 0);
                let mut stopped = false;
                let mut bounded_stop = false;
                let mut next_deadline = None;
                for _ in 0..24 {
                    unsafe { enter(control, &mut frame, state, clock); }
                    entries += 1;
                    let v = unsafe { &mut *control };
                    let observed = unsafe { clock.sample() };
                    samples += 1;
                    if scheduler.apic().controller().delivery_armed() {
                        if scheduler.observe_after_exit(v, observed).unwrap().is_some() {
                            consumed += 1;
                            delivered += 1;
                        }
                    } else {
                        scheduler.service(v, observed).unwrap();
                    }
                    let snap = v.exit_snapshot();
                    match snap.code {
                        0x400 | 0x7c => {
                            let initial_write = if snap.code == 0x400 {
                                frame.rbx == memory::APIC_ALIAS + 0x380
                            } else { frame.rcx == 0x838 };
                            let bytes = unsafe { memory::installed_instruction(snap.rip, 2) };
                            if snap.code == 0x400 {
                                scheduler.handle_mmio(v, &frame, bytes, prepared.mmio_mapping.as_ref().unwrap()).unwrap();
                            } else {
                                scheduler.handle_msr(v, &mut frame, bytes).unwrap();
                            }
                            if initial_write { next_deadline = scheduler.deadline().unwrap(); }
                        }
                        0x78 => {
                            assert_eq!(snap.rip, if session_halts == 0 { first_hlt } else { second_hlt });
                            assert_eq!(frame.r13, session_halts);
                            // Keep the programmed expiration target even if
                            // service already expired the one-shot before HLT.
                            let deadline = next_deadline;
                            if deadline.is_some_and(|d| observed.ticks < d) { before_deadline += 1; }
                            if v.interrupt_shadow() { sti_shadows += 1; }
                            let bytes = unsafe { memory::installed_instruction(snap.rip, 1) };
                            scheduler.park_hlt(v, bytes).unwrap();
                            assert!(scheduler.is_halted());
                            assert_eq!(v.guest_rip(), snap.rip);
                            let parked_rip = v.guest_rip();
                            let saved_frame = frame;
                            let frozen = scheduler.apic().controller().timer_remaining();
                            session_halts += 1;
                            halted += 1;
                            let mut ready = false;
                            for poll in 0..MAX_POLL {
                                let sample = unsafe { clock.sample() };
                                samples += 1;
                                polls += 1;
                                match scheduler.poll_halted(v, sample).unwrap() {
                                    WaitOutcome::Ready { vector } => {
                                        assert!(case < 2);
                                        assert_eq!(vector, 0x50);
                                        assert!(!scheduler.is_halted());
                                        assert_eq!(v.guest_rip(), parked_rip + 1);
                                        let late = sample.ticks.checked_sub(deadline.unwrap()).unwrap();
                                        min_late = min_late.min(late);
                                        max_late = max_late.max(late);
                                        next_deadline = scheduler.deadline().unwrap();
                                        wakes += 1;
                                        ready = true;
                                        break;
                                    }
                                    WaitOutcome::Parked => {
                                        assert_eq!(v.guest_rip(), parked_rip);
                                        assert_eq!(frame, saved_frame);
                                        assert!(!scheduler.apic().controller().delivery_armed());
                                    }
                                }
                                if case >= 2 && poll >= 63 &&
                                    (case == 6 || scheduler.apic().controller().timer_remaining() == 0) {
                                    bounded_stop = true;
                                    break;
                                }
                            }
                            if case >= 2 {
                                assert!(bounded_stop && !ready);
                                assert_eq!((wakes, consumed, frame.r12, frame.r13), (0, 0, 0, 0));
                                assert!(scheduler.is_halted());
                                assert_eq!(scheduler.apic().controller().pending(0x50), case == 3 || case == 4);
                                if case == 6 { assert_eq!(scheduler.apic().controller().timer_remaining(), frozen); }
                                break;
                            }
                            assert!(ready, "bounded host clock wait exhausted");
                        }
                        0x81 => {
                            assert_eq!(v.guest_rax(), 1, "scheduled guest failed");
                            let bytes = unsafe { memory::installed_instruction(snap.rip, 3) };
                            assert_eq!(handle_exit_with_instruction(snap, v, &mut frame, bytes).unwrap(),
                                DispatchOutcome::Stop(StopReason::Requested));
                            assert!(initial.ticks <= frame.r8 && frame.r8 <= frame.r9 && frame.r9 <= observed.ticks);
                            stopped = true;
                            break;
                        }
                        _ => panic!("unexpected scheduled timer exit"),
                    }
                }
                if case < 2 {
                    let expected = case + 1;
                    assert!(stopped && !bounded_stop);
                    assert_eq!((wakes, consumed, session_halts, frame.r12, frame.r13),
                        (expected, expected, expected, expected, expected));
                    assert!(!scheduler.is_halted());
                    assert!(!scheduler.apic().controller().delivery_armed());
                    assert_eq!(scheduler.apic().controller().eoi_target(), None);
                    assert_eq!(scheduler.apic().controller().timer_remaining(), 0);
                    assert_eq!(unsafe { (&*control).guest_rsp() }, 0x9000);
                } else { assert!(bounded_stop && !stopped); }
                unsafe { memory::verify_exception_stack(); }
            }
        }
    }
    assert_eq!((delivered, halted), (96, 108));
    print("PASS apic-scheduled=64 clock-driven-hlt-eoi-iretq\n");
    print("PASS apic-scheduled-oneshot=32 completed-hlt-once\n");
    print("PASS apic-scheduled-periodic=32 two-hlt-wakes\n");
    print("PASS apic-scheduled-gates=12 masked-if0-priority-svr-base-cancel\n");
    print("PASS apic-scheduled-clock=64 guest-host-monotonic-identity\n");
    print("SCHEDULED-TIMER ratio-apic=1 ratio-tsc=1024 initial=4096\n");
    print("SCHEDULED-TIMER delivered=96 halted=108 bounded-stops=12\n");
    for (label, value) in [
        ("SCHEDULED-TIMER samples=", samples),
        ("SCHEDULED-TIMER poll-samples=", polls),
        ("SCHEDULED-TIMER entries=", entries),
        ("SCHEDULED-TIMER before-deadline=", before_deadline),
        ("SCHEDULED-TIMER sti-shadows=", sti_shadows),
        ("SCHEDULED-TIMER min-wake-lateness-tsc=", min_late),
        ("SCHEDULED-TIMER max-wake-lateness-tsc=", max_late),
    ] { print(label); hex(value); }
}
