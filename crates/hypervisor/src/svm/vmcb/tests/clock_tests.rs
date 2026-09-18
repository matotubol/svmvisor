use crate::svm::vmcb::*;

#[test]
fn identity_offset_clears_stale_offset_and_invalidates_clean_bits_only() {
    let mut vmcb = Vmcb::new();
    vmcb.set_instruction_intercept(InstructionIntercept::Rdtscp, true);
    vmcb.write_u64::<TSC_OFFSET>(u64::MAX);
    vmcb.write_u32::<CLEAN_BITS>(u32::MAX);
    let mut expected = *vmcb.bytes();
    expected[TSC_OFFSET..TSC_OFFSET + 8].fill(0);
    expected[CLEAN_BITS..CLEAN_BITS + 4].fill(0);
    vmcb.set_tsc_offset_zero();
    assert_eq!(vmcb.tsc_offset(), 0);
    assert_eq!(vmcb.bytes(), &expected);
}
