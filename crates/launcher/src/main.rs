//! UEFI DXE entry point.
//!
//! Firmware-facing setup stays in this crate. The post-boot-services runtime
//! belongs in `svmvisor-hypervisor`.

#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(all(target_os = "uefi", not(feature = "native-resident")))]
compile_error!("select a native-resident image; the card loader is svmvisor-card-loader");

#[cfg(target_os = "uefi")]
use uefi_raw::{Handle, Status, table::system::SystemTable};

// Binary-only modules keep their established crate-local names. The paths group
// firmware ownership without changing the reviewed call graph or
// compiling these image-specific modules into the host-testable library.

#[cfg(all(target_os = "uefi", feature = "native-resident"))]
#[path = "native/resident/activation/mod.rs"]
mod resident_activation;

#[cfg(all(target_os = "uefi", feature = "native-resident"))]
#[unsafe(no_mangle)]
/// Firmware image entry, UEFI 2.10 §4.1.1.
///
/// # Safety
/// Firmware must supply a live image handle and system table with boot services
/// available, and invoke this entry once, at TPL_APPLICATION.
pub unsafe extern "efiapi" fn efi_main(image: Handle, table: *const SystemTable) -> Status {
    unsafe {
        return resident_activation::install(image, table.cast_mut());
    }
}

// The resident candidate has an explicit terminal invariant-failure path.
// Expected admission refusals return through the original callback instead.
#[cfg(all(target_os = "uefi", feature = "native-resident"))]
#[panic_handler]
fn resident_panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(feature = "native-resident-test")]
    for byte in b"resident-dxe-panic\n" {
        unsafe {
            core::arch::asm!("out dx,al",in("dx")0xe9u16,in("al")*byte,options(nomem,nostack));
        }
    }
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("svmvisor-launcher is a UEFI-only native child; use cargo xtask resident");
}
