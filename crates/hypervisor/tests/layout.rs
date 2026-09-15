use svmvisor_hypervisor::layout::{
    Layout, LayoutError, LayoutRequest, PAGE_SIZE, Permissions, RegionKind,
};

fn request() -> LayoutRequest {
    LayoutRequest {
        code_bytes: 1,
        data_bytes: PAGE_SIZE + 1,
        stack_bytes: 3 * PAGE_SIZE,
    }
}

#[test]
fn rounded_ranges_are_disjoint_guarded_and_have_explicit_permissions() {
    let plan = Layout::plan(17 * PAGE_SIZE, request()).unwrap();
    assert_eq!(plan.used_bytes(), 17 * PAGE_SIZE);
    let regions = plan.regions();
    let expected = [
        (RegionKind::Code, 1, Permissions::ReadExecute),
        (RegionKind::Data, 2, Permissions::ReadWriteNoExecute),
        (RegionKind::StackGuardLow, 1, Permissions::Unmapped),
        (RegionKind::Stack, 3, Permissions::ReadWriteNoExecute),
        (RegionKind::StackGuardHigh, 1, Permissions::Unmapped),
        (
            RegionKind::ExecutionVmcb,
            1,
            Permissions::ReadWriteNoExecute,
        ),
        (RegionKind::HostAuxVmcb, 1, Permissions::ReadWriteNoExecute),
        (
            RegionKind::NativeReturnVmcb,
            1,
            Permissions::ReadWriteNoExecute,
        ),
        (RegionKind::Hsave, 1, Permissions::ReadWriteNoExecute),
        (RegionKind::Iopm, 3, Permissions::ReadWriteNoExecute),
        (RegionKind::Msrpm, 2, Permissions::ReadWriteNoExecute),
    ];
    let mut end = 0;
    for (region, (kind, pages, permissions)) in regions.iter().zip(expected) {
        assert_eq!(region.kind(), kind);
        assert_eq!(region.offset(), end);
        assert_eq!(region.offset() % PAGE_SIZE, 0);
        assert_eq!(region.len(), pages * PAGE_SIZE);
        assert_eq!(region.permissions(), permissions);
        end += region.len();
    }
    assert_eq!(end, plan.used_bytes());
}

#[test]
fn exact_capacity_succeeds_and_one_page_short_fails() {
    assert!(Layout::plan(17 * PAGE_SIZE, request()).is_ok());
    assert_eq!(
        Layout::plan(16 * PAGE_SIZE, request()),
        Err(LayoutError::InsufficientArena)
    );
    let larger = Layout::plan(18 * PAGE_SIZE, request()).unwrap();
    assert_eq!(larger.used_bytes(), 17 * PAGE_SIZE);
}

#[test]
fn rejects_zero_sizes_and_non_page_arena() {
    for arena in [0, PAGE_SIZE - 1, 17 * PAGE_SIZE + 1] {
        assert_eq!(
            Layout::plan(arena, request()),
            Err(LayoutError::InvalidSize)
        );
    }
    for bad in [
        LayoutRequest {
            code_bytes: 0,
            ..request()
        },
        LayoutRequest {
            data_bytes: 0,
            ..request()
        },
        LayoutRequest {
            stack_bytes: 0,
            ..request()
        },
    ] {
        assert_eq!(
            Layout::plan(17 * PAGE_SIZE, bad),
            Err(LayoutError::InvalidSize)
        );
    }
}

#[test]
fn rejects_rounding_and_cumulative_overflow() {
    let arena = u64::MAX & !(PAGE_SIZE - 1);
    for bad in [
        LayoutRequest {
            code_bytes: u64::MAX,
            ..request()
        },
        LayoutRequest {
            data_bytes: u64::MAX,
            ..request()
        },
        LayoutRequest {
            stack_bytes: u64::MAX,
            ..request()
        },
        LayoutRequest {
            code_bytes: arena,
            ..request()
        },
    ] {
        assert_eq!(Layout::plan(arena, bad), Err(LayoutError::Overflow));
    }
}
