fn main() {
    // The Rust UEFI target defaults to EFI_APPLICATION. This package is the
    // resident option-ROM DXE driver, so give only this PE/COFF image the boot
    // service driver subsystem. Other UEFI packages keep their own subsystem.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
        println!("cargo:rustc-link-arg=/subsystem:efi_boot_service_driver");
    }
}
