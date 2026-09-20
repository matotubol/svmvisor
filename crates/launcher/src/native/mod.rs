//! Native admission evidence and resident preparation.
//!
//! `resident/activation/` belongs to the firmware binary: `main.rs` selects it
//! without exposing firmware ownership through this library.

pub mod admission;
pub mod resident;
