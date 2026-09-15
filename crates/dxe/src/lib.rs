//! Firmware admission, image delivery, and observable DXE results.
//!
//! Public modules follow the source directories. Firmware image entry points,
//! resource ownership, and emulator fixtures are wired separately by `main.rs`.

#![no_std]

pub mod diagnostics;
#[cfg(any(feature = "card-load-only", feature = "card-returning-loader", feature = "card-resident-loader"))]
pub mod delivery;
#[cfg(feature = "memory-attribute-provider")]
pub mod memory_attributes;
#[cfg(feature = "native-preflight")]
pub mod native;

// Keep existing callers and reviewed host tests on the same implementations.
// New code can use the grouped paths above; these aliases add no wrappers.
pub use diagnostics::{journal, trace};
#[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader", feature = "native-preflight"))]
pub use diagnostics::native_result;
#[cfg(feature = "card-returning-loader")]
pub use diagnostics::{outcome as returning_outcome, returning as returning_diagnostics};

#[cfg(feature = "card-load-only")]
pub use delivery::card;
#[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
pub use delivery::returning as card_returning;

#[cfg(feature = "memory-attribute-provider")]
pub use memory_attributes::registration as memory_attribute_registration;
#[cfg(feature = "memory-attribute-firmware")]
pub use memory_attributes::firmware as memory_attribute_firmware;
#[cfg(feature = "memory-attribute-probe")]
pub use memory_attributes::probe as memory_attribute_probe;
#[cfg(all(feature = "memory-attribute-probe", target_os = "uefi", target_arch = "x86_64"))]
pub use memory_attributes::native as memory_attribute_native;
#[cfg(feature = "memory-attribute-f7")]
pub use memory_attributes::f7 as memory_attribute_f7;

#[cfg(feature = "native-preflight")]
pub use native::admission::{
    boundary as native_boundary, cpu as native_cpu, memory as native_memory,
    preflight as native_preflight, snapshot as native_snapshot,
};
#[cfg(feature = "native-resource-observe")]
pub use native::admission::{cache as native_cache, cache_rendezvous as native_cache_rendezvous};
#[cfg(any(feature = "native-transition-test", feature = "native-returning"))]
pub use native::transition::{canary as native_transition_canary, state as native_transition};
