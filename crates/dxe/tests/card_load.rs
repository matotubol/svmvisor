//! Runs the production load-only adapter with mocked PCI transport and firmware.
#![cfg(all(feature = "card-load-only", target_os = "windows"))]
#![allow(dead_code)]
#[path = "../src/delivery/load.rs"]
mod card_load;
use sha2::{Digest, Sha256};
use std::{
    ffi::c_void,
    mem::{size_of, MaybeUninit},
    sync::Mutex,
};
use uefi_raw::{
    table::boot::{AllocateType, BootServices, MemoryType},
    Status,
};
const ARENA: usize = 0x100000;
struct State {
    slot: Vec<u8>,
    pool: usize,
    pool_len: usize,
    pages: u64,
    mode: u8,
    reads: usize,
    staged: [u32; 8],
    last: [u32; 8],
    frees: usize,
}
static STATE: Mutex<State> = Mutex::new(State {
    slot: Vec::new(),
    pool: 0,
    pool_len: 0,
    pages: 0,
    mode: 0,
    reads: 0,
    staged: [0; 8],
    last: [0; 8],
    frees: 0,
});
mod pci_io {
    use super::*;
    use svmvisor_dxe::diagnostics::journal::JournalIo;
    pub struct Bar0;
    pub fn status_result(s: Status) -> Result<(), Status> {
        if s.is_error() {
            Err(s)
        } else {
            Ok(())
        }
    }
    impl Bar0 {
        pub fn card_word(&self, offset: u64) -> Result<u32, Status> {
            let mut s = STATE.lock().unwrap();
            s.reads += 1;
            assert_eq!(offset & 3, 0);
            assert!(offset as usize + 4 <= s.slot.len());
            if s.mode == 3 && offset >= 128 {
                return Err(Status::DEVICE_ERROR);
            }
            Ok(u32::from_le_bytes(
                s.slot[offset as usize..offset as usize + 4]
                    .try_into()
                    .unwrap(),
            ))
        }
    }
    impl JournalIo for Bar0 {
        fn read(&mut self, o: u64) -> Result<u32, Status> {
            let s = STATE.lock().unwrap();
            Ok(match o {
                0x24 => 0,
                0x2c => s.last[0],
                0x80..=0x9c => s.last[(o as usize - 0x80) / 4],
                _ => panic!("unexpected journalread"),
            })
        }
        fn write(&mut self, o: u64, v: u32) -> Result<(), Status> {
            let mut s = STATE.lock().unwrap();
            if o == 0x60 {
                s.last = s.staged;
            } else {
                s.staged[(o as usize - 0x40) / 4] = v;
            }
            Ok(())
        }
    }
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protection: u32) -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
}
unsafe extern "efiapi" fn allocate_pool(ty: MemoryType, size: usize, out: *mut *mut u8) -> Status {
    assert_eq!(ty, MemoryType::LOADER_DATA);
    let mut s = STATE.lock().unwrap();
    if s.mode == 1 {
        return Status::OUT_OF_RESOURCES;
    }
    assert_eq!(s.pool, 0);
    let p = Box::into_raw(vec![0u8; size].into_boxed_slice()).cast::<u8>();
    s.pool = p as usize;
    s.pool_len = size;
    unsafe {
        *out = p;
    }
    Status::SUCCESS
}
unsafe extern "efiapi" fn free_pool(p: *mut u8) -> Status {
    let mut s = STATE.lock().unwrap();
    assert_eq!(p as usize, s.pool);
    if s.mode == 5 {
        return Status::DEVICE_ERROR;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            p, s.pool_len,
        )));
    }
    s.pool = 0;
    s.frees += 1;
    Status::SUCCESS
}
unsafe extern "efiapi" fn allocate_pages(
    kind: AllocateType,
    ty: MemoryType,
    pages: usize,
    out: *mut u64,
) -> Status {
    assert_eq!(kind, AllocateType::ADDRESS);
    assert_eq!(ty, MemoryType::LOADER_DATA);
    assert_eq!(pages * 4096, ARENA);
    let mut s = STATE.lock().unwrap();
    if s.mode == 2 {
        return Status::OUT_OF_RESOURCES;
    }
    assert_eq!(s.pages, 0);
    let requested = unsafe { *out };
    let actual = if s.mode == 6 {
        requested + 0x100000
    } else {
        requested
    };
    let p = unsafe { VirtualAlloc(actual as *mut c_void, ARENA, 0x3000, 4) };
    if p.is_null() {
        return Status::OUT_OF_RESOURCES;
    }
    s.pages = p as u64;
    unsafe {
        *out = s.pages;
    }
    Status::SUCCESS
}
unsafe extern "efiapi" fn free_pages(base: u64, pages: usize) -> Status {
    let mut s = STATE.lock().unwrap();
    assert_eq!(base, s.pages);
    assert_eq!(pages * 4096, ARENA);
    if s.mode == 4 {
        return Status::DEVICE_ERROR;
    }
    if s.mode == 0 {
        assert_eq!(unsafe { *(base as *const u64) }, base + 16);
    }
    assert_ne!(unsafe { VirtualFree(base as *mut c_void, 0, 0x8000) }, 0);
    s.pages = 0;
    s.frees += 1;
    Status::SUCCESS
}
unsafe extern "efiapi" fn forbidden() -> Status {
    panic!("unexpected firmware call")
}
fn services() -> BootServices {
    let mut raw = MaybeUninit::<BootServices>::uninit();
    unsafe {
        let words = raw.as_mut_ptr().cast::<usize>();
        for i in 0..size_of::<BootServices>() / size_of::<usize>() {
            words.add(i).write(forbidden as *const () as usize);
        }
        std::ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(std::mem::zeroed());
        std::ptr::addr_of_mut!((*raw.as_mut_ptr()).allocate_pool).write(allocate_pool);
        std::ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pool).write(free_pool);
        std::ptr::addr_of_mut!((*raw.as_mut_ptr()).allocate_pages).write(allocate_pages);
        std::ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pages).write(free_pages);
        raw.assume_init()
    }
}
fn fixture() -> (Vec<u8>, String) {
    let mut p = vec![0; 96];
    p[..8].copy_from_slice(b"SVMRELO1");
    for (offset, value) in [
        (8, 0x100000u64),
        (16, ARENA as u64),
        (24, 16),
        (32, 32),
        (40, 8),
        (48, 1),
        (64, 0x100010),
        (80, 0),
        (88, 8),
    ] {
        p[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    let hash = Sha256::digest(&p);
    let pin = hash.iter().map(|b| format!("{b:02x}")).collect();
    let mut slot = vec![0xff; ARENA];
    slot[..128].fill(0);
    slot[..8].copy_from_slice(b"SVMCRD01");
    slot[8..12].copy_from_slice(&1u32.to_le_bytes());
    slot[12..16].copy_from_slice(&128u32.to_le_bytes());
    for (offset, value) in [(16, 96u64), (24, ARENA as u64), (32, 128), (40, 1)] {
        slot[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    slot[48..80].copy_from_slice(&hash);
    slot[128..224].copy_from_slice(&p);
    (slot, pin)
}
#[test]
fn production_load_adapter_reclaims_success_and_all_error_paths_without_execution() {
    let services = services();
    let mut io = pci_io::Bar0;
    for mode in 0..=7u8 {
        let (slot, pin) = fixture();
        {
            let mut s = STATE.lock().unwrap();
            assert_eq!((s.pool, s.pages), (0, 0));
            s.slot = slot;
            s.mode = mode;
            s.reads = 0;
            s.frees = 0;
            if mode == 7 {
                s.slot[190] ^= 1;
            }
        }
        let result = card_load::verify_with_pin(&mut io, &services, 7, 9, 11, &pin);
        let expected_success = mode == 0;
        assert_eq!(result.is_ok(), expected_success, "mode{mode}");
        {
            let s = STATE.lock().unwrap();
            assert_eq!(s.last[7], if expected_success { 0x50010 } else { 0x5001f });
            assert_eq!(&s.last[4..6], &[0x44524143, 0x44414f4c]);
            if mode == 4 {
                assert_ne!(s.pages, 0);
                assert_ne!(s.pool, 0);
            } else if mode == 5 {
                assert_eq!(s.pages, 0);
                assert_ne!(s.pool, 0);
            } else {
                assert_eq!((s.pool, s.pages), (0, 0));
            }
        }
        STATE.lock().unwrap().mode = 8;
        card_load::cleanup(&services).unwrap();
        let s = STATE.lock().unwrap();
        assert_eq!((s.pool, s.pages), (0, 0));
        if expected_success {
            assert_eq!(s.frees, 2);
        }
    }
}
