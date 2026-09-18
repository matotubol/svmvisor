use core::ptr;

use crate::{
    host::resident::runtime::{
        INITIAL_STATE,
        exit::{check_exit_event, record_unexplained_stop},
    },
    svm::vmcb::Vmcb,
};

#[test]
fn interrupted_fault_refusal_preserves_request_and_records_terminal_reason() {
    for (code, interrupted) in [(0x400, 0x8000_0b0d_u64), (u64::MAX, 0), (0x7f, 0)] {
        let mut vmcb = Vmcb::new();
        for (offset, value) in [(0x70, code), (0x88, interrupted), (0xa8, 0x8000_0b0d)] {
            unsafe {
                ptr::copy_nonoverlapping(
                    value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                    8,
                );
            }
        }
        let before = *vmcb.bytes();
        let mut state = INITIAL_STATE;
        state.pending_fault = true;
        assert!(!check_exit_event(&mut state, &mut vmcb));
        assert!(state.pending_fault && state.stopped_valid);
        assert_eq!(
            (state.stopped, state.stopped_info1, state.stopped_info2),
            (code, if code == 0x400 { 0xf10c } else { 0xf110 }, interrupted)
        );
        assert_eq!(*vmcb.bytes(), before);
    }
    let mut vmcb = Vmcb::new();
    let mut state = INITIAL_STATE;
    state.pending_fault = true;
    assert!(check_exit_event(&mut state, &mut vmcb));
    assert!(!state.pending_fault && !state.stopped_valid);
}

#[test]
fn interrupted_delivery_is_reinjected_through_eventinj_before_resume() {
    // An interrupted external interrupt (TYPE 0, vector 51h) on an INTR or
    // AVIC exit completes by re-injection, not by an unchanged retry.
    for code in [0x400_u64, 0x60, 0x61, 0x401] {
        let mut vmcb = Vmcb::new();
        let interrupted = 0x8000_0051_u64;
        for (offset, value) in
            [(0x70, code), (0x88, interrupted), (0x578, 0x1234), (0xc0, u64::from(u32::MAX))]
        {
            unsafe {
                ptr::copy_nonoverlapping(
                    value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                    8,
                );
            }
        }
        let mut state = INITIAL_STATE;
        assert!(check_exit_event(&mut state, &mut vmcb));
        assert!(!state.pending_fault && !state.stopped_valid);
        // EXITINTINFO is retained; EVENTINJ now carries the same event and
        // the clean bits are cleared.
        assert_eq!(vmcb.event_injection(), interrupted);
        assert_eq!(u64::from_le_bytes(vmcb.bytes()[0x88..0x90].try_into().unwrap()), interrupted);
        assert_eq!(vmcb.bytes()[0xc0..0xc4], [0; 4]);
    }
}

#[test]
fn software_interrupt_and_conflicting_delivery_stay_terminal() {
    // TYPE 4 (INTn) is not re-injectable and stays a terminal stop.
    let mut vmcb = Vmcb::new();
    for (offset, value) in [(0x70, 0x400u64), (0x88, 0x8000_0451), (0x578, 0x1234)] {
        unsafe {
            ptr::copy_nonoverlapping(
                value.to_le_bytes().as_ptr(),
                (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                8,
            );
        }
    }
    let before = *vmcb.bytes();
    let mut state = INITIAL_STATE;
    assert!(!check_exit_event(&mut state, &mut vmcb));
    assert_eq!(
        (state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
        (0x400, 0x1234, 0xf10f, 0x8000_0451)
    );
    assert_eq!(*vmcb.bytes(), before);
    // A different pending EVENTINJ conflicts with re-injection: terminal.
    let mut vmcb = Vmcb::new();
    for (offset, value) in
        [(0x70, 0x400u64), (0x88, 0x8000_0051), (0xa8, (1 << 31) | (3 << 8) | 13), (0x578, 0x1234)]
    {
        unsafe {
            ptr::copy_nonoverlapping(
                value.to_le_bytes().as_ptr(),
                (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                8,
            );
        }
    }
    let before = *vmcb.bytes();
    let mut state = INITIAL_STATE;
    assert!(!check_exit_event(&mut state, &mut vmcb));
    assert_eq!(
        (state.stopped, state.stopped_info1, state.stopped_info2),
        (0x400, 0xf112, 0x8000_0051)
    );
    assert_eq!(*vmcb.bytes(), before);
}

#[test]
fn shutdown_and_invalid_entry_do_not_interpret_poisoned_saved_event_or_rip() {
    for code in [0x7f_u64, u64::MAX] {
        let mut vmcb = Vmcb::new();
        for (offset, value) in [(0x70, code), (0x88, u64::MAX), (0x578, u64::MAX)] {
            unsafe {
                ptr::copy_nonoverlapping(
                    value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                    8,
                );
            }
        }
        let before = *vmcb.bytes();
        let mut state = INITIAL_STATE;
        assert!(!check_exit_event(&mut state, &mut vmcb));
        assert_eq!(
            (state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
            (code, 0, 0xf110, 0)
        );
        assert!(state.stopped_valid);
        assert_eq!(*vmcb.bytes(), before);
    }
}

#[test]
fn unexplained_refusal_records_exit_without_changing_guest_or_existing_reason() {
    let vmcb = Vmcb::new();
    let before = *vmcb.bytes();
    let exit = vmcb.exit_snapshot();
    let mut state = INITIAL_STATE;
    record_unexplained_stop(&mut state, true, exit);
    assert!(!state.stopped_valid);
    record_unexplained_stop(&mut state, false, exit);
    assert!(state.stopped_valid);
    assert_eq!(
        (state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
        (exit.code, exit.rip, 0xf10e, exit.info1)
    );
    state.stopped_info1 = 0xf400;
    state.stopped_info2 = 16;
    record_unexplained_stop(&mut state, false, exit);
    assert_eq!((state.stopped_info1, state.stopped_info2), (0xf400, 16));
    assert_eq!(*vmcb.bytes(), before);
}
