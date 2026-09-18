//! AMD SVM control structures and bounded exit classification and emulation.
//!
//! Preparing a resume here does not itself enter a guest or establish hardware
//! readiness; the caller must satisfy the entry and state-ownership contracts.

pub mod cpu_model;
pub mod dispatch;
pub mod emulation;
pub mod events;
pub mod exit;
pub mod native_cache;
pub mod native_diagnostic_config;
pub mod native_mcax;
pub mod native_pause;
pub mod native_syscfg;
pub mod permission_maps;
pub mod vmcb;
pub mod x2avic;
