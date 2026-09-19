//! AMD SVM control structures and bounded exit classification and emulation.
//!
//! Preparing a resume here does not itself enter a guest or establish hardware
//! readiness; the caller must satisfy the entry and state-ownership contracts.

pub mod cache;
pub mod cpu_model;
pub mod diagnostic_config;
pub mod dispatch;
pub mod emulation;
pub mod events;
pub mod exit;
pub mod mcax;
pub mod native_pause;
pub mod permission_maps;
pub mod syscfg;
pub mod vmcb;
pub mod x2avic;
