//! Card payload validation and returning child image delivery.
//!
//! `adapter.rs` is a firmware binary module wired by `main.rs`.

#[cfg(any(feature = "card-returning-loader", feature = "card-resident"))]
pub mod child_image;
