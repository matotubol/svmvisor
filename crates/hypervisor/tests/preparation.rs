//! Exercise the wire handoff, layout and CPU address policy together.
use svmvisor_hypervisor::{
    address::{AddressError, AddressPolicy, EncryptionState},
    handoff::Handoff,
    layout::{LayoutRequest, PAGE_SIZE},
};

fn handoff(base: u64) -> Handoff {
    let encoded = Handoff::new(
        base,
        16 * PAGE_SIZE,
        LayoutRequest {
            code_bytes: 1,
            data_bytes: 1,
            stack_bytes: 1,
        },
        1,
    )
    .unwrap()
    .encode();
    Handoff::decode(&encoded).unwrap()
}

#[test]
fn structural_handoff_must_also_fit_cpu_address_policy() {
    let policy = AddressPolicy::new(
        32,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let top = handoff((1u64 << 32) - 16 * PAGE_SIZE);
    let span = top.validate_addresses(&policy).unwrap();
    assert_eq!(span.last_byte(), (1u64 << 32) - 1);
    for region in top.layout().regions() {
        let base = top.arena_base() + region.offset();
        let checked = policy.validate(base, region.len(), PAGE_SIZE).unwrap();
        assert!(checked.last_byte() <= span.last_byte());
    }
    assert_eq!(
        handoff((1u64 << 32) - 15 * PAGE_SIZE).validate_addresses(&policy),
        Err(AddressError::OutsidePhysicalWidth)
    );
}

#[test]
fn arena_cannot_straddle_an_encryption_address_bit() {
    let policy = AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: Some(40),
        },
    )
    .unwrap();
    assert!(
        handoff((1u64 << 40) - 16 * PAGE_SIZE)
            .validate_addresses(&policy)
            .is_ok()
    );
    assert_eq!(
        handoff((1u64 << 40) - 15 * PAGE_SIZE).validate_addresses(&policy),
        Err(AddressError::EncryptionBitEncoded)
    );
}
