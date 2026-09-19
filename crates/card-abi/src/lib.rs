//! Wire contract between the card loader, the EFI child it starts, and the
//! resident runtime.
//!
//! The card image envelope, the relocatable package it can wrap, the boot
//! options and result records the loader hands to a child, the card journal's
//! record commits, and the admitted terminal endpoint live here. The
//! crate depends on nothing and knows nothing about UEFI or SVM, so any child
//! payload can use it.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod boot_options;
pub mod endpoint;
pub mod envelope;
pub mod journal;
pub mod native_result;
pub mod package;
