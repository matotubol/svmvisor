//! OVMF-only launcher for the separately built boot-services driver image.
#![no_std]
#![no_main]

use core::{arch::asm, panic::PanicInfo};
use uefi::{
    Status,
    boot::{self, LoadImageSource},
    entry,
};

static DRIVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/driver.efi"));

fn debug(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0xe9_u16, in("al") byte, options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    debug("FAIL uefi-launcher\n");
    terminate(false)
}

// Disposable emulator process outcome; no boot-services call or firmware return.
fn terminate(success: bool) -> ! {
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4_u16, in("eax") if success {16_u32}else{17_u32}, options(nomem, nostack));
    }
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    debug("uefi-launcher-entry\n");
    let handle = match boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromBuffer {
            buffer: DRIVER,
            file_path: None,
        },
    ) {
        Ok(handle) => handle,
        Err(_) => {
            debug("FAIL uefi-driver-load\n");
            terminate(false)
        }
    };
    debug("uefi-driver-loaded\n");
    if !cfg!(feature = "no-authorization") {
        let mut loaded = boot::open_protocol_exclusive::<uefi::proto::loaded_image::LoadedImage>(handle).unwrap();
        static TOKEN: &[u8] = b"SVMVISOR-EMULATOR-HANDOFF-V1";
        unsafe { loaded.set_load_options(TOKEN.as_ptr().cast(), TOKEN.len() as u32); }
    }
    // The fixture driver exits boot services and transfers to its payload, so
    // successful execution intentionally never returns to the launcher.
    match boot::start_image(handle) {
        Ok(()) => {
            debug("FAIL uefi-driver-returned\n");
            terminate(false)
        }
        Err(error) => {
            if cfg!(feature = "no-authorization") && error.status() == Status::ACCESS_DENIED {
                debug("PASS handoff-not-authorized\n");
                terminate(true)
            }
            debug("FAIL uefi-driver-start\n");
            terminate(false)
        }
    }
}
