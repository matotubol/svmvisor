//! Returning native MP observations for resident preparation; never AP takeover.
//!
//! PI 1.10 II-13.4.5 (StartupThisAP/Table 13.5, printed II-133--135) defines
//! blocking completion and termination on timeout. UEFI 2.11 7.4.6 requires this
//! work before the first ExitBootServices attempt. CPUID follows AMD APM2 3.44
//! 15.4 and the applicable processor CPUID definitions. Captures grant no lease
//! on firmware AP state and do not change SVM, control registers, MSRs or TPL.
use core::{
    arch::x86_64::__cpuid_count,
    cell::UnsafeCell,
    ffi::c_void,
    mem::align_of,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use uefi_raw::{Status, table::boot::BootServices};

use crate::{
    diagnostics::resident_boot::AdmissionFailure,
    native::admission::cpu::{
        AP_TIMEOUT_MICROSECONDS, MP_SERVICES_GUID, MpServicesProtocol, ProcessorInformation,
    },
};

pub use svmvisor_hypervisor::host::resident::MAX_RESIDENT_CPUS as MAX_PROCESSORS;

/// Owned bounded snapshot; firmware processor numbers and APIC IDs stay distinct.
#[derive(Debug)]
pub struct Inventory {
    processors: [Processor; MAX_PROCESSORS],
    count: usize,
    bsp_number: usize,
    completed: usize,
}

impl Inventory {
    pub fn processors(&self) -> &[Processor] {
        &self.processors[..self.count]
    }
    pub const fn bsp_number(&self) -> usize {
        self.bsp_number
    }
    pub const fn completed_ap_callbacks(&self) -> usize {
        self.completed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Processor {
    pub firmware_number: usize,
    pub information: ProcessorInformation,
    pub identity: Identity,
}

/// CPU-local read-only observations. Missing optional CPUID leaves are zero.
/// `apic_id` uses AMD extended APIC identity when TOPOEXT is advertised.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub vendor: [u8; 12],
    pub max_basic: u32,
    pub max_extended: u32,
    pub signature: u32,
    pub basic_features_ecx: u32,
    pub basic_features_edx: u32,
    pub extended_features_ecx: u32,
    pub extended_features_edx: u32,
    pub svm_revision: u32,
    pub svm_asids: u32,
    pub svm_features: u32,
    pub physical_bits: u8,
    pub apic_id: u32,
}

struct Capture {
    mp: *const MpServicesProtocol,
    expected: usize,
    read: fn() -> Result<Identity, Error>,
    result: UnsafeCell<Result<Identity, Error>>,
    state: AtomicUsize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Admission(AdmissionFailure),
}

/// Capture every admitted CPU with returned, bounded blocking AP callbacks.
/// The snapshot is suitable for allocation planning, not persistent activation.
///
/// # Safety
/// Trusted live x64 Boot Services on the BSP at TPL <= NOTIFY, before the first
/// EBS attempt, with exclusive MP dispatch and stable processor inventory.
/// The provider must implement PI blocking completion/timeout termination: no
/// callback may still access its record after StartupThisAP returns on any path.
/// Callback CPUID and WhoAmI must be permitted. Firmware retains AP ownership;
/// all allocations and subsequent irreversible activation remain caller-owned.
/// PI1.10 II-13.4.1/.5/.8, Table13.5; UEFI2.11 7.4.6; AMD APM2 3.44 15.4.
pub unsafe fn inspect(boot_services: &BootServices) -> Result<Inventory, Error> {
    unsafe { inspect_with(boot_services, capture_identity) }
}

/// Returning CPU-local admission using the same bounded MP owner.
/// # Safety
/// Same firmware/lifetime contract as inspect. Reader must be bounded,
/// preserve CPU state, and make no allocation, firmware call, or persistent
/// activation. It must return the current native CPUID identity.
pub unsafe fn inspect_with(
    boot_services: &BootServices,
    read: fn() -> Result<Identity, Error>,
) -> Result<Inventory, Error> {
    let mut raw = ptr::null_mut();
    let status =
        unsafe { (boot_services.locate_protocol)(&MP_SERVICES_GUID, ptr::null_mut(), &mut raw) };
    if status != Status::SUCCESS {
        return Err(admission_error(1, usize::MAX, 0, 0, 0, status));
    }
    if raw.is_null() || raw as usize % align_of::<MpServicesProtocol>() != 0 {
        return Err(admission_error(
            2,
            usize::MAX,
            raw as u64,
            raw as u64,
            align_of::<MpServicesProtocol>() as u64,
            Status::UNSUPPORTED,
        ));
    }
    unsafe { inspect_protocol(raw.cast(), read) }
}

pub fn capture_identity() -> Result<Identity, Error> {
    let basic = __cpuid_count(0, 0);
    let extended = __cpuid_count(0x80000000, 0);
    let mut vendor = [0; 12];
    vendor[..4].copy_from_slice(&basic.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&basic.edx.to_le_bytes());
    vendor[8..].copy_from_slice(&basic.ecx.to_le_bytes());
    if vendor != *b"AuthenticAMD" || basic.eax < 1 || extended.eax < 0x80000008 {
        let (item, observed, expected): (u64, u32, u32) = if vendor != *b"AuthenticAMD" {
            if basic.ebx != 0x68747541 {
                (0, basic.ebx, 0x68747541)
            } else if basic.edx != 0x69746e65 {
                (1, basic.edx, 0x69746e65)
            } else {
                (2, basic.ecx, 0x444d4163)
            }
        } else if basic.eax < 1 {
            (3, basic.eax, 1)
        } else {
            (4, extended.eax, 0x80000008)
        };
        return Err(admission_error(
            19,
            usize::MAX,
            item,
            observed as u64,
            expected as u64,
            Status::UNSUPPORTED,
        ));
    }
    let one = __cpuid_count(1, 0);
    let ext = __cpuid_count(0x80000001, 0);
    let width = __cpuid_count(0x80000008, 0).eax as u8;
    if !(32..=52).contains(&width) {
        return Err(admission_error(
            20,
            usize::MAX,
            0x80000008,
            width as u64,
            32 | 52 << 32,
            Status::UNSUPPORTED,
        ));
    }
    let topology = ext.ecx & (1 << 22) != 0;
    if topology && extended.eax < 0x8000001e {
        return Err(admission_error(
            21,
            usize::MAX,
            0x80000000,
            extended.eax as u64,
            0x8000001e,
            Status::UNSUPPORTED,
        ));
    }
    let apic_id = if topology { __cpuid_count(0x8000001e, 0).eax } else { one.ebx >> 24 };
    let svm = if extended.eax >= 0x8000000a && ext.ecx & 4 != 0 {
        let leaf = __cpuid_count(0x8000000a, 0);
        [leaf.eax, leaf.ebx, leaf.edx]
    } else {
        [0; 3]
    };
    Ok(Identity {
        vendor,
        max_basic: basic.eax,
        max_extended: extended.eax,
        signature: one.eax,
        basic_features_ecx: one.ecx,
        basic_features_edx: one.edx,
        extended_features_ecx: ext.ecx,
        extended_features_edx: ext.edx,
        svm_revision: svm[0],
        svm_asids: svm[1],
        svm_features: svm[2],
        physical_bits: width,
        apic_id,
    })
}

unsafe fn inspect_protocol(
    mp: *const MpServicesProtocol,
    read: fn() -> Result<Identity, Error>,
) -> Result<Inventory, Error> {
    let (mut total, mut enabled, mut bsp) = (0, 0, usize::MAX);
    unsafe {
        firmware(((*mp).get_number_of_processors)(mp, &mut total, &mut enabled), 3, usize::MAX)?;
        firmware(((*mp).who_am_i)(mp, &mut bsp), 4, usize::MAX)?;
    }
    if total == 0 || total > MAX_PROCESSORS || enabled != total || bsp >= total {
        let (item, observed, expected) = if total == 0 {
            (0, total, 1)
        } else if total > MAX_PROCESSORS {
            (1, total, MAX_PROCESSORS)
        } else if enabled != total {
            (2, enabled, total)
        } else {
            (3, bsp, total)
        };
        return Err(admission_error(
            5,
            bsp,
            item,
            observed as u64,
            expected as u64,
            Status::UNSUPPORTED,
        ));
    }
    let mut inventory = Inventory {
        processors: [Processor::default(); MAX_PROCESSORS],
        count: total,
        bsp_number: bsp,
        completed: 0,
    };
    for number in 0..total {
        let mut info = ProcessorInformation::default();
        unsafe {
            firmware(((*mp).get_processor_info)(mp, number, &mut info), 6, number)?;
        }
        let expected_flags = if number == bsp { 7 } else { 6 };
        if info.status_flag != expected_flags
            || inventory.processors[..number]
                .iter()
                .any(|p| p.information.processor_id == info.processor_id)
        {
            return Err(admission_error(
                if info.status_flag != expected_flags { 7 } else { 8 },
                number,
                info.processor_id,
                info.status_flag as u64,
                expected_flags as u64,
                Status::UNSUPPORTED,
            ));
        }
        inventory.processors[number] =
            Processor { firmware_number: number, information: info, identity: Identity::default() };
    }
    for number in 0..total {
        let identity = if number == bsp {
            read().map_err(|error| match error {
                Error::Admission(mut f) => {
                    f.processor = number as u32;
                    Error::Admission(f)
                }
            })?
        } else {
            let record = Capture {
                mp,
                expected: number,
                read,
                result: UnsafeCell::new(Err(admission_error(
                    10,
                    number,
                    0,
                    0,
                    2,
                    Status::NOT_READY,
                ))),
                state: AtomicUsize::new(0),
            };
            let status = unsafe {
                ((*mp).startup_this_ap)(
                    mp,
                    capture_ap,
                    number,
                    ptr::null_mut(),
                    AP_TIMEOUT_MICROSECONDS,
                    ptr::from_ref(&record).cast_mut().cast(),
                    ptr::null_mut(),
                )
            };
            if status != Status::SUCCESS {
                return Err(admission_error(9, number, 0, 0, 0, status));
            }
            let completed = record.state.load(Ordering::Acquire);
            if completed != 2 {
                return Err(admission_error(
                    10,
                    number,
                    0,
                    completed as u64,
                    2,
                    Status::UNSUPPORTED,
                ));
            }
            let identity = record.result.into_inner().map_err(|error| match error {
                Error::Admission(mut f) => {
                    f.processor = number as u32;
                    Error::Admission(f)
                }
            })?;
            inventory.completed += 1;
            identity
        };
        // Native admission requires the firmware hardware ID to match actual
        // CPUID APIC identity. PI defines a unique hardware ID, not this equality
        // for every architecture; other firmware identity conventions refuse.
        if inventory.processors[number].information.processor_id != u64::from(identity.apic_id) {
            let mut f = admission_error(
                13,
                number,
                0,
                identity.apic_id as u64,
                inventory.processors[number].information.processor_id,
                Status::UNSUPPORTED,
            );
            let Error::Admission(ref mut value) = f;
            value.apic_id = identity.apic_id;
            return Err(f);
        }
        inventory.processors[number].identity = identity;
    }
    // Do not publish an inventory that changed while AP observations ran.
    let (mut final_total, mut final_enabled, mut final_bsp) = (0, 0, usize::MAX);
    unsafe {
        firmware(
            ((*mp).get_number_of_processors)(mp, &mut final_total, &mut final_enabled),
            14,
            usize::MAX,
        )?;
        firmware(((*mp).who_am_i)(mp, &mut final_bsp), 15, usize::MAX)?;
    }
    if (total, enabled, bsp) != (final_total, final_enabled, final_bsp) {
        let (item, observed, expected) = if total != final_total {
            (0, final_total, total)
        } else if enabled != final_enabled {
            (1, final_enabled, enabled)
        } else {
            (2, final_bsp, bsp)
        };
        return Err(admission_error(
            16,
            final_bsp,
            item,
            observed as u64,
            expected as u64,
            Status::UNSUPPORTED,
        ));
    }
    for processor in inventory.processors() {
        let mut info = ProcessorInformation::default();
        unsafe {
            firmware(
                ((*mp).get_processor_info)(mp, processor.firmware_number, &mut info),
                17,
                processor.firmware_number,
            )?;
        }
        if info != processor.information {
            let (item, observed, expected) =
                if info.processor_id != processor.information.processor_id {
                    (0, info.processor_id, processor.information.processor_id)
                } else if info.status_flag != processor.information.status_flag {
                    (1, info.status_flag as u64, processor.information.status_flag as u64)
                } else if info.package != processor.information.package {
                    (2, info.package as u64, processor.information.package as u64)
                } else if info.core != processor.information.core {
                    (3, info.core as u64, processor.information.core as u64)
                } else {
                    (4, info.thread as u64, processor.information.thread as u64)
                };
            return Err(admission_error(
                18,
                processor.firmware_number,
                item,
                observed,
                expected,
                Status::UNSUPPORTED,
            ));
        }
    }
    Ok(inventory)
}

extern "efiapi" fn capture_ap(argument: *mut c_void) {
    // Only the synchronous dispatcher below supplies this live callback record.
    let record = unsafe { &*argument.cast::<Capture>() };
    if record.state.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        record.state.store(3, Ordering::Release);
        return;
    }
    let mut who = usize::MAX;
    let status = unsafe { ((*record.mp).who_am_i)(record.mp, &mut who) };
    let result = if status != Status::SUCCESS {
        Err(admission_error(11, record.expected, 0, who as u64, record.expected as u64, status))
    } else if who != record.expected {
        Err(admission_error(
            12,
            record.expected,
            0,
            who as u64,
            record.expected as u64,
            Status::UNSUPPORTED,
        ))
    } else {
        (record.read)()
    };
    unsafe {
        record.result.get().write(result);
    }
    // A duplicate callback cannot overwrite the failure with completion.
    let _ = record.state.compare_exchange(1, 2, Ordering::Release, Ordering::Relaxed);
}

fn firmware(status: Status, predicate: u32, processor: usize) -> Result<(), Error> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(admission_error(predicate, processor, 0, 0, 0, status))
    }
}

fn admission_error(
    predicate: u32,
    processor: usize,
    item: u64,
    observed: u64,
    expected: u64,
    status: Status,
) -> Error {
    let mut f = AdmissionFailure::new(2, predicate, item, observed, expected, status.0 as u64);
    f.processor = u32::try_from(processor).unwrap_or(u32::MAX);
    Error::Admission(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::admission::cpu::ApProcedure;
    use uefi_raw::{Boolean, Event};
    // One test owns this synthetic MP provider; callbacks execute synchronously.
    static CURRENT: AtomicUsize = AtomicUsize::new(2);
    static MODE: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static COUNTS: AtomicUsize = AtomicUsize::new(0);
    static INFOS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "efiapi" fn counts(
        _: *const MpServicesProtocol,
        total: *mut usize,
        enabled: *mut usize,
    ) -> Status {
        let mode = MODE.load(Ordering::Relaxed);
        let observation = COUNTS.fetch_add(1, Ordering::Relaxed);
        let count = if mode == 5 {
            MAX_PROCESSORS + 1
        } else if mode == 10 && observation != 0 {
            2
        } else {
            3
        };
        unsafe {
            total.write(count);
            enabled.write(if mode == 8 { count - 1 } else { count });
        }
        Status::SUCCESS
    }
    unsafe extern "efiapi" fn who(_: *const MpServicesProtocol, number: *mut usize) -> Status {
        if MODE.load(Ordering::Relaxed) == 14 && CURRENT.load(Ordering::Relaxed) == 1 {
            return Status::DEVICE_ERROR;
        }
        unsafe {
            number.write(
                if MODE.load(Ordering::Relaxed) == 12 && COUNTS.load(Ordering::Relaxed) == 2 {
                    0
                } else {
                    CURRENT.load(Ordering::Relaxed)
                },
            );
        }
        Status::SUCCESS
    }
    unsafe extern "efiapi" fn info(
        _: *const MpServicesProtocol,
        number: usize,
        output: *mut ProcessorInformation,
    ) -> Status {
        let mode = MODE.load(Ordering::Relaxed);
        let observation = INFOS.fetch_add(1, Ordering::Relaxed);
        let id = if mode == 4 { 17 } else { (number + 1) as u64 * 17 };
        let flags = if (mode == 6 && number == 1) || (mode == 11 && observation >= 3) {
            2
        } else if number == 2 {
            7
        } else {
            6
        };
        unsafe {
            output.write(ProcessorInformation {
                processor_id: id,
                status_flag: flags,
                ..ProcessorInformation::default()
            });
        }
        Status::SUCCESS
    }
    unsafe extern "efiapi" fn startup(
        _: *const MpServicesProtocol,
        procedure: ApProcedure,
        number: usize,
        event: Event,
        timeout: usize,
        argument: *mut c_void,
        finished: *mut Boolean,
    ) -> Status {
        assert!(event.is_null() && finished.is_null());
        assert_eq!(timeout, AP_TIMEOUT_MICROSECONDS);
        assert_ne!(number, 2);
        CALLS.fetch_add(1, Ordering::Relaxed);
        let mode = MODE.load(Ordering::Relaxed);
        if mode == 1 {
            return Status::TIMEOUT;
        }
        if mode == 2 {
            return Status::SUCCESS;
        }
        CURRENT.store(if mode == 7 { 2 } else { number }, Ordering::Relaxed);
        procedure(argument);
        if mode == 3 {
            procedure(argument);
        }
        CURRENT.store(2, Ordering::Relaxed);
        if mode == 15 && number == 1 {
            let record = unsafe { &*argument.cast::<Capture>() };
            assert!(
                matches!(unsafe{*record.result.get()},Err(Error::Admission(f)) if f.operation==4&&f.observed==0xfedcba9876543210)
            );
            return Status::TIMEOUT;
        }
        Status::SUCCESS
    }
    unsafe extern "efiapi" fn all(
        _: *const MpServicesProtocol,
        _: ApProcedure,
        _: Boolean,
        _: Event,
        _: usize,
        _: *mut c_void,
        _: *mut *mut usize,
    ) -> Status {
        panic!("StartupAllAPs must not be used")
    }
    unsafe extern "efiapi" fn switch(_: *const MpServicesProtocol, _: usize, _: Boolean) -> Status {
        panic!("BSP must not change")
    }
    unsafe extern "efiapi" fn enable(
        _: *const MpServicesProtocol,
        _: usize,
        _: Boolean,
        _: *const u32,
    ) -> Status {
        panic!("CPU enablement must not change")
    }
    fn identity() -> Result<Identity, Error> {
        let mode = MODE.load(Ordering::Relaxed);
        let current = CURRENT.load(Ordering::Relaxed);
        if ((mode == 13 || mode == 15) && current == 1) || (mode == 16 && current == 2) {
            let mut f =
                AdmissionFailure::new(4, 8, 0xc0010010, 0xfedcba9876543210, 0x123456789abcdef0, 48);
            f.apic_id = (current as u32 + 1) * 17;
            return Err(Error::Admission(f));
        }
        Ok(Identity {
            apic_id: (CURRENT.load(Ordering::Relaxed) + 1) as u32 * 17
                + u32::from(MODE.load(Ordering::Relaxed) == 9),
            vendor: *b"AuthenticAMD",
            physical_bits: 48,
            ..Identity::default()
        })
    }
    static MP: MpServicesProtocol = MpServicesProtocol {
        get_number_of_processors: counts,
        get_processor_info: info,
        startup_all_aps: all,
        startup_this_ap: startup,
        switch_bsp: switch,
        enable_disable_ap: enable,
        who_am_i: who,
    };

    #[test]
    fn returned_inventory_binds_sparse_ids_nonzero_bsp_and_refuses_incomplete_callbacks() {
        MODE.store(0, Ordering::Relaxed);
        COUNTS.store(0, Ordering::Relaxed);
        INFOS.store(0, Ordering::Relaxed);
        let inventory = unsafe { inspect_protocol(&MP, identity) }.unwrap();
        assert_eq!(inventory.bsp_number(), 2);
        assert_eq!(inventory.completed_ap_callbacks(), 2);
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
        for (number, processor) in inventory.processors().iter().enumerate() {
            assert_eq!(processor.firmware_number, number);
            assert_eq!(processor.identity.apic_id, (number as u32 + 1) * 17);
        }
        for (mode, predicate) in [
            (1, 9),
            (2, 10),
            (3, 10),
            (4, 8),
            (5, 5),
            (6, 7),
            (7, 12),
            // Broadcast startup may not use an enabled subset, a CPU-local
            // identity mismatch, or an inventory changed during observation.
            (8, 5),
            (9, 13),
            (10, 16),
            (11, 18),
            (12, 16),
            (13, 8),
            (14, 11),
            (15, 9),
            (16, 8),
        ] {
            MODE.store(mode, Ordering::Relaxed);
            COUNTS.store(0, Ordering::Relaxed);
            INFOS.store(0, Ordering::Relaxed);
            CALLS.store(0, Ordering::Relaxed);
            let Error::Admission(failure) = unsafe { inspect_protocol(&MP, identity) }.unwrap_err();
            assert_eq!(failure.predicate, predicate, "mode {mode}");
            if mode == 1 || mode == 15 {
                assert_eq!(failure.status, Status::TIMEOUT.0 as u64);
                assert_eq!(failure.operation, 2);
            }
            if mode == 14 {
                assert_eq!(failure.status, Status::DEVICE_ERROR.0 as u64);
                assert_eq!(failure.processor, 1);
            }
            if mode == 13 || mode == 16 {
                assert_eq!(failure.operation, 4);
                assert_eq!(failure.processor, if mode == 13 { 1 } else { 2 });
                assert_eq!(failure.apic_id, if mode == 13 { 34 } else { 51 });
                assert_eq!(failure.observed, 0xfedcba9876543210);
                assert_eq!(failure.expected, 0x123456789abcdef0);
                let contexts = failure.contexts(3);
                assert_eq!(contexts[2], failure.observed);
                assert_eq!(contexts[3], failure.expected);
            }
            assert_eq!(CURRENT.load(Ordering::Relaxed), 2);
            if mode == 8 {
                assert_eq!(CALLS.load(Ordering::Relaxed), 0);
            }
        }
    }
}
