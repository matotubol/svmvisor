//! Firmware admission, image delivery, and observable DXE results.
//!
//! Public modules follow the source directories, for example
//! `native::admission::cpu` or `diagnostics::journal`. Firmware image entry
//! points, resource ownership, and emulator fixtures are wired separately by
//! `main.rs`.

#![no_std]

pub mod diagnostics;
#[cfg(any(feature = "card-load-only", feature = "card-returning-loader", feature = "card-resident-loader"))]
pub mod delivery;
#[cfg(feature = "memory-attribute-provider")]
pub mod memory_attributes;
#[cfg(feature = "native-preflight")]
pub mod native;
