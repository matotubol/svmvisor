#![cfg(feature = "native-preflight")]
use svmvisor_dxe::native::{
    admission::boundary::NativeBoundary,
    resident::launch::{Mtrrs, directory_valid, native_paging_config, xstate_valid},
};
use svmvisor_hypervisor::host::resident::{ResidentDirectory, DIRECTORY_VERSION};

#[test]
fn current_pcid_tags_select_the_same_root_without_becoming_cache_bits() {
    for fsgsbase in [0, 1 << 16] {
        for pcid in [0, 1, 0x18, 0xfff] {
            let cfg = native_paging_config(
                0x80010033,
                0x1000 | pcid,
                0x620 | fsgsbase | (1 << 17),
                48,
                true,
            )
            .unwrap();
            assert_eq!(cfg.cr3, 0x1000 | pcid);
            assert!(cfg.pcid);
            let translated =
                svmvisor_hypervisor::host::paging::translate(cfg, 0x123, |address| match address {
                    0x1000 => Some(0x2003),
                    0x2000 => Some(0x3003),
                    0x3000 => Some(0x4003),
                    0x4000 => Some(0x5003),
                    _ => None,
                })
                .unwrap();
            assert_eq!(translated.physical_address, 0x5123);
            assert!(!translated.user);
        }
    }
    let cfg = native_paging_config(0x80010033, 0x1000, 0x10620, 48, false).unwrap();
    assert!(!cfg.pcid);
    assert!(!cfg.nxe);
    for low in [1, 8, 16, 0xfff] {
        assert!(native_paging_config(0x80010033, 0x1000 | low, 0x10620, 48, false).is_none());
    }
}

#[test]
fn current_controls_still_refuse_unowned_protection_and_paging_modes() {
    for bit in [11, 12, 13, 14, 15, 19, 20, 21, 22, 23, 24, 63] {
        assert!(
            native_paging_config(0x80010033, 0x1000, 0x70620 | (1 << bit), 48, true).is_none(),
            "CR4 bit {bit}"
        );
    }
    for cr0 in [0x10033, 0xe0010033] {
        assert!(native_paging_config(cr0, 0x1000, 0x70620, 48, true).is_none());
    }
    for cr4 in [0x70600, 0x70420] {
        assert!(native_paging_config(0x80010033, 0x1000, cr4, 48, true).is_none());
    }
}

fn directory(base: u64) -> ResidentDirectory {
    ResidentDirectory {
        version: DIRECTORY_VERSION,
        arena_base: base,
        arena_bytes: 0x100000,
        context: base + 0x10000,
        vmcb: base + 0x11000,
        auxiliary: base + 0x12000,
        registers: base + 0x10100,
        npt: base + 0x13000,
        arm: base + 16,
        enter: base + 32,
        text_end: base + 0x8000,
        data_start: base + 0x10000,
        memory_end: base + 0x30000,
        pool_base: base,
        pool_bytes: 0x100000,
        cpu_slot: 0,
        apic_id: 0,
        avic_backing: base + 0x23000,
        reserved: [0; 2],
    }
}

#[test]
fn relocated_directory_binds_disjoint_writable_objects_and_code_functions() {
    for base in [0x100000, 0x280000, 0x3ff00000] {
        let d = directory(base);
        assert!(directory_valid(&d, base));
        assert!(!directory_valid(&d, base + 4096));
    }
}
#[test]
fn directory_binds_each_cpu_copy_to_the_complete_retained_pool() {
    for count in [2u64, 24, 32] {
        for slot in 0..count {
            let base = 0x200000 + slot * 0x100000;
            let mut d = directory(base);
            d.pool_base = 0x200000;
            d.pool_bytes = count * 0x100000;
            d.cpu_slot = slot;
            d.apic_id = slot;
            assert!(directory_valid(&d, base));
            d.cpu_slot = count;
            assert!(!directory_valid(&d, base));
            d.cpu_slot = slot;
            d.pool_bytes = 0x2100000;
            assert!(!directory_valid(&d, base));
        }
    }
}

#[test]
fn directory_rejects_overlaps_oob_alignment_code_data_confusion_and_overflow() {
    let original = directory(0x200000);
    for which in 0..13 {
        let mut d = original;
        match which {
            0 => d.auxiliary = d.vmcb,
            1 => d.npt = d.memory_end - 4096,
            2 => d.registers = d.context + 16,
            3 => d.vmcb += 1,
            4 => d.arm = d.data_start,
            5 => d.enter = d.arena_base - 1,
            6 => d.context = d.text_end,
            7 => d.memory_end = d.arena_base + 0x100000,
            8 => d.reserved[1] = 1,
            9 => d.version = 1,
            10 => d.version = 4,
            11 => d.version = 5, // Pre-initial-ICR arm ABI must never be called.
            _ => d.memory_end = d.arena_base + 0xfe001,
        }
        assert!(!directory_valid(&d, original.arena_base), "case {which}");
    }
    assert!(!directory_valid(&original, u64::MAX));
}

fn fx() -> Box<NativeBoundary> {
    // Test-only inert representation; all fields are integer byte records.
    let mut b: Box<NativeBoundary> = Box::new(unsafe { core::mem::zeroed() });
    b.abi_version = 1;
    b.xstate_size = 512;
    b.xstate[0..2].copy_from_slice(&0x37fu16.to_le_bytes());
    b.xstate[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
    b
}

#[test]
fn amd_mxcsr_capability_bit_is_not_reserved_capture_state() {
    let mut b = fx();
    // Observed with user-mode FXSAVE on Ryzen 9900X, signature 00b40f40.
    // APM1 3.24 4.2.2: MASK.MM is set independently of current MXCSR.MM.
    b.xstate[28..32].copy_from_slice(&0x2ffffu32.to_le_bytes());
    assert!(xstate_valid(&b));
    b.xstate[24..28].copy_from_slice(&0x21f80u32.to_le_bytes());
    assert!(xstate_valid(&b));
    b.xstate[28..32].copy_from_slice(&0xffffu32.to_le_bytes());
    assert!(!xstate_valid(&b));
    b.xstate[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
    for unsupported in [1 << 16, 1 << 18, 1 << 31] {
        b.xstate[28..32].copy_from_slice(&(0x2ffffu32 | unsupported).to_le_bytes());
        assert!(!xstate_valid(&b));
    }
}

#[test]
fn xstate_refuses_pending_x87_and_reserved_mxcsr_without_changing_capture() {
    let mut b = fx();
    assert!(xstate_valid(&b));
    b.xstate[2] = 0x80;
    assert!(!xstate_valid(&b));
    assert_eq!(b.xstate[2], 0x80);
    b.xstate[2] = 0;
    b.xstate[26] = 1;
    assert!(!xstate_valid(&b));
    b.xstate[26] = 0;
    b.xstate[24] |= 0x40; // DAZ without a mask indicating support.
    assert!(!xstate_valid(&b));
    b.xstate[28..32].copy_from_slice(&0xffffu32.to_le_bytes());
    assert!(xstate_valid(&b));
}

#[test]
fn xsave_refuses_compaction_supervisor_and_uncaptured_components() {
    let mut b = fx();
    b.profile = 3;
    b.xcr0 = 3;
    b.captured_fields = 1;
    b.supported_xcr0 = 3;
    b.cr4 = 1 << 18;
    b.leaf1_ecx = 3 << 26;
    b.xstate_size = 576;
    b.xstate[512] = 3;
    assert!(xstate_valid(&b));
    for offset in [512, 520, 527, 575] {
        let old = b.xstate[offset];
        b.xstate[offset] |= 4;
        assert!(!xstate_valid(&b));
        b.xstate[offset] = old;
    }
}

fn mtrrs() -> Mtrrs {
    Mtrrs {
        default: 0x806,
        count: 0,
        variable: [(0, 0); 16],
        physical_bits: 48,
        tom2_default: None,
    }
}
fn range(base: u64, bytes: u64, ty: u64) -> (u64, u64) {
    (base | ty, (((1u64 << 48) - 1) & !(bytes - 1)) | 0x800)
}

#[test]
fn mtrr_wb_requires_enabled_memory_and_rejects_any_conflicting_overlap() {
    let mut m = mtrrs();
    assert!(m.page_is_wb(0x400000));
    assert!(!m.page_is_wb(0xff000));
    m.default = 6;
    assert!(!m.page_is_wb(0x400000));
    m.default = 0x800;
    m.count = 1;
    m.variable[0] = range(0x400000, 0x200000, 6);
    assert!(m.page_is_wb(0x400000));
    assert!(m.page_is_wb(0x5ff000));
    assert!(!m.page_is_wb(0x600000));
    m.count = 2;
    m.variable[1] = range(0x500000, 0x100000, 0);
    assert!(m.page_is_wb(0x4ff000));
    assert!(!m.page_is_wb(0x500000));
}

#[test]
fn mtrr_rejects_noncontiguous_masks_reserved_bits_and_misaligned_ranges() {
    let mut m = mtrrs();
    m.count = 1;
    m.variable[0] = range(0x400000, 0x200000, 6);
    m.variable[0].1 ^= 1 << 24;
    assert!(!m.page_is_wb(0x400000));
    m.variable[0] = range(0x500000, 0x200000, 6);
    assert!(!m.page_is_wb(0x400000));
    m.variable[0] = range(0x400000, 0x200000, 6);
    m.variable[0].1 |= 1 << 60;
    assert!(!m.page_is_wb(0x400000));
}
