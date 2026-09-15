use svmvisor_firmware_handoff::{AUTHORIZATION, authorized, valid_payload};

#[test]
fn authorization_requires_exact_binary_token() {
    assert!(authorized(Some(AUTHORIZATION)));
    assert!(!authorized(None));
    assert!(!authorized(Some(b"")));
    assert!(!authorized(Some(b"SVMVISOR-EMULATOR-HANDOFF-V1\0")));
    assert!(!authorized(Some(b"SVMVISOR-EMULATOR-HANDOFF-V2")));
    assert!(!authorized(Some(&AUTHORIZATION[..AUTHORIZATION.len() - 1])));
    let utf16: Vec<u8> = AUTHORIZATION.iter().flat_map(|byte| [*byte, 0]).collect();
    assert!(!authorized(Some(&utf16)));
}

#[test]
fn payload_and_entry_stay_below_the_handoff_page() {
    assert!(valid_payload(1, 0));
    assert!(valid_payload(0xff000, 0xfefff));
    assert!(!valid_payload(0, 0));
    assert!(!valid_payload(0xff001, 0));
    assert!(!valid_payload(0xff000, 0xff000));
    assert!(!valid_payload(1, usize::MAX));
    assert!(!valid_payload(usize::MAX, 0));
}
