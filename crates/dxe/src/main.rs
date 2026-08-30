//! UEFI DXE entry point.
//!
//! Firmware-facing setup stays in this crate. The post-boot-services runtime
//! belongs in `svmvisor-hypervisor`.

#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "uefi")]
use uefi::prelude::{Status, entry};

#[cfg(target_os = "uefi")]
#[entry]
fn main() -> Status {
    Status::SUCCESS
}

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("svmvisor-dxe is a UEFI-only driver; use cargo build-dxe");
}
