//! Firmware admission, image delivery, and observable DXE results.
//!
//! Public modules follow the source directories, for example
//! `native::admission::cpu` or `diagnostics::journal`. Firmware image entry
//! points, resource ownership, and emulator fixtures are wired separately by
//! `main.rs`.

#![no_std]

#[cfg(all(feature = "card-resident-loader", feature = "card-resident-dev-loader"))]
compile_error!(
    "card-resident-loader (compiled-in header pin) and card-resident-dev-loader (header trusted from the flash slot) are mutually exclusive; enable exactly one"
);
#[cfg(all(
    feature = "card-resident",
    not(any(feature = "card-resident-loader", feature = "card-resident-dev-loader"))
))]
compile_error!(
    "card-resident is internal; select card-resident-loader or card-resident-dev-loader"
);

#[cfg(any(
    feature = "card-load-only",
    feature = "card-returning-loader",
    feature = "card-resident"
))]
pub mod delivery;
pub mod diagnostics;
#[cfg(feature = "memory-attribute-provider")]
pub mod memory_attributes;
#[cfg(feature = "native-preflight")]
pub mod native;
