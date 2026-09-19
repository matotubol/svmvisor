//! Diagnostic post-EBS physical activation interface. The caller establishes
//! successful ExitBootServices return; an event notification is insufficient.
use core::sync::atomic::AtomicU32;

use uefi_raw::{Guid, guid};

pub const ACTIVATION_GUID: Guid = guid!("eb2dfe74-3958-4a2f-98aa-f5c347fa7628");

#[repr(C)]
pub struct ActivationInterface {
    pub version: u64,
    pub count: u64,
    pub pool_base: u64,
    pub pool_bytes: u64,
    /// # Safety
    /// BSP only, IF=0, after successful ExitBootServices return, in the
    /// admitted identity mapping. Retain the driver's image, complete runtime
    /// pool and owned AP bootstrap root/code until reset. Retain the low startup
    /// page until this call returns zero; it has then been consumed by every AP.
    /// The original BSP root/stack remain ordinary guest continuation resources
    /// until the guest replaces them; APs retain no firmware-table dependency.
    /// No firmware MP call may remain active. This diagnostic consumer
    /// owns AP startup; it is not an unmodified OS-loader entry contract.
    /// PI1.10 II-13.4.1; UEFI2.11 7.4.6; AMD APM2 rev3.44 14.1/16.5.
    /// Zero confirms every captured guest continuation; 32 rejects a repeated
    /// call or invalid BSP entry. Other errors retain all active resources.
    pub start: unsafe extern "efiapi" fn() -> u64,
    pub completed: AtomicU32,
    pub failed: AtomicU32,
}

const _: () = {
    assert!(core::mem::size_of::<ActivationInterface>() == 48);
    assert!(core::mem::align_of::<ActivationInterface>() == 8);
    assert!(core::mem::offset_of!(ActivationInterface, version) == 0);
    assert!(core::mem::offset_of!(ActivationInterface, count) == 8);
    assert!(core::mem::offset_of!(ActivationInterface, pool_base) == 16);
    assert!(core::mem::offset_of!(ActivationInterface, pool_bytes) == 24);
    assert!(core::mem::offset_of!(ActivationInterface, start) == 32);
    assert!(core::mem::offset_of!(ActivationInterface, completed) == 40);
    assert!(core::mem::offset_of!(ActivationInterface, failed) == 44);
};
