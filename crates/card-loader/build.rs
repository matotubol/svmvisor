use svmvisor_card_abi::envelope::{HEADER_BYTES, MAGIC};

fn main() {
    let pinned_resident = std::env::var_os("CARGO_FEATURE_CARD_RESIDENT_LOADER").is_some();
    let dev_resident = std::env::var_os("CARGO_FEATURE_CARD_RESIDENT_DEV_LOADER").is_some();
    assert!(
        !(pinned_resident && dev_resident),
        "card-resident-loader (compiled-in header pin) and card-resident-dev-loader (header trusted from the flash slot) are mutually exclusive; enable exactly one"
    );
    // The dev loader has no compiled-in header: it needs no SVMVISOR_CARD_PE_HEADER.
    if pinned_resident {
        println!("cargo:rerun-if-env-changed=SVMVISOR_CARD_PE_HEADER");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
            let pin = std::fs::canonicalize(
                std::env::var_os("SVMVISOR_CARD_PE_HEADER")
                    .expect("card-resident-loader requires SVMVISOR_CARD_PE_HEADER"),
            )
            .expect("resident PE pin must exist");
            let bytes = std::fs::read(&pin).expect("read resident PE pin");
            assert!(
                bytes.len() == HEADER_BYTES && bytes[..8] == MAGIC,
                "resident PE pin must be the 128-byte SVMBPE01 envelope"
            );
            let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
            std::fs::write(out.join("card-pe-header.bin"), bytes).expect("copy resident PE pin");
            println!("cargo:rerun-if-changed={}", pin.display());
        }
    }
    // The Rust UEFI target defaults to EFI_APPLICATION. This package is the
    // resident option-ROM DXE driver, so give only this PE/COFF image the boot
    // service driver subsystem. Other UEFI packages keep their own subsystem.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
        println!("cargo:rustc-link-arg=/subsystem:efi_boot_service_driver");
        // Keep constants read-only alongside code to conserve the ROM aperture.
        // Writable binding state remains in its own non-executable .data section.
        println!("cargo:rustc-link-arg=/merge:.rdata=.text");
        // No CodeView/PDB directory in the constrained ROM. A fixed timestamp
        // also makes repeated links of the same inputs produce identical bytes.
        println!("cargo:rustc-link-arg=/debug:none");
        println!("cargo:rustc-link-arg=/timestamp:0");
    }
}
