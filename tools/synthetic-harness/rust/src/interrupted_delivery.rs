//! Bounded real nested frames; reuses the sole APIC and entry/exit owners.
//! APM2 rev3.44 8.2.9,8.9,15.7.2,15.20: handler execution and interrupted
//! IDT delivery are separate cases. No physical timing or OS claim.
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
/// Exclusive stopped VMCB, single CPU, existing bridge/backing and clock owner.
/// Prior code/mapping tokens retired. initialize flushes ASID before each entry.
pub unsafe fn run(
    control: *mut Vmcb,
    ownership: Option<&OwnershipRecord<'_>>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_irq_fault_start: u8;
        static guest_irq_fault_end: u8;
        static guest_irq_fault_ud: u8;
        static guest_irq_fault_gp: u8;
        static guest_irq_fault_pf: u8;
        static guest_irq_fault_handler: u8;
    }
    let source = ptr::addr_of!(guest_irq_fault_start) as usize;
    let code = unsafe {
        core::slice::from_raw_parts(
            source as *const u8,
            ptr::addr_of!(guest_irq_fault_end) as usize - source,
        )
    };
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
    let (mut entries, mut faults, mut delivered, mut eois) = (0u64, 0u64, 0u64, 0u64);
    let (mut min_span, mut max_span) = (u64::MAX, 0);
    for bus in 0..2 {
        for fault in 0..3 {
            for round in 0..4 {
                unsafe {
                    initialize(
                        control,
                        &prepared,
                        caps,
                        state,
                        0x1000,
                        address(ptr::addr_of!(guest_irq_fault_handler)),
                        true,
                        round,
                    );
                    admit(control);
                    memory::install_idt(&[
                        (6, address(ptr::addr_of!(guest_irq_fault_ud)), true),
                        (13, address(ptr::addr_of!(guest_irq_fault_gp)), true),
                        (14, address(ptr::addr_of!(guest_irq_fault_pf)), true),
                        (0x50, address(ptr::addr_of!(guest_irq_fault_handler)), true),
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
                    .unwrap();
                let mut frame = GuestRegisters {
                    r9: fault,
                    r15: bus,
                    ..GuestRegisters::default()
                };
                let (mut checkpoints, mut reflected, mut consumed, mut acknowledged) = (0, 0, 0, 0);
                let mut stopped = false;
                for _ in 0..24 {
                    let before = unsafe { clock.sample() };
                    unsafe { enter(control, &mut frame, state, clock) };
                    let sample = unsafe { clock.sample() };
                    let span = sample.ticks.checked_sub(before.ticks).unwrap();
                    min_span = min_span.min(span);
                    max_span = max_span.max(span);
                    entries += 1;
                    let v = unsafe { &mut *control };
                    v.clear_event_injection_after_exit().unwrap();
                    let outcome = scheduler.settle_after_exit(v, sample).unwrap();
                    if let Some(vector) = outcome.consumed {
                        assert_eq!(vector, 0x50);
                        consumed += 1;
                        delivered += 1;
                    }
                    let snap = v.exit_snapshot();
                    match snap.code {
                        0x400 | 0x7c => {
                            let eoi = if snap.code == 0x400 {
                                frame.rbx == memory::APIC_ALIAS + 0xb0
                            } else {
                                frame.rcx == 0x80b
                            };
                            if eoi {
                                assert_eq!((reflected, frame.r13, frame.r14), (1, 1, 1));
                                assert_eq!(frame.r12, consumed);
                                assert_eq!(consumed, acknowledged + 1);
                                assert!(scheduler.apic().controller().in_service(0x50));
                                assert_eq!(
                                    scheduler.apic().controller().pending(0x50),
                                    consumed == 1
                                );
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
                            assert_eq!(
                                (
                                    checkpoints,
                                    reflected,
                                    consumed,
                                    acknowledged,
                                    frame.r12,
                                    frame.r13
                                ),
                                (2, 0, 1, 0, 1, 0)
                            );
                            assert_eq!(snap.code, [0x46, 0x4d, 0x4e][fault as usize]);
                            assert!(scheduler.apic().controller().pending(0x50));
                            assert!(scheduler.apic().controller().in_service(0x50));
                            assert_eq!(
                                v.reflect_exception().unwrap().vector(),
                                [6, 13, 14][fault as usize]
                            );
                            assert_eq!(v.guest_rip(), snap.rip);
                            reflected += 1;
                            faults += 1;
                            continue;
                        }
                        0x81 => {
                            assert!(v.guest_rax() <= 1, "IRQ fault guest failure");
                            if v.guest_rax() == 0 {
                                assert_eq!(frame.r11, checkpoints);
                                if checkpoints <= 1 {
                                    for _ in 0..200_000 {
                                        if scheduler.apic().controller().pending(0x50) {
                                            break;
                                        }
                                        scheduler.service(v, unsafe { clock.sample() }).unwrap();
                                    }
                                    assert!(
                                        scheduler.apic().controller().pending(0x50),
                                        "bounded second timer wait"
                                    );
                                    assert_eq!((consumed, frame.r12), (checkpoints, checkpoints));
                                }
                                if (1..=3).contains(&checkpoints) {
                                    assert_eq!(
                                        u64::from_le_bytes(
                                            v.bytes()[0x570..0x578].try_into().unwrap()
                                        ) & 0x200,
                                        0
                                    );
                                    assert_eq!((consumed, acknowledged, frame.r12), (1, 0, 1));
                                    assert!(scheduler.apic().controller().pending(0x50));
                                    assert!(scheduler.apic().controller().in_service(0x50));
                                    if checkpoints >= 2 {
                                        assert_eq!((reflected, frame.r13), (1, 1));
                                    }
                                }
                                if checkpoints == 4 {
                                    assert_eq!(
                                        (consumed, acknowledged, frame.r12, frame.r13),
                                        (2, 2, 2, 1)
                                    );
                                }
                                checkpoints += 1;
                            } else {
                                assert_eq!(
                                    (
                                        checkpoints,
                                        reflected,
                                        consumed,
                                        acknowledged,
                                        frame.r12,
                                        frame.r13,
                                        frame.r14
                                    ),
                                    (5, 1, 2, 2, 2, 1, 1)
                                );
                                assert_eq!(
                                    u64::from_le_bytes(v.bytes()[0x5d8..0x5e0].try_into().unwrap()),
                                    0x9000
                                );
                                assert_ne!(
                                    u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap())
                                        & 0x200,
                                    0
                                );
                                stopped = true;
                            }
                            let bytes = unsafe { memory::installed_instruction(snap.rip, 3) };
                            assert_eq!(
                                handle_exit_with_instruction(snap, v, &mut frame, bytes).unwrap(),
                                if stopped {
                                    DispatchOutcome::Stop(StopReason::Requested)
                                } else {
                                    DispatchOutcome::ResumePrepared
                                }
                            );
                        }
                        _ => panic!("unexpected IRQ handler fault exit"),
                    }
                    if stopped {
                        break;
                    }
                    scheduler.arm_pending(v).unwrap();
                }
                assert!(stopped, "IRQ handler entry bound");
                assert!(!scheduler.apic().controller().delivery_armed());
                assert!(!scheduler.apic().controller().pending(0x50));
                assert!(!scheduler.apic().controller().in_service(0x50));
                unsafe { memory::verify_nested_exception_stack() };
            }
        }
    }
    assert_eq!((faults, delivered, eois), (24, 48, 48));
    print("PASS irq-handler-fault=24 ud-gp-pf-second-timer-xapic-x2apic\n");
    print("PASS irq-handler-order=24 nested-frame-fault-iretq-two-eoi-two-iretq-once\n");
    for (name, value) in [
        ("entries", entries),
        ("faults", faults),
        ("delivered", delivered),
        ("eois", eois),
        ("min-entry-exit-tsc", min_span),
        ("max-entry-exit-tsc", max_span),
    ] {
        print("IRQFAULT ");
        print(name);
        print("=");
        hex(value);
    }
    drop(prepared);
    unsafe { run_delivery(control, ownership, caps, state, clock) };
}

/// Deliberately non-present gates cause actual secondary delivery faults.
/// #DF is terminal: its saved RIP is architecturally undefined (APM2 8.2.9).
unsafe fn run_delivery(
    control: *mut Vmcb,
    ownership: Option<&OwnershipRecord<'_>>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    use svmvisor_hypervisor::svm::{
        events::{DeliveryOutcome, GuestShutdown},
        vmcb::EventIntercept,
    };
    unsafe extern "C" {
        static guest_delivery_start: u8;
        static guest_delivery_end: u8;
        static guest_delivery_np: u8;
        static guest_delivery_pf: u8;
        static guest_delivery_df: u8;
    }
    let source = ptr::addr_of!(guest_delivery_start) as usize;
    let code = unsafe {
        core::slice::from_raw_parts(
            source as *const u8,
            ptr::addr_of!(guest_delivery_end) as usize - source,
        )
    };
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
    let (
        mut entries,
        mut secondary,
        mut np_returns,
        mut pf_returns,
        mut df_terminals,
        mut synthetic_shutdowns,
        mut direct_shutdowns,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    for mode in 0..7 {
        for round in 0..4 {
            unsafe {
                initialize(
                    control,
                    &prepared,
                    caps,
                    state,
                    0x1000,
                    address(ptr::addr_of!(guest_delivery_np)),
                    true,
                    round,
                );
                memory::install_idt(&[
                    (6, address(ptr::addr_of!(guest_delivery_np)), false),
                    (13, address(ptr::addr_of!(guest_delivery_np)), false),
                    (14, address(ptr::addr_of!(guest_delivery_np)), false),
                    (11, address(ptr::addr_of!(guest_delivery_np)), mode != 4),
                    (8, address(ptr::addr_of!(guest_delivery_df)), mode < 3),
                ]);
                (*control).set_event_intercept(EventIntercept::Shutdown, true);
                if mode == 4 {
                    // Let the CPU combine a guest-raised GP with a failed NP
                    // gate and failed DF gate. No EVENTINJ is manufactured.
                    crate::field(
                        control,
                        0x008,
                        (u32::MAX & !((1 << 8) | (1 << 11) | (1 << 13))).to_le_bytes(),
                    );
                }
                if mode == 5 {
                    // GP gate at unmapped2ff0h; PF gate at owned IDT[0]/3000h.
                    memory::install_idt(&[(0, address(ptr::addr_of!(guest_delivery_pf)), true)]);
                    crate::field(control, 0x488, 0x2f20u64.to_le_bytes());
                }
                if mode == 6 {
                    // PF gate at unmapped b000h; DF at afa0h in owned IST RAM.
                    memory::install_idt(&[(0, address(ptr::addr_of!(guest_delivery_df)), true)]);
                    memory::install_boundary_df_gate();
                    crate::field(control, 0x488, 0xaf20u64.to_le_bytes());
                }
            }
            let mut frame = GuestRegisters {
                r9: mode,
                ..GuestRegisters::default()
            };
            let mut stage = 0;
            let mut terminal = false;
            for _ in 0..6 {
                unsafe { enter(control, &mut frame, state, clock) };
                entries += 1;
                let v = unsafe { &mut *control };
                let snap = v.exit_snapshot();
                // Shutdown saved guest state is undefined: classify before
                // injection retirement, APIC settlement, frame or RIP reads.
                if snap.code == 0x7f {
                    assert_eq!(mode, 4);
                    assert_eq!(stage, 0);
                    let before = *v.bytes();
                    assert_eq!(
                        v.resolve_exception_delivery_after_exit().unwrap(),
                        DeliveryOutcome::Shutdown(GuestShutdown::Intercepted)
                    );
                    assert_eq!(v.bytes(), &before);
                    direct_shutdowns += 1;
                    terminal = true;
                    break;
                }
                if stage == 0 {
                    assert_ne!(mode, 4, "direct CPU shutdown did not occur");
                    assert_eq!(
                        snap.code,
                        if mode == 0 {
                            0x46
                        } else if mode == 2 || mode == 6 {
                            0x4e
                        } else {
                            0x4d
                        }
                    );
                    v.clear_event_injection_after_exit().unwrap();
                    v.reflect_exception().unwrap();
                    stage = 1;
                    continue;
                }
                let prior = u64::from_le_bytes(v.bytes()[0x088..0x090].try_into().unwrap());
                if prior & (1 << 31) != 0 {
                    assert_eq!(snap.code, if mode >= 5 { 0x4e } else { 0x4b });
                    assert_eq!(
                        prior & 0x800007ff,
                        0x80000300
                            | if stage == 2 {
                                8
                            } else if mode == 0 {
                                6
                            } else if mode == 2 || mode == 6 {
                                14
                            } else {
                                13
                            }
                    );
                    if mode >= 5 {
                        assert_eq!(snap.info2, if mode == 5 { 0x2ff0 } else { 0xb000 });
                    }
                    secondary += 1;
                    let before = *v.bytes();
                    match v.resolve_exception_delivery_after_exit().unwrap() {
                        DeliveryOutcome::Injected(event) => {
                            assert_eq!(stage, 1);
                            assert_eq!(
                                event.vector(),
                                if mode == 0 {
                                    11
                                } else if mode == 5 {
                                    14
                                } else {
                                    8
                                }
                            );
                            if mode >= 5 {
                                assert_eq!(v.guest_cr2(), snap.info2);
                            }
                            assert_eq!(v.guest_rip(), snap.rip);
                            stage = 2;
                        }
                        DeliveryOutcome::Shutdown(reason) => {
                            assert_eq!((mode, stage), (3, 2));
                            assert_eq!(
                                reason,
                                GuestShutdown::ExceptionDelivery {
                                    interrupted_vector: 8,
                                    fault_vector: 11
                                }
                            );
                            assert_eq!(v.bytes(), &before);
                            synthetic_shutdowns += 1;
                            terminal = true;
                            break;
                        }
                    }
                    continue;
                }
                v.clear_event_injection_after_exit().unwrap();
                assert_eq!(snap.code, 0x81);
                if frame.r12 != 1 {
                    print("DELIVERY failure-mode=");
                    hex(mode);
                    print("DELIVERY frame-error=");
                    hex(frame.r10);
                    print("DELIVERY failure-rip=");
                    hex(snap.rip);
                }
                assert_eq!(frame.r12, 1, "delivery handler witness");
                if mode == 0 || mode == 5 {
                    if stage == 2 {
                        assert_eq!(v.guest_rax(), 0);
                        assert_eq!(frame.r11, if mode == 0 { 1 } else { 14 });
                        let bytes = unsafe { memory::installed_instruction(snap.rip, 3) };
                        assert_eq!(
                            handle_exit_with_instruction(snap, v, &mut frame, bytes).unwrap(),
                            DispatchOutcome::ResumePrepared
                        );
                        stage = 3;
                    } else {
                        assert_eq!(stage, 3);
                        assert_eq!(v.guest_rax(), 1);
                        assert_eq!(
                            u64::from_le_bytes(v.bytes()[0x5d8..0x5e0].try_into().unwrap()),
                            0x9000
                        );
                        if mode == 0 {
                            np_returns += 1;
                        } else {
                            pf_returns += 1;
                        }
                        terminal = true;
                        break;
                    }
                } else {
                    assert!(mode == 1 || mode == 2 || mode == 6);
                    assert_eq!(stage, 2);
                    assert_eq!(v.guest_rax(), 1);
                    assert_eq!(frame.r11, 8);
                    df_terminals += 1;
                    terminal = true;
                    break;
                }
            }
            assert!(terminal, "bounded IDT-delivery entry budget exhausted");
            unsafe {
                if mode == 6 {
                    memory::verify_boundary_delivery_stack();
                } else {
                    memory::verify_exception_stack();
                }
            };
        }
    }
    assert_eq!(
        (
            secondary,
            np_returns,
            pf_returns,
            df_terminals,
            synthetic_shutdowns,
            direct_shutdowns
        ),
        (28, 4, 4, 12, 4, 4)
    );
    print("PASS interrupted-idt=24 ud-np-iretq-gp-pf-iretq-gp-pf-df-terminal-df-shutdown\n");
    print("PASS guest-shutdown=4 actual-intercept-terminal\n");
    for (name, value) in [
        ("entries", entries),
        ("secondary-faults", secondary),
        ("np-iretq", np_returns),
        ("pf-iretq", pf_returns),
        ("df-terminal", df_terminals),
        ("resolved-shutdown", synthetic_shutdowns),
        ("intercepted-shutdown", direct_shutdowns),
    ] {
        print("DELIVERY ");
        print(name);
        print("=");
        hex(value);
    }
}
