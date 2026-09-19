//! Resident driver wrapper. Driver callbacks serialize all access to STATE.

use core::ptr;

use svmvisor_card_abi::boot_options::ResidentBootOptions;
#[cfg(not(feature = "card-resident-dev-loader"))]
use svmvisor_card_loader::delivery::child_image::Pin;
#[cfg(feature = "card-returning-loader")]
use svmvisor_card_loader::diagnostics::returning_detail::ReturningDiagnostics;
use svmvisor_card_loader::{
    delivery::child_image::{self as card_returning, State},
    diagnostics::journal::{self, JournalIo},
};
use uefi_raw::{Handle, Status, table::boot::BootServices};

use crate::pci_io::Bar0;

// The dev loader compiles in no header: it adopts the one in the flash slot.
#[cfg(not(feature = "card-resident-dev-loader"))]
const PIN: &[u8; 128] = include_bytes!(concat!(env!("OUT_DIR"), "/card-pe-header.bin"));

static mut STATE: State = State::new();
#[cfg(feature = "card-returning-loader")]
static mut RESULT_BITS: u32 = 0;
#[cfg(feature = "card-returning-loader")]
static mut DIAGNOSTICS: Option<ReturningDiagnostics> = None;
static mut ATTEMPTED: bool = false;
#[cfg(feature = "card-resident")]
static RETAINED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "card-resident")]
static DELIVERY_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Called only inside the driver's non-reentrant callback guard.
pub(crate) fn cleanup(services: &BootServices) -> Result<(), Status> {
    unsafe { (&mut *ptr::addr_of_mut!(STATE)).cleanup(services) }
}

#[cfg(feature = "card-returning-loader")]
pub(crate) fn result_bits() -> u32 {
    // Copied before lifecycle callbacks are armed; no borrow crosses a service.
    unsafe { RESULT_BITS }
}

#[cfg(feature = "card-returning-loader")]
pub(crate) fn diagnostics() -> Option<ReturningDiagnostics> {
    // A value copy only; no child-owned memory or borrow crosses a service.
    unsafe { DIAGNOSTICS }
}

#[cfg(feature = "card-returning-loader")]
pub(crate) fn execute(
    io: &mut Bar0,
    services: &BootServices,
    parent: Handle,
    controller: Handle,
    boot_id: u32,
    tsc: u64,
) -> Result<(), Status> {
    if has_attempted() {
        return Err(Status::UNSUPPORTED);
    }
    // One attempt for this loaded parent image, including refused/failed Start.
    // Stop only releases ownership; it never rearms the returning experiment.
    unsafe { ATTEMPTED = true };
    let pin = Pin::parse(PIN)?;
    let report = unsafe {
        card_returning::execute(
            &mut *ptr::addr_of_mut!(STATE),
            services,
            parent,
            controller,
            &pin,
            |offset| io.card_word(offset),
        )
    };
    let diagnostics = ReturningDiagnostics::capture(&report);
    let bits = diagnostics.result_bits();
    unsafe {
        RESULT_BITS = bits;
        DIAGNOSTICS = Some(diagnostics);
    }
    let sequence = io.read(0x02c)?.wrapping_add(1);
    // Detail 7 has the same diagnostic layout here and in every lifecycle
    // notification. Capture precedes publication, without another child attempt.
    journal::commit(io, diagnostics.immediate_record(sequence, boot_id, tsc))?;
    if report.status() != Status::SUCCESS {
        return Err(report.status());
    }
    // A restored returning refusal still permits ordinary firmware boot. A
    // malformed/incomplete inner result remains a delivery failure.
    if bits == 1 << 15 { Err(Status::PROTOCOL_ERROR) } else { Ok(()) }
}

#[cfg(feature = "card-resident")]
pub(crate) fn journal_owned_by_child() -> bool {
    DELIVERY_ACTIVE.load(core::sync::atomic::Ordering::Acquire) || is_retained()
}

#[cfg(feature = "card-resident")]
pub(crate) fn execute_resident(
    io: &mut Bar0,
    services: &BootServices,
    parent: Handle,
    controller: Handle,
    boot_id: u32,
    journal_base: u64,
) -> Result<(), Status> {
    if has_attempted() {
        return Err(Status::UNSUPPORTED);
    }
    unsafe {
        ATTEMPTED = true;
    }
    #[cfg(not(feature = "card-resident-dev-loader"))]
    let pin = Pin::parse_resident(PIN)?;
    // Unsupported PCI/configuration provenance disables this optional terminal
    // observer. It does not change native boot admission or firmware decoding.
    let mut options = ResidentBootOptions::new(journal_base, boot_id);
    if let Ok(endpoint) = io.terminal_endpoint(journal_base, boot_id) {
        options = options.with_terminal(endpoint).unwrap_or(options);
    }
    // A TPL_NOTIFY lifecycle callback may interrupt StartImage or the final
    // parent record. Suppress its journal writes for this entire interval.
    DELIVERY_ACTIVE.store(true, core::sync::atomic::Ordering::Release);
    #[cfg(not(feature = "card-resident-dev-loader"))]
    let report = unsafe {
        card_returning::execute_resident(
            &mut *ptr::addr_of_mut!(STATE),
            services,
            parent,
            controller,
            &pin,
            options,
            |offset| io.card_word(offset),
        )
    };
    // Dev loader: the slot's own header is validated and adopted as the pin.
    #[cfg(feature = "card-resident-dev-loader")]
    let report = unsafe {
        card_returning::execute_resident_dev(
            &mut *ptr::addr_of_mut!(STATE),
            services,
            parent,
            controller,
            options,
            |offset| io.card_word(offset),
        )
    };
    // The lifecycle callback may run during StartImage, while execute owns
    // &mut STATE. It reads only this separate atomic, never aliases that borrow.
    RETAINED.store(
        unsafe { (&*ptr::addr_of!(STATE)).is_retained() },
        core::sync::atomic::Ordering::Release,
    );
    let observed = unsafe { (&*ptr::addr_of!(STATE)).resident_options() };
    let record = (|| {
        let sequence = io.read(0x02c)?.wrapping_add(1);
        let options = observed.unwrap_or(ResidentBootOptions::new(journal_base, boot_id));
        let status = report.status().0 as u64;
        if let Some([metadata, underlying, address]) = options.preparation_words() {
            // Detail8 phase0x14: failed child preparation. Unlike phase0x10,
            // metadata packs stage/reason/address-high, then exact compressed
            // EFI status and address-low. No armed hook is implied.
            return journal::commit(
                io,
                [
                    sequence,
                    boot_id,
                    status as u32,
                    (status >> 32) as u32,
                    metadata,
                    underlying,
                    address,
                    0x0008_0014,
                ],
            );
        }
        let selected_error = if options.failure != 0 { options.failure } else { status };
        // Detail 8, phase0x10 is parent load/arm evidence, never resident entry.
        // Word4: delivery stage, entered, armed, status kind, and bit11 set by
        // the dev loader (slot-supplied header); word5/6: exact failure/status.
        journal::commit(
            io,
            [
                sequence,
                boot_id,
                status as u32,
                (status >> 32) as u32,
                report.stage
                    | (options.rust_entered.min(1) << 8)
                    | (u32::from(report.status() == Status::SUCCESS && options.is_armed()) << 9)
                    | (u32::from(options.failure == 0) << 10)
                    | (u32::from(cfg!(feature = "card-resident-dev-loader")) << 11),
                selected_error as u32,
                (selected_error >> 32) as u32,
                0x0008_0010,
            ],
        )
    })();
    DELIVERY_ACTIVE.store(false, core::sync::atomic::Ordering::Release);
    if is_retained() {
        // An installed or possibly installed hook outlives callback failure.
        // Preserve binding/decode even when publication or acknowledgement failed.
        // The report above distinguishes those failures from successful arming.
        Ok(())
    } else {
        record?;
        if report.status() == Status::SUCCESS { Ok(()) } else { Err(report.status()) }
    }
}

pub(crate) fn has_attempted() -> bool {
    unsafe { ATTEMPTED }
}

#[cfg(feature = "card-resident")]
pub(crate) fn is_retained() -> bool {
    RETAINED.load(core::sync::atomic::Ordering::Acquire)
}
