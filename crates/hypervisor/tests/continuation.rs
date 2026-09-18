use svmvisor_hypervisor::guest::continuation::{
    CONTINUATION_RFLAGS_MASK, ContinuationError, IntegerContinuation,
};
use svmvisor_hypervisor::memory::address::{AddressPolicy, EncryptionState, PhysicalRange};

fn page(base: u64) -> PhysicalRange {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None })
        .unwrap()
        .validate(base, 4096, 4096)
        .unwrap()
}

fn captured() -> IntegerContinuation {
    IntegerContinuation { rip: 0x10200, rsp: 0x21ff0, rflags: 3, ..Default::default() }
}

#[test]
fn assembly_record_offsets_and_size_are_exact() {
    use core::mem::{align_of, offset_of, size_of};
    assert_eq!(size_of::<IntegerContinuation>(), 144);
    assert_eq!(align_of::<IntegerContinuation>(), 8);
    assert_eq!(offset_of!(IntegerContinuation, registers), 0);
    assert_eq!(offset_of!(IntegerContinuation, rax), 112);
    assert_eq!(offset_of!(IntegerContinuation, rip), 120);
    assert_eq!(offset_of!(IntegerContinuation, rsp), 128);
    assert_eq!(offset_of!(IntegerContinuation, rflags), 136);
}

#[test]
fn capture_accepts_arithmetic_flags_and_both_boundary_instruction_addresses() {
    let mut state = captured();
    for rip in [0x10000, 0x10fff] {
        for flags in [2, 3, CONTINUATION_RFLAGS_MASK] {
            state.rip = rip;
            state.rflags = flags;
            assert_eq!(state.validate_bounds(page(0x10000), page(0x21000)), Ok(()));
        }
    }
}

#[test]
fn capture_refuses_every_nonarithmetic_flag_without_mutating_snapshot() {
    let mut state = captured();
    for bit in 0..64 {
        if (1u64 << bit) & CONTINUATION_RFLAGS_MASK != 0 {
            continue;
        }
        state.rflags = 2 | (1u64 << bit);
        assert_eq!(
            state.validate_bounds(page(0x10000), page(0x21000)),
            Err(ContinuationError::UnsupportedRflags)
        );
        assert_eq!(state.rflags, 2 | (1u64 << bit));
    }
    state.rflags = 1;
    assert_eq!(
        state.validate_bounds(page(0x10000), page(0x21000)),
        Err(ContinuationError::UnsupportedRflags)
    );
}

#[test]
fn capture_refuses_outside_code_or_incomplete_stack_markers() {
    let mut state = captured();
    for rip in [0xffff, 0x11000, 0x0000_8000_0000_0000] {
        state.rip = rip;
        assert_eq!(
            state.validate_bounds(page(0x10000), page(0x21000)),
            Err(ContinuationError::RipOutsideCode)
        );
    }
    state.rip = 0x10200;
    for rsp in [0x20ff8, 0x21ff8, 0x22000, 0x21001, 0x0000_8000_0000_0000] {
        state.rsp = rsp;
        assert_eq!(
            state.validate_bounds(page(0x10000), page(0x21000)),
            Err(ContinuationError::RspOutsideStack)
        );
    }
}

#[test]
fn capture_requires_separate_whole_pages() {
    let state = captured();
    assert_eq!(
        state.validate_bounds(page(0x10000), page(0x10000)),
        Err(ContinuationError::OverlappingPages)
    );
    let policy =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    for range in
        [policy.validate(0x10000, 8192, 4096).unwrap(), policy.validate(0x10001, 4096, 1).unwrap()]
    {
        assert_eq!(
            state.validate_bounds(range, page(0x21000)),
            Err(ContinuationError::InvalidCodePage)
        );
        assert_eq!(
            state.validate_bounds(page(0x10000), range),
            Err(ContinuationError::InvalidStackPage)
        );
    }
}
