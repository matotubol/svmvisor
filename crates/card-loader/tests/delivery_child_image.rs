//! Exact production PE loader with real owned allocations and a fake firmware ABI.

#![cfg(all(feature = "card-resident", target_os = "windows"))]

use std::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr,
    sync::Mutex,
};

use sha2::{Digest, Sha256};
use svmvisor_card_abi::{
    boot_options::ResidentBootOptions, endpoint::TerminalEndpoint, envelope::SLOT_BYTES,
    native_result::NativeResult,
};
use svmvisor_card_loader::delivery::child_image::{self, Pin, State};
use uefi_raw::{
    Boolean, Char16, Handle, Status,
    protocol::{device_path::DevicePathProtocol, loaded_image::LoadedImageProtocol},
    table::boot::{BootServices, MemoryType},
};

// Fixture modes of the failure paths. 20..=32 are the StartImage outcomes of
// `resident_lifetime_requires_ack_and_retains_all_nonerror_returns`.
const ALLOCATE_FAILS: u8 = 40;
const ALLOCATE_RETURNS_NULL: u8 = 41;
const FREE_POOL_FAILS: u8 = 42;
const FREE_EXIT_DATA_FAILS: u8 = 43;
const PATH_OPEN_FAILS: u8 = 44;
const PATH_IS_NULL: u8 = 45;
const PATH_NODE_TOO_SHORT: u8 = 46;
const PATH_NODE_TOO_LONG: u8 = 47;
const PATH_END_IS_AN_INSTANCE_END: u8 = 48;
const PATH_END_HAS_A_BODY: u8 = 49;
const PATH_NEVER_ENDS: u8 = 50;
const LOAD_SECURITY_VIOLATION: u8 = 51;
const LOAD_SECURITY_VIOLATION_UNLOAD_FAILS: u8 = 52;
const LOAD_ERROR: u8 = 53;
const LOAD_RETURNS_NULL: u8 = 54;
const CHILD_OPEN_FAILS: u8 = 55;
const CHILD_IS_NULL: u8 = 56;
const CHILD_PARENT_DIFFERS: u8 = 57;
const CHILD_BASE_IS_NULL: u8 = 58;
const CHILD_BYTES_DIFFER: u8 = 59;
const CHILD_CODE_IS_BOOT_SERVICES: u8 = 60;
const CHILD_DATA_IS_BOOT_SERVICES: u8 = 61;
const CHILD_OPTIONS_BYTES_SET: u8 = 62;
const CHILD_OPTIONS_SET: u8 = 63;
const START_REFUSES_PARAMETER: u8 = 64;
const START_FAILS_BEFORE_ENTRY: u8 = 65;
const START_ERROR_WITH_EXIT_DATA: u8 = 66;
const START_SUCCESS_WITH_EXIT_DATA: u8 = 67;

#[cfg(feature = "card-resident-dev-loader")]
const ENTRIES: [Entry; 2] = [Entry::Pinned, Entry::Dev];
#[cfg(not(feature = "card-resident-dev-loader"))]
const ENTRIES: [Entry; 1] = [Entry::Pinned];

static FIX: Mutex<Fixture> =
    Mutex::new(Fixture { mode: 0, pool: 0, bytes: 0, exit: 0, loaded: 0, events: Vec::new() });
static TEST_LOCK: Mutex<()> = Mutex::new(());
static PATH: [u8; 10] = [1, 1, 6, 0, 0, 0, 0x7f, 0xff, 4, 0];
static PATH_WITH_SHORT_NODE: [u8; 8] = [1, 1, 3, 0, 0x7f, 0xff, 4, 0];
static PATH_WITH_LONG_NODE: [u8; 8] = [1, 1, 0xff, 0xff, 0x7f, 0xff, 4, 0];
static PATH_OF_TWO_INSTANCES: [u8; 14] = [1, 1, 6, 0, 0, 0, 0x7f, 0x01, 4, 0, 0x7f, 0xff, 4, 0];
static PATH_WITH_END_BODY: [u8; 14] = [1, 1, 6, 0, 0, 0, 0x7f, 0xff, 8, 0, 0, 0, 0, 0];
// 1024 four-byte nodes and no End node inside the 4096 bytes the loader accepts.
static PATH_WITHOUT_END: [u8; 4096] = {
    let mut path = [0; 4096];
    let mut offset = 0;
    while offset < 4096 {
        path[offset] = 1;
        path[offset + 1] = 1;
        path[offset + 2] = 4;
        offset += 4;
    }
    path
};

struct Fixture {
    mode: u8,
    pool: usize,
    bytes: usize,
    exit: usize,
    loaded: usize,
    events: Vec<&'static str>,
}

fn parent() -> Handle {
    0x100usize as Handle
}

fn controller() -> Handle {
    0x200usize as Handle
}

fn child() -> Handle {
    0x300usize as Handle
}

fn w16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn w32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn w64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn fixture() -> Vec<u8> {
    let mut pe = vec![0; 1024];
    pe[..2].copy_from_slice(b"MZ");
    w32(&mut pe, 0x3c, 64);
    pe[64..68].copy_from_slice(b"PE\0\0");
    for (o, v) in [(68, 0x8664), (70, 1), (84, 240), (86, 2), (88, 0x20b), (156, 11)] {
        w16(&mut pe, o, v);
    }
    for (o, v) in [
        (104, 4096),
        (120, 4096),
        (124, 512),
        (144, 8192),
        (148, 512),
        (196, 16),
        (336, 1),
        (340, 4096),
        (344, 512),
        (348, 512),
        (364, 0x60000020),
    ] {
        w32(&mut pe, o, v);
    }
    pe[512] = 0xc3;
    let mut slot = vec![0xff; SLOT_BYTES];
    slot[..128].fill(0);
    slot[..8].copy_from_slice(b"SVMPE001");
    for (o, v) in
        [(8, 1), (12, 128), (88, 4096), (92, 8192), (96, 512), (100, 4096), (104, 512), (108, 1)]
    {
        w32(&mut slot, o, v);
    }
    for (o, v) in [(16, 1024), (24, 0x100000), (32, 128), (40, 2)] {
        w64(&mut slot, o, v);
    }
    for (o, v) in [(80, 0x8664), (82, 11), (84, 0x20b)] {
        w16(&mut slot, o, v);
    }
    slot[48..80].copy_from_slice(&Sha256::digest(&pe));
    slot[128..1152].copy_from_slice(&pe);
    slot
}

unsafe extern "efiapi" fn alloc(ty: MemoryType, n: usize, out: *mut *mut u8) -> Status {
    let mut f = FIX.lock().unwrap();
    assert_eq!(
        ty,
        if f.mode >= 20 { MemoryType::RUNTIME_SERVICES_DATA } else { MemoryType::LOADER_DATA }
    );
    f.events.push("allocate");
    if f.mode == 7 || f.mode == ALLOCATE_FAILS {
        return Status::OUT_OF_RESOURCES;
    }
    if f.mode == ALLOCATE_RETURNS_NULL {
        return Status::SUCCESS;
    }
    assert_eq!(f.pool, 0);
    let p = Box::into_raw(vec![0u8; n].into_boxed_slice()).cast::<u8>();
    f.pool = p as usize;
    f.bytes = n;
    unsafe { *out = p };
    Status::SUCCESS
}

unsafe extern "efiapi" fn free(p: *mut u8) -> Status {
    let mut f = FIX.lock().unwrap();
    if p as usize == f.exit {
        f.events.push("free-exit");
        if f.mode == FREE_EXIT_DATA_FAILS {
            return Status::DEVICE_ERROR;
        }
        unsafe {
            drop(Box::from_raw(p.cast::<[u8; 4]>()));
        }
        f.exit = 0;
        return Status::SUCCESS;
    }
    assert_eq!(p as usize, f.pool);
    f.events.push("free-pool");
    if f.mode == 5 || f.mode == FREE_POOL_FAILS {
        return Status::DEVICE_ERROR;
    }
    assert_eq!(f.loaded, 0, "mailbox must outlive resident child");
    unsafe {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(p, f.bytes)));
    }
    f.pool = 0;
    Status::SUCCESS
}

unsafe extern "efiapi" fn open(
    handle: Handle,
    guid: *const uefi_raw::Guid,
    out: *mut *mut c_void,
    agent: Handle,
    _: Handle,
    attrs: u32,
) -> Status {
    assert_eq!(agent, parent());
    assert_eq!(attrs, 2);
    let mut f = FIX.lock().unwrap();
    if unsafe { *guid } == DevicePathProtocol::GUID {
        assert_eq!(handle, controller());
        f.events.push("open-path");
        if f.mode == PATH_OPEN_FAILS {
            return Status::UNSUPPORTED;
        }
        unsafe {
            *out = device_path(f.mode).cast_mut().cast();
        }
    } else {
        assert_eq!(unsafe { *guid }, LoadedImageProtocol::GUID);
        assert_eq!(handle, child());
        f.events.push("open-child");
        if f.mode == CHILD_OPEN_FAILS {
            return Status::ACCESS_DENIED;
        }
        unsafe {
            *out = if f.mode == 11 || f.mode == CHILD_IS_NULL {
                ptr::null_mut()
            } else {
                f.loaded as *mut c_void
            };
        }
    }
    Status::SUCCESS
}

fn device_path(mode: u8) -> *const u8 {
    match mode {
        PATH_IS_NULL => ptr::null(),
        PATH_NODE_TOO_SHORT => PATH_WITH_SHORT_NODE.as_ptr(),
        PATH_NODE_TOO_LONG => PATH_WITH_LONG_NODE.as_ptr(),
        PATH_END_IS_AN_INSTANCE_END => PATH_OF_TWO_INSTANCES.as_ptr(),
        PATH_END_HAS_A_BODY => PATH_WITH_END_BODY.as_ptr(),
        PATH_NEVER_ENDS => PATH_WITHOUT_END.as_ptr(),
        _ => PATH.as_ptr(),
    }
}

unsafe extern "efiapi" fn close(
    _: Handle,
    guid: *const uefi_raw::Guid,
    _: Handle,
    _: Handle,
) -> Status {
    let mut f = FIX.lock().unwrap();
    if unsafe { *guid } == DevicePathProtocol::GUID {
        f.events.push("close-path");
        if f.mode == 15 {
            return Status::DEVICE_ERROR;
        }
    } else {
        f.events.push("close-child");
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn load(
    _: Boolean,
    agent: Handle,
    path: *const DevicePathProtocol,
    source: *const u8,
    size: usize,
    out: *mut Handle,
) -> Status {
    assert_eq!(agent, parent());
    assert_eq!(size, 1024);
    assert_eq!(unsafe { std::slice::from_raw_parts(source, 2) }, b"MZ");
    let p = unsafe { std::slice::from_raw_parts(path.cast::<u8>(), 62) };
    assert_eq!(&p[..6], &PATH[..6]);
    assert_eq!(&p[6..10], &[4, 3, 52, 0]);
    assert_eq!(&p[58..62], &[0x7f, 0xff, 4, 0]);
    let mut f = FIX.lock().unwrap();
    f.events.push("load");
    if f.mode == 12 || f.mode == LOAD_ERROR {
        return Status::LOAD_ERROR;
    }
    if f.mode == LOAD_RETURNS_NULL {
        return Status::SUCCESS;
    }
    let mut l = Box::new(LoadedImageProtocol {
        revision: 0x1000,
        parent_handle: parent(),
        system_table: ptr::null(),
        device_handle: controller(),
        file_path: path,
        reserved: ptr::null(),
        load_options_size: 0,
        load_options: ptr::null(),
        image_base: 0x100000 as *const c_void,
        image_size: if f.mode == 14 { 4096 } else { 8192 },
        image_code_type: if f.mode >= 20 {
            MemoryType::RUNTIME_SERVICES_CODE
        } else {
            MemoryType::BOOT_SERVICES_CODE
        },
        image_data_type: if f.mode >= 20 {
            MemoryType::RUNTIME_SERVICES_DATA
        } else {
            MemoryType::BOOT_SERVICES_DATA
        },
        unload: None,
    });
    match f.mode {
        CHILD_PARENT_DIFFERS => l.parent_handle = controller(),
        CHILD_BASE_IS_NULL => l.image_base = ptr::null(),
        CHILD_BYTES_DIFFER => l.image_size = 4096,
        CHILD_CODE_IS_BOOT_SERVICES => l.image_code_type = MemoryType::BOOT_SERVICES_CODE,
        CHILD_DATA_IS_BOOT_SERVICES => l.image_data_type = MemoryType::BOOT_SERVICES_DATA,
        CHILD_OPTIONS_BYTES_SET => l.load_options_size = 4,
        CHILD_OPTIONS_SET => l.load_options = PATH.as_ptr().cast(),
        _ => {}
    }
    f.loaded = Box::into_raw(l) as usize;
    unsafe { *out = child() };
    if matches!(f.mode, 2 | 6 | LOAD_SECURITY_VIOLATION | LOAD_SECURITY_VIOLATION_UNLOAD_FAILS) {
        Status::SECURITY_VIOLATION
    } else {
        Status::SUCCESS
    }
}

unsafe extern "efiapi" fn start(handle: Handle, _: *mut usize, exit: *mut *mut Char16) -> Status {
    assert_eq!(handle, child());
    let mut f = FIX.lock().unwrap();
    f.events.push("start");
    if f.mode == 3 {
        return Status::SECURITY_VIOLATION;
    }
    if f.mode == 16 {
        return Status::INVALID_PARAMETER;
    }
    let l = unsafe { &*(f.loaded as *const LoadedImageProtocol) };
    assert_eq!(l.load_options_size, 128);
    if f.mode >= 20 {
        let m = unsafe { &mut *l.load_options.cast_mut().cast::<ResidentBootOptions>() };
        assert_eq!(*m, resident_options(f.mode));
        if f.mode == 25 {
            return Status::SECURITY_VIOLATION;
        }
        if f.mode == START_REFUSES_PARAMETER {
            return Status::INVALID_PARAMETER;
        }
        if f.mode == START_FAILS_BEFORE_ENTRY {
            auto_unload(&mut f);
            return Status::ABORTED;
        }
        m.rust_entered = 1;
        if matches!(f.mode, START_ERROR_WITH_EXIT_DATA | FREE_EXIT_DATA_FAILS) {
            m.failure = 17;
            return_exit_data(&mut f, exit);
            auto_unload(&mut f);
            return Status::ABORTED;
        }
        if f.mode == START_SUCCESS_WITH_EXIT_DATA {
            return_exit_data(&mut f, exit);
        }
        if f.mode == 28 {
            m.failure = Status::OUT_OF_RESOURCES.0 as u64;
            m.preparation_stage = 6;
            m.preparation_reason = 2;
            m.preparation_address = 0x123456789abc;
            unsafe {
                drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
            }
            f.loaded = 0;
            f.events.push("auto-unload");
            return Status::OUT_OF_RESOURCES;
        }
        if matches!(f.mode, 24 | 26 | FREE_POOL_FAILS) {
            m.failure = 17;
            unsafe {
                drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
            }
            f.loaded = 0;
            f.events.push("auto-unload");
            return if f.mode == 26 { Status::INVALID_PARAMETER } else { Status::UNSUPPORTED };
        }
        if f.mode != 21 {
            m.armed = 1;
        }
        if f.mode == 22 {
            m.magic[0] ^= 1;
        }
        if f.mode == 27 {
            m.journal_base += 4096;
        }
        // These mutations remain valid headers, so only exact comparison with
        // the original immutable parent inputs can reject the acknowledgement.
        if f.mode == 29 {
            m.version = 1;
        }
        if f.mode == 31 {
            m.reserved[2] ^= 2;
        }
        if f.mode == 32 {
            m.version = 2;
            m.reserved = [0; 7];
        }
        return if f.mode == 23 { Status::WARN_UNKNOWN_GLYPH } else { Status::SUCCESS };
    }
    let m = unsafe { &mut *l.load_options.cast_mut().cast::<NativeResult>() };
    assert_eq!(*m, NativeResult::new());
    if matches!(f.mode, 1 | 4) {
        m.rust_entered = 1;
        m.rust_completed = 1;
        m.outcome = 2;
        m.attempted_entries = 1;
        m.completed_exits = 1;
        m.restoration_complete = 1;
        m.cleanup_complete = 1;
    }
    if f.mode == 4 {
        return Status::SUCCESS;
    }
    if f.mode == 13 {
        let p = Box::into_raw(Box::new([0u8; 4]));
        f.exit = p as usize;
        unsafe {
            *exit = p.cast();
        }
    }
    // Documented driver error return: firmware has freed the child already.
    unsafe {
        drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
    }
    f.loaded = 0;
    f.events.push("auto-unload");
    Status::UNSUPPORTED
}

/// Documented driver error return: firmware has freed the child already.
fn auto_unload(f: &mut Fixture) {
    unsafe {
        drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
    }
    f.loaded = 0;
    f.events.push("auto-unload");
}

fn return_exit_data(f: &mut Fixture, exit: *mut *mut Char16) {
    let p = Box::into_raw(Box::new([0u8; 4]));
    f.exit = p as usize;
    unsafe {
        *exit = p.cast();
    }
}

unsafe extern "efiapi" fn unload(handle: Handle) -> Status {
    assert_eq!(handle, child());
    let mut f = FIX.lock().unwrap();
    f.events.push("unload");
    assert_ne!(f.loaded, 0, "stale child handle used");
    if f.mode == 6 || f.mode == LOAD_SECURITY_VIOLATION_UNLOAD_FAILS {
        return Status::DEVICE_ERROR;
    }
    unsafe {
        drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
    }
    f.loaded = 0;
    Status::SUCCESS
}

unsafe extern "efiapi" fn forbidden() -> Status {
    panic!("unexpected firmware service")
}

fn services() -> BootServices {
    let mut raw = MaybeUninit::<BootServices>::uninit();
    unsafe {
        let words = raw.as_mut_ptr().cast::<usize>();
        for i in 0..size_of::<BootServices>() / size_of::<usize>() {
            words.add(i).write(forbidden as *const () as usize);
        }
        ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(std::mem::zeroed());
        ptr::addr_of_mut!((*raw.as_mut_ptr()).allocate_pool).write(alloc);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pool).write(free);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).open_protocol).write(open);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).close_protocol).write(close);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).load_image).write(load);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).start_image).write(start);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).unload_image).write(unload);
        raw.assume_init()
    }
}

/// The resident entries this configuration has; every failure path runs through each of them.
#[derive(Clone, Copy, Debug)]
enum Entry {
    Pinned,
    #[cfg(feature = "card-resident-dev-loader")]
    Dev,
}

/// One delivery of the valid resident slot with the fixture freshly put in `mode`.
fn deliver(entry: Entry, mode: u8, state: &mut State, bs: &BootServices) -> child_image::Delivery {
    reset_fixture(mode);
    let slot = resident_slot();
    execute(entry, state, bs, (parent(), controller()), resident_options(mode), |offset| {
        word(&slot, offset)
    })
}

/// The pinned entry holds the header of the valid resident slot, whatever `read` supplies.
fn execute(
    entry: Entry,
    state: &mut State,
    bs: &BootServices,
    (parent, controller): (Handle, Handle),
    options: ResidentBootOptions,
    read: impl FnMut(u64) -> Result<u32, Status>,
) -> child_image::Delivery {
    match entry {
        Entry::Pinned => {
            let pin = Pin::parse_resident(&resident_slot()[..128]).unwrap();
            unsafe {
                child_image::execute_resident(state, bs, parent, controller, &pin, options, read)
            }
        }
        #[cfg(feature = "card-resident-dev-loader")]
        Entry::Dev => unsafe {
            child_image::execute_resident_dev(state, bs, parent, controller, options, read)
        },
    }
}

/// A failed delivery that left the parent nothing to own and no hook it could call armed.
fn assert_released(report: &child_image::Delivery, state: &State, name: &str) {
    assert_ne!(report.status(), Status::SUCCESS, "{name}");
    assert_eq!(report.cleanup_status, Status::SUCCESS, "{name}");
    assert!(state.is_clean() && !state.is_retained(), "{name}");
    assert!(state.resident_options().is_none_or(|options| !options.is_armed()), "{name}");
    let f = FIX.lock().unwrap();
    assert_eq!((f.pool, f.exit, f.loaded), (0, 0, 0), "{name}");
    assert!(!f.events.contains(&"close-path") && !f.events.contains(&"close-child"), "{name}");
}

fn events() -> Vec<&'static str> {
    FIX.lock().unwrap().events.clone()
}

fn resident_options(mode: u8) -> ResidentBootOptions {
    let options = ResidentBootOptions::new(0xd0000000, 42);
    if mode < 30 {
        return options;
    }
    options
        .with_terminal(TerminalEndpoint {
            config_page: 0xe012a000,
            bar0_host_page: 0xd0000000,
            fpga_build_id: 1,
            rom_build_id: 2,
            mmio_config_msr: 0xe0000021,
            bar0_raw: 0xd0000000,
            segment_bdf: 0x12a,
            boot_id: 42,
            command: 2,
            version: 1,
            reserved: 0,
        })
        .unwrap()
}

/// A valid SVMBPE01 resident slot: header + 1024-byte child + 0xff padding.
fn resident_slot() -> Vec<u8> {
    let mut slot = fixture();
    slot[..8].copy_from_slice(b"SVMBPE01");
    w64(&mut slot, 40, 4);
    w16(&mut slot, 82, 12);
    w16(&mut slot, 128 + 156, 12);
    let digest = Sha256::digest(&slot[128..1152]);
    slot[48..80].copy_from_slice(&digest);
    slot
}

fn word(slot: &[u8], offset: u64) -> Result<u32, Status> {
    assert_eq!(offset & 3, 0);
    Ok(u32::from_le_bytes(slot[offset as usize..offset as usize + 4].try_into().unwrap()))
}

/// End the simulated machine lifetime of a retained child. Production has none.
fn end_retained_lifetime() {
    let mut f = FIX.lock().unwrap();
    unsafe {
        drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(f.pool as *mut u8, f.bytes)));
        if f.exit != 0 {
            drop(Box::from_raw(f.exit as *mut [u8; 4]));
        }
    }
    f.loaded = 0;
    f.pool = 0;
    f.exit = 0;
}

fn reset_fixture(mode: u8) {
    *FIX.lock().unwrap() =
        Fixture { mode, pool: 0, bytes: 0, exit: 0, loaded: 0, events: Vec::new() };
}

#[test]
fn resident_lifetime_requires_ack_and_retains_all_nonerror_returns() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    for mode in 20..=32 {
        *FIX.lock().unwrap() =
            Fixture { mode, pool: 0, bytes: 0, exit: 0, loaded: 0, events: Vec::new() };
        let mut slot = fixture();
        slot[..8].copy_from_slice(b"SVMBPE01");
        w64(&mut slot, 40, 4);
        w16(&mut slot, 82, 12);
        w16(&mut slot, 128 + 156, 12);
        let digest = Sha256::digest(&slot[128..1152]);
        slot[48..80].copy_from_slice(&digest);
        let pin = Pin::parse_resident(&slot[..128]).unwrap();
        let mut state = State::new();
        let report = unsafe {
            child_image::execute_resident(
                &mut state,
                &bs,
                parent(),
                controller(),
                &pin,
                resident_options(mode),
                |offset| {
                    Ok(u32::from_le_bytes(
                        slot[offset as usize..offset as usize + 4].try_into().unwrap(),
                    ))
                },
            )
        };
        let expected = match mode {
            20 | 30 => Status::SUCCESS,
            24 => Status::UNSUPPORTED,
            25 => Status::SECURITY_VIOLATION,
            26 => Status::INVALID_PARAMETER,
            28 => Status::OUT_OF_RESOURCES,
            _ => Status::PROTOCOL_ERROR,
        };
        assert_eq!(report.status(), expected, "mode{mode}");
        let retained = !matches!(mode, 24..=26 | 28);
        if mode == 28 {
            let observed = state.resident_options().unwrap();
            assert_eq!(observed.preparation_words(), Some([0x12340206, 0, 0x56789abc]));
            assert_eq!(observed.failure, Status::OUT_OF_RESOURCES.0 as u64);
            assert_eq!(observed.armed, 0);
        }
        assert_eq!(state.is_retained(), retained);
        let before = FIX.lock().unwrap().events.clone();
        assert!(!before.contains(&"close-path") && !before.contains(&"close-child"));
        if retained {
            assert!(!before.contains(&"unload") && !before.contains(&"free-pool"));
            assert_eq!(unsafe { state.cleanup(&bs) }, Err(Status::UNSUPPORTED));
            assert_eq!(FIX.lock().unwrap().events, before);
            // End the simulated machine lifetime. Production has no such cleanup.
            let mut f = FIX.lock().unwrap();
            unsafe {
                drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol));
                drop(Box::from_raw(ptr::slice_from_raw_parts_mut(f.pool as *mut u8, f.bytes)));
            }
            f.loaded = 0;
            f.pool = 0;
        } else {
            assert!(state.is_clean());
        }
    }
}

#[test]
fn invalid_inputs_are_refused_before_any_firmware_service() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let slot = resident_slot();
    let good = resident_options(20);
    let null: Handle = ptr::null_mut();
    let mut magic = good;
    magic.magic[0] ^= 1;
    let cases = [
        ("null parent", (null, controller()), good),
        ("null controller", (parent(), null), good),
        ("options header", (parent(), controller()), magic),
        (
            "options already entered",
            (parent(), controller()),
            ResidentBootOptions { rust_entered: 1, ..good },
        ),
        (
            "options already armed",
            (parent(), controller()),
            ResidentBootOptions { armed: 1, ..good },
        ),
        (
            "options already failed",
            (parent(), controller()),
            ResidentBootOptions { failure: 1, ..good },
        ),
    ];
    for entry in ENTRIES {
        for (name, handles, options) in cases {
            let name = format!("{entry:?}: {name}");
            reset_fixture(20);
            let mut state = State::new();
            let mut highest = 0;
            let report = execute(entry, &mut state, &bs, handles, options, |offset| {
                highest = highest.max(offset);
                word(&slot, offset)
            });
            assert_eq!((report.status(), report.stage), (Status::INVALID_PARAMETER, 0), "{name}");
            assert_eq!((report.load_status, report.start_status), (None, None), "{name}");
            assert!(highest < 128, "{name}");
            assert!(events().is_empty(), "{name}");
            assert!(state.resident_options().is_none(), "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn slot_that_differs_from_the_pin_is_refused_before_load() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let good = resident_slot();
    type Corrupt = fn(&mut Vec<u8>);
    let classes: [(&str, Corrupt, u32, &[&str]); 3] = [
        ("header bit flip", |slot| slot[16] ^= 1, 0, &[]),
        ("digest bit flip", |slot| slot[48] ^= 1, 0, &[]),
        ("payload bit flip", |slot| slot[128 + 512] ^= 1, 1, &["allocate", "free-pool"]),
    ];
    for (name, corrupt, stage, expected) in classes {
        let mut slot = good.clone();
        corrupt(&mut slot);
        reset_fixture(20);
        let mut state = State::new();
        let report = execute(
            Entry::Pinned,
            &mut state,
            &bs,
            (parent(), controller()),
            resident_options(20),
            |offset| word(&slot, offset),
        );
        assert_eq!((report.status(), report.stage), (Status::COMPROMISED_DATA, stage), "{name}");
        assert_eq!(events(), expected, "{name}");
        assert_released(&report, &state, name);
    }
    // The slot equals the pin and the digest binds the child, but the pinned PE metadata does
    // not describe that child.
    let mut slot = good.clone();
    w32(&mut slot, 88, 4096 + 1); // header entry RVA
    let pin = Pin::parse_resident(&slot[..128]).unwrap();
    reset_fixture(20);
    let mut state = State::new();
    let report = unsafe {
        child_image::execute_resident(
            &mut state,
            &bs,
            parent(),
            controller(),
            &pin,
            resident_options(20),
            |offset| word(&slot, offset),
        )
    };
    assert_eq!((report.status(), report.stage), (Status::COMPROMISED_DATA, 1));
    assert_eq!(events(), ["allocate", "free-pool"]);
    assert_released(&report, &state, "metadata");
}

#[test]
fn pool_allocation_failure_reads_no_payload_and_owns_nothing() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let slot = resident_slot();
    for entry in ENTRIES {
        for (mode, expected) in [
            (ALLOCATE_FAILS, Status::OUT_OF_RESOURCES),
            (ALLOCATE_RETURNS_NULL, Status::DEVICE_ERROR),
        ] {
            let name = format!("{entry:?}: mode{mode}");
            reset_fixture(mode);
            let mut state = State::new();
            let mut highest = 0;
            let report = execute(
                entry,
                &mut state,
                &bs,
                (parent(), controller()),
                resident_options(mode),
                |offset| {
                    highest = highest.max(offset);
                    word(&slot, offset)
                },
            );
            assert_eq!((report.status(), report.stage), (expected, 1), "{name}");
            assert_eq!((report.load_status, report.start_status), (None, None), "{name}");
            assert!(highest < 128, "{name}");
            assert_eq!(events(), ["allocate"], "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn transport_errors_are_reported_and_the_pool_is_freed() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let slot = resident_slot();
    let cases: [(&str, u64, u32, &[&str]); 3] = [
        ("header", 64, 0, &[]),
        ("first payload word", 128, 1, &["allocate", "free-pool"]),
        ("middle of the slot copy", 128 + 512, 1, &["allocate", "free-pool"]),
    ];
    for entry in ENTRIES {
        for (name, failing, stage, expected) in cases {
            let name = format!("{entry:?}: {name}");
            reset_fixture(20);
            let mut state = State::new();
            let mut highest = 0;
            let report = execute(
                entry,
                &mut state,
                &bs,
                (parent(), controller()),
                resident_options(20),
                |offset| {
                    highest = highest.max(offset);
                    if offset == failing {
                        return Err(Status::DEVICE_ERROR);
                    }
                    word(&slot, offset)
                },
            );
            assert_eq!((report.status(), report.stage), (Status::DEVICE_ERROR, stage), "{name}");
            assert_eq!((report.load_status, report.start_status), (None, None), "{name}");
            assert_eq!(highest, failing, "{name}");
            assert_eq!(events(), expected, "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn unusable_device_path_stops_the_delivery_before_load() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    for entry in ENTRIES {
        for (mode, expected) in [
            (PATH_OPEN_FAILS, Status::UNSUPPORTED),
            (PATH_IS_NULL, Status::DEVICE_ERROR),
            (PATH_NODE_TOO_SHORT, Status::COMPROMISED_DATA),
            (PATH_NODE_TOO_LONG, Status::COMPROMISED_DATA),
            (PATH_END_IS_AN_INSTANCE_END, Status::COMPROMISED_DATA),
            (PATH_END_HAS_A_BODY, Status::COMPROMISED_DATA),
            (PATH_NEVER_ENDS, Status::COMPROMISED_DATA),
        ] {
            let name = format!("{entry:?}: mode{mode}");
            let mut state = State::new();
            let report = deliver(entry, mode, &mut state, &bs);
            assert_eq!((report.status(), report.stage), (expected, 2), "{name}");
            assert_eq!((report.load_status, report.start_status), (None, None), "{name}");
            assert_eq!(events(), ["allocate", "open-path", "free-pool"], "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn load_image_failure_unloads_a_returned_handle_and_never_starts() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let cases: [(u8, Status, Status, &[&str]); 3] = [
        (
            LOAD_SECURITY_VIOLATION,
            Status::SECURITY_VIOLATION,
            Status::SECURITY_VIOLATION,
            &["allocate", "open-path", "load", "unload", "free-pool"],
        ),
        (
            LOAD_ERROR,
            Status::LOAD_ERROR,
            Status::LOAD_ERROR,
            &["allocate", "open-path", "load", "free-pool"],
        ),
        (
            LOAD_RETURNS_NULL,
            Status::SUCCESS,
            Status::DEVICE_ERROR,
            &["allocate", "open-path", "load", "free-pool"],
        ),
    ];
    for entry in ENTRIES {
        for (mode, loaded, expected, expected_events) in cases {
            let name = format!("{entry:?}: mode{mode}");
            let mut state = State::new();
            let report = deliver(entry, mode, &mut state, &bs);
            assert_eq!((report.status(), report.stage), (expected, 3), "{name}");
            assert_eq!((report.load_status, report.start_status), (Some(loaded), None), "{name}");
            assert_eq!(events(), expected_events, "{name}");
            assert!(state.resident_options().is_none(), "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn child_that_firmware_describes_differently_is_unloaded_unstarted() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    for entry in ENTRIES {
        for (mode, expected) in [
            (CHILD_OPEN_FAILS, Status::ACCESS_DENIED),
            (CHILD_IS_NULL, Status::DEVICE_ERROR),
            (CHILD_PARENT_DIFFERS, Status::COMPROMISED_DATA),
            (CHILD_BASE_IS_NULL, Status::COMPROMISED_DATA),
            (CHILD_BYTES_DIFFER, Status::COMPROMISED_DATA),
            (CHILD_CODE_IS_BOOT_SERVICES, Status::COMPROMISED_DATA),
            (CHILD_DATA_IS_BOOT_SERVICES, Status::COMPROMISED_DATA),
            (CHILD_OPTIONS_BYTES_SET, Status::COMPROMISED_DATA),
            (CHILD_OPTIONS_SET, Status::COMPROMISED_DATA),
        ] {
            let name = format!("{entry:?}: mode{mode}");
            let mut state = State::new();
            let report = deliver(entry, mode, &mut state, &bs);
            assert_eq!((report.status(), report.stage), (expected, 3), "{name}");
            assert_eq!(
                (report.load_status, report.start_status),
                (Some(Status::SUCCESS), None),
                "{name}"
            );
            assert_eq!(
                events(),
                ["allocate", "open-path", "load", "open-child", "unload", "free-pool"],
                "{name}"
            );
            assert!(state.resident_options().is_none(), "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn start_image_error_releases_the_child_its_exit_data_and_the_pool() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    // Only the two documented pre-entry refusals leave the image loaded for the parent to unload.
    let cases: [(u8, Status, u32, &[&str]); 4] = [
        (25, Status::SECURITY_VIOLATION, 0, &["start", "unload", "free-pool"]),
        (START_REFUSES_PARAMETER, Status::INVALID_PARAMETER, 0, &["start", "unload", "free-pool"]),
        (START_FAILS_BEFORE_ENTRY, Status::ABORTED, 0, &["start", "auto-unload", "free-pool"]),
        (
            START_ERROR_WITH_EXIT_DATA,
            Status::ABORTED,
            1,
            &["start", "auto-unload", "free-exit", "free-pool"],
        ),
    ];
    for entry in ENTRIES {
        for (mode, expected, entered, expected_events) in cases {
            let name = format!("{entry:?}: mode{mode}");
            let mut state = State::new();
            let report = deliver(entry, mode, &mut state, &bs);
            assert_eq!((report.status(), report.stage), (expected, 4), "{name}");
            assert_eq!(
                (report.load_status, report.start_status),
                (Some(Status::SUCCESS), Some(expected)),
                "{name}"
            );
            assert_eq!(
                events(),
                [&["allocate", "open-path", "load", "open-child"][..], expected_events].concat(),
                "{name}"
            );
            let observed = state.resident_options().unwrap();
            assert_eq!((observed.rust_entered, observed.armed), (entered, 0), "{name}");
            assert_released(&report, &state, &name);
        }
    }
}

#[test]
fn failed_cleanup_keeps_ownership_until_the_retry_succeeds() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let slot = resident_slot();
    // What the delivery reported, what the failed cleanup got to, what the retry still frees.
    let cases: [(u8, Status, &[&str], &[&str]); 3] = [
        (
            LOAD_SECURITY_VIOLATION_UNLOAD_FAILS,
            Status::SECURITY_VIOLATION,
            &["allocate", "open-path", "load", "unload"],
            &["unload", "free-pool"],
        ),
        (
            FREE_EXIT_DATA_FAILS,
            Status::ABORTED,
            &["allocate", "open-path", "load", "open-child", "start", "auto-unload", "free-exit"],
            &["free-exit", "free-pool"],
        ),
        (
            FREE_POOL_FAILS,
            Status::UNSUPPORTED,
            &["allocate", "open-path", "load", "open-child", "start", "auto-unload", "free-pool"],
            &["free-pool"],
        ),
    ];
    for entry in ENTRIES {
        for (mode, operation, expected_events, retried) in cases {
            let name = format!("{entry:?}: mode{mode}");
            let mut state = State::new();
            let report = deliver(entry, mode, &mut state, &bs);
            assert_eq!(report.operation_status, operation, "{name}");
            assert_eq!(report.cleanup_status, Status::DEVICE_ERROR, "{name}");
            assert_eq!(report.status(), Status::DEVICE_ERROR, "{name}");
            assert_eq!(events(), expected_events, "{name}");
            // A failed unload keeps the LoadOptions pool alive; nothing is retained as armed.
            assert!(!state.is_clean() && !state.is_retained(), "{name}");
            assert_ne!(FIX.lock().unwrap().pool, 0, "{name}");
            // Ownership that is still owed refuses another delivery without touching it.
            let mut reads = 0;
            let again = execute(
                entry,
                &mut state,
                &bs,
                (parent(), controller()),
                resident_options(mode),
                |offset| {
                    reads += 1;
                    word(&slot, offset)
                },
            );
            assert_eq!((again.status(), again.stage), (Status::NOT_READY, 0), "{name}");
            assert_eq!(again.cleanup_status, Status::SUCCESS, "{name}");
            assert_eq!((reads, events()), (0, expected_events.to_vec()), "{name}");
            // The retry still fails while the firmware does, and changes nothing.
            assert_eq!(unsafe { state.cleanup(&bs) }, Err(Status::DEVICE_ERROR), "{name}");
            assert!(!state.is_clean() && !state.is_retained(), "{name}");
            FIX.lock().unwrap().mode = 20;
            assert_eq!(unsafe { state.cleanup(&bs) }, Ok(()), "{name}");
            assert_eq!(events(), [expected_events, &retried[..1], retried].concat(), "{name}");
            assert!(state.is_clean() && !state.is_retained(), "{name}");
            let f = FIX.lock().unwrap();
            assert_eq!((f.pool, f.exit, f.loaded), (0, 0, 0), "{name}");
            drop(f);
            // Clean again: the same state delivers and retains a child.
            let report = deliver(entry, 20, &mut state, &bs);
            assert_eq!(report.status(), Status::SUCCESS, "{name}");
            assert!(state.is_retained(), "{name}");
            end_retained_lifetime();
        }
    }
}

#[test]
fn nonerror_return_retains_exit_data_with_the_child() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    for entry in ENTRIES {
        let mut state = State::new();
        let report = deliver(entry, START_SUCCESS_WITH_EXIT_DATA, &mut state, &bs);
        assert_eq!((report.status(), report.stage), (Status::SUCCESS, 4), "{entry:?}");
        assert!(state.is_retained() && !state.is_clean(), "{entry:?}");
        assert!(state.resident_options().unwrap().is_armed(), "{entry:?}");
        let expected = events();
        assert_eq!(expected, ["allocate", "open-path", "load", "open-child", "start"]);
        assert_eq!(unsafe { state.cleanup(&bs) }, Err(Status::UNSUPPORTED), "{entry:?}");
        assert_eq!(events(), expected, "{entry:?}");
        let f = FIX.lock().unwrap();
        assert!(f.pool != 0 && f.exit != 0 && f.loaded != 0, "{entry:?}");
        drop(f);
        end_retained_lifetime();
    }
}

/// `firmware/card/package-payload.py --resident` writes `payload-slot.bin`; point
/// `SVMVISOR_CARD_PE_TEST_SLOT` at one to check it against the parser the loader runs.
#[test]
fn optional_python_actual_slot_matches_rust_parser() {
    let Ok(path) = std::env::var("SVMVISOR_CARD_PE_TEST_SLOT") else {
        return;
    };
    let slot = std::fs::read(path).unwrap();
    assert_eq!(slot.len(), SLOT_BYTES);
    let pin = Pin::parse_resident(&slot[..128]).unwrap();
    pin.verify(&slot[128..128 + pin.payload_bytes]).unwrap();
}

#[cfg(feature = "card-resident-dev-loader")]
#[test]
fn dev_loader_adopts_a_valid_slot_header_and_matches_the_pinned_result() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let slot = resident_slot();
    // Pinned reference on the same slot.
    reset_fixture(20);
    let pin = Pin::parse_resident(&slot[..128]).unwrap();
    let mut pinned_state = State::new();
    let pinned = unsafe {
        child_image::execute_resident(
            &mut pinned_state,
            &bs,
            parent(),
            controller(),
            &pin,
            resident_options(20),
            |offset| word(&slot, offset),
        )
    };
    let pinned_events = FIX.lock().unwrap().events.clone();
    end_retained_lifetime();
    // Dev loader: no pin supplied, only the slot.
    reset_fixture(20);
    let mut state = State::new();
    let report = unsafe {
        child_image::execute_resident_dev(
            &mut state,
            &bs,
            parent(),
            controller(),
            resident_options(20),
            |offset| word(&slot, offset),
        )
    };
    assert_eq!(report.status(), Status::SUCCESS);
    assert_eq!(
        (report.stage, report.load_status, report.start_status),
        (pinned.stage, pinned.load_status, pinned.start_status)
    );
    assert_eq!(report.stage, 4);
    assert!(state.is_retained() && pinned_state.is_retained());
    assert_eq!(FIX.lock().unwrap().events, pinned_events);
    end_retained_lifetime();
    // A child failure after a good header is reported identically too.
    reset_fixture(24);
    let mut state = State::new();
    let report = unsafe {
        child_image::execute_resident_dev(
            &mut state,
            &bs,
            parent(),
            controller(),
            resident_options(24),
            |offset| word(&slot, offset),
        )
    };
    assert_eq!(report.status(), Status::UNSUPPORTED);
    assert!(state.is_clean());
}

#[cfg(feature = "card-resident-dev-loader")]
#[test]
fn dev_loader_refuses_every_corruption_class_with_the_pinned_status() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let good = resident_slot();
    let good_pin = Pin::parse_resident(&good[..128]).unwrap();
    type Corrupt = fn(&mut Vec<u8>);
    let classes: [(&str, Corrupt, u32); 12] = [
        ("magic", |s| s[0] ^= 1, 0),
        ("wrong kind: fully valid returning slot", |s| *s = fixture(), 0),
        ("version", |s| w32(s, 8, 2), 0),
        ("header size", |s| w32(s, 12, 132), 0),
        ("payload bytes beyond slot", |s| w64(s, 16, (SLOT_BYTES - 127) as u64), 0),
        ("payload bytes below minimum", |s| w64(s, 16, 511), 0),
        ("slot size", |s| w64(s, 24, 0x200000), 0),
        ("flags", |s| w64(s, 40, 2), 0),
        ("reserved tail of header", |s| s[127] = 1, 0),
        ("PE metadata: entry outside image", |s| w32(s, 88, 8192), 0),
        // Structurally valid header, so these reach stage 1 and fail the binding.
        ("digest mismatch", |s| s[48] ^= 1, 1),
        ("payload bit flip", |s| s[128 + 700] ^= 0x10, 1),
    ];
    for (name, corrupt, stage) in classes {
        let mut slot = good.clone();
        corrupt(&mut slot);
        reset_fixture(20);
        let mut state = State::new();
        let report = unsafe {
            child_image::execute_resident_dev(
                &mut state,
                &bs,
                parent(),
                controller(),
                resident_options(20),
                |offset| word(&slot, offset),
            )
        };
        assert_eq!(report.status(), Status::COMPROMISED_DATA, "{name}");
        assert_eq!(report.stage, stage, "{name}");
        assert!(state.is_clean() && !state.is_retained(), "{name}");
        assert!(!FIX.lock().unwrap().events.contains(&"load"), "{name}");
        assert_eq!(FIX.lock().unwrap().pool, 0, "{name}");
        // The pinned parent, holding the good header, refuses the same slot
        // with the same status.
        reset_fixture(20);
        let mut pinned_state = State::new();
        let pinned = unsafe {
            child_image::execute_resident(
                &mut pinned_state,
                &bs,
                parent(),
                controller(),
                &good_pin,
                resident_options(20),
                |offset| word(&slot, offset),
            )
        };
        assert_eq!(pinned.status(), report.status(), "{name}");
        assert!(pinned_state.is_clean(), "{name}");
    }
    // Digest matches a modified child, but the header's PE metadata no longer
    // describes that child.
    let mut slot = good.clone();
    w32(&mut slot, 128 + 104, 4096 + 1); // child entry RVA
    let digest = Sha256::digest(&slot[128..1152]);
    slot[48..80].copy_from_slice(&digest);
    reset_fixture(20);
    let mut state = State::new();
    let report = unsafe {
        child_image::execute_resident_dev(
            &mut state,
            &bs,
            parent(),
            controller(),
            resident_options(20),
            |offset| word(&slot, offset),
        )
    };
    assert_eq!((report.status(), report.stage), (Status::COMPROMISED_DATA, 1));
    assert!(state.is_clean());
    // Transport failure while reading the header is reported, not masked.
    reset_fixture(20);
    let mut state = State::new();
    let report = unsafe {
        child_image::execute_resident_dev(
            &mut state,
            &bs,
            parent(),
            controller(),
            resident_options(20),
            |_| Err(Status::DEVICE_ERROR),
        )
    };
    assert_eq!((report.status(), report.stage), (Status::DEVICE_ERROR, 0));
    // A header that changes between the adopting read and the delivery read.
    reset_fixture(20);
    let mut state = State::new();
    let mut header_reads = 0;
    let report = unsafe {
        child_image::execute_resident_dev(
            &mut state,
            &bs,
            parent(),
            controller(),
            resident_options(20),
            |offset| {
                if offset == 48 {
                    header_reads += 1;
                }
                let value = word(&good, offset)?;
                Ok(if offset == 48 && header_reads == 2 { value ^ 1 } else { value })
            },
        )
    };
    assert_eq!((report.status(), report.stage), (Status::COMPROMISED_DATA, 0));
    assert!(state.is_clean());
}

/// Parity, not policy: neither parent reads the slot beyond header + child, so
/// neither inspects the erased tail. `flash-card.ps1 -Action ProgramPayload`
/// enforces the 0xff tail by full-slot readback instead.
#[cfg(feature = "card-resident-dev-loader")]
#[test]
fn dev_loader_reads_no_more_of_the_slot_than_the_pinned_parent() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    let mut slot = resident_slot();
    slot[0x80000] = 0; // stale non-0xff tail byte: invisible to both parents
    reset_fixture(24);
    let (mut state, mut highest, mut reads) = (State::new(), 0u64, 0usize);
    let report = unsafe {
        child_image::execute_resident_dev(
            &mut state,
            &bs,
            parent(),
            controller(),
            resident_options(24),
            |offset| {
                highest = highest.max(offset);
                reads += 1;
                word(&slot, offset)
            },
        )
    };
    assert_eq!(report.status(), Status::UNSUPPORTED);
    assert_eq!(highest, 128 + 1024 - 4);
    assert_eq!(reads, 32 + 32 + 256);
}
