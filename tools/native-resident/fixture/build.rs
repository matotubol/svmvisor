fn main() {
    println!("cargo:rerun-if-env-changed=SVMVISOR_RESIDENT_DRIVER");
    println!("cargo:rerun-if-env-changed=CLANG");
    println!("cargo:rerun-if-changed=src/witness.S");
    let source =
        std::env::var("SVMVISOR_RESIDENT_DRIVER").expect("explicit resident driver required");
    println!("cargo:rerun-if-changed={source}");
    let bytes = std::fs::read(source).expect("read exact resident driver");
    assert!(bytes.len() >= 0x40 && &bytes[..2] == b"MZ");
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    assert!(pe.checked_add(94).is_some_and(|end| end <= bytes.len()));
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    let subsystem = u16::from_le_bytes(bytes[pe + 92..pe + 94].try_into().unwrap());
    assert!(
        matches!(subsystem, 11 | 12),
        "requires a real EFI driver, not an application substitute"
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("driver.efi"), bytes).unwrap();
    let object = out.join("witness.obj");
    let status =
        std::process::Command::new(std::env::var_os("CLANG").unwrap_or_else(|| "clang".into()))
            .args([
                "--target=x86_64-pc-windows-msvc",
                "-c",
                "src/witness.S",
                "-o",
            ])
            .arg(&object)
            .status()
            .expect("clang for signal witness");
    assert!(status.success(), "compile signal witness");
    println!("cargo:rustc-link-arg={}", object.display());
    if std::env::var_os("CARGO_FEATURE_GUEST_STARTUP").is_some() {
        println!("cargo:rerun-if-changed=src/startup.S");
        let object = out.join("startup.obj");
        let status =
            std::process::Command::new(std::env::var_os("CLANG").unwrap_or_else(|| "clang".into()))
                .args([
                    "--target=x86_64-pc-windows-msvc",
                    "-c",
                    "src/startup.S",
                    "-o",
                ])
                .arg(&object)
                .arg(if std::env::var_os("CARGO_FEATURE_GUEST_VMCR").is_some() {
                    "-DSVMVISOR_GUEST_VMCR=1"
                } else { "-DSVMVISOR_GUEST_VMCR=0" })
                .arg(if std::env::var_os("CARGO_FEATURE_GUEST_CACHE").is_some() {
                    "-DSVMVISOR_GUEST_CACHE=1"
                } else { "-DSVMVISOR_GUEST_CACHE=0" })
                .arg(if std::env::var_os("CARGO_FEATURE_GUEST_CPUID_NRIP").is_some() {
                    "-DSVMVISOR_GUEST_CPUID_NRIP=1"
                } else { "-DSVMVISOR_GUEST_CPUID_NRIP=0" })
                .status()
                .expect("clang for guest startup");
        assert!(status.success(), "compile guest startup");
        println!("cargo:rustc-link-arg={}", object.display());
    }
    println!("cargo:rustc-link-arg=/timestamp:0");
    if std::env::var_os("CARGO_FEATURE_GUEST_APIC_CONTRACT").is_some() {
        println!("cargo:rerun-if-changed=src/apic_contract.S");
        let object = out.join("apic-contract.obj");
        let status = std::process::Command::new(std::env::var_os("CLANG").unwrap_or_else(|| "clang".into()))
            .args(["--target=x86_64-pc-windows-msvc", "-c", "src/apic_contract.S", "-o"])
            .arg(&object).status().expect("clang for APIC contract fixture");
        assert!(status.success(), "compile APIC contract fixture");
        println!("cargo:rustc-link-arg={}", object.display());
    }
    if std::env::var_os("CARGO_FEATURE_GUEST_VMCR").is_some() {
        println!("cargo:rerun-if-changed=src/vmcr.S");
        let object = out.join("vmcr.obj");
        let status = std::process::Command::new(std::env::var_os("CLANG").unwrap_or_else(|| "clang".into()))
            .args(["--target=x86_64-pc-windows-msvc", "-c", "src/vmcr.S", "-o"])
            .arg(&object).status().expect("clang for VM_CR fixture");
        assert!(status.success(), "compile VM_CR fixture");
        println!("cargo:rustc-link-arg={}", object.display());
    }
}
