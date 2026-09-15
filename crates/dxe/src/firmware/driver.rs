//! Minimal UEFI Driver Model adapter. Normative: UEFI 2.10 §§11.1.1–4,
//! §9.1.1 Loaded Image, §14.4 PCI I/O (Mem.Read/Write, Pci.Read).
//! Entry installs binding only; Supported has no device writes. Start owns PCI
//! I/O BY_DRIVER, retains memory decoding for the trace and creates no child.
#[cfg(not(feature = "emulator-pci-handoff"))]
use crate::cpu;
use crate::lifecycle;
use crate::pci_io::{Bar0, DecodeState, PciIo, status_result};
use core::ptr::{null, null_mut};
#[cfg(not(feature = "emulator-pci-handoff"))]
use svmvisor_dxe::journal::{self, JournalIo};
use uefi_raw::Status;
use uefi_raw::{
    Handle, guid,
    protocol::{
        device_path::DevicePathProtocol, driver::DriverBindingProtocol,
        loaded_image::LoadedImageProtocol,
    },
    table::boot::{BootServices, InterfaceType},
};

const PCI_IO_GUID: uefi_raw::Guid = guid!("4cf5b200-68b8-4ca5-9eec-b23e3f50029a");
static mut SERVICES: *const BootServices = null();
static mut OWNER: Handle = null_mut();
static mut DECODE: Option<DecodeState> = None;
static mut OWNED_PCI: *const PciIo = null();
#[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
static CALLBACK_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
struct CallbackGuard;
#[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
impl CallbackGuard {
    fn acquire() -> Result<Self, Status> {
        CALLBACK_ACTIVE.compare_exchange(false, true,
            core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed)
            .map(|_| Self).map_err(|_| Status::NOT_READY)
    }
}
#[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
impl Drop for CallbackGuard {
    fn drop(&mut self) {
        CALLBACK_ACTIVE.store(false, core::sync::atomic::Ordering::Release);
    }
}
static mut BINDING: DriverBindingProtocol = DriverBindingProtocol {
    supported,
    start,
    stop,
    version: 0x10,
    image_handle: null_mut(),
    driver_binding_handle: null_mut(),
};

// SAFETY of the firmware adapter: UEFI calls entry once and binding callbacks at
// TPL_APPLICATION (§11.1). SERVICES/OWNER/binding handles are initialized before
// publishing the protocol and thereafter immutable. PCI interface pointers are
// used only between successful OpenProtocol(BY_DRIVER) and CloseProtocol.
fn services() -> &'static BootServices {
    unsafe { &*SERVICES }
}

pub(super) unsafe fn install(
    image: Handle,
    table: *const uefi_raw::table::system::SystemTable,
) -> Status {
    if image.is_null() || table.is_null() {
        return Status::INVALID_PARAMETER;
    }

    unsafe {
        SERVICES = (*table).boot_services;
    }
    let mut loaded = null_mut();
    let status = unsafe {
        (services().open_protocol)(
            image,
            &LoadedImageProtocol::GUID,
            &mut loaded,
            image,
            null_mut(),
            2,
        )
    };
    if status.is_error() {
        return status;
    }
    if loaded.is_null() {
        return Status::DEVICE_ERROR;
    }
    let owner = unsafe { (*(loaded as *const LoadedImageProtocol)).device_handle };
    #[cfg(not(feature = "card-resident-loader"))]
    let close = unsafe {
        (services().close_protocol)(image, &LoadedImageProtocol::GUID, image, null_mut())
    };
    #[cfg(feature = "card-resident-loader")]
    let close = Status::SUCCESS;
    if close.is_error() {
        return close;
    }
    if owner.is_null() {
        return Status::UNSUPPORTED;
    }
    unsafe {
        OWNER = owner;
        BINDING.image_handle = image;
        BINDING.driver_binding_handle = image;
        let mut handle = image;
        (services().install_protocol_interface)(
            &mut handle,
            &DriverBindingProtocol::GUID,
            InterfaceType::NATIVE_INTERFACE,
            (&raw const BINDING).cast(),
        )
    }
}

fn open(controller: Handle) -> Result<*const PciIo, Status> {
    if controller != unsafe { OWNER } {
        return Err(Status::UNSUPPORTED);
    }
    let mut pci = null_mut();
    status_result(unsafe {
        (services().open_protocol)(
            controller,
            &PCI_IO_GUID,
            &mut pci,
            BINDING.driver_binding_handle,
            controller,
            0x10,
        )
    })?;
    if pci.is_null() {
        let _ = close(controller);
        return Err(Status::DEVICE_ERROR);
    }
    Ok(pci.cast())
}
fn close(controller: Handle) -> Status {
    unsafe {
        (services().close_protocol)(
            controller,
            &PCI_IO_GUID,
            BINDING.driver_binding_handle,
            controller,
        )
    }
}

unsafe extern "efiapi" fn supported(
    _: *const DriverBindingProtocol,
    controller: Handle,
    _: *const DevicePathProtocol,
) -> Status {
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    let _guard = match CallbackGuard::acquire() { Ok(guard) => guard, Err(e) => return e };
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    if crate::card_returning_adapter::has_attempted() { return Status::UNSUPPORTED; }
    let pci = match open(controller) {
        Ok(pci) => pci,
        Err(e) => return e,
    };
    let result = identity(&Bar0(pci));
    let closed = close(controller);
    match result {
        Err(e) => e,
        Ok(()) => {
            #[cfg(feature = "emulator-pci-handoff")]
            if !closed.is_error() {
                svmvisor_firmware_handoff::debug("pci-binding-supported\n");
            }
            closed
        }
    }
}

fn identity(io: &Bar0) -> Result<(), Status> {
    #[cfg(feature = "emulator-pci-handoff")]
    {
        // QEMU 10.1 pci-testdev: a fixture identity, never the physical card.
        if io.config(0)? != 0x0005_1b36 {
            return Err(Status::UNSUPPORTED);
        }
        Ok(())
    }
    #[cfg(not(feature = "emulator-pci-handoff"))]
    io.identity()
}

#[cfg(not(feature = "emulator-pci-handoff"))]
fn mark(io: &mut Bar0, boot_id: u32, tsc: u64, cpu: u32) -> Result<(), Status> {
    if io.read(0)? != 0x4a4d5653 || io.read(4)? & !0x00020000 != 0x00010001 {
        return Err(Status::UNSUPPORTED);
    }
    if io.read(0x024)? != 0 {
        return Err(Status::DEVICE_ERROR);
    }
    let sequence = io.read(0x02c)?.wrapping_add(1);
    // Context is ASCII "DXEMARK2" in little-endian byte order.
    let mut record = [
        sequence,
        boot_id,
        tsc as u32,
        (tsc >> 32) as u32,
        0x4d455844,
        0x324b5241,
        cpu,
        0x00020010,
    ];
    journal::commit(io, record)?;
    record[0] = sequence.wrapping_add(1);
    record[7] = 0x00020013; // detail 2: scoped MSE marker, first commit verified
    journal::commit(io, record)
}

unsafe extern "efiapi" fn start(
    _: *const DriverBindingProtocol,
    controller: Handle,
    _: *const DevicePathProtocol,
) -> Status {
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    let _guard = match CallbackGuard::acquire() { Ok(guard) => guard, Err(e) => return e };
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    if unsafe { !OWNED_PCI.is_null() } { return Status::ALREADY_STARTED; }
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    if crate::card_returning_adapter::has_attempted() { return Status::UNSUPPORTED; }
    let pci = match open(controller) {
        Ok(pci) => pci,
        Err(e) => return e,
    };
    unsafe {
        OWNED_PCI = pci;
    }
    let mut io = Bar0(pci);
    #[cfg(feature = "emulator-pci-handoff")]
    let setup = (|| {
        identity(&io)?;
        let original = io.enable_memory_decode()?;
        unsafe {
            DECODE = Some(original);
        }
        crate::emulator_mmio::verify(&mut io)?;
        // This fixture exercises real PCI I/O ownership and decode management,
        // not the production journal aperture or lifecycle publication.
        let published = unsafe { crate::emulator::publish() };
        status_result(published)?;
        svmvisor_firmware_handoff::debug("pci-binding-start\n");
        Ok(())
    })();
    #[cfg(not(feature = "emulator-pci-handoff"))]
    let setup = (|| {
        io.identity()?;
        let mut mapping = io.journal_mapping(services())?;
        let original = io.enable_memory_decode()?;
        unsafe {
            DECODE = Some(original);
        }
        // Validate direct mapping while all boot protocols are still available.
        if mapping.read(0)? != 0x4a4d5653 || mapping.read(4)? & !0x00020000 != 0x00010001 {
            return Err(Status::DEVICE_ERROR);
        }
        let (tsc, cpu) = cpu::sample();
        let boot_id = (tsc as u32) ^ (tsc >> 32) as u32;
        mark(&mut io, boot_id, tsc, cpu)?;
        #[cfg(feature = "card-load-only")]
        crate::card_load::verify(&mut io, services(), boot_id, tsc, cpu)?;
        #[cfg(feature = "card-returning-loader")]
        crate::card_returning_adapter::execute(
            &mut io, services(), unsafe { BINDING.image_handle }, controller, boot_id, tsc,
        )?;
        #[cfg(feature = "card-resident-loader")]
        {
            // No fallible registration follows successful resident StartImage.
            lifecycle::register(services(), mapping, boot_id)?;
            crate::card_returning_adapter::execute_resident(
                &mut io, services(), unsafe { BINDING.image_handle }, controller,
                boot_id, mapping.physical_base())
        }
        #[cfg(not(feature = "card-resident-loader"))]
        lifecycle::register(services(), mapping, boot_id)
    })();
    match setup {
        Ok(()) => Status::SUCCESS,
        Err(error) => {
            let cleanup = cleanup(controller, &mut io);
            #[cfg(feature = "emulator-pci-handoff")]
            if !cleanup.is_error() {
                svmvisor_firmware_handoff::debug("pci-start-failure-cleaned\n");
            }
            if cleanup.is_error() { cleanup } else { error }
        }
    }
}

unsafe extern "efiapi" fn stop(
    _: *const DriverBindingProtocol,
    controller: Handle,
    children: usize,
    _: *const Handle,
) -> Status {
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    let _guard = match CallbackGuard::acquire() { Ok(guard) => guard, Err(e) => return e };
    if children != 0 || controller != unsafe { OWNER } {
        return Status::UNSUPPORTED;
    }
    if lifecycle::has_exited() {
        return Status::UNSUPPORTED;
    }
    let pci = unsafe { OWNED_PCI };
    if pci.is_null() {
        return Status::NOT_STARTED;
    }
    let status = cleanup(controller, &mut Bar0(pci));
    #[cfg(feature = "emulator-pci-handoff")]
    if !status.is_error() {
        svmvisor_firmware_handoff::debug("pci-binding-stop\n");
    }
    status
}

fn cleanup(controller: Handle, io: &mut Bar0) -> Status {
    #[cfg(any(feature = "card-returning-loader", feature = "card-resident-loader"))]
    if let Err(error) = crate::card_returning_adapter::cleanup(services()) {
        return error;
    }
    #[cfg(feature = "card-load-only")]
    if let Err(error) = crate::card_load::cleanup(services()) {
        return error;
    }
    #[cfg(feature = "emulator-pci-handoff")]
    {
        let revoked = unsafe { crate::emulator::unpublish() };
        if revoked.is_error() {
            return revoked;
        }
    }
    let events = lifecycle::unregister(services());
    let restore = match unsafe { DECODE } {
        Some(original) => io.restore_memory_decode(original),
        None => Ok(()),
    };
    // Keep ownership/context if cleanup failed so Stop can retry safely. No
    // callback remains armed, and no freed context can be dereferenced.
    if let Err(error) = events.and(restore) {
        return error;
    }
    unsafe {
        DECODE = None;
    }
    let status = close(controller);
    if !status.is_error() {
        unsafe {
            OWNED_PCI = null();
        }
    }
    status
}
