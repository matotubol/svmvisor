//! UEFI DXE entry point.
//!
//! Firmware-facing setup stays in this crate. The post-boot-services runtime
//! belongs in `svmvisor-hypervisor`.

#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(all(target_os = "uefi", not(feature = "native-preflight")))]
compile_error!("select a native-* image; the card loader is svmvisor-card-loader");
#[cfg(all(feature = "native-returning", feature = "native-transition-test"))]
compile_error!("native returning admission cannot combine with a TCG transition fixture");

#[cfg(target_os = "uefi")]
use uefi_raw::{Handle, Status, table::system::SystemTable};

// Binary-only modules keep their established crate-local names. The paths group
// firmware ownership and fixtures without changing the reviewed call graph or
// compiling these image-specific modules into the host-testable library.

// Native child entry, resource ownership, and the returning SVM execution path.
#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature = "native-resident")))]
#[path = "native/child_result.rs"]
mod native_child_result;
#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature = "native-resident")))]
#[path = "native/entry.rs"]
mod native_entry;
#[cfg(all(
    target_os = "uefi",
    any(feature = "native-transition-test", feature = "native-returning")
))]
#[path = "native/resources/guest.rs"]
mod native_guest_resources;
#[cfg(all(target_os = "uefi", feature = "native-resource-observe"))]
#[path = "native/resources/image.rs"]
mod native_image_resources;
#[cfg(all(target_os = "uefi", feature = "native-resource-observe"))]
#[path = "native/resources/cache.rs"]
mod native_resource_cache;
#[cfg(all(target_os = "uefi", feature = "native-resource-observe"))]
#[path = "native/resources/arena.rs"]
mod native_resources;
#[cfg(all(target_os = "uefi", feature = "native-returning"))]
#[path = "native/returning.rs"]
mod native_returning;
#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature = "native-resident")))]
#[path = "native/resources/tables/mod.rs"]
mod native_tables;
#[cfg(all(target_os = "uefi", feature = "native-transition-test"))]
#[path = "fixtures/transition.rs"]
mod native_transition_fixture;
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

#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature = "native-resident")))]
#[unsafe(no_mangle)]
/// Called only by the assembly image entry after original-state capture.
///
/// # Safety
/// The image/system table follow the EFI entry contract. `capture` is the
/// immutable stack-owned assembly capture and remains live until this returns.
pub unsafe extern "efiapi" fn svmvisor_native_efi_main_inner(
    image: Handle,
    table: *const SystemTable,
    capture: *const svmvisor_launcher::native::admission::boundary::NativeBoundary,
) -> Status {
    let Some(capture) = (unsafe { capture.as_ref() }) else {
        return Status::INVALID_PARAMETER;
    };
    if !capture.has_valid_shape() {
        return Status::UNSUPPORTED;
    }
    unsafe { native_entry::run(image, table, capture) }
}

// A non-resident native image has no runtime panic policy. A reachable Rust panic makes linking
// fail; size-optimized LTO must prove this handler unreachable. This avoids
// pulling in the general UEFI crate's console/delay/shutdown panic machinery.
#[cfg(all(target_os = "uefi", not(feature = "native-resident")))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn svmvisor_dxe_must_not_panic() -> !;
    }
    unsafe { svmvisor_dxe_must_not_panic() }
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
