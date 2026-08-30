//! Bare-metal AMD SVM hypervisor core.
//!
//! This crate stays independent of UEFI so its code and data remain usable
//! after `ExitBootServices`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]
