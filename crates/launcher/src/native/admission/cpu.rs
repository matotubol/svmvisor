//! The PI MP Services protocol as the resident processor inventory
//! (`native::resident::processors`) calls it: GUID, processor record, the seven
//! protocol slots and the blocking AP timeout.

use core::{ffi::c_void, mem::size_of};

use uefi_raw::{Boolean, Event, Guid, Status, guid};

pub const AP_TIMEOUT_MICROSECONDS: usize = 1_000_000;
pub const MP_SERVICES_GUID: Guid = guid!("3fdda605-a76e-4f46-ad29-12f4531b3d08");

/// PI's legacy (non-CPU_V2_EXTENDED_TOPOLOGY) processor record.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessorInformation {
    pub processor_id: u64,
    pub status_flag: u32,
    pub package: u32,
    pub core: u32,
    pub thread: u32,
}

const _: () = assert!(size_of::<ProcessorInformation>() == 24);

pub type ApProcedure = extern "efiapi" fn(*mut c_void);

/// uefi-raw 0.15.1 has no MP Services definition. Keep all seven PI slots in
/// their specified order, using uefi-raw's ABI scalar types.
#[repr(C)]
pub struct MpServicesProtocol {
    pub get_number_of_processors:
        unsafe extern "efiapi" fn(*const Self, *mut usize, *mut usize) -> Status,
    pub get_processor_info:
        unsafe extern "efiapi" fn(*const Self, usize, *mut ProcessorInformation) -> Status,
    pub startup_all_aps: unsafe extern "efiapi" fn(
        *const Self,
        ApProcedure,
        Boolean,
        Event,
        usize,
        *mut c_void,
        *mut *mut usize,
    ) -> Status,
    pub startup_this_ap: unsafe extern "efiapi" fn(
        *const Self,
        ApProcedure,
        usize,
        Event,
        usize,
        *mut c_void,
        *mut Boolean,
    ) -> Status,
    pub switch_bsp: unsafe extern "efiapi" fn(*const Self, usize, Boolean) -> Status,
    pub enable_disable_ap:
        unsafe extern "efiapi" fn(*const Self, usize, Boolean, *const u32) -> Status,
    pub who_am_i: unsafe extern "efiapi" fn(*const Self, *mut usize) -> Status,
}

const _: () = assert!(size_of::<MpServicesProtocol>() == 7 * size_of::<usize>());
