use svmvisor_hypervisor::host::resident::terminal;

// Independent transport check: every operand bit survives in its declared
// mode, CPU slot does not collide with predicate/mode, and omission is explicit.
#[test]
fn syscfg_terminal_preserves_every_operand_bit_and_cpu_slot() {
    for slot in 0..32 {
        for bit in 0..64 {
            for (requested, observed) in [(1u64 << bit, 0x740000), (0x7c0000, 1u64 << bit)] {
                let (tag, value) = terminal::syscfg_operands(0x85, true, requested, observed);
                let words = terminal::stop_words(slot, 0x7c, u64::MAX, tag, value).unwrap();
                assert_eq!(words[0] & 255, 0x85);
                assert_eq!((words[0] >> 8) & 31, slot as u32);
                assert_eq!((words[0] >> 13) & 255, 0x85);
                assert_eq!(words[0] >> 24, 0x10);
                assert_ne!(words[0] & (1 << 23), 0);
                let payload = u64::from(words[1]) | (u64::from(words[2]) << 32);
                let mode = (words[0] >> 21) & 3;
                if observed > u32::MAX as u64 {
                    assert_eq!(mode, 2);
                    assert_eq!(payload, observed);
                } else if requested > u32::MAX as u64 {
                    assert_eq!(mode, 1);
                    assert_eq!(payload, requested);
                } else {
                    assert_eq!(mode, 0);
                    assert_eq!(words[1], requested as u32);
                    assert_eq!(words[2], observed as u32);
                }
            }
        }
    }
}

#[test]
fn syscfg_context_and_malformed_transport_have_no_guest_side_effects() {
    use svmvisor_hypervisor::svm::{dispatch::NativeEferError, syscfg::SyscfgError, vmcb::Vmcb};
    let vmcb = Vmcb::new();
    let before = *vmcb.bytes();
    let (tag, payload) = terminal::syscfg_failure(
        SyscfgError::UnsupportedProfile { signature: 0xb40f40, physical_bits: 48 },
        &vmcb,
        None,
    );
    let words = terminal::stop_words(31, 0x7c, 0, tag, payload).unwrap();
    assert_eq!((words[0] >> 13) & 255, 0x80);
    assert_eq!((words[0] >> 21) & 3, 3);
    assert_eq!(words[1], 0xb40f40);
    assert_eq!(words[2], 48);
    let (tag, payload) = terminal::syscfg_failure(
        SyscfgError::Boundary(NativeEferError::UnsupportedMode),
        &vmcb,
        None,
    );
    let words = terminal::stop_words(0, 0x7c, 0, tag, payload).unwrap();
    assert_eq!((words[0] >> 21) & 3, 3);
    assert_eq!(*vmcb.bytes(), before);
    // Stage85 only accepts policy operand modes with write=true, and
    // instruction/profile context in mode3. No malformed record is exported.
    for reason in [0u64, 1, 0x80, 0x81, 0x85, 0x86, 255] {
        for mode in 0..4u64 {
            for write in 0..2u64 {
                let tag = 0xf10d | ((reason | (mode << 8) | (write << 10)) << 16);
                let valid = if mode == 3 {
                    matches!(reason, 1 | 0x80)
                } else {
                    (0x81..=0x85).contains(&reason) && write == 1
                };
                assert_eq!(
                    terminal::stop_words(0, 0x7c, 0, tag, u64::MAX).is_some(),
                    valid,
                    "reason={reason:x} mode={mode} write={write}"
                );
                assert!(terminal::stop_words(32, 0x7c, 0, tag, 0).is_none());
                assert!(terminal::stop_words(0, 0x7c, 0, tag | (1 << 40), 0).is_none());
            }
        }
    }
}
