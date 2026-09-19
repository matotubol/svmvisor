#![cfg(feature = "card-returning-loader")]

use svmvisor_card_abi::native_result::NativeResult;
use svmvisor_card_loader::{
    delivery::child_image::Delivery,
    diagnostics::outcome::{ReturningOutcome::*, classify},
};
use uefi_raw::Status;

fn delivered(inner: NativeResult) -> Delivery {
    Delivery {
        stage: 4,
        load_status: Some(Status::SUCCESS),
        start_status: Some(Status::UNSUPPORTED),
        inner,
        operation_status: Status::SUCCESS,
        cleanup_status: Status::SUCCESS,
    }
}

fn completed() -> NativeResult {
    NativeResult {
        rust_entered: 1,
        rust_completed: 1,
        outcome: 2,
        attempted_entries: 1,
        completed_exits: 1,
        restoration_complete: 1,
        cleanup_complete: 1,
        adapter_checks: 15,
        canary_observed: 1,
        canary_called: 1,
        ..NativeResult::new()
    }
}

#[test]
fn delivery_never_substitutes_for_complete_probe_evidence() {
    assert_eq!(classify(&delivered(completed())), Completed);
    for changed in [
        NativeResult { attempted_entries: 0, ..completed() },
        NativeResult { attempted_entries: 2, ..completed() },
        NativeResult { completed_exits: 0, ..completed() },
        NativeResult { restoration_complete: 0, ..completed() },
        NativeResult { cleanup_complete: 0, ..completed() },
        NativeResult { adapter_checks: 7, ..completed() },
        NativeResult { canary_failures: 1, ..completed() },
        NativeResult { canary_observed: 0, ..completed() },
        NativeResult { canary_called: 0, ..completed() },
        NativeResult { rust_completed: 0, ..completed() },
        NativeResult { outcome: 3, ..completed() },
        NativeResult { reserved: [1, 0], ..completed() },
    ] {
        assert_eq!(classify(&delivered(changed)), Failed, "{changed:?}");
    }
    let mut report = delivered(completed());
    report.start_status = Some(Status::SUCCESS);
    assert_eq!(classify(&report), Failed);
    report.start_status = Some(Status::UNSUPPORTED);
    report.cleanup_status = Status::DEVICE_ERROR;
    assert_eq!(classify(&report), Failed);
    report.cleanup_status = Status::SUCCESS;
    report.load_status = None;
    assert_eq!(classify(&report), Failed);
}

#[test]
fn refusal_requires_zero_entries_and_consistent_inner_canary_markers() {
    assert_eq!(classify(&delivered(NativeResult::new())), Refused);
    let refused = NativeResult {
        rust_entered: 1,
        rust_completed: 1,
        outcome: 1,
        refusal: 7,
        cleanup_complete: 1,
        ..NativeResult::new()
    };
    assert_eq!(classify(&delivered(refused)), Refused);
    assert_eq!(
        classify(&delivered(NativeResult { canary_called: 1, canary_observed: 1, ..refused })),
        Refused
    );
    for changed in [
        NativeResult { attempted_entries: 1, ..refused },
        NativeResult { attempted_entries: u64::MAX, ..refused },
        NativeResult { canary_called: 1, ..refused },
        NativeResult { canary_called: 2, canary_observed: 1, ..refused },
        NativeResult { canary_observed: 1, ..refused },
        NativeResult { canary_failures: 1, ..refused },
        NativeResult { cleanup_complete: 0, ..refused },
        NativeResult { rust_entered: 0, ..refused },
    ] {
        assert_eq!(classify(&delivered(changed)), Failed, "{changed:?}");
    }
}

#[test]
fn multi_exit_completion_requires_the_whole_fixed_run_and_valid_return() {
    // Independent wire values: a single-entry success cannot satisfy this profile.
    let multi =
        NativeResult { outcome: 12, attempted_entries: 65, completed_exits: 65, ..completed() };
    assert_eq!(classify(&delivered(multi)), Completed);
    for entries in 0..=66 {
        for exits in 0..=66 {
            let candidate =
                NativeResult { attempted_entries: entries, completed_exits: exits, ..multi };
            assert_eq!(
                classify(&delivered(candidate)),
                if entries == 65 && exits == 65 { Completed } else { Failed },
                "entries={entries}, exits={exits}"
            );
        }
    }
    for changed in [
        NativeResult { outcome: 2, ..multi },
        NativeResult { outcome: 7, ..multi },
        NativeResult { refusal: 0x4303, ..multi },
        NativeResult { restoration_complete: 0, ..multi },
        NativeResult { restoration_complete: 2, ..multi },
        NativeResult { cleanup_complete: 0, ..multi },
        NativeResult { adapter_checks: 7, ..multi },
        NativeResult { adapter_checks: 31, ..multi },
        NativeResult { canary_failures: 1, ..multi },
        NativeResult { canary_observed: 0, ..multi },
        NativeResult { canary_called: 0, ..multi },
        NativeResult { rust_entered: 0, ..multi },
        NativeResult { rust_completed: 0, ..multi },
        NativeResult { bytes: 0, ..multi },
        NativeResult { reserved: [0, 1], ..multi },
        NativeResult { attempted_entries: u64::MAX, ..multi },
        NativeResult { completed_exits: u64::MAX, ..multi },
    ] {
        assert_eq!(classify(&delivered(changed)), Failed, "{changed:?}");
    }
    let mut delivery = delivered(multi);
    delivery.stage = 3;
    assert_eq!(classify(&delivery), Failed);
    delivery.stage = 4;
    delivery.start_status = Some(Status::SUCCESS);
    assert_eq!(classify(&delivery), Failed);
    delivery.start_status = Some(Status::UNSUPPORTED);
    delivery.cleanup_status = Status::DEVICE_ERROR;
    assert_eq!(classify(&delivery), Failed);
    // Partial execution cannot be relabeled as a refusal with no guest execution.
    assert_eq!(
        classify(&delivered(NativeResult {
            outcome: 1,
            refusal: 0x4301,
            attempted_entries: 17,
            completed_exits: 17,
            ..multi
        })),
        Failed
    );
}
