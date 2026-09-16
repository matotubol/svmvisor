use svmvisor_hypervisor::svm::native_apic_reset::NativeApicResetError;

#[test]
fn hidden_extension_admission_checks_defined_bits_and_retains_failing_operand() {
    use svmvisor_hypervisor::svm::native_apic_reset::admit_hidden_native_apic_state;
    let mut state = [0u64; 0x54];
    for item in &mut state[0x50..=0x53] { *item = 0x10000; }
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
