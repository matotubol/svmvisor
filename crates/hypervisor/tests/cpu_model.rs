use svmvisor_hypervisor::{
    arch::x86_64::xstate::{XstateCapabilities, XstateLayout},
    svm::cpu_model::{
        AmdCpuModel, CpuIdentity, CpuIdentityError, CpuModelError, GuestCpuState,
        HostCacheEvidence, HostCpuEvidence, MAX_BASIC_LEAF, MAX_EXTENDED_LEAF, RuntimeCpuContract,
    },
};

// Supplied test identity/cache observations, not hardcoded implementation data.
const SIGNATURE: u32 = 0x0080_0f10;
const BRAND: &[u8] = b"AMD CPU model test fixture";

fn host() -> HostCpuEvidence {
    HostCpuEvidence {
        vendor: *b"AuthenticAMD",
        max_basic: 0x20,
        max_extended: 0x8000_0026,
        leaf1_ecx: u32::MAX,
        leaf1_edx: u32::MAX,
        leaf7_ebx: u32::MAX,
        leaf7_ecx: u32::MAX,
        leaf7_edx: u32::MAX,
        extended1_ecx: u32::MAX,
        extended1_edx: u32::MAX,
        extended8_ebx: u32::MAX,
        extended21_eax: 0,
        extended21_ebx: 0,
        address_sizes: 0x3028,
        clflush_bytes: 64,
        caches: caches(),
    }
}

fn caches() -> HostCacheEvidence {
    HostCacheEvidence {
        legacy_l1: [0xff20_ff20, 0xff20_ff20, 0x2008_0140, 0x2008_0140],
        legacy_l2_l3: [0x4080_4080, 0x4080_4080, 0x0200_6140, 0x0010_6140],
        deterministic: [
            [0x121, (7 << 22) | 63, 63, 0],
            [0x122, (7 << 22) | 63, 63, 0],
            [0x143, (7 << 22) | 63, 1023, 0],
            [0x163 | (23 << 14), (7 << 22) | 63, 4095, 1],
            [0; 4],
            [0; 4],
            [0; 4],
            [0; 4],
        ],
        deterministic_count: 4,
    }
}

fn contract(mask: u64) -> RuntimeCpuContract {
    RuntimeCpuContract {
        identity: identity(SIGNATURE, BRAND),
        xstate: layout(mask),
        physical_address_bits: 40,
        tsc: true,
        rdtscp: true,
        nx: true,
    }
}

fn identity(signature: u32, brand: &[u8]) -> CpuIdentity {
    CpuIdentity::from_leaves(
        vendor_leaf(0x20),
        Some([signature, 0, 0, 0]),
        vendor_leaf(0x8000_0026),
        Some([signature, 0, 0, 0]),
        Some(brand_leaves(brand)),
    )
    .unwrap()
}

fn vendor_leaf(max: u32) -> [u32; 4] {
    [max, 0x6874_7541, 0x444d_4163, 0x6974_6e65]
}

fn brand_leaves(bytes: &[u8]) -> [[u32; 4]; 3] {
    let mut padded = [0; 48];
    padded[..bytes.len()].copy_from_slice(bytes);
    let mut leaves = [[0; 4]; 3];
    for (leaf, words) in leaves.iter_mut().enumerate() {
        for (word, value) in words.iter_mut().enumerate() {
            let offset = leaf * 16 + word * 4;
            *value = u32::from_le_bytes(padded[offset..offset + 4].try_into().unwrap());
        }
    }
    leaves
}

fn layout(mask: u64) -> XstateLayout {
    XstateLayout::detect(XstateCapabilities {
        leaf1_edx: u32::MAX,
        leaf1_ecx: if mask == 0 { 0 } else { (1 << 26) | if mask == 7 { 1 << 28 } else { 0 } },
        supported_xcr0: mask,
        enabled_size: 832,
        max_size: 832,
        avx_size: 256,
        avx_offset: 576,
        avx_flags: 0,
    })
    .unwrap()
}

fn query(model: AmdCpuModel, leaf: u32, subleaf: u32) -> [u32; 4] {
    model
        .cpuid(leaf, subleaf, state(if model.uses_xsave() { model.xcr0_mask() } else { 0 }))
        .unwrap()
}

fn state(mask: u64) -> GuestCpuState {
    GuestCpuState { vcpu_id: 0, cr4: if mask == 0 { 0 } else { 1 << 18 }, xcr0: mask }
}

#[test]
fn native_identity_has_amd_vendor_in_both_namespaces_and_complete_brand() {
    let model = AmdCpuModel::admit(host(), contract(7)).unwrap();
    for (leaf, max) in [(0, MAX_BASIC_LEAF), (0x8000_0000, MAX_EXTENDED_LEAF)] {
        let words = query(model, leaf, u32::MAX);
        assert_eq!(words[0], max);
        let vendor: Vec<u8> =
            [words[1], words[3], words[2]].into_iter().flat_map(u32::to_le_bytes).collect();
        assert_eq!(vendor, b"AuthenticAMD");
    }
    assert_eq!(query(model, 1, 0)[0], SIGNATURE);
    assert_eq!(query(model, 0x8000_0001, 0)[0], SIGNATURE);
    let brand: Vec<u8> = (0x8000_0002..=0x8000_0004)
        .flat_map(|leaf| query(model, leaf, 0))
        .flat_map(u32::to_le_bytes)
        .collect();
    assert_eq!(&brand[..BRAND.len()], BRAND);
    assert!(brand[BRAND.len()..].iter().all(|&b| b == 0));
    assert_eq!(query(model, 1, 0)[2] >> 31, 0);
    for leaf in [0x4000_0000, 0x4000_0001, 0x4000_0100, 0x4fff_ffff] {
        assert_eq!(query(model, leaf, 0), [0; 4]);
    }
}

#[test]
fn two_cores_and_cache_sharing_are_consistent_across_all_leaves() {
    let model = AmdCpuModel::admit(host(), contract(7)).unwrap();
    for id in 0..2 {
        let guest = GuestCpuState { vcpu_id: id, ..state(7) };
        let basic = model.cpuid(1, 0, guest).unwrap();
        assert_eq!(basic[1] >> 24, id);
        assert_eq!((basic[1] >> 16) & 0xff, 2);
        assert_ne!(basic[3] & (1 << 28), 0);
        assert_eq!(model.cpuid(0x0b, 0, guest).unwrap(), [0, 1, 0x100, id]);
        assert_eq!(model.cpuid(0x0b, 1, guest).unwrap(), [1, 2, 0x201, id]);
        for subleaf in [2, 3, 255, 256, u32::MAX] {
            assert_eq!(model.cpuid(0x0b, subleaf, guest).unwrap(), [0, 0, subleaf & 255, id]);
        }
        assert_eq!(model.cpuid(0x8000_0008, 0, guest).unwrap(), [0x3028, 1, 0x1001, 0]);
        assert_eq!(model.cpuid(0x8000_001e, 0, guest).unwrap(), [0, id, 0, 0]);
    }
    assert_eq!(
        model.cpuid(0, 0, GuestCpuState { vcpu_id: 2, ..state(7) }),
        Err(CpuModelError::InvalidVcpuId)
    );
    let sizes = [32 * 1024, 32 * 1024, 512 * 1024, 2 * 1024 * 1024];
    for (index, size) in sizes.into_iter().enumerate() {
        let leaf = query(model, 0x8000_001d, index as u32);
        let decoded = ((leaf[1] & 0xfff) + 1)
            * (((leaf[1] >> 12) & 0x3ff) + 1)
            * ((leaf[1] >> 22) + 1)
            * (leaf[2] + 1);
        assert_eq!(decoded, size);
        assert_eq!(((leaf[0] >> 14) & 0xfff) + 1, if index == 3 { 2 } else { 1 });
        assert_eq!(leaf[0] >> 26, 0, "AMD reserved, not Intel max cores");
        assert_eq!(leaf[3] & !3, 0, "AMD has no Intel complex-index bit");
    }
    let l1 = query(model, 0x8000_0005, 0);
    let l2_l3 = query(model, 0x8000_0006, 0);
    assert_eq!(l1[..2], [0xff20_ff20; 2]);
    assert_eq!(l2_l3[..2], [0x4080_4080; 2]);
    assert_eq!(l1[2] >> 24, 32);
    assert_eq!(l1[3] >> 24, 32);
    assert_eq!(l2_l3[2] >> 16, 512);
    assert_eq!(l2_l3[3] >> 18, 4);
}

#[test]
fn xsave_avx_and_osxsave_follow_owned_layout_and_stopped_state() {
    for mask in [0, 3, 7] {
        let model = AmdCpuModel::admit(host(), contract(mask)).unwrap();
        let basic = query(model, 1, 0);
        assert_eq!(basic[2] & (1 << 26) != 0, mask != 0);
        assert_eq!(basic[2] & (1 << 28) != 0, mask == 7);
        assert_eq!(basic[2] & (1 << 12) != 0, mask == 7);
        assert_eq!(basic[2] & (1 << 29) != 0, mask == 7);
        assert_eq!(query(model, 7, 0)[1] & (1 << 5) != 0, mask == 7);
        if mask == 0 {
            for subleaf in [0, 1, 2, 3, 63, 64, u32::MAX] {
                assert_eq!(query(model, 0x0d, subleaf), [0; 4]);
            }
            assert_eq!(model.cpuid(1, 0, state(1)), Err(CpuModelError::InvalidGuestXstate));
            continue;
        }
        for xcr0 in [1, 3, 7].into_iter().filter(|&x| x & !mask == 0) {
            for cr4 in [0, 1 << 18] {
                let guest = GuestCpuState { cr4, xcr0, ..state(mask) };
                assert_eq!(model.cpuid(1, 0, guest).unwrap()[2] & (1 << 27) != 0, cr4 != 0);
                assert_eq!(
                    model.cpuid(0x0d, 0, guest).unwrap(),
                    [
                        mask as u32,
                        if xcr0 & 4 == 0 { 576 } else { 832 },
                        if mask == 7 { 832 } else { 576 },
                        0
                    ]
                );
            }
        }
        assert_eq!(query(model, 0x0d, 1), [0; 4]);
        assert_eq!(query(model, 0x0d, 2), if mask == 7 { [256, 576, 0, 0] } else { [0; 4] });
        for invalid in [0, 2, 4, 5, 6, 8, 0x8000_0000_0000_0001] {
            assert_eq!(
                model.cpuid(0, 0, GuestCpuState { xcr0: invalid, ..state(mask) }),
                Err(CpuModelError::InvalidGuestXstate)
            );
        }
    }
}

#[test]
fn no_host_stateful_features_leak_into_the_admitted_model() {
    let model = AmdCpuModel::admit(host(), contract(7)).unwrap();
    let basic = query(model, 1, 0);
    // APIC, SYSENTER, MTRR, PAT, machine check and debug extensions.
    assert_eq!(
        basic[3]
            & ((1 << 1)
                | (1 << 2)
                | (1 << 7)
                | (1 << 9)
                | (1 << 11)
                | (1 << 12)
                | (1 << 14)
                | (1 << 16)),
        0
    );
    assert_eq!(basic[2] & ((1 << 3) | (1 << 17) | (1 << 21) | (1 << 31)), 0);
    let ext = query(model, 0x8000_0001, 0);
    assert_eq!(ext[2] & ((1 << 2) | (1 << 10) | (1 << 15)), 0);
    assert_eq!(ext[3] & ((1 << 11) | (1 << 26)), 0);
    let structured = query(model, 7, 0);
    assert_eq!(
        structured[1],
        [3, 5, 8, 9, 18, 19, 23, 24, 29].into_iter().fold(0, |mask, bit| mask | (1 << bit))
    );
    assert_eq!(structured[2], (1 << 8) | (1 << 9) | (1 << 10) | (1 << 22));
    assert_eq!(structured[3], 1 << 4);
    let duplicates = 0x0183_f3ff;
    assert_eq!(ext[3] & duplicates, basic[3] & duplicates);
}

#[test]
fn every_reserved_and_unsupported_namespace_has_bounded_zero_behavior() {
    let model = AmdCpuModel::admit(host(), contract(7)).unwrap();
    for leaf in (2..=6)
        .chain(8..=10)
        .chain([12])
        .chain(14..=0x30)
        .chain([0x8000_0007])
        .chain(0x8000_0009..=0x8000_001c)
        .chain(0x8000_001f..=0x8000_0040)
        .chain([0x3fff_ffff, 0x7fff_ffff, 0x9000_0000, 0xc000_0000, u32::MAX])
    {
        for subleaf in [0, 1, 2, 63, 64, u32::MAX] {
            assert_eq!(query(model, leaf, subleaf), [0; 4], "{leaf:08x}:{subleaf:x}");
        }
    }
    for subleaf in [1, 2, 64, u32::MAX] {
        assert_eq!(query(model, 7, subleaf), [0; 4]);
    }
    for subleaf in [3, 4, 63, 64, u32::MAX] {
        assert_eq!(query(model, 0x0d, subleaf), [0; 4]);
    }
    for subleaf in [4, 5, 64, u32::MAX] {
        assert_eq!(query(model, 0x8000_001d, subleaf), [0; 4]);
    }
    for leaf in [
        0,
        1,
        0x8000_0000,
        0x8000_0001,
        0x8000_0002,
        0x8000_0003,
        0x8000_0004,
        0x8000_0005,
        0x8000_0006,
        0x8000_0008,
        0x8000_001e,
    ] {
        assert_eq!(query(model, leaf, u32::MAX), query(model, leaf, 0));
    }
}

#[test]
fn admission_refuses_missing_host_evidence_and_unbacked_dependencies() {
    let runtime = contract(7);
    let mut evidence = host();
    evidence.vendor = *b"GenuineIntel";
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::UnsupportedVendor));
    for bit in [0, 5, 6, 8, 15, 23, 24, 25, 26] {
        evidence = host();
        evidence.leaf1_edx &= !(1 << bit);
        assert_eq!(
            AmdCpuModel::admit(evidence, runtime),
            Err(CpuModelError::MissingBaselineFeatures)
        );
    }
    evidence = host();
    evidence.extended1_edx &= !(1 << 29);
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::MissingBaselineFeatures));
    evidence = host();
    evidence.max_extended = 0x8000_0007;
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::MissingHostLeaves));
    evidence = host();
    evidence.max_basic = 0;
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::MissingHostLeaves));
    for bits in [0, 31, 39, 41, 49, 53] {
        assert_eq!(
            AmdCpuModel::admit(
                host(),
                RuntimeCpuContract { physical_address_bits: bits, ..runtime }
            ),
            Err(CpuModelError::InvalidAddressWidth)
        );
    }
    evidence = host();
    evidence.address_sizes = 0x2030;
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::InvalidAddressWidth));
    evidence = host();
    evidence.clflush_bytes = 128;
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::InvalidClflushSize));
    for bit in [26, 28] {
        evidence = host();
        evidence.leaf1_ecx &= !(1 << bit);
        assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::XstateNotSupported));
    }
    evidence = host();
    evidence.max_basic = 7;
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::XstateNotSupported));
    evidence = host();
    evidence.extended1_edx &= !(1 << 20);
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::NxNotSupported));
    evidence = host();
    evidence.extended1_edx &= !(1 << 27);
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::ClockNotSupported));
    evidence = host();
    evidence.leaf1_edx &= !(1 << 4);
    assert_eq!(AmdCpuModel::admit(evidence, runtime), Err(CpuModelError::ClockNotSupported));
    assert_eq!(
        AmdCpuModel::admit(host(), RuntimeCpuContract { tsc: false, ..runtime }),
        Err(CpuModelError::ClockNotSupported)
    );
}

#[test]
fn optional_features_require_host_evidence_and_explicit_runtime_owners() {
    let mut evidence = host();
    evidence.leaf1_ecx = 0;
    evidence.extended1_ecx = 0;
    evidence.caches.deterministic_count = 0;
    evidence.caches.deterministic = [[0; 4]; 8];
    evidence.leaf7_ebx = 0;
    evidence.leaf7_ecx = 0;
    evidence.leaf7_edx = 0;
    evidence.extended8_ebx = 0;
    evidence.leaf1_edx &= !(1 << 19);
    evidence.clflush_bytes = 0;
    let model = AmdCpuModel::admit(
        evidence,
        RuntimeCpuContract { tsc: false, rdtscp: false, nx: false, ..contract(0) },
    )
    .unwrap();
    assert_eq!(query(model, 1, 0)[2], 0);
    assert_eq!(query(model, 1, 0)[1] & 0xff00, 0);
    assert_eq!(query(model, 1, 0)[3] & (1 << 4), 0);
    assert_eq!(query(model, 7, 0), [0; 4]);
    assert_eq!(query(model, 0x8000_0001, 0)[2], 1 << 1);
    assert_eq!(query(model, 0x8000_0001, 0)[3] & ((1 << 4) | (1 << 20) | (1 << 27)), 0);
}

#[test]
fn xstate_component_metadata_matches_the_actual_admitted_standard_layout() {
    let layout = XstateLayout::detect(XstateCapabilities {
        leaf1_edx: u32::MAX,
        leaf1_ecx: u32::MAX,
        supported_xcr0: 7,
        enabled_size: 1280,
        max_size: 1280,
        avx_size: 256,
        avx_offset: 1024,
        avx_flags: 2,
    })
    .unwrap();
    let model =
        AmdCpuModel::admit(host(), RuntimeCpuContract { xstate: layout, ..contract(7) }).unwrap();
    assert_eq!(query(model, 0x0d, 2), [256, 1024, 2, 0]);
    assert_eq!(query(model, 0x0d, 0), [7, 1280, 1280, 0]);
    assert_eq!(
        model.cpuid(0x0d, 0, GuestCpuState { xcr0: 1, ..state(7) }).unwrap(),
        [7, 576, 1280, 0]
    );
}

#[test]
fn optional_native_instruction_groups_have_exact_dependency_gates() {
    let vector_extended = (1 << 11) | (1 << 16); // XOP, FMA4.
    let vector_structured = (1 << 8) | (1 << 9) | (1 << 10); // GFNI, VAES, VPCLMUL.
    for mask in [0, 3, 7] {
        let model = AmdCpuModel::admit(host(), contract(mask)).unwrap();
        assert_eq!(
            query(model, 0x8000_0001, 0)[2] & vector_extended,
            if mask == 7 { vector_extended } else { 0 }
        );
        assert_eq!(
            query(model, 7, 0)[2] & vector_structured,
            if mask == 7 { vector_structured } else { 0 }
        );
        assert_ne!(query(model, 1, 0)[2] & (1 << 30), 0); // RDRAND.
        assert_ne!(query(model, 7, 0)[1] & (1 << 18), 0); // RDSEED.
        assert_eq!(query(model, 0x8000_0001, 0)[2] & ((1 << 8) | (1 << 21)), (1 << 8) | (1 << 21));
        assert_eq!(
            query(model, 0x8000_0001, 0)[3] & ((1 << 22) | (3 << 30)),
            (1 << 22) | (3 << 30)
        );
    }
    let model =
        AmdCpuModel::admit(host(), RuntimeCpuContract { rdtscp: false, ..contract(7) }).unwrap();
    assert_eq!(query(model, 7, 0)[2] & (1 << 22), 0); // RDPID requires owned AUX.
    let mut missing = host();
    missing.leaf1_edx &= !(1 << 19);
    missing.clflush_bytes = 0;
    let model = AmdCpuModel::admit(missing, contract(7)).unwrap();
    assert_eq!(query(model, 7, 0)[1] & ((1 << 23) | (1 << 24)), 0);
    assert_eq!(query(model, 0x8000_0008, 0)[1], 0);
    missing = host();
    missing.extended1_edx &= !(1 << 31);
    let model = AmdCpuModel::admit(missing, contract(7)).unwrap();
    assert_eq!(query(model, 0x8000_0001, 0)[3] & (3 << 30), 0);
    missing = host();
    missing.leaf1_ecx &= !(1 << 30);
    missing.leaf7_ebx = 0;
    missing.leaf7_ecx = 0;
    missing.leaf7_edx = 0;
    missing.extended1_ecx = 0;
    missing.caches.deterministic_count = 0;
    missing.caches.deterministic = [[0; 4]; 8];
    missing.extended1_edx &= !((1 << 22) | (3 << 30));
    missing.extended8_ebx = 0;
    let model = AmdCpuModel::admit(missing, contract(7)).unwrap();
    assert_eq!(query(model, 1, 0)[2] & (1 << 30), 0);
    assert_eq!(query(model, 7, 0), [0; 4]);
    assert_eq!(query(model, 0x8000_0001, 0)[2], 1 << 1);
    assert_eq!(query(model, 0x8000_0001, 0)[3] & ((1 << 22) | (3 << 30)), 0);
    assert_eq!(query(model, 0x8000_0008, 0)[1], 0);
}

#[test]
fn captured_identity_preserves_all_bytes_without_importing_features_or_topology() {
    let raw_brand = *b"0123456789ABCDEFfedcba9876543210ABCDEFGHIJKLMNOP";
    let native = identity(0x00b4_0f40, &raw_brand);
    assert_eq!(native.vendor(), *b"AuthenticAMD");
    assert_eq!(native.signature(), native.extended_signature());
    assert_eq!(native.family_model_stepping(), (0x1a, 0x44, 0));
    let model =
        AmdCpuModel::admit(host(), RuntimeCpuContract { identity: native, ..contract(7) }).unwrap();
    let actual: Vec<u8> = (0x8000_0002..=0x8000_0004)
        .flat_map(|leaf| query(model, leaf, 0))
        .flat_map(u32::to_le_bytes)
        .collect();
    assert_eq!(actual, raw_brand);
    assert_eq!(query(model, 1, 0)[0], 0x00b4_0f40);
    assert_eq!(query(model, 0x8000_0001, 0)[0], 0x00b4_0f40);
    assert_eq!((query(model, 1, 0)[1] >> 16) & 255, 2);
    assert_eq!(query(model, 1, 0)[2] >> 31, 0);
    assert_eq!(query(model, 0x8000_0001, 0)[2] & (1 << 2), 0);
    assert_eq!(query(model, 0x8000_0000, 0)[0], MAX_EXTENDED_LEAF);
    assert_eq!(identity(0x0003_06a2, BRAND).family_model_stepping(), (6, 0x3a, 2));
    assert_eq!(identity(0x00f3_05a2, BRAND).family_model_stepping(), (5, 0xa, 2));
}

#[test]
fn identity_missing_and_inconsistent_observations_are_not_fabricated() {
    let basic = vendor_leaf(0x20);
    let extended = vendor_leaf(0x8000_0026);
    let sig = Some([SIGNATURE, 0, 0, 0]);
    let brand = Some(brand_leaves(BRAND));
    assert_eq!(
        CpuIdentity::from_leaves(vendor_leaf(0), sig, extended, sig, brand),
        Err(CpuIdentityError::InvalidMaxima)
    );
    assert_eq!(
        CpuIdentity::from_leaves(basic, sig, vendor_leaf(0x8000_0003), sig, brand),
        Err(CpuIdentityError::InvalidMaxima)
    );
    assert_eq!(
        CpuIdentity::from_leaves(basic, None, extended, sig, brand),
        Err(CpuIdentityError::MissingIdentityLeaves)
    );
    assert_eq!(
        CpuIdentity::from_leaves(basic, sig, extended, None, brand),
        Err(CpuIdentityError::MissingIdentityLeaves)
    );
    assert_eq!(
        CpuIdentity::from_leaves(basic, sig, extended, sig, None),
        Err(CpuIdentityError::MissingIdentityLeaves)
    );
    let mut other_vendor = extended;
    other_vendor[1] ^= 1;
    assert_eq!(
        CpuIdentity::from_leaves(basic, sig, other_vendor, sig, brand),
        Err(CpuIdentityError::InconsistentVendor)
    );
    assert_eq!(
        CpuIdentity::from_leaves(basic, sig, extended, Some([SIGNATURE ^ 1, 0, 0, 0]), brand),
        Err(CpuIdentityError::InconsistentSignature)
    );
    let bad = Some([SIGNATURE | (1 << 28), 0, 0, 0]);
    assert_eq!(
        CpuIdentity::from_leaves(basic, bad, extended, bad, brand),
        Err(CpuIdentityError::InvalidSignature)
    );
}

#[test]
fn native_cache_geometry_and_size_metadata_survive_topology_projection() {
    let mut evidence = host();
    evidence.caches.legacy_l1 = [0xff60_ff40, 0xff60_ff40, 0x300c_0140, 0x2008_0140];
    evidence.caches.legacy_l2_l3 = [0x4080_2040, 0x6080_4040, 0x0400_8140, 0x0100_9140];
    evidence.caches.deterministic[0] = [0x121 | (1 << 14), (11 << 22) | 63, 63, 0];
    evidence.caches.deterministic[2] = [0x143 | (1 << 14), (15 << 22) | 63, 1023, 2];
    evidence.caches.deterministic[3] = [0x163 | (23 << 14), (15 << 22) | 63, 32767, 1];
    evidence.extended21_eax = u32::MAX;
    evidence.extended21_ebx = 0x0010_1000;
    let model = AmdCpuModel::admit(evidence, contract(7)).unwrap();
    assert_eq!(query(model, 0x8000_0005, 0), evidence.caches.legacy_l1);
    assert_eq!(query(model, 0x8000_0006, 0), evidence.caches.legacy_l2_l3);
    assert_eq!(query(model, 0x8000_0021, 0), [1 << 14, 0x0010_1000, 0, 0]);
    for index in 0..4 {
        let native = evidence.caches.deterministic[index];
        let guest = query(model, 0x8000_001d, index as u32);
        assert_eq!(guest[0] & !(0xfff << 14), native[0] & !(0xfff << 14));
        assert_eq!(guest[1..], native[1..]);
        assert_eq!(((guest[0] >> 14) & 0xfff) + 1, if index == 3 { 2 } else { 1 });
    }
    evidence.max_extended = 0x8000_001e;
    let old = AmdCpuModel::admit(evidence, contract(7)).unwrap();
    assert_eq!(query(old, 0x8000_0000, 0)[0], 0x8000_001e);
    assert_eq!(query(old, 0x8000_0021, 0), [0; 4]);
}

#[test]
fn cache_capture_refuses_unterminated_reserved_or_missing_dependency_data() {
    let mut evidence = host();
    evidence.caches.deterministic_count = 8;
    assert_eq!(AmdCpuModel::admit(evidence, contract(7)), Err(CpuModelError::InvalidCacheEvidence));
    for (word, bit) in [(0, 26), (0, 10), (3, 2)] {
        evidence = host();
        evidence.caches.deterministic[0][word] |= 1 << bit;
        assert_eq!(
            AmdCpuModel::admit(evidence, contract(7)),
            Err(CpuModelError::InvalidCacheEvidence)
        );
    }
    evidence = host();
    evidence.caches.deterministic[4][1] = 1;
    assert_eq!(AmdCpuModel::admit(evidence, contract(7)), Err(CpuModelError::InvalidCacheEvidence));
    evidence = host();
    evidence.caches.deterministic_count = 0;
    evidence.caches.deterministic = [[0; 4]; 8];
    assert_eq!(AmdCpuModel::admit(evidence, contract(7)), Err(CpuModelError::InvalidCacheEvidence));
    evidence.extended1_ecx &= !(1 << 22);
    let no_topology = AmdCpuModel::admit(evidence, contract(7)).unwrap();
    assert_eq!(query(no_topology, 0x8000_001d, 0), [0; 4]);
    assert_eq!(query(no_topology, 0x8000_001e, 0), [0; 4]);
    assert_eq!(query(no_topology, 0x8000_0001, 0)[2] & (1 << 22), 0);
    evidence.caches.legacy_l2_l3[3] = 0x0100_9140;
    assert_eq!(AmdCpuModel::admit(evidence, contract(7)), Err(CpuModelError::InvalidCacheEvidence));
    evidence = host();
    evidence.extended21_ebx = 1 << 24;
    assert_eq!(
        AmdCpuModel::admit(evidence, contract(7)),
        Err(CpuModelError::InvalidExtendedMetadata)
    );
}
