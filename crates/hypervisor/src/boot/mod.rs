//! Firmware-to-hypervisor contracts and validation over supplied observations.
//!
//! This namespace contains no UEFI protocol or allocation dependency. The DXE
//! crate gathers observations and owns the firmware lifecycle that uses them.

pub mod descriptors;
pub mod handoff;
pub mod memory;
pub mod ownership;
pub mod preflight;
pub mod probe;
pub mod xstate;
