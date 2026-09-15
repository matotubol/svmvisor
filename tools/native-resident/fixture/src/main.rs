//! Disposable one-CPU QEMU consumer of the actual resident DXE driver.
//! UEFI2.11 7.1.2/7.1.4 event dispatch, 7.4.6 EBS, 8.4.1 runtime mapping.
//! No disk/network writes, physical firmware programming or Windows payload.
#![no_std]
#![no_main]

use core::{
    arch::{asm, x86_64::__cpuid},
    ffi::c_void,
    ptr::NonNull,
};
use uefi::{
    Event, Status,
    boot::{self, AllocateType, EventType, LoadImageSource, MemoryAttribute, MemoryType, Tpl},
    mem::memory_map::MemoryMap,
};

#[cfg(feature = "loader-boot")]
mod loader;
#[cfg(feature = "smp-activate")]
mod smp;
#[cfg(feature = "guest-startup")]
mod startup;
#[cfg(feature = "guest-vmcr")]
mod vmcr;
#[cfg(feature = "guest-cache")]
mod cache;
#[cfg(feature = "guest-apic-contract")]
mod apic_contract;

const TEST_LEAF: u32 = 0x4fff_0000;
const TEST_MAGIC: u32 = 0x5356_4d52;

unsafe extern "efiapi" {
    fn svmvisor_fixture_signal_witness(signal: usize, event: *mut c_void) -> u64;
}

fn marker(text: &str) {
    for byte in text.bytes() {
        // Safety: this binary is gated on the disposable TCG test backend;
        // these debug ports have no physical delivery authorization.
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn hex(value: u64) {
    for shift in (0..16).rev() {
        let nibble = ((value >> (shift * 4)) & 15) as usize;
        let byte = b"0123456789abcdef"[nibble];
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn finish(success: bool) -> ! {
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4u16,
            in("eax") if success { 0x10u32 } else { 0x11u32 }, options(nomem, nostack));
    }
    loop {
        core::hint::spin_loop();
    }
}
fn require(ok: bool, why: &str) {
    if !ok {
        marker("FAIL ");
        marker(why);
        marker("\n");
        finish(false);
    }
}

// Independent before/after observation, including the logical intercepted EFER.
// No SVM enable/disable or table dereference occurs in this witness.
fn observe() -> [u64; 10] {
    let mut gdt = [0u8; 10];
    let mut idt = [0u8; 10];
    let (cr0, cr3, cr4, flags): (u64, u64, u64, u64);
    let (eax, edx): (u32, u32);
    unsafe {
        asm!("sgdt [{}]", in(reg) gdt.as_mut_ptr(), options(nostack, preserves_flags));
        asm!("sidt [{}]", in(reg) idt.as_mut_ptr(), options(nostack, preserves_flags));
        asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
        asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags));
        asm!("rdmsr", in("ecx") 0xc000_0080u32, out("eax") eax, out("edx") edx,
            options(nomem, nostack, preserves_flags));
    }
    [
        cr0,
        cr3,
        cr4,
        flags & 0x600,
        u64::from_le_bytes(gdt[2..].try_into().unwrap()),
        u16::from_le_bytes(gdt[..2].try_into().unwrap()) as u64,
        u64::from_le_bytes(idt[2..].try_into().unwrap()),
        u16::from_le_bytes(idt[..2].try_into().unwrap()) as u64,
        eax as u64 | ((edx as u64) << 32),
        __cpuid(1).edx as u64,
    ]
}

fn resident_witness() -> [u32; 3] {
    let leaf = __cpuid(TEST_LEAF);
    require(leaf.eax == TEST_MAGIC, "resident-test-leaf-missing");
    require(leaf.ebx >= 2 && leaf.ecx > 0, "resident-exit-counts");
    marker("native-fixture-counts exits=");
    hex(leaf.ebx as u64);
    marker(" cpuid=");
    hex(leaf.ecx as u64);
    marker(" msr=");
    hex(leaf.edx as u64);
    marker("\n");
    [leaf.ebx, leaf.ecx, leaf.edx]
}

unsafe extern "efiapi" fn notify(_: Event, _: Option<NonNull<c_void>>) {}

fn allocate_after_ready() {
    let page = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .expect("post-ready allocation");
    let address = page.as_ptr() as u64;
    let map = boot::memory_map(MemoryType::LOADER_DATA).expect("post-ready map");
    let mut runtime_code = false;
    for entry in map.entries() {
        if entry.att.contains(MemoryAttribute::RUNTIME) {
            let end = entry
                .page_count
                .checked_mul(4096)
                .and_then(|size| entry.phys_start.checked_add(size))
                .expect("runtime map extent");
            require(
                address + 4096 <= entry.phys_start || address >= end,
                "allocator-overlaps-runtime-reservation",
            );
            runtime_code |= entry.ty == MemoryType::RUNTIME_SERVICES_CODE;
        }
    }
    require(runtime_code, "runtime-code-descriptor-missing");
    drop(map);
    unsafe {
        page.as_ptr().write_volatile(0x5a);
        require(
            page.as_ptr().read_volatile() == 0x5a,
            "post-ready-allocated-memory",
        );
        boot::free_pages(page, 1).expect("post-ready free");
    }
    marker("native-fixture-post-ready-allocator\n");
}

#[cfg(feature = "virtual-map")]
fn install_identity_map(
    mut final_map: uefi::mem::memory_map::MemoryMapOwned,
    previous_counts: [u32; 3],
) {
    use uefi::mem::memory_map::MemoryMapMut;
    for index in 0..final_map.len() {
        let entry = final_map.get_mut(index).unwrap();
        if entry.att.contains(MemoryAttribute::RUNTIME) {
            entry.virt_start = entry.phys_start;
        }
    }
    let meta = final_map.meta();
    // Preserve the firmware's descriptor stride/version and every runtime
    // range. Identity virtual addresses retain the current page tables and
    // system-table pointer, while exercising firmware VA-change callbacks.
    let status = unsafe {
        let system = uefi::table::system_table_raw().unwrap().as_ref();
        ((*system.runtime_services).set_virtual_address_map)(
            meta.map_size,
            meta.desc_size,
            meta.desc_version,
            final_map.buffer_mut().as_mut_ptr().cast(),
        )
    };
    require(status == Status::SUCCESS, "identity-virtual-address-map");
    uefi::runtime::get_time().expect("runtime GetTime after virtual map");
    marker("native-fixture-after-virtual-map\n");
    let final_counts = resident_witness();
    require(
        final_counts[0] > previous_counts[0],
        "resident-progress-after-virtual-map",
    );
    core::mem::forget(final_map);
}

#[uefi::entry]
fn main() -> Status {
    // This test emits raw debug I/O only after establishing the TCG backend.
    let hv = __cpuid(0x4000_0000);
    let mut vendor = [0u8; 12];
    vendor[..4].copy_from_slice(&hv.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&hv.ecx.to_le_bytes());
    vendor[8..].copy_from_slice(&hv.edx.to_le_bytes());
    if &vendor != b"TCGTCGTCGTCG" {
        return Status::UNSUPPORTED;
    }

    marker("native-fixture-entry\n");
    #[cfg(feature = "loader-boot")]
    loader::prepare();
    let driver = include_bytes!(concat!(env!("OUT_DIR"), "/driver.efi"));
    let image = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromBuffer {
            buffer: driver,
            file_path: None,
        },
    )
    .expect("load resident driver");
    #[cfg(any(
        feature = "expect-rejection",
        feature = "smp-prepare",
        feature = "smp-activate"
    ))]
    let before_start = observe();
    if let Err(error) = boot::start_image(image) {
        #[cfg(feature = "expect-rejection")]
        {
            require(
                error.status() == Status::UNSUPPORTED,
                "unexpected-rejection-status",
            );
            require(
                observe() == before_start,
                "rejection-changed-native-cpu-state",
            );
            require(
                __cpuid(TEST_LEAF).eax != TEST_MAGIC,
                "rejection-left-resident-active",
            );
            marker("native-fixture-refusal-cpu-witness\nPASS native-resident-rejection\n");
            finish(true);
        }
        #[cfg(not(feature = "expect-rejection"))]
        {
            marker("FAIL native-resident-driver-start status=");
            hex(error.status().0 as u64);
            marker("\n");
            finish(false);
        }
    }
    #[cfg(feature = "expect-rejection")]
    require(false, "unsupported-profile-was-accepted");
    #[cfg(feature = "smp-activate")]
    smp::activate(before_start);
    #[cfg(feature = "smp-prepare")]
    {
        require(
            observe() == before_start,
            "percpu-preparation-changed-native-cpu-state",
        );
        require(
            __cpuid(TEST_LEAF).eax != TEST_MAGIC,
            "preparation-unexpectedly-activated",
        );
        marker("native-smp-preparation-native-witness\n");
        allocate_after_ready();
        let final_map = unsafe { boot::exit_boot_services(None) };
        marker("native-smp-preparation-after-ebs\n");
        for entry in final_map.entries() {
            if entry.ty == MemoryType::RUNTIME_SERVICES_CODE
                && entry.att.contains(MemoryAttribute::RUNTIME)
            {
                marker("native-retained-range ");
                hex(entry.phys_start);
                marker(" ");
                hex(entry
                    .page_count
                    .checked_mul(4096)
                    .expect("retained range extent"));
                marker("\n");
            }
        }
        uefi::runtime::get_time().expect("GetTime after prepared-pool EBS");
        core::mem::forget(final_map);
        marker("PASS native-smp-preparation-ebs\n");
        finish(true);
    }
    #[allow(unreachable_code)]
    marker("native-fixture-driver-registered\n");

    let mut ready_group = uefi::guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b");
    let event = unsafe {
        boot::create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(notify),
            None,
            Some(NonNull::from(&mut ready_group)),
        )
    }
    .expect("create real ReadyToBoot event");
    let signal = unsafe {
        let system = uefi::table::system_table_raw().unwrap().as_ref();
        (*system.boot_services).signal_event as usize
    };
    let before = observe();
    marker("native-fixture-before-ready\n");
    let witness = unsafe { svmvisor_fixture_signal_witness(signal, event.as_ptr()) };
    require(witness == 0, "ready-signal-status-or-callee-saved-witness");
    let after = observe();
    marker("native-fixture-after-ready\n");
    for (index, (old, new)) in before.iter().zip(after.iter()).enumerate() {
        if old != new {
            marker("native-fixture-state-mismatch index=");
            hex(index as u64);
            marker(" before=");
            hex(*old);
            marker(" after=");
            hex(*new);
            marker("\n");
            finish(false);
        }
    }
    marker("native-fixture-cpu-and-abi-witness-pass\n");
    let first_counts = resident_witness();
    require(first_counts[2] > 0, "logical-efer-exit-not-observed");
    boot::close_event(event).expect("close signal event before EBS");
    allocate_after_ready();

    // No scoped protocol/event or boot allocation owner crosses EBS except
    // the deliberately retained final map returned by the library.
    let final_map = unsafe { boot::exit_boot_services(None) };
    marker("native-fixture-after-ebs\n");
    let second_counts = resident_witness();
    require(
        second_counts[0] > first_counts[0] && second_counts[1] > first_counts[1],
        "resident-progress-after-ebs",
    );

    #[cfg(feature = "runtime-time")]
    {
        uefi::runtime::get_time().expect("runtime GetTime after EBS");
        marker("native-fixture-runtime-time\n");
    }
    #[cfg(feature = "virtual-map")]
    install_identity_map(final_map, second_counts);
    #[cfg(not(feature = "virtual-map"))]
    core::mem::forget(final_map);
    marker("PASS native-resident-callback-ebs\n");
    finish(true)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    marker("FAIL native-resident-fixture-panic\n");
    finish(false)
}
