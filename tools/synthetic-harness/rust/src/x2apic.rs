//! Real trapped MSRs, checked #GP retries and bounded APIC mode cycles.
use crate::{
    clock, field,
    interrupts::{enter, initialize},
    memory, print, xstate,
};
use core::ptr;
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::ValidatedCapabilities, registers::GuestRegisters},
    svm::{
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::{
            ApicMode, Cr8Error, FIXTURE_APIC_BASE, FixtureApic, MsrError, handle_fixture_cr8_write,
            handle_fixture_msr,
        },
        xapic::handle_fixture_mmio,
    },
};
unsafe extern "C" {
    static guest_x2apic: u8;
    static guest_x2apic_ready: u8;
    static guest_x2apic_handler: u8;
    static guest_x2apic_write_bad: u8;
}
/// # Safety
/// Same owned CPU, stopped VMCB, mappings and bridge contract as interrupts::run.
/// All MSRs and CR8 writes are intercepted. The fixture admits a fixed x2APIC
/// mode; default CPUID still withholds generic APIC/x2APIC support.
pub unsafe fn run(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    let address = |p: *const u8| 0x1000 + p as u64 - code_start as u64;
    let handler = address(ptr::addr_of!(guest_x2apic_handler));
    for round in 0..16 {
        unsafe {
            initialize(
                control,
                prepared,
                caps,
                state,
                address(ptr::addr_of!(guest_x2apic)),
                handler,
                true,
                round,
            );
            admit(control);
        }
        let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
        let mut frame = GuestRegisters {
            r14: address(ptr::addr_of!(guest_x2apic_ready)),
            ..GuestRegisters::default()
        };
        let mut msrs = 0;
        let mut cr8_writes = 0;
        let mut queries = 0;
        let mut stopped = false;
        for _ in 0..32 {
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            if apic.controller().delivery_armed() {
                apic.observe(v).unwrap();
            }
            let snap = v.exit_snapshot();
            let offset = (snap.rip - 0x1000) as usize;
            let length = if snap.code == 0x81 {
                3
            } else if snap.code == 0x18 {
                4
            } else {
                2
            };
            let bytes = code.get(offset..offset + length).unwrap();
            if snap.code == 0x7c {
                handle_fixture_msr(&mut apic, v, &mut frame, bytes).unwrap();
                msrs += 1;
            } else if snap.code == 0x18 {
                let old_frame = frame;
                let flags: [u8; 8] = v.bytes()[0x570..0x578].try_into().unwrap();
                handle_fixture_cr8_write(&mut apic, v, &frame, bytes).unwrap();
                assert_eq!(frame, old_frame);
                assert_eq!(&v.bytes()[0x570..0x578], &flags);
                cr8_writes += 1;
            } else {
                if snap.code == 0x81 && v.guest_rax() == 0 {
                    if queries == 0 {
                        assert_eq!(frame.r11, 0);
                        assert_eq!(apic.controller().task_priority(), 0x50);
                        apic.queue(0x50).unwrap();
                    } else {
                        assert_eq!(frame.r11, 1);
                        assert_eq!(frame.r12, 1);
                        assert_eq!(v.guest_rsp(), 0x9000);
                    }
                    queries += 1;
                }
                match handle_exit_with_instruction(snap, v, &mut frame, bytes).unwrap() {
                    DispatchOutcome::ResumePrepared => (),
                    DispatchOutcome::Stop(StopReason::Requested) => {
                        stopped = true;
                        break;
                    }
                    _ => panic!("unexpected x2APIC fixture stop"),
                }
            }
            if !apic.controller().delivery_armed() {
                apic.arm(v).unwrap();
            }
        }
        assert!(stopped);
        assert_eq!(msrs, 13);
        assert_eq!(cr8_writes, 2);
        assert_eq!(queries, 2);
        assert_eq!(frame.r12, 1);
        assert_eq!(apic.controller().task_priority(), 0x20);
        assert_eq!(apic.controller().eoi_target(), None);
        assert!(!apic.controller().pending(0x50));
        unsafe {
            memory::verify_exception_stack();
        }
    }
    // A legal but unsupported base relocation remains a terminal policy stop.
    unsafe {
        initialize(
            control,
            prepared,
            caps,
            state,
            address(ptr::addr_of!(guest_x2apic_write_bad)),
            handler,
            true,
            0,
        );
        admit(control);
        field(control, 0x5f8, 0xfed00d00u64.to_le_bytes());
    }
    let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
    let mut frame = GuestRegisters {
        rcx: 0x1b,
        ..GuestRegisters::default()
    };
    unsafe {
        enter(control, &mut frame, state, clock);
    }
    let v = unsafe { &mut *control };
    let before = *v.bytes();
    let old_frame = frame;
    let offset = (v.guest_rip() - 0x1000) as usize;
    assert!(matches!(
        handle_fixture_msr(&mut apic, v, &mut frame, &code[offset..offset + 2]),
        Err(MsrError::Unsupported {
            index: 0x1b,
            write: true
        })
    ));
    assert_eq!(v.bytes(), &before);
    assert_eq!(frame, old_frame);
    unsafe {
        run_faults_and_modes(control, prepared, code, code_start, caps, state, clock);
        run_cr8(control, prepared, code, code_start, caps, state, clock);
        run_mmio(control, prepared, code, code_start, caps, state, clock);
        crate::svr::run(control, prepared, code_start, caps, state, clock);
    }
    print("PASS apic-cr8-writes=32 same-class-priority-iretq\n");
    print("PASS x2apic-msr=16 tpr-eoi-bitmaps\n");
    print("PASS x2apic-refusals=1 no-state-change\n");
}
/// Fixed profile setup before entry. APM Appendix B CR write vector at 02h,
/// bit8 intercepts CR8 writes; virtual masking at60h owns guest CR8 reads.
pub(super) unsafe fn admit(control: *mut Vmcb) {
    unsafe {
        field(control, 0x002, (1u16 << 8).to_le_bytes());
        field(control, 0x060, (1u64 << 24).to_le_bytes());
    }
}

/// Real final-access NPFs over the same APIC used by MSR and CR8 handlers.
/// # Safety
/// Same stopped single-CPU, immutable code and mapping ownership as `run`.
unsafe fn run_mmio(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_xapic: u8;
        static guest_xapic_ready: u8;
        static guest_xapic_handler: u8;
        static guest_xapic_read: u8;
        static guest_xapic_write: u8;
        static guest_xapic_word: u8;
        static guest_xapic_qword: u8;
    }
    let address = |p: *const u8| 0x1000 + p as u64 - code_start as u64;
    let handler = address(ptr::addr_of!(guest_xapic_handler));
    for round in 0..16 {
        unsafe {
            initialize(
                control,
                prepared,
                caps,
                state,
                address(ptr::addr_of!(guest_xapic)),
                handler,
                true,
                round,
            );
            admit(control);
        }
        let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), 0xfee00900).unwrap();
        let mut frame = GuestRegisters {
            r14: address(ptr::addr_of!(guest_xapic_ready)),
            ..GuestRegisters::default()
        };
        let (mut reads, mut writes, mut msrs, mut cr8, mut queries, mut consumed) =
            (0, 0, 0, 0, 0, 0);
        let mut stopped = false;
        for _ in 0..40 {
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            if apic.controller().delivery_armed() && apic.observe(v).unwrap().is_some() {
                consumed += 1;
            }
            let snap = v.exit_snapshot();
            let length = if snap.code == 0x81 {
                3
            } else if snap.code == 0x18 {
                4
            } else {
                2
            };
            let bytes = unsafe { memory::installed_instruction(snap.rip, length) };
            match snap.code {
                0x400 => {
                    let old_frame = frame;
                    let old_rax = v.guest_rax();
                    let flags: [u8; 8] = v.bytes()[0x570..0x578].try_into().unwrap();
                    let write = bytes == [0x89, 0x03];
                    // Retain one observed sample per access direction; the
                    // validator checks every actual raw NPF, operand and GPA.
                    if round == 0 && ((write && writes == 0) || (!write && reads == 0)) {
                        print(if write {
                            "XAPIC-NPF write-info1="
                        } else {
                            "XAPIC-NPF read-info1="
                        });
                        crate::hex(snap.info1);
                        print("XAPIC-NPF final-gpa=");
                        crate::hex(snap.info2);
                    }
                    handle_fixture_mmio(
                        &mut apic,
                        v,
                        &frame,
                        bytes,
                        prepared.mmio_mapping.as_ref().unwrap(),
                    )
                    .unwrap();
                    assert_eq!(frame, old_frame);
                    assert_eq!(&v.bytes()[0x570..0x578], &flags);
                    assert_eq!(v.guest_rip(), snap.rip + 2);
                    if write {
                        assert_eq!(v.guest_rax(), old_rax);
                        writes += 1;
                    } else {
                        assert_eq!(v.guest_rax() >> 32, 0);
                        reads += 1;
                    }
                }
                0x7c => {
                    handle_fixture_msr(&mut apic, v, &mut frame, bytes).unwrap();
                    msrs += 1;
                }
                0x18 => {
                    handle_fixture_cr8_write(&mut apic, v, &frame, bytes).unwrap();
                    cr8 += 1;
                }
                _ => {
                    if snap.code == 0x81 && v.guest_rax() == 0 {
                        if queries == 0 {
                            assert_eq!(frame.r12, 0);
                            assert_eq!(apic.controller().task_priority(), 0x50);
                            assert!(apic.queue(0x50).unwrap());
                        } else {
                            assert_eq!(frame.r12, 1);
                            assert_eq!(v.guest_rsp(), 0x9000);
                            assert_eq!(consumed, 1);
                        }
                        queries += 1;
                    }
                    match handle_exit_with_instruction(snap, v, &mut frame, bytes).unwrap() {
                        DispatchOutcome::ResumePrepared => (),
                        DispatchOutcome::Stop(StopReason::Requested) => {
                            stopped = true;
                            break;
                        }
                        _ => panic!("unexpected xAPIC fixture stop"),
                    }
                }
            }
            if !apic.controller().delivery_armed() {
                apic.arm(v).unwrap();
            }
        }
        assert!(stopped);
        assert_eq!(
            (reads, writes, msrs, cr8, queries, consumed),
            (13, 3, 6, 1, 2, 1)
        );
        assert_eq!(frame.r12, 1);
        assert_eq!(apic.mode(), ApicMode::XApic);
        assert_eq!(apic.controller().task_priority(), 0x2b);
        assert_eq!(apic.controller().eoi_target(), None);
        assert!(!apic.controller().pending(0x50));
        assert!(!apic.controller().delivery_armed());
        unsafe {
            memory::verify_exception_stack();
        }
    }
    print("PASS xapic-mmio=16 id-tpr-ppr-irr-isr-eoi\n");
    print("PASS xapic-cross-mode=16 retained-owner\n");
    print("PASS xapic-iretq=16 exactly-once\n");
    print("XAPIC-NPF per-session reads=13 writes=3 consumed=1\n");

    // Each refusal starts from an actual stopped guest NPF. No exit fields or
    // guest registers are fabricated after entry; bad provenance is supplied
    // only as an input to the adapter, and is never a fabricated guest fault.
    for case in 0..13 {
        let write = matches!(case, 1 | 2 | 3 | 11);
        let entry = address(match case {
            6 => ptr::addr_of!(guest_xapic_word),
            7 => ptr::addr_of!(guest_xapic_qword),
            _ if write => ptr::addr_of!(guest_xapic_write),
            _ => ptr::addr_of!(guest_xapic_read),
        });
        let base = match case {
            4 => FIXTURE_APIC_BASE,
            5 => 0xfee00100,
            _ => 0xfee00900,
        };
        let operand = match case {
            0 => memory::APIC_ALIAS + 0x90,
            1 => memory::APIC_ALIAS + 0x20,
            2 => memory::APIC_ALIAS + 0xb0,
            8 => memory::APIC_ALIAS + 0x81,
            9 => 0x6000,
            _ => memory::APIC_ALIAS + 0x80,
        };
        unsafe {
            initialize(
                control,
                prepared,
                caps,
                state,
                if case == 12 {
                    memory::APIC_ALIAS
                } else {
                    entry
                },
                handler,
                true,
                case,
            );
            admit(control);
            field(
                control,
                0x5f8,
                (if case == 3 { 0x100u64 } else { 1u64 }).to_le_bytes(),
            );
        }
        let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), base).unwrap();
        let mut frame = GuestRegisters {
            rbx: operand,
            ..GuestRegisters::default()
        };
        if case == 11 {
            apic.queue(0x50).unwrap();
            assert_eq!(apic.arm(unsafe { &mut *control }).unwrap(), Some(0x50));
        }
        unsafe {
            enter(control, &mut frame, state, clock);
        }
        let v = unsafe { &mut *control };
        assert_eq!(v.exit_snapshot().code, 0x400);
        if case == 11 {
            assert_eq!(apic.observe(v).unwrap(), None);
        }
        let before = *v.bytes();
        let old_frame = frame;
        let before_owner = (
            apic.apic_base(),
            apic.controller().task_priority(),
            apic.controller().processor_priority(),
            apic.controller().eoi_target(),
            apic.controller().delivery_armed(),
        );
        let length = if case == 6 || case == 7 { 3 } else { 2 };
        let bytes = if case == 10 {
            // Correct bytes at a different host address are not owned evidence.
            &code[(entry - 0x1000) as usize..(entry - 0x1000) as usize + length]
        } else {
            unsafe { memory::installed_instruction(entry, length) }
        };
        assert!(
            handle_fixture_mmio(
                &mut apic,
                v,
                &frame,
                bytes,
                prepared.mmio_mapping.as_ref().unwrap()
            )
            .is_err()
        );
        assert_eq!(v.bytes(), &before);
        assert_eq!(frame, old_frame);
        assert_eq!(
            (
                apic.apic_base(),
                apic.controller().task_priority(),
                apic.controller().processor_priority(),
                apic.controller().eoi_target(),
                apic.controller().delivery_armed()
            ),
            before_owner
        );
        for vector in 0..=255 {
            assert_eq!(
                apic.controller().pending(vector),
                case == 11 && vector == 0x50
            );
            assert!(!apic.controller().in_service(vector));
        }
    }
    print("PASS xapic-refusals=13 actual-npf-unchanged\n");
}

/// Same stopped-CPU and owned backing contract as run. #GP retries happen only
/// after the guest handler repairs operands and performs IRETQ at unchanged RIP.
unsafe fn run_faults_and_modes(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_msr_gp_read: u8;
        static guest_msr_gp_write: u8;
        static guest_msr_gp_handler: u8;
        static guest_apic_modes: u8;
    }
    let address = |p: *const u8| 0x1000 + p as u64 - code_start as u64;
    let handler = address(ptr::addr_of!(guest_msr_gp_handler));
    // base, MSR index, original EDX:EAX, write?, repaired index, repaired value
    let cases: [(u64, u64, u64, bool, u64, u64); 18] = [
        (0xfee00d00, 0x80b, 0, false, 0x808, 0),
        (0xfee00d00, 0x808, 0x100, true, 0x808, 0x20),
        (0xfee00d00, 0x80b, 1, true, 0x80b, 0),
        (0xfee00100, 0x808, 0, false, 0x1b, 0),
        (0xfee00900, 0x808, 0, false, 0x1b, 0),
        (0xfee00100, 0x1b, 0xfee00d00, true, 0x1b, 0xfee00900),
        (0xfee00d00, 0x1b, 0xfee00900, true, 0x1b, 0xfee00d00),
        (0xfee00d00, 0x1b, 0xfee00500, true, 0x1b, 0xfee00d00),
        (0xfee00d00, 0x800, 0, false, 0x808, 0),
        (0xfee00d00, 0x1b, 0xfee00f00, true, 0x1b, 0xfee00d00),
        (0xfee00d00, 0x802, 0, true, 0x808, 0x2b),
        (0xfee00d00, 0x80f, 0x400, true, 0x80f, 0x1ff),
        (0xfee00d00, 0x80f, 0x1000001ff, true, 0x80f, 0x1ff),
        (0xfee00d00, 0x832, 0x80000, true, 0x832, 0x10050),
        (0xfee00d00, 0x83e, 4, true, 0x83e, 0xb),
        (0xfee00d00, 0x803, 0, true, 0x808, 0),
        (0xfee00d00, 0x839, 0, true, 0x808, 0),
        (0xfee00d00, 0x838, 0x100000000, true, 0x838, 0),
    ];
    for round in 0..16 {
        for &(base, index, value, write, repair_index, repair_value) in &cases {
            let entry = address(if write {
                ptr::addr_of!(guest_msr_gp_write)
            } else {
                ptr::addr_of!(guest_msr_gp_read)
            });
            unsafe {
                initialize(control, prepared, caps, state, entry, handler, true, round);
                admit(control);
                memory::install_idt(&[(13, handler, true)]);
                field(control, 0x5f8, value.to_le_bytes());
            }
            let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), base).unwrap();
            let mut frame = GuestRegisters {
                rcx: index,
                rdx: value >> 32,
                rbx: entry,
                r13: repair_value as u32 as u64,
                r14: repair_index,
                r15: repair_value >> 32,
                ..GuestRegisters::default()
            };
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            let bytes = &code[(entry - 0x1000) as usize..(entry - 0x1000) as usize + 2];
            let old = *v.bytes();
            let old_frame = frame;
            assert_eq!(
                handle_fixture_msr(&mut apic, v, &mut frame, bytes),
                Err(MsrError::GeneralProtectionRequired)
            );
            assert_eq!(v.bytes(), &old);
            assert_eq!(frame, old_frame);
            v.queue_msr_general_protection(bytes).unwrap();
            assert_eq!(v.guest_rip(), entry);
            assert_eq!(v.event_injection(), 0x80000b0d);
            assert_eq!(frame, old_frame);
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x7c);
            assert_eq!(v.guest_rip(), entry);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.guest_rsp(), 0x9000);
            v.clear_event_injection_after_exit().unwrap();
            handle_fixture_msr(&mut apic, v, &mut frame, bytes).unwrap();
            assert_eq!(v.guest_rip(), entry + 2);
            if index == 0x802 {
                assert_eq!(apic.controller().task_priority(), 0x2b);
                assert_eq!(v.virtual_interrupt_control() & 15, 2);
                assert_eq!(frame.rcx, 0x808);
            }
            match repair_index {
                0x80f => assert_eq!(apic.controller().spurious_vector_register(), repair_value as u32),
                0x832 => assert_eq!(apic.controller().timer_lvt(), repair_value as u32),
                0x83e => assert_eq!(apic.controller().timer_divide(), repair_value as u32),
                0x838 => assert_eq!(apic.controller().timer_initial(), repair_value as u32),
                _ => (),
            }
            if repair_index == 0x1b {
                if write {
                    assert_eq!(apic.apic_base(), repair_value);
                } else {
                    assert_eq!(v.guest_rax(), apic.apic_base());
                }
            }
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 1);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.event_injection(), 0);
            unsafe {
                memory::verify_exception_stack();
            }
        }
        let entry = address(ptr::addr_of!(guest_apic_modes));
        unsafe {
            initialize(control, prepared, caps, state, entry, handler, true, round);
            admit(control);
        }
        let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), 0xfee00100).unwrap();
        let mut frame = GuestRegisters::default();
        let mut count = 0;
        let mut done = false;
        for _ in 0..16 {
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            let snap = v.exit_snapshot();
            if snap.code == 0x81 {
                assert_eq!(v.guest_rax(), 1);
                done = true;
                break;
            }
            let offset = (snap.rip - 0x1000) as usize;
            handle_fixture_msr(&mut apic, v, &mut frame, &code[offset..offset + 2]).unwrap();
            count += 1;
            if apic.mode() == ApicMode::Disabled {
                assert_eq!(apic.arm(v), Ok(None));
                assert!(apic.queue(0x50).is_err());
            }
        }
        assert!(done);
        assert_eq!(count, 11);
        assert_eq!(apic.mode(), ApicMode::X2Apic);
    }
    // Not-present #GP gate must retain interrupted-delivery evidence and stop.
    let entry = address(ptr::addr_of!(guest_msr_gp_write));
    unsafe {
        initialize(control, prepared, caps, state, entry, handler, true, 0);
        admit(control);
        memory::install_idt(&[(13, handler, false)]);
        field(control, 0x5f8, 0x100u64.to_le_bytes());
    }
    let mut apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
    let mut frame = GuestRegisters {
        rcx: 0x808,
        ..GuestRegisters::default()
    };
    unsafe {
        enter(control, &mut frame, state, clock);
    }
    let v = unsafe { &mut *control };
    let offset = (v.guest_rip() - 0x1000) as usize;
    assert_eq!(
        handle_fixture_msr(&mut apic, v, &mut frame, &code[offset..offset + 2]),
        Err(MsrError::GeneralProtectionRequired)
    );
    v.queue_msr_general_protection(&code[offset..offset + 2])
        .unwrap();
    unsafe {
        enter(control, &mut frame, state, clock);
    }
    let v = unsafe { &mut *control };
    assert_eq!(v.exit_snapshot().code, 0x4b);
    let prior = u64::from_le_bytes(v.bytes()[0x88..0x90].try_into().unwrap());
    assert_eq!(prior & 0x800007ff, 0x8000030d);
    let old = *v.bytes();
    assert!(v.clear_event_injection_after_exit().is_err());
    assert!(v.reflect_exception().is_err());
    assert_eq!(v.bytes(), &old);
    print("PASS apic-register-gp=112 repaired-operand-iretq-retry\n");
    print("PASS msr-gp=160 repaired-operand-iretq-retry\n");
    print("PASS x2apic-id-gp=16 repaired-tpr-iretq\n");
    print("PASS msr-gp-nested-delivery-refused\n");
    print("PASS apic-modes=16 disabled-xapic-x2apic\n");
}

/// Same exclusive CPU/backing contract as run; only benign register fixtures.
unsafe fn run_cr8(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_cr8_source_0: u8;
        static guest_cr8_source_1: u8;
        static guest_cr8_source_2: u8;
        static guest_cr8_source_3: u8;
        static guest_cr8_source_4: u8;
        static guest_cr8_source_5: u8;
        static guest_cr8_source_6: u8;
        static guest_cr8_source_7: u8;
        static guest_cr8_source_8: u8;
        static guest_cr8_source_9: u8;
        static guest_cr8_source_10: u8;
        static guest_cr8_source_11: u8;
        static guest_cr8_source_12: u8;
        static guest_cr8_source_13: u8;
        static guest_cr8_source_14: u8;
        static guest_cr8_source_15: u8;
        static guest_msr_gp_handler: u8;
    }
    let address = |p: *const u8| 0x1000 + p as u64 - code_start as u64;
    let entries = [
        address(ptr::addr_of!(guest_cr8_source_0)),
        address(ptr::addr_of!(guest_cr8_source_1)),
        address(ptr::addr_of!(guest_cr8_source_2)),
        address(ptr::addr_of!(guest_cr8_source_3)),
        address(ptr::addr_of!(guest_cr8_source_4)),
        address(ptr::addr_of!(guest_cr8_source_5)),
        address(ptr::addr_of!(guest_cr8_source_6)),
        address(ptr::addr_of!(guest_cr8_source_7)),
        address(ptr::addr_of!(guest_cr8_source_8)),
        address(ptr::addr_of!(guest_cr8_source_9)),
        address(ptr::addr_of!(guest_cr8_source_10)),
        address(ptr::addr_of!(guest_cr8_source_11)),
        address(ptr::addr_of!(guest_cr8_source_12)),
        address(ptr::addr_of!(guest_cr8_source_13)),
        address(ptr::addr_of!(guest_cr8_source_14)),
        address(ptr::addr_of!(guest_cr8_source_15)),
    ];
    let handler = address(ptr::addr_of!(guest_msr_gp_handler));
    for (source, &entry) in entries.iter().enumerate() {
        unsafe {
            initialize(control, prepared, caps, state, entry, handler, true, source);
            admit(control);
            field(control, 0x5f8, 5u64.to_le_bytes());
            // RSP is a source register too; IF=0 and no stack instruction runs
            // until the guest restores its stack after the intercepted MOV.
            if source == 4 {
                field(control, 0x5d8, 5u64.to_le_bytes());
            }
            (&mut *control).set_virtual_interrupt_tpr(5).unwrap();
        }
        let mut inner = LocalApic::admit_enabled();
        inner.set_task_priority(0x5b).unwrap();
        let mut apic = FixtureApic::admit_fixed_bsp(inner, FIXTURE_APIC_BASE).unwrap();
        let mut frame = GuestRegisters {
            rcx: 5,
            rdx: 5,
            rbx: 5,
            rbp: 5,
            rsi: 5,
            rdi: 5,
            r8: 5,
            r9: 5,
            r10: 5,
            r11: 5,
            r12: 5,
            r13: 5,
            r14: 5,
            r15: 5,
        };
        unsafe {
            enter(control, &mut frame, state, clock);
        }
        let v = unsafe { &mut *control };
        assert_eq!(v.exit_snapshot().code, 0x18);
        assert_eq!(v.guest_rip(), entry);
        let old = *v.bytes();
        let old_frame = frame;
        let offset = (entry - 0x1000) as usize;
        handle_fixture_cr8_write(&mut apic, v, &frame, &code[offset..offset + 4]).unwrap();
        assert_eq!(v.guest_rip(), entry + 4);
        assert_eq!(apic.controller().task_priority(), 0x50);
        assert_eq!(v.virtual_interrupt_control() & 0xf, 5);
        assert_eq!(frame, old_frame);
        assert_eq!(&v.bytes()[0x570..0x578], &old[0x570..0x578]);
        assert_eq!(v.guest_rax(), 5);
        assert_eq!(v.guest_rsp(), if source == 4 { 5 } else { 0x9000 });
        unsafe {
            enter(control, &mut frame, state, clock);
        }
        let v = unsafe { &mut *control };
        assert_eq!(v.exit_snapshot().code, 0x81);
        assert_eq!(v.guest_rax(), 1);
        assert_eq!(v.guest_rsp(), 0x9000);
    }
    // APM2 Table15-7: all normal CR8 exceptions precede the CR intercept.
    // Hardware #GP is reflected; the handler repairs RAX and retries at same RIP.
    let entry = entries[0];
    let mut ordering_gaps = 0;
    let mut fault_retries = 0;
    for round in 0..16 {
        for value in [0x10u64, 1u64 << 63] {
            unsafe {
                initialize(control, prepared, caps, state, entry, handler, true, round);
                admit(control);
                memory::install_idt(&[(13, handler, true)]);
                field(control, 0x5f8, value.to_le_bytes());
            }
            let mut apic =
                FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
            let mut frame = GuestRegisters {
                rbx: entry,
                r13: 5,
                ..GuestRegisters::default()
            };
            let before_fault_frame = frame;
            let before_fault_flags: [u8; 8] =
                unsafe { (&*control).bytes()[0x570..0x578].try_into().unwrap() };
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.guest_rip(), entry);
            assert_eq!(v.guest_rax(), value);
            if v.exit_snapshot().code == 0x18 {
                // Known backend ordering gap: invalid operand reached intercept.
                // Reject unchanged; never pretend a hardware fault was delivered.
                let old = *v.bytes();
                let old_frame = frame;
                let offset = (entry - 0x1000) as usize;
                assert_eq!(
                    handle_fixture_cr8_write(&mut apic, v, &frame, &code[offset..offset + 4]),
                    Err(Cr8Error::InvalidOperand { value })
                );
                assert_eq!(v.bytes(), &old);
                assert_eq!(frame, old_frame);
                assert_eq!(apic.controller().task_priority(), 0);
                assert_eq!(
                    apic,
                    FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap()
                );
                ordering_gaps += 1;
                continue;
            }
            assert_eq!(v.exit_snapshot().code, 0x4d);
            assert_eq!(v.exit_snapshot().info1, 0);
            assert_eq!(frame, before_fault_frame);
            assert_eq!(v.guest_rsp(), 0x9000);
            assert_eq!(&v.bytes()[0x570..0x578], &before_fault_flags);
            assert_eq!(v.guest_rip(), entry);
            assert_eq!(v.guest_rax(), value);
            assert_eq!(apic.controller().task_priority(), 0);
            assert_eq!(v.reflect_exception().unwrap().vector(), 13);
            assert_eq!(v.guest_rip(), entry);
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x18);
            assert_eq!(v.guest_rip(), entry);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.guest_rax(), 5);
            assert_eq!(v.guest_rsp(), 0x9000);
            v.clear_event_injection_after_exit().unwrap();
            let offset = (entry - 0x1000) as usize;
            handle_fixture_cr8_write(&mut apic, v, &frame, &code[offset..offset + 4]).unwrap();
            assert_eq!(apic.controller().task_priority(), 0x50);
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 1);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.event_injection(), 0);
            unsafe {
                memory::verify_exception_stack();
            }
            fault_retries += 1;
        }
    }
    // Exercise the backend's normal CR8 write path without an SVM intercept.
    // This is a separate backend fixture with no LocalApic owner to go stale.
    for value in 0u64..16 {
        unsafe {
            initialize(
                control,
                prepared,
                caps,
                state,
                entry,
                handler,
                true,
                value as usize,
            );
            admit(control);
            field(control, 0x002, 0u16.to_le_bytes());
            field(control, 0x5f8, value.to_le_bytes());
        }
        let mut frame = GuestRegisters::default();
        unsafe {
            enter(control, &mut frame, state, clock);
        }
        let v = unsafe { &mut *control };
        assert_eq!(v.exit_snapshot().code, 0x81);
        assert_eq!(v.guest_rax(), 1);
        assert_eq!(v.virtual_interrupt_control() & 15, value);
    }
    let mut direct_faults = 0;
    let mut direct_gaps = 0;
    for round in 0..16 {
        for value in [0x10u64, 1u64 << 63] {
            unsafe {
                initialize(control, prepared, caps, state, entry, handler, true, round);
                admit(control);
                field(control, 0x002, 0u16.to_le_bytes());
                field(control, 0x5f8, value.to_le_bytes());
                memory::install_idt(&[(13, handler, true)]);
            }
            let mut frame = GuestRegisters {
                rbx: entry,
                r13: 5,
                ..GuestRegisters::default()
            };
            let before_fault_frame = frame;
            let before_fault_flags: [u8; 8] =
                unsafe { (&*control).bytes()[0x570..0x578].try_into().unwrap() };
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            if v.exit_snapshot().code == 0x81 {
                // Old backend masks invalid bits and executes the instruction.
                assert_eq!(v.guest_rax(), 1);
                assert_eq!(v.virtual_interrupt_control() & 15, 0);
                assert_eq!(frame.r12, 0);
                direct_gaps += 1;
                continue;
            }
            assert_eq!(v.exit_snapshot().code, 0x4d);
            assert_eq!(v.exit_snapshot().info1, 0);
            assert_eq!(frame, before_fault_frame);
            assert_eq!(v.guest_rsp(), 0x9000);
            assert_eq!(&v.bytes()[0x570..0x578], &before_fault_flags);
            assert_eq!(v.guest_rip(), entry);
            assert_eq!(v.guest_rax(), value);
            assert_eq!(v.virtual_interrupt_control() & 15, 0);
            assert_eq!(v.reflect_exception().unwrap().vector(), 13);
            assert_eq!(v.guest_rip(), entry);
            unsafe {
                enter(control, &mut frame, state, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 1);
            assert_eq!(v.guest_rsp(), 0x9000);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.virtual_interrupt_control() & 15, 5);
            v.clear_event_injection_after_exit().unwrap();
            assert_eq!(v.event_injection(), 0);
            unsafe {
                memory::verify_exception_stack();
            }
            direct_faults += 1;
        }
    }
    assert_eq!(direct_faults + direct_gaps, 32);
    print("PASS apic-cr8-direct-writes=16 full-priority-range\n");
    if direct_gaps == 32 {
        print("GAP apic-cr8-direct-gp=32 reserved-operand-truncated\n");
    } else {
        assert_eq!(direct_faults, 32);
        print("PASS apic-cr8-direct-gp=32 hardware-fault-iretq-retry\n");
    }
    assert_eq!(fault_retries + ordering_gaps, 32);
    print("PASS apic-cr8-sources=16 gpr-vmcb-preserved\n");
    if ordering_gaps == 32 {
        print("GAP apic-cr8-gp=32 intercept-before-operand-fault refused-unchanged\n");
    } else {
        assert_eq!(fault_retries, 32);
        print("PASS apic-cr8-gp=32 hardware-fault-iretq-retry\n");
    }
}
