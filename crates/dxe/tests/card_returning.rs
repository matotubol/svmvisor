//! Exact production PE loader with real owned allocations and a fake firmware ABI.
#![cfg(all(any(feature = "card-returning-loader", feature = "card-resident-loader"), target_os = "windows"))]
use sha2::{Digest, Sha256};
use std::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr,
    sync::Mutex,
};
use svmvisor_dxe::{
    card_returning::{self, Pin, State},
    native_result::NativeResult,
};
use uefi_raw::{
    Boolean, Char16, Handle, Status,
    protocol::{device_path::DevicePathProtocol, loaded_image::LoadedImageProtocol},
    table::boot::{BootServices, MemoryType},
};
struct Fixture {
    mode: u8,
    pool: usize,
    bytes: usize,
    exit: usize,
    loaded: usize,
    events: Vec<&'static str>,
}
static FIX: Mutex<Fixture> = Mutex::new(Fixture {
    mode: 0,
    pool: 0,
    bytes: 0,
    exit: 0,
    loaded: 0,
    events: Vec::new(),
});
static TEST_LOCK: Mutex<()> = Mutex::new(());
static PATH: [u8; 10] = [1, 1, 6, 0, 0, 0, 0x7f, 0xff, 4, 0];
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
fn fixture() -> (Pin, Vec<u8>) {
    let mut pe = vec![0; 1024];
    pe[..2].copy_from_slice(b"MZ");
    w32(&mut pe, 0x3c, 64);
    pe[64..68].copy_from_slice(b"PE\0\0");
    for (o, v) in [
        (68, 0x8664),
        (70, 1),
        (84, 240),
        (86, 2),
        (88, 0x20b),
        (156, 11),
    ] {
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
    let mut slot = vec![0xff; card_returning::SLOT_BYTES];
    slot[..128].fill(0);
    slot[..8].copy_from_slice(b"SVMPE001");
    for (o, v) in [
        (8, 1),
        (12, 128),
        (88, 4096),
        (92, 8192),
        (96, 512),
        (100, 4096),
        (104, 512),
        (108, 1),
    ] {
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
    (Pin::parse(&slot[..128]).unwrap(), slot)
}
unsafe extern "efiapi" fn alloc(ty: MemoryType, n: usize, out: *mut *mut u8) -> Status {
    let mut f = FIX.lock().unwrap();
    assert_eq!(ty, if f.mode >= 20 { MemoryType::RUNTIME_SERVICES_DATA } else { MemoryType::LOADER_DATA });
    f.events.push("allocate");
    if f.mode == 7 {
        return Status::OUT_OF_RESOURCES;
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
        unsafe {
            drop(Box::from_raw(p.cast::<[u8; 4]>()));
        }
        f.exit = 0;
        return Status::SUCCESS;
    }
    assert_eq!(p as usize, f.pool);
    f.events.push("free-pool");
    if f.mode == 5 {
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
        unsafe {
            *out = PATH.as_ptr().cast_mut().cast();
        }
    } else {
        assert_eq!(unsafe { *guid }, LoadedImageProtocol::GUID);
        assert_eq!(handle, child());
        f.events.push("open-child");
        unsafe {
            *out = if f.mode == 11 {
                ptr::null_mut()
            } else {
                f.loaded as *mut c_void
            };
        }
    }
    Status::SUCCESS
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
    if f.mode == 12 {
        return Status::LOAD_ERROR;
    }
    let l = Box::new(LoadedImageProtocol {
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
        image_code_type: if f.mode >= 20 { MemoryType::RUNTIME_SERVICES_CODE } else { MemoryType::BOOT_SERVICES_CODE },
        image_data_type: if f.mode >= 20 { MemoryType::RUNTIME_SERVICES_DATA } else { MemoryType::BOOT_SERVICES_DATA },
        unload: None,
    });
    f.loaded = Box::into_raw(l) as usize;
    unsafe { *out = child() };
    if matches!(f.mode, 2 | 6) {
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
        use svmvisor_dxe::diagnostics::resident_boot::ResidentBootOptions;
        let m = unsafe { &mut *l.load_options.cast_mut().cast::<ResidentBootOptions>() };
        assert_eq!(*m, resident_options(f.mode));
        if f.mode == 25 { return Status::SECURITY_VIOLATION; }
        m.rust_entered = 1;
        if f.mode == 28 {
            m.failure = Status::OUT_OF_RESOURCES.0 as u64;
            m.preparation_stage = 6;
            m.preparation_reason = 2;
            m.preparation_address = 0x123456789abc;
            unsafe { drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol)); }
            f.loaded = 0;
            f.events.push("auto-unload");
            return Status::OUT_OF_RESOURCES;
        }
        if f.mode == 24 || f.mode == 26 {
            m.failure = 17;
            unsafe { drop(Box::from_raw(f.loaded as *mut LoadedImageProtocol)); }
            f.loaded = 0;
            f.events.push("auto-unload");
            return if f.mode == 26 { Status::INVALID_PARAMETER } else { Status::UNSUPPORTED };
        }
        if f.mode != 21 { m.armed = 1; }
        if f.mode == 22 { m.magic[0] ^= 1; }
        if f.mode == 27 { m.journal_base += 4096; }
        // These mutations remain valid headers, so only exact comparison with
        // the original immutable parent inputs can reject the acknowledgement.
        if f.mode == 29 { m.version = 1; }
        if f.mode == 31 { m.reserved[2] ^= 2; }
        if f.mode == 32 { m.version = 2; m.reserved = [0; 7]; }
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
unsafe extern "efiapi" fn unload(handle: Handle) -> Status {
    assert_eq!(handle, child());
    let mut f = FIX.lock().unwrap();
    f.events.push("unload");
    assert_ne!(f.loaded, 0, "stale child handle used");
    if f.mode == 6 {
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
#[test]
fn actual_adapter_covers_return_security_transport_and_retry_ownership() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    for mode in 0..=16 {
        *FIX.lock().unwrap() = Fixture {
            mode,
            pool: 0,
            bytes: 0,
            exit: 0,
            loaded: 0,
            events: Vec::new(),
        };
        let (pin, mut slot) = fixture();
        if mode == 8 {
            slot[16] ^= 1;
        }
        if mode == 9 {
            slot[128 + 512] ^= 1;
        }
        let mut state = State::new();
        let mut reads = 0usize;
        let report = unsafe {
            card_returning::execute(&mut state, &bs, parent(), controller(), &pin, |offset| {
                assert_eq!(offset & 3, 0);
                assert!(offset + 4 <= card_returning::SLOT_BYTES as u64);
                reads += 1;
                if mode == 10 && offset >= 128 {
                    return Err(Status::DEVICE_ERROR);
                }
                Ok(u32::from_le_bytes(
                    slot[offset as usize..offset as usize + 4]
                        .try_into()
                        .unwrap(),
                ))
            })
        };
        let expected = match mode {
            0 | 1 | 13 => Status::SUCCESS,
            2 | 3 | 6 => Status::SECURITY_VIOLATION,
            4 => Status::PROTOCOL_ERROR,
            7 => Status::OUT_OF_RESOURCES,
            8 | 9 | 14 => Status::COMPROMISED_DATA,
            10 | 11 | 15 => Status::DEVICE_ERROR,
            12 => Status::LOAD_ERROR,
            16 => Status::INVALID_PARAMETER,
            5 => Status::SUCCESS,
            _ => unreachable!(),
        };
        assert_eq!(report.operation_status, expected, "mode{mode}");
        let f = FIX.lock().unwrap();
        if matches!(mode, 0 | 1 | 5 | 13) {
            assert!(!f.events.contains(&"unload"));
            assert!(f.events.contains(&"auto-unload"));
        }
        if matches!(mode, 2 | 6 | 11 | 14) {
            assert!(!f.events.contains(&"start"));
            assert!(f.events.contains(&"unload"));
        }
        if matches!(mode, 7 | 8 | 9 | 10 | 15) {
            assert!(!f.events.contains(&"load"));
        }
        if mode == 13 {
            assert!(
                f.events.iter().position(|e| *e == "free-exit")
                    < f.events.iter().position(|e| *e == "free-pool")
            );
        }
        if mode == 1 {
            assert_eq!(report.inner.rust_completed, 1);
        } else if mode != 4 {
            assert_eq!(report.inner.rust_completed, 0);
        }
        drop(f);
        if matches!(mode, 5 | 6) {
            assert_eq!(report.cleanup_status, Status::DEVICE_ERROR);
            assert!(!state.is_clean());
            FIX.lock().unwrap().mode = 0;
            unsafe {
                state.cleanup(&bs).unwrap();
            }
        } else {
            assert_eq!(report.cleanup_status, Status::SUCCESS);
        }
        assert!(state.is_clean(), "mode{mode}");
        assert_eq!(FIX.lock().unwrap().pool, 0);
        assert_eq!(FIX.lock().unwrap().exit, 0);
        assert!(reads <= 32 + 256);
    }
}
#[test]
fn optional_python_actual_slot_matches_rust_parser() {
    let Ok(path) = std::env::var("SVMVISOR_CARD_PE_TEST_SLOT") else {
        return;
    };
    let slot = std::fs::read(path).unwrap();
    assert_eq!(slot.len(), card_returning::SLOT_BYTES);
    let pin = Pin::parse(&slot[..128]).unwrap();
    pin.verify(&slot[128..128 + pin.payload_bytes]).unwrap();
}

fn resident_options(mode: u8) -> svmvisor_dxe::diagnostics::resident_boot::ResidentBootOptions {
    use svmvisor_dxe::diagnostics::resident_boot::ResidentBootOptions;
    use svmvisor_hypervisor::host::resident::terminal::TerminalEndpoint;
    let options = ResidentBootOptions::new(0xd0000000, 42);
    if mode < 30 { return options; }
    options.with_terminal(TerminalEndpoint { config_page: 0xe012a000,
        bar0_host_page: 0xd0000000, fpga_build_id: 1, rom_build_id: 2,
        mmio_config_msr: 0xe0000021, bar0_raw: 0xd0000000, segment_bdf: 0x12a,
        boot_id: 42, command: 2, version: 1, reserved: 0 }).unwrap()
}

#[test]
fn resident_lifetime_requires_ack_and_retains_all_nonerror_returns() {
    let _guard = TEST_LOCK.lock().unwrap();
    let bs = services();
    for mode in 20..=32 {
        *FIX.lock().unwrap() = Fixture { mode, pool: 0, bytes: 0, exit: 0, loaded: 0, events: Vec::new() };
        let (_, mut slot) = fixture();
        slot[..8].copy_from_slice(b"SVMBPE01");
        w64(&mut slot, 40, 4); w16(&mut slot, 82, 12);
        w16(&mut slot, 128 + 156, 12);
        let digest = Sha256::digest(&slot[128..1152]);
        slot[48..80].copy_from_slice(&digest);
        assert!(Pin::parse(&slot[..128]).is_err());
        let pin = Pin::parse_resident(&slot[..128]).unwrap();
        let mut state = State::new();
        let report = unsafe { card_returning::execute_resident(&mut state, &bs,
            parent(), controller(), &pin, resident_options(mode),
            |offset| Ok(u32::from_le_bytes(slot[offset as usize..offset as usize + 4].try_into().unwrap()))) };
        let expected = match mode { 20 | 30 => Status::SUCCESS, 24 => Status::UNSUPPORTED,
            25 => Status::SECURITY_VIOLATION, 26 => Status::INVALID_PARAMETER,
            28 => Status::OUT_OF_RESOURCES,
            _ => Status::PROTOCOL_ERROR };
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
            f.loaded = 0; f.pool = 0;
        } else { assert!(state.is_clean()); }
    }
}
