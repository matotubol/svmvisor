use svmvisor_hypervisor::{
    address::{AddressPolicy, EncryptionState},
    guest_state::GuestStateRequest,
    permission_maps::{IOPM_BYTES, MSRPM_BYTES},
    vmcb::{InstructionIntercept, Vmcb},
};

#[test]
fn validated_tuple_and_permission_controls_have_exact_independent_layout() {
    let policy = AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let state = GuestStateRequest {
        rip: 0xffff_8000_1234_5678,
        rsp: 0x1234_5678,
        rflags: 2,
        cr0: 0x8001_0033,
        cr3: 0x1234_5000,
        cr4: 0x20,
        efer: 0x1500,
        rax: 0x1234_5678_9abc_def0,
    }
    .validate(&policy)
    .unwrap();
    let mut vmcb = Vmcb::new();
    vmcb.set_permission_maps(0x1000, 0x4000, &policy).unwrap();
    vmcb.set_instruction_intercept(InstructionIntercept::Ioio, true);
    vmcb.set_instruction_intercept(InstructionIntercept::Msr, true);
    vmcb.set_synthetic_state(&state);
    let mut expected = [0u8; 4096];
    expected[0x00f] = 0x18; // IOIO_PROT/MSR_PROT; no other intercepts implied.
    expected[0x05c] = 1; // Publishing paging controls requests a flush.
    for (offset, value) in [
        (0x040, 0x1000u64),
        (0x048, 0x4000),
        (0x4d0, 0x1500),
        (0x548, 0x20),
        (0x550, 0x1234_5000),
        (0x558, 0x8001_0033),
        (0x570, 2),
        (0x578, 0xffff_8000_1234_5678),
        (0x5d8, 0x1234_5678),
        (0x5f8, 0x1234_5678_9abc_def0),
    ] {
        expected[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    assert_eq!(vmcb.bytes(), &expected);
    vmcb.set_instruction_intercept(InstructionIntercept::Ioio, false);
    assert!(!vmcb.instruction_intercept(InstructionIntercept::Ioio));
    assert!(vmcb.instruction_intercept(InstructionIntercept::Msr));
    assert_eq!(vmcb.guest_rip(), state.rip());
    assert_eq!(vmcb.guest_rsp(), state.rsp());
    assert_eq!(vmcb.guest_rax(), state.rax());
    assert_eq!((IOPM_BYTES, MSRPM_BYTES), (12288, 8192));
}
