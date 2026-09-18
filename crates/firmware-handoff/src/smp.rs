//! Disposable q35/OVMF two-CPU admission before ExitBootServices.
//!
//! PI 1.8A (March 2024), vol.2 MP Services StartupThisAP and Table 13.7
//! define blocking completion/timeout termination. Pinned OVMF provenance and
//! exact EDK II b158dad implementation: work/uefi-smp/research/ovmf-ap-contract.md.
//! UEFI 2.11 7.2.1/7.2.2
//! define AllocatePages/FreePages; 7.4.6 forbids their use after successful EBS.
//! AMD APM vol.2 rev.3.44 sections 16.6/15.27.8 define the SIPI page/vector;
//! the SKINIT-specific state effects in 15.27.8 do not apply to this fixture.

use core::{
    arch::x86_64::__cpuid,
    cell::UnsafeCell,
    ffi::c_void,
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use svmvisor_hypervisor::{
    boot::ownership::{SmpCpuIdentity, SmpResources},
    memory::address::{AddressPolicy, EncryptionState},
};
use uefi::{
    Status,
    boot::{self, AllocateType, MemoryType},
    proto::pi::mp::MpServices,
};

/// This owner has no Drop: freeing is an explicit pre-EBS failure operation.
/// On success the page remains owned across the nonreturning resident transfer.
pub(crate) struct PreparedSmp {
    page: NonNull<u8>,
    resources: SmpResources,
}

impl PreparedSmp {
    pub(crate) fn resources(&self) -> SmpResources {
        self.resources
    }

    /// # Safety
    /// Boot services remain active and no CPU has started from this page.
    /// UEFI 2.11 7.2.2 FreePages transfers ownership back to firmware.
    pub(crate) unsafe fn release(self) {
        unsafe {
            let _ = boot::free_pages(self.page, 1);
        }
    }
}

struct ApCapture {
    identity: UnsafeCell<SmpCpuIdentity>,
    completed: AtomicU32,
}

/// # Safety
/// Called on the BSP of the explicitly authorized disposable emulator image,
/// before EBS, with no concurrent MP procedure dispatched by this image.
/// Blocking PI StartupThisAP must implement its specified completion/timeout
/// contract: after it returns there is no executing procedure using our record.
pub(crate) unsafe fn prepare() -> Result<PreparedSmp, Status> {
    let requested = env!("SVMVISOR_SELECTED_SIPI_PAGE").parse::<u64>().unwrap();
    let explicit = env!("SVMVISOR_SELECTED_SIPI_PAGE_REQUESTED") == "1";
    if explicit && !valid_requested_page(requested) {
        crate::debug("FAIL uefi-smp-low-page-request\n");
        return Err(Status::INVALID_PARAMETER);
    }
    // The scoped protocol is dropped before any page preparation or EBS call.
    let cpus = {
        let handle = boot::get_handle_for_protocol::<MpServices>().map_err(|e| e.status())?;
        let mp = boot::open_protocol_exclusive::<MpServices>(handle).map_err(|e| e.status())?;
        let count = mp.get_number_of_processors().map_err(|e| e.status())?;
        let bsp_index = mp.who_am_i().map_err(|e| e.status())?;
        crate::debug("uefi-smp-firmware-count total=0x");
        crate::debug_hex(count.total as u64);
        crate::debug(" enabled=0x");
        crate::debug_hex(count.enabled as u64);
        crate::debug(" bsp=0x");
        crate::debug_hex(bsp_index as u64);
        crate::debug("\n");
        if count.total != 2 || count.enabled != 2 || bsp_index != 0 {
            crate::debug("REFUSE uefi-smp-topology\n");
            return Err(Status::UNSUPPORTED);
        }
        for index in 0..2 {
            let cpu = mp.get_processor_info(index).map_err(|e| e.status())?;
            if cpu.processor_id != index as u64
                || cpu.is_bsp() != (index == 0)
                || !cpu.is_enabled()
                || !cpu.is_healthy()
            {
                crate::debug("REFUSE uefi-smp-topology\n");
                return Err(Status::UNSUPPORTED);
            }
        }
        let bsp = identity(0);
        let capture = ApCapture { identity: UnsafeCell::new(bsp), completed: AtomicU32::new(0) };
        mp.startup_this_ap(
            1,
            capture_ap,
            core::ptr::from_ref(&capture).cast_mut().cast(),
            None,
            Some(Duration::from_secs(1)),
        )
        .map_err(|e| e.status())?;
        if capture.completed.load(Ordering::Acquire) != 1 {
            return Err(Status::DEVICE_ERROR);
        }
        [bsp, capture.identity.into_inner()]
    };
    crate::debug("uefi-smp-mp-callbacks-returned=1\n");
    let allocation_type =
        if explicit { AllocateType::Address(requested) } else { AllocateType::MaxAddress(0xfffff) };
    let page = boot::allocate_pages(allocation_type, MemoryType::LOADER_CODE, 1).map_err(|e| {
        crate::debug("REFUSE uefi-smp-low-page-allocation\n");
        e.status()
    })?;
    let base = page.as_ptr() as u64;
    // The physical page itself is admitted here; final-map LoaderCode coverage
    // is checked in the shared ownership encoder, before and after EBS.
    let admitted = (|| {
        if !valid_requested_page(base)
            || (explicit && base != requested)
            || __cpuid(0x80000000).eax < 0x80000008
        {
            return Err(Status::UNSUPPORTED);
        }
        let policy = AddressPolicy::new(
            __cpuid(0x80000008).eax as u8,
            EncryptionState::Unencrypted { encryption_bit: None },
        )
        .map_err(|_| Status::UNSUPPORTED)?;
        let range = policy.validate(base, 4096, 4096).map_err(|_| Status::UNSUPPORTED)?;
        SmpResources::new(range, cpus, 1).map_err(|_| Status::UNSUPPORTED)
    })();
    let resources = match admitted {
        Ok(resources) => resources,
        Err(status) => {
            unsafe {
                let _ = boot::free_pages(page, 1);
            }
            return Err(status);
        }
    };
    unsafe {
        core::ptr::write_bytes(page.as_ptr(), 0, 4096);
    }
    crate::debug("uefi-smp-low-page=0x");
    crate::debug_hex(base);
    crate::debug("\n");
    for cpu in cpus {
        crate::debug("uefi-smp-cpu processor=0x");
        crate::debug_hex(cpu.processor_id);
        crate::debug(" apic=0x");
        crate::debug_hex(cpu.apic_id as u64);
        crate::debug(" signature=0x");
        crate::debug_hex(cpu.signature as u64);
        crate::debug(" vendor=AuthenticAMD\n");
    }
    Ok(PreparedSmp { page, resources })
}

pub(crate) const fn valid_requested_page(page: u64) -> bool {
    page != 0 && page < 0x100000 && page & 4095 == 0
}

extern "efiapi" fn capture_ap(argument: *mut c_void) {
    // The blocking producer owns this stack record until StartupThisAP returns.
    // The callback does bounded CPUID/store work only and always returns.
    let capture = unsafe { &*argument.cast::<ApCapture>() };
    unsafe {
        capture.identity.get().write(identity(1));
    }
    capture.completed.fetch_add(1, Ordering::Release);
}

fn identity(processor_id: u64) -> SmpCpuIdentity {
    let vendor = __cpuid(0);
    let signature = __cpuid(1);
    let mut bytes = [0; 12];
    bytes[..4].copy_from_slice(&vendor.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&vendor.edx.to_le_bytes());
    bytes[8..].copy_from_slice(&vendor.ecx.to_le_bytes());
    SmpCpuIdentity {
        processor_id,
        apic_id: signature.ebx >> 24,
        signature: if vendor.eax >= 1 { signature.eax } else { 0 },
        vendor: bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_sipi_pages_cover_the_architectural_vector_range() {
        for page in [0x1000, 0x8000, 0x9f000, 0xff000] {
            assert!(valid_requested_page(page));
        }
        for page in [0, 1, 0x8001, 0xfffff, 0x100000, u64::MAX] {
            assert!(!valid_requested_page(page));
        }
    }

    #[test]
    fn returning_callback_captures_native_cpuid_before_publishing_completion() {
        let capture = ApCapture {
            identity: UnsafeCell::new(SmpCpuIdentity {
                processor_id: u64::MAX,
                apic_id: u32::MAX,
                signature: 0,
                vendor: [0; 12],
            }),
            completed: AtomicU32::new(0),
        };
        capture_ap(core::ptr::from_ref(&capture).cast_mut().cast());
        assert_eq!(capture.completed.load(Ordering::Acquire), 1);
        let actual = capture.identity.into_inner();
        let expected = identity(1);
        assert_eq!(actual.processor_id, 1);
        assert_eq!(actual.apic_id, expected.apic_id);
        assert_eq!(actual.signature, expected.signature);
        assert_eq!(actual.vendor, expected.vendor);
    }
}
