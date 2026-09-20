//! Optional card handoff and BSP-only boot journal, UEFI2.11 7.4.2/9.1.1/14.4.
//! The pre-loader journal owner ends before returning to the Windows loader.
//! Optional terminal diagnostics copy separately validated numeric PCI provenance
//! to the core, which owns stopped-state revalidation and the sole terminal writer.
use svmvisor_card_abi::{
    boot_options::ResidentBootOptions,
    endpoint::{self as card_endpoint, PCI_CLASS_REVISION, PCI_VENDOR_DEVICE, TerminalEndpoint},
    journal::{self as card_journal, JournalIo},
};
use svmvisor_hypervisor::arch::x86_64::msr::MMIO_CFG_BASE_ADDR;
use svmvisor_launcher::diagnostics::resident_boot::{ApFailureObservation, ap_failure_words};

use super::*;

static mut JOURNAL: Option<(u64, u32)> = None;
// Written only by serialized pre-EBS child preparation, then immutable. ArmRuntime
// copies this value per CPU before changing roots and retains no DXE pointer.
static mut TERMINAL: Option<TerminalEndpoint> = None;
static JOURNAL_LOST: AtomicBool = AtomicBool::new(false);
// Only the serialized BSP StartImage call writes this record. It is copied
// into caller-owned options before that call returns, never used by VM exits.
static mut PREPARATION: (u32, u32, u64, u64) = (0, 0, 0, 0);
static mut ADMISSION_FAILURE: Option<(
    svmvisor_launcher::diagnostics::resident_boot::AdmissionFailure,
    u32,
)> = None;
// Exclusively owned by the serialized BSP activation before loader return.
// AP callbacks and resident VM-exit paths never access these records.
static mut BSP_TAKEOVER_FAILURE: Option<[u32; 3]> = None;
// BSP copies this only after acquiring the AP failed-bit publication.
static mut AP_FAILURE: Option<[u32; 3]> = None;

pub(super) struct Prepared(*mut ResidentBootOptions);

impl Prepared {
    /// Copy options under the current validated mapping. Only the StartImage
    /// call retains this pointer; the later EBS path retains numeric inputs.
    pub(super) unsafe fn new(
        image: Handle,
        boot_services: &BootServices,
    ) -> Result<Option<Self>, Status> {
        let mut raw = ptr::null_mut();
        let status =
            unsafe { (boot_services.handle_protocol)(image, &LoadedImageProtocol::GUID, &mut raw) };
        if status != Status::SUCCESS {
            return Err(status);
        }
        let loaded =
            unsafe { raw.cast::<LoadedImageProtocol>().as_ref() }.ok_or(Status::DEVICE_ERROR)?;
        let bytes = loaded.load_options_size as usize;
        let options = loaded.load_options.cast::<ResidentBootOptions>().cast_mut();
        if bytes == 0 && options.is_null() {
            return Ok(None);
        }
        if bytes != core::mem::size_of::<ResidentBootOptions>()
            || options.is_null()
            || options as usize & 7 != 0
        {
            return Err(Status::COMPROMISED_DATA);
        }
        let processor = unsafe { cpu() }.map_err(unsupported)?;
        let cfg = unsafe { config(processor) }.map_err(unsupported)?;
        let mt = unsafe { mtrrs(processor.physical_bits) }.map_err(unsupported)?;
        let pat = unsafe { rdmsr(PAT) };
        let mut map =
            unsafe { memory::collect(boot_services) }.map_err(|_| Status::OUT_OF_RESOURCES)?;
        let checked = (|| {
            unsafe {
                mapped(map.descriptors(), cfg, &mt, pat, options as u64, bytes as u64, true, false)
            }
            .map_err(unsupported)?;
            let value = unsafe { options.read() };
            if !value.is_valid_header()
                || value.rust_entered != 0
                || value.armed != 0
                || value.failure != 0
                || !value.preparation_empty()
            {
                return Err(Status::COMPROMISED_DATA);
            }
            unsafe {
                ptr::addr_of_mut!((*options).rust_entered).write_volatile(1);
            }
            unsafe { validate_uc_mmio(map.descriptors(), cfg, &mt, pat, value.journal_base) }
                .map_err(unsupported)?;
            let mut io = Direct(value.journal_base);
            if io.read(0)? != 0x4a4d5653
                || io.read(4)? & !0x00020000 != 0x00010001
                || io.read(0x24)? != 0
            {
                return Err(Status::DEVICE_ERROR);
            }
            unsafe {
                JOURNAL = Some((value.journal_base, value.boot_id));
                TERMINAL = None;
            }
            if let Some(endpoint) = value.terminal_endpoint() {
                // Failure disables only this optional observer. A malformed
                // descriptor was already rejected by is_valid_header above.
                if unsafe {
                    validate_uc_mmio(map.descriptors(), cfg, &mt, pat, endpoint.config_page)
                }
                .is_ok()
                    && unsafe { terminal_config_matches(endpoint) }
                    && (u64::from(io.read(8)?) | (u64::from(io.read(12)?) << 32))
                        == endpoint.fpga_build_id
                    && (u64::from(io.read(16)?) | (u64::from(io.read(20)?) << 32))
                        == endpoint.rom_build_id
                {
                    unsafe {
                        TERMINAL = Some(endpoint);
                    }
                }
            }
            Ok(Self(options))
        })();
        map.release()?;
        checked.map(Some)
    }

    pub(super) fn complete(self, result: Status, armed: bool) {
        if !armed {
            unsafe {
                publish_admission_failure();
            }
        }
        // Same BSP StartImage invocation; parent keeps options immutable except
        // these child acknowledgement fields and does not race this call.
        unsafe {
            if !armed && (*self.0).version >= 2 {
                let (stage, reason, status, address) = PREPARATION;
                ptr::addr_of_mut!((*self.0).preparation_stage).write_volatile(stage);
                ptr::addr_of_mut!((*self.0).preparation_reason).write_volatile(reason);
                ptr::addr_of_mut!((*self.0).preparation_status).write_volatile(if reason == 0 {
                    result.0 as u64
                } else {
                    status
                });
                ptr::addr_of_mut!((*self.0).preparation_address).write_volatile(address);
            }
            ptr::addr_of_mut!((*self.0).failure).write_volatile(if armed {
                0
            } else {
                result.0 as u64
            });
            ptr::addr_of_mut!((*self.0).armed).write_volatile(u32::from(armed));
        }
    }
}

struct Direct(u64);

impl JournalIo for Direct {
    type Error = Status;
    fn read(&mut self, offset: u64) -> Result<u32, Status> {
        if offset > 0x9c || offset & 3 != 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        Ok(unsafe { ((self.0 + offset) as *const u32).read_volatile() })
    }
    fn write(&mut self, offset: u64, value: u32) -> Result<(), Status> {
        if (!(0x40..=0x60).contains(&offset) && !(0x600..=0xffc).contains(&offset))
            || offset & 3 != 0
        {
            return Err(Status::INVALID_PARAMETER);
        }
        unsafe {
            ((self.0 + offset) as *mut u32).write_volatile(value);
        }
        Ok(())
    }
}

/// Apply endpoint lifetime protection to each rebuilt private NPT. The
/// descriptor is written during serialized preparation and immutable thereafter.
pub(super) fn protect_config(
    npt: &mut svmvisor_hypervisor::memory::npt::IdentityNpt<'_>,
) -> Result<(), svmvisor_hypervisor::memory::npt::IdentityNptError> {
    if let Some(endpoint) = unsafe { *ptr::addr_of!(TERMINAL) } {
        let (base, bytes) = endpoint
            .config_aperture()
            .ok_or(svmvisor_hypervisor::memory::npt::IdentityNptError::InvalidExclusion)?;
        npt.protect_write_range(base, bytes)?;
    }
    Ok(())
}

/// Serialized BSP only, after MP completion or post-EBS survey collection has
/// been resolved. USER3 carries exact full operands; the preparation or
/// activation result identifies which ownership boundary refused execution.
pub(super) fn admission_failure(
    value: svmvisor_launcher::diagnostics::resident_boot::AdmissionFailure,
    count: u32,
) {
    unsafe {
        ADMISSION_FAILURE = Some((value, count));
    }
    preparation_failure(
        32,
        Status::UNSUPPORTED.0 as u64,
        (value.operation as u64) << 32 | value.predicate as u64,
    );
}

/// Stages 16/17: the serialized BSP has not retained CPU/MAP yet, so the
/// refused slot's record is published at once through the caller's current
/// collected map. `reason` 33-36 selects the preparation record, whose status
/// is the source's exact code and whose address packs slot, operation and
/// predicate (`slot_preparation_address`).
pub(super) unsafe fn slot_admission_failure(
    value: svmvisor_launcher::diagnostics::resident_boot::AdmissionFailure,
    count: u32,
    reason: u32,
    processor: Cpu,
    map: &[MemoryDescriptor],
) {
    preparation_failure(
        reason,
        value.status,
        svmvisor_launcher::diagnostics::resident_boot::slot_preparation_address(
            value.processor,
            value.operation,
            value.predicate,
        ),
    );
    unsafe {
        ADMISSION_FAILURE = Some((value, count));
        publish_admission_failure_with(Some(processor), map);
        ADMISSION_FAILURE = None;
    }
}

/// Serialized BSP activation only, before returning to the loader.
pub(super) unsafe fn takeover_failure(slot: u32, count: u32, code: u64) {
    unsafe {
        BSP_TAKEOVER_FAILURE =
            svmvisor_launcher::diagnostics::resident_boot::takeover_failure_words(
                slot, count, code,
            );
    }
}

/// # Safety
/// Serialized BSP startup only, before loader return. The caller must acquire
/// the matching AP's failed bit before reading its final private BOOT record.
pub(super) unsafe fn ap_failure(slot: u32, count: u32, sample: ApFailureObservation) {
    unsafe { AP_FAILURE = ap_failure_words(slot, count, sample) };
}

/// # Safety
/// Same BSP-only, validated pre-loader journal access contract as stage.
pub(super) unsafe fn activation_failure(result: u64) {
    if result == 48 {
        // The post-EBS survey happens after Prepared.complete returned. Publish
        // its retained operands now through the same freshly validated BSP-only
        // transport, before committing the ordinary activation failure header.
        unsafe {
            publish_admission_failure();
        }
    }
    if result == 35 {
        if let Some(words) = unsafe { BSP_TAKEOVER_FAILURE } {
            unsafe { commit_words(words) };
            return;
        }
    }
    if result == 33 {
        if let Some(words) = unsafe { AP_FAILURE } {
            unsafe { commit_words(words) };
            return;
        }
    }
    unsafe { stage(0x80, 0, result as u32) };
}

pub(super) fn preparation_step(stage: u32, address: u64) {
    unsafe {
        PREPARATION = (stage, 0, 0, address);
    }
}

pub(super) fn preparation_failure(reason: u32, status: u64, address: u64) {
    unsafe {
        PREPARATION = (PREPARATION.0, reason, status, address);
    }
}

/// BSP only, before returning to the OS loader; every AP is still in owned
/// bootstrap/wait code. No firmware call, allocation or shared transport lock.
/// A failed access ends observation for this boot, never retried indefinitely.
pub(super) unsafe fn stage(stage: u32, slot: u32, detail: u32) {
    // Stage 5 follows every successful CPU activation: version 1 in bit 24,
    // bit 0 reports the validated endpoint copied by those ArmRuntime calls.
    // This is preparation evidence, not a promise of future PCI/BAR availability.
    // Legacy stage-5 detail 0 means the endpoint state was not reported.
    let detail =
        if stage == 5 { 0x0100_0000 | u32::from(!terminal_endpoint().is_null()) } else { detail };
    unsafe { commit_words([stage | (slot << 16), detail, CPU_COUNT as u32]) };
}

pub(super) fn terminal_endpoint() -> *const TerminalEndpoint {
    unsafe {
        match &*ptr::addr_of!(TERMINAL) {
            Some(endpoint) => endpoint as *const TerminalEndpoint,
            None => ptr::null(),
        }
    }
}

unsafe fn publish_admission_failure() {
    let map = unsafe { core::slice::from_raw_parts(ptr::addr_of!(MAP).cast(), MAP_COUNT) };
    unsafe { publish_admission_failure_with(CPU, map) };
}

unsafe fn publish_admission_failure_with(processor: Option<Cpu>, map: &[MemoryDescriptor]) {
    let Some((value, count)) = (unsafe { ADMISSION_FAILURE }) else {
        return;
    };
    let Some(endpoint) = (unsafe { TERMINAL }) else {
        return;
    };
    if !(1..=32).contains(&count) || JOURNAL_LOST.load(Ordering::Acquire) {
        return;
    }
    let result = (|| -> Result<(), Status> {
        let processor = processor.ok_or(Status::NOT_READY)?;
        let cfg = unsafe { config(processor) }.map_err(unsupported)?;
        let mt = unsafe { mtrrs(processor.physical_bits) }.map_err(unsupported)?;
        let pat = unsafe { rdmsr(PAT) };
        for page in [endpoint.config_page, endpoint.bar0_host_page] {
            unsafe { validate_uc_mmio(map, cfg, &mt, pat, page) }.map_err(unsupported)?;
        }
        if !unsafe { terminal_config_matches(endpoint) } {
            return Err(Status::DEVICE_ERROR);
        }
        let mut io = Direct(endpoint.bar0_host_page);
        if io.read(0)? != 0x4a4d5653
            || io.read(4)? != 0x00030001
            || (io.read(8)? as u64 | ((io.read(12)? as u64) << 32)) != endpoint.fpga_build_id
            || (io.read(16)? as u64 | ((io.read(20)? as u64) << 32)) != endpoint.rom_build_id
            || io.read(0x84)? != endpoint.boot_id
        {
            return Err(Status::DEVICE_ERROR);
        }
        let low: u32;
        let high: u32;
        unsafe {
            asm!("rdtsc",out("eax")low,out("edx")high,options(nostack,preserves_flags));
        }
        let payload = card_journal::diagnostic_payload(
            1,
            12,
            true,
            endpoint.boot_id,
            value.apic_id,
            low as u64 | ((high as u64) << 32),
            value.contexts(count),
            value.operation,
        );
        // No resident bank has been published before admission. Unknown failing
        // identity uses the known BSP transport bank; the payload stays unknown.
        let bank = if value.processor < count {
            value.processor as usize
        } else {
            unsafe { physical_boot::bsp_slot() }
        };
        card_journal::commit_diagnostic(&mut io, bank, payload).map_err(|_| Status::DEVICE_ERROR)
    })();
    if result.is_err() {
        JOURNAL_LOST.store(true, Ordering::Release);
    }
}

/// Same bounded BSP-only lifetime as stage; every path shares access validation
/// and permanent loss accounting, including the precise AP and takeover records.
unsafe fn commit_words(words: [u32; 3]) {
    let Some((base, boot_id)) = (unsafe { JOURNAL }) else {
        return;
    };
    if JOURNAL_LOST.load(Ordering::Acquire) {
        return;
    }
    let result = (|| -> Result<(), Status> {
        let processor = unsafe { CPU }.ok_or(Status::NOT_READY)?;
        let cfg = unsafe { config(processor) }.map_err(unsupported)?;
        let mt = unsafe { mtrrs(processor.physical_bits) }.map_err(unsupported)?;
        let map = unsafe { core::slice::from_raw_parts(ptr::addr_of!(MAP).cast(), MAP_COUNT) };
        unsafe { validate_uc_mmio(map, cfg, &mt, rdmsr(PAT), base) }.map_err(unsupported)?;
        let mut io = Direct(base);
        if io.read(0)? != 0x4a4d5653 || io.read(4)? & !0x00020000 != 0x00010001 {
            return Err(Status::DEVICE_ERROR);
        }
        let sequence = io.read(0x2c)?.wrapping_add(1);
        let low: u32;
        let high: u32;
        unsafe {
            asm!("rdtsc", out("eax") low, out("edx") high, options(nostack, preserves_flags));
        }
        card_journal::commit_record(
            &mut io,
            [sequence, boot_id, low, high, words[0], words[1], words[2], 0x0008_0013],
        )
        .map_err(|_| Status::DEVICE_ERROR)
    })();
    if result.is_err() {
        JOURNAL_LOST.store(true, Ordering::Release);
    }
}

/// Called only after validating the UC function page in the firmware mapping.
/// The exact CPU gate precedes the processor-specific MSR read (PPR57896 p210).
unsafe fn terminal_config_matches(endpoint: TerminalEndpoint) -> bool {
    let vendor = core::arch::x86_64::__cpuid(0);
    if vendor.ebx != 0x6874_7541
        || vendor.edx != 0x6974_6e65
        || vendor.ecx != 0x444d_4163
        || vendor.eax < 1
        || core::arch::x86_64::__cpuid(1).eax != TARGET_SIGNATURE
        || unsafe { rdmsr(MMIO_CFG_BASE_ADDR) } != endpoint.mmio_config_msr
    {
        return false;
    }
    // PPR2.1.6.1 requires UC, aligned DWORDs and mov eax,[address].
    let read = |offset| unsafe { card_endpoint::read_config_dword(endpoint.config_page, offset) };
    read(0) == PCI_VENDOR_DEVICE
        && read(8) == PCI_CLASS_REVISION
        && ((read(0x0c) >> 16) & 0xff) == 0
        && read(4) as u16 == endpoint.command
        && read(0x10) == endpoint.bar0_raw
}
