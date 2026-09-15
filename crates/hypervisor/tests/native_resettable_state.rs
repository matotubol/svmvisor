//! Reset configuration with a pending-free physical LAPIC. The model derives
//! priority and timer/error side effects independently of the emitted writes.
use svmvisor_hypervisor::svm::{
    native_apic_reset::{NativeApicReset, NativeApicResetError},
    x2apic::ApicMode,
};

fn initial(extended: bool, priority: u64) -> [u64; 0x54] {
    let mut r = [0; 0x54];
    r[3] = if extended { 0x8105_0010 } else { 0x0005_0014 };
    r[0xf] = 0xff;
    r[8] = priority;
    r[9] = priority;
    r[0xa] = priority;
    r[0x28] = 0x60;
    r[0x38] = u32::MAX as u64;
    r[0x39] = 123;
    r[0x40] = 0x40007;
    r[0x41] = 4;
    for i in (0x32..=0x37).chain(0x50..=0x53) { r[i] = 0x10000; }
    r[0x32] |= 0x200e1; // Masked periodic timer, with a live countdown.
    r
}

#[test]
fn every_task_priority_and_masked_timer_reset_without_device_acknowledgement() {
    for extended in [false, true] {
        for mode in [ApicMode::XApic, ApicMode::X2Apic] {
            for priority in 0..=255 {
                let mut r = initial(extended, priority);
                let before = r;
                let plan = NativeApicReset::prepare(0xb40f40, mode, |offset| {
                    Some(r[offset as usize / 16])
                }).unwrap();
                assert_eq!(r, before, "preparation must not mutate");
                let mut internal_error = 0x20;
                let mut errors_cleared = 0;
                plan.writes(|offset, value| {
                    assert!(!matches!(offset, 0xb0 | 0x420 | 0x300 | 0x310),
                        "reset must not acknowledge devices or send interrupts");
                    r[offset as usize / 16] = value as u64;
                    match offset {
                        0x80 => { r[9] = value as u64; r[0xa] = value as u64; }
                        0x380 => r[0x39] = value as u64,
                        0x280 => {
                            r[0x28] = internal_error;
                            internal_error = 0;
                            errors_cleared += 1;
                        }
                        _ => {}
                    }
                    assert_eq!(r[0xf] & 0x100, 0);
                    assert_ne!(r[0x32] & 0x10000, 0);
                });
                assert_eq!((r[8], r[9], r[0xa], r[0x28], r[0x38], r[0x39]),
                    (0, 0, 0, 0, 0, 0));
                assert_eq!(errors_cleared, 2);
            }
        }
    }
}

#[test]
fn resettable_state_does_not_hide_live_interrupts_or_unmasked_sources() {
    for (index, bits) in [(0x10, 1), (0x18, 1), (0x20, 1), (0x32, 0x1000), (0x35, 0x4000)] {
        let mut r = initial(true, 0xff);
        r[index] |= bits;
        let before = r;
        assert_eq!(NativeApicReset::prepare(0xb40f40, ApicMode::XApic,
            |offset| Some(r[offset as usize / 16])),
            Err(NativeApicResetError::RegisterState {
                offset: (index * 16) as u16, value: r[index]
            }));
        assert_eq!(r, before);
    }
    let mut r = initial(true, 0xff);
    r[0x32] &= !0x10000;
    assert!(NativeApicReset::prepare(0xb40f40, ApicMode::XApic,
        |offset| Some(r[offset as usize / 16])).is_err());
}
