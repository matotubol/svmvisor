use crate::{memory::address::AddressPolicy, svm::vmcb::*};

#[test]
fn successful_entry_consumes_once_and_invalid_entry_keeps_request() {
    let mut vmcb = Vmcb::new();
    vmcb.request_full_tlb_flush();
    for invalid in [u64::MAX, u32::MAX as u64] {
        vmcb.write_u64::<0x070>(invalid);
        let before = vmcb.bytes;
        // Synthetic model of the assembly's post-entry call, not hardware proof.
        unsafe {
            vmcb.consume_tlb_flush_after_exit();
        }
        assert_eq!(vmcb.bytes, before);
    }
    vmcb.write_u64::<0x070>(0x72);
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    assert_eq!(vmcb.bytes[0x05c], 0);
    vmcb.set_guest_rax(42);
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    assert_eq!(vmcb.bytes[0x05c], 0);
    vmcb.commit_native_efer(0xd01);
    assert_eq!(vmcb.bytes[0x05c], 1);
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    vmcb.initialize_ap_after_init();
    assert_eq!(vmcb.bytes[0x05c], 1);
    vmcb.start_ap_from_sipi(8);
    assert_eq!(vmcb.bytes[0x05c], 1);
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    assert_eq!(vmcb.bytes[0x05c], 0);
}

#[test]
fn nested_root_change_requests_flush_but_rejected_root_keeps_state() {
    let policy = AddressPolicy::new(
        48,
        crate::memory::address::EncryptionState::Unencrypted { encryption_bit: None },
    )
    .unwrap();
    let mut vmcb = Vmcb::new();
    vmcb.set_nested_root(0x1000, &policy).unwrap();
    assert_eq!(vmcb.bytes[0x05c], 1);
    vmcb.write_u64::<0x070>(0x400);
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    let before = vmcb.bytes;
    assert!(vmcb.set_nested_root(0x2001, &policy).is_err());
    assert_eq!(vmcb.bytes, before);
    vmcb.set_nested_root(0x2000, &policy).unwrap();
    assert_eq!(vmcb.bytes[0x05c], 1);
    assert_eq!(vmcb.nested_root(), 0x2000);
}
