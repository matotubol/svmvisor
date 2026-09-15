use svmvisor_hypervisor::svm::{
    native_apic_reset::{NativeApicReset, NativeApicResetError},
    x2apic::ApicMode,
};

const RYZEN_SIGNATURE: u32 = 0x00b4_0f40;

fn registers(extended: bool) -> [u64; 0x54] {
    let mut registers = [0; 0x54];
    registers[3] = if extended { 0x8105_0010 } else { 0x0005_0014 };
    registers[0x0f] = 0xff;
    registers[0x40] = 0x40007;
    registers[0x41] = 4;
    for index in 0x32..=0x37 {
        registers[index] = 0x10000;
    }
    for index in 0x50..=0x53 {
        registers[index] = 0x10000;
    }
    registers
}

fn prepare(
    registers: &[u64; 0x54],
    mode: ApicMode,
) -> Result<NativeApicReset, NativeApicResetError> {
    NativeApicReset::prepare(RYZEN_SIGNATURE, mode, |offset| {
        Some(registers[offset as usize / 16])
    })
}

#[test]
fn ryzen_extended_state_has_bounded_complete_reset_and_no_acknowledgments() {
    for mode in [ApicMode::XApic, ApicMode::X2Apic] {
        let mut state = registers(true);
        state[0x41] = 6;
        // Non-reset configuration with no active source is reset, not refused.
        state[0x35] |= 0x80f1;
        state[0x53] |= 0x4f1;
        let before = state;
        let plan = prepare(&state, mode).unwrap();
        assert_eq!(plan.destination_mode(), if mode == ApicMode::XApic { svmvisor_hypervisor::svm::ipi::NativeDestinationMode::ExtendedXApic8 } else { svmvisor_hypervisor::svm::ipi::NativeDestinationMode::X2Apic });
        assert_eq!(state, before);
        let mut writes = Vec::new();
        plan.writes(|offset, value| {
            writes.push((offset, value));
            state[offset as usize / 16] = value as u64;
        });
        assert_eq!(writes.len(), if mode == ApicMode::XApic { 28 } else { 26 });
        for index in 0x32..=0x37 {
            assert_eq!(state[index], 0x10000);
        }
        for index in 0x50..=0x53 {
            assert_eq!(state[index], 0x10000);
        }
        let enable = writes.iter().position(|&pair| pair == (0x410, 7)).unwrap();
        let disable = writes.iter().position(|&pair| pair == (0x410, 4)).unwrap();
        for (index, &(offset, value)) in writes.iter().enumerate() {
            assert!(!matches!(offset, 0x020 | 0x0b0 | 0x300 | 0x310 | 0x420));
            if (0x480..=0x4f0).contains(&offset) {
                assert!(enable < index && index < disable);
                assert_eq!(
                    value,
                    if offset == 0x480 {
                        0xffff0000
                    } else {
                        u32::MAX
                    }
                );
            }
        }
        assert_eq!(state[0x41], 4);
        assert_eq!(state[0x0f], 0xff);
        assert_eq!(
            &writes[writes.len() - 3..],
            &[(0x280, 0), (0x280, 0), (0x0f0, 0xff)]
        );
        if mode == ApicMode::XApic {
            assert_eq!(state[0x0d], 0);
            assert_eq!(state[0x0e], 0xf0000000);
        } else {
            assert!(
                !writes
                    .iter()
                    .any(|&(offset, _)| matches!(offset, 0x0d0 | 0x0e0))
            );
        }
    }
}

#[test]
fn legacy_profile_never_accesses_extended_registers() {
    let state = registers(false);
    for mode in [ApicMode::XApic, ApicMode::X2Apic] {
        let plan = NativeApicReset::prepare(0, mode, |offset| {
            assert!(offset < 0x400);
            assert!(mode != ApicMode::X2Apic || offset != 0x090);
            Some(state[offset as usize / 16])
        })
        .unwrap();
        plan.writes(|offset, value| {
            assert!(offset < 0x400);
            if offset == 0x0e0 {
                assert_eq!(value, u32::MAX);
            }
        });
    }
}

#[test]
fn every_interrupt_bitmap_bank_refuses_without_mutation() {
    for mode in [ApicMode::XApic, ApicMode::X2Apic] {
        for base in [0x10, 0x18, 0x20] {
            for bank in 0..8 {
                for bit in 0..32 {
                    let mut state = registers(true);
                    state[base + bank] = 1 << bit;
                    let before = state;
                    assert_eq!(
                        prepare(&state, mode),
                        Err(NativeApicResetError::RegisterState {
                            offset: ((base + bank) * 16) as u16,
                            value: 1 << bit,
                        })
                    );

                    assert_eq!(state, before);
                }
            }
        }
    }
}

#[test]
fn each_standard_and_extended_lvt_must_be_masked_and_idle() {
    for index in (0x32..=0x37).chain(0x50..=0x53) {
        for value in [0, 0x11000] {
            let mut state = registers(true);
            state[index] = value;
            assert_eq!(
                prepare(&state, ApicMode::X2Apic),
                Err(NativeApicResetError::RegisterState {
                    offset: (index * 16) as u16,
                    value,
                })
            );
        }
    }
    for index in 0x35..=0x36 {
        let mut state = registers(true);
        state[index] |= 1 << 14;
        assert!(prepare(&state, ApicMode::X2Apic).is_err());
    }
}

#[test]
fn enabled_svr_busy_icr_or_malformed_register_width_refuses() {
    for (index, bit) in [
        (0x0f, 8),
        (0x08, 8),
        (0x09, 8),
        (0x0a, 8),
        (0x28, 8),
        (0x38, 32),
        (0x39, 32),
        (0x30, 12),
    ] {
        let mut state = registers(true);
        state[index] |= 1 << bit;
        assert_eq!(
            prepare(&state, ApicMode::XApic),
            Err(NativeApicResetError::RegisterState {
                offset: (index * 16) as u16,
                value: state[index],
            })
        );
    }
}

#[test]
fn unrecognized_layout_is_refused_before_extended_access() {
    for (signature, version) in [
        (0, 0x81050010),
        (RYZEN_SIGNATURE, 0x81060010),
        (RYZEN_SIGNATURE, 0x80050010),
        (RYZEN_SIGNATURE, 0x81050014),
        (RYZEN_SIGNATURE, 0x181050010),
    ] {
        let result = NativeApicReset::prepare(signature, ApicMode::X2Apic, |offset| {
            assert_eq!(offset, 0x030);
            Some(version)
        });
        assert_eq!(result, Err(if signature == 0 {
            NativeApicResetError::UnsupportedSignature { signature }
        } else { NativeApicResetError::UnsupportedLayout { version } }));
    }
    assert_eq!(
        NativeApicReset::prepare(RYZEN_SIGNATURE, ApicMode::Disabled, |_| panic!(
            "disabled APIC read"
        )),
        Err(NativeApicResetError::UnsupportedMode)
    );
}

#[test]
fn malformed_extended_feature_or_controls_refuses() {
    for (index, value) in [(0x40, 0x30007), (0x40, 0x40006), (0x40, 0x50007), (0x41, 8)] {
        let mut state = registers(true);
        state[index] = value;
        assert_eq!(
            prepare(&state, ApicMode::X2Apic),
            Err(NativeApicResetError::RegisterState {
                offset: (index * 16) as u16,
                value,
            })
        );
    }
}

#[test]
fn missing_native_read_is_a_stopped_refusal() {
    let state = registers(true);
    assert_eq!(
        NativeApicReset::prepare(RYZEN_SIGNATURE, ApicMode::X2Apic, |offset| {
            if offset == 0x530 {
                None
            } else {
                Some(state[offset as usize / 16])
            }
        }),
        Err(NativeApicResetError::UnavailableRegister(0x530))
    );
}

#[test]
fn repeated_guest_init_retains_exact_physical_id_routing() {
    let mut state = registers(true);
    for _ in 0..8 {
        let reset = prepare(&state, ApicMode::XApic).unwrap();
        reset.writes(|offset, value| {
            if offset == 0x410 { assert_ne!(value & 4, 0); }
            state[offset as usize / 16] = value as u64;
        });
        assert_eq!(state[0x41], 4);
    }
    state[0x41] = 0;
    assert_eq!(prepare(&state, ApicMode::XApic),
        Err(NativeApicResetError::RegisterState { offset: 0x410, value: 0 }));
}

#[test]
fn hidden_extension_admission_checks_defined_bits_and_retains_failing_operand() {
    use svmvisor_hypervisor::svm::native_apic_reset::admit_hidden_native_apic_state;
    let mut state = registers(true);
    for i in 0x48..=0x4f { state[i] = u32::MAX as u64; }
    for reserved in [0, 1, 0x1234, 0xffff] {
        state[0x48] = 0xffff0000 | reserved;
        assert!(admit_hidden_native_apic_state(|o| state[o as usize / 16]).is_ok());
    }
    for index in 0x48..=0x53 {
        let original = state[index];
        state[index] = if index < 0x50 { original & !(1 << 20) } else { 0x11000 };
        let before = state;
        assert_eq!(admit_hidden_native_apic_state(|o| state[o as usize / 16]),
            Err(NativeApicResetError::RegisterState { offset: (index * 16) as u16, value: state[index] }));
        assert_eq!(state, before);
        state[index] = original;
    }
    state[0x48] |= 1u64 << 32;
    assert!(admit_hidden_native_apic_state(|o| state[o as usize / 16]).is_err());
}
