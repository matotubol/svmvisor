fn main() {
    let resident = std::env::var_os("CARGO_FEATURE_NATIVE_RESIDENT").is_some();
    if resident {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
            embed_resident_payload();
            assemble_native("resident_callback");
            if std::env::var_os("CARGO_FEATURE_NATIVE_RESIDENT_BOOT").is_some() {
                assemble_native("resident_boot");
            }
            if std::env::var_os("CARGO_FEATURE_NATIVE_RESIDENT_SMP_ACTIVATE").is_some() {
                assemble_native("resident_physical");
            }
            println!("cargo:rerun-if-changed=src/native/admission/boundary.S");
        }
    }

    // The Rust UEFI target defaults to EFI_APPLICATION. The resident native child
    // is an EFI runtime driver; every other native image is a boot service
    // driver. Other UEFI packages keep their own subsystem.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
        println!(
            "cargo:rustc-link-arg=/subsystem:{}",
            if resident { "efi_runtime_driver" } else { "efi_boot_service_driver" }
        );
        // Keep constants read-only alongside code to conserve the ROM aperture.
        // Writable binding state remains in its own non-executable .data section.
        println!("cargo:rustc-link-arg=/merge:.rdata=.text");
        // No CodeView/PDB directory in the constrained ROM. A fixed timestamp
        // also makes repeated links of the same inputs produce identical bytes.
        println!("cargo:rustc-link-arg=/debug:none");
        println!("cargo:rustc-link-arg=/timestamp:0");
    }
}

// Source locations follow crate responsibilities; output object names stay
// stable for the linker, stack audit and retained assembly provenance.
fn assemble_native(name: &str) {
    let source = match name {
        "resident_callback" => "src/native/resident/bridge.S",
        "resident_boot" => "src/native/resident/boot.S",
        "resident_physical" => "src/native/resident/physical.S",
        _ => panic!("unknown native assembly unit: {name}"),
    };
    println!("cargo:rerun-if-changed={source}");
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(format!("{name}.obj"));
    let mut command = std::process::Command::new("clang");
    let status = command
        .args(["--target=x86_64-pc-windows-msvc", "-c", source, "-o"])
        .arg(&out)
        .status()
        .expect("native assembly requires clang");
    assert!(status.success(), "native assembly failed: {name}");
    println!("cargo:rustc-link-arg={}", out.display());
}

fn embed_resident_payload() {
    use std::{env, fs, path::PathBuf};
    println!("cargo:rerun-if-env-changed=SVMVISOR_RESIDENT_PAYLOAD");
    let path = fs::canonicalize(
        env::var_os("SVMVISOR_RESIDENT_PAYLOAD")
            .expect("native resident requires its audited raw payload"),
    )
    .unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.len() >= 64 && &bytes[..8] == b"SVMRELO1");
    let entry = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
    assert!(entry < u64::from_le_bytes(bytes[24..32].try_into().unwrap()));
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("resident-payload.bin"), bytes).unwrap();
    fs::write(
        out.join("resident-entry.rs"),
        format!("const RESIDENT_ENTRY_OFFSET: usize = {entry};\n"),
    )
    .unwrap();
    println!("cargo:rerun-if-changed={}", path.display());
}
