fn main() {
    println!("cargo:rerun-if-env-changed=SVMVISOR_UEFI_SMP");
    let smp = std::env::var("SVMVISOR_UEFI_SMP").unwrap_or_default();
    assert!(matches!(smp.as_str(), "" | "0" | "1"));
    println!("cargo:rustc-env=SVMVISOR_SELECTED_UEFI_SMP={}", if smp == "1" { "1" } else { "0" });
    println!("cargo:rerun-if-env-changed=SVMVISOR_REJECT_SMP_RECORD");
    let reject_smp = std::env::var("SVMVISOR_REJECT_SMP_RECORD").unwrap_or_default();
    assert!(matches!(reject_smp.as_str(), "" | "0" | "1"));
    assert!(
        reject_smp != "1" || smp == "1",
        "SVMVISOR_REJECT_SMP_RECORD requires SVMVISOR_UEFI_SMP=1"
    );
    println!(
        "cargo:rustc-env=SVMVISOR_SELECTED_REJECT_SMP_RECORD={}",
        if reject_smp == "1" { "1" } else { "0" }
    );
    println!("cargo:rerun-if-env-changed=SVMVISOR_SIPI_PAGE");
    let raw_page = std::env::var("SVMVISOR_SIPI_PAGE").unwrap_or_default();
    let page = if raw_page.is_empty() {
        0
    } else if let Some(hex) = raw_page.strip_prefix("0x").or_else(|| raw_page.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).expect("SVMVISOR_SIPI_PAGE must be a hexadecimal address")
    } else {
        raw_page.parse::<u64>().expect("SVMVISOR_SIPI_PAGE must be a decimal address")
    };
    assert!(raw_page.is_empty() || smp == "1", "SVMVISOR_SIPI_PAGE requires SVMVISOR_UEFI_SMP=1");
    // Keep malformed addresses representable so the actual pre-EBS refusal
    // path, including explicit zero, can be exercised by the negative fixture.
    println!("cargo:rustc-env=SVMVISOR_SELECTED_SIPI_PAGE={page}");
    println!(
        "cargo:rustc-env=SVMVISOR_SELECTED_SIPI_PAGE_REQUESTED={}",
        if raw_page.is_empty() { "0" } else { "1" }
    );
    println!("cargo:rerun-if-env-changed=SVMVISOR_REJECT_RESIDENT_OWNERSHIP");
    let reject = std::env::var("SVMVISOR_REJECT_RESIDENT_OWNERSHIP").unwrap_or_default();
    assert!(matches!(reject.as_str(), "" | "0" | "1"));
    println!(
        "cargo:rustc-env=SVMVISOR_SELECTED_REJECT_RESIDENT_OWNERSHIP={}",
        if reject == "1" { "1" } else { "0" }
    );
    println!("cargo:rerun-if-env-changed=SVMVISOR_ARENA_BASE");
    let raw = std::env::var("SVMVISOR_ARENA_BASE").unwrap_or_default();
    let value = if raw.is_empty() {
        0
    } else if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).expect("SVMVISOR_ARENA_BASE must be a hexadecimal address")
    } else {
        raw.parse::<u64>().expect("SVMVISOR_ARENA_BASE must be a decimal address")
    };
    if !raw.is_empty() {
        assert!(value != 0, "an explicit arena base must not be zero");
    }
    println!("cargo:rustc-env=SVMVISOR_SELECTED_ARENA_BASE={value}");
}
