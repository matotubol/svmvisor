use svmvisor_hypervisor::memory::address::{AddressError as E, AddressPolicy, EncryptionState};

fn policy(bits: u8) -> AddressPolicy {
    AddressPolicy::new(bits, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

#[test]
fn inclusive_last_byte_boundary_and_overflow() {
    for bits in [32, 48, 52] {
        let p = policy(bits);
        let limit = 1u64 << bits;
        let range = p.validate(limit - 4096, 4096, 4096).unwrap();
        assert_eq!(range.last_byte(), limit - 1);
        assert_eq!(range.len(), 4096);
        assert_eq!(p.validate(limit - 4096, 4097, 4096), Err(E::OutsidePhysicalWidth));
        assert_eq!(p.validate(limit, 1, 1), Err(E::OutsidePhysicalWidth));
        assert_eq!(p.validate(0, limit, 4096).unwrap().last_byte(), limit - 1);
        assert_eq!(p.validate(u64::MAX, 2, 1), Err(E::Overflow));
    }
}

#[test]
fn malformed_sizes_alignments_and_widths_fail_closed() {
    let p = policy(48);
    assert_eq!(p.validate(0, 0, 4096), Err(E::EmptyRange));
    for alignment in [0, 3, 4095] {
        assert_eq!(p.validate(0, 4096, alignment), Err(E::InvalidAlignment));
    }
    assert_eq!(p.validate(4097, 4096, 4096), Err(E::Misaligned));
    // Length need not be a multiple of the required base alignment.
    assert!(p.validate(4096, 3, 4096).is_ok());
    for bits in [0, 31, 53, 64, 255] {
        assert_eq!(
            AddressPolicy::new(bits, EncryptionState::Unencrypted { encryption_bit: None }),
            Err(E::UnsupportedPhysicalWidth)
        );
    }
}

#[test]
fn encryption_evidence_and_encoded_ranges_are_rejected() {
    assert_eq!(AddressPolicy::new(48, EncryptionState::Unknown), Err(E::UnknownEncryption));
    assert_eq!(
        AddressPolicy::new(48, EncryptionState::Active),
        Err(E::ActiveEncryptionUnsupported)
    );
    assert_eq!(
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: Some(64) }),
        Err(E::InvalidEncryptionBit)
    );
    let p =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: Some(20) }).unwrap();
    let bit = 1 << 20;
    assert!(p.validate(0, bit, 4096).is_ok());
    assert_eq!(p.validate(bit, 4096, 4096), Err(E::EncryptionBitEncoded));
    assert_eq!(p.validate(0, bit + 1, 4096), Err(E::EncryptionBitEncoded));
    // Both endpoint bits clear, but the interior still contains encoded addresses.
    assert_eq!(p.validate(0, 2 * bit + 1, 4096), Err(E::EncryptionBitEncoded));
    assert!(p.validate(2 * bit, 4096, 4096).is_ok());
}
