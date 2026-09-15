//! Explicit emulator branches of the DXE entry. The PCI feature binds QEMU's
//! test device and exposes an opt-in handoff after Start has finished. Neither
//! emulator branch may be packaged into a physical ROM.
use uefi_raw::{Handle, Status, table::system::SystemTable};

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/emulator-payload.bin"));
include!(concat!(env!("OUT_DIR"), "/emulator-entry.rs"));

/// The caller provides the original live firmware entry arguments in the
/// disposable emulator. The shared handoff owns all subsequent firmware use.
#[cfg(not(feature = "emulator-pci-handoff"))]
pub unsafe fn run(image: Handle, table: *const SystemTable) -> Status {
    svmvisor_firmware_handoff::debug("production-dxe-emulator-entry\n");
    unsafe {
        svmvisor_firmware_handoff::run(
            image,
            table,
            PAYLOAD,
            ENTRY_OFFSET,
            cfg!(feature = "emulator-reject-handoff"),
        )
    }
}

#[cfg(feature = "emulator-pci-handoff")]
pub use pci::{initialize, publish, unpublish};

#[cfg(feature = "emulator-pci-handoff")]
mod pci {
    use super::*;
    use core::{
        ptr::{null, null_mut},
        sync::atomic::{AtomicBool, Ordering},
    };
    use uefi_raw::{guid, protocol::loaded_image::LoadedImageProtocol, table::boot::InterfaceType};

    const HANDOFF_GUID: uefi_raw::Guid = guid!("c5a5c5a5-726f-4d44-8877-736d76706963");
    #[repr(C)]
    struct HandoffProtocol {
        revision: u64,
        run: unsafe extern "efiapi" fn(*const u8, usize) -> Status,
    }
    static PROTOCOL: HandoffProtocol = HandoffProtocol {
        revision: 1,
        run: handoff,
    };
    static mut IMAGE: Handle = null_mut();
    static mut TABLE: *const SystemTable = null();
    static mut PUBLISHED: bool = false;
    static STARTED: AtomicBool = AtomicBool::new(false);
    static RUNNING: AtomicBool = AtomicBool::new(false);

    pub unsafe fn initialize(image: Handle, table: *const SystemTable) -> Status {
        if image.is_null() || table.is_null() || unsafe { (*table).boot_services.is_null() } {
            return Status::INVALID_PARAMETER;
        }
        unsafe {
            IMAGE = image;
            TABLE = table;
        }
        svmvisor_firmware_handoff::debug("pci-rom-entry\n");
        Status::SUCCESS
    }

    pub unsafe fn publish() -> Status {
        if unsafe { PUBLISHED } {
            return Status::ALREADY_STARTED;
        }
        let services = unsafe { &*(*TABLE).boot_services };
        let mut handle = unsafe { IMAGE };
        let status = unsafe {
            (services.install_protocol_interface)(
                &mut handle,
                &HANDOFF_GUID,
                InterfaceType::NATIVE_INTERFACE,
                (&raw const PROTOCOL).cast(),
            )
        };
        if !status.is_error() {
            unsafe {
                PUBLISHED = true;
            }
            STARTED.store(true, Ordering::Release);
        }
        status
    }

    pub unsafe fn unpublish() -> Status {
        STARTED.store(false, Ordering::Release);
        if !unsafe { PUBLISHED } {
            return Status::SUCCESS;
        }
        let services = unsafe { &*(*TABLE).boot_services };
        let status = unsafe {
            (services.uninstall_protocol_interface)(
                IMAGE,
                &HANDOFF_GUID,
                (&raw const PROTOCOL).cast(),
            )
        };
        if !status.is_error() {
            unsafe {
                PUBLISHED = false;
            }
        }
        status
    }

    /// Firmware caller must supply a live readable token buffer for `len` bytes
    /// and call at TPL_APPLICATION after controller connection has returned.
    unsafe extern "efiapi" fn handoff(token: *const u8, len: usize) -> Status {
        let authorization = svmvisor_firmware_handoff::AUTHORIZATION;
        if token.is_null()
            || len != authorization.len()
            || unsafe { core::slice::from_raw_parts(token, len) } != authorization
        {
            svmvisor_firmware_handoff::debug("handoff-authorization-rejected\n");
            return Status::ACCESS_DENIED;
        }
        if !STARTED.load(Ordering::Acquire) {
            return Status::NOT_READY;
        }
        if RUNNING.swap(true, Ordering::AcqRel) {
            return Status::ALREADY_STARTED;
        }
        let services = unsafe { &*(*TABLE).boot_services };
        let image = unsafe { IMAGE };
        let mut loaded = null_mut();
        let opened = unsafe {
            (services.open_protocol)(
                image,
                &LoadedImageProtocol::GUID,
                &mut loaded,
                image,
                null_mut(),
                2,
            )
        };
        if opened.is_error() {
            return opened;
        }
        if loaded.is_null() {
            let _ = unsafe {
                (services.close_protocol)(image, &LoadedImageProtocol::GUID, image, null_mut())
            };
            return Status::DEVICE_ERROR;
        }
        unsafe {
            let loaded = &mut *loaded.cast::<LoadedImageProtocol>();
            loaded.load_options = authorization.as_ptr().cast();
            loaded.load_options_size = authorization.len() as u32;
        }
        let closed = unsafe {
            (services.close_protocol)(image, &LoadedImageProtocol::GUID, image, null_mut())
        };
        if closed.is_error() {
            return closed;
        }
        svmvisor_firmware_handoff::debug("pci-binding-handoff\n");
        unsafe {
            svmvisor_firmware_handoff::run(
                image,
                TABLE,
                PAYLOAD,
                ENTRY_OFFSET,
                cfg!(feature = "emulator-reject-handoff"),
            )
        }
    }
}
