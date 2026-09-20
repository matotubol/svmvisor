use svmvisor_hypervisor::arch::x86_64::xstate::*;

#[test]
fn mxcsr_masks_and_reserved_values_fail_before_restore() {
    assert_eq!(effective_mxcsr_mask(0), Ok(0xffbf));
    assert_eq!(effective_mxcsr_mask(0xffff), Ok(0xffff));
    assert_eq!(effective_mxcsr_mask(0x2ffff), Ok(0x2ffff));
    for mask in [1, 0x1f00, 0x1ffff, 0x3ffff, 0x6ffff, 0x8002ffff] {
        assert_eq!(effective_mxcsr_mask(mask), Err(XstateError::InvalidMxcsrMask));
    }
}
