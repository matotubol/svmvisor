fn main() {
    // Explicit evidence schema and scenario; absent variables retain the
    // historical detail-6 harness contract for frozen deliveries.
    for key in [
        "SVMVISOR_JOURNAL_DETAIL",
        "SVMVISOR_RETURNING_MODE",
        "SVMVISOR_TERMINAL_EBS",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    for name in [
        "diagnostics_v7",
        "pristine_refused",
        "structured_refused",
        "terminal_ebs",
    ] {
        println!("cargo:rustc-check-cfg=cfg({name})");
    }
    let detail = std::env::var("SVMVISOR_JOURNAL_DETAIL").unwrap_or_else(|_| "6".into());
    let resident = std::env::var_os("CARGO_FEATURE_RESIDENT").is_some();
    assert!(if resident { detail == "8" } else { detail == "6" || detail == "7" });
    if detail == "7" {
        println!("cargo:rustc-cfg=diagnostics_v7");
    }
    match std::env::var("SVMVISOR_RETURNING_MODE")
        .as_deref()
        .unwrap_or("Positive")
    {
        "Positive" | "Header" | "Digest" | "Admission" => {}
        "PristineRefused" => {
            assert_eq!(detail, "7");
            println!("cargo:rustc-cfg=pristine_refused");
        }
        "StructuredRefused" => {
            assert_eq!(detail, "7");
            println!("cargo:rustc-cfg=structured_refused");
        }
        _ => panic!("unknown returning-card scenario"),
    }
    if std::env::var("SVMVISOR_TERMINAL_EBS").as_deref() == Ok("1") {
        assert_eq!(detail, "7");
        assert_eq!(
            std::env::var("SVMVISOR_RETURNING_MODE").as_deref(),
            Ok("StructuredRefused")
        );
        println!("cargo:rustc-cfg=terminal_ebs");
    }
    println!("cargo:rerun-if-env-changed=SVMVISOR_RETURNING_PARENT");
    let source = std::env::var("SVMVISOR_RETURNING_PARENT").expect("explicit driver required");
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
    println!("cargo:rerun-if-env-changed=SVMVISOR_RETURNING_SLOT");
    let slot_path = std::env::var("SVMVISOR_RETURNING_SLOT").expect("explicit slot required");
    println!("cargo:rerun-if-changed={slot_path}");
    let slot = std::fs::read(slot_path).expect("read exact slot");
    assert_eq!(slot.len(), 0x100000);
    assert_eq!(&slot[..8], if resident { b"SVMBPE01" } else { b"SVMPE001" });
    std::fs::write(out.join("slot.bin"), slot).unwrap();
    std::fs::write(out.join("driver.efi"), bytes).unwrap();
    {
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
