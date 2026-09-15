//! Native admission evidence and the bounded guest transition contract.
//!
//! `entry.rs`, `child_result.rs`, `returning.rs`, and `resources/` belong to the
//! firmware binary: `main.rs` selects them without exposing firmware ownership
//! through this library.

pub mod admission;
pub mod resident;
#[cfg(any(feature = "native-transition-test", feature = "native-returning"))]
pub mod transition;
