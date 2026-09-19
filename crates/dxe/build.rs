fn main() {
    let resident = std::env::var_os("CARGO_FEATURE_NATIVE_RESIDENT").is_some();
    if resident {
        for name in ["NATIVE_RETURNING", "NATIVE_TRANSITION_TEST"] {
            assert!(
                std::env::var_os(format!("CARGO_FEATURE_{name}")).is_none(),
                "native resident is a separate image: {name}"
            );
        }
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
            embed_resident_payload();
            assemble_native("resident_callback", false);
            if std::env::var_os("CARGO_FEATURE_NATIVE_RESIDENT_BOOT").is_some() {
                assemble_native("resident_boot", false);
            }
            if std::env::var_os("CARGO_FEATURE_NATIVE_RESIDENT_SMP_ACTIVATE").is_some() {
                assemble_native("resident_physical", false);
            }
            println!("cargo:rerun-if-changed=src/native/admission/boundary.S");
        }
    }
    let multi_exit = std::env::var_os("CARGO_FEATURE_NATIVE_TRANSITION_MULTI_EXIT").is_some();
    if multi_exit {
        for incompatible in [
            "CARGO_FEATURE_NATIVE_RETURNING",
            "CARGO_FEATURE_NATIVE_TRANSITION_ROUNDTRIP",
            "CARGO_FEATURE_NATIVE_TRANSITION_EVENT_TEST",
            "CARGO_FEATURE_NATIVE_TRANSITION_CANARY_NEGATIVE",
        ] {
            assert!(
                std::env::var_os(incompatible).is_none(),
                "multi-exit TCG fixture cannot combine with {incompatible}"
            );
        }
        let negatives = [
            "CARGO_FEATURE_NATIVE_TRANSITION_MULTI_EXIT_UNEXPECTED",
            "CARGO_FEATURE_NATIVE_TRANSITION_MULTI_EXIT_MISMATCH",
            "CARGO_FEATURE_NATIVE_TRANSITION_MULTI_EXIT_BAD_MODE",
        ]
        .into_iter()
        .filter(|feature| std::env::var_os(feature).is_some())
        .count();
        assert!(negatives <= 1, "multi-exit negative fixtures must be selected alone");
    }
    println!("cargo:rerun-if-changed=src/native/admission/snapshot.S");
    if std::env::var_os("CARGO_FEATURE_NATIVE_PREFLIGHT").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi")
    {
        assemble_native("native_snapshot", false);
        if !resident {
            assemble_native("native_boundary", false);
        }
    }
    if std::env::var_os("CARGO_FEATURE_NATIVE_RESOURCE_OBSERVE").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi")
    {
        assemble_native("native_cache", false);
    }
    if (std::env::var_os("CARGO_FEATURE_NATIVE_TRANSITION_TEST").is_some()
        || std::env::var_os("CARGO_FEATURE_NATIVE_RETURNING").is_some())
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi")
    {
        assert!(
            std::env::var_os("CARGO_FEATURE_NATIVE_RETURNING").is_none()
                || std::env::var_os("CARGO_FEATURE_NATIVE_TRANSITION_TEST").is_none(),
            "native returning admission must not link a TCG test configuration"
        );
        let transition =
            if std::env::var_os("CARGO_FEATURE_NATIVE_TRANSITION_CANARY_NEGATIVE").is_some() {
                assert!(
                    std::env::var_os("CARGO_FEATURE_NATIVE_TRANSITION_EVENT_TEST").is_none()
                        && std::env::var_os("CARGO_FEATURE_NATIVE_TRANSITION_ROUNDTRIP").is_none(),
                    "negative detector test must be selected alone"
                );
                "native_transition_canary_negative"
            } else {
                "native_transition"
            };
        for name in [transition, "native_transition_canary"] {
            assemble_native(
                name,
                name == "native_transition_canary"
                    && std::env::var_os("CARGO_FEATURE_NATIVE_RETURNING").is_some(),
            );
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
fn assemble_native(name: &str, returning_canary: bool) {
    let source = match name {
        "native_snapshot" => "src/native/admission/snapshot.S",
        "native_boundary" => "src/native/admission/boundary.S",
        "resident_callback" => "src/native/resident/bridge.S",
        "resident_boot" => "src/native/resident/boot.S",
        "resident_physical" => "src/native/resident/physical.S",
        "native_cache" => "src/native/admission/cache.S",
        "native_transition" => "src/native/transition/run.S",
        "native_transition_canary" => "src/native/transition/canary.S",
        "native_transition_canary_negative" => "src/fixtures/canary_negative.S",
        _ => panic!("unknown native assembly unit: {name}"),
    };
    println!("cargo:rerun-if-changed={source}");
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(format!("{name}.obj"));
    let mut command = std::process::Command::new("clang");
    if returning_canary {
        command.arg("-DSVMVISOR_NATIVE_RETURNING=1");
    }
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
