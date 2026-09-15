use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=SVMVISOR_DRIVER");
    let driver = PathBuf::from(
        env::var_os("SVMVISOR_DRIVER")
            .expect("Set SVMVISOR_DRIVER to the boot-services driver EFI image"),
    );
    let driver = fs::canonicalize(driver).expect("Driver path must exist");
    let bytes = fs::read(&driver).expect("Read driver");
    // Reject accidentally embedding the direct application prototype. The PE
    // subsystem is a u16 at optional-header offset68 for PE32 and PE32+.
    assert_eq!(bytes.get(..2), Some(&b"MZ"[..]), "Expected PE DOS header");
    let pe = u32::from_le_bytes(
        bytes
            .get(0x3c..0x40)
            .expect("DOS header truncated")
            .try_into()
            .unwrap(),
    ) as usize;
    assert_eq!(
        bytes.get(pe..pe.checked_add(4).unwrap()),
        Some(&b"PE\0\0"[..]),
        "Expected PE signature"
    );
    let optional = pe.checked_add(24).unwrap();
    let subsystem = optional.checked_add(68).unwrap();
    assert_eq!(
        bytes.get(subsystem..subsystem + 2),
        Some(&11_u16.to_le_bytes()[..]),
        "Driver must have EFI boot-service-driver subsystem 11"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("driver.efi"), bytes).unwrap();
    println!("cargo:rerun-if-changed={}", driver.display());
}
