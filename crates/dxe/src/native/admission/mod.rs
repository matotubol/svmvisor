//! CPU, memory, entry-boundary, and cache evidence required before guest entry.

pub mod boundary;
#[cfg(feature = "native-resource-observe")]
pub mod cache;
#[cfg(feature = "native-resource-observe")]
pub mod cache_rendezvous;
pub mod cpu;
pub mod memory;
pub mod preflight;
pub mod snapshot;
