use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        apic_scheduler::{
            ClockRate, ClockSample, PreemptionOutcome, ScheduleError, ScheduledApic, WaitOutcome,
        },
        ipi::{IpiMailbox, MailboxTarget},
        local_apic::{LocalApic, TickOutcome},
        vmcb::{EventIntercept, InstructionIntercept, Vmcb},
        x2apic::{FIXTURE_APIC_BASE, FixtureApic},
    },
};

// Inert unit-test exit images; actual CPU execution belongs to the DXE harness.
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn sample(ticks: u64) -> ClockSample {
    ClockSample { ticks, cpu: 7 }
}
fn setup(rate: (u32, u32), start: u64) -> (ScheduledApic, Vmcb) {
    let a = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
    let s =
        ScheduledApic::admit(a, ClockRate::new(rate.0, rate.1).unwrap(), sample(start)).unwrap();
    let mut v = Vmcb::new();
    write(&mut v, 0x4d0, 1 << 10);
    write(&mut v, 0x410, 2 << 24);
    write(&mut v, 0x570, 0x202);
    write(&mut v, 0x578, 0x1000);
    v.set_instruction_intercept(InstructionIntercept::Hlt, true);
    (s, v)
}
fn msr(s: &mut ScheduledApic, v: &mut Vmcb, index: u32, value: u64) -> Result<(), ScheduleError> {
    write(v, 0x70, 0x7c);
    write(v, 0x78, 1);
    write(v, 0x5f8, value as u32 as u64);
    let mut f = GuestRegisters {
        rcx: index as u64,
        rdx: value >> 32,
        ..GuestRegisters::default()
    };
    s.handle_msr(v, &mut f, &[0xf, 0x30])
}
fn program(s: &mut ScheduledApic, v: &mut Vmcb, lvt: u32, count: u32, divisor: u32) {
    msr(s, v, 0x838, 0).unwrap();
    msr(s, v, 0x83e, divisor as u64).unwrap();
    msr(s, v, 0x832, lvt as u64).unwrap();
    msr(s, v, 0x838, count as u64).unwrap();
}
fn park(s: &mut ScheduledApic, v: &mut Vmcb) {
    write(v, 0x70, 0x78);
    s.park_hlt(v, &[0xf4]).unwrap();
}

fn mailbox_send(mailbox: &IpiMailbox) {
    let source = FixtureApic::admit_fixed_cpu(LocalApic::admit_enabled(), 0xfee00c00, 1).unwrap();
    let mut scheduler =
        ScheduledApic::admit(source, ClockRate::new(1, 1).unwrap(), sample(0)).unwrap();
    let mut v = Vmcb::new();
    write(&mut v, 0x70, 0x7c);
    write(&mut v, 0x78, 1);
    write(&mut v, 0x578, 0x1000);
    write(&mut v, 0x5f8, 0x50);
    let mut frame = GuestRegisters {
        rcx: 0x830,
        ..GuestRegisters::default()
    };
    let mut route = MailboxTarget::new(mailbox);
    assert_eq!(
        scheduler.handle_msr_with_mailbox(&mut v, &mut frame, &[0x0f, 0x30], &mut route),
        Ok(())
    );
    assert!(route.published());
    assert_eq!(v.guest_rip(), 0x1002);
}

#[test]
fn mailbox_publication_wakes_parked_hlt_through_the_existing_owner() {
    let mailbox = IpiMailbox::new(0);
    let (mut owner, mut v) = setup((1, 1), 0);
    park(&mut owner, &mut v);
    assert_eq!(owner.drain_mailbox(&v, &mailbox), Ok(0));
    assert_eq!(
        owner.poll_halted(&mut v, sample(1)),
        Ok(WaitOutcome::Parked)
    );
    let halted_rip = v.guest_rip();
    mailbox_send(&mailbox);
    assert_eq!(owner.drain_mailbox(&v, &mailbox), Ok(1));
    assert!(owner.is_halted());
    assert_eq!(v.guest_rip(), halted_rip);
    assert_eq!(
        owner.poll_halted(&mut v, sample(2)),
        Ok(WaitOutcome::Ready { vector: 0x50 })
    );
    assert_eq!(v.guest_rip(), halted_rip + 1);
    assert!(!owner.is_halted());
    assert!(!mailbox.pending());
}

#[test]
fn altered_parked_state_cannot_consume_a_mailbox_request() {
    for (offset, poison) in [
        (0x68, 1u64),
        (0x70, 0x60),
        (0x4d0, 0),
        (0x570, 2),
        (0x578, 0x2000),
    ] {
        let mailbox = IpiMailbox::new(0);
        let (mut owner, mut v) = setup((1, 1), 0);
        park(&mut owner, &mut v);
        mailbox_send(&mailbox);
        write(&mut v, offset, poison);
        let bytes = *v.bytes();
        let before = format!("{owner:?}");
        assert!(owner.drain_mailbox(&v, &mailbox).is_err());
        assert!(mailbox.pending());
        assert_eq!(v.bytes(), &bytes);
        assert_eq!(format!("{owner:?}"), before);
    }
}

#[test]
fn shutdown_never_settles_pending_irq_or_charges_clock_from_undefined_state() {
    for flight in [0, 1, 2] {
        for poison in [false, true] {
            let (mut s, mut v) = setup((1, 1), 0);
            program(&mut s, &mut v, 0x50 | (1 << 17), 1, 0xb);
            s.service(&v, sample(1)).unwrap();
            if flight != 0 {
                assert_eq!(s.arm_pending(&mut v), Ok(Some(0x50)));
            }
            write(&mut v, 0x070, 0x7f);
            // Shutdown state cannot establish dispatch even if V_IRQ looks cleared.
            if flight == 2 {
                let control = v.virtual_interrupt_control() & !(1 << 8);
                write(&mut v, 0x060, control);
            }
            // With normal-looking fields only EXITCODE proves shutdown. Keeping
            // this case prevents pending-injection/nested-delivery guards from
            // masking a regression in the actual shutdown check.
            if poison {
                for offset in [0x078, 0x080, 0x088, 0x0a8, 0x578, 0x5d8] {
                    write(&mut v, offset, u64::MAX);
                }
            } else {
                write(&mut v, 0x088, 0);
                write(&mut v, 0x0a8, 0);
            }
            let owner = format!("{s:?}");
            let bytes = *v.bytes();
            assert!(s.settle_after_exit(&mut v, sample(5)).is_err());
            assert!(s.observe_after_exit(&v, sample(5)).is_err());
            assert!(s.service(&v, sample(5)).is_err());
            assert!(s.arm_pending(&mut v).is_err());
            assert_eq!(format!("{s:?}"), owner);
            assert_eq!(v.bytes(), &bytes);
        }
    }
}

#[test]
fn rational_clock_and_apic_divider_phases_determine_exact_deadline() {
    let (mut s, mut v) = setup((2, 3), 100);
    program(&mut s, &mut v, 0x50, 2, 0); // 4 timer-source ticks, 6 clock ticks.
    assert_eq!(s.deadline(), Ok(Some(106)));
    assert_eq!(s.service(&v, sample(102)), Ok(TickOutcome::Counting));
    assert_eq!(s.source_phase(), 1);
    assert_eq!(s.apic().controller().timer_remaining(), 2);
    assert_eq!(s.deadline(), Ok(Some(106)));
    s.service(&v, sample(105)).unwrap();
    assert_eq!(s.apic().controller().timer_remaining(), 1);
    assert_eq!(s.deadline(), Ok(Some(106)));
    assert_eq!(s.service(&v, sample(106)), Ok(TickOutcome::Queued));
    assert_eq!(s.deadline(), Ok(None));
    assert!(s.apic().controller().pending(0x50));
}

#[test]
fn old_configuration_is_serviced_before_reprogram_and_new_phase_starts_at_write() {
    let (mut s, mut v) = setup((1, 3), 0);
    program(&mut s, &mut v, 0x50, 1, 0xb);
    s.service(&v, sample(4)).unwrap();
    assert!(s.apic().controller().pending(0x50));
    assert_eq!(s.source_phase(), 1);
    msr(&mut s, &mut v, 0x838, 2).unwrap();
    assert_eq!(s.source_phase(), 0);
    assert_eq!(s.deadline(), Ok(Some(10)));
    s.service(&v, sample(6)).unwrap();
    assert_eq!(s.apic().controller().timer_remaining(), 2);
    assert_eq!(s.source_phase(), 2);
    msr(&mut s, &mut v, 0x838, 0).unwrap();
    assert_eq!(s.deadline(), Ok(None));
    assert_eq!(s.source_phase(), 0);
    assert!(s.apic().controller().pending(0x50));
}

#[test]
fn service_refusals_keep_epoch_phase_timer_and_vmcb_unchanged() {
    let (mut s, mut v) = setup((3, 2), 10);
    program(&mut s, &mut v, 0x50, 100, 0xb);
    s.service(&v, sample(11)).unwrap();
    let before = format!("{s:?}");
    let bytes = *v.bytes();
    assert_eq!(
        s.service(&v, sample(10)),
        Err(ScheduleError::ClockBackwards)
    );
    assert_eq!(
        s.service(&v, ClockSample { ticks: 12, cpu: 8 }),
        Err(ScheduleError::WrongCpu)
    );
    assert_eq!(
        s.service(&v, sample(u64::MAX)),
        Err(ScheduleError::ClockOverflow)
    );
    assert_eq!(format!("{s:?}"), before);
    assert_eq!(v.bytes(), &bytes);
    for (offset, value) in [(0xa8, 1 << 31), (0x88, 1 << 31), (0x60, 1 << 8), (0x90, 2)] {
        write(&mut v, offset, value);
        let bytes = *v.bytes();
        assert!(s.service(&v, sample(12)).is_err());
        assert_eq!(format!("{s:?}"), before);
        assert_eq!(v.bytes(), &bytes);
        write(&mut v, offset, 0);
    }
    s.service(&v, sample(12)).unwrap();
    assert_eq!(s.apic().controller().timer_remaining(), 97);
}

#[test]
fn refused_instruction_preserves_post_service_not_previous_clock_boundary() {
    let (mut s, mut v) = setup((1, 2), 0);
    program(&mut s, &mut v, 0x50, 10, 0xb);
    s.service(&v, sample(5)).unwrap();
    write(&mut v, 0x70, 0x7c);
    write(&mut v, 0x78, 1);
    write(&mut v, 0x5f8, 1); // active divisor change is outside admission.
    let mut frame = GuestRegisters {
        rcx: 0x83e,
        ..GuestRegisters::default()
    };
    let before = format!("{s:?}");
    let bytes = *v.bytes();
    let saved = frame;
    assert!(s.handle_msr(&mut v, &mut frame, &[0xf, 0x30]).is_err());
    assert_eq!(format!("{s:?}"), before);
    assert_eq!(v.bytes(), &bytes);
    assert_eq!(frame, saved);
    assert_eq!(s.last_sample(), sample(5));
    assert_eq!(s.source_phase(), 1);
    assert_eq!(s.apic().controller().timer_remaining(), 8);
}

#[test]
fn derived_deadline_tracks_masks_svr_cancel_and_base_freeze() {
    let (mut s, mut v) = setup((1, 3), 0);
    program(&mut s, &mut v, 0x50, 10, 0xb);
    s.service(&v, sample(4)).unwrap();
    msr(&mut s, &mut v, 0x832, 0x10050).unwrap();
    assert_eq!(s.deadline(), Ok(None));
    s.service(&v, sample(5)).unwrap();
    msr(&mut s, &mut v, 0x832, 0x50).unwrap();
    assert_eq!(s.deadline(), Ok(Some(30)));
    msr(&mut s, &mut v, 0x1b, FIXTURE_APIC_BASE & !(3 << 10)).unwrap();
    assert_eq!(s.deadline(), Ok(None));
    let count = s.apic().controller().timer_remaining();
    let phase = s.source_phase();
    assert_eq!(s.service(&v, sample(100)), Ok(TickOutcome::Stopped));
    assert_eq!(s.apic().controller().timer_remaining(), count);
    assert_eq!(s.source_phase(), phase);
    msr(&mut s, &mut v, 0x1b, FIXTURE_APIC_BASE & !(1 << 10)).unwrap();
    msr(&mut s, &mut v, 0x1b, FIXTURE_APIC_BASE).unwrap();
    assert_eq!(s.deadline(), Ok(Some(125)));
    msr(&mut s, &mut v, 0x80f, 0xff).unwrap();
    assert_eq!(s.deadline(), Ok(None));
    s.service(&v, sample(130)).unwrap();
    assert!(!s.apic().controller().pending(0x50));
    msr(&mut s, &mut v, 0x80f, 0x1ff).unwrap();
    assert_eq!(s.deadline(), Ok(None)); // forced mask is retained.
}

#[test]
fn deadline_overflow_is_explicit_query_refusal_without_wrapping_or_mutation() {
    assert_eq!(ClockRate::new(0, 1), Err(ScheduleError::InvalidRate));
    assert_eq!(ClockRate::new(1, 0), Err(ScheduleError::InvalidRate));
    let (mut s, mut v) = setup((1, u32::MAX), u64::MAX - 2);
    program(&mut s, &mut v, 0x50, u32::MAX, 0xa);
    let before = format!("{s:?}");
    assert_eq!(s.deadline(), Err(ScheduleError::DeadlineOverflow));
    assert_eq!(format!("{s:?}"), before);
    assert_eq!(s.service(&v, sample(0)), Err(ScheduleError::ClockBackwards));
    assert_eq!(format!("{s:?}"), before);
}

#[test]
fn checked_hlt_consumes_sti_shadow_and_advances_exactly_once_on_ready() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x50, 10, 0xb);
    write(&mut v, 0x68, 1);
    write(&mut v, 0xc8, 0xdead_beef); // stale nRIP is irrelevant.
    write(&mut v, 0x5f8, 0xabcdef);
    let rip = v.guest_rip();
    park(&mut s, &mut v);
    assert!(!v.interrupt_shadow());
    assert_eq!(v.guest_rip(), rip);
    assert_eq!(s.poll_halted(&mut v, sample(9)), Ok(WaitOutcome::Parked));
    assert_eq!(v.guest_rip(), rip);
    assert_eq!(
        s.poll_halted(&mut v, sample(10)),
        Ok(WaitOutcome::Ready { vector: 0x50 })
    );
    assert_eq!(v.guest_rip(), rip + 1);
    assert_eq!(v.guest_rax(), 0xabcdef);
    assert_eq!(
        u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap()),
        0x202
    );
    assert_eq!(
        s.poll_halted(&mut v, sample(11)),
        Err(ScheduleError::NotHalted)
    );
    assert_eq!(v.guest_rip(), rip + 1);
    assert!(s.apic().controller().pending(0x50));
    assert!(!s.apic().controller().in_service(0x50));
}

#[test]
fn hlt_readiness_respects_if_tpr_and_existing_isr_priority() {
    for gate in 0..3 {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 1, 0xb);
        match gate {
            0 => write(&mut v, 0x570, 2),
            1 => msr(&mut s, &mut v, 0x808, 0x50).unwrap(),
            _ => {
                park(&mut s, &mut v);
                s.poll_halted(&mut v, sample(1)).unwrap();
                write(&mut v, 0x70, 0x81);
                let control = v.virtual_interrupt_control();
                write(&mut v, 0x60, control & !(1 << 8));
                assert_eq!(s.observe_after_exit(&v, sample(1)), Ok(Some(0x50)));
                assert!(s.apic().controller().in_service(0x50));
                program(&mut s, &mut v, 0x5f, 1, 0xb);
            }
        }
        park(&mut s, &mut v);
        let rip = v.guest_rip();
        assert_eq!(s.poll_halted(&mut v, sample(10)), Ok(WaitOutcome::Parked));
        assert!(s.is_halted());
        assert_eq!(v.guest_rip(), rip);
        assert_eq!(v.virtual_interrupt_control() & (1 << 8), 0);
    }
}

#[test]
fn masked_disabled_software_disabled_and_cancelled_hlt_remain_parked() {
    for gate in 0..4 {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 1, 0xb);
        match gate {
            0 => msr(&mut s, &mut v, 0x832, 0x10050).unwrap(),
            1 => msr(&mut s, &mut v, 0x80f, 0xff).unwrap(),
            2 => msr(&mut s, &mut v, 0x1b, FIXTURE_APIC_BASE & !(3 << 10)).unwrap(),
            _ => msr(&mut s, &mut v, 0x838, 0).unwrap(),
        }
        park(&mut s, &mut v);
        let rip = v.guest_rip();
        assert_eq!(s.poll_halted(&mut v, sample(10)), Ok(WaitOutcome::Parked));
        assert_eq!(v.guest_rip(), rip);
        assert_eq!(s.deadline(), Ok(None));
        assert!(!s.apic().controller().pending(0x50));
    }
}

#[test]
fn armed_service_refusal_retains_elapsed_until_real_consumption_observation() {
    let (mut s, mut v) = setup((1, 2), 0);
    program(&mut s, &mut v, 0x20050, 2, 0xb);
    park(&mut s, &mut v);
    assert_eq!(
        s.poll_halted(&mut v, sample(4)),
        Ok(WaitOutcome::Ready { vector: 0x50 })
    );
    let before = format!("{s:?}");
    assert_eq!(s.service(&v, sample(7)), Err(ScheduleError::Armed));
    assert_eq!(format!("{s:?}"), before);
    write(&mut v, 0x70, 0x81);
    assert_eq!(s.observe_after_exit(&v, sample(7)), Ok(None));
    assert_eq!(s.last_sample(), sample(4));
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8));
    assert_eq!(s.observe_after_exit(&v, sample(10)), Ok(Some(0x50)));
    assert_eq!(s.last_sample(), sample(10));
    assert_eq!(s.apic().controller().timer_remaining(), 1);
    assert!(s.apic().controller().in_service(0x50));
    assert!(s.apic().controller().pending(0x50));
    msr(&mut s, &mut v, 0x80b, 0).unwrap();
    assert!(!s.apic().controller().in_service(0x50));
}

#[test]
fn bad_clock_or_delivery_evidence_does_not_retire_flight() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x50, 1, 0xb);
    park(&mut s, &mut v);
    s.poll_halted(&mut v, sample(1)).unwrap();
    let before = format!("{s:?}");
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8));
    write(&mut v, 0x70, 0x81);
    assert_eq!(
        s.observe_after_exit(&v, sample(0)),
        Err(ScheduleError::ClockBackwards)
    );
    assert_eq!(format!("{s:?}"), before);
    write(&mut v, 0x88, 1 << 31);
    assert!(s.observe_after_exit(&v, sample(2)).is_err());
    assert_eq!(format!("{s:?}"), before);
    write(&mut v, 0x88, 0);
    assert_eq!(s.observe_after_exit(&v, sample(2)), Ok(Some(0x50)));
}

#[test]
fn unsupported_hlt_evidence_preserves_all_stopped_state() {
    for case in 0..11 {
        let (mut s, mut v) = setup((1, 1), 0);
        write(&mut v, 0x70, 0x78);
        write(&mut v, 0x68, 1);
        match case {
            0 => write(&mut v, 0x70, 0x81),
            1 => write(&mut v, 0x4cb, 3),
            2 => write(&mut v, 0x4d0, 0),
            3 => write(&mut v, 0x578, 0x7fff_ffff_ffff),
            4 => write(&mut v, 0xa8, 1 << 31),
            5 => v.set_instruction_intercept(InstructionIntercept::Hlt, false),
            6 => write(&mut v, 0x60, 1 << 8),
            8 => write(&mut v, 0x570, 0x302),
            9 => write(&mut v, 0x570, 0x10202),
            10 => write(&mut v, 0x560, 0x401),
            _ => (),
        }
        let bytes = *v.bytes();
        let before = format!("{s:?}");
        let instruction: &[u8] = if case == 7 { &[0x66, 0xf4] } else { &[0xf4] };
        assert!(s.park_hlt(&mut v, instruction).is_err());
        assert_eq!(v.bytes(), &bytes);
        assert_eq!(format!("{s:?}"), before);
    }
}

#[test]
fn maximum_rate_components_and_periodic_expirations_use_bounded_arithmetic() {
    let (mut s, mut v) = setup((u32::MAX, u32::MAX - 1), 0);
    program(&mut s, &mut v, 0x20050, 3, 0xb);
    s.service(&v, sample(u32::MAX as u64)).unwrap();
    let expected_ticks = u32::MAX as u64 + 1;
    assert_eq!(
        s.apic().controller().timer_remaining(),
        3 - (expected_ticks % 3) as u32
    );
    assert_eq!(s.source_phase(), 1);
    assert!(s.apic().controller().pending(0x50));
    let expected_delta = (s.apic().controller().timer_remaining() as u128 * (u32::MAX - 1) as u128
        - 1)
    .div_ceil(u32::MAX as u128) as u64;
    assert_eq!(s.deadline(), Ok(Some(u32::MAX as u64 + expected_delta)));
}

#[test]
fn retained_pending_interrupt_cannot_wake_software_or_base_disabled_apic() {
    for base_disabled in [false, true] {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 1, 0xb);
        s.service(&v, sample(1)).unwrap();
        assert!(s.apic().controller().pending(0x50));
        if base_disabled {
            msr(&mut s, &mut v, 0x1b, FIXTURE_APIC_BASE & !(3 << 10)).unwrap();
        } else {
            msr(&mut s, &mut v, 0x80f, 0xff).unwrap();
        }
        park(&mut s, &mut v);
        let rip = v.guest_rip();
        assert_eq!(s.poll_halted(&mut v, sample(50)), Ok(WaitOutcome::Parked));
        assert_eq!(v.guest_rip(), rip);
        assert!(s.apic().controller().pending(0x50));
    }
}

#[test]
fn parked_state_cannot_be_completed_twice_or_mutated_through_bus_escape() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x50, 10, 0xb);
    park(&mut s, &mut v);
    let before = format!("{s:?}");
    let bytes = *v.bytes();
    assert_eq!(
        s.park_hlt(&mut v, &[0xf4]),
        Err(ScheduleError::AlreadyHalted)
    );
    assert_eq!(
        s.handle_msr(&mut v, &mut GuestRegisters::default(), &[0xf, 0x30]),
        Err(ScheduleError::AlreadyHalted)
    );
    assert_eq!(v.bytes(), &bytes);
    let rip = v.guest_rip();
    write(&mut v, 0x578, rip + 1);
    assert_eq!(
        s.poll_halted(&mut v, sample(10)),
        Err(ScheduleError::HaltedStateChanged)
    );
    assert_eq!(format!("{s:?}"), before);
}

fn preemption_exit(v: &mut Vmcb) {
    v.enable_physical_interrupt_virtualization().unwrap();
    write(v, 0x70, 0x60);
    // Undefined exit information and stale nRIP must never drive this path.
    write(v, 0x78, u64::MAX);
    write(v, 0x80, u64::MAX);
    write(v, 0xc8, 0xdead_beef);
}

#[test]
fn physical_interrupt_setup_is_atomic_and_preserves_other_controls() {
    let (_, mut v) = setup((1, 1), 0);
    v.set_event_intercept(EventIntercept::Nmi, true);
    v.set_virtual_interrupt_tpr(6).unwrap();
    let guest = v.bytes()[0x400..].to_vec();
    v.enable_physical_interrupt_virtualization().unwrap();
    assert!(v.event_intercept(EventIntercept::PhysicalInterrupt));
    assert!(v.event_intercept(EventIntercept::Nmi));
    assert!(v.instruction_intercept(InstructionIntercept::Hlt));
    assert_eq!(v.virtual_interrupt_control(), (1 << 24) | 6);
    assert_eq!(&v.bytes()[0x400..], &guest);
    for (offset, bits) in [
        (0x60, 1 << 8),
        (0x60, 1 << 31),
        (0xa8, 1 << 31),
        (0x88, 1 << 31),
        (0x90, 2),
    ] {
        let (_, mut v) = setup((1, 1), 0);
        write(&mut v, offset, bits);
        let before = *v.bytes();
        assert!(v.enable_physical_interrupt_virtualization().is_err());
        assert_eq!(v.bytes(), &before);
    }
}

#[test]
fn preemption_services_deadline_without_advancing_asynchronous_rip() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x50, 10, 0xb);
    preemption_exit(&mut v);
    let before = *v.bytes();
    assert_eq!(
        s.handle_preemption(&mut v, sample(9)),
        Ok(PreemptionOutcome {
            consumed: None,
            armed: None,
        })
    );
    assert_eq!(v.bytes(), &before);
    assert_eq!(
        s.handle_preemption(&mut v, sample(10)),
        Ok(PreemptionOutcome {
            consumed: None,
            armed: Some(0x50),
        })
    );
    assert_eq!(&v.bytes()[0x400..], &before[0x400..]);
    assert_eq!(&v.bytes()[0x68..0xc0], &before[0x68..0xc0]);
    assert_eq!(s.last_sample(), sample(10));
    assert!(s.apic().controller().pending(0x50));
    assert!(!s.apic().controller().in_service(0x50));
}

#[test]
fn preemption_refusals_preserve_vmcb_and_entire_owner() {
    for case in 0..13 {
        let (mut s, mut v) = setup((1, 1), 10);
        program(&mut s, &mut v, 0x50, 100, 0xb);
        preemption_exit(&mut v);
        let mut observed = sample(20);
        match case {
            0 => write(&mut v, 0x70, 0x78),
            1 => v.set_event_intercept(EventIntercept::PhysicalInterrupt, false),
            2 => write(&mut v, 0x60, 0),
            3 => write(&mut v, 0x88, 1 << 31),
            4 => write(&mut v, 0xa8, 1 << 31),
            5 => write(&mut v, 0x60, (1 << 24) | (1 << 8)),
            6 => write(&mut v, 0x60, (1 << 24) | (1 << 31)),
            7 => write(&mut v, 0x90, 2),
            8 => write(&mut v, 0x578, 0x8000_0000_0000),
            9 => write(&mut v, 0x4d0, 0),
            10 => observed.cpu += 1,
            11 => observed.ticks = 9,
            _ => write(&mut v, 0x60, (1 << 24) | 1),
        }
        let owner = format!("{s:?}");
        let bytes = *v.bytes();
        assert!(
            s.handle_preemption(&mut v, observed).is_err(),
            "case {case}"
        );
        assert_eq!(format!("{s:?}"), owner, "case {case}");
        assert_eq!(v.bytes(), &bytes, "case {case}");
    }
    let (mut s, mut v) = setup((u32::MAX, 1), 0);
    program(&mut s, &mut v, 0x50, 100, 0xb);
    preemption_exit(&mut v);
    let owner = format!("{s:?}");
    let bytes = *v.bytes();
    assert_eq!(
        s.handle_preemption(&mut v, sample(u64::MAX)),
        Err(ScheduleError::ClockOverflow)
    );
    assert_eq!(format!("{s:?}"), owner);
    assert_eq!(v.bytes(), &bytes);
}

#[test]
fn preemption_if_shadow_and_priority_gates_retain_pending_clock_progress() {
    for gate in 0..3 {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 10, 0xb);
        if gate == 2 {
            msr(&mut s, &mut v, 0x808, 0x50).unwrap();
        }
        preemption_exit(&mut v);
        if gate == 0 {
            write(&mut v, 0x570, 2);
        }
        if gate == 1 {
            write(&mut v, 0x68, 1);
        }
        let before = *v.bytes();
        assert_eq!(
            s.handle_preemption(&mut v, sample(10)),
            Ok(PreemptionOutcome {
                consumed: None,
                armed: None,
            })
        );
        assert_eq!(v.bytes(), &before);
        assert!(s.apic().controller().pending(0x50));
        assert_eq!(s.last_sample(), sample(10));
        assert!(!s.apic().controller().delivery_armed());
        // Inert model of subsequent guest execution or a checked TPR write.
        if gate == 0 {
            write(&mut v, 0x570, 0x202);
        }
        if gate == 1 {
            write(&mut v, 0x68, 0);
        }
        if gate == 2 {
            msr(&mut s, &mut v, 0x808, 0).unwrap();
        }
        write(&mut v, 0x70, 0x60);
        let rip = v.guest_rip();
        assert_eq!(
            s.handle_preemption(&mut v, sample(20)).unwrap().armed,
            Some(0x50)
        );
        assert_eq!(v.guest_rip(), rip);
    }
}

#[test]
fn preemption_retains_armed_flight_and_observes_consumption_before_reexpiration() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x20050, 10, 0xb);
    preemption_exit(&mut v);
    s.handle_preemption(&mut v, sample(10)).unwrap();
    let owner = format!("{s:?}");
    let bytes = *v.bytes();
    assert_eq!(
        s.handle_preemption(&mut v, sample(25)),
        Ok(PreemptionOutcome {
            consumed: None,
            armed: None,
        })
    );
    assert_eq!(format!("{s:?}"), owner);
    assert_eq!(v.bytes(), &bytes);
    write(&mut v, 0x88, 1 << 31);
    assert!(s.handle_preemption(&mut v, sample(25)).is_err());
    assert_eq!(format!("{s:?}"), owner);
    write(&mut v, 0x88, 0);
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8));
    assert_eq!(
        s.handle_preemption(&mut v, sample(25)),
        Ok(PreemptionOutcome {
            consumed: Some(0x50),
            armed: None,
        })
    );
    assert_eq!(s.last_sample(), sample(25));
    assert_eq!(s.apic().controller().timer_remaining(), 5);
    assert!(s.apic().controller().in_service(0x50));
    assert!(s.apic().controller().pending(0x50));
    msr(&mut s, &mut v, 0x80b, 0).unwrap();
    write(&mut v, 0x70, 0x60);
    assert_eq!(
        s.handle_preemption(&mut v, sample(26)).unwrap().armed,
        Some(0x50)
    );
}

#[test]
fn preemption_mask_base_svr_and_cancellation_preserve_existing_policy() {
    for gate in 0..4 {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 10, 0xb);
        match gate {
            0 => msr(&mut s, &mut v, 0x832, 0x10050).unwrap(),
            1 => msr(&mut s, &mut v, 0x80f, 0xff).unwrap(),
            2 => msr(&mut s, &mut v, 0x1b, FIXTURE_APIC_BASE & !(3 << 10)).unwrap(),
            _ => msr(&mut s, &mut v, 0x838, 0).unwrap(),
        }
        preemption_exit(&mut v);
        let before = *v.bytes();
        assert_eq!(s.handle_preemption(&mut v, sample(20)).unwrap().armed, None);
        assert_eq!(v.bytes(), &before);
        assert!(!s.apic().controller().pending(0x50));
        assert_eq!(s.last_sample(), sample(20));
        assert_eq!(
            s.apic().controller().timer_remaining(),
            if gate == 2 { 10 } else { 0 }
        );
    }
}

#[test]
fn preemption_cannot_operate_on_a_parked_hlt_owner() {
    let (mut s, mut v) = setup((1, 1), 0);
    v.enable_physical_interrupt_virtualization().unwrap();
    park(&mut s, &mut v);
    write(&mut v, 0x70, 0x60);
    let before = format!("{s:?}");
    let bytes = *v.bytes();
    assert_eq!(
        s.handle_preemption(&mut v, sample(20)),
        Err(ScheduleError::AlreadyHalted)
    );
    assert_eq!(format!("{s:?}"), before);
    assert_eq!(v.bytes(), &bytes);
}

#[test]
fn blocked_irq_survives_fault_reflection_then_consumes_once_after_handler_return() {
    for (code, error, address) in [(0x46, 0, 0), (0x4d, 0x1234, 0), (0x4e, 0x15, 0xdead_0000)] {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 10, 0xb);
        write(&mut v, 0x570, 2); // IF=0; hardware will retain V_IRQ.
        s.service(&v, sample(10)).unwrap();
        assert_eq!(s.arm_pending(&mut v), Ok(Some(0x50)));
        write(&mut v, 0x70, code);
        write(&mut v, 0x78, error);
        write(&mut v, 0x80, address);
        let guest = v.bytes()[0x400..].to_vec();
        let settled = s.settle_after_exit(&mut v, sample(11)).unwrap();
        assert_eq!(settled.consumed, None);
        assert_eq!(settled.deferred, Some(0x50));
        assert!(s.apic().controller().pending(0x50));
        assert!(!s.apic().controller().in_service(0x50));
        assert!(!s.apic().controller().delivery_armed());
        assert_eq!(&v.bytes()[0x400..], &guest);
        let rip = v.guest_rip();
        let fault = v.reflect_exception().unwrap();
        assert_eq!(fault.vector() as u64, code - 0x40);
        assert_eq!(v.guest_rip(), rip);
        if code == 0x4e {
            assert_eq!(v.guest_cr2(), address);
        }
        let owner = format!("{s:?}");
        let bytes = *v.bytes();
        assert!(s.arm_pending(&mut v).is_err());
        assert!(s.settle_after_exit(&mut v, sample(12)).is_err());
        assert_eq!(format!("{s:?}"), owner);
        assert_eq!(v.bytes(), &bytes);

        // Model an actual handler checkpoint, then a subsequent IRETQ/entry.
        // Only the DXE fixture executes those instructions on a CPU.
        write(&mut v, 0x70, 0x81);
        v.clear_event_injection_after_exit().unwrap();
        assert_eq!(
            s.settle_after_exit(&mut v, sample(12)).unwrap().consumed,
            None
        );
        assert_eq!(s.arm_pending(&mut v), Ok(Some(0x50)));
        write(&mut v, 0x570, 0x202);
        let control = v.virtual_interrupt_control();
        write(&mut v, 0x60, control & !(1 << 8));
        assert_eq!(
            s.settle_after_exit(&mut v, sample(13)).unwrap().consumed,
            Some(0x50)
        );
        assert!(!s.apic().controller().pending(0x50));
        assert!(s.apic().controller().in_service(0x50));
        assert_eq!(
            s.settle_after_exit(&mut v, sample(13)).unwrap().consumed,
            None
        );
        msr(&mut s, &mut v, 0x80b, 0).unwrap();
        assert!(!s.apic().controller().in_service(0x50));
        assert_eq!(s.arm_pending(&mut v), Ok(None));
    }
}

#[test]
fn settlement_refusals_preserve_armed_owner_pending_irq_and_vmcb() {
    for case in 0..12 {
        let (mut s, mut v) = setup((1, 1), 10);
        program(&mut s, &mut v, 0x20050, 10, 0xb);
        s.service(&v, sample(20)).unwrap();
        s.arm_pending(&mut v).unwrap();
        write(&mut v, 0x70, 0x46);
        let mut clock = sample(30);
        let control = v.virtual_interrupt_control();
        match case {
            0 => write(&mut v, 0x70, u64::MAX),
            1 => write(&mut v, 0x88, 1 << 31),
            2 => write(&mut v, 0xa8, 1 << 31),
            3 => write(&mut v, 0x60, control | (1 << 31)),
            4 => write(&mut v, 0x90, 2),
            5 => write(&mut v, 0x60, control ^ (1 << 32)),
            6 => write(&mut v, 0x60, control | 1),
            7 => clock.cpu += 1,
            8 => clock.ticks = 19,
            9 => {
                write(&mut v, 0x70, 0x64);
                write(&mut v, 0x60, control & !(1 << 8));
            }
            10 => {
                write(&mut v, 0x88, 1 << 31);
                write(&mut v, 0x60, control & !(1 << 8));
            }
            _ => {
                write(&mut v, 0x70, u64::MAX);
                write(&mut v, 0x60, control & !(1 << 8));
            }
        }
        let owner = format!("{s:?}");
        let bytes = *v.bytes();
        assert!(s.settle_after_exit(&mut v, clock).is_err(), "case {case}");
        assert_eq!(format!("{s:?}"), owner, "case {case}");
        assert_eq!(v.bytes(), &bytes, "case {case}");
    }
}

#[test]
fn settlement_of_blocked_periodic_irq_charges_elapsed_time_and_allows_cancellation() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x20050, 10, 0xb);
    write(&mut v, 0x570, 2);
    s.service(&v, sample(10)).unwrap();
    s.arm_pending(&mut v).unwrap();
    write(&mut v, 0x70, 0x7c);
    assert_eq!(
        s.settle_after_exit(&mut v, sample(35)).unwrap().deferred,
        Some(0x50)
    );
    assert_eq!(s.last_sample(), sample(35));
    assert_eq!(s.apic().controller().timer_remaining(), 5);
    assert!(s.apic().controller().pending(0x50));
    assert!(!s.apic().controller().in_service(0x50));
    msr(&mut s, &mut v, 0x838, 0).unwrap();
    assert_eq!(s.deadline(), Ok(None));
    // Cancellation stops future edges; the already accepted IRR bit remains.
    assert_eq!(s.arm_pending(&mut v), Ok(Some(0x50)));
}

#[test]
fn fault_in_irq_handler_preserves_isr_and_second_pending_timer_until_eoi() {
    let (mut s, mut v) = setup((1, 1), 0);
    program(&mut s, &mut v, 0x20050, 10, 0xb);
    s.service(&v, sample(10)).unwrap();
    s.arm_pending(&mut v).unwrap();
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8));
    write(&mut v, 0x70, 0x46); // supported fault after IRQ dispatch, EXITINTINFO invalid.
    write(&mut v, 0x570, 2);
    assert_eq!(
        s.settle_after_exit(&mut v, sample(20)).unwrap().consumed,
        Some(0x50)
    );
    assert!(s.apic().controller().pending(0x50));
    assert!(s.apic().controller().in_service(0x50));
    v.reflect_exception().unwrap();
    write(&mut v, 0x70, 0x81);
    v.clear_event_injection_after_exit().unwrap();
    s.settle_after_exit(&mut v, sample(21)).unwrap();
    assert_eq!(s.arm_pending(&mut v), Ok(None)); // PPR includes existing ISR.
    assert_eq!(s.apic().controller().processor_priority(), 0x50);
    msr(&mut s, &mut v, 0x80b, 0).unwrap();
    assert_eq!(s.arm_pending(&mut v), Ok(Some(0x50)));
}

#[test]
fn fault_retirement_refuses_invalid_entry_and_interrupted_delivery_before_settlement() {
    for (offset, value) in [(0x70, u64::MAX), (0x88, 1 << 31)] {
        let (mut s, mut v) = setup((1, 1), 0);
        program(&mut s, &mut v, 0x50, 10, 0xb);
        s.service(&v, sample(10)).unwrap();
        write(&mut v, 0x70, 0x46);
        v.reflect_exception().unwrap();
        write(&mut v, offset, value);
        let owner = format!("{s:?}");
        let bytes = *v.bytes();
        assert!(v.clear_event_injection_after_exit().is_err());
        assert!(s.settle_after_exit(&mut v, sample(11)).is_err());
        assert_eq!(format!("{s:?}"), owner);
        assert_eq!(v.bytes(), &bytes);
        assert!(s.apic().controller().pending(0x50));
    }
}

#[test]
fn settlement_cannot_resume_a_parked_hlt_or_accept_unowned_virq() {
    let (mut s, mut v) = setup((1, 1), 0);
    park(&mut s, &mut v);
    let owner = format!("{s:?}");
    let bytes = *v.bytes();
    assert_eq!(
        s.settle_after_exit(&mut v, sample(1)),
        Err(ScheduleError::AlreadyHalted)
    );
    assert_eq!(format!("{s:?}"), owner);
    assert_eq!(v.bytes(), &bytes);
    let (mut s, mut v) = setup((1, 1), 0);
    write(&mut v, 0x60, (1 << 24) | (1 << 8));
    let owner = format!("{s:?}");
    let bytes = *v.bytes();
    assert!(s.settle_after_exit(&mut v, sample(1)).is_err());
    assert_eq!(format!("{s:?}"), owner);
    assert_eq!(v.bytes(), &bytes);
}
