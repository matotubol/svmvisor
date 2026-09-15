#![cfg(feature = "native-preflight")]

use svmvisor_dxe::native_boundary::{
    ABI_VERSION, CAPTURE_XCR0, CAPTURE_XSS, MAX_BOUNDARY_STACK_BYTES, NativeBoundary,
};

fn fixture(profile: u64) -> NativeBoundary {
    // The ABI has only integer/byte-array fields; zero is valid for each.
    let mut capture: NativeBoundary = unsafe { core::mem::zeroed() };
    capture.abi_version = ABI_VERSION;
    capture.profile = profile;
    capture.xstate_size = if profile == 0 { 512 } else { 832 };
    if profile != 0 {
        capture.captured_fields = CAPTURE_XCR0;
        capture.xcr0 = profile;
        capture.supported_xcr0 = 7;
        capture.cr4 = 1 << 18;
        capture.leaf1_ecx = 7 << 26;
        capture.avx_offset = 576;
        capture.avx_size = 256;
    }
    capture
}

#[test]
fn original_capture_is_aligned_and_fits_one_efi_stack_page() {
    let capture = fixture(7);
    assert_eq!(core::mem::size_of::<NativeBoundary>(), 1408);
    assert_eq!(capture.xstate_address() & 63, 0);
    assert_eq!(capture.xstate_address() - &capture as *const _ as u64, 384);
    assert_eq!(core::mem::offset_of!(NativeBoundary, entry_rsp), 168);
    assert_eq!(core::mem::offset_of!(NativeBoundary, entry_rip), 176);
    assert!(MAX_BOUNDARY_STACK_BYTES < 4096);
}

#[test]
fn xss_capability_requires_an_actual_zero_observation() {
    let mut capture = fixture(7);
    assert!(capture.has_valid_shape());
    capture.leaf_d1_eax = 8;
    assert!(!capture.has_valid_shape());
    capture.captured_fields |= CAPTURE_XSS;
    assert!(capture.has_valid_shape());
    capture.xss = 1 << 8;
    assert!(!capture.has_valid_shape());
    capture.xss = 0;
    capture.leaf_d1_eax = 0;
    assert!(!capture.has_valid_shape());
}

#[test]
fn dormant_xcr0_is_not_invented_for_the_fx_profile() {
    let mut capture = fixture(0);
    capture.leaf1_ecx = 1 << 26; // XSAVE exists; OSXSAVE is disabled.
    assert!(capture.has_valid_shape());
    capture.captured_fields |= CAPTURE_XCR0;
    assert!(!capture.has_valid_shape());
    capture.captured_fields = 0;
    capture.xcr0 = 3;
    assert!(!capture.has_valid_shape());
    capture.xcr0 = 0;
    capture.leaf1_ecx |= 1 << 27;
    assert!(!capture.has_valid_shape());
}

#[test]
fn advanced_state_cannot_be_silently_truncated_to_avx() {
    let mut capture = fixture(7);
    capture.xcr0 = 0xe7;
    assert!(!capture.has_valid_shape());
    capture.xcr0 = 7;
    capture.leaf_d1_eax = 1 << 4; // Dynamic XFD has no qualified MSR path.
    assert!(!capture.has_valid_shape());
    capture.leaf_d1_eax = 0;
    capture.supported_xcr0 = 3;
    assert!(!capture.has_valid_shape());
}

#[test]
fn original_enabled_image_must_cover_the_enumerated_avx_component() {
    let mut capture = fixture(7);
    for (offset, size, enabled, accepted) in [
        (576, 256, 832, true),
        (576, 256, 831, false),
        (768, 256, 1024, true),
        (769, 256, 1024, false),
        (576, 255, 832, false),
        (575, 256, 832, false),
        (576, 256, 1025, false),
    ] {
        capture.avx_offset = offset;
        capture.avx_size = size;
        capture.xstate_size = enabled;
        assert_eq!(capture.has_valid_shape(), accepted);
    }
}
