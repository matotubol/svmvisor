use svmvisor_hypervisor::svm::{
    local_apic::{Error, LocalApic, TickOutcome},
    vmcb::Vmcb,
};
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn consumed(a: &mut LocalApic, v: &mut Vmcb, vector: u8) {
    assert_eq!(a.arm(v), Ok(Some(vector)));
    write(v, 0x70, 0x81);
    write(v, 0x60, v.virtual_interrupt_control() & !(1 << 8));
    assert_eq!(a.observe(v), Ok(Some(vector)));
}
#[test]
fn ppr_order_eoi_and_same_class_pending() {
    let (mut a, mut v) = (LocalApic::admit_enabled(), Vmcb::new());
    a.queue(0x50).unwrap();
    consumed(&mut a, &mut v, 0x50);
    a.queue(0x5f).unwrap();
    assert_eq!(a.arm(&mut v), Ok(None));
    a.set_task_priority(0x5b).unwrap();
    assert_eq!(a.processor_priority(), 0x5b);
    a.set_task_priority(0x20).unwrap();
    assert_eq!(a.processor_priority(), 0x50);
    a.set_task_priority(0).unwrap();
    a.queue(0x70).unwrap();
    consumed(&mut a, &mut v, 0x70);
    assert_eq!(a.eoi(), Ok(Some(0x70)));
    assert!(a.in_service(0x50));
    assert_eq!(a.eoi(), Ok(Some(0x50)));
    assert!(a.pending(0x5f));
    consumed(&mut a, &mut v, 0x5f);
    assert_eq!(a.eoi(), Ok(Some(0x5f)));
    assert_eq!(a.eoi(), Ok(None));
}
#[test]
fn armed_and_invalid_exit_preserve_ownership() {
    let (mut a, mut v) = (LocalApic::admit_enabled(), Vmcb::new());
    a.queue(0x50).unwrap();
    a.write_timer_initial(0).unwrap();
    a.write_timer_divide(0xb).unwrap();
    a.write_timer_lvt(0x60).unwrap();
    a.write_timer_initial(2).unwrap();
    a.arm(&mut v).unwrap();
    let before = format!("{a:?}");
    let bytes = *v.bytes();
    assert_eq!(a.queue(0x70), Err(Error::Armed));
    assert_eq!(a.eoi(), Err(Error::Armed));
    assert_eq!(a.advance_timer(9), Err(Error::Armed));
    assert_eq!(
        a.write_timer_lvt(a.timer_lvt() | (1 << 16)),
        Err(Error::Armed)
    );
    assert_eq!(a.write_timer_initial(0), Err(Error::Armed));
    assert_eq!(a.set_task_priority(4), Err(Error::Armed));
    assert_eq!(a.arm(&mut v), Err(Error::Armed));
    assert_eq!(v.bytes(), &bytes);
    assert_eq!(format!("{a:?}"), before);
    write(&mut v, 0x70, 0x81);
    assert_eq!(a.observe(&v), Ok(None));
    write(&mut v, 0x88, 1 << 31);
    assert!(a.observe(&v).is_err());
    assert_eq!(format!("{a:?}"), before);
    write(&mut v, 0x88, 0);
    write(&mut v, 0x70, u64::MAX);
    assert!(a.observe(&v).is_err());
    assert_eq!(format!("{a:?}"), before);
}
#[test]
fn refusal_does_not_modify_vmcb_or_controller() {
    let (mut a, mut v) = (LocalApic::admit_enabled(), Vmcb::new());
    assert!(a.queue(31).is_err());
    a.queue(0x50).unwrap();
    a.set_task_priority(0x20).unwrap();
    let bytes = *v.bytes();
    assert_eq!(a.arm(&mut v), Err(Error::TaskPriorityMismatch));
    assert_eq!(v.bytes(), &bytes);
    a.set_task_priority(0).unwrap();
    write(&mut v, 0xa8, 1 << 31);
    let before = format!("{a:?}");
    let bytes = *v.bytes();
    assert!(a.arm(&mut v).is_err());
    assert_eq!(v.bytes(), &bytes);
    assert_eq!(format!("{a:?}"), before);
    write(&mut v, 0xa8, 0);
    a.arm(&mut v).unwrap();
    let ctl = v.virtual_interrupt_control();
    write(&mut v, 0x60, ctl | 1);
    let before = format!("{a:?}");
    assert_eq!(a.observe(&v), Err(Error::TaskPriorityMismatch));
    assert_eq!(format!("{a:?}"), before);
}
#[test]
fn one_shot_masking_cancellation_and_coalescing() {
    let mut a = LocalApic::admit_enabled();
    a.write_timer_initial(0).unwrap();
    a.write_timer_divide(0xb).unwrap();
    a.write_timer_lvt(0x50 | (1 << 16)).unwrap();
    a.write_timer_initial(3).unwrap();
    assert_eq!(a.advance_timer(0), Ok(TickOutcome::Counting));
    assert_eq!(a.timer_remaining(), 3);
    assert_eq!(a.advance_timer(2), Ok(TickOutcome::Counting));
    assert_eq!(a.timer_remaining(), 1);
    assert_eq!(a.advance_timer(1), Ok(TickOutcome::MaskedExpiration));
    a.write_timer_lvt(a.timer_lvt() & !(1 << 16)).unwrap();
    assert_eq!(a.advance_timer(u64::MAX), Ok(TickOutcome::Stopped));
    assert!(!a.pending(0x50));
    a.write_timer_initial(0).unwrap();
    a.write_timer_divide(0xb).unwrap();
    a.write_timer_lvt(0x50).unwrap();
    a.write_timer_initial(2).unwrap();
    assert_eq!(a.advance_timer(9), Ok(TickOutcome::Queued));
    a.write_timer_lvt(a.timer_lvt() | (1 << 16)).unwrap();
    assert!(a.pending(0x50));
    assert!(!a.queue(0x50).unwrap());
    a.write_timer_initial(0).unwrap();
    a.write_timer_divide(0xb).unwrap();
    a.write_timer_lvt(0x50).unwrap();
    a.write_timer_initial(2).unwrap();
    assert_eq!(a.advance_timer(2), Ok(TickOutcome::Coalesced));
    a.write_timer_initial(0).unwrap();
    a.write_timer_divide(0xb).unwrap();
    a.write_timer_lvt(0x60).unwrap();
    a.write_timer_initial(1).unwrap();
    a.write_timer_initial(0).unwrap();
    a.write_timer_divide(0xb).unwrap();
    a.write_timer_lvt(0x60).unwrap();
    a.write_timer_initial(0).unwrap();
    assert_eq!(a.advance_timer(9), Ok(TickOutcome::Stopped));
    assert!(a.pending(0x50));
    assert!(!a.pending(0x60));
}

#[test]
fn pending_order_crosses_bitmap_words_and_same_vector_redelivers_once() {
    let (mut a, mut v) = (LocalApic::admit_enabled(), Vmcb::new());
    for vector in [0x20, 0x80, 0xff, 0x50] {
        a.queue(vector).unwrap();
    }
    consumed(&mut a, &mut v, 0xff);
    assert!(a.queue(0xff).unwrap());
    assert!(!a.queue(0xff).unwrap());
    assert_eq!(a.arm(&mut v), Ok(None));
    assert_eq!(a.eoi(), Ok(Some(0xff)));
    consumed(&mut a, &mut v, 0xff);
    assert_eq!(a.eoi(), Ok(Some(0xff)));
    assert!(!a.pending(0xff));
    for vector in [0x80, 0x50, 0x20] {
        consumed(&mut a, &mut v, vector);
        assert_eq!(a.eoi(), Ok(Some(vector)));
    }
    assert_eq!(a.arm(&mut v), Ok(None));
}

#[test]
fn software_disable_holds_interrupts_forces_masks_and_gates_every_input() {
    let (mut a, mut v) = (LocalApic::admit_enabled(), Vmcb::new());
    a.queue(0x50).unwrap();
    consumed(&mut a, &mut v, 0x50);
    a.queue(0x70).unwrap();
    a.write_timer_lvt(0x60 | (1 << 17)).unwrap();
    a.write_timer_initial(3).unwrap();
    a.write_spurious_vector(0x22f).unwrap();
    assert!(!a.software_enabled());
    assert_eq!(a.spurious_vector_register(), 0x22f);
    assert!(a.pending(0x70) && a.in_service(0x50));
    assert_eq!(a.timer_remaining(), 3);
    assert_eq!(a.timer_lvt(), 0x30060);
    let before = format!("{a:?}");
    assert_eq!(a.queue(0x80), Err(Error::SoftwareDisabled));
    assert_eq!(a.arm(&mut v), Ok(None));
    assert_eq!(format!("{a:?}"), before);
    a.write_timer_lvt(0x20060).unwrap();
    assert_eq!(a.timer_lvt(), 0x30060);
    assert_eq!(a.advance_timer(6), Ok(TickOutcome::MaskedExpiration));
    assert_eq!(a.timer_remaining(), 3);
    assert!(!a.pending(0x60));
    a.write_spurious_vector(0x32f).unwrap();
    assert_eq!(a.timer_lvt(), 0x30060);
    a.write_timer_lvt(0x20060).unwrap();
    assert_eq!(a.advance_timer(6), Ok(TickOutcome::Queued));
    assert_eq!(a.eoi(), Ok(Some(0x50)));
    consumed(&mut a, &mut v, 0x70);
    assert!(a.pending(0x60));
}

#[test]
fn all_dividers_preserve_partial_ticks_and_periodic_reload_boundaries() {
    for (encoding, divisor) in [
        (0, 2),
        (1, 4),
        (2, 8),
        (3, 16),
        (8, 32),
        (9, 64),
        (10, 128),
        (11, 1),
    ] {
        let mut a = LocalApic::admit_enabled();
        a.write_timer_divide(encoding).unwrap();
        a.write_timer_lvt(0x20050).unwrap();
        a.write_timer_initial(3).unwrap();
        assert_eq!(a.advance_timer(divisor - 1), Ok(TickOutcome::Counting));
        assert_eq!(a.timer_remaining(), 3);
        assert_eq!(a.advance_timer(1), Ok(TickOutcome::Counting));
        assert_eq!(a.timer_remaining(), 2);
        assert_eq!(a.advance_timer(2 * divisor), Ok(TickOutcome::Queued));
        assert_eq!(a.timer_remaining(), 3);
        assert_eq!(a.advance_timer(7 * divisor), Ok(TickOutcome::Coalesced));
        assert_eq!(a.timer_remaining(), 2);
        assert_eq!(a.timer_initial(), 3);
    }
}

#[test]
fn huge_tick_delta_with_nonzero_phase_is_bounded_and_exact() {
    let mut a = LocalApic::admit_enabled();
    a.write_timer_divide(10).unwrap();
    a.write_timer_lvt(0x20050).unwrap();
    a.write_timer_initial(u32::MAX).unwrap();
    a.advance_timer(127).unwrap();
    assert_eq!(a.advance_timer(u64::MAX), Ok(TickOutcome::Queued));
    let decrements = ((u64::MAX as u128 + 127) / 128) as u64;
    let expected = u32::MAX - (decrements % u32::MAX as u64) as u32;
    assert_eq!(a.timer_remaining(), expected);
    assert_eq!(a.advance_timer(2), Ok(TickOutcome::Counting));
    assert_eq!(a.timer_remaining(), expected - 1);
}

#[test]
fn timer_reprogramming_refusals_and_reload_cancel_are_transactional() {
    let mut a = LocalApic::admit_enabled();
    a.write_timer_lvt(0x50).unwrap();
    a.write_timer_initial(5).unwrap();
    a.advance_timer(1).unwrap();
    let before = format!("{a:?}");
    for value in [0x51, 0x20050] {
        assert_eq!(a.write_timer_lvt(value), Err(Error::TimerRunning));
        assert_eq!(format!("{a:?}"), before);
    }
    assert_eq!(a.write_timer_divide(1), Err(Error::TimerRunning));
    assert_eq!(a.write_timer_divide(0), Ok(()));
    assert_eq!(format!("{a:?}"), before);
    a.write_timer_lvt(0x10050).unwrap();
    assert_eq!(a.advance_timer(1), Ok(TickOutcome::Counting));
    assert_eq!(a.timer_remaining(), 4);
    a.write_timer_initial(2).unwrap();
    a.advance_timer(1).unwrap();
    assert_eq!(a.timer_remaining(), 2);
    a.write_timer_initial(0).unwrap();
    assert_eq!(a.advance_timer(u64::MAX), Ok(TickOutcome::Stopped));
    a.write_timer_lvt(0x10000).unwrap();
    assert_eq!(a.write_timer_lvt(16), Err(Error::UnsupportedTimerVector));
    assert_eq!(a.write_timer_lvt(0), Err(Error::UnsupportedTimerVector));
    a.write_spurious_vector(0).unwrap();
    a.write_timer_lvt(16).unwrap();
    assert_eq!(a.timer_lvt(), 0x10010);
}

#[test]
fn new_register_mutations_refuse_armed_and_invalid_operands_unchanged() {
    let (mut a, mut v) = (LocalApic::admit_enabled(), Vmcb::new());
    let before = format!("{a:?}");
    assert_eq!(
        a.write_spurious_vector(1 << 12),
        Err(Error::ReservedRegisterBits)
    );
    assert_eq!(a.write_timer_lvt(1 << 18), Err(Error::ReservedRegisterBits));
    assert_eq!(a.write_timer_lvt(1 << 12), Err(Error::ReadOnlyTimerStatus));
    assert_eq!(a.write_timer_divide(4), Err(Error::ReservedRegisterBits));
    assert_eq!(format!("{a:?}"), before);
    a.queue(0x50).unwrap();
    a.arm(&mut v).unwrap();
    let before = format!("{a:?}");
    assert_eq!(a.write_spurious_vector(0), Err(Error::Armed));
    assert_eq!(a.write_timer_lvt(0x10050), Err(Error::Armed));
    assert_eq!(a.write_timer_initial(1), Err(Error::Armed));
    assert_eq!(a.write_timer_divide(11), Err(Error::Armed));
    assert_eq!(format!("{a:?}"), before);
}
