//! UEFI DXE entry point.
//!
//! Firmware-facing setup stays in this crate. The post-boot-services runtime
//! belongs in `svmvisor-hypervisor`.

#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

// Binary-only modules keep their established crate-local names. The paths group
// firmware ownership and fixtures without changing the reviewed call graph or
// compiling these image-specific modules into the host-testable library.

#[cfg(all(feature = "native-preflight", any(feature = "card-load-only", feature = "card-returning-loader", feature = "card-resident-loader", feature = "emulator-handoff")))]
compile_error!("native child and resident card loader are separate images");
#[cfg(all(feature = "card-returning-loader", any(feature = "card-load-only", feature = "emulator-handoff")))]
compile_error!("returning PE delivery is a separate resident loader mode");
#[cfg(all(target_os = "uefi", any(feature = "card-returning-loader", feature = "card-resident-loader")))]
#[path = "delivery/adapter.rs"]
mod card_returning_adapter;

#[cfg(all(feature = "card-resident-loader", any(feature = "card-returning-loader", feature = "card-load-only", feature = "emulator-handoff")))]
compile_error!("resident PE delivery is a separate parent image");

// Native child entry, resource ownership, and the returning SVM execution path.
#[cfg(all(target_os = "uefi", feature = "native-resident"))]
#[path = "native/resident/activation.rs"]
mod resident_activation;
#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature="native-resident")))]
#[path = "native/entry.rs"]
mod native_entry;
#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature="native-resident")))]
#[path = "native/child_result.rs"]
mod native_child_result;
#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature="native-resident")))]
#[path = "native/resources/tables.rs"]
mod native_tables;
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
#[cfg(all(target_os = "uefi", any(feature = "native-transition-test", feature = "native-returning")))]
#[path = "native/resources/guest.rs"]
mod native_guest_resources;
#[cfg(all(feature = "native-returning", feature = "native-transition-test"))]
compile_error!("native returning admission cannot combine with a TCG transition fixture");
#[cfg(all(target_os = "uefi", feature = "native-transition-test"))]
#[path = "fixtures/transition.rs"]
mod native_transition_fixture;

#[cfg(all(feature = "card-load-only", feature = "emulator-handoff"))]
compile_error!("card-load-only cannot be combined with an executing emulator handoff");
#[cfg(all(target_os = "uefi", feature = "card-load-only"))]
#[path = "delivery/load.rs"]
mod card_load;

#[cfg(target_os = "uefi")]
use uefi_raw::{Handle, Status, table::system::SystemTable};

// Resident option-ROM driver binding and firmware lifecycle observation.
// Lifecycle's retained callbacks refer to CPU sampling even though the PCI
// fixture never registers them; keep this dependency compiled in that mode.
#[cfg(all(
    target_os = "uefi",
    not(feature = "native-preflight"),
    any(not(feature = "emulator-handoff"), feature = "emulator-pci-handoff")
))]
#[cfg_attr(feature = "emulator-pci-handoff", allow(dead_code))]
#[path = "firmware/cpu.rs"]
mod cpu;
#[cfg(all(
    target_os = "uefi",
    not(feature = "native-preflight"),
    any(not(feature = "emulator-handoff"), feature = "emulator-pci-handoff")
))]
#[path = "firmware/driver.rs"]
mod driver;
#[cfg(all(
    target_os = "uefi",
    not(feature = "native-preflight"),
    any(not(feature = "emulator-handoff"), feature = "emulator-pci-handoff")
))]
#[cfg_attr(feature = "emulator-pci-handoff", allow(dead_code))]
#[path = "firmware/lifecycle.rs"]
mod lifecycle;
#[cfg(all(
    target_os = "uefi",
    not(feature = "native-preflight"),
    any(not(feature = "emulator-handoff"), feature = "emulator-pci-handoff")
))]
#[cfg_attr(feature = "emulator-pci-handoff", allow(dead_code))]
#[path = "firmware/mmio.rs"]
mod mmio;
#[cfg(all(
    target_os = "uefi",
    not(feature = "native-preflight"),
    any(not(feature = "emulator-handoff"), feature = "emulator-pci-handoff")
))]
#[cfg_attr(feature = "emulator-pci-handoff", allow(dead_code))]
#[path = "firmware/pci_io.rs"]
mod pci_io;

// Disposable emulator entry and MMIO fixtures, selected only by their features.
#[cfg(all(target_os = "uefi", feature = "emulator-handoff"))]
#[path = "fixtures/emulator.rs"]
mod emulator;

#[cfg(all(target_os = "uefi", feature = "emulator-pci-handoff"))]
#[path = "fixtures/emulator_mmio.rs"]
mod emulator_mmio;

#[cfg(all(target_os = "uefi", any(not(feature = "native-preflight"), feature="native-resident")))]
#[unsafe(no_mangle)]
/// Firmware image entry, UEFI 2.10 §4.1.1.
///
/// # Safety
/// Firmware must supply a live image handle and system table with boot services
/// available, and invoke this entry once, at TPL_APPLICATION.
pub unsafe extern "efiapi" fn efi_main(image: Handle, table: *const SystemTable) -> Status {
    #[cfg(feature="native-resident")]
    unsafe { return resident_activation::install(image, table.cast_mut()); }
    #[cfg(feature = "emulator-pci-handoff")]
    unsafe {
        let initialized = emulator::initialize(image, table);
        if initialized.is_error() {
            return initialized;
        }
        driver::install(image, table)
    }
    #[cfg(all(feature = "emulator-handoff", not(feature = "emulator-pci-handoff")))]
    unsafe {
        emulator::run(image, table)
    }
    #[cfg(all(not(feature = "emulator-handoff"), not(feature = "native-preflight")))]
    unsafe {
        driver::install(image, table)
    }
}

#[cfg(all(target_os = "uefi", feature = "native-preflight", not(feature="native-resident")))]
#[unsafe(no_mangle)]
/// Called only by the assembly image entry after original-state capture.
///
/// # Safety
/// The image/system table follow the EFI entry contract. `capture` is the
/// immutable stack-owned assembly capture and remains live until this returns.
pub unsafe extern "efiapi" fn svmvisor_native_efi_main_inner(
    image: Handle,
    table: *const SystemTable,
    capture: *const svmvisor_dxe::native_boundary::NativeBoundary,
) -> Status {
    let Some(capture) = (unsafe { capture.as_ref() }) else {
        return Status::INVALID_PARAMETER;
    };
    if !capture.has_valid_shape() {
        return Status::UNSUPPORTED;
    }
    unsafe { native_entry::run(image, table, capture) }
}

// The default ROM image has no runtime panic policy. A reachable Rust panic makes linking
// fail; size-optimized LTO must prove this handler unreachable. This avoids
// pulling in the general UEFI crate's console/delay/shutdown panic machinery.
#[cfg(all(target_os = "uefi", not(feature = "emulator-handoff"), not(feature = "card-load-only"), not(feature="native-resident")))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn svmvisor_dxe_must_not_panic() -> !;
    }
    unsafe { svmvisor_dxe_must_not_panic() }
}

// The resident candidate has an explicit terminal invariant-failure path.
// Expected admission refusals return through the original callback instead.
#[cfg(all(target_os="uefi", feature="native-resident"))]
#[panic_handler]
fn resident_panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(feature="native-resident-test")]
    for byte in b"resident-dxe-panic\n" {
        unsafe {core::arch::asm!("out dx,al",in("dx")0xe9u16,in("al")*byte,options(nomem,nostack));}
    }
    loop {unsafe {core::arch::asm!("cli; hlt",options(nomem,nostack));}}
}

// Candidate-only last resort for an internal invariant failure. Expected bad
// card data never takes this path. No firmware calls or transfer are possible;
// recovery requires an external reset. Default record-only policy is unchanged.
#[cfg(all(target_os = "uefi", feature = "card-load-only"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}

#[cfg(all(target_os = "uefi", feature = "emulator-handoff"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    svmvisor_firmware_handoff::panic_fail()
}

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("svmvisor-dxe is a UEFI-only driver; use cargo build-dxe");
}
