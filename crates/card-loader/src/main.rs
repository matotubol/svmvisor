//! UEFI card option-ROM loader entry point.
//!
//! Driver binding, card image delivery and lifecycle observation stay in this
//! crate. What the loaded child does belongs to the child image.

#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(all(feature = "card-returning-loader", feature = "card-load-only"))]
compile_error!("returning PE delivery is a separate resident loader mode");
#[cfg(all(
    feature = "card-resident",
    any(feature = "card-returning-loader", feature = "card-load-only")
))]
compile_error!("resident PE delivery is a separate parent image");

#[cfg(target_os = "uefi")]
use uefi_raw::{Handle, Status, table::system::SystemTable};

// Binary-only modules keep their established crate-local names. The paths group
// firmware ownership without changing the reviewed call graph or compiling
// these image-specific modules into the host-testable library.

#[cfg(all(target_os = "uefi", feature = "card-load-only"))]
#[path = "delivery/load.rs"]
mod card_load;
#[cfg(all(target_os = "uefi", any(feature = "card-returning-loader", feature = "card-resident")))]
#[path = "delivery/adapter.rs"]
mod card_returning_adapter;

// Resident option-ROM driver binding and firmware lifecycle observation.
#[cfg(target_os = "uefi")]
#[path = "firmware/cpu.rs"]
mod cpu;
#[cfg(target_os = "uefi")]
#[path = "firmware/driver.rs"]
mod driver;
#[cfg(target_os = "uefi")]
#[path = "firmware/lifecycle.rs"]
mod lifecycle;
#[cfg(target_os = "uefi")]
#[path = "firmware/mmio.rs"]
mod mmio;
#[cfg(target_os = "uefi")]
#[path = "firmware/pci_io.rs"]
mod pci_io;

#[cfg(target_os = "uefi")]
#[unsafe(no_mangle)]
/// Firmware image entry, UEFI 2.10 §4.1.1.
///
/// # Safety
/// Firmware must supply a live image handle and system table with boot services
/// available, and invoke this entry once, at TPL_APPLICATION.
pub unsafe extern "efiapi" fn efi_main(image: Handle, table: *const SystemTable) -> Status {
    unsafe { driver::install(image, table) }
}

// The default ROM image has no runtime panic policy. A reachable Rust panic makes linking
// fail; size-optimized LTO must prove this handler unreachable. This avoids
// pulling in the general UEFI crate's console/delay/shutdown panic machinery.
#[cfg(all(target_os = "uefi", not(feature = "card-load-only")))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn svmvisor_dxe_must_not_panic() -> !;
    }
    unsafe { svmvisor_dxe_must_not_panic() }
}

// Candidate-only last resort for an internal invariant failure. Expected bad
// card data never takes this path. No firmware calls or transfer are possible;
// recovery requires an external reset. Default record-only policy is unchanged.
#[cfg(all(target_os = "uefi", feature = "card-load-only"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("svmvisor-card-loader is a UEFI-only driver; use cargo build-card-loader");
}
