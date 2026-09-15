//! Real guest SVR/timer programming over both admitted buses. Source ticks are
//! bounded test data at guest checkpoints, not elapsed host or physical time.
use crate::{clock, field, interrupts::{enter, initialize}, memory, print, x2apic::admit, xstate};
use core::ptr;
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::ValidatedCapabilities, registers::GuestRegisters},
    svm::{
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        local_apic::{LocalApic, TickOutcome},
        vmcb::Vmcb,
        x2apic::{ApicMode, FixtureApic, QueueError, handle_fixture_msr},
        xapic::handle_fixture_mmio,
    },
};

/// # Safety
/// The caller exclusively owns the stopped CPU/VMCB and `prepared` mappings.
/// Installed code is immutable; no borrow survives an entry. APIC MSRs and the
/// unmapped guest APIC GPA are intercepted. No host APIC access is performed.
pub unsafe fn run(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    unsafe extern "C" { static guest_svr: u8; static guest_svr_handler: u8; }
    let address = |p: *const u8| 0x1000 + p as u64 - code_start as u64;
    let entry = address(ptr::addr_of!(guest_svr));
    let handler = address(ptr::addr_of!(guest_svr_handler));
    for bus in 0..2 {
        for round in 0..16 {
            unsafe {
                initialize(control, prepared, caps, state, entry, handler, true, round);
                admit(control);
                memory::install_idt(&[(0x50, handler, true), (0x51, handler, true)]);
            }
            let mut apic = FixtureApic::admit_fixed_bsp(
                LocalApic::admit_enabled(), if bus == 0 { 0xfee00900 } else { 0xfee00d00 },
            ).unwrap();
            let mut frame = GuestRegisters { r15: bus, ..GuestRegisters::default() };
            let (mut queries, mut consumed, mut accesses, mut entries) = (0, 0, 0, 0);
            let mut stopped = false;
            for _ in 0..80 {
                unsafe { enter(control, &mut frame, state, clock); }
                entries += 1;
                let v = unsafe { &mut *control };
                if apic.controller().delivery_armed() && apic.observe(v).unwrap().is_some() {
                    consumed += 1;
                }
                let snap = v.exit_snapshot();
                let bytes = unsafe { memory::installed_instruction(snap.rip, if snap.code == 0x81 { 3 } else { 2 }) };
                match snap.code {
                    0x400 => {
                        handle_fixture_mmio(&mut apic, v, &frame, bytes, prepared.mmio_mapping.as_ref().unwrap()).unwrap();
                        accesses += 1;
                    }
                    0x7c => {
                        handle_fixture_msr(&mut apic, v, &mut frame, bytes).unwrap();
                        accesses += 1;
                    }
                    0x81 => {
                        if v.guest_rax() == 0 {
                            assert_eq!(frame.r13, queries);
                            match queries {
                                0 => {
                                    assert_eq!(apic.controller().task_priority(), 0x70);
                                    assert!(apic.queue(0x50).unwrap());
                                    assert!(apic.queue(0x51).unwrap());
                                }
                                1 | 2 => {
                                    assert!(!apic.controller().software_enabled());
                                    assert_eq!(apic.controller().timer_lvt(), 0x10070);
                                    assert!(apic.queue(0x60).is_err());
                                    assert_eq!(apic.arm(v), Ok(None));
                                    assert!(apic.controller().pending(0x50));
                                    if queries == 1 {
                                        assert_eq!(consumed, 0);
                                        assert!(apic.controller().pending(0x51));
                                        assert_eq!(apic.advance_timer(8), Ok(TickOutcome::MaskedExpiration));
                                        assert!(!apic.controller().pending(0x70));
                                    } else {
                                        assert_eq!(consumed, 1);
                                        assert!(apic.controller().in_service(0x51));
                                        assert!(!apic.controller().pending(0x51));
                                    }
                                }
                                3 => {
                                    assert_eq!((consumed, frame.r12), (2, 2));
                                    assert_eq!(v.guest_rsp(), 0x9000);
                                    assert_eq!(apic.advance_timer(1), Ok(TickOutcome::Counting));
                                    assert_eq!(apic.controller().timer_remaining(), 3);
                                }
                                4 => {
                                    assert_eq!(apic.advance_timer(3), Ok(TickOutcome::Counting));
                                    assert_eq!(apic.controller().timer_remaining(), 1);
                                }
                                5 => assert_eq!(apic.advance_timer(2), Ok(TickOutcome::Queued)),
                                6 => {
                                    assert_eq!((consumed, frame.r12), (3, 3));
                                    // Two periodic expirations coalesce into one IRR bit.
                                    assert_eq!(apic.advance_timer(14), Ok(TickOutcome::Queued));
                                    assert_eq!(apic.controller().timer_remaining(), 2);
                                }
                                7 => {
                                    assert_eq!((consumed, frame.r12), (4, 4));
                                    assert_eq!(apic.advance_timer(4), Ok(TickOutcome::Queued));
                                }
                                8 => {
                                    assert_eq!((consumed, frame.r12), (5, 5));
                                    assert_eq!(apic.mode(), ApicMode::Disabled);
                                    assert_eq!(apic.controller().timer_remaining(), 3);
                                    assert_eq!(apic.advance_timer(6), Err(QueueError::Disabled));
                                    assert_eq!(apic.controller().timer_remaining(), 3);
                                    assert_eq!(apic.queue(0x50), Err(QueueError::Disabled));
                                    assert_eq!(apic.arm(v), Ok(None));
                                }
                                9 => assert_eq!(apic.advance_timer(6), Ok(TickOutcome::Queued)),
                                _ => panic!("unexpected timer checkpoint"),
                            }
                            queries += 1;
                        }
                        match handle_exit_with_instruction(snap, v, &mut frame, bytes).unwrap() {
                            DispatchOutcome::ResumePrepared => (),
                            DispatchOutcome::Stop(StopReason::Requested) => { stopped = true; break; }
                            _ => panic!("unexpected SVR fixture stop"),
                        }
                    }
                    _ => panic!("unexpected SVR fixture exit"),
                }
                if !apic.controller().delivery_armed() { apic.arm(v).unwrap(); }
            }
            assert!(stopped);
            assert_eq!((queries, consumed, frame.r12), (10, 6, 6));
            assert_eq!(accesses, 51 + bus);
            assert_eq!(entries, 62 + bus);
            assert_eq!(apic.controller().eoi_target(), None);
            assert_eq!(apic.controller().timer_remaining(), 0);
            assert!(!apic.controller().delivery_armed());
            for vector in 0..=255 { assert!(!apic.controller().pending(vector)); }
            assert_eq!(unsafe { (&*control).guest_rsp() }, 0x9000);
            unsafe { memory::verify_exception_stack(); }
        }
    }
    print("PASS apic-svr=32 disabled-retained-irr-isr-iretq\n");
    print("PASS apic-lvt=32 forced-mask-no-restore-version\n");
    print("PASS apic-timer-registers=32 oneshot-periodic-divided-source-ticks\n");
    print("PASS apic-timer-gates=32 software-and-base-disabled\n");
    print("APIC-TIMER per-session source-ticks=38 denied-ticks=6 queries=10 consumed=6\n");
    print("APIC-SVR entries-per-session xapic=62 x2apic=63\n");
    unsafe { refusals(control, prepared, code_start, caps, state, clock); }
}

/// Real stopped guest writes with deliberately refused operands/state. The
/// initial timer setup is supplied fixture admission; the refused operation
/// always executes in the guest and its unchanged stopped state is checked.
unsafe fn refusals(
    control: *mut Vmcb, prepared: &memory::Prepared, code_start: usize,
    caps: &ValidatedCapabilities, state: &xstate::State, clock: &clock::State,
) {
    unsafe extern "C" {
        static guest_xapic_write: u8;
        static guest_msr_gp_write: u8;
        static guest_svr_handler: u8;
    }
    let address = |p: *const u8| 0x1000 + p as u64 - code_start as u64;
    let cases = [
        (0xf0, 0x400), (0x320, 0x80000), (0x3e0, 4),
        (0x30, 0), (0x390, 0), (0x3e0, 1),
        (0x320, 0x51), (0x320, 0x20050), (0xf0, 0xff),
    ];
    for bus in 0..2 {
        for (case, &(offset, value)) in cases.iter().enumerate() {
            if bus == 1 && case < 5 { continue; }
            let entry = address(if bus == 0 { ptr::addr_of!(guest_xapic_write) } else { ptr::addr_of!(guest_msr_gp_write) });
            unsafe {
                initialize(control, prepared, caps, state, entry, address(ptr::addr_of!(guest_svr_handler)), true, case);
                admit(control);
                field(control, 0x5f8, (value as u64).to_le_bytes());
            }
            let mut inner = LocalApic::admit_enabled();
            if (5..8).contains(&case) {
                inner.write_timer_lvt(0x50).unwrap();
                inner.write_timer_initial(3).unwrap();
            }
            let mut apic = FixtureApic::admit_fixed_bsp(inner, if bus == 0 { 0xfee00900 } else { 0xfee00d00 }).unwrap();
            let mut frame = GuestRegisters { rbx: memory::APIC_ALIAS + offset, rcx: 0x800 + offset / 16, ..GuestRegisters::default() };
            if case == 8 {
                apic.queue(0x50).unwrap();
                assert_eq!(apic.arm(unsafe { &mut *control }), Ok(Some(0x50)));
            }
            unsafe { enter(control, &mut frame, state, clock); }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, if bus == 0 { 0x400 } else { 0x7c });
            if case == 8 { assert_eq!(apic.observe(v), Ok(None)); }
            let saved_vmcb = *v.bytes();
            let saved_frame = frame;
            let saved_owner = snapshot(&apic);
            let bytes = unsafe { memory::installed_instruction(entry, 2) };
            if bus == 0 {
                assert!(handle_fixture_mmio(&mut apic, v, &frame, bytes, prepared.mmio_mapping.as_ref().unwrap()).is_err());
            } else {
                assert!(handle_fixture_msr(&mut apic, v, &mut frame, bytes).is_err());
            }
            assert_eq!(v.bytes(), &saved_vmcb);
            assert_eq!(frame, saved_frame);
            assert_eq!(snapshot(&apic), saved_owner);
        }
    }
    print("PASS apic-register-refusals=13 actual-exit-unchanged\n");
}

fn snapshot(apic: &FixtureApic) -> ([u64; 9], [bool; 256], [bool; 256]) {
    let c = apic.controller();
    (
        [apic.apic_base(), c.spurious_vector_register() as u64, c.timer_lvt() as u64,
         c.timer_initial() as u64, c.timer_remaining() as u64, c.timer_divide() as u64,
         c.task_priority() as u64, c.processor_priority() as u64, c.delivery_armed() as u64],
        core::array::from_fn(|v| c.pending(v as u8)),
        core::array::from_fn(|v| c.in_service(v as u8)),
    )
}
