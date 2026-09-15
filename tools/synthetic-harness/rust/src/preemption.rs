//! Finite emulator LAPIC one-shot ownership and real running-guest INTR exits.
//! APM2 rev3.44 7.8.5, 15.13.1, 15.21.1, 16.3/16.4.1. The host timer count
//! is an emulator source setting; neither its rate nor TSC ratio is calibrated.
use crate::{
    clock, hex,
    host_lapic::HostTimer,
    interrupts::{enter, initialize},
    memory, print,
    x2apic::admit,
    xstate,
};
use core::ptr;
use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{EvidenceFlag, ValidatedCapabilities},
        registers::GuestRegisters,
    },
    boot::ownership::OwnershipRecord,
    memory::npt::NptEvidence,
    svm::{
        apic_scheduler::{ClockRate, ScheduledApic},
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::FixtureApic,
    },
};

const MAX_ENTRIES: usize = 256;
const NEGATIVE_EXITS: u64 = 64;
/// # Safety
/// Same single stopped emulator CPU/backing contract as scheduled_timer::run;
/// all previous Prepared tokens and installed-code borrows have been retired.
/// Requires empty host IRR/ISR. An inherited active timer must already be masked
/// and accompanied by the retained post-EBS ownership record; only that terminal
/// path may explicitly discard its phase before acquiring the idle baseline.
pub unsafe fn run(
    control: *mut Vmcb,
    ownership: Option<&OwnershipRecord<'_>>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_preempt_start: u8;
        static guest_preempt_end: u8;
        static guest_preempt_handler: u8;
        static guest_preempt_spin: u8;
        static guest_preempt_spin_end: u8;
    }
    let source = ptr::addr_of!(guest_preempt_start) as usize;
    let length = ptr::addr_of!(guest_preempt_end) as usize - source;
    let code = unsafe { core::slice::from_raw_parts(source as *const u8, length) };
    let prepared = unsafe {
        memory::prepare(
            caps.address_policy(),
            code,
            ownership,
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
    };
    let address = |symbol: *const u8| 0x1000 + symbol as u64 - source as u64;
    let handler = address(ptr::addr_of!(guest_preempt_handler));
    let spin = address(ptr::addr_of!(guest_preempt_spin));
    let spin_end = address(ptr::addr_of!(guest_preempt_spin_end));
    let mut timer = unsafe { HostTimer::acquire(ownership) };
    let (
        mut entries,
        mut intrs,
        mut spin_intrs,
        mut progress,
        mut delivered,
        mut armed,
        mut bounded,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut min_late, mut max_late, mut min_interval, mut max_interval) =
        (u64::MAX, 0, u64::MAX, 0);
    let mut voluntary_acks = 0u64;
    let mut no_progress_spin_exits = 0u64;
    for bus in 0..2 {
        for case in 0..8 {
            for round in 0..if case < 2 { 8 } else { 1 } {
                unsafe {
                    initialize(
                        control, &prepared, caps, state, 0x1000, handler, true, round,
                    );
                    admit(control);
                }
                unsafe {
                    (&mut *control)
                        .enable_physical_interrupt_virtualization()
                        .unwrap();
                }
                let apic = FixtureApic::admit_fixed_bsp(
                    LocalApic::admit_enabled(),
                    if bus == 0 { 0xfee00900 } else { 0xfee00d00 },
                )
                .unwrap();
                let initial = unsafe { clock.sample() };
                let mut scheduler =
                    ScheduledApic::admit(apic, ClockRate::new(1, 1024).unwrap(), initial)
                        .unwrap_or_else(|_| panic!("preemption clock admission"));
                let mut frame = GuestRegisters {
                    r10: case,
                    r15: bus,
                    ..GuestRegisters::default()
                };
                let (mut count, mut session_spin, mut session_delivered, mut last_progress) =
                    (0u64, 0u64, 0u64, 0u64);
                let mut deadline = None;
                let mut expected_return_rip = 0;
                let mut stopped = false;
                let mut bounded_stop = false;
                for _ in 0..MAX_ENTRIES {
                    let before_entry = unsafe { clock.sample() };
                    unsafe {
                        timer.arm();
                        enter(control, &mut frame, state, clock);
                    }
                    entries += 1;
                    let v = unsafe { &mut *control };
                    let snapshot = v.exit_snapshot();
                    let acknowledged = unsafe { timer.cancel_and_drain(snapshot.code == 0x60) };
                    if acknowledged && snapshot.code != 0x60 {
                        voluntary_acks += 1;
                    }
                    let sample = unsafe { clock.sample() };
                    if snapshot.code == 0x60 {
                        intrs += 1;
                        count += 1;
                        let interval = sample.ticks.checked_sub(before_entry.ticks).unwrap();
                        min_interval = min_interval.min(interval);
                        max_interval = max_interval.max(interval);
                        let rip = v.guest_rip();
                        let rflags =
                            u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap());
                        let saved = frame;
                        let outcome = scheduler.handle_preemption(v, sample).unwrap();
                        assert_eq!(v.guest_rip(), rip);
                        assert_eq!(
                            u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap()),
                            rflags
                        );
                        assert_eq!(frame, saved);
                        if outcome.consumed.is_some() {
                            delivered += 1;
                            session_delivered += 1;
                        }
                        if let Some(vector) = outcome.armed {
                            assert_eq!(vector, 0x50);
                            assert!(case < 2);
                            assert!(
                                (spin..spin_end).contains(&rip),
                                "virtual timer must interrupt running loop"
                            );
                            expected_return_rip = rip;
                            unsafe {
                                memory::set_preemption_return_rip(rip);
                            }
                            armed += 1;
                            let late = sample.ticks.checked_sub(deadline.unwrap()).unwrap();
                            min_late = min_late.min(late);
                            max_late = max_late.max(late);
                            deadline = scheduler.deadline().unwrap();
                        }
                        if (spin..spin_end).contains(&rip) {
                            assert!(
                                frame.r13 >= last_progress,
                                "loop progress counter moved backwards"
                            );
                            // The source can expire before entry or interrupt
                            // between two loop INCs. Preserve that real exit and
                            // its clock sample; do not label it as new progress.
                            if frame.r13 == last_progress {
                                no_progress_spin_exits += 1;
                            }
                            progress += frame.r13 - last_progress;
                            last_progress = frame.r13;
                            spin_intrs += 1;
                            session_spin += 1;
                        }
                        if case >= 2 && count >= NEGATIVE_EXITS {
                            assert!(session_spin > 0 && last_progress > 0);
                            assert_eq!((frame.r12, session_delivered), (0, 0));
                            assert!(!scheduler.apic().controller().delivery_armed());
                            assert_eq!(
                                scheduler.apic().controller().pending(0x50),
                                case == 2 || case == 3
                            );
                            if case == 7 {
                                assert!(scheduler.apic().controller().timer_remaining() > 0);
                            } else {
                                assert_eq!(scheduler.apic().controller().timer_remaining(), 0);
                            }
                            bounded += 1;
                            bounded_stop = true;
                            break;
                        }
                        continue;
                    }
                    if scheduler.apic().controller().delivery_armed() {
                        if scheduler.observe_after_exit(v, sample).unwrap().is_some() {
                            delivered += 1;
                            session_delivered += 1;
                        }
                    } else {
                        scheduler.service(v, sample).unwrap();
                    }
                    match snapshot.code {
                        0x7c | 0x400 => {
                            let initial_write = if snapshot.code == 0x400 {
                                frame.rbx == memory::APIC_ALIAS + 0x380
                            } else {
                                frame.rcx == 0x838
                            };
                            let bytes = unsafe { memory::installed_instruction(snapshot.rip, 2) };
                            if snapshot.code == 0x400 {
                                scheduler
                                    .handle_mmio(
                                        v,
                                        &frame,
                                        bytes,
                                        prepared.mmio_mapping.as_ref().unwrap(),
                                    )
                                    .unwrap();
                            } else {
                                scheduler.handle_msr(v, &mut frame, bytes).unwrap();
                            }
                            if initial_write {
                                deadline = scheduler.deadline().unwrap();
                            }
                        }
                        0x81 => {
                            assert_eq!(v.guest_rax(), 1, "preempted guest failed");
                            let bytes = unsafe { memory::installed_instruction(snapshot.rip, 3) };
                            assert_eq!(
                                handle_exit_with_instruction(snapshot, v, &mut frame, bytes)
                                    .unwrap(),
                                DispatchOutcome::Stop(StopReason::Requested)
                            );
                            stopped = true;
                            break;
                        }
                        _ => panic!("unexpected running-guest exit"),
                    }
                }
                if case < 2 {
                    assert!(stopped && !bounded_stop && session_spin > 0 && last_progress > 0);
                    assert_eq!((frame.r12, session_delivered), (case + 1, case + 1));
                    assert_eq!(scheduler.apic().controller().eoi_target(), None);
                    assert_eq!(scheduler.apic().controller().timer_remaining(), 0);
                    assert_eq!(unsafe { (&*control).guest_rsp() }, 0x9000);
                } else {
                    assert!(bounded_stop && !stopped);
                }
                assert!(frame.r13 > 0, "session did not execute the loop");
                unsafe {
                    memory::verify_preemption_stack(expected_return_rip);
                }
            }
        }
    }
    let acks = timer.acks;
    let rebased = timer.rebased;
    unsafe {
        timer.restore();
    }
    assert_eq!((delivered, armed, bounded), (48, 48, 12));
    assert_eq!(acks, intrs + voluntary_acks);
    print("PASS apic-preemption=32 running-loop-eoi-iretq\n");
    print("PASS apic-preemption-oneshot=16 completed-once\n");
    print("PASS apic-preemption-periodic=16 completed-twice\n");
    print("PASS apic-preemption-gates=12 if0-priority-mask-svr-cancel-base\n");
    print("PASS apic-preemption-restored timer-lvt-divide-tpr-svr-pic-idt-map-if\n");
    print(if rebased {
        "PASS apic-preemption-baseline post-ebs-rebased-phase-discarded-nonreturning\n"
    } else {
        "PASS apic-preemption-baseline idle-exact\n"
    });
    print("PREEMPTION source=lapic-oneshot vector=f0 count=100000 divide=1\n");
    print(
        "PREEMPTION ratio-apic=1 ratio-tsc=1024 initial=4096 max-entries=256 negative-exits=64\n",
    );
    print("PREEMPTION delivered=48 armed=48 bounded-stops=12\n");
    for (label, value) in [
        ("PREEMPTION entries=", entries),
        ("PREEMPTION intr-exits=", intrs),
        ("PREEMPTION spin-exits=", spin_intrs),
        ("PREEMPTION no-progress-spin-exits=", no_progress_spin_exits),
        ("PREEMPTION spin-progress=", progress),
        ("PREEMPTION host-acks=", acks),
        ("PREEMPTION voluntary-acks=", voluntary_acks),
        ("PREEMPTION min-lateness-tsc=", min_late),
        ("PREEMPTION max-lateness-tsc=", max_late),
        ("PREEMPTION min-entry-exit-tsc=", min_interval),
        ("PREEMPTION max-entry-exit-tsc=", max_interval),
    ] {
        print(label);
        hex(value);
    }
}
