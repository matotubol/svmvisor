#![cfg(feature = "native-preflight")]

use svmvisor_dxe::native::{
    admission::boundary::NativeBoundary,
    resident::launch::{
        Mtrrs, backing_aliases, common_backing_offset, directory_valid, native_paging_config,
        xstate_valid,
    },
};
use svmvisor_hypervisor::host::resident::{
    DIRECTORY_VERSION, MAX_RESIDENT_CPUS, ResidentDirectory, X2AVIC_BACKING_ALIASES_OFFSET,
    X2AVIC_TABLE_OFFSET,
};

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

/// One dense pool of identical relocated images, with this machine's IDs.
fn pool(count: u64) -> Vec<ResidentDirectory> {
    (0..count)
        .map(|slot| {
            let mut d = directory(0x200000 + slot * 0x100000);
            d.pool_base = 0x200000;
            d.pool_bytes = count * 0x100000;
            d.cpu_slot = slot;
            d.apic_id = if slot < 12 { slot } else { slot + 4 };
            d
        })
        .collect()
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

#[test]
fn relocated_directory_binds_disjoint_writable_objects_and_code_functions() {
    for base in [0x100000, 0x280000, 0x3ff00000] {
        let d = directory(base);
        assert!(directory_valid(&d, base));
        assert!(!directory_valid(&d, base + 4096));
    }
}

#[test]
fn image_may_end_at_but_not_cross_the_remote_backing_alias_range() {
    for base in [0x200000, 0x3ff00000] {
        let mut d = directory(base);
        d.memory_end = base + X2AVIC_BACKING_ALIASES_OFFSET;
        assert!(directory_valid(&d, base));
        d.memory_end += 4096;
        assert!(!directory_valid(&d, base));
        // The Phase A bound, the shared physical-ID table, is no longer enough.
        d.memory_end = base + X2AVIC_TABLE_OFFSET;
        assert!(!directory_valid(&d, base));
    }
}

#[test]
fn host_apic_ids_stop_below_unresolved_x2avic_entry_255() {
    let base = 0x200000;
    // This machine's IDs, and the doorbell/table-safe maximum.
    for id in (0..=11).chain(16..=27).chain([254]) {
        let mut d = directory(base);
        d.apic_id = id;
        assert!(directory_valid(&d, base), "id {id}");
    }
    for id in [255, 256, 511, u64::MAX] {
        let mut d = directory(base);
        d.apic_id = id;
        assert!(!directory_valid(&d, base), "id {id}");
    }
}

#[test]
fn every_private_root_plans_one_alias_per_pool_slot_and_no_more() {
    for count in [1u64, 2, 24, 32] {
        let p = pool(count);
        assert_eq!(common_backing_offset(&p), Some(0x23000));
        for slot in [0, count as usize / 2, count as usize - 1] {
            let base = p[slot].arena_base;
            let plan: Vec<_> = backing_aliases(&p, slot).unwrap().collect();
            assert_eq!(plan.len(), MAX_RESIDENT_CPUS);
            for (s, &(alias, target)) in plan.iter().enumerate() {
                assert_eq!(alias, base + 0xd4000 + s as u64 * 4096);
                assert!(alias >= p[slot].memory_end && alias < base + X2AVIC_TABLE_OFFSET);
                // Slot s's page: pool base + s MiB + the common image offset.
                assert_eq!(target, p.get(s).map(|other| other.avic_backing));
                assert_eq!(target, (s < count as usize).then(|| 0x223000 + s as u64 * 0x100000));
            }
        }
        assert!(backing_aliases(&p, count as usize).is_none());
    }
}

#[test]
fn backing_page_may_be_the_last_image_page_below_the_aliases() {
    let mut p = pool(24);
    for d in &mut p {
        d.memory_end = d.arena_base + X2AVIC_BACKING_ALIASES_OFFSET;
        d.avic_backing = d.memory_end - 4096;
    }
    assert_eq!(common_backing_offset(&p), Some(0xd3000));
    let plan: Vec<_> = backing_aliases(&p, 5).unwrap().collect();
    assert_eq!(
        plan[23],
        (0x700000 + 0xd4000 + 23 * 4096, Some(0x200000 + 23 * 0x100000 + 0xd3000))
    );
    assert_eq!(plan[24].1, None);
}

#[test]
fn disagreeing_or_incomplete_pools_have_no_alias_plan() {
    let original = pool(24);
    for which in 0..9 {
        let mut p = original.clone();
        match which {
            // A valid, disjoint backing page at a different image offset.
            0 => p[7].avic_backing += 4096,
            1 => p[0].avic_backing -= 4096,
            2 => p.swap(3, 4),
            3 => p[5].cpu_slot = 6,
            4 => p[9].pool_base = 0x400000,
            5 => p.truncate(23),
            6 => p.iter_mut().for_each(|d| d.pool_bytes = 32 * 0x100000),
            7 => p[2].avic_backing = p[2].memory_end,
            _ => p[11].version -= 1,
        }
        assert_eq!(common_backing_offset(&p), None, "case {which}");
        assert!(backing_aliases(&p, 0).is_none(), "case {which}");
    }
    assert_eq!(common_backing_offset(&[]), None);
    let mut oversized = pool(32);
    let mut extra = oversized[31];
    extra.arena_base += 0x100000;
    oversized.push(extra);
    assert_eq!(common_backing_offset(&oversized), None);
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
    for which in 0..14 {
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
            12 => d.version = 9, // Pre-alias layout: no remote backing aliases.
            _ => d.memory_end = d.arena_base + 0xfe001,
        }
        assert!(!directory_valid(&d, original.arena_base), "case {which}");
    }
    assert!(!directory_valid(&original, u64::MAX));
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
