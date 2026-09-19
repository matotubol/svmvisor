use crate::{
    arch::x86_64::msr::{HWCR_CPUID_FLT_EN, HWCR_IRPERF_EN},
    svm::cache::*,
};

#[test]
fn hwcr_uses_live_thread_state_and_preserves_every_non_counter_bit() {
    use core::cell::Cell;
    let baseline = 0x0900_6011;
    let hardware = Cell::new(baseline);
    let writes = Cell::new(0);
    for enabled in [true, true, false, false, true] {
        let requested = baseline | if enabled { HWCR_IRPERF_EN } else { 0 };
        let before = hardware.get();
        let count = writes.get();
        assert_eq!(
            access_hwcr(
                baseline,
                Some(requested),
                true,
                false,
                false,
                || hardware.get(),
                |v| {
                    writes.set(writes.get() + 1);
                    hardware.set(v);
                }
            ),
            Ok(requested)
        );
        assert_eq!(writes.get(), count + u32::from(before != requested));
        assert_eq!(hardware.get() & !HWCR_IRPERF_EN, baseline);
        assert_eq!(
            access_hwcr(
                baseline,
                None,
                true,
                false,
                false,
                || hardware.get(),
                |_| panic!("RDMSR wrote HWCR")
            ),
            Ok(requested)
        );
    }
    // Live bit30 changes by firmware/SMM are accepted as physical state;
    // no stale per-core/per-thread shadow overrides the hardware value.
    hardware.set(baseline);
    assert_eq!(
        access_hwcr(baseline, None, true, false, false, || hardware.get(), |_| panic!()),
        Ok(baseline)
    );
}

#[test]
fn hwcr_rejects_non_counter_changes_drift_and_missing_owners_before_write() {
    let baseline = 0x0900_6011;
    for bit in (0..64).filter(|&bit| bit != 30) {
        let changed = baseline ^ (1 << bit);
        assert_eq!(
            access_hwcr(
                baseline,
                Some(changed),
                true,
                false,
                false,
                || baseline,
                |_| panic!("unsupported write reached hardware")
            ),
            Err(HwcrError::UnsupportedChange { current: baseline, requested: changed })
        );
        for request in [None, Some(changed), Some(changed | HWCR_IRPERF_EN)] {
            assert_eq!(
                access_hwcr(
                    baseline,
                    request,
                    true,
                    false,
                    false,
                    || changed,
                    |_| panic!("drift reached hardware write")
                ),
                Err(HwcrError::PhysicalDrift { observed: changed, baseline })
            );
        }
    }
    assert_eq!(
        access_hwcr(
            baseline,
            Some(baseline | HWCR_IRPERF_EN),
            false,
            false,
            false,
            || baseline,
            |_| panic!()
        ),
        Err(HwcrError::UnsupportedChange {
            current: baseline,
            requested: baseline | HWCR_IRPERF_EN
        })
    );
    assert_eq!(
        access_hwcr(baseline, Some(baseline), false, false, false, || baseline, |_| panic!()),
        Ok(baseline)
    );
    assert_eq!(
        access_hwcr(
            baseline,
            None,
            true,
            true,
            false,
            || panic!("unsupported owner reached RDMSR"),
            |_| panic!()
        ),
        Err(HwcrError::PmcVirtualization)
    );
}

#[test]
fn hwcr_failed_readback_reports_side_effect_without_false_success_or_rollback() {
    use core::cell::Cell;
    let baseline = 0x10;
    let writes = Cell::new(0);
    let requested = baseline | HWCR_IRPERF_EN;
    let result = access_hwcr(
        baseline,
        Some(requested),
        true,
        false,
        false,
        || baseline,
        |value| {
            assert_eq!(value, requested);
            writes.set(writes.get() + 1);
        },
    );
    assert_eq!(result, Err(HwcrError::Readback { observed: baseline, expected: requested }));
    assert_eq!(writes.get(), 1);
    let mut shared = replay_core();
    assert_eq!(shared.read(HWCR, false), None);
    assert_eq!(shared.write(HWCR, baseline, &mut false), Err(CacheWriteError::Unsupported));
}

#[test]
fn hwcr_cpuid_fault_control_requires_owner_and_preserves_other_controls() {
    use core::cell::Cell;
    let baseline = 0x0900_6011;
    let physical = Cell::new(baseline);
    for enabled in [true, true, false] {
        let requested = baseline | if enabled { HWCR_CPUID_FLT_EN } else { 0 };
        assert_eq!(
            access_hwcr(
                baseline,
                Some(requested),
                false,
                false,
                true,
                || physical.get(),
                |v| physical.set(v)
            ),
            Ok(requested)
        );
        assert_eq!(
            access_hwcr(baseline, None, false, false, true, || physical.get(), |_| panic!()),
            Ok(requested)
        );
        assert_eq!(physical.get() & !HWCR_CPUID_FLT_EN, baseline);
    }
    for bit in (0..64).filter(|b| ![30, 35].contains(b)) {
        assert!(matches!(
            access_hwcr(
                baseline,
                Some(baseline ^ (1 << bit)),
                true,
                false,
                true,
                || physical.get(),
                |_| panic!()
            ),
            Err(HwcrError::UnsupportedChange { .. })
        ));
    }
}

#[test]
fn owned_inventory_is_exactly_the_reviewed_register_set() {
    let expected = [
        0x200,
        0x201,
        0x202,
        0x203,
        0x204,
        0x205,
        0x206,
        0x207,
        0x208,
        0x209,
        0x20a,
        0x20b,
        0x20c,
        0x20d,
        0x20e,
        0x20f,
        0x250,
        0x258,
        0x259,
        0x268,
        0x269,
        0x26a,
        0x26b,
        0x26c,
        0x26d,
        0x26e,
        0x26f,
        0xfe,
        0x2ff,
        0xc001_0010,
        0xc001_0015,
        0xc001_0016,
        0xc001_0017,
        0xc001_0018,
        0xc001_0019,
        0xc001_001a,
        0xc001_001d,
        0xc001_0058,
    ];
    assert!(owned_msrs().eq(expected));
    assert!(expected.into_iter().all(owned_msr));
    // PAT keeps its hardware G_PAT owner; neighbours stay unowned.
    for index in [
        0x1ff,
        0x210,
        0x251,
        0x25a,
        0x267,
        0x270,
        0x277,
        0x2fe,
        0x300,
        0xc001_000f,
        0xc001_0011,
        0xc001_0014,
        0xc001_001b,
        0xc001_001c,
        0xc001_001e,
        0xc001_0057,
        0xc001_0059,
    ] {
        assert!(!owned_msr(index), "{index:#x}");
    }
}

#[test]
fn post_ebs_survey_requires_every_fresh_sample_and_keeps_failure_stopped() {
    let survey = CacheSurvey::new();
    assert!(!survey.admit(0));
    assert!(!survey.admit(33));
    assert!(survey.complete_sample(2));
    assert!(!survey.complete_sample(2));
    assert!(!survey.complete_sample(32));
    assert!(!survey.admit(3));
    assert!(!survey.admitted());
    assert!(survey.complete_sample(0));
    assert!(!survey.admit(3));
    assert!(survey.complete_sample(1));
    assert!(survey.admit(3));
    assert!(survey.admitted());
    survey.abort();
    assert!(!survey.admitted());
    assert!(!survey.admit(3));
}

#[test]
fn active_extended_fixed_tuple_validation_uses_complete_table() {
    let mut bank = CacheObservation { default: 0xc00, sys_cfg: 1 << 18, ..CacheObservation::EMPTY };
    for byte in 0..=255u8 {
        bank.fixed[7] = u64::from_le_bytes([byte; 8]);
        assert_eq!(
            bank.validate_active_fixed_types().is_ok(),
            matches!(
                byte,
                0x00 | 0x01 | 0x04 | 0x05 | 0x08 | 0x09 | 0x10 | 0x15 | 0x18 | 0x19 | 0x1c | 0x1e
            )
        );
    }
    bank.fixed[7] = 0x1d1d_1d1d_1d1d_1d1d;
    assert_eq!(bank.validate_active_fixed_types().unwrap_err().index, 0x26c);
    for default in [0, 0x400, 0x800] {
        bank.default = default;
        assert!(bank.validate_active_fixed_types().is_ok());
    }
    bank.default = 0xc00;
    bank.sys_cfg = 0;
    assert_eq!(bank.validate_active_fixed_types().unwrap_err().predicate, 21);
}

#[test]
fn late_firmware_sync_is_admitted_only_from_complete_fresh_bank() {
    let bsp = CacheObservation {
        default: 0xc00,
        sys_cfg: 1 << 18,
        fixed: [0x1515_1515_1515_1515; 11],
        topology: [0, 0, 0, 0x501f],
        ..CacheObservation::EMPTY
    };
    let mut earlier_ap = bsp;
    earlier_ap.topology = [1, 1, 0, 0x501f];
    earlier_ap.fixed[7] = 0x1d1d_1d1d_1d1d_1d1d;
    assert!(!earlier_ap.restored_mtrrs(bsp.default, &bsp.variable, &bsp.fixed, bsp.sys_cfg));
    let fresh_ap = CacheObservation { fixed: bsp.fixed, ..earlier_ap };
    let mut capture = CacheCapture::empty();
    let survey = CacheSurvey::new();
    assert!(capture.initialize(2));
    assert!(capture.seed(1, fresh_ap));
    assert!(survey.complete_sample(1));
    assert!(capture.agrees_with_bsp_detailed(0, 2).is_err());
    assert!(!survey.admit(2));
    assert!(capture.seed(0, bsp));
    assert!(survey.complete_sample(0));
    assert!(capture.agrees_with_bsp(0, 2));
    assert_eq!(capture.domain_mask(1, &[0, 1]), Some(2));
    let mut owner = CacheOwner::empty();
    assert!(owner.initialize(&capture, &[0, 1]));
    assert!(survey.admit(2));
    assert!(capture.observation(1, 2).unwrap().same_physical_state(&fresh_ap));
    assert!(!capture.observation(1, 2).unwrap().same_physical_state(&earlier_ap));
}

fn replay_core() -> CacheCoreState {
    let mut bank = CacheObservation::EMPTY;
    bank.default = 0xc06;
    bank.sys_cfg = 1 << 18;
    bank.fixed.fill(0x1e1e_1e1e_1e1e_1e1e);
    CacheCoreState { bank, members: 0b1010, ..CacheCoreState::EMPTY }
}

#[test]
fn delayed_e0_and_e1_consumers_cannot_lose_release_or_clear_next_generation() {
    let mut core = replay_core();
    let baseline = core.bank;
    let generation = core.enter(2, 0x406).unwrap();
    assert_eq!(core.bank.default, 0xc06); // first E0 is not published alone
    assert_eq!(core.enter(2, 0x406), Err(CacheWriteError::Unsupported));
    core.enter(8, 0x406).unwrap();
    assert_eq!(core.phase, 2);
    core.leave(8, baseline.default, &baseline).unwrap();
    // Last E0 CPU reached E1 before the first host waiter was scheduled.
    assert_eq!(core.generation, generation);
    assert!(matches!(core.phase, 2 | 3));
    core.leave(2, baseline.default, &baseline).unwrap();
    core.depart(8, generation).unwrap();
    assert_eq!(core.phase, 4);
    assert_eq!(core.enter(8, 0x406), Err(CacheWriteError::Unsupported));
    core.depart(2, generation).unwrap();
    assert_eq!(core.phase, 0);
    core.enter(8, 0x406).unwrap();
    // Late old-generation waiter only observes completion; it cannot
    // perform another depart or clear the newly armed local guard/root.
    assert_ne!(core.generation, generation);
    assert_eq!(core.depart(2, generation), Err(CacheWriteError::Unsupported));
    assert_eq!(core.phase, 1);
}

#[test]
fn shared_shadow_preserves_thread_visibility_hidden_attributes_and_final_routing() {
    let mut core = replay_core();
    let baseline = core.bank;
    let mut a = false;
    let b = false;
    assert_eq!(core.read(0x250, a), Some(0x0606_0606_0606_0606));
    assert_eq!(core.write(0x250, baseline.fixed[0], &mut a), Err(CacheWriteError::Fault));
    core.enter(2, 0x406).unwrap();
    core.enter(8, 0x406).unwrap();
    core.write(SYS_CFG, 1 << 19, &mut a).unwrap();
    assert!(a);
    assert!(!b);
    assert_eq!(core.read(0x250, a), Some(baseline.fixed[0]));
    assert_eq!(core.read(SYS_CFG, b), Some(0));
    core.leave(2, baseline.default, &baseline).unwrap();
    assert_eq!(core.leave(8, baseline.default, &baseline), Err(CacheWriteError::Unsupported));
    assert_eq!(core.leaving, 2);
    core.write(SYS_CFG, 1 << 18, &mut a).unwrap();
    core.leave(8, baseline.default, &baseline).unwrap();
    assert_eq!(core.phase, 4);
    assert_eq!(core.write(0xc001_001a, 1, &mut a), Err(CacheWriteError::Unsupported));
    assert_eq!(core.write(0xfe, 0, &mut a), Err(CacheWriteError::Fault));
}

#[test]
fn startup_core_lease_excludes_e0_publication_until_local_commit_finishes() {
    let core = CacheCore::new(replay_core());
    let lease = core.try_lock().unwrap();
    assert_eq!(lease.phase, 0);
    assert!(core.with(|s| s.enter(2, 0x406)).is_none());
    drop(lease);
    assert_eq!(core.with(|s| s.enter(2, 0x406)), Some(Ok(0)));
    assert_ne!(core.try_lock().unwrap().phase, 0);
}

#[test]
fn initial_capture_is_unavailable_until_every_unique_slot_finishes() {
    let mut capture = CacheCapture::empty();
    assert!(!capture.initialize(33));
    assert!(capture.initialize(2));
    assert!(!capture.initialize(2));
    assert!(capture.seed(1, CacheObservation::EMPTY));
    assert!(capture.observation(1, 2).is_none());
    assert!(!capture.seed(1, CacheObservation::EMPTY));
    assert!(capture.seed(0, CacheObservation::EMPTY));
    assert!(capture.observation(1, 2).is_some());
    assert!(capture.observation(2, 2).is_none());
    assert!(!capture.complete(1));
}

#[test]
fn sharing_uses_captured_package_core_not_dense_slot_or_apic_parity() {
    let mut capture = CacheCapture::empty();
    assert!(capture.initialize(4));
    for (slot, (apic, core)) in [(18, 7), (2, 7), (19, 7), (3, 7)].into_iter().enumerate() {
        let observation = CacheObservation {
            topology: [apic, 0x100 | core, 0, 0x400f],
            ..CacheObservation::EMPTY
        };
        assert!(capture.seed(slot, observation));
    }
    assert_eq!(capture.domain_mask(0, &[18, 2, 19, 3]), Some(0b0101));
    assert_eq!(capture.domain_mask(1, &[18, 2, 19, 3]), Some(0b1010));
    assert_eq!(capture.domain_mask(0, &[18, 2, 18, 3]), None);
    capture.observations[2].default = 0xc06;
    assert_eq!(capture.domain_mask(0, &[18, 2, 19, 3]), None);
}

#[test]
fn bsp_replay_admission_ignores_only_disabled_variable_contents() {
    let mut capture = CacheCapture::empty();
    assert!(capture.initialize(2));
    let mut bsp = CacheObservation::EMPTY;
    bsp.default = 0xc06;
    bsp.variable[3] = (0x1234_5006, 0xffff_ff00_0000);
    let peer = CacheObservation { variable: [(0, 0); 8], ..bsp };
    assert!(capture.seed(0, peer));
    assert!(capture.seed(1, bsp));
    assert!(capture.agrees_with_bsp(1, 2));
    capture.observations[0].variable[0] = (6, 0xffff_8000_0800);
    assert!(!capture.agrees_with_bsp(1, 2));
}

#[test]
fn fixed_capture_restores_visibility_after_failed_readback() {
    use core::cell::Cell;
    let syscfg = Cell::new(1 << 18);
    let writes = Cell::new(0);
    let result = CacheObservation::capture(
        0x00b4_0f40,
        48,
        Some([0, 0, 0, 0x400f]),
        |index| match index {
            0xfe => 0x508,
            0xc001_0015 => 0x10,
            SYS_CFG => syscfg.get(),
            _ => 0,
        },
        |index, value| {
            assert_eq!(index, SYS_CFG);
            writes.set(writes.get() + 1);
            // Simulate a failed attempt to expose bit19; restoration is
            // nevertheless required and must target the original value.
            if value & SYS_CFG_MTRR_FIX_DRAM_MOD_EN == 0 {
                syscfg.set(value);
            }
        },
    );
    assert!(result.is_none());
    assert_eq!(writes.get(), 2);
    assert_eq!(syscfg.get(), 1 << 18);
}

#[test]
fn replay_can_zero_disabled_slots_but_cannot_change_active_ranges() {
    let mut physical = CacheObservation::EMPTY;
    physical.default = 0xc06;
    physical.sys_cfg = 1 << 18;
    physical.variable[0] = (0x8000_0006, 0xffff_8000_0000);
    physical.variable[1] = (6, 0xffff_8000_0800);
    physical.fixed.fill(0x1e1e_1e1e_1e1e_1e1e);
    let mut replay = physical.variable;
    replay[0] = (0, 0);
    assert!(physical.restored_mtrrs(
        physical.default,
        &replay,
        &physical.fixed,
        physical.sys_cfg | SYS_CFG_MTRR_FIX_DRAM_MOD_EN
    ));
    replay[1].0 += 0x8000_0000;
    assert!(!physical.restored_mtrrs(physical.default, &replay, &physical.fixed, physical.sys_cfg));
    assert!(!physical.same_physical_state(&CacheObservation { variable: replay, ..physical }));
}

#[test]
fn replay_cannot_change_default_fixed_or_shared_routing() {
    let mut physical = CacheObservation::EMPTY;
    physical.default = 0xc06;
    physical.sys_cfg = 1 << 18;
    assert!(!physical.restored_mtrrs(0x406, &physical.variable, &physical.fixed, physical.sys_cfg));
    let mut fixed = physical.fixed;
    fixed[0] = 0x10;
    assert!(!physical.restored_mtrrs(
        physical.default,
        &physical.variable,
        &fixed,
        physical.sys_cfg
    ));
    assert!(!physical.restored_mtrrs(physical.default, &physical.variable, &physical.fixed, 0));
}
