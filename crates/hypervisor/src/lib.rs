//! Bare-metal AMD SVM hypervisor core.
//!
//! Architecture primitives, boot contracts, guest and host state, memory, and
//! SVM exit handling live in separate namespaces, for example `svm::dispatch`,
//! `guest::state`, `memory::address` or `boot::preflight`. This crate stays
//! independent of UEFI; firmware allocation, protocols, and lifecycle belong
//! to `dxe`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod arch;
pub mod boot;
pub mod guest;
pub mod host;
pub mod memory;
pub mod svm;
pub mod sync;
