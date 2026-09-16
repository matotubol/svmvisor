use svmvisor_hypervisor::{
    boot::handoff::{HANDOFF_SIZE, Handoff, HandoffError},
    memory::layout::{LayoutRequest, PAGE_SIZE},
};

fn request() -> LayoutRequest {
    LayoutRequest {
        code_bytes: 4097,
        data_bytes: 1,
        stack_bytes: 8192,
    }
}

fn valid() -> Handoff {
    Handoff::new(0x12_3450_0000, 64 * PAGE_SIZE, request(), 1).unwrap()
}

fn fails(bytes: &[u8], expected: HandoffError) {
    match Handoff::decode(bytes) {
        Ok(_) => panic!("invalid handoff accepted"),
        Err(error) => assert_eq!(error, expected),
    }
}

#[test]
fn fixed_wire_layout_round_trips_and_reconstructs_regions() {
    let original = valid();
    let bytes = original.encode();
    assert_eq!(&bytes[..16], b"SVMDEV01\x01\x00\x50\x00\x01\x00\x00\x00");
    assert_eq!(&bytes[16..24], &[0, 0, 0x50, 0x34, 0x12, 0, 0, 0]);
    assert_eq!(&bytes[32..40], &[1, 0x10, 0, 0, 0, 0, 0, 0]);
    assert!(bytes[56..].iter().all(|byte| *byte == 0));
    let restored = Handoff::decode(&bytes).unwrap();
    assert_eq!(restored.encode(), bytes);
    assert_eq!(restored.arena_base(), original.arena_base());
    assert_eq!(
        restored.arena_end(),
        original.arena_base() + original.arena_bytes()
    );
    assert_eq!(
        restored.layout().used_bytes(),
        original.layout().used_bytes()
    );
    for (left, right) in restored
        .layout()
        .regions()
        .iter()
        .zip(original.layout().regions())
    {
        assert_eq!(left.kind(), right.kind());
        assert_eq!(left.offset(), right.offset());
        assert_eq!(left.len(), right.len());
        assert_eq!(left.permissions(), right.permissions());
    }
}

#[test]
fn malformed_headers_are_rejected_without_ignoring_trailing_bytes() {
    let valid = valid().encode();
    for size in 0..HANDOFF_SIZE {
        fails(&valid[..size], HandoffError::Size);
    }
    let mut oversized = [0; HANDOFF_SIZE + 1];
    oversized[..HANDOFF_SIZE].copy_from_slice(&valid);
    fails(&oversized, HandoffError::Size);
    for (offset, error) in [
        (0, HandoffError::Magic),
        (8, HandoffError::Version),
        (10, HandoffError::Size),
    ] {
        let mut bytes = valid;
        bytes[offset] ^= 1;
        fails(&bytes, error);
    }
    for offset in 56..HANDOFF_SIZE {
        let mut bytes = valid;
        bytes[offset] = 1;
        fails(&bytes, HandoffError::Reserved);
    }
    for count in [0u32, 2, u32::MAX] {
        let mut bytes = valid;
        bytes[12..16].copy_from_slice(&count.to_le_bytes());
        fails(&bytes, HandoffError::CpuCount);
    }
}

#[test]
fn untrusted_arena_metadata_cannot_wrap_or_bypass_layout_validation() {
    let valid = valid().encode();
    for base in [0u64, 1, PAGE_SIZE + 1] {
        let mut bytes = valid;
        bytes[16..24].copy_from_slice(&base.to_le_bytes());
        fails(&bytes, HandoffError::ArenaBase);
    }
    let mut bytes = valid;
    bytes[16..24].copy_from_slice(&(u64::MAX - PAGE_SIZE + 1).to_le_bytes());
    fails(&bytes, HandoffError::AddressOverflow);
    // The final representable aligned span is accepted without end truncation.
    let length = 64 * PAGE_SIZE;
    let highest = u64::MAX - (PAGE_SIZE - 1) - length;
    let handoff = Handoff::new(highest, length, request(), 1).unwrap();
    assert_eq!(handoff.arena_end(), u64::MAX - (PAGE_SIZE - 1));
    for (offset, value) in [
        (24, 0u64),
        (24, PAGE_SIZE),
        (24, 4097),
        (32, 0),
        (40, u64::MAX),
        (48, 0),
    ] {
        let mut bytes = valid;
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        assert!(matches!(
            Handoff::decode(&bytes),
            Err(HandoffError::Layout(_))
        ));
    }
}
