use svmvisor_hypervisor::svm::emulation::{
    ABI_VERSION, HYPERCALL_QUERY, HYPERCALL_STOP, HypercallAction, hypercall,
};

#[test]
fn hypercalls_only_query_or_stop_without_opcode_truncation() {
    assert_eq!(
        hypercall(HYPERCALL_QUERY),
        HypercallAction::Query { abi_version: ABI_VERSION as u64 }
    );
    assert_eq!(hypercall(HYPERCALL_STOP), HypercallAction::Stop);
    for opcode in [2, 3, 1 << 32, (1 << 32) | HYPERCALL_STOP, u64::MAX] {
        assert_eq!(hypercall(opcode), HypercallAction::Unsupported { opcode });
    }
}
