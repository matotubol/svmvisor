//! Card image delivery and observable loader results.
//!
//! Public modules follow the source directories, for example
//! `delivery::child_image` or `diagnostics::journal`. The firmware image entry
//! point and the option-ROM driver binding are wired separately by `main.rs`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

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

#[cfg(feature = "card-resident")]
pub mod delivery;
pub mod diagnostics;
