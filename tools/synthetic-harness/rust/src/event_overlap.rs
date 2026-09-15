//! Real timer IRQ/fault overlap over the existing single APIC owner.
//! APM2 rev3.44 8.9,15.7.2,15.20,15.21.4: an interrupt gate clears IF;
//! V_IRQ remains hardware gated, while reflected faults retain faulting RIP.
//! Timer arrival is sampled while stopped, not physical preemption or latency.
use crate::{
    clock, hex,
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

/// # Safety
/// Exclusive stopped single CPU, VMCB, bridge and all backing pages, as in
/// scheduled_timer::run. Previous mapping/code tokens are retired; fresh setup
/// flushes the guest ASID. No borrowed mutable guest state survives an entry.
pub unsafe fn run(
    control: *mut Vmcb,
    ownership: Option<&OwnershipRecord<'_>>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_overlap_start: u8;
        static guest_overlap_end: u8;
        static guest_overlap_ud: u8;
        static guest_overlap_gp: u8;
        static guest_overlap_pf: u8;
        static guest_overlap_irq: u8;
    }
    let source = ptr::addr_of!(guest_overlap_start) as usize;
    let length = ptr::addr_of!(guest_overlap_end) as usize - source;
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
    let address = |p: *const u8| 0x1000 + p as u64 - source as u64;
    let (mut entries, mut deferred, mut delivered, mut faults, mut eois) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut min_span, mut max_span) = (u64::MAX, 0u64);
    for bus in 0..2 {
        for blocker in 0..2 {
            for fault in 0..3 {
                for round in 0..4 {
                    unsafe {
                        initialize(
                            control,
                            &prepared,
                            caps,
                            state,
                            0x1000,
                            address(ptr::addr_of!(guest_overlap_irq)),
                            true,
                            round,
                        );
                        admit(control);
                        memory::install_idt(&[
                            (6, address(ptr::addr_of!(guest_overlap_ud)), true),
                            (13, address(ptr::addr_of!(guest_overlap_gp)), true),
                            (14, address(ptr::addr_of!(guest_overlap_pf)), true),
                            (0x50, address(ptr::addr_of!(guest_overlap_irq)), true),
                        ]);
                    }
                    let apic = FixtureApic::admit_fixed_bsp(
                        LocalApic::admit_enabled(),
                        if bus == 0 { 0xfee00900 } else { 0xfee00d00 },
                    )
                    .unwrap();
                    let mut scheduler =
                        ScheduledApic::admit(apic, ClockRate::new(1, 1024).unwrap(), unsafe {
                            clock.sample()
                        })
                        .unwrap_or_else(|_| panic!("overlap admission"));
                    let mut frame = GuestRegisters {
                        r9: fault,
                        r10: blocker,
                        r15: bus,
                        ..GuestRegisters::default()
                    };
                    let (mut checkpoints, mut reflected, mut consumed, mut acknowledged) =
                        (0, 0, 0, 0);
                    let mut stopped = false;
                    for _ in 0..16 {
                        let before = unsafe { clock.sample() };
                        unsafe {
                            enter(control, &mut frame, state, clock);
                        }
                        let sample = unsafe { clock.sample() };
                        let span = sample.ticks.checked_sub(before.ticks).unwrap();
                        min_span = min_span.min(span);
                        max_span = max_span.max(span);
                        entries += 1;
                        let v = unsafe { &mut *control };
                        // Retire only after an actual exit with no interrupted
                        // IDT delivery; failed entry and nested events refuse.
                        v.clear_event_injection_after_exit().unwrap();
                        let outcome = scheduler.settle_after_exit(v, sample).unwrap();
                        if outcome.deferred.is_some() {
                            deferred += 1;
                        }
                        if outcome.consumed.is_some() {
                            assert_eq!(outcome.consumed, Some(0x50));
                            consumed += 1;
                            delivered += 1;
                        }
                        let snap = v.exit_snapshot();
                        if snap.code == 0x81 && frame.r11 == 3 {
                            assert_ne!(
                                u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap())
                                    & 0x200,
                                0
                            );
                        }
                        match snap.code {
                            0x400 | 0x7c => {
                                let eoi = if snap.code == 0x400 {
                                    frame.rbx == memory::APIC_ALIAS + 0xb0
                                } else {
                                    frame.rcx == 0x80b
                                };
                                if eoi {
                                    assert_eq!(
                                        (frame.r12, frame.r13, frame.r14, consumed),
                                        (1, 1, 1, 1)
                                    );
                                    assert!(scheduler.apic().controller().in_service(0x50));
                                }
                                let bytes = unsafe { memory::installed_instruction(snap.rip, 2) };
                                if snap.code == 0x400 {
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
                                if eoi {
                                    assert!(!scheduler.apic().controller().in_service(0x50));
                                    acknowledged += 1;
                                    eois += 1;
                                }
                            }
                            0x46 | 0x4d | 0x4e => {
                                assert_eq!(checkpoints, 1);
                                assert_eq!(
                                    (reflected, frame.r12, frame.r13, consumed),
                                    (0, 0, 0, 0)
                                );
                                assert_eq!(snap.code, [0x46, 0x4d, 0x4e][fault as usize]);
                                assert_eq!(
                                    outcome.deferred,
                                    if blocker == 0 { Some(0x50) } else { None }
                                );
                                assert!(scheduler.apic().controller().pending(0x50));
                                assert!(!scheduler.apic().controller().in_service(0x50));
                                assert_eq!(
                                    v.reflect_exception().unwrap().vector(),
                                    [6, 13, 14][fault as usize]
                                );
                                assert_eq!(v.guest_rip(), snap.rip);
                                reflected += 1;
                                faults += 1;
                                continue; // no competing V_IRQ beside EVENTINJ
                            }
                            0x81 => {
                                assert!(v.guest_rax() <= 1, "overlap guest failure");
                                if v.guest_rax() == 0 {
                                    assert_eq!(frame.r11, checkpoints);
                                    if checkpoints == 0 {
                                        let mut expired =
                                            scheduler.apic().controller().pending(0x50);
                                        for _ in 0..200_000 {
                                            if expired {
                                                break;
                                            }
                                            scheduler
                                                .service(v, unsafe { clock.sample() })
                                                .unwrap();
                                            expired = scheduler.apic().controller().pending(0x50);
                                        }
                                        assert!(expired, "bounded overlap timer wait exhausted");
                                    } else if checkpoints < 3 {
                                        assert_eq!((frame.r12, frame.r13, consumed), (0, 1, 0));
                                        assert!(scheduler.apic().controller().pending(0x50));
                                        if checkpoints == 1 {
                                            assert_eq!(
                                                u64::from_le_bytes(
                                                    v.bytes()[0x570..0x578].try_into().unwrap()
                                                ) & 0x200,
                                                0
                                            );
                                        } else {
                                            assert_eq!(
                                                u64::from_le_bytes(
                                                    v.bytes()[0x570..0x578].try_into().unwrap()
                                                ) & 0x200
                                                    != 0,
                                                blocker != 0
                                            );
                                        }
                                    } else {
                                        assert_eq!(
                                            (
                                                frame.r12,
                                                frame.r13,
                                                frame.r14,
                                                consumed,
                                                acknowledged
                                            ),
                                            (1, 1, 1, 1, 1)
                                        );
                                        assert!(!scheduler.apic().controller().pending(0x50));
                                    }
                                    checkpoints += 1;
                                } else {
                                    assert_eq!(
                                        (checkpoints, reflected, consumed, acknowledged),
                                        (4, 1, 1, 1)
                                    );
                                    assert_eq!((frame.r12, frame.r13, frame.r14), (1, 1, 1));
                                    assert_eq!(
                                        u64::from_le_bytes(
                                            v.bytes()[0x5d8..0x5e0].try_into().unwrap()
                                        ),
                                        0x9000
                                    );
                                    stopped = true;
                                }
                                let bytes = unsafe { memory::installed_instruction(snap.rip, 3) };
                                assert_eq!(
                                    handle_exit_with_instruction(snap, v, &mut frame, bytes)
                                        .unwrap(),
                                    if stopped {
                                        DispatchOutcome::Stop(StopReason::Requested)
                                    } else {
                                        DispatchOutcome::ResumePrepared
                                    }
                                );
                            }
                            _ => panic!("unexpected overlap exit"),
                        }
                        if stopped {
                            break;
                        }
                        scheduler.arm_pending(v).unwrap();
                    }
                    assert!(stopped, "bounded overlap entry budget exhausted");
                    assert!(!scheduler.apic().controller().delivery_armed());
                    assert!(!scheduler.apic().controller().pending(0x50));
                    assert!(!scheduler.apic().controller().in_service(0x50));
                    unsafe {
                        memory::verify_exception_stack();
                    }
                }
            }
        }
    }
    assert_eq!(
        (entries, deferred, faults, delivered, eois),
        (552, 72, 48, 48, 48)
    );
    print("PASS event-overlap=48 ud-gp-pf-if0-tpr-xapic-x2apic\n");
    print("PASS event-overlap-order=48 fault-handler-iretq-sti-shadow-irq-eoi-iretq-once\n");
    print("OVERLAP entries=");
    hex(entries);
    print("OVERLAP deferred=");
    hex(deferred);
    print("OVERLAP faults=");
    hex(faults);
    print("OVERLAP delivered=");
    hex(delivered);
    print("OVERLAP eois=");
    hex(eois);
    print("OVERLAP min-entry-exit-tsc=");
    hex(min_span);
    print("OVERLAP max-entry-exit-tsc=");
    hex(max_span);
    print("\n");
}
