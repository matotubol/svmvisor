#![cfg(feature = "memory-attribute-provider")]

use std::sync::{Arc, Mutex};

use svmvisor_dxe::memory_attributes::Adapter;
use svmvisor_memory_attributes::{ACCESS_MASK, Attributes, Error, READ_ONLY};
use uefi_raw::Status;
use uefi_raw::protocol::memory_protection::MemoryAttributeProtocol;
use uefi_raw::table::boot::MemoryAttribute;

#[derive(Default)]
struct State {
    calls: Vec<(char, u64, u64, u64)>,
    result: u64,
    failure: Option<Error>,
    reenter: usize,
    reentry_status: Option<Status>,
}

struct Backend(Arc<Mutex<State>>);

impl Attributes for Backend {
    fn get(&mut self, base: u64, length: u64) -> Result<u64, Error> {
        let mut state = self.0.lock().unwrap();
        state.calls.push(('g', base, length, 0));
        if state.reenter != 0 {
            let pointer = state.reenter as *const MemoryAttributeProtocol;
            // SAFETY: The test retains the pinned owner for this invocation.
            state.reentry_status = Some(unsafe {
                ((*pointer).clear_memory_attributes)(
                    pointer,
                    base,
                    length,
                    MemoryAttribute::READ_ONLY,
                )
            });
        }
        state.failure.map_or(Ok(state.result), Err)
    }

    fn set(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap();
        state.calls.push(('s', base, length, attributes));
        state.failure.map_or(Ok(()), Err)
    }

    fn clear(&mut self, base: u64, length: u64, attributes: u64) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap();
        state.calls.push(('c', base, length, attributes));
        state.failure.map_or(Ok(()), Err)
    }
}

#[test]
fn exact_efi_interface_layout_and_guid() {
    use core::mem::{offset_of, size_of};
    assert_eq!(size_of::<MemoryAttributeProtocol>(), 3 * size_of::<usize>());
    assert_eq!(offset_of!(MemoryAttributeProtocol, get_memory_attributes), 0);
    assert_eq!(offset_of!(MemoryAttributeProtocol, set_memory_attributes), size_of::<usize>());
    assert_eq!(
        offset_of!(MemoryAttributeProtocol, clear_memory_attributes),
        2 * size_of::<usize>()
    );
    assert_eq!(
        MemoryAttributeProtocol::GUID,
        uefi_raw::guid!("f4560cf6-40ec-4b4a-a192-bf1d57d0b189")
    );
    assert_eq!(size_of::<MemoryAttribute>(), 8);
    assert_eq!(MemoryAttribute::READ_PROTECT.bits(), 0x2000);
    assert_eq!(MemoryAttribute::EXECUTE_PROTECT.bits(), 0x4000);
    assert_eq!(MemoryAttribute::READ_ONLY.bits(), 0x20000);
}

#[test]
fn callbacks_forward_full_width_arguments_and_all_masks() {
    let state = Arc::new(Mutex::new(State { result: ACCESS_MASK, ..State::default() }));
    let adapter = Box::pin(Adapter::new(Backend(state.clone())));
    let this = adapter.as_ref().protocol_ptr();
    // SAFETY: The pinned owner stays live throughout each test.
    let protocol = unsafe { &*this };
    let base = 0x1_2345_6000;
    let length = 0x1_0000_1000;
    let mut output = MemoryAttribute::empty();
    unsafe {
        assert_eq!(
            (protocol.get_memory_attributes)(this, base, length, &mut output),
            Status::SUCCESS
        );
        assert_eq!(output.bits(), ACCESS_MASK);
        for bits in 1..8 {
            let mask = (if bits & 1 != 0 { 0x2000 } else { 0 })
                | (if bits & 2 != 0 { 0x4000 } else { 0 })
                | (if bits & 4 != 0 { 0x20000 } else { 0 });
            let attributes = MemoryAttribute::from_bits_retain(mask);
            assert_eq!(
                (protocol.set_memory_attributes)(this, base, length, attributes),
                Status::SUCCESS
            );
            assert_eq!(
                (protocol.clear_memory_attributes)(this, base, length, attributes),
                Status::SUCCESS
            );
        }
    }
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 15);
    assert!(state.calls.iter().all(|call| call.1 == base && call.2 == length));
    for pair in state.calls[1..].chunks_exact(2) {
        assert_eq!(pair[0].0, 's');
        assert_eq!(pair[1].0, 'c');
        assert_eq!(pair[0].3, pair[1].3);
    }
}

#[test]
fn validation_precedes_backend_access_and_preserves_output() {
    let state = Arc::new(Mutex::new(State::default()));
    let adapter = Box::pin(Adapter::new(Backend(state.clone())));
    let this = adapter.as_ref().protocol_ptr();
    // SAFETY: The pinned owner stays live throughout each test.
    let protocol = unsafe { &*this };
    let mut output = MemoryAttribute::RUNTIME;
    unsafe {
        assert_eq!(
            (protocol.get_memory_attributes)(this, 1, 0, core::ptr::null_mut()),
            Status::UNSUPPORTED
        );
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0, 0, &mut output),
            Status::INVALID_PARAMETER
        );
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0, 4096, core::ptr::null_mut()),
            Status::INVALID_PARAMETER
        );
        assert_eq!(
            (protocol.get_memory_attributes)(core::ptr::null(), 0, 4096, &mut output),
            Status::INVALID_PARAMETER
        );
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0, 4096, core::ptr::without_provenance_mut(1)),
            Status::INVALID_PARAMETER
        );
        for change in [protocol.set_memory_attributes, protocol.clear_memory_attributes] {
            assert_eq!(change(this, 1, 1, MemoryAttribute::empty()), Status::INVALID_PARAMETER);
            assert_eq!(
                change(this, 0, 4096, MemoryAttribute::WRITE_BACK),
                Status::INVALID_PARAMETER
            );
            assert_eq!(change(this, 1, 0, MemoryAttribute::READ_ONLY), Status::INVALID_PARAMETER);
            assert_eq!(change(this, 1, 4096, MemoryAttribute::READ_ONLY), Status::UNSUPPORTED);
            assert_eq!(
                change(core::ptr::null(), 0, 4096, MemoryAttribute::READ_ONLY),
                Status::INVALID_PARAMETER
            );
        }
    }
    assert_eq!(output, MemoryAttribute::RUNTIME);
    assert!(state.lock().unwrap().calls.is_empty());
}

#[test]
fn every_engine_error_maps_to_efi_and_get_never_clobbers_on_failure() {
    let errors = [
        (Error::InvalidParameter, Status::INVALID_PARAMETER),
        (Error::Unsupported, Status::UNSUPPORTED),
        (Error::NoMapping, Status::NO_MAPPING),
        (Error::OutOfResources, Status::OUT_OF_RESOURCES),
        (Error::AccessDenied, Status::ACCESS_DENIED),
        (Error::DeviceError, Status::DEVICE_ERROR),
    ];
    let state = Arc::new(Mutex::new(State::default()));
    let adapter = Box::pin(Adapter::new(Backend(state.clone())));
    let this = adapter.as_ref().protocol_ptr();
    // SAFETY: The pinned owner stays live throughout each test.
    let protocol = unsafe { &*this };
    for (error, expected) in errors {
        state.lock().unwrap().failure = Some(error);
        let mut output = MemoryAttribute::RUNTIME;
        unsafe {
            assert_eq!((protocol.get_memory_attributes)(this, 0, 4096, &mut output), expected);
            assert_eq!(
                (protocol.set_memory_attributes)(this, 0, 4096, MemoryAttribute::READ_ONLY),
                expected
            );
            assert_eq!(
                (protocol.clear_memory_attributes)(this, 0, 4096, MemoryAttribute::READ_ONLY),
                expected
            );
        }
        assert_eq!(output, MemoryAttribute::RUNTIME);
    }
    state.lock().unwrap().failure = None;
    unsafe {
        assert_eq!(
            (protocol.clear_memory_attributes)(this, 0, 4096, MemoryAttribute::READ_ONLY),
            Status::SUCCESS
        );
    }
}

#[test]
fn reentry_fails_without_deadlock_and_guard_releases() {
    let state = Arc::new(Mutex::new(State { result: READ_ONLY, ..State::default() }));
    let adapter = Box::pin(Adapter::new(Backend(state.clone())));
    let this = adapter.as_ref().protocol_ptr();
    // SAFETY: The pinned owner stays live throughout each test.
    let protocol = unsafe { &*this };
    state.lock().unwrap().reenter = this as *const _ as usize;
    let mut output = MemoryAttribute::empty();
    for _ in 0..2 {
        unsafe {
            assert_eq!(
                (protocol.get_memory_attributes)(this, 0, 4096, &mut output),
                Status::SUCCESS
            );
        }
        assert_eq!(output.bits(), READ_ONLY);
        assert_eq!(state.lock().unwrap().reentry_status, Some(Status::ACCESS_DENIED));
    }
    assert_eq!(state.lock().unwrap().calls.len(), 2);
}

/// A complete owned four-level table snapshot for an ABI-to-engine test.
struct Tables {
    live: Box<[[u64; 512]; 4]>,
    staged: Option<Box<[[u64; 512]; 4]>>,
}

impl svmvisor_memory_attributes::Memory for Tables {
    fn read_entry(&mut self, address: u64) -> Result<u64, Error> {
        let tables = self.staged.as_ref().unwrap_or(&self.live);
        let page = (address / 4096).checked_sub(1).ok_or(Error::DeviceError)? as usize;
        if page >= 4 || address & 7 != 0 {
            return Err(Error::DeviceError);
        }
        Ok(tables[page][(address % 4096 / 8) as usize])
    }
    fn begin_update(&mut self) -> Result<(), Error> {
        self.staged = Some(self.live.clone());
        Ok(())
    }
    fn write_entry(&mut self, address: u64, value: u64) -> Result<(), Error> {
        let page = (address / 4096).checked_sub(1).ok_or(Error::DeviceError)? as usize;
        if page >= 4 || address & 7 != 0 {
            return Err(Error::DeviceError);
        }
        self.staged.as_mut().ok_or(Error::AccessDenied)?[page][(address % 4096 / 8) as usize] =
            value;
        Ok(())
    }
    fn allocate_table(&mut self) -> Result<u64, Error> {
        Err(Error::OutOfResources)
    }
    fn commit_update(&mut self) -> Result<(), Error> {
        self.live = self.staged.take().ok_or(Error::AccessDenied)?;
        Ok(())
    }
    fn abort_update(&mut self) {
        self.staged = None;
    }
}

#[test]
fn real_provider_round_trip_through_efi_callbacks() {
    let mut live = Box::new([[0; 512]; 4]);
    live[0][0] = 0x2003;
    live[1][0] = 0x3003;
    live[2][0] = 0x4003;
    live[3][8] = 0x8003;
    live[3][9] = 0x9003;
    let provider = svmvisor_memory_attributes::Provider {
        config: svmvisor_memory_attributes::Config {
            root: 0x1000,
            physical_bits: 48,
            nxe: true,
            page1gb: true,
        },
        memory: Tables { live, staged: None },
    };
    let adapter = Box::pin(Adapter::new(provider));
    let this = adapter.as_ref().protocol_ptr();
    // SAFETY: The pinned owner stays live throughout each test.
    let protocol = unsafe { &*this };
    let mask = MemoryAttribute::from_bits_retain(ACCESS_MASK);
    let mut output = MemoryAttribute::RUNTIME;
    unsafe {
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0x8000, 4096, &mut output),
            Status::SUCCESS
        );
        assert_eq!(output.bits(), 0);
        assert_eq!((protocol.set_memory_attributes)(this, 0x8000, 4096, mask), Status::SUCCESS);
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0x8000, 4096, &mut output),
            Status::SUCCESS
        );
        assert_eq!(output, mask);
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0x8000, 8192, &mut output),
            Status::NO_MAPPING
        );
        assert_eq!(output, mask);
        assert_eq!((protocol.clear_memory_attributes)(this, 0x8000, 4096, mask), Status::SUCCESS);
        assert_eq!(
            (protocol.get_memory_attributes)(this, 0x8000, 8192, &mut output),
            Status::SUCCESS
        );
        assert_eq!(output.bits(), 0);
    }
}
