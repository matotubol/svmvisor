use crate::svm::vmcb::*;

fn vmcb_with(code: u64, exitintinfo: u64, eventinj: u64) -> Vmcb {
    let mut vmcb = Vmcb::new();
    vmcb.write_u64::<0x070>(code);
    vmcb.write_u64::<0x088>(exitintinfo);
    vmcb.write_u64::<0x0a8>(eventinj);
    vmcb.write_u32::<CLEAN_BITS>(u32::MAX);
    vmcb
}

#[test]
fn interrupted_intr_nmi_and_exception_are_copied_verbatim_to_eventinj() {
    // TYPE 0 external interrupt vector 51h, no error code.
    let mut vmcb = vmcb_with(0x400, 0x8000_0051, 0);
    assert_eq!(
        vmcb.reinject_interrupted_delivery(),
        ReinjectOutcome::Reinjected { kind: 0, vector: 0x51 }
    );
    assert_eq!(vmcb.event_injection(), 0x8000_0051);
    assert_eq!(vmcb.bytes[CLEAN_BITS..CLEAN_BITS + 4], [0; 4]);
    // TYPE 2 NMI: vector field ignored, no error code.
    let mut vmcb = vmcb_with(0x400, 0x8000_0202, 0);
    assert_eq!(
        vmcb.reinject_interrupted_delivery(),
        ReinjectOutcome::Reinjected { kind: 2, vector: 0x02 }
    );
    assert_eq!(vmcb.event_injection(), 0x8000_0202);
    // TYPE 3 exception with an error code (both halves kept).
    let mut vmcb = vmcb_with(0x400, 0x0000_0030_8000_0b0d, 0);
    assert_eq!(
        vmcb.reinject_interrupted_delivery(),
        ReinjectOutcome::Reinjected { kind: 3, vector: 0x0d }
    );
    assert_eq!(vmcb.event_injection(), 0x0000_0030_8000_0b0d);
}

#[test]
fn undefined_error_and_reserved_bits_are_dropped_when_ev_is_clear() {
    // EV=0 but garbage in reserved (30:12) and the error-code half: the
    // rebuilt EVENTINJ keeps only V, TYPE and vector (15.7.2 p510).
    let mut vmcb = vmcb_with(0x400, 0xdead_beef_8000_7040, 0);
    assert_eq!(
        vmcb.reinject_interrupted_delivery(),
        ReinjectOutcome::Reinjected { kind: 0, vector: 0x40 }
    );
    assert_eq!(vmcb.event_injection(), 0x8000_0040);
}

#[test]
fn software_interrupt_reserved_type_and_conflicts_stay_terminal() {
    // TYPE 4 (INTn) is not re-injectable here.
    let mut vmcb = vmcb_with(0x400, 0x8000_0451, 0);
    assert_eq!(
        vmcb.reinject_interrupted_delivery(),
        ReinjectOutcome::Unsupported { interrupted: 0x8000_0451 }
    );
    assert_eq!(vmcb.event_injection(), 0);
    // A reserved TYPE (5).
    let mut vmcb = vmcb_with(0x400, 0x8000_0551, 0);
    assert!(matches!(vmcb.reinject_interrupted_delivery(), ReinjectOutcome::Unsupported { .. }));
    // A different event already queued in EVENTINJ.
    let mut vmcb = vmcb_with(0x400, 0x8000_0051, (1 << 31) | (3 << 8) | 13);
    assert_eq!(vmcb.reinject_interrupted_delivery(), ReinjectOutcome::Conflict);
    // The same event already queued is not a conflict.
    let mut vmcb = vmcb_with(0x400, 0x8000_0051, 0x8000_0051);
    assert!(matches!(vmcb.reinject_interrupted_delivery(), ReinjectOutcome::Reinjected { .. }));
    // Shutdown and invalid entry are terminal; V=0 is a no-op.
    for code in [0x7f, u64::MAX] {
        let mut vmcb = vmcb_with(code, 0x8000_0051, 0);
        assert_eq!(vmcb.reinject_interrupted_delivery(), ReinjectOutcome::Conflict);
    }
    let mut vmcb = vmcb_with(0x400, 0, 0);
    assert_eq!(vmcb.reinject_interrupted_delivery(), ReinjectOutcome::NoEvent);
}
