//! Bare-metal AMD SVM hypervisor core.
//!
//! Architecture primitives, boot contracts, guest and host state, memory, and
//! SVM exit handling live in separate namespaces. This crate stays independent
//! of UEFI; firmware allocation, protocols, and lifecycle belong to `dxe`.
//!
//! Root module aliases preserve the existing API. New code should use the
//! grouped paths, such as `svm::dispatch`, `guest::state`, or `boot::preflight`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod arch;
pub mod boot;
pub mod guest;
pub mod host;
pub mod memory;
pub mod svm;

// Compatibility aliases refer to the same modules and types, without compiling
// a second copy of an implementation. Keep existing callers source compatible.
pub use arch::x86_64::{capabilities, descriptors, registers, xstate};
pub use boot::{
    descriptors as firmware_descriptors, handoff, memory as firmware_memory,
    preflight as native_preflight, probe as firmware_probe, xstate as firmware_xstate,
};
pub use guest::{pages as guest_pages, state as guest_state};
pub use host::{descriptors as host_descriptors, paging as host_paging};
pub use memory::{address, layout, npt};
pub use svm::{dispatch, emulation, exit, permission_maps, vmcb};
