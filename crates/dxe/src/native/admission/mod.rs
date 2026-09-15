//! CPU, memory, entry-boundary, and cache evidence required before guest entry.

pub mod boundary;
pub mod cpu;
pub mod memory;
pub mod preflight;
pub mod snapshot;
#[cfg(feature = "native-resource-observe")]
pub mod cache;
#[cfg(feature = "native-resource-observe")]
pub mod cache_rendezvous;
