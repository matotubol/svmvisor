use svmvisor_hypervisor::memory::mtrrs::Mtrrs;

#[test]
fn fixed_wb_requires_visible_enabled_dram_routing() {
    for controls in 0..4u64 {
        for byte in 0..=255u8 {
            assert_eq!(Mtrrs::native_fixed_page_is_wb(controls << 18, byte),
                controls == 3 && byte == 0x1e);
        }
    }
    for bit in (0..18).chain(23..64) {
        assert!(!Mtrrs::native_fixed_page_is_wb(0xc0000 | (1 << bit), 0x1e));
    }
    assert!(Mtrrs::native_fixed_page_is_wb(0x7c0000, 0x1e));
}

#[test]
fn terminal_uc_during_disabled_mtrrs_does_not_admit_wb_reads() {
    let mut mt = Mtrrs { default: 6, count: 0, variable: [(0,0);16],
        physical_bits: 48, tom2_default: None };
    for pat in 0..=255 {
        assert_eq!(mt.terminal_page_is_uc(0x100000, pat), matches!(pat,0|4|5|6|7));
    }
    assert!(!mt.page_is_wb(0x100000));
    assert!(!mt.page_is_uc(0x100000, 0));
    for page in [0,0xff000,0x100001,1<<48] {
        assert!(!mt.terminal_page_is_uc(page, 0));
    }
    for default in [2,3,7,0x100,0x1000,u64::MAX] {
        mt.default=default;
        assert!(!mt.terminal_page_is_uc(0x100000,0));
    }
    mt.default=0x806;
    assert!(!mt.terminal_page_is_uc(0x100000,0));
    assert!(mt.page_is_wb(0x100000));
}
