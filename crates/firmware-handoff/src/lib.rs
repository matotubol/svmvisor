//! Shared emulator-only firmware-to-private-payload handoff.
//! The exact image LoadOptions token is an explicit fixture opt-in, not proof
//! of emulator identity or a security boundary against a malicious loader.

#![no_std]

use core::arch::asm;

use svmvisor_card_abi::package::{ARENA_BYTES, HANDOFF_OFFSET, Payload, is_valid_arena};
use uefi::{
    Handle, Status,
    boot::{self, AllocateType, MemoryType},
    proto::loaded_image::LoadedImage,
};

mod ownership;
mod smp;

pub const AUTHORIZATION: &[u8] = b"SVMVISOR-EMULATOR-HANDOFF-V1";

/// Initialize uefi-rs globals for an image with a raw EFI entry point and run.
///
/// # Safety
/// `image` and `table` must be this image's valid firmware entry arguments.
/// Call once before using any other uefi-rs API in this image. Boot services
/// must be active. The additional requirements of `run_initialized` apply.
pub unsafe fn run(
    image: uefi_raw::Handle,
    table: *const uefi_raw::table::system::SystemTable,
    payload: &[u8],
    entry_offset: usize,
    reject: bool,
) -> uefi_raw::Status {
    if table.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let Some(handle) = (unsafe { Handle::from_ptr(image) }) else {
        return Status::INVALID_PARAMETER;
    };
    unsafe {
        uefi::table::set_system_table(table);
        boot::set_image_handle(handle);
        run_initialized(payload, entry_offset, reject)
    }
}

/// Check fixture authorization, reserve/copy the payload, exit boot services
/// and transfer control. Failure before ExitBootServices returns a status.
/// `reject` deliberately corrupts the handoff magic for the negative fixture.
///
/// # Safety
/// uefi-rs globals must belong to the current, running emulator image. Its
/// LoadOptions backing must be valid. The payload must be the reviewed relocatable
/// package with entry's SysV64 contract accepting the header
/// pointer, and all memory within its declared arena. It must install its private stack
/// and mappings, never call firmware and never return. The emulator must
/// identity-map the allocated arena. The caller must release all protocol,
/// event, pool and other firmware-owned resources before calling; it must not
/// retain a callback that could execute after this handoff.
pub unsafe fn run_initialized(payload: &[u8], entry_offset: usize, reject: bool) -> Status {
    // The protocol guard and borrowed options end before payload allocation.
    let allowed = match boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()) {
        Ok(image) => authorized(image.load_options_as_bytes()),
        Err(_) => false,
    };
    if !allowed {
        debug("handoff-not-authorized\n");
        return Status::ACCESS_DENIED;
    }
    let package = match Payload::parse(payload, entry_offset) {
        Ok(package) => package,
        Err(_) => {
            debug("FAIL uefi-payload-layout\n");
            return Status::INVALID_PARAMETER;
        }
    };
    let requested: u64 = env!("SVMVISOR_SELECTED_ARENA_BASE").parse().unwrap();
    if requested != 0 && !is_valid_arena(requested) {
        debug("FAIL uefi-arena-layout\n");
        return Status::INVALID_PARAMETER;
    }
    // Bound allocation work to 128 independent exact ownership requests. The
    // first available arena is chosen; no dependence on the legacy 1MiB hole.
    let mut allocation = None;
    let mut last_status = Status::OUT_OF_RESOURCES;
    for index in 1..=128_u64 {
        let base = if requested == 0 { index * 0x200000 } else { requested };
        match boot::allocate_pages(
            AllocateType::Address(base),
            MemoryType::LOADER_CODE,
            ARENA_BYTES / 4096,
        ) {
            Ok(pages) => {
                allocation = Some(pages);
                break;
            }
            Err(error) => last_status = error.status(),
        }
        if requested != 0 {
            break;
        }
    }
    let Some(allocation) = allocation else {
        debug("FAIL uefi-payload-allocation\n");
        return last_status;
    };
    let base = allocation.as_ptr() as u64;
    let arena = unsafe { core::slice::from_raw_parts_mut(allocation.as_ptr(), ARENA_BYTES) };
    if package.load(arena, base).is_err() {
        unsafe {
            let _ = boot::free_pages(allocation, ARENA_BYTES / 4096);
        }
        debug("FAIL uefi-arena-layout\n");
        return Status::INVALID_PARAMETER;
    }
    let handoff = unsafe { allocation.as_ptr().add(HANDOFF_OFFSET) };
    let prepared_smp = if env!("SVMVISOR_SELECTED_UEFI_SMP") == "1" {
        match unsafe { smp::prepare() } {
            Ok(prepared) => Some(prepared),
            Err(status) => {
                unsafe {
                    let _ = boot::free_pages(allocation, ARENA_BYTES / 4096);
                }
                debug("FAIL uefi-smp-preparation\n");
                return status;
            }
        }
    } else {
        None
    };
    let smp_resources = prepared_smp.as_ref().map(smp::PreparedSmp::resources);
    if smp_resources.is_some() {
        // Admission uses a preliminary snapshot, never presented as the final
        // EBS map. Drop its pool owner before EBS and overwrite the record after
        // success. Failure releases both exact page allocations while legal.
        let admitted = match boot::memory_map(MemoryType::LOADER_DATA) {
            Ok(map) => {
                debug_smp_map(&map, base, smp_resources.unwrap());
                match ownership::retain_detailed(
                    &map,
                    unsafe { core::slice::from_raw_parts_mut(handoff, 4096) },
                    base,
                    smp_resources,
                ) {
                    Ok(()) => true,
                    Err(error) => {
                        debug_retain_error(error);
                        false
                    }
                }
            }
            Err(error) => {
                debug("uefi-smp-map-acquisition-error=0x");
                debug_hex(error.status().0 as u64);
                debug("\n");
                false
            }
        };
        if !admitted {
            if let Some(prepared) = prepared_smp {
                unsafe {
                    prepared.release();
                }
            }
            unsafe {
                let _ = boot::free_pages(allocation, ARENA_BYTES / 4096);
            }
            debug("FAIL uefi-smp-pre-ebs-ownership\n");
            return Status::UNSUPPORTED;
        }
        debug("uefi-smp-pre-ebs-admitted\n");
    }
    debug("uefi-payload-base=0x");
    debug_hex(base);
    debug("\n");
    debug("uefi-payload-reserved\n");
    if reject {
        unsafe {
            handoff.write(b'X');
        }
        debug("uefi-handoff-corrupted-for-test\n");
    }
    // Retain the final map through the nonreturning transfer. uefi-rs obtains
    // a fresh map key and retries ExitBootServices when needed.
    let final_memory_map = unsafe { boot::exit_boot_services(None) };
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
    debug("uefi-boot-services-exited\n");
    // Ownership commits only from the map returned by successful EBS. No
    // firmware allocation/free/status return is permitted beyond this point.
    let page = unsafe { core::slice::from_raw_parts_mut(handoff, 4096) };
    if let Some(resources) = smp_resources {
        debug_smp_map(&final_memory_map, base, resources);
    }
    let retained = ownership::retain_detailed(&final_memory_map, page, base, smp_resources);
    if let Err(error) = retained {
        debug_retain_error(error);
        debug("FAIL uefi-resident-ownership\n");
        panic_fail();
    }
    if smp_resources.is_some() {
        debug("uefi-smp-post-ebs-owned\n");
    }
    if env!("SVMVISOR_SELECTED_REJECT_SMP_RECORD") == "1" {
        // Negative fixture: leave every final-map descriptor intact and omit
        // mandatory SMP metadata from the declared v2 record. This is malformed
        // v2 (not a valid v1 map); the resident consumer must refuse decoding.
        use svmvisor_hypervisor::boot::ownership::{
            OWNERSHIP_HEADER_BYTES, OWNERSHIP_OFFSET, OWNERSHIP_SMP_HEADER_BYTES,
        };
        page[OWNERSHIP_OFFSET + OWNERSHIP_HEADER_BYTES
            ..OWNERSHIP_OFFSET + OWNERSHIP_SMP_HEADER_BYTES]
            .fill(0);
        debug("uefi-smp-record-omitted-for-test\n");
    }
    if env!("SVMVISOR_SELECTED_REJECT_RESIDENT_OWNERSHIP") == "1" {
        page[64] ^= 1;
        debug("uefi-resident-ownership-corrupted-for-test\n");
    }
    let entry: unsafe extern "sysv64" fn(*const u8) -> ! =
        unsafe { core::mem::transmute(base as usize + entry_offset) };
    unsafe { entry(handoff.cast_const()) }
}

/// Exact binary ASCII match: no terminator, whitespace or UTF-16 encoding.
pub fn authorized(options: Option<&[u8]>) -> bool {
    options == Some(AUTHORIZATION)
}

/// The image ends before the reserved handoff page and entry lies in its bytes.
pub const fn is_valid_payload(payload_bytes: usize, entry_offset: usize) -> bool {
    payload_bytes != 0 && payload_bytes <= HANDOFF_OFFSET && entry_offset < payload_bytes
}

/// Terminal emulator panic path; it performs no firmware calls.
pub fn panic_fail() -> ! {
    debug("FAIL firmware-handoff\n");
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4_u16, in("eax") 17_u32, options(nomem, nostack));
    }
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

/// Emulator debugcon output. Call only in the emulator's privileged firmware.
pub fn debug(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0xe9_u16, in("al") byte, options(nomem, nostack));
        }
    }
}

fn debug_retain_error(error: ownership::RetainError) {
    use core::fmt::Write;
    struct BoundedDebug(usize);
    impl Write for BoundedDebug {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            if text.len() > self.0 {
                return Err(core::fmt::Error);
            }
            self.0 -= text.len();
            debug(text);
            Ok(())
        }
    }
    debug("uefi-ownership-error=");
    // RetainError contains only fixed enums. Bound formatting even if a later
    // error gains data: this terminal/bootstrap diagnostic never allocates.
    let _ = write!(&mut BoundedDebug(160), "{error:?}");
    debug("\n");
}

fn debug_smp_map(
    map: &impl uefi::mem::memory_map::MemoryMap,
    arena_base: u64,
    resources: svmvisor_hypervisor::boot::ownership::SmpResources,
) {
    debug("uefi-smp-map count=0x");
    debug_hex(map.len() as u64);
    debug(" version=0x");
    debug_hex(map.meta().desc_version as u64);
    debug("\n");
    let low = resources.low_page().base();
    for descriptor in
        map.entries().take(svmvisor_hypervisor::boot::ownership::MAX_OWNERSHIP_SMP_DESCRIPTORS)
    {
        let start = descriptor.phys_start;
        let end =
            descriptor.page_count.checked_mul(4096).and_then(|bytes| start.checked_add(bytes));
        if end.is_some_and(|end| {
            (start < low + 4096 && end > low)
                || (start < arena_base + ARENA_BYTES as u64 && end > arena_base)
        }) {
            debug("uefi-smp-map-owned start=0x");
            debug_hex(start);
            debug(" pages=0x");
            debug_hex(descriptor.page_count);
            debug(" type=0x");
            debug_hex(descriptor.ty.0 as u64);
            debug(" attributes=0x");
            debug_hex(descriptor.att.bits());
            debug("\n");
        }
    }
}

fn debug_hex(value: u64) {
    for shift in (0..16).rev() {
        let digit = b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize];
        unsafe {
            asm!("out dx, al", in("dx") 0xe9_u16, in("al") digit, options(nomem, nostack));
        }
    }
}
