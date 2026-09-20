//! Execute the actual binding callbacks against a tiny firmware/PCI-I/O fixture.
//! This checks software dispatch/ownership, not automatic platform ROM dispatch.

// This record-only fixture does not provide payload-loader hooks.
// Returning delivery/lifecycle have their own fixtures and image build checks.
#![cfg(not(any(feature = "card-returning-loader", feature = "card-resident")))]

#[path = "../src/firmware/cpu.rs"]
mod cpu;
#[path = "../src/firmware/driver.rs"]
mod driver;
#[path = "../src/firmware/lifecycle.rs"]
mod lifecycle;
#[path = "../src/firmware/pci_io.rs"]
mod pci_io;
#[path = "../src/firmware/mmio.rs"]
mod real_mmio;

// Only the physical bus is substituted. Descriptor validation, binding,
// registration, notification functions and journal verification are real code.
mod mmio {
    use svmvisor_card_loader::diagnostics::journal::JournalIo;
    use uefi_raw::Status;
    #[derive(Clone, Copy)]
    pub(crate) struct JournalMapping;
    impl JournalMapping {
        pub(crate) unsafe fn from_descriptor(p: *const u8) -> Result<Self, Status> {
            unsafe { super::real_mmio::JournalMapping::from_descriptor(p) }?;
            Ok(Self)
        }
    }
    impl JournalIo for JournalMapping {
        fn read(&mut self, offset: u64) -> Result<u32, Status> {
            let mut word = 0u32;
            let status = unsafe {
                super::read(core::ptr::null(), 2, 0, offset, 1, (&mut word as *mut u32).cast())
            };
            if status.is_error() { Err(status) } else { Ok(word) }
        }
        fn write(&mut self, offset: u64, mut word: u32) -> Result<(), Status> {
            let status = unsafe {
                super::write(core::ptr::null(), 2, 0, offset, 1, (&mut word as *mut u32).cast())
            };
            if status.is_error() { Err(status) } else { Ok(()) }
        }
    }
}

use core::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr::{null, null_mut},
};
use std::sync::Mutex;

use uefi_raw::{
    Event, Guid, Handle, Status,
    protocol::{driver::DriverBindingProtocol, loaded_image::LoadedImageProtocol},
    table::{
        boot::{BootServices, EventNotifyFn, EventType, InterfaceType, Tpl},
        system::SystemTable,
    },
};

static STATE: Mutex<State> = Mutex::new(State {
    owned: false,
    writes: Vec::new(),
    staging: [0; 8],
    last: [0; 8],
    drop_commit: false,
    command: 2,
    identity: 0x066610ee,
    abi: 0x10001,
    polls: 0,
    binding: 0,
    attribute_calls: Vec::new(),
    attribute_mode: 0,
    events: Vec::new(),
    create_failure: 255,
    close_failure: false,
    firmware_calls: 0,
});

static PCI: MockPci = MockPci {
    unused_poll: [0; 2],
    read,
    write,
    unused_io: [0; 2],
    config,
    unused: [0; 8],
    attributes,
    get_bar,
};

struct State {
    owned: bool,
    writes: Vec<(u64, u32)>,
    staging: [u32; 8],
    last: [u32; 8],
    drop_commit: bool,
    command: u32,
    identity: u32,
    abi: u32,
    polls: usize,
    binding: usize,
    attribute_calls: Vec<(u32, u64)>,
    attribute_mode: u8,
    events: Vec<(usize, EventNotifyFn, usize, bool)>,
    create_failure: u8,
    close_failure: bool,
    firmware_calls: usize,
}

fn image() -> Handle {
    0x1000usize as Handle
}

fn owner() -> Handle {
    0x2000usize as Handle
}

// Mirror the normative prefix independently of the production private fields.
#[repr(C)]
struct MockPci {
    unused_poll: [usize; 2],
    read: unsafe extern "efiapi" fn(*const c_void, u32, u8, u64, usize, *mut c_void) -> Status,
    write: unsafe extern "efiapi" fn(*const c_void, u32, u8, u64, usize, *mut c_void) -> Status,
    unused_io: [usize; 2],
    config: unsafe extern "efiapi" fn(*const c_void, u32, u32, usize, *mut c_void) -> Status,
    // Pci.Write, CopyMem, Map, Unmap, AllocateBuffer, FreeBuffer, Flush, GetLocation.
    unused: [usize; 8],
    attributes: unsafe extern "efiapi" fn(*const c_void, u32, u64, *mut u64) -> Status,
    get_bar: unsafe extern "efiapi" fn(*const c_void, u8, *mut u64, *mut *mut u8) -> Status,
}

unsafe extern "efiapi" fn get_bar(
    _: *const c_void,
    index: u8,
    _: *mut u64,
    result: *mut *mut u8,
) -> Status {
    STATE.lock().unwrap().firmware_calls += 1;
    assert_eq!(index, 0);
    let mut bytes = Box::new([0u8; 48]);
    bytes[0] = 0x8a;
    bytes[1] = 43;
    bytes[6..14].copy_from_slice(&32u64.to_le_bytes());
    bytes[14..22].copy_from_slice(&0x80000000u64.to_le_bytes());
    bytes[38..46].copy_from_slice(&4096u64.to_le_bytes());
    unsafe {
        *result = Box::into_raw(bytes).cast();
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn free_pool(buffer: *mut u8) -> Status {
    STATE.lock().unwrap().firmware_calls += 1;
    unsafe {
        drop(Box::from_raw(buffer.cast::<[u8; 48]>()));
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn create_event(
    ty: EventType,
    tpl: Tpl,
    callback: Option<EventNotifyFn>,
    context: *mut c_void,
    group: *mut Guid,
    out: *mut Event,
) -> Status {
    assert_eq!(ty, EventType::NOTIFY_SIGNAL);
    assert_eq!(tpl, Tpl::NOTIFY);
    let kind = context as usize;
    assert!(kind < 3);
    let expected = if kind == 0 {
        uefi_raw::guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b")
    } else if kind == 1 {
        uefi_raw::guid!("3a2a00ad-98b9-4cdf-a478-702777f1c10b")
    } else {
        uefi_raw::guid!("27abf055-b1b8-4c26-8048-748f37baa2df")
    };
    assert_eq!(unsafe { *group }, expected);
    let mut s = STATE.lock().unwrap();
    s.firmware_calls += 1;
    if s.create_failure == kind as u8 {
        return Status::OUT_OF_RESOURCES;
    }
    let event = (s.events.len() + 1) * 16;
    s.events.push((event, callback.unwrap(), kind, true));
    unsafe {
        *out = event as Event;
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn close_event(event: Event) -> Status {
    let mut s = STATE.lock().unwrap();
    s.firmware_calls += 1;
    if s.close_failure {
        return Status::DEVICE_ERROR;
    }
    let found = s.events.iter_mut().find(|item| item.0 == event as usize).unwrap();
    assert!(found.3);
    found.3 = false;
    Status::SUCCESS
}

fn signal(kind: usize) {
    let event = *STATE.lock().unwrap().events.iter().rev().find(|e| e.2 == kind && e.3).unwrap();
    unsafe {
        (event.1)(event.0 as Event, event.2 as *mut c_void);
    }
}

unsafe extern "efiapi" fn attributes(
    _: *const c_void,
    operation: u32,
    mask: u64,
    result: *mut u64,
) -> Status {
    let mut s = STATE.lock().unwrap();
    s.attribute_calls.push((operation, mask));
    s.firmware_calls += 1;
    if operation == 4 {
        assert_eq!(mask, 0);
        unsafe {
            *result = if s.attribute_mode == 1 { 0 } else { 0x200 };
        }
        return Status::SUCCESS;
    }
    // Production must never request IO, BME, full Set, or caching changes.
    assert_eq!(mask, 0x200);
    assert!(result.is_null());
    match operation {
        2 => {
            if s.attribute_mode != 3 {
                s.command |= 2;
            }
            if s.attribute_mode == 6 {
                s.command ^= 4;
            }
            if s.attribute_mode == 2 {
                return Status::DEVICE_ERROR;
            }
        }
        3 => {
            if s.attribute_mode != 5 {
                s.command &= !2;
            }
            if s.attribute_mode == 4 {
                return Status::DEVICE_ERROR;
            }
        }
        _ => panic!("unexpected attribute operation"),
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn config(
    _: *const c_void,
    width: u32,
    offset: u32,
    count: usize,
    buffer: *mut c_void,
) -> Status {
    STATE.lock().unwrap().firmware_calls += 1;
    assert_eq!((width, count), (2, 1));
    let s = STATE.lock().unwrap();
    unsafe {
        *buffer.cast::<u32>() = match offset {
            0 => s.identity,
            4 => s.command,
            8 => 0xff000003,
            _ => panic!("unexpected config access"),
        };
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn read(
    this: *const c_void,
    width: u32,
    bar: u8,
    offset: u64,
    count: usize,
    buffer: *mut c_void,
) -> Status {
    if !this.is_null() {
        STATE.lock().unwrap().firmware_calls += 1;
    }
    assert_eq!((width, bar, count), (2, 0, 1));
    let mut s = STATE.lock().unwrap();
    assert_ne!(s.command & 2, 0, "BAR read with memory decode disabled");
    let value = match offset {
        0 => 0x4a4d5653,
        4 => s.abi,
        0x024 => 0,
        0x02c => {
            s.polls += 1;
            s.last[0]
        }
        0x080..=0x09c => s.last[(offset as usize - 0x080) / 4],
        _ => panic!("unexpected BAR read"),
    };
    unsafe {
        *buffer.cast::<u32>() = value;
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn write(
    this: *const c_void,
    width: u32,
    bar: u8,
    offset: u64,
    count: usize,
    buffer: *mut c_void,
) -> Status {
    if !this.is_null() {
        STATE.lock().unwrap().firmware_calls += 1;
    }
    assert_eq!((width, bar, count), (2, 0, 1));
    assert!(offset >= 0x040 && offset <= 0x060 && offset % 4 == 0);
    let value = unsafe { *buffer.cast::<u32>() };
    let mut s = STATE.lock().unwrap();
    assert_ne!(s.command & 2, 0, "BAR write with memory decode disabled");
    s.writes.push((offset, value));
    if offset == 0x060 {
        assert_eq!(value, s.staging[0]);
        if !s.drop_commit {
            s.last = s.staging;
        }
    } else {
        s.staging[(offset as usize - 0x040) / 4] = value;
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn open(
    handle: Handle,
    guid: *const Guid,
    out: *mut *mut c_void,
    agent: Handle,
    controller: Handle,
    attrs: u32,
) -> Status {
    assert_eq!(agent, image());
    if unsafe { *guid } == LoadedImageProtocol::GUID {
        assert_eq!((handle, controller, attrs), (image(), null_mut(), 2));
        // LoadedImage consists of integer/newtype/pointer fields and Option<fn>;
        // zero is a valid representation for this fixture before setting owner.
        let mut loaded: Box<LoadedImageProtocol> = Box::new(unsafe { core::mem::zeroed() });
        loaded.device_handle = owner();
        unsafe {
            *out = Box::into_raw(loaded).cast();
        }
    } else {
        assert_eq!(unsafe { *guid }, uefi_raw::guid!("4cf5b200-68b8-4ca5-9eec-b23e3f50029a"));
        assert_eq!((handle, controller, attrs), (owner(), owner(), 0x10));
        let mut s = STATE.lock().unwrap();
        if s.owned {
            return Status::ALREADY_STARTED;
        }
        s.owned = true;
        unsafe {
            *out = (&raw const PCI).cast_mut().cast();
        }
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn close(_: Handle, guid: *const Guid, _: Handle, _: Handle) -> Status {
    if unsafe { *guid } != LoadedImageProtocol::GUID {
        let mut s = STATE.lock().unwrap();
        assert!(s.owned);
        s.owned = false;
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn install(
    handle: *mut Handle,
    guid: *const Guid,
    kind: InterfaceType,
    interface: *const c_void,
) -> Status {
    assert_eq!(unsafe { *handle }, image());
    assert_eq!(unsafe { *guid }, DriverBindingProtocol::GUID);
    assert_eq!(kind, InterfaceType::NATIVE_INTERFACE);
    STATE.lock().unwrap().binding = interface as usize;
    Status::SUCCESS
}

#[test]
fn binding_owner_success_retry_stop_and_fail_closed() {
    // Fill unused function slots with a non-null function address. They are
    // never invoked. All invoked slots below have their exact ABI/signature.
    // BootServices is a Header followed solely by function/raw pointers.
    let mut raw = MaybeUninit::<BootServices>::uninit();
    unsafe {
        let words = raw.as_mut_ptr().cast::<usize>();
        for i in 0..size_of::<BootServices>() / size_of::<usize>() {
            words.add(i).write(install as *const () as usize);
        }
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(core::mem::zeroed());
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).open_protocol).write(open);
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).close_protocol).write(close);
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).install_protocol_interface).write(install);
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pool).write(free_pool);
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).create_event_ex).write(create_event);
        core::ptr::addr_of_mut!((*raw.as_mut_ptr()).close_event).write(close_event);
    }
    let services = unsafe { raw.assume_init() };
    let mut table: SystemTable = unsafe { core::mem::zeroed() };
    table.boot_services = &services as *const _ as *mut _;
    assert_eq!(unsafe { driver::install(image(), &table) }, Status::SUCCESS);
    assert!(STATE.lock().unwrap().writes.is_empty()); // no hardware writes at entry
    let binding = unsafe { &*(STATE.lock().unwrap().binding as *const DriverBindingProtocol) };
    unsafe {
        assert_eq!((binding.supported)(binding, image(), null()), Status::UNSUPPORTED);
        assert_eq!((binding.supported)(binding, owner(), null()), Status::SUCCESS);
        assert!(!STATE.lock().unwrap().owned);
        assert!(STATE.lock().unwrap().writes.is_empty());
        assert_eq!((binding.start)(binding, owner(), null()), Status::SUCCESS);
        {
            let s = STATE.lock().unwrap();
            assert_eq!(s.writes.len(), 18);
            assert_eq!(s.last[0], 2);
            assert_eq!(s.last[7], 0x00020013);
            assert_eq!(&s.last[4..6], &[0x4d455844, 0x324b5241]);
        }
        assert_eq!((binding.start)(binding, owner(), null()), Status::ALREADY_STARTED);
        assert_eq!(STATE.lock().unwrap().writes.len(), 18);
        assert_eq!((binding.stop)(binding, owner(), 0, null()), Status::SUCCESS);
        // Memory-off boot now succeeds. Preserve both initial BME values and
        // unrelated command bits; hold MSE for callbacks, restore on Stop.
        for command in [0, 4, 2, 6, 0x405] {
            {
                let mut s = STATE.lock().unwrap();
                s.command = command;
                s.writes.clear();
                s.attribute_calls.clear();
            }
            assert_eq!((binding.start)(binding, owner(), null()), Status::SUCCESS);
            {
                let s = STATE.lock().unwrap();
                assert_eq!(s.command, command | 2);
                assert_eq!(s.writes.len(), 18);
                if command & 2 == 0 {
                    assert_eq!(s.attribute_calls, [(4, 0), (2, 0x200)]);
                } else {
                    assert!(s.attribute_calls.is_empty());
                }
            }
            assert_eq!((binding.stop)(binding, owner(), 0, null()), Status::SUCCESS);
            assert_eq!(STATE.lock().unwrap().command, command);
        }
        // Unsupported, partial failed enable, enable without effect, and failed
        // cleanup must never be reported as success. An unexpected BME change
        // from broken firmware is detected before any BAR write.
        for mode in 1..=6 {
            {
                let mut s = STATE.lock().unwrap();
                s.command = 0;
                s.writes.clear();
                s.attribute_calls.clear();
                s.attribute_mode = mode;
            }
            if mode == 4 || mode == 5 {
                assert_eq!((binding.start)(binding, owner(), null()), Status::SUCCESS);
                assert!((binding.stop)(binding, owner(), 0, null()).is_error());
                assert!(STATE.lock().unwrap().owned);
                STATE.lock().unwrap().attribute_mode = 0;
                assert_eq!((binding.stop)(binding, owner(), 0, null()), Status::SUCCESS);
            } else {
                assert!((binding.start)(binding, owner(), null()).is_error());
            }
            let s = STATE.lock().unwrap();
            assert!(!s.owned);
            if mode == 4 || mode == 5 {
                assert_eq!(s.writes.len(), 18);
            } else {
                assert!(s.writes.is_empty());
            }
            if mode != 5 && mode != 6 {
                assert_eq!(s.command, 0);
            }
        }
        STATE.lock().unwrap().attribute_mode = 0;
        // Failure conditions must close ownership and never stage a record.
        for condition in 1..3 {
            {
                let mut s = STATE.lock().unwrap();
                s.writes.clear();
                s.command = 0;
                s.identity = if condition == 1 { 0xffffffff } else { 0x066610ee };
                s.abi = if condition == 2 { 2 } else { 0x10001 };
            }
            assert!((binding.start)(binding, owner(), null()).is_error());
            let s = STATE.lock().unwrap();
            assert!(!s.owned);
            assert!(s.writes.is_empty());
            assert_eq!(s.command, 0);
        }
        {
            let mut s = STATE.lock().unwrap();
            s.abi = 0x10001;
            s.drop_commit = true;
            s.polls = 0;
        }
        assert_eq!((binding.start)(binding, owner(), null()), Status::TIMEOUT);
        {
            let s = STATE.lock().unwrap();
            assert!(!s.owned);
            assert_eq!(s.command, 0);
            assert_eq!(s.writes.len(), 9);
            assert_eq!(s.polls, 1026);
        }
        STATE.lock().unwrap().drop_commit = false;
        // Registration failures release the first event and restore decoding.
        for fail in [0, 1, 2] {
            STATE.lock().unwrap().create_failure = fail;
            assert_eq!((binding.start)(binding, owner(), null()), Status::OUT_OF_RESOURCES);
            let s = STATE.lock().unwrap();
            assert!(!s.owned);
            assert_eq!(s.command, 0);
            assert!(s.events.iter().all(|e| !e.3));
        }
        STATE.lock().unwrap().create_failure = 255;
        assert_eq!((binding.start)(binding, owner(), null()), Status::SUCCESS);
        STATE.lock().unwrap().close_failure = true;
        assert_eq!((binding.stop)(binding, owner(), 0, null()), Status::DEVICE_ERROR);
        let writes = STATE.lock().unwrap().writes.len();
        signal(0); // still-registered callbacks are inactive; no use after Stop
        signal(1);
        signal(2);
        assert_eq!(STATE.lock().unwrap().writes.len(), writes);
        STATE.lock().unwrap().close_failure = false;
        assert_eq!((binding.stop)(binding, owner(), 0, null()), Status::SUCCESS);

        assert_eq!((binding.start)(binding, owner(), null()), Status::SUCCESS);
        let (calls, first_sequence, boot_id) = {
            let s = STATE.lock().unwrap();
            (s.firmware_calls, s.last[0], s.last[1])
        };
        signal(0);
        {
            let s = STATE.lock().unwrap();
            assert_eq!(s.last[7], 0x00040028);
            assert_eq!(s.last[4..6], [1, 0]);
            assert_eq!(s.last[1], boot_id);
        }
        signal(1);
        {
            let s = STATE.lock().unwrap();
            assert_eq!(s.last[7], 0x00040035);
            assert_eq!(s.last[4..6], [0x00010001, 0]);
            assert_eq!(s.firmware_calls, calls);
        }
        signal(2);
        {
            let s = STATE.lock().unwrap();
            assert_eq!(s.last[7], 0x00040040);
            assert_eq!(s.last[4..6], [0x00010001, 1]);
            assert_eq!(s.last[0], first_sequence + 3);
            assert_eq!(s.firmware_calls, calls);
        }
        signal(1); // late AfterReadyToBoot is also ignored
        signal(0); // late ReadyToBoot cannot overwrite ExitBootServices
        assert_eq!(STATE.lock().unwrap().last[7], 0x00040040);
        assert_eq!((binding.stop)(binding, owner(), 0, null()), Status::UNSUPPORTED);
        assert_eq!(STATE.lock().unwrap().firmware_calls, calls);
    }
}
