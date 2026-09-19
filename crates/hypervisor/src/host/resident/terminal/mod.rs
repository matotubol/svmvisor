//! One-shot terminal evidence. PPR57896 rev3.00 pp40-41,210; APM2 rev3.44
//! 15.17/Table15-10,15.21.8,15.28. No resumable dispatcher performs transport.

// `refusal::fetch_failure_code` and its test name `fetch` as `super::fetch` inside function bodies
// that moved here unchanged.
use crate::host::resident::fetch;

pub use crate::host::resident::terminal::{
    control::{CONTROL_OFFSET, DiagnosticGuard, TerminalControl, cpu_mask},
    deferred_fault::DeferredFault,
    refusal::{
        FetchReadFailure, IrqSite, StartupStage, X2AvicStop, avic_exit_refusal, efer_failure,
        efer_nrip_failure, fan_out_failure, fetch_failure_code, init_error_code, ipi_refusal,
        irq_error_code, irq_failure, profile_mismatch_at_entry, register_refusal, startup_failure,
        startup_pending_failure, startup_route_refusal, stop_words, syscfg_failure,
        syscfg_operands, vmcr_failure, vmcr_nrip_failure, x2avic_error_code,
    },
};

mod control;
mod deferred_fault;
mod refusal;
