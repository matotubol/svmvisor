//! Card payload validation and returning child image delivery.
//!
//! `adapter.rs` and `load.rs` are firmware binary modules wired by `main.rs`.

#[cfg(feature = "card-load-only")]
pub mod card;
#[cfg(any(feature = "card-returning-loader", feature = "card-resident"))]
pub mod returning;
