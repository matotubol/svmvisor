fn main() {
    println!("cargo:rerun-if-env-changed=SVMVISOR_PREFLIGHT_DRIVER");
    let source = std::env::var("SVMVISOR_PREFLIGHT_DRIVER").expect("explicit driver required");
    println!("cargo:rerun-if-changed={source}");
    let bytes = std::fs::read(source).expect("read exact test driver");
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    assert_eq!(
        u16::from_le_bytes(bytes[pe + 92..pe + 94].try_into().unwrap()),
        11,
        "test must load EFI_BOOT_SERVICE_DRIVER, not an application substitute"
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("driver.efi"), bytes).unwrap();
    if std::env::var_os("CARGO_FEATURE_BOUNDARY_CALL").is_some() {
        println!("cargo:rerun-if-changed=src/boundary_call.S");
        let object = out.join("boundary_call.obj");
        let status = std::process::Command::new("clang")
            .args([
                "--target=x86_64-pc-windows-msvc",
                "-c",
                "src/boundary_call.S",
                "-o",
            ])
            .arg(&object)
            .status()
            .expect("clang for boundary fixture");
        assert!(status.success(), "boundary fixture assembly");
        println!("cargo:rustc-link-arg={}", object.display());
    }
    println!("cargo:rustc-link-arg=/timestamp:0");
}
