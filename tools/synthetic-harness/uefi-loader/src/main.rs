//! Emulator-only OVMF application. This is not the physical DXE driver.
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use svmvisor_firmware_handoff::{debug, panic_fail, run_initialized};
use uefi::{Status, entry};

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
include!(concat!(env!("OUT_DIR"), "/entry.rs"));

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    panic_fail()
}

#[entry]
fn main() -> Status {
    if cfg!(feature = "driver") {
        debug("uefi-driver-entry\n");
    }
    // #[entry] initializes the globals. The launcher must supply LoadOptions.
    unsafe { run_initialized(PAYLOAD, ENTRY_OFFSET, cfg!(feature = "bad-handoff")) }
}
