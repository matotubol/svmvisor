//! Memory attribute protocol lookup and current-permission queries.

use core::ptr::{self, NonNull};

use uefi_raw::{
    Status,
    protocol::memory_protection::MemoryAttributeProtocol,
    table::boot::{BootServices, MemoryAttribute},
};

use super::{AttributeLookupFailure, BorrowedAccess};

/// The real interface is always preferred. Only exact EFI_NOT_FOUND can select
/// the explicitly enabled F7 compatibility reader; malformed SUCCESS pointers,
/// warnings and other failures keep their original lookup diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttributeSource {
    Firmware(NonNull<MemoryAttributeProtocol>),
    #[cfg(feature = "memory-attribute-f7")]
    F7,
}

/// Perform exactly one raw lookup, accepting only SUCCESS, non-NULL and aligned.
/// A returned pointer on non-SUCCESS is ignored, without dereferencing it.
///
/// # Safety
/// Live conforming UEFI x64 Boot Services, at TPL <= NOTIFY. The caller retains
/// the protocol's firmware lifetime contract before using the resulting pointer.
pub(super) unsafe fn acquire_memory_attributes(
    services: &BootServices,
) -> Result<NonNull<MemoryAttributeProtocol>, AttributeLookupFailure> {
    // The persisted interface remainder is defined for the UEFI x64 ABI.
    const _: () = assert!(core::mem::align_of::<MemoryAttributeProtocol>() == 8);
    const _: () = assert!(usize::BITS == 64);
    let mut interface = ptr::null_mut();
    let status = unsafe {
        (services.locate_protocol)(&MemoryAttributeProtocol::GUID, ptr::null_mut(), &mut interface)
    };
    if status != Status::SUCCESS {
        return Err(AttributeLookupFailure::NonSuccess(status));
    }
    let interface = NonNull::new(interface.cast::<MemoryAttributeProtocol>())
        .ok_or(AttributeLookupFailure::SuccessNull)?;
    let remainder = interface.as_ptr().addr() % core::mem::align_of::<MemoryAttributeProtocol>();
    if remainder != 0 {
        return Err(AttributeLookupFailure::SuccessUnaligned { remainder: remainder as u8 });
    }
    Ok(interface)
}

pub(super) fn select_attribute_source(
    lookup: Result<NonNull<MemoryAttributeProtocol>, AttributeLookupFailure>,
) -> Result<AttributeSource, AttributeLookupFailure> {
    match lookup {
        Ok(interface) => Ok(AttributeSource::Firmware(interface)),
        #[cfg(feature = "memory-attribute-f7")]
        Err(AttributeLookupFailure::NonSuccess(Status::NOT_FOUND)) => Ok(AttributeSource::F7),
        Err(error) => Err(error),
    }
}

pub(super) unsafe fn current_access(
    protocol: &MemoryAttributeProtocol,
    address: u64,
    access: BorrowedAccess,
) -> bool {
    let mut attributes = MemoryAttribute::empty();
    let status = unsafe {
        (protocol.get_memory_attributes)(protocol, address & !4095, 4096, &mut attributes)
    };
    attributes_allow(status, attributes, access)
}

#[cfg(any(feature = "memory-attribute-f7", test))]
pub(super) fn internal_access<E>(
    query: Result<u64, E>,
    access: BorrowedAccess,
    failed: &core::cell::Cell<bool>,
) -> bool {
    match query {
        Ok(attributes) => {
            attributes_allow(Status::SUCCESS, MemoryAttribute::from_bits_retain(attributes), access)
        }
        Err(_) => {
            failed.set(true);
            false
        }
    }
}

pub(super) fn attributes_allow(
    status: Status,
    attributes: MemoryAttribute,
    access: BorrowedAccess,
) -> bool {
    status == Status::SUCCESS
        && !attributes.contains(MemoryAttribute::READ_PROTECT)
        && (access != BorrowedAccess::ReadWrite || attributes.bits() & 0x20000 == 0)
        && (access != BorrowedAccess::ReadExecute
            || !attributes.contains(MemoryAttribute::EXECUTE_PROTECT))
}
