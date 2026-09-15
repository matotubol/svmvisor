//! Emulator-only launcher for the actual preflight DXE driver.
#![no_std]
#![no_main]
#[cfg(feature = "boundary-call")]
mod boundary_call;
#[cfg(feature = "attribute-fixture")]
mod current_attributes;
use core::arch::{asm, x86_64::__cpuid};
use uefi::{
    Status,
    boot::{self, AllocateType, LoadImageSource, MemoryType},
};

fn marker(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn finish(value: u32) -> ! {
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4u16, in("eax") value, options(nomem, nostack));
    }
    loop {
        core::hint::spin_loop();
    }
}
// Independent launcher observations, not the driver's assembly record layout.
// No table dereference, privileged state write or SVM instruction is involved.
fn observe() -> [u64; 12] {
    let mut gdt = [0u8; 10];
    let mut idt = [0u8; 10];
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    let flags: u64;
    let cs: u16;
    let ss: u16;
    let ds: u16;
    let es: u16;
    unsafe {
        asm!("sgdt [{}]", in(reg) gdt.as_mut_ptr(), options(nostack, preserves_flags));
        asm!("sidt [{}]", in(reg) idt.as_mut_ptr(), options(nostack, preserves_flags));
        asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
        asm!("mov {:x}, cs", out(reg) cs, options(nostack, preserves_flags));
        asm!("mov {:x}, ss", out(reg) ss, options(nostack, preserves_flags));
        asm!("mov {:x}, ds", out(reg) ds, options(nostack, preserves_flags));
        asm!("mov {:x}, es", out(reg) es, options(nostack, preserves_flags));
        asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags));
    }
    [
        cr0,
        cr3,
        cr4,
        flags & 0x600,
        u64::from_le_bytes(gdt[2..].try_into().unwrap()),
        u16::from_le_bytes(gdt[..2].try_into().unwrap()).into(),
        u64::from_le_bytes(idt[2..].try_into().unwrap()),
        u16::from_le_bytes(idt[..2].try_into().unwrap()).into(),
        cs.into(),
        ss.into(),
        ds.into(),
        es.into(),
    ]
}
fn hex(value: u64) {
    for shift in (0..16).rev() {
        let byte = b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize];
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
#[uefi::entry]
fn main() -> Status {
    // Deliberately check the TCG vendor even when testing hypervisor=off. The
    // driver under test keeps its real CPUID observation, with no substitution.
    let hv = __cpuid(0x40000000);
    let mut vendor = [0; 12];
    vendor[..4].copy_from_slice(&hv.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&hv.ecx.to_le_bytes());
    vendor[8..].copy_from_slice(&hv.edx.to_le_bytes());
    if &vendor != b"TCGTCGTCGTCG" {
        return Status::UNSUPPORTED;
    }
    #[cfg(feature = "attribute-fixture")]
    let attribute_handle =
        unsafe { current_attributes::install() }.expect("install emulator attribute fixture");
    #[cfg(feature = "attribute-fixture")]
    marker("FIXTURE memory-attributes installed\n");
    marker("START actual-dxe-preflight\n");
    let driver = include_bytes!(concat!(env!("OUT_DIR"), "/driver.efi"));
    let handle = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromBuffer {
            buffer: driver,
            file_path: None,
        },
    )
    .expect("load actual DXE image");
    let before = observe();
    #[cfg(feature = "boundary-call")]
    {
        boundary_call::run(handle, driver);
        boot::unload_image(handle).expect("unload directly invoked driver");
        marker("PASS directly-invoked-driver-unloaded\n");
    }
    #[cfg(not(feature = "boundary-call"))]
    let result = boot::start_image(handle);
    let after = observe();
    #[cfg(not(feature = "boundary-call"))]
    if !matches!(result, Err(ref error) if error.status() == Status::UNSUPPORTED) {
        marker("FAIL unexpected-driver-status\n");
        finish(0x11);
    }
    // Firmware unloads a driver returning an error from its entry point.
    // No stale handle use or UnloadImage call follows that ordinary return.
    marker("PASS actual-dxe-returned-unsupported\n");
    assert_eq!(before, after, "driver changed observed firmware state");
    for (index, name) in [
        "cr0",
        "cr3",
        "cr4",
        "flags-if-df",
        "gdtr-base",
        "gdtr-limit",
        "idtr-base",
        "idtr-limit",
        "cs",
        "ss",
        "ds",
        "es",
    ]
    .iter()
    .enumerate()
    {
        marker("HOST ");
        marker(name);
        marker("=");
        hex(before[index]);
        marker("\n");
    }
    marker("PASS observed-host-state-unchanged\n");
    #[cfg(feature = "attribute-fixture")]
    {
        unsafe { current_attributes::uninstall(attribute_handle) }
            .expect("remove emulator attribute fixture");
        marker("PASS memory-attribute-fixture-removed\n");
    }
    let page = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .expect("post-return allocation");
    unsafe {
        page.as_ptr().write_volatile(0x5a);
        assert_eq!(page.as_ptr().read_volatile(), 0x5a);
        boot::free_pages(page, 1).expect("post-return free");
    }
    marker("PASS post-dxe-boot-services\n");
    finish(0x10)
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    marker("FAIL preflight-launcher-panic\n");
    finish(0x11)
}
