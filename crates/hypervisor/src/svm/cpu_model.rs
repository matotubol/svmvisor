//! CPUID policy of the native boot guest (`native_boot_cpuid`).
//!
//! `CpuModelError` is what remains of the synthetic AMD CPU model. It stays
//! because `svm::dispatch::DispatchError::CpuModel` carries it, and that enum is
//! compiled into the resident payload.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuModelError {
    UnsupportedVendor,
    MissingHostLeaves,
    MissingBaselineFeatures,
    InvalidAddressWidth,
    InvalidClflushSize,
    XstateNotSupported,
    ClockNotSupported,
    NxNotSupported,
    InvalidVcpuId,
    InvalidGuestXstate,
    InvalidCacheEvidence,
    InvalidExtendedMetadata,
}

/// Trusted native first-boot CPUID filter, separate from the synthetic model.
/// The runtime samples the requested leaf/subleaf on this same physical CPU
/// with guest XCR0 live. Native topology is retained: the caller must admit
/// every exposed CPU (the first executable fixture admits only one).
/// APM2 rev3.44 15.4/15.27: nested SVM/SKINIT are unavailable; no encrypted
/// guest/platform contract exists. Active host encryption or another owning
/// hypervisor must be refused at admission, not concealed by this filter.
/// Other native features remain execution capabilities, not Windows proof;
/// currently unsupported EFER feature writes stop at the EFER owner.
pub fn native_boot_cpuid(leaf: u32, mut native: [u32; 4], guest_cr4: u64) -> [u32; 4] {
    match leaf {
        1 => {
            // OSXSAVE reports the guest's CR4, not the stopped host CR4.
            native[2] = (native[2] & !(1 << 27))
                | if native[2] & (1 << 26) != 0 && guest_cr4 & (1 << 18) != 0 {
                    1 << 27
                } else {
                    0
                };
        }
        // ExtApicSpace (bit3) is absent from the native guest LAPIC model.
        // Its physical extension controls remain exclusively host-owned.
        0x8000_0001 => native[2] &= !((1 << 2) | (1 << 3) | (1 << 12)),
        0x8000_000a | 0x8000_001f | 0x8000_0023 => return [0; 4],
        _ => {}
    }
    native
}
