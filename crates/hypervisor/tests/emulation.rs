use svmvisor_hypervisor::svm::emulation::{
    ABI_VERSION, HYPERCALL_QUERY, HYPERCALL_STOP, HypercallAction, cpuid, hypercall,
};

#[test]
fn scalar_leaf_contract_and_vendor_register_orders() {
    let basic = cpuid(0, 0);
    let hypervisor = cpuid(0x4000_0000, 0);
    let basic_vendor: Vec<_> =
        [basic[1], basic[3], basic[2]].into_iter().flat_map(u32::to_le_bytes).collect();
    let hypervisor_vendor: Vec<_> =
        hypervisor[1..].iter().copied().flat_map(u32::to_le_bytes).collect();
    assert_eq!(basic_vendor, b"SvmVisorTest");
    assert_eq!(hypervisor_vendor, basic_vendor);
    assert_eq!(basic[0], 1);
    assert_eq!(hypervisor[0], 0x4000_0001);
    assert_eq!(cpuid(1, 0), [0, 0, 1 << 31, (1 << 5) | (1 << 6)]);
    assert_eq!(cpuid(0x4000_0001, 0), [ABI_VERSION, 0, 0, 0]);
    assert_eq!(cpuid(0x8000_0000, 0), [0x8000_0001, 0, 0, 0]);
    assert_eq!(cpuid(0x8000_0001, 0), [0, 0, 0, 1 << 29]);
    // No FPU, SSE, guest NX, or SVM support is advertised by this protocol.
    assert_eq!(cpuid(1, 0)[3] & (1 | (1 << 25) | (1 << 26)), 0);
    assert_eq!(cpuid(0x8000_0001, 0)[3] & (1 << 20), 0);
    assert_eq!(cpuid(0x8000_0001, 0)[2] & (1 << 2), 0);
    for leaf in [0, 1, 0x4000_0000, 0x4000_0001, 0x8000_0000, 0x8000_0001] {
        for subleaf in [1, 2, 31, u32::MAX] {
            assert_eq!(cpuid(leaf, subleaf), cpuid(leaf, 0));
        }
    }
}

#[test]
fn unsupported_leaves_never_forward_host_data() {
    for leaf in [
        2,
        7,
        0xb,
        0x1f,
        0x3fff_ffff,
        0x4000_0002,
        0x4000_0010,
        0x7fff_ffff,
        0x8000_0002,
        0x8000_0008,
        0x8000_000a,
        u32::MAX,
    ] {
        for subleaf in [0, 1, u32::MAX] {
            assert_eq!(cpuid(leaf, subleaf), [0; 4]);
        }
    }
}

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
