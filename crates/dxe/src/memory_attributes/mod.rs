//! Memory Attribute Protocol provider, registration, and qualified table access.

#[cfg(feature = "memory-attribute-f7")]
pub mod f7;
#[cfg(feature = "memory-attribute-firmware")]
pub mod firmware;
#[cfg(all(feature = "memory-attribute-probe", target_os = "uefi", target_arch = "x86_64"))]
pub mod native;
#[cfg(feature = "memory-attribute-probe")]
pub mod probe;
pub mod provider;
pub mod registration;

// Preserve the original `memory_attributes::Adapter` and related item paths.
pub use provider::*;
