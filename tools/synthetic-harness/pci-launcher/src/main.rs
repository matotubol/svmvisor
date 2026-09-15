//! OVMF fixture launcher for a driver dispatched from a PCI option ROM.
//! It neither embeds nor loads a driver and never exits boot services itself.
#![no_std]
#![no_main]

use core::{arch::asm, panic::PanicInfo};
use uefi::{
    Status,
    boot::{self, SearchType},
    entry, guid,
    proto::unsafe_protocol,
};

const PCI_IO: uefi::Guid = guid!("4cf5b200-68b8-4ca5-9eec-b23e3f50029a");
const HANDOFF: uefi::Guid = guid!("c5a5c5a5-726f-4d44-8877-736d76706963");
static TOKEN: &[u8] = b"SVMVISOR-EMULATOR-HANDOFF-V1";
static WRONG_TOKEN: &[u8] = b"SVMVISOR-EMULATOR-HANDOFF-V0";

#[repr(C)]
#[unsafe_protocol(HANDOFF)]
struct HandoffProtocol {
    revision: u64,
    run: unsafe extern "efiapi" fn(token: *const u8, length: usize) -> Status,
}

fn debug(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}

fn finish(message: &str, pass: bool) -> ! {
    debug(message);
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4u16, in("eax") if pass { 16u32 } else { 17u32 }, options(nomem, nostack));
    }
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    finish("FAIL pci-launcher-panic\n", false)
}

#[entry]
fn main() -> Status {
    debug("pci-launcher-entry\n");
    {
        let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&PCI_IO)) {
            Ok(handles) => handles,
            Err(_) => finish("FAIL pci-binding-not-found\n", false),
        };
        if handles.len() > 64 {
            finish("FAIL pci-handle-limit\n", false);
        }
        for handle in handles.iter().copied() {
            // Firmware chooses matching Driver Binding instances. Unsupported
            // devices are expected; success is established by the protocol below.
            let _ = boot::connect_controller(handle, &[], None, true);
        }
    }
    let mut handle = match boot::get_handle_for_protocol::<HandoffProtocol>() {
        Ok(handle) => handle,
        Err(_) => finish("FAIL pci-binding-not-found\n", false),
    };
    if cfg!(feature = "reconnect") {
        // The fixture publishes this protocol on its driver image handle.
        // Ask firmware to stop that driver on PCI controllers; require exactly
        // one successful disconnection rather than accepting a no-op retry.
        let driver_image = handle;
        let mut disconnected = 0;
        {
            let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&PCI_IO)) {
                Ok(handles) => handles,
                Err(_) => finish("FAIL pci-reconnect-enumeration\n", false),
            };
            if handles.len() > 64 {
                finish("FAIL pci-handle-limit\n", false);
            }
            for controller in handles.iter().copied() {
                if boot::disconnect_controller(controller, Some(driver_image), None).is_ok() {
                    disconnected += 1;
                }
            }
        }
        if disconnected != 1 {
            finish("FAIL pci-disconnect-count\n", false);
        }
        match boot::get_handle_for_protocol::<HandoffProtocol>() {
            Err(error) if error.status() == Status::NOT_FOUND => (),
            _ => finish("FAIL pci-handoff-still-published\n", false),
        }
        debug("pci-binding-disconnected\n");
        {
            let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&PCI_IO)) {
                Ok(handles) => handles,
                Err(_) => finish("FAIL pci-reconnect-enumeration\n", false),
            };
            if handles.len() > 64 {
                finish("FAIL pci-handle-limit\n", false);
            }
            for controller in handles.iter().copied() {
                let _ = boot::connect_controller(controller, &[], None, true);
            }
        }
        handle = match boot::get_handle_for_protocol::<HandoffProtocol>() {
            Ok(handle) => handle,
            Err(_) => finish("FAIL pci-reconnect-not-found\n", false),
        };
        if handle != driver_image {
            finish("FAIL pci-reconnect-driver-changed\n", false);
        }
        debug("pci-binding-reconnected\n");
    }
    let run = {
        let protocol = match boot::open_protocol_exclusive::<HandoffProtocol>(handle) {
            Ok(protocol) => protocol,
            Err(_) => finish("FAIL pci-handoff-open\n", false),
        };
        if protocol.revision != 1 {
            finish("FAIL pci-handoff-revision\n", false);
        }
        protocol.run
    };
    // No handle buffers or protocol guards survive across the call: an
    // authorized driver may exit boot services and never return.
    debug("pci-binding-found\n");
    let token = if cfg!(feature = "no-authorization") {
        WRONG_TOKEN
    } else {
        TOKEN
    };
    let status = unsafe { run(token.as_ptr(), token.len()) };
    if cfg!(feature = "no-authorization") && status == Status::ACCESS_DENIED {
        finish("PASS pci-authorization-rejected\n", true);
    }
    finish("FAIL pci-handoff-returned\n", false)
}
