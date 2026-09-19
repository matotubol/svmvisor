//! Native child admission, resource ownership, and observable launcher results.
//!
//! Public modules follow the source directories, for example
//! `native::admission::cpu` or `diagnostics::resident_boot`. Firmware image entry
//! points, resource ownership, and emulator fixtures are wired separately by
//! `main.rs`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod diagnostics;
#[cfg(feature = "memory-attribute-provider")]
pub mod memory_attributes;
#[cfg(feature = "native-preflight")]
pub mod native;
