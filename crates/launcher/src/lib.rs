//! Native child admission, resident preparation, and observable launcher results.
//!
//! Public modules follow the source directories, for example
//! `native::admission::cpu` or `diagnostics::resident_boot`. The firmware image
//! entry point and the resident activation are wired separately by `main.rs`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod diagnostics;
#[cfg(feature = "native-preflight")]
pub mod native;
