use svmvisor_hypervisor::svm::{native_pause::native_pause_retry_ready, vmcb::Vmcb};

fn write(vmcb: &mut Vmcb, offset: usize, bytes: &[u8]) {
    // Test-only construction of fields normally written by VMRUN/VMEXIT.
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            bytes.len(),
        );
    }
}

#[test]
fn unsupported_filter_configuration_is_inert() {
    let mut v = Vmcb::new();
    let before = *v.bytes();
    assert!(!v.configure_native_pause_filter(0));
    assert_eq!(v.bytes(), &before);
}

#[test]
fn pause_retries_preserve_debug_pending_and_all_guest_state() {
    for mode in [0u64, 1, 0x80000001] {
        let mut v = Vmcb::new();
        assert!(v.configure_native_pause_filter(1 << 10));
        write(&mut v, 0x070, &0x77u64.to_le_bytes());
        write(&mut v, 0x558, &mode.to_le_bytes());
        write(&mut v, 0x570, &0x10302u64.to_le_bytes()); // TF,IF,RF remain hardware-owned.
        write(&mut v, 0x578, &0x1234u64.to_le_bytes());
        write(&mut v, 0x0a8, &0x8000030du64.to_le_bytes());
        let before = *v.bytes();
        assert!(native_pause_retry_ready(&v));
        assert_eq!(v.bytes(), &before);
    }
}

#[test]
fn zero_or_advanced_filters_and_wrong_exit_cannot_retry() {
    for (offset, bytes) in
        [(0x03e, vec![0, 0]), (0x03c, vec![1, 0]), (0x070, vec![0x78]), (0x00e, vec![0])]
    {
        let mut v = Vmcb::new();
        v.configure_native_pause_filter(1 << 10);
        write(&mut v, 0x070, &0x77u64.to_le_bytes());
        write(&mut v, offset, &bytes);
        assert!(!native_pause_retry_ready(&v));
    }
}
