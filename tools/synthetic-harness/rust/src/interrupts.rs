//! Bounded virtual INTR delivery; no physical interrupt routing or APIC model.
use crate::{clock, execution, memory, print, xstate};
use core::{arch::asm, ptr};
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::ValidatedCapabilities, registers::GuestRegisters},
    guest::state::GuestStateRequest,
    svm::{
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        events::{ExternalInterruptError, ExternalInterruptState, PendingExternalInterrupt},
        vmcb::{EventIntercept, Vmcb},
    },
};

unsafe extern "C" {
    static guest_irq_if: u8;
    static guest_irq_if_ready: u8;
    static guest_irq_tpr: u8;
    static guest_irq_tpr_ready: u8;
    static guest_irq_tpr_stopped: u8;
    static guest_irq_tpr_stopped_ready: u8;
    static guest_irq_handler: u8;
    static guest_irq_probe: u8;
    static guest_irq_probe_ready: u8;
}

const VECTOR: u8 = 0x50;
const V_IRQ: u64 = 1 << 8;

/// # Safety
/// Same exclusive stopped VMCB, mapped backing and single-CPU bridge contract
/// as session::run_sessions. Host IF remains clear; V_INTR_MASKING keeps guest
/// IF/CR8 independent of physical INTR/TPR. No outstanding VMCB/backing borrows
/// survive VMRUN. AMD APM vol.2 rev.3.44 sections8.9,15.5,15.20,15.21.1-6.
pub unsafe fn run(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    let address = |symbol: *const u8| 0x1000 + symbol as u64 - code_start as u64;
    let handler = address(ptr::addr_of!(guest_irq_handler));
    for round in 0..32 {
        if round == 16 {
            print("PASS guest-interrupt-if-shadow=16 iretq-once\n");
        }
        let priority_case = round >= 16;
        let guest_cr8 = cfg!(feature = "guest-cr8-unblock");
        let entry = address(if priority_case && !guest_cr8 {
            ptr::addr_of!(guest_irq_tpr_stopped)
        } else if priority_case {
            ptr::addr_of!(guest_irq_tpr)
        } else {
            ptr::addr_of!(guest_irq_if)
        });
        let ready = address(if priority_case && !guest_cr8 {
            ptr::addr_of!(guest_irq_tpr_stopped_ready)
        } else if priority_case {
            ptr::addr_of!(guest_irq_tpr_ready)
        } else {
            ptr::addr_of!(guest_irq_if_ready)
        });
        unsafe { initialize(control, prepared, caps, state, entry, handler, true, round) };
        let mut frame = GuestRegisters {
            r14: ready,
            ..GuestRegisters::default()
        };
        let mut request = PendingExternalInterrupt::new(VECTOR).unwrap();
        {
            let v = unsafe { &mut *control };
            v.set_virtual_interrupt_tpr(if priority_case { 6 } else { 0 })
                .unwrap();
            v.set_event_intercept(EventIntercept::VirtualInterrupt, true);
            v.arm_external_interrupt(&mut request).unwrap();
            let before = *v.bytes();
            assert_eq!(
                v.arm_external_interrupt(&mut request),
                Err(ExternalInterruptError::RequestNotQueued)
            );
            assert_eq!(v.bytes(), &before);
            let mut conflicting = PendingExternalInterrupt::new(VECTOR + 1).unwrap();
            assert_eq!(
                v.arm_external_interrupt(&mut conflicting),
                Err(ExternalInterruptError::PendingVirtualInterrupt)
            );
            assert_eq!(conflicting.state(), ExternalInterruptState::Queued);
            assert_eq!(v.bytes(), &before);
        }
        // First guest query proves either IF=0 or TPR6 keeps this IRQ pending.
        unsafe { enter(control, &mut frame, state, clock) };
        {
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 0);
            assert_eq!(frame.r12, 0);
            assert_eq!(
                v.observe_external_interrupt_after_exit(&mut request)
                    .unwrap(),
                ExternalInterruptState::Armed
            );
            let flags = u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap());
            assert_eq!(flags & 0x200 != 0, priority_case);
            if priority_case {
                assert_eq!(frame.r15, 6);
            }
            assert_eq!(
                dispatch_query(v, &mut frame, code),
                DispatchOutcome::ResumePrepared
            );
            if priority_case && !guest_cr8 {
                // Default TCG scope: stopped V_TPR update. Keep the actual
                // guest CR8 write as a separate backend crash diagnostic.
                v.set_virtual_interrupt_tpr(4).unwrap();
            }
        }
        // Hardware opens the virtual interrupt window after STI's following
        // INC, or after the selected priority change. No instruction is skipped.
        unsafe { enter(control, &mut frame, state, clock) };
        {
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x64);
            assert_eq!(v.guest_rip(), ready);
            assert!(!v.interrupt_shadow());
            assert_eq!(frame.r12, 0);
            assert_eq!(frame.r13, u64::from(!priority_case));
            assert_eq!(
                v.virtual_interrupt_control() & 0xf,
                if priority_case { 4 } else { 0 }
            );
            assert_eq!(
                v.observe_external_interrupt_after_exit(&mut request)
                    .unwrap(),
                ExternalInterruptState::Armed
            );
            v.set_event_intercept(EventIntercept::VirtualInterrupt, false);
            assert_eq!(v.guest_rip(), ready);
            assert_ne!(v.virtual_interrupt_control() & V_IRQ, 0);
        }
        unsafe { enter(control, &mut frame, state, clock) };
        {
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 0);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.guest_rsp(), 0x9000);
            assert_eq!(
                v.observe_external_interrupt_after_exit(&mut request)
                    .unwrap(),
                ExternalInterruptState::Consumed
            );
            let before = *v.bytes();
            assert_eq!(
                v.observe_external_interrupt_after_exit(&mut request),
                Err(ExternalInterruptError::RequestNotArmed)
            );
            assert_eq!(
                v.arm_external_interrupt(&mut request),
                Err(ExternalInterruptError::RequestNotQueued)
            );
            assert_eq!(v.bytes(), &before);
            assert_eq!(
                dispatch_query(v, &mut frame, code),
                DispatchOutcome::ResumePrepared
            );
        }
        // One more genuine entry, with no re-arm, proves IRETQ continuation
        // reaches STOP without delivering the owned interrupt twice.
        unsafe { enter(control, &mut frame, state, clock) };
        {
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 1);
            assert_eq!(frame.r12, 1);
            assert_eq!(v.guest_rsp(), 0x9000);
            assert_eq!(v.virtual_interrupt_control() & V_IRQ, 0);
            assert_eq!(
                dispatch_query(v, &mut frame, code),
                DispatchOutcome::Stop(StopReason::Requested)
            );
        }
        unsafe { memory::verify_exception_stack() };
    }
    if cfg!(feature = "guest-cr8-unblock") {
        print("PASS guest-external-interrupts=32 if-shadow-guest-cr8 iretq-once\n");
        print("PASS guest-cr8-unblock guest-write\n");
    } else {
        print("PASS guest-external-interrupts=32 if-shadow-stopped-tpr iretq-once\n");
        print("SKIP guest-cr8-unblock backend-locking-gap\n");
    }
    print("PASS guest-vintr-window=32 pending-preserved\n");
    print("PASS guest-interrupt-conflicts-refused\n");

    // Separate equality conformance probe: AMD requires priority > V_TPR.
    // QEMU10.1 ctl_has_irq uses >=; record this without dispatching the IRQ.
    let entry = address(ptr::addr_of!(guest_irq_probe));
    unsafe { initialize(control, prepared, caps, state, entry, handler, true, 32) };
    let mut frame = GuestRegisters::default();
    let mut request = PendingExternalInterrupt::new(VECTOR).unwrap();
    {
        let v = unsafe { &mut *control };
        v.set_virtual_interrupt_tpr(5).unwrap();
        v.set_event_intercept(EventIntercept::VirtualInterrupt, true);
        v.arm_external_interrupt(&mut request).unwrap();
    }
    unsafe { enter(control, &mut frame, state, clock) };
    {
        let v = unsafe { &*control };
        assert_eq!(frame.r12, 0);
        assert_eq!(v.virtual_interrupt_control() & 0xf, 5);
        assert_eq!(
            v.observe_external_interrupt_after_exit(&mut request)
                .unwrap(),
            ExternalInterruptState::Armed
        );
        match v.exit_snapshot().code {
            0x64 => {
                assert_eq!(v.guest_rip(), address(ptr::addr_of!(guest_irq_probe_ready)));
                print("GAP guest-interrupt-tpr-equality tcg-greater-or-equal\n");
            }
            0x81 => {
                assert_eq!(v.guest_rax(), 0);
                print("PASS guest-interrupt-tpr-equality blocked\n");
            }
            _ => panic!("unexpected equality exit"),
        }
    }
    unsafe { memory::verify_exception_stack() };

    // Not-present IRQ gate forces a real #NP while delivering this event.
    // V_IRQ clearing and EXITINTINFO type have known QEMU differences. Neither
    // controls retirement: every valid EXITINTINFO is refused without mutation.
    unsafe { initialize(control, prepared, caps, state, entry, handler, false, 33) };
    let mut frame = GuestRegisters::default();
    let mut request = PendingExternalInterrupt::new(VECTOR).unwrap();
    unsafe {
        (&mut *control)
            .arm_external_interrupt(&mut request)
            .unwrap()
    };
    unsafe { enter(control, &mut frame, state, clock) };
    {
        let v = unsafe { &mut *control };
        assert_eq!(v.exit_snapshot().code, 0x4b);
        let prior = u64::from_le_bytes(v.bytes()[0x088..0x090].try_into().unwrap());
        assert_eq!(prior & 0x8000_00ff, 0x8000_0050);
        assert_eq!(frame.r12, 0);
        let before = *v.bytes();
        assert_eq!(
            v.observe_external_interrupt_after_exit(&mut request),
            Err(ExternalInterruptError::NestedDeliveryUnsupported)
        );
        assert_eq!(request.state(), ExternalInterruptState::Armed);
        assert!(v.reflect_exception().is_err());
        assert!(v.clear_event_injection_after_exit().is_err());
        assert_eq!(v.bytes(), &before);
        if v.virtual_interrupt_control() & V_IRQ != 0 {
            print("GAP guest-interrupt-nested-pending tcg-clears-after-idt\n");
        }
        match (prior >> 8) & 7 {
            0 => (),
            3 => print("GAP guest-interrupt-nested-type tcg-exception-type\n"),
            _ => panic!("unexpected interrupted IRQ type"),
        }
    }
    unsafe { memory::verify_exception_stack() };
    print("PASS guest-interrupt-nested-refused\n");
}

/// The caller owns the stopped guest and backing under run's safety contract.
pub(super) unsafe fn initialize(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    entry: u64,
    handler: u64,
    present: bool,
    round: usize,
) {
    unsafe {
        memory::reset_session();
        state.reset(round);
        memory::install_idt(&[(VECTOR, handler, present)]);
    }
    let guest = GuestStateRequest {
        rip: entry,
        rsp: 0x9000,
        rflags: 2,
        cr0: 0x80010033,
        cr3: prepared.guest_cr3,
        cr4: state.guest_cr4(),
        efer: 0x1500,
        rax: 0,
    }
    .validate_with_xstate(&caps.address_policy(), state.layout())
    .unwrap();
    unsafe { execution::initialize(control, prepared, caps, &guest, Some((0x3000, 4095))) };
}

/// Exclusive CPU/bridge ownership as run. CR8 accesses are CPL0; APM15.21.2.
pub(super) unsafe fn enter(
    control: *mut Vmcb,
    frame: &mut GuestRegisters,
    state: &xstate::State,
    clock: &clock::State,
) {
    let before: u64;
    let after: u64;
    unsafe {
        asm!("mov {}, cr8", out(reg) before, options(nomem, nostack, preserves_flags));
        state.run(control, frame, 0, clock);
        asm!("mov {}, cr8", out(reg) after, options(nomem, nostack, preserves_flags));
    }
    assert_eq!(after, before);
}

fn dispatch_query(v: &mut Vmcb, frame: &mut GuestRegisters, code: &[u8]) -> DispatchOutcome {
    let snapshot = v.exit_snapshot();
    let offset = snapshot.rip.checked_sub(0x1000).unwrap() as usize;
    let instruction = code.get(offset..offset.checked_add(3).unwrap()).unwrap();
    handle_exit_with_instruction(snapshot, v, frame, instruction).unwrap()
}

/// Execute a bounded internal controller through real IRQ entries and IRETQ.
/// # Safety
/// Same exclusive CPU, backing and stopped-VMCB contract as `run`. Timer ticks
/// are supplied test data. The guest does not change CR8 or access APIC registers.
pub unsafe fn run_controller(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    use svmvisor_hypervisor::svm::local_apic::{LocalApic, TickOutcome};
    unsafe extern "C" {
        static guest_apic_loop: u8;
        static guest_apic_ready: u8;
        static guest_apic_handler: u8;
        static guest_apic_eoi: u8;
    }
    let address = |symbol: *const u8| 0x1000 + symbol as u64 - code_start as u64;
    let handler = address(ptr::addr_of!(guest_apic_handler));
    let eoi_pc = address(ptr::addr_of!(guest_apic_eoi));
    let ready = address(ptr::addr_of!(guest_apic_ready));
    for round in 0..16 {
        unsafe {
            initialize(
                control,
                prepared,
                caps,
                state,
                address(ptr::addr_of!(guest_apic_loop)),
                handler,
                true,
                round,
            );
            memory::install_idt(&[(0x50, handler, true), (0x51, handler, true)]);
        }
        let mut frame = GuestRegisters {
            r14: ready,
            ..GuestRegisters::default()
        };
        let mut apic = LocalApic::admit_enabled();
        apic.write_timer_divide(0xb).unwrap();
        apic.write_timer_lvt(0x50).unwrap();
        apic.write_timer_initial(2).unwrap();
        assert_eq!(apic.advance_timer(1), Ok(TickOutcome::Counting));
        assert_eq!(apic.arm(unsafe { &mut *control }), Ok(None));
        assert_eq!(apic.advance_timer(1), Ok(TickOutcome::Queued));
        assert_eq!(apic.arm(unsafe { &mut *control }), Ok(Some(0x50)));
        unsafe { enter(control, &mut frame, state, clock) };
        let v = unsafe { &mut *control };
        assert_eq!(apic.observe(v), Ok(Some(0x50)));
        assert_eq!(frame.r12, 1);
        assert!(apic.in_service(0x50));
        apic.queue(0x51).unwrap();
        assert_eq!(apic.arm(v), Ok(None)); // same class remains behind ISR
        let before = *v.bytes();
        let saved_frame = frame;
        frame.r11 = 0;
        let invalid_frame = frame;
        assert!(!fixture_eoi(&mut apic, v, &mut frame, code, eoi_pc, 0x50));
        assert_eq!(v.bytes(), &before);
        assert_eq!(frame, invalid_frame);
        assert!(apic.in_service(0x50) && apic.pending(0x51));
        frame = saved_frame;
        assert!(fixture_eoi(&mut apic, v, &mut frame, code, eoi_pc, 0x50));
        assert_eq!(apic.processor_priority(), 0);
        assert_eq!(apic.arm(v), Ok(Some(0x51)));
        // The first handler still has IF clear. Its IRETQ permits the next IRQ.
        unsafe { enter(control, &mut frame, state, clock) };
        let v = unsafe { &mut *control };
        assert_eq!(apic.observe(v), Ok(Some(0x51)));
        assert_eq!(frame.r12, 2);
        assert!(fixture_eoi(&mut apic, v, &mut frame, code, eoi_pc, 0x51));
        assert_eq!(apic.arm(v), Ok(None));
        assert_eq!(apic.advance_timer(u64::MAX), Ok(TickOutcome::Stopped));
        for _ in 0..2 {
            unsafe { enter(control, &mut frame, state, clock) };
            let v = unsafe { &mut *control };
            assert_eq!(frame.r12, 2);
            assert_eq!(v.guest_rsp(), 0x9000);
            assert_eq!(
                dispatch_query(v, &mut frame, code),
                DispatchOutcome::ResumePrepared
            );
        }
        unsafe { memory::verify_exception_stack() };
    }
    print("PASS local-apic=16 timer-eoi-priority iretq-once\n");
    print("PASS local-apic-eoi-adapter-refusal\n");
}

// Only this fixture's exact guest EOI query is accepted. Validate before either
// architectural continuation or controller mutation; the ordinary query
// dispatcher validates instruction bytes/RIP and commits its existing ABI.
fn fixture_eoi(
    apic: &mut svmvisor_hypervisor::svm::local_apic::LocalApic,
    v: &mut Vmcb,
    frame: &mut GuestRegisters,
    code: &[u8],
    pc: u64,
    vector: u8,
) -> bool {
    if v.exit_snapshot().code != 0x81
        || v.guest_rip() != pc
        || v.guest_rax() != 0
        || frame.r11 != 0xe01
        || apic.delivery_armed()
        || apic.eoi_target() != Some(vector)
    {
        return false;
    }
    assert_eq!(
        dispatch_query(v, frame, code),
        DispatchOutcome::ResumePrepared
    );
    assert_eq!(apic.eoi(), Ok(Some(vector)));
    true
}
