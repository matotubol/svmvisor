//! Interpret returned child evidence without equating image delivery with SVM.
use crate::{
    card_returning::Delivery,
    native_result::{MULTI_EXIT_ENTRIES, MULTI_EXIT_OUTCOME, NativeResult},
};
use uefi_raw::Status;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ReturningOutcome {
    Completed = 1 << 13,
    Refused = 1 << 14,
    Failed = 1 << 15,
}

pub fn classify(report: &Delivery) -> ReturningOutcome {
    use ReturningOutcome::*;
    let inner = report.inner;
    if report.status() != Status::SUCCESS
        || !inner.valid_header()
        || report.stage != 4
        || report.load_status != Some(Status::SUCCESS)
        || report.start_status != Some(Status::UNSUPPORTED)
    {
        return Failed;
    }
    if inner == NativeResult::new() {
        return Refused;
    }
    if inner.rust_entered != 1 || inner.rust_completed != 1 || inner.cleanup_complete != 1 {
        return Failed;
    }
    let single_entry = inner.outcome == 2
        && inner.attempted_entries == 1
        && inner.completed_exits == 1;
    let multi_exit = inner.outcome == MULTI_EXIT_OUTCOME
        && inner.attempted_entries == MULTI_EXIT_ENTRIES
        && inner.completed_exits == MULTI_EXIT_ENTRIES;
    if (single_entry || multi_exit)
        && inner.refusal == 0
        && inner.restoration_complete == 1
        && inner.adapter_checks == 15
        && inner.canary_failures == 0
        && inner.canary_observed == 1
        && inner.canary_called == 1
    {
        return Completed;
    }
    let uncalled = inner.canary_called == 0
        && inner.canary_observed == 0
        && inner.adapter_checks == 0
        && inner.restoration_complete == 0;
    let called = inner.canary_called == 1
        && inner.canary_observed == 1
        && ((inner.restoration_complete == 0 && inner.adapter_checks == 0)
            || (inner.restoration_complete == 1 && inner.adapter_checks == 7));
    if inner.outcome == 1
        && inner.refusal != 0
        && inner.attempted_entries == 0
        && inner.completed_exits == 0
        && inner.canary_failures == 0
        && (uncalled || called)
    {
        return Refused;
    }
    Failed
}
