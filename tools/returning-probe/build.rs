fn main() {
    for name in ["entry", "fixture"] {
        println!("cargo:rerun-if-changed=src/{name}.S");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("uefi") {
            continue;
        }
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
            .join(format!("{name}.obj"));
        let status = std::process::Command::new("clang")
            .args([
                "--target=x86_64-pc-windows-msvc",
                "-c",
                &format!("src/{name}.S"),
                "-o",
            ])
            .arg(&out)
            .status()
            .expect("clang must be available");
        assert!(status.success(), "Returning assembly failed: {name}");
        println!("cargo:rustc-link-arg={}", out.display());
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
        println!("cargo:rustc-link-arg=/timestamp:0");
    }
}
