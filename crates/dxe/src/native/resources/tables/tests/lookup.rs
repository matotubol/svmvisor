use super::super::*;

use core::{ffi::c_void, mem::MaybeUninit};
use std::cell::RefCell;

use uefi_raw::{
    Guid,
    protocol::memory_protection::MemoryAttributeProtocol,
    table::boot::{MemoryAttribute, MemoryType, Tpl},
};

use super::super::attribute::{
    AttributeSource, acquire_memory_attributes, current_access, select_attribute_source,
};

struct State {
    status: Status,
    output: Option<usize>,
    tpl: Tpl,
    calls: Vec<&'static str>,
    unaligned_allocation: bool,
    free_failures: usize,
}
thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State {
        status: Status::SUCCESS,
        output: None,
        tpl: Tpl::NOTIFY,
        calls: Vec::new(),
        unaligned_allocation: false,
        free_failures: 0,
    }) };
}
fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|state| f(&mut state.borrow_mut()))
}
fn setup(status: Status, output: Option<usize>) -> BootServices {
    with(|state| {
        *state = State {
            status,
            output,
            tpl: Tpl::NOTIFY,
            calls: Vec::new(),
            unaligned_allocation: false,
            free_failures: 0,
        };
    });
    let mut raw = MaybeUninit::<BootServices>::uninit();
    unsafe {
        // Match the existing native_cpu host fixture: every unused service
        // slot has a non-NULL address and must never be invoked.
        for index in 0..size_of::<BootServices>() / size_of::<usize>() {
            raw.as_mut_ptr().cast::<usize>().add(index).write(unused as *const () as usize);
        }
        ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(core::mem::zeroed());
        ptr::addr_of_mut!((*raw.as_mut_ptr()).raise_tpl).write(raise_tpl);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).restore_tpl).write(restore_tpl);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).locate_protocol).write(locate);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).allocate_pool).write(allocate_pool);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).free_pool).write(free_pool);
        raw.assume_init()
    }
}
unsafe extern "efiapi" fn unused() {
    panic!("unexpected firmware service in lookup test")
}
unsafe extern "efiapi" fn raise_tpl(tpl: Tpl) -> Tpl {
    with(|state| {
        assert_eq!(tpl, Tpl::HIGH_LEVEL);
        state.calls.push("raise");
        let old = state.tpl;
        state.tpl = tpl;
        old
    })
}
unsafe extern "efiapi" fn restore_tpl(tpl: Tpl) {
    with(|state| {
        assert_eq!(state.tpl, Tpl::HIGH_LEVEL);
        state.calls.push("restore");
        state.tpl = tpl;
    });
}
unsafe extern "efiapi" fn locate(
    guid: *const Guid,
    registration: *mut c_void,
    output: *mut *mut c_void,
) -> Status {
    // Independent literal, not the same symbol passed by the helper.
    assert_eq!(unsafe { *guid }, uefi_raw::guid!("f4560cf6-40ec-4b4a-a192-bf1d57d0b189"));
    assert!(registration.is_null());
    assert!(!output.is_null());
    assert!(unsafe { *output }.is_null());
    with(|state| {
        assert!(state.tpl == Tpl::APPLICATION || state.tpl == Tpl::NOTIFY);
        state.calls.push("locate");
        if let Some(address) = state.output {
            unsafe { *output = address as *mut c_void };
        }
        state.status
    })
}
unsafe extern "efiapi" fn allocate_pool(
    kind: MemoryType,
    bytes: usize,
    output: *mut *mut u8,
) -> Status {
    assert_eq!(kind, MemoryType::BOOT_SERVICES_DATA);
    assert_eq!(bytes, size_of::<TableStorage>());
    assert!(unsafe { *output }.is_null());
    with(|state| {
        state.calls.push("allocate");
        if state.unaligned_allocation {
            // No backing allocation is needed: this sentinel is rejected
            // before writes and the fixture free only records its address.
            unsafe { *output = 1usize as *mut u8 };
            Status::SUCCESS
        } else {
            Status::OUT_OF_RESOURCES
        }
    })
}
unsafe extern "efiapi" fn free_pool(pointer: *mut u8) -> Status {
    assert_eq!(pointer.addr(), 1);
    with(|state| {
        state.calls.push("free");
        if state.free_failures != 0 {
            state.free_failures -= 1;
            Status::DEVICE_ERROR
        } else {
            Status::SUCCESS
        }
    })
}
unsafe extern "efiapi" fn get_attributes(
    this: *const MemoryAttributeProtocol,
    base: u64,
    bytes: u64,
    attributes: *mut MemoryAttribute,
) -> Status {
    assert_eq!(this, &PROTOCOL);
    assert_eq!(base, 0x9000);
    assert_eq!(bytes, 4096);
    assert_eq!(unsafe { *attributes }, MemoryAttribute::empty());
    with(|state| state.calls.push("get"));
    Status::SUCCESS
}
unsafe extern "efiapi" fn change_attributes(
    _: *const MemoryAttributeProtocol,
    _: u64,
    _: u64,
    _: MemoryAttribute,
) -> Status {
    panic!("lookup must not change attributes")
}
static PROTOCOL: MemoryAttributeProtocol = MemoryAttributeProtocol {
    get_memory_attributes: get_attributes,
    set_memory_attributes: change_attributes,
    clear_memory_attributes: change_attributes,
};
fn protocol_address() -> usize {
    (&PROTOCOL as *const MemoryAttributeProtocol).addr()
}
fn failed_preparation(status: Status, output: Option<usize>) -> TableFailure {
    let services = setup(status, output);
    let failure = unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
        .err()
        .expect("lookup must refuse");
    with(|state| {
        assert_eq!(state.calls, ["raise", "restore", "locate"]);
        assert_eq!(state.tpl, Tpl::NOTIFY);
    });
    assert_eq!(failure.kind, TableError::AttributeProtocol);
    assert_eq!(failure.resource_code() & 0xffff, 0x101);
    failure
}

#[test]
fn actual_efiapi_lookup_accepts_aligned_success_without_allocation_or_query() {
    for tpl in [Tpl::APPLICATION, Tpl::NOTIFY] {
        let services = setup(Status::SUCCESS, Some(protocol_address()));
        with(|state| state.tpl = tpl);
        let pointer = unsafe { acquire_memory_attributes(&services) }.unwrap();
        assert_eq!(pointer.as_ptr().addr(), protocol_address());
        with(|state| assert_eq!(state.calls, ["locate"]));
        assert!(unsafe { current_access(pointer.as_ref(), 0x9123, BorrowedAccess::ReadWrite) });
        with(|state| assert_eq!(state.calls, ["locate", "get"]));
    }
}

#[test]
fn every_standard_error_and_warning_preserves_exact_status_before_pointer_checks() {
    let errors = [
        (Status::LOAD_ERROR, 1),
        (Status::INVALID_PARAMETER, 2),
        (Status::UNSUPPORTED, 3),
        (Status::BAD_BUFFER_SIZE, 4),
        (Status::BUFFER_TOO_SMALL, 5),
        (Status::NOT_READY, 6),
        (Status::DEVICE_ERROR, 7),
        (Status::WRITE_PROTECTED, 8),
        (Status::OUT_OF_RESOURCES, 9),
        (Status::VOLUME_CORRUPTED, 10),
        (Status::VOLUME_FULL, 11),
        (Status::NO_MEDIA, 12),
        (Status::MEDIA_CHANGED, 13),
        (Status::NOT_FOUND, 14),
        (Status::ACCESS_DENIED, 15),
        (Status::NO_RESPONSE, 16),
        (Status::NO_MAPPING, 17),
        (Status::TIMEOUT, 18),
        (Status::NOT_STARTED, 19),
        (Status::ALREADY_STARTED, 20),
        (Status::ABORTED, 21),
        (Status::ICMP_ERROR, 22),
        (Status::TFTP_ERROR, 23),
        (Status::PROTOCOL_ERROR, 24),
        (Status::INCOMPATIBLE_VERSION, 25),
        (Status::SECURITY_VIOLATION, 26),
        (Status::CRC_ERROR, 27),
        (Status::END_OF_MEDIA, 28),
        (Status::END_OF_FILE, 31),
        (Status::INVALID_LANGUAGE, 32),
        (Status::COMPROMISED_DATA, 33),
        (Status::IP_ADDRESS_CONFLICT, 34),
        (Status::HTTP_ERROR, 35),
    ];
    let warnings = [
        (Status::WARN_UNKNOWN_GLYPH, 1),
        (Status::WARN_DELETE_FAILURE, 2),
        (Status::WARN_WRITE_FAILURE, 3),
        (Status::WARN_BUFFER_TOO_SMALL, 4),
        (Status::WARN_STALE_DATA, 5),
        (Status::WARN_FILE_SYSTEM, 6),
        (Status::WARN_RESET_REQUIRED, 7),
    ];
    for (cases, prefix) in [(&errors[..], 0x1800_4101u64), (&warnings[..], 0x1000_4101)] {
        for &(status, code) in cases {
            if cfg!(feature = "memory-attribute-f7") && status == Status::NOT_FOUND {
                continue; // Exact fallback selection has dedicated tests below.
            }
            // Valid, NULL, unaligned and deliberately unusable aligned
            // outputs must all be ignored when status is non-SUCCESS.
            for output in
                [None, Some(0), Some(protocol_address()), Some(1), Some(8), Some(usize::MAX)]
            {
                let failure = failed_preparation(status, output);
                assert_eq!(failure.lookup, Some(AttributeLookupFailure::NonSuccess(status)));
                assert_eq!(0x4000 | failure.resource_code(), prefix | (code << 16));
            }
        }
    }
}

#[test]
fn success_null_and_all_unaligned_remainders_are_distinct_refusals() {
    for output in [None, Some(0)] {
        let failure = failed_preparation(Status::SUCCESS, output);
        assert_eq!(failure.lookup, Some(AttributeLookupFailure::SuccessNull));
        assert_eq!(0x4000 | failure.resource_code(), 0x2000_4101);
    }
    for remainder in 1..=7u8 {
        let failure =
            failed_preparation(Status::SUCCESS, Some(protocol_address() + usize::from(remainder)));
        assert_eq!(failure.lookup, Some(AttributeLookupFailure::SuccessUnaligned { remainder }));
        assert_eq!(0x4000 | failure.resource_code(), 0x3000_4101 | (u64::from(remainder) << 16));
    }
}

#[test]
fn implementation_status_bits_are_loss_marked_and_cannot_alias_exact_not_found() {
    for error in [0, 1usize << 63] {
        for code in [0usize, 14, 0x3ff] {
            let raw = error | code;
            if raw != 0
                && !(cfg!(feature = "memory-attribute-f7") && Status(raw) == Status::NOT_FOUND)
            {
                let exact = failed_preparation(Status(raw), Some(1)).resource_code();
                assert_eq!(exact & (1 << 26), 0);
                assert_eq!((exact >> 16) & 0x3ff, code as u64);
                assert_eq!(exact & (1 << 27) != 0, error != 0);
            }
            for bit in 10..63 {
                let raw = raw | (1usize << bit);
                let failure = failed_preparation(Status(raw), Some(protocol_address()));
                assert_eq!(failure.lookup, Some(AttributeLookupFailure::NonSuccess(Status(raw))));
                let encoded = 0x4000 | failure.resource_code();
                assert!(encoded <= u32::MAX as u64);
                assert_eq!(encoded >> 28, 1);
                assert_eq!(encoded & (1 << 26), 1 << 26);
                assert_eq!((encoded >> 16) & 0x3ff, code as u64);
                assert_eq!(encoded & (1 << 27) != 0, error != 0);
                assert_ne!(encoded, 0x180e_4101);
            }
        }
    }
    let incomplete = failed_preparation(Status(0x8000_0000_0000_040e), None);
    assert_eq!(0x4000 | incomplete.resource_code(), 0x1c0e_4101);
    let all_bits = failed_preparation(Status(usize::MAX), Some(1));
    assert_eq!(0x4000 | all_bits.resource_code(), 0x1fff_4101);
}

#[test]
fn legacy_wrappers_keep_kind_and_original_acquisition_family() {
    for (status, output) in [
        (Status::NOT_FOUND, Some(protocol_address())),
        (Status::SUCCESS, None),
        (Status::SUCCESS, Some(1)),
    ] {
        if cfg!(feature = "memory-attribute-f7") && status == Status::NOT_FOUND {
            continue;
        }
        let services = setup(status, output);
        assert_eq!(
            unsafe { prepare(&services, 48, true) }.err(),
            Some(TableError::AttributeProtocol)
        );
        let services = setup(status, output);
        assert_eq!(
            unsafe { prepare_owned_ranges(&services, 48, true, &[]) }.err(),
            Some(TableError::AttributeProtocol)
        );
        let services = setup(status, output);
        assert_eq!(
            unsafe { prepare_resource_ranges(&services, 48, true, &[], &[]) }.err(),
            Some(TableError::AttributeProtocol)
        );
    }
    let legacy = TableFailure::from(TableError::AttributeProtocol);
    assert_eq!(legacy.lookup, None);
    assert_eq!(0x4000 | legacy.resource_code(), 0x0000_4101);
}

#[test]
fn validation_and_tpl_refuse_before_lookup_with_unchanged_codes() {
    let services = setup(Status::NOT_FOUND, None);
    let failure = unsafe {
        prepare_resource_ranges_detailed(
            &services,
            48,
            true,
            &[OwnedRange { base: 1, bytes: 4096 }],
            &[],
        )
    }
    .err()
    .unwrap();
    assert_eq!(failure, TableFailure::from(TableError::OwnedRange));
    assert_eq!(failure.resource_code(), 0x111);
    with(|state| assert!(state.calls.is_empty()));
    let services = setup(Status::NOT_FOUND, None);
    with(|state| state.tpl = Tpl::CALLBACK);
    let failure =
        unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }.err().unwrap();
    assert_eq!(failure, TableFailure::from(TableError::EntryTpl));
    assert_eq!(failure.resource_code(), 0x110);
    with(|state| {
        assert_eq!(state.calls, ["raise", "restore"]);
        assert_eq!(state.tpl, Tpl::CALLBACK);
    });
}

#[test]
fn later_allocation_and_cleanup_failure_never_inherit_lookup_tags() {
    let services = setup(Status::SUCCESS, Some(protocol_address()));
    let failure =
        unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }.err().unwrap();
    assert_eq!(failure, TableFailure::from(TableError::Allocation));
    assert_eq!(failure.resource_code(), 0x102);
    with(|state| assert_eq!(state.calls, ["raise", "restore", "locate", "allocate"]));
    for free_failures in [0, 1] {
        let services = setup(Status::SUCCESS, Some(protocol_address()));
        with(|state| {
            state.unaligned_allocation = true;
            state.free_failures = free_failures;
        });
        let failure = unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
            .err()
            .unwrap();
        if free_failures == 0 {
            assert_eq!(failure, TableFailure::from(TableError::Allocation));
            assert_eq!(failure.resource_code(), 0x102);
            with(|state| {
                assert_eq!(state.calls, ["raise", "restore", "locate", "allocate", "free"])
            });
        } else {
            assert_eq!(failure, TableFailure::from(TableError::Cleanup));
            assert_eq!(failure.resource_code(), 0x10d);
            // The explicit first free failed. A later successful Drop must
            // never turn that observation into reported complete cleanup.
            with(|state| {
                assert_eq!(state.calls, ["raise", "restore", "locate", "allocate", "free", "free"])
            });
        }
    }
    let cleanup = TableFailure {
        kind: TableError::Cleanup,
        lookup: Some(AttributeLookupFailure::NonSuccess(Status::NOT_FOUND)),
        fallback_reason: Some(0x1234),
    };
    assert_eq!(cleanup.resource_code(), 0x10d);
}

#[test]
fn source_selection_preserves_firmware_and_only_accepts_exact_missing_status() {
    let interface = NonNull::new(protocol_address() as *mut MemoryAttributeProtocol).unwrap();
    assert_eq!(select_attribute_source(Ok(interface)), Ok(AttributeSource::Firmware(interface)));
    for failure in [
        AttributeLookupFailure::SuccessNull,
        AttributeLookupFailure::SuccessUnaligned { remainder: 1 },
        AttributeLookupFailure::NonSuccess(Status::UNSUPPORTED),
        AttributeLookupFailure::NonSuccess(Status::ACCESS_DENIED),
        AttributeLookupFailure::NonSuccess(Status::DEVICE_ERROR),
        AttributeLookupFailure::NonSuccess(Status(14)),
        AttributeLookupFailure::NonSuccess(Status(Status::NOT_FOUND.0 | 0x400)),
    ] {
        assert_eq!(select_attribute_source(Err(failure)), Err(failure));
    }
    let missing = AttributeLookupFailure::NonSuccess(Status::NOT_FOUND);
    #[cfg(feature = "memory-attribute-f7")]
    assert_eq!(select_attribute_source(Err(missing)), Ok(AttributeSource::F7));
    #[cfg(not(feature = "memory-attribute-f7"))]
    assert_eq!(select_attribute_source(Err(missing)), Err(missing));
}

#[test]
#[cfg(feature = "memory-attribute-f7")]
fn exact_not_found_fallback_reaches_allocation_without_reading_returned_pointer() {
    for output in [None, Some(0), Some(1), Some(8), Some(usize::MAX), Some(protocol_address())] {
        let services = setup(Status::NOT_FOUND, output);
        let failure = unsafe { prepare_resource_ranges_detailed(&services, 48, true, &[], &[]) }
            .err()
            .unwrap();
        assert_eq!(failure, TableFailure::from(TableError::Allocation));
        with(|state| assert_eq!(state.calls, ["raise", "restore", "locate", "allocate"]));
    }
}

#[test]
fn fallback_failures_have_distinct_codes_without_corrupting_lookup_diagnostics() {
    let acquisition = TableFailure::from(TableError::AttributeFallback);
    let reader = TableFailure::from(TableError::AttributeFallbackRead);
    assert_eq!(0x4000 | acquisition.resource_code(), 0x4115);
    assert_eq!(0x4000 | reader.resource_code(), 0x4116);
    assert_eq!(acquisition.lookup, None);
    assert_eq!(reader.lookup, None);
    for reason in [1u16, 16, 24, 0xfffe, 0xffff] {
        for (kind, low) in [
            (TableError::AttributeFallback, 0x4115u64),
            (TableError::AttributeFallbackRead, 0x4116u64),
        ] {
            let detailed = TableFailure::fallback(kind, reason);
            let code = 0x4000 | detailed.resource_code();
            assert_eq!(code & 0xffff, low);
            assert_eq!(code >> 16, u64::from(reason));
            assert_eq!(detailed.lookup, None);
            assert_ne!(code & 0xffff, 0x4101);
        }
    }
    let missing = TableFailure::from(AttributeLookupFailure::NonSuccess(Status::NOT_FOUND));
    assert_eq!(0x4000 | missing.resource_code(), 0x180e4101);
}
