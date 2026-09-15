use std::{env, fs, path::PathBuf};

fn main() {
    if env::var_os("CARGO_FEATURE_DRIVER").is_some() {
        println!("cargo:rustc-link-arg-bin=BOOTX64=/subsystem:efi_boot_service_driver");
    }
    println!("cargo:rerun-if-env-changed=SVMVISOR_PAYLOAD");
    println!("cargo:rerun-if-env-changed=SVMVISOR_ENTRY");
    let payload = PathBuf::from(
        env::var_os("SVMVISOR_PAYLOAD").expect("Set SVMVISOR_PAYLOAD to the flat emulator payload"),
    );
    let payload = fs::canonicalize(payload).expect("Payload path must exist");
    let bytes = fs::read(&payload).expect("Read payload");
    assert!(
        bytes.len() >= 64 && &bytes[..8] == b"SVMRELO1",
        "Payload must be a relocation package"
    );
    let entry: usize = env::var("SVMVISOR_ENTRY")
        .expect("Set SVMVISOR_ENTRY to the decimal byte offset from 0x100000")
        .parse()
        .expect("Entry offset must be decimal");
    let image_bytes = u64::from_le_bytes(bytes[24..32].try_into().unwrap()) as usize;
    let package_entry = u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize;
    assert!(
        entry < image_bytes && entry == package_entry,
        "Entry must match packaged image"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("payload.bin"), bytes).unwrap();
    fs::write(
        out.join("entry.rs"),
        format!("const ENTRY_OFFSET: usize = {entry};\n"),
    )
    .unwrap();
    println!("cargo:rerun-if-changed={}", payload.display());
}
