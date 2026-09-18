#![no_std]

use svmvisor_hypervisor::host::resident::{self, ResidentDirectory};

/// # Safety
/// Native DXE must establish the raw allocation, exact linked image and all
/// writable/executable identity mappings before calling this Win64 entry.
#[unsafe(no_mangle)]
pub unsafe extern "win64" fn entry_uefi(
    base: u64,
    directory: *mut ResidentDirectory,
    pool_base: u64,
    pool_bytes: u64,
    cpu_slot: u64,
    apic_id: u64,
) -> u64 {
    unsafe { resident::prepare(base, directory, pool_base, pool_bytes, cpu_slot, apic_id) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(feature = "test-output")]
    for byte in b"resident-panic\n" {
        unsafe {
            core::arch::asm!("out dx,al",in("dx")0xe9u16,in("al")*byte,options(nomem,nostack));
        }
    }
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
