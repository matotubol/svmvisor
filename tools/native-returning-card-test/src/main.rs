//! Disposable exact-PE card delivery test. The parent has no fixture features.
//! The real EFI protocol database supplies controller ownership and image load.
//! BAR0 is a RAM-backed external journal model, not physical UC/MMIO evidence.
#![no_std]
#![no_main]
mod boundary_call;
#[cfg(feature="admission-negative")]
mod admission;
mod current_attributes;
mod journal_cache;
#[cfg(feature = "resident")]
mod resident;
use core::{
    arch::{asm, x86_64::__cpuid},
    ffi::c_void,
    ptr,
};
use uefi::{
    Status,
    boot::{self, AllocateType, MemoryType},
};
use uefi_raw::{
    Handle, guid,
    protocol::{
        device_path::DevicePathProtocol, driver::DriverBindingProtocol,
        loaded_image::LoadedImageProtocol,
    },
    table::boot::{BootServices, EventType, InterfaceType, Tpl},
};

const PCI_GUID: uefi_raw::Guid = guid!("4cf5b200-68b8-4ca5-9eec-b23e3f50029a");
// Hardware vendor node followed by End Entire, installed in the actual handle
// database. Firmware LocateDevicePath identifies this exact controller owner.
static PATH: [u8; 24] = [
    1, 4, 20, 0, 0xe6, 0x11, 0x6c, 0x23, 0x7f, 0x9c, 0x29, 0x4e, 0x96, 0xb9, 0x57, 0x71, 0x2e,
    0x9e, 0x80, 0xe0, 0x7f, 0xff, 4, 0,
];
static SLOT: &[u8; 0x100000] = include_bytes!(concat!(env!("OUT_DIR"), "/slot.bin"));
static mut JOURNAL: *mut u8 = ptr::null_mut();
static mut CONTROLLER: Handle = ptr::null_mut();
static mut BINDING: *const DriverBindingProtocol = ptr::null();
static mut COMMAND: u32 = 0;
static mut BAR1_READS: usize = 0;
static mut REENTERED: bool = false;
static mut ENABLES: usize = 0;
static mut DISABLES: usize = 0;

type Access = unsafe extern "efiapi" fn(*const PciIo, u32, u8, u64, usize, *mut c_void) -> Status;
type Config = unsafe extern "efiapi" fn(*const PciIo, u32, u32, usize, *mut c_void) -> Status;
#[repr(C)]
struct PciIo {
    poll_mem: usize,
    poll_io: usize,
    mem_read: Access,
    mem_write: Access,
    io_read: usize,
    io_write: usize,
    pci_read: Config,
    pci_write: usize,
    copy_mem: usize,
    map: usize,
    unmap: usize,
    allocate_buffer: usize,
    free_buffer: usize,
    flush: usize,
    get_location: usize,
    attributes: unsafe extern "efiapi" fn(*const PciIo, u32, u64, *mut u64) -> Status,
    get_bar_attributes:
        unsafe extern "efiapi" fn(*const PciIo, u8, *mut u64, *mut *mut u8) -> Status,
    set_bar_attributes: usize,
    rom_size: u64,
    rom_image: *const c_void,
}
// Unused protocol entry points are never used by the parent; their null values
// make this a deliberately bounded protocol mock, not a general PCI provider.
static mut PCI: PciIo = PciIo {
    poll_mem: 0,
    poll_io: 0,
    mem_read,
    mem_write,
    io_read: 0,
    io_write: 0,
    pci_read,
    pci_write: 0,
    copy_mem: 0,
    map: 0,
    unmap: 0,
    allocate_buffer: 0,
    free_buffer: 0,
    flush: 0,
    get_location: 0,
    attributes,
    get_bar_attributes,
    set_bar_attributes: 0,
    rom_size: 0,
    rom_image: ptr::null(),
};
const _: () = {
    assert!(core::mem::offset_of!(PciIo, attributes) == 120);
    assert!(core::mem::offset_of!(PciIo, get_bar_attributes) == 128);
    assert!(size_of::<PciIo>() == 160);
};

fn services() -> &'static BootServices {
    unsafe {
        &*uefi::table::system_table_raw()
            .unwrap()
            .as_ref()
            .boot_services
    }
}
fn marker(s: &str) {
    for b in s.bytes() {
        unsafe {
            asm!("out dx, al",in("dx")0xe9u16,in("al")b,options(nomem,nostack));
        }
    }
}
fn hex(v: u64) {
    for n in (0..16).rev() {
        let b = b"0123456789abcdef"[((v >> (n * 4)) & 15) as usize];
        unsafe {
            asm!("out dx, al",in("dx")0xe9u16,in("al")b,options(nomem,nostack));
        }
    }
}
fn field(name: &str, v: u64) {
    marker("CARD ");
    marker(name);
    marker("=");
    hex(v);
    marker("\n");
}
fn finish(v: u32) -> ! {
    unsafe {
        asm!("out dx, eax",in("dx")0xf4u16,in("eax")v,options(nomem,nostack));
    }
    loop {
        core::hint::spin_loop();
    }
}
fn check(status: Status, label: &str) {
    if status != Status::SUCCESS {
        marker("FAIL ");
        marker(label);
        marker(" status=");
        hex(unsafe { core::mem::transmute::<Status, usize>(status) } as u64);
        marker("\n");
        finish(0x11);
    }
}
fn tcg() -> bool {
    let h = __cpuid(0x40000000);
    let mut v = [0; 12];
    v[..4].copy_from_slice(&h.ebx.to_le_bytes());
    v[4..8].copy_from_slice(&h.ecx.to_le_bytes());
    v[8..].copy_from_slice(&h.edx.to_le_bytes());
    &v == b"TCGTCGTCGTCG"
}
unsafe fn read_j(offset: usize) -> u32 {
    unsafe { JOURNAL.add(offset).cast::<u32>().read_volatile() }
}
unsafe fn write_j(offset: usize, value: u32) {
    unsafe { JOURNAL.add(offset).cast::<u32>().write_volatile(value) }
}

unsafe extern "efiapi" fn mem_read(
    this: *const PciIo,
    width: u32,
    bar: u8,
    offset: u64,
    count: usize,
    out: *mut c_void,
) -> Status {
    if this != ptr::addr_of!(PCI) || width != 2 || count != 1 || out.is_null() || offset & 3 != 0 {
        return Status::INVALID_PARAMETER;
    }
    if unsafe { COMMAND } & 2 == 0 {
        return Status::NOT_READY;
    }
    let value = if bar == 0 && offset <= 0x9c {
        unsafe { read_j(offset as usize) }
    } else if bar == 1 && offset <= 0xffffc {
        unsafe {
            BAR1_READS += 1;
        }
        if !unsafe { REENTERED } {
            unsafe {
                REENTERED = true;
            }
            let b = unsafe { BINDING };
            let c = unsafe { CONTROLLER };
            assert!(!b.is_null());
            assert_eq!(
                unsafe { ((*b).supported)(b, c, ptr::null()) },
                Status::NOT_READY
            );
            assert_eq!(
                unsafe { ((*b).start)(b, c, ptr::null()) },
                Status::NOT_READY
            );
            assert_eq!(
                unsafe { ((*b).stop)(b, c, 0, ptr::null()) },
                Status::NOT_READY
            );
            marker("PASS reentrant-supported-start-stop-refused\n");
        }
        let o = offset as usize;
        let mut v = u32::from_le_bytes(SLOT[o..o + 4].try_into().unwrap());
        if cfg!(feature = "header-negative") && o == 16 {
            v ^= 1;
        }
        if cfg!(feature = "digest-negative") && o == 640 {
            v ^= 1;
        }
        v
    } else {
        return Status::UNSUPPORTED;
    };
    unsafe {
        out.cast::<u32>().write(value);
    }
    Status::SUCCESS
}
unsafe extern "efiapi" fn mem_write(
    this: *const PciIo,
    width: u32,
    bar: u8,
    offset: u64,
    count: usize,
    input: *mut c_void,
) -> Status {
    if this != ptr::addr_of!(PCI)
        || width != 2
        || bar != 0
        || count != 1
        || input.is_null()
        || offset & 3 != 0
        || !(0x40..=0x60).contains(&offset)
    {
        return Status::INVALID_PARAMETER;
    }
    if unsafe { COMMAND } & 2 == 0 {
        return Status::NOT_READY;
    }
    // Only the external GDB model acknowledges commit writes, for both this
    // protocol path and the parent's later direct lifecycle stores.
    unsafe {
        write_j(offset as usize, input.cast::<u32>().read());
    }
    Status::SUCCESS
}
unsafe extern "efiapi" fn pci_read(
    this: *const PciIo,
    width: u32,
    offset: u32,
    count: usize,
    out: *mut c_void,
) -> Status {
    if this != ptr::addr_of!(PCI) || width != 2 || count != 1 || out.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let value = match offset {
        0 => 0x066610ee,
        4 => unsafe { COMMAND },
        8 => 0xff000003,
        _ => return Status::UNSUPPORTED,
    };
    unsafe {
        out.cast::<u32>().write(value);
    }
    Status::SUCCESS
}
unsafe extern "efiapi" fn attributes(
    this: *const PciIo,
    op: u32,
    value: u64,
    out: *mut u64,
) -> Status {
    if this != ptr::addr_of!(PCI) {
        return Status::INVALID_PARAMETER;
    }
    match op {
        4 if !out.is_null() => unsafe { out.write(0x200) },
        2 if value == 0x200 => unsafe {
            COMMAND |= 2;
            ENABLES += 1;
        },
        3 if value == 0x200 => unsafe {
            COMMAND &= !2;
            DISABLES += 1;
        },
        _ => return Status::UNSUPPORTED,
    }
    Status::SUCCESS
}
unsafe extern "efiapi" fn get_bar_attributes(
    this: *const PciIo,
    bar: u8,
    supports: *mut u64,
    out: *mut *mut u8,
) -> Status {
    if this != ptr::addr_of!(PCI) || bar != 0 || out.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let mut p = ptr::null_mut();
    let s = unsafe {
        (services().allocate_pool)(
            uefi_raw::table::boot::MemoryType::BOOT_SERVICES_DATA,
            48,
            &mut p,
        )
    };
    if s != Status::SUCCESS {
        return s;
    }
    unsafe {
        ptr::write_bytes(p, 0, 48);
        p.write(0x8a);
        p.add(1).write(43);
        p.add(6).cast::<u64>().write_unaligned(32);
        p.add(14).cast::<u64>().write_unaligned(JOURNAL as u64);
        p.add(22)
            .cast::<u64>()
            .write_unaligned(JOURNAL as u64 + 4095);
        p.add(38).cast::<u64>().write_unaligned(4096);
        p.add(46).write(0x79);
        out.write(p);
        if !supports.is_null() {
            supports.write(0);
        }
    }
    Status::SUCCESS
}
fn open(handle: Handle, guid: &uefi_raw::Guid) -> *mut c_void {
    let mut p = ptr::null_mut();
    check(
        unsafe {
            (services().open_protocol)(
                handle,
                guid,
                &mut p,
                boot::image_handle().as_ptr(),
                ptr::null_mut(),
                2,
            )
        },
        "open protocol",
    );
    assert!(!p.is_null());
    p
}
fn close(handle: Handle, guid: &uefi_raw::Guid) {
    check(
        unsafe {
            (services().close_protocol)(
                handle,
                guid,
                boot::image_handle().as_ptr(),
                ptr::null_mut(),
            )
        },
        "close protocol",
    );
}
unsafe extern "efiapi" fn noop_event(_: uefi_raw::Event, _: *mut c_void) {}
fn signal(mut group: uefi_raw::Guid) {
    let mut e = ptr::null_mut();
    check(
        unsafe {
            (services().create_event_ex)(
                EventType::NOTIFY_SIGNAL,
                Tpl::CALLBACK,
                Some(noop_event),
                ptr::null_mut(),
                &mut group,
                &mut e,
            )
        },
        "create group signal",
    );
    check(unsafe { (services().signal_event)(e) }, "signal group");
    check(unsafe { (services().close_event)(e) }, "close group signal");
}
fn expected_diagnostics() -> (u32, u32, u32, u32) {
    if cfg!(pristine_refused) {
        (0, 0, 0x40, 0x40070000)
    } else if cfg!(structured_refused) {
        (5, 0, 0x3c1, 0x40070000)
    } else if cfg!(feature = "header-negative") {
        (0, 0, 0, 0x80070000)
    } else if cfg!(feature = "digest-negative") {
        (0, 0, 0x10, 0x80070000)
    } else {
        (0, 0x10001, 0x1fc2, 0x20070000)
    }
}
fn assert_diagnostics(phase: u32, lifecycle: u32) {
    let (refusal, counts, meta, detail) = expected_diagnostics();
    assert_eq!(unsafe { read_j(0x90) }, refusal, "persistent refusal code");
    assert_eq!(
        unsafe { read_j(0x94) },
        counts,
        "persistent entry/exit counts"
    );
    assert_eq!(
        unsafe { read_j(0x98) },
        meta | lifecycle,
        "persistent exact metadata"
    );
    assert_eq!(
        unsafe { read_j(0x9c) },
        detail | phase,
        "explicit detail-7 phase/result"
    );
}
/// Separate terminal scenario: no Stop or post-return Boot Services claim.
/// Retain all fixture resources and the observer page until disposable VM exit.
#[cfg(terminal_ebs)]
fn terminal_exit_boot_services() -> ! {
    let services = services();
    let image = boot::image_handle().as_ptr();
    // Preallocated aligned storage: neither attempt allocates or frees memory.
    let mut map = [0u64; 8192];
    marker("CARD terminal-ebs-begin\n");
    for attempt in 1..=2 {
        let mut size = core::mem::size_of_val(&map);
        let mut key = 0;
        let mut descriptor_size = 0;
        let mut descriptor_version = 0;
        check(
            unsafe {
                (services.get_memory_map)(
                    &mut size,
                    map.as_mut_ptr().cast(),
                    &mut key,
                    &mut descriptor_size,
                    &mut descriptor_version,
                )
            },
            "terminal GetMemoryMap",
        );
        assert!(size <= core::mem::size_of_val(&map) && descriptor_size != 0);
        let status = unsafe { (services.exit_boot_services)(image, key) };
        if status == Status::SUCCESS {
            // Raw I/O and owned memory only after genuine firmware success.
            field("terminal-ebs-attempts", attempt);
            assert_diagnostics(0x40, 0x01084000);
            marker("PASS genuine-exit-boot-services-final-journal\n");
            finish(0x10);
        }
        if status != Status::INVALID_PARAMETER || attempt == 2 {
            check(status, "terminal ExitBootServices");
        }
    }
    finish(0x11)
}
fn observe() -> [u64; 11] {
    let mut g = [0u8; 10];
    let mut i = [0u8; 10];
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    let flags: u64;
    let cs: u16;
    let ss: u16;
    let ds: u16;
    let es: u16;
    unsafe {
        asm!("sgdt [{}]",in(reg)g.as_mut_ptr(),options(nostack,preserves_flags));
        asm!("sidt [{}]",in(reg)i.as_mut_ptr(),options(nostack,preserves_flags));
        asm!("mov {}, cr0",out(reg)cr0,options(nostack,preserves_flags));
        asm!("mov {}, cr3",out(reg)cr3,options(nostack,preserves_flags));
        asm!("mov {}, cr4",out(reg)cr4,options(nostack,preserves_flags));
        asm!("pushfq; pop {}",out(reg)flags,options(preserves_flags));
        asm!("mov {:x}, cs",out(reg)cs,options(nostack,preserves_flags));
        asm!("mov {:x}, ss",out(reg)ss,options(nostack,preserves_flags));
        asm!("mov {:x}, ds",out(reg)ds,options(nostack,preserves_flags));
        asm!("mov {:x}, es",out(reg)es,options(nostack,preserves_flags));
    }
    [
        cr0,
        cr3,
        cr4,
        flags & 0x600,
        u64::from_le_bytes(g[2..].try_into().unwrap()),
        u64::from_le_bytes(i[2..].try_into().unwrap()),
        u16::from_le_bytes(g[..2].try_into().unwrap()) as u64
            | ((u16::from_le_bytes(i[..2].try_into().unwrap()) as u64) << 16),
        cs as u64,
        ss as u64,
        ds as u64,
        es as u64,
    ]
}
#[uefi::entry]
fn main() -> Status {
    if !tcg() {
        return Status::UNSUPPORTED;
    }
    let attribute = unsafe { current_attributes::install() }.expect("fixture attributes");
    let page = boot::allocate_pages(AllocateType::AnyPages,
        if cfg!(feature = "resident") { MemoryType::RUNTIME_SERVICES_DATA } else { MemoryType::LOADER_DATA }, 1).unwrap();
    journal_cache::qualify(page.as_ptr() as u64);
    unsafe {
        JOURNAL = page.as_ptr();
        ptr::write_bytes(JOURNAL, 0, 4096);
        write_j(0, 0x4a4d5653);
        write_j(4, 0x00010001);
    }
    // Host establishes one exact write watchpoint, then releases this gate.
    // No guest DR register or production input is changed by the harness.
    field("journal-base", page.as_ptr() as u64);
    while unsafe { read_j(0x100) } != 1 {
        core::hint::spin_loop();
    }
    marker("PASS external-journal-model-attached\n");
    let mut controller = ptr::null_mut();
    check(
        unsafe {
            (services().install_protocol_interface)(
                &mut controller,
                &PCI_GUID,
                InterfaceType::NATIVE_INTERFACE,
                ptr::addr_of!(PCI).cast(),
            )
        },
        "install PCI fixture",
    );
    check(
        unsafe {
            (services().install_protocol_interface)(
                &mut controller,
                &DevicePathProtocol::GUID,
                InterfaceType::NATIVE_INTERFACE,
                PATH.as_ptr().cast(),
            )
        },
        "install controller path",
    );
    unsafe {
        CONTROLLER = controller;
    }
    let pe = include_bytes!(concat!(env!("OUT_DIR"), "/driver.efi"));
    let mut parent = ptr::null_mut();
    check(
        unsafe {
            (services().load_image)(
                false.into(),
                boot::image_handle().as_ptr(),
                PATH.as_ptr().cast(),
                pe.as_ptr(),
                pe.len(),
                &mut parent,
            )
        },
        "firmware LoadImage parent",
    );
    let loaded = open(parent, &LoadedImageProtocol::GUID).cast::<LoadedImageProtocol>();
    assert_eq!(
        unsafe { (*loaded).device_handle },
        controller,
        "firmware-derived parent owner"
    );
    assert_eq!(
        unsafe { (*loaded).parent_handle },
        boot::image_handle().as_ptr()
    );
    close(parent, &LoadedImageProtocol::GUID);
    check(
        unsafe { (services().start_image)(parent, ptr::null_mut(), ptr::null_mut()) },
        "firmware StartImage parent",
    );
    marker("PASS real-parent-load-start-owner\n");
    let binding = open(parent, &DriverBindingProtocol::GUID).cast::<DriverBindingProtocol>();
    unsafe {
        BINDING = binding;
    }
    close(parent, &DriverBindingProtocol::GUID);
    assert_eq!(unsafe { (*binding).image_handle }, parent);
    assert_eq!(unsafe { (*binding).driver_binding_handle }, parent);
    check(
        unsafe { ((*binding).supported)(binding, controller, ptr::null()) },
        "initial Supported",
    );
    assert_eq!(unsafe { COMMAND }, 0);
    assert_eq!(unsafe { BAR1_READS }, 0);
    let image_count =
        boot::locate_handle_buffer(boot::SearchType::ByProtocol(&LoadedImageProtocol::GUID))
            .unwrap()
            .len();
    #[cfg(feature="admission-negative")]
    unsafe{admission::install();}
    let before = observe();
    let started = boundary_call::run(binding, controller);
    #[cfg(feature="admission-negative")]
    unsafe{admission::restore();}
    let after = observe();
    assert_eq!(
        before, after,
        "independent host state across real parent Start"
    );
    marker("PASS observed-host-state-unchanged\n");
    assert_eq!(
        boot::locate_handle_buffer(boot::SearchType::ByProtocol(&LoadedImageProtocol::GUID))
            .unwrap()
            .len(),
        image_count + usize::from(cfg!(feature = "resident")
            && !cfg!(feature = "header-negative") && !cfg!(feature = "digest-negative") && !cfg!(feature="admission-negative")),
        "actual firmware loaded-image lifetime"
    );
    if cfg!(feature = "resident") && !cfg!(feature = "header-negative") && !cfg!(feature = "digest-negative") && !cfg!(feature="admission-negative") {
        marker("PASS real-loaded-image-retained\n");
    } else {
        marker("PASS real-loaded-image-count-restored\n");
    }
    let negative = cfg!(feature = "header-negative") || cfg!(feature = "digest-negative") || cfg!(feature="admission-negative");
    if negative {
        assert_eq!(started, if cfg!(feature="admission-negative"){Status::UNSUPPORTED}else{Status::COMPROMISED_DATA});
    } else {
        check(started, "actual parent DriverBinding.Start");
    }
    assert!(unsafe { REENTERED });
    let reads = unsafe { BAR1_READS };
    field("bar1-dword-reads", reads as u64);
    let payload_bytes = u64::from_le_bytes(SLOT[16..24].try_into().unwrap()) as usize;
    assert_eq!(
        reads,
        if cfg!(feature = "header-negative") {
            32
        } else {
            32 + (payload_bytes + 3) / 4
        }
    );
    #[cfg(feature = "resident")]
    if !negative { resident::run(parent, binding, controller); }
    if cfg!(feature="admission-negative") {
        assert_eq!(unsafe{read_j(0x9c)},0x00080014);
        assert_eq!(unsafe{read_j(0x90)},0x00022013);
        assert_eq!(unsafe{read_j(0x94)},0x80000003);
        assert_eq!(unsafe{read_j(0x98)},3);
        marker("PASS resident-admission-firmware-refusal-retained\n");
    } else if cfg!(feature = "resident") {
        assert_eq!(unsafe { read_j(0x9c) }, 0x00080010);
        assert_eq!(unsafe { read_j(0x90) }, (if cfg!(feature = "header-negative") { 0 } else { 1 }) | (1 << 10));
        assert_eq!(unsafe { read_j(0x94) }, 33);
        assert_eq!(unsafe { read_j(0x98) }, 0x80000000);
    } else if cfg!(diagnostics_v7) {
        assert_diagnostics(0x10, 0);
    } else {
        let expected_detail = if negative { 0x80060010 } else { 0x20060010 };
        assert_eq!(
            unsafe { read_j(0x9c) },
            expected_detail,
            "actual parent result journal"
        );
        assert_eq!(
            unsafe { read_j(0x98) },
            if cfg!(feature = "header-negative") {
                0
            } else if cfg!(feature = "digest-negative") {
                1
            } else {
                4
            }
        );
        if !negative {
            assert_eq!(unsafe { read_j(0x90) }, 2);
            assert_eq!(unsafe { read_j(0x94) }, 0x10001);
        }
    }
    marker("PASS exact-parent-result-journal\n");
    assert_eq!(
        unsafe { ((*binding).supported)(binding, controller, ptr::null()) },
        Status::UNSUPPORTED
    );
    assert_eq!(
        unsafe { ((*binding).start)(binding, controller, ptr::null()) },
        if negative {
            Status::UNSUPPORTED
        } else {
            Status::ALREADY_STARTED
        }
    );
    if !negative {
        signal(guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b"));
        if cfg!(diagnostics_v7) {
            assert_diagnostics(0x28, 0x4000);
        } else {
            assert_eq!(unsafe { read_j(0x9c) }, 0x20060028);
            assert_eq!(unsafe { read_j(0x90) }, 1);
        }
        signal(guid!("3a2a00ad-98b9-4cdf-a478-702777f1c10b"));
        if cfg!(diagnostics_v7) {
            assert_diagnostics(0x35, 0x84000);
        } else {
            assert_eq!(unsafe { read_j(0x9c) }, 0x20060035);
            assert_eq!(unsafe { read_j(0x90) }, 0x10001);
        }
        marker("PASS ready-after-lifecycle-journal\n");
        #[cfg(terminal_ebs)]
        terminal_exit_boot_services();
        check(
            unsafe { ((*binding).stop)(binding, controller, 0, ptr::null()) },
            "Stop",
        );
    }
    assert_eq!(unsafe { COMMAND }, 0);
    assert_eq!(unsafe { ENABLES }, 1);
    assert_eq!(unsafe { DISABLES }, 1);
    assert_eq!(
        unsafe { ((*binding).stop)(binding, controller, 0, ptr::null()) },
        Status::NOT_STARTED
    );
    let seq = unsafe { read_j(0x2c) };
    signal(guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b"));
    assert_eq!(unsafe { read_j(0x2c) }, seq);
    assert_eq!(
        unsafe { ((*binding).supported)(binding, controller, ptr::null()) },
        Status::UNSUPPORTED
    );
    assert_eq!(
        unsafe { ((*binding).start)(binding, controller, ptr::null()) },
        Status::UNSUPPORTED
    );
    assert_eq!(unsafe { BAR1_READS }, reads);
    assert_eq!(unsafe { ENABLES }, 1);
    marker("PASS permanent-one-attempt-latch-after-stop\n");
    marker("PASS stop-cleanup-decode-events\n");
    // Verify BY_DRIVER ownership was actually closed by taking it as a new
    // agent in the genuine protocol database, then closing that probe.
    let mut p = ptr::null_mut();
    check(
        unsafe {
            (services().open_protocol)(
                controller,
                &PCI_GUID,
                &mut p,
                boot::image_handle().as_ptr(),
                controller,
                0x10,
            )
        },
        "reopen released PCI ownership",
    );
    check(
        unsafe {
            (services().close_protocol)(
                controller,
                &PCI_GUID,
                boot::image_handle().as_ptr(),
                controller,
            )
        },
        "close ownership probe",
    );
    marker("PASS real-pci-ownership-released\n");
    check(
        unsafe {
            (services().uninstall_protocol_interface)(
                controller,
                &PCI_GUID,
                ptr::addr_of!(PCI).cast(),
            )
        },
        "remove PCI fixture",
    );
    check(
        unsafe {
            (services().uninstall_protocol_interface)(
                controller,
                &DevicePathProtocol::GUID,
                PATH.as_ptr().cast(),
            )
        },
        "remove path fixture",
    );
    unsafe { current_attributes::uninstall(attribute) }.unwrap();
    marker("PASS fixture-protocols-removed\n");
    // Parent deliberately remains resident after Stop: it has no unload entry.
    // Its event callbacks and PCI ownership are gone before VM destruction.
    let page2 = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1).unwrap();
    unsafe {
        page2.as_ptr().write_volatile(0x5a);
        assert_eq!(page2.as_ptr().read_volatile(), 0x5a);
        boot::free_pages(page2, 1).unwrap();
    }
    marker("PASS post-card-boot-services\n");
    finish(0x10)
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    marker("FAIL card-launcher-panic\n");
    if let Some(l) = info.location() {
        marker(l.file());
        marker(":");
        hex(l.line() as u64);
        marker("\n");
    }
    finish(0x11)
}
