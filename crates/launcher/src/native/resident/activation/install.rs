//! Serialized BSP installation: payload delivery, per-slot preparation, admission and
//! publication of the resident pool before READY.
#[cfg(feature = "native-resident-boot")]
use core::arch::x86_64::__cpuid_count;
use core::{ffi::c_void, ptr, sync::atomic::Ordering};

use svmvisor_card_abi::package::Package;
#[cfg(feature = "native-resident-boot")]
use svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE;
#[cfg(any(feature = "native-resident-smp-prepare", feature = "native-resident-smp-activate"))]
use svmvisor_hypervisor::{
    arch::x86_64::capabilities::EvidenceFlag,
    memory::npt::{NptEvidence, TableStorage},
};
use svmvisor_hypervisor::{
    arch::x86_64::msr::PAT,
    boot::memory::ValidatedMemoryMap,
    host::resident::{self as abi, ResidentDirectory},
    memory::address::AddressPolicy,
};
use svmvisor_launcher::native::{
    admission::memory,
    resident::{
        self, allocation,
        launch::{common_backing_offset, directory_valid},
    },
};
use uefi_raw::{
    Handle, Status, guid,
    protocol::loaded_image::LoadedImageProtocol,
    table::{
        boot::{BootServices, EventType, MemoryType, Tpl},
        system::SystemTable,
    },
};

use super::{
    COOKIE, CPU, CPU_COUNT, CPU_IDS, DIRECTORIES, IMAGE, INSTALLED, MAP, MAP_COUNT, PACKAGE, READY,
    RESIDENT_ENTRY_OFFSET,
    capture::{config, cpu, mtrrs, rdmsr},
    closure::host_closure,
    diagnostic::{trace, trace_detail, unsupported},
    mapping::mapped,
    preparation::{preparation_failure, preparation_map_failure, preparation_step},
};
#[cfg(feature = "native-resident-boot")]
use super::{boot_handoff, card_boot, preparation::identity_npt_error_code};
#[cfg(feature = "native-resident-smp-activate")]
use super::{
    diagnostic::admission_hint,
    physical_boot,
    preparation::{address_error_code, resident_memory_error_code},
};

/// Install retained payload and the selected event, diagnostic seam or EBS interposer once.
/// # Safety
/// These are the live runtime-driver entry arguments. Firmware provides trusted
/// x64 identity mappings, coherent page tables, a flat hidden-segment profile,
/// no debugger/NMI/SMM interference during capture, and this single BSP owns
/// the eventual resident interval. No physical launch is implied by the test
/// profile: AMD routing/encryption profiles outside this admission are refused.
pub(crate) unsafe fn install(image: Handle, table: *mut SystemTable) -> Status {
    trace(b'I');
    if table.is_null() || INSTALLED.swap(true, Ordering::AcqRel) {
        return Status::UNSUPPORTED;
    }
    let boot_services = unsafe { (*table).boot_services };
    if boot_services.is_null() {
        return Status::INVALID_PARAMETER;
    }
    #[cfg(feature = "native-resident-boot")]
    let card = match unsafe { card_boot::Prepared::new(image, &*boot_services) } {
        Ok(value) => value,
        Err(status) => return status,
    };
    #[cfg(feature = "native-resident-boot")]
    let handoff = match {
        preparation_step(1, boot_services as u64);
        unsafe { boot_handoff::Prepared::new(boot_services) }
    } {
        Ok(value) => value,
        Err(status) => {
            if let Some(card) = card {
                card.complete(status, false);
            }
            return status;
        }
    };
    let result = unsafe { install_inner(image, &*boot_services) };
    #[cfg(feature = "native-resident-boot")]
    if result.is_ok() && READY.load(Ordering::Acquire) {
        unsafe { handoff.commit(boot_services) };
        unsafe { card_boot::stage(1, 0, 0) };
    }
    #[cfg(feature = "native-resident-boot")]
    if let Some(card) = card {
        card.complete(
            result.as_ref().err().copied().unwrap_or(Status::PROTOCOL_ERROR),
            result.is_ok() && READY.load(Ordering::Acquire),
        );
    }
    match result {
        Ok(()) => {
            trace(if READY.load(Ordering::Acquire) { b'R' } else { b'J' });
            Status::SUCCESS
        }
        Err(status) => {
            trace(b'F');
            trace_detail(&status);
            status
        }
    }
}

unsafe fn install_inner(image: Handle, boot_services: &BootServices) -> Result<(), Status> {
    trace(b'a');
    preparation_step(2, 0);
    let processor = unsafe { cpu() }.map_err(unsupported)?;
    trace(b'b');
    preparation_step(3, 0);
    let inventory = unsafe { resident::processors::inspect(boot_services) }.map_err(|error| {
        trace_detail(&error);
        Status::UNSUPPORTED
    })?;
    let count = inventory.processors().len();
    #[cfg(not(any(
        feature = "native-resident-smp-prepare",
        feature = "native-resident-smp-activate"
    )))]
    if count != 1 {
        return Err(Status::UNSUPPORTED);
    }
    let bsp = inventory.bsp_number();
    let common = inventory.processors()[bsp].identity;
    if common.apic_id != processor.apic_id {
        return Err(Status::UNSUPPORTED);
    }
    for p in inventory.processors() {
        let mut identity = p.identity;
        // Identity fields are CPU-local; all common requirements must agree.
        // Host APIC IDs above 254 are refused: the AVIC doorbell ID field and
        // x2AVIC table entry 255 (see abi::is_valid_pool_slot).
        identity.apic_id = common.apic_id;
        if identity != common || p.identity.apic_id > 254 {
            return Err(Status::UNSUPPORTED);
        }
    }
    trace(b'c');
    preparation_step(4, 0);
    let mut loaded = ptr::null_mut();
    let status =
        unsafe { (boot_services.handle_protocol)(image, &LoadedImageProtocol::GUID, &mut loaded) };
    if status != Status::SUCCESS {
        return Err(status);
    }
    if loaded.is_null() || loaded as usize % core::mem::align_of::<LoadedImageProtocol>() != 0 {
        return Err(Status::LOAD_ERROR);
    }
    let loaded = unsafe { &*loaded.cast::<LoadedImageProtocol>() };
    let image_base = loaded.image_base as u64;
    if loaded.image_code_type != MemoryType::RUNTIME_SERVICES_CODE
        || loaded.image_data_type != MemoryType::RUNTIME_SERVICES_DATA
        || image_base == 0
        || image_base & 4095 != 0
        || loaded.image_size == 0
        || loaded.image_size > 16 * 1024 * 1024
        || image_base.checked_add(loaded.image_size).is_none()
    {
        return Err(Status::UNSUPPORTED);
    }
    unsafe {
        IMAGE = (image_base, loaded.image_size);
    }
    trace(b'd');
    preparation_step(5, image_base);
    let package = Package::parse(PACKAGE, RESIDENT_ENTRY_OFFSET).map_err(|error| {
        trace_detail(&error);
        Status::LOAD_ERROR
    })?;
    trace(b'e');
    preparation_step(6, 0);
    let mut arena =
        unsafe { allocation::allocate_for_processors(boot_services, count) }.map_err(|error| {
            trace_detail(&error);
            let (reason, status, address) = error.diagnostic();
            preparation_failure(reason, status, address);
            Status::OUT_OF_RESOURCES
        })?;
    trace(b'f');
    preparation_step(7, arena.base());
    let cfg = unsafe { config(processor) }.map_err(unsupported)?;
    trace(b'g');
    preparation_step(8, arena.base());
    let mt = unsafe { mtrrs(processor.physical_bits) }.map_err(unsupported)?;
    let pat = unsafe { rdmsr(PAT) };
    let policy = AddressPolicy::new(processor.physical_bits, processor.encryption)
        .map_err(|_| Status::UNSUPPORTED)?;
    {
        trace(b'h');
        preparation_step(9, 0);
        let mut map = unsafe { memory::collect(boot_services) }.map_err(|error| {
            trace_detail(&error);
            preparation_map_failure(error)
        })?;
        trace(b'i');
        preparation_step(10, arena.base());
        arena.validate_map(policy, map.descriptors()).map_err(|error| {
            trace_detail(&error);
            Status::UNSUPPORTED
        })?;
        trace(b'j');
        for slot in 0..count {
            preparation_step(11, arena.slot_base(slot).unwrap_or(0));
            unsafe {
                mapped(
                    map.descriptors(),
                    cfg,
                    &mt,
                    pat,
                    arena.slot_base(slot).ok_or(Status::LOAD_ERROR)?,
                    0x100000,
                    true,
                    true,
                )
            }
            .map_err(unsupported)?;
            trace(b'k');
            preparation_step(12, arena.slot_base(slot).unwrap_or(0));
            unsafe { arena.initialize_slot(&package, slot) }.map_err(|error| {
                trace_detail(&error);
                Status::LOAD_ERROR
            })?;
        }
        map.release()?;
    }
    let mut directories = [ResidentDirectory::default(); abi::MAX_RESIDENT_CPUS];
    #[cfg(feature = "native-resident-smp-activate")]
    let mut physical_storage = {
        preparation_step(13, 0);
        unsafe { physical_boot::prepare(boot_services, bsp, count, cfg)? }
    };
    trace(b'l');
    for (slot, directory) in directories.iter_mut().enumerate().take(count) {
        let base = arena.slot_base(slot).ok_or(Status::LOAD_ERROR)?;
        preparation_step(14, base);
        let prepare: abi::PrepareRuntime =
            unsafe { core::mem::transmute(base as usize + RESIDENT_ENTRY_OFFSET) };
        let prepared = unsafe {
            prepare(
                base,
                directory,
                arena.base(),
                arena.bytes() as u64,
                slot as u64,
                inventory.processors()[slot].identity.apic_id as u64,
            )
        };
        if prepared != 0
            || !directory_valid(directory, base)
            || base + RESIDENT_ENTRY_OFFSET as u64 >= directory.text_end
        {
            trace_detail(&prepared);
            trace_detail(&directory);
            return Err(Status::LOAD_ERROR);
        }
    }
    // Every private root aliases each slot's backing at slot base plus one
    // common image offset; refuse a pool whose copies disagree.
    if common_backing_offset(&directories[..count]).is_none() {
        trace_detail(&("backing-offset", count));
        return Err(Status::LOAD_ERROR);
    }
    // Hardware's physical-ID table has one excluded WB backing. No processor
    // has entered yet, so all valid entries can be constructed before publication.
    {
        use svmvisor_hypervisor::svm::x2avic::PhysicalIdTable;
        let table = (arena.base() + abi::X2AVIC_TABLE_OFFSET) as *mut PhysicalIdTable;
        unsafe {
            table.write(PhysicalIdTable::new());
        }
        for d in &directories[..count] {
            unsafe { (&mut *table).insert_stopped(d.apic_id as u16, d.avic_backing, &policy) }
                .map_err(|_| Status::UNSUPPORTED)?;
        }
    }
    // Capture storage has one pool-owned backing and read-only host aliases.
    // Native boot populates it after successful EBS return; firmware can still
    // synchronize MTRRs in EBS callbacks. Every owned CPU samples before entry.
    {
        use svmvisor_hypervisor::svm::cache::CacheCapture;
        let capture = (arena.base() + abi::CACHE_CAPTURE_OFFSET) as *mut CacheCapture;
        unsafe {
            capture.write(CacheCapture::empty());
        }
        unsafe {
            ((arena.base() + abi::CACHE_OWNER_OFFSET)
                as *mut svmvisor_hypervisor::svm::cache::CacheOwner)
                .write(svmvisor_hypervisor::svm::cache::CacheOwner::empty());
        }
        #[cfg(feature = "native-resident-boot")]
        if __cpuid_count(1, 0).eax == TARGET_SIGNATURE {
            if !unsafe { (&mut *capture).initialize(count) } {
                return Err(Status::LOAD_ERROR);
            }
        }
    }
    #[cfg(feature = "native-resident-smp-activate")]
    {
        // Separate fixed block follows all 32 mailboxes; initialize before any
        // AP can enter. It is not a guest-writable acknowledgement buffer.
        let terminal_control =
            (arena.base() + abi::STARTUP_PAGE_OFFSET + abi::terminal::CONTROL_OFFSET)
                as *mut abi::terminal::TerminalControl;
        unsafe {
            terminal_control.write(abi::terminal::TerminalControl::new());
        }
    }
    #[cfg(feature = "native-resident-guest-startup")]
    {
        use svmvisor_hypervisor::svm::x2avic::startup::NativeStartupMailbox;
        const _: () = assert!(
            core::mem::size_of::<NativeStartupMailbox>() * abi::MAX_RESIDENT_CPUS
                <= abi::terminal::CONTROL_OFFSET as usize
        );
        let shared = (arena.base() + abi::STARTUP_PAGE_OFFSET) as *mut NativeStartupMailbox;
        for slot in 0..count {
            // The admitted pool is exclusively DXE-owned before activation.
            // Constructors publish Assigned slots; the target later marks ready.
            unsafe {
                shared.add(slot).write(NativeStartupMailbox::new(
                    inventory.processors()[slot].identity.apic_id,
                ));
            }
        }
    }
    #[cfg(feature = "native-resident-smp-prepare")]
    {
        preparation_step(15, 0);
        let mut map = unsafe { memory::collect(boot_services) }.map_err(preparation_map_failure)?;
        arena.validate_map(policy, map.descriptors()).map_err(|_| Status::UNSUPPORTED)?;
        let pool = policy
            .validate(arena.base(), arena.bytes() as u64, 4096)
            .map_err(|_| Status::UNSUPPORTED)?;
        for (slot, d) in directories.iter().enumerate().take(count) {
            unsafe {
                host_closure(&directories[..count], slot, map.descriptors(), processor, &mt, pat)
            }
            .map_err(unsupported)?;
            let storage = unsafe { &mut *(d.npt as *mut TableStorage) };
            let mut npt = resident::memory::prepare_identity_npt(
                storage,
                d.npt,
                policy,
                pool,
                map.descriptors(),
                NptEvidence {
                    nx_supported: EvidenceFlag::Set,
                    host_nxe: EvidenceFlag::Set,
                    host_four_level: EvidenceFlag::Set,
                },
                EvidenceFlag::Set,
                pat,
            )
            .map_err(|error| {
                trace_detail(&error);
                Status::UNSUPPORTED
            })?;
            #[cfg(feature = "native-resident-boot")]
            card_boot::protect_config(&mut npt).map_err(|_| Status::UNSUPPORTED)?;
            for other in directories.iter().take(count) {
                if npt.translate(other.arena_base).map_err(|_| Status::LOAD_ERROR)?.is_some()
                    || npt
                        .translate(other.arena_base + 0xfffff)
                        .map_err(|_| Status::LOAD_ERROR)?
                        .is_some()
                {
                    return Err(Status::LOAD_ERROR);
                }
            }
            trace_detail(&(
                "private-slot",
                slot,
                d.apic_id,
                d.arena_base,
                d.context,
                d.vmcb,
                d.npt,
            ));
        }
        map.release()?;

        let retained = arena.publish().map_err(|_| Status::DEVICE_ERROR)?;
        trace_detail(&(
            "native-smp-prepared",
            count,
            inventory.completed_ap_callbacks(),
            bsp,
            retained.base(),
            retained.bytes(),
        ));
        return Ok(());
    }
    #[cfg(feature = "native-resident-smp-activate")]
    {
        preparation_step(15, 0);
        let mut map = unsafe { memory::collect(boot_services) }.map_err(preparation_map_failure)?;
        arena.validate_map(policy, map.descriptors()).map_err(|_| Status::UNSUPPORTED)?;
        ValidatedMemoryMap::new(map.descriptors(), processor.physical_bits.min(40))
            .map_err(|_| Status::UNSUPPORTED)?;
        // The admission-hint recorder is armed around each slot's closure and
        // NPT work (operations 9/10) so a refusal names its predicate and code
        // on the card without any test feature; see `slot_admission_refused`.
        for (slot, d) in directories.iter().enumerate().take(count) {
            preparation_step(16, d.arena_base);
            physical_boot::admission_begin(9, slot as u32, d.apic_id as u32);
            unsafe {
                host_closure(&directories[..count], slot, map.descriptors(), processor, &mt, pat)
            }
            .map_err(|code| unsafe {
                physical_boot::slot_admission_refused(33, count, code, processor, map.descriptors())
            })?;
            let pool = policy.validate(d.pool_base, d.pool_bytes, 4096).map_err(|error| {
                let code = address_error_code(error);
                admission_hint(635, d.pool_base, d.pool_bytes, code);
                unsafe {
                    physical_boot::slot_admission_refused(
                        34,
                        count,
                        code,
                        processor,
                        map.descriptors(),
                    )
                }
            })?;
            physical_boot::admission_clear();
            preparation_step(17, d.arena_base);
            physical_boot::admission_begin(10, slot as u32, d.apic_id as u32);
            let mut _npt = resident::memory::prepare_identity_npt(
                unsafe { &mut *(d.npt as *mut TableStorage) },
                d.npt,
                policy,
                pool,
                map.descriptors(),
                NptEvidence {
                    nx_supported: EvidenceFlag::Set,
                    host_nxe: EvidenceFlag::Set,
                    host_four_level: EvidenceFlag::Set,
                },
                EvidenceFlag::Set,
                pat,
            )
            .map_err(|error| {
                let code = resident_memory_error_code(error);
                admission_hint(636, d.npt, d.pool_base, code);
                unsafe {
                    physical_boot::slot_admission_refused(
                        35,
                        count,
                        code,
                        processor,
                        map.descriptors(),
                    )
                }
            })?;
            #[cfg(feature = "native-resident-boot")]
            card_boot::protect_config(&mut _npt).map_err(|error| {
                let code = identity_npt_error_code(error);
                admission_hint(637, d.npt, d.pool_base, code);
                unsafe {
                    physical_boot::slot_admission_refused(
                        36,
                        count,
                        code,
                        processor,
                        map.descriptors(),
                    )
                }
            })?;
            physical_boot::admission_clear();
        }
        preparation_step(18, 0);
        unsafe { physical_boot::validate(map.descriptors(), cfg, &mt, pat, count) }
            .map_err(unsupported)?;
        unsafe {
            ptr::copy_nonoverlapping(
                map.descriptors().as_ptr(),
                ptr::addr_of_mut!(MAP).cast(),
                map.descriptors().len(),
            );
            MAP_COUNT = map.descriptors().len();
            DIRECTORIES = directories;
            CPU_COUNT = count;
            CPU = Some(processor);
            for (slot, p) in inventory.processors().iter().enumerate() {
                CPU_IDS[slot] = p.identity.apic_id;
            }
        }
        map.release()?;
        preparation_step(19, 0);
        unsafe {
            physical_boot::admit_processors(boot_services)?;
        }
        preparation_step(20, arena.base());
        let _retained = arena.register_and_publish(|base, bytes| unsafe {
            physical_boot::publish(boot_services, count, base, bytes as u64)
        })?;
        physical_storage.retain();
        READY.store(true, Ordering::Release);
        return Ok(());
    }
    #[allow(unreachable_code)]
    let prepared = &directories[..count];
    trace(b'm');
    let mut event = ptr::null_mut();
    let mut group = guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b");
    let status = unsafe {
        (boot_services.create_event_ex)(
            EventType::NOTIFY_SIGNAL,
            Tpl::NOTIFY,
            Some(abi::svmvisor_resident_callback),
            COOKIE as *mut c_void,
            &mut group,
            &mut event,
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    if event.is_null() {
        return Err(Status::DEVICE_ERROR);
    }
    let finish = (|| {
        trace(b'n');
        let mut map = unsafe { memory::collect(boot_services) }.map_err(|error| {
            trace_detail(&error);
            preparation_map_failure(error)
        })?;
        arena.validate_map(policy, map.descriptors()).map_err(|error| {
            trace_detail(&error);
            Status::UNSUPPORTED
        })?;
        ValidatedMemoryMap::new(map.descriptors(), processor.physical_bits.min(40)).map_err(
            |error| {
                trace_detail(&error);
                Status::UNSUPPORTED
            },
        )?;
        trace(b'o');
        unsafe { host_closure(prepared, bsp, map.descriptors(), processor, &mt, pat) }
            .map_err(unsupported)?;
        unsafe {
            ptr::copy_nonoverlapping(
                map.descriptors().as_ptr(),
                ptr::addr_of_mut!(MAP).cast(),
                map.descriptors().len(),
            );
            MAP_COUNT = map.descriptors().len();
            DIRECTORIES = directories;
            CPU_COUNT = count;
            CPU_IDS[0] = processor.apic_id;
            CPU = Some(processor);
        }
        map.release()?;
        Ok(())
    })();
    if let Err(error) = finish {
        let close = unsafe { (boot_services.close_event)(event) };
        if close != Status::SUCCESS {
            // Returning an EFI error permits image unload while the event can
            // still call its assembly. Retain both image and raw allocation as
            // an inert success instead, leave READY=false and expose failure.
            let _retained = arena.publish().map_err(|_| Status::DEVICE_ERROR)?;
            trace(b'X');
            return Ok(());
        }
        return Err(error);
    }
    // The event may now hold numeric pointers, but READY kept it inert across
    // registration and map collection. There are no fallible firmware calls
    // after publication. Refused callbacks retain the inert arena until reset.
    trace(b'p');
    let _retained = arena.publish().map_err(|_| Status::DEVICE_ERROR)?;
    READY.store(true, Ordering::Release);
    Ok(())
}
