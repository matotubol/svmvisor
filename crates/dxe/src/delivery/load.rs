//! Opt-in load-only diagnostic. All data stays in LoaderData allocations,
//! is freed before successful Start, and is never called or handed off.
use crate::pci_io::{status_result, Bar0};
use core::ptr::{null_mut, slice_from_raw_parts_mut};
use svmvisor_dxe::{
    card::{self, Manifest},
    journal::{self, JournalIo},
};
use svmvisor_firmware_handoff::layout::ARENA_BYTES;
use uefi_raw::{
    table::boot::{AllocateType, BootServices, MemoryType},
    Status,
};

// Retain only failed-free ownership, allowing driver cleanup/Stop to retry.
static mut POOL: *mut u8 = null_mut();
static mut ARENA: Option<u64> = None;

pub(crate) fn cleanup(services: &BootServices) -> Result<(), Status> {
    let arena = unsafe { ARENA };
    if let Some(base) = arena {
        status_result(unsafe { (services.free_pages)(base, ARENA_BYTES / 4096) })?;
        unsafe {
            ARENA = None;
        }
    }
    let pool = unsafe { POOL };
    if !pool.is_null() {
        status_result(unsafe { (services.free_pool)(pool) })?;
        unsafe {
            POOL = null_mut();
        }
    }
    Ok(())
}

fn stage(io: &Bar0, services: &BootServices, pinned: &str) -> Result<(), Status> {
    let existing_arena = unsafe { ARENA };
    if unsafe { !POOL.is_null() } || existing_arena.is_some() {
        return Err(Status::NOT_READY);
    }
    let pin = card::parse_pin(pinned).map_err(|_| Status::INVALID_PARAMETER)?;
    let mut header = [0u8; card::HEADER_BYTES];
    for (i, chunk) in header.chunks_exact_mut(4).enumerate() {
        chunk.copy_from_slice(&io.card_word((i * 4) as u64)?.to_le_bytes());
    }
    let manifest = Manifest::parse(&header, &pin).map_err(|_| Status::COMPROMISED_DATA)?;
    let rounded = (manifest.package_bytes() + 3) & !3;
    let mut pool: *mut u8 = null_mut();
    status_result(unsafe {
        (services.allocate_pool)(MemoryType::LOADER_DATA, rounded, &mut pool)
    })?;
    if pool.is_null() {
        return Err(Status::DEVICE_ERROR);
    }
    unsafe {
        POOL = pool.cast();
    }
    // Exact allocated slice; manifest bounds make every DWORD remain in BAR1.
    let bytes = unsafe { &mut *slice_from_raw_parts_mut(pool.cast::<u8>(), rounded) };
    for (i, chunk) in bytes.chunks_exact_mut(4).enumerate() {
        chunk.copy_from_slice(
            &io.card_word((card::HEADER_BYTES + i * 4) as u64)?
                .to_le_bytes(),
        );
    }
    let package = manifest
        .package(&bytes[..manifest.package_bytes()])
        .map_err(|_| Status::COMPROMISED_DATA)?;
    let mut selected = None;
    for index in 1..=128u64 {
        let mut address = index * 0x200000;
        let status = unsafe {
            (services.allocate_pages)(
                AllocateType::ADDRESS,
                MemoryType::LOADER_DATA,
                ARENA_BYTES / 4096,
                &mut address,
            )
        };
        if !status.is_error() {
            unsafe {
                ARENA = Some(address);
            }
            if address != index * 0x200000
                || !svmvisor_firmware_handoff::layout::valid_arena(address)
            {
                return Err(Status::DEVICE_ERROR);
            }
            selected = Some(address);
            break;
        }
    }
    let address = selected.ok_or(Status::OUT_OF_RESOURCES)?;
    unsafe {
        ARENA = Some(address);
    }
    // No executable mapping, function-pointer conversion or transfer occurs.
    let arena = unsafe { &mut *slice_from_raw_parts_mut(address as *mut u8, ARENA_BYTES) };
    package
        .load(arena, address)
        .map_err(|_| Status::COMPROMISED_DATA)
}

pub(crate) fn verify(
    io: &mut Bar0,
    services: &BootServices,
    boot_id: u32,
    tsc: u64,
    cpu: u32,
) -> Result<(), Status> {
    verify_with_pin(
        io,
        services,
        boot_id,
        tsc,
        cpu,
        option_env!("SVMVISOR_CARD_PAYLOAD_SHA256").unwrap_or(""),
    )
}

pub(crate) fn verify_with_pin(
    io: &mut Bar0,
    services: &BootServices,
    boot_id: u32,
    tsc: u64,
    cpu: u32,
    pinned: &str,
) -> Result<(), Status> {
    let staged = stage(io, services, pinned);
    // Success means all ownership returned as well as package validation/load.
    let result = cleanup(services).and(staged);
    let phase = if result.is_ok() {
        card::JOURNAL_SUCCESS
    } else {
        card::JOURNAL_FAILURE
    };
    let sequence = io.read(0x02c)?.wrapping_add(1);
    // ASCII CARDLOAD; distinct detail5 from default DXEMARK2 and lifecycle.
    journal::commit(
        io,
        [
            sequence,
            boot_id,
            tsc as u32,
            (tsc >> 32) as u32,
            0x44524143,
            0x44414f4c,
            cpu,
            phase,
        ],
    )?;
    result
}
