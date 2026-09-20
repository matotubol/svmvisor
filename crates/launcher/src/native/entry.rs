//! Opt-in native preflight invoked at firmware image entry, before binding.
use svmvisor_card_abi::native_result::NativeResult;
use svmvisor_hypervisor::boot::preflight::{CpuidEvidence, CpuidRegisters};
use svmvisor_launcher::native::admission::preflight::{Outcome, collect};
use uefi_raw::{Handle, Status, table::system::SystemTable};

/// Caller provides the same live image/system table and TPL_APPLICATION entry
/// contract as efi_main. Temporary observation buffers are released; no driver
/// binding or SVM state is installed.
#[cfg_attr(feature = "native-returning", inline(always))]
pub(crate) unsafe fn run(
    image: Handle,
    table: *const SystemTable,
    capture: &svmvisor_launcher::native::admission::boundary::NativeBoundary,
) -> Status {
    if image.is_null() || table.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let table = unsafe { &*table };
    if table.boot_services.is_null() {
        return Status::UNSUPPORTED;
    }
    let mailbox = match unsafe {
        crate::native_child_result::Mailbox::attach(image, &*table.boot_services)
    } {
        Ok(mailbox) => mailbox,
        Err(_) => return Status::UNSUPPORTED,
    };
    // Default refusal until an admitted caller supplies actual observations.
    let mut inner_result = NativeResult::new();
    inner_result.outcome = 1;
    inner_result.refusal = 0x1000;
    for (field, value) in [
        ("entry-profile", capture.profile),
        ("entry-xstate-bytes", capture.xstate_size),
        ("entry-xstate-captured", 1),
        ("entry-cr2", capture.cr2),
        ("entry-cr8", capture.cr8),
    ] {
        unsafe { snapshot_line(table, field, value) };
    }
    let (evidence, outcome) = unsafe { collect_boot_identity(table) };
    if outcome == Outcome::NativeBoundaryUnavailable {
        let physical_bits = evidence.address_width.map(|r| r.eax as u8).unwrap_or(0);
        let page1gb = evidence.extended_features.is_some_and(|r| r.edx & (1 << 26) != 0);
        #[cfg(feature = "native-returning")]
        {
            inner_result = unsafe {
                crate::native_returning::run(image, table, capture, physical_bits, page1gb)
            };
        }
        #[cfg(not(feature = "native-returning"))]
        {
            #[cfg(feature = "native-resource-observe")]
            let resources_observed = unsafe {
                crate::native_resources::observe(image, table, capture, physical_bits, page1gb)
            };
            let owned_tables_clean = unsafe { observe_owned_tables(table, physical_bits, page1gb) };
            #[cfg(feature = "native-transition-test")]
            {
                inner_result = unsafe {
                    crate::native_transition_fixture::run(table, capture, physical_bits, page1gb)
                };
                if !owned_tables_clean {
                    inner_result.cleanup_complete = 0;
                }
            }
            #[cfg(not(feature = "native-transition-test"))]
            {
                inner_result.cleanup_complete = u64::from(owned_tables_clean);
            }
            #[cfg(feature = "native-resource-observe")]
            if !resources_observed {
                inner_result.cleanup_complete = 0;
            }
            match unsafe { svmvisor_launcher::native::admission::snapshot::capture() } {
                Ok(snapshot) => {
                    for (field, value) in [
                        ("cr0", snapshot.cr0),
                        ("cr3", snapshot.cr3),
                        ("cr4", snapshot.cr4),
                        ("rflags", snapshot.rflags),
                        ("gdtr-base", snapshot.gdtr.base()),
                        ("gdtr-limit", u64::from(snapshot.gdtr.limit())),
                        ("idtr-base", snapshot.idtr.base()),
                        ("idtr-limit", u64::from(snapshot.idtr.limit())),
                        ("cs", u64::from(snapshot.cs)),
                        ("ss", u64::from(snapshot.ss)),
                        ("ds", u64::from(snapshot.ds)),
                        ("es", u64::from(snapshot.es)),
                    ] {
                        unsafe { snapshot_line(table, field, value) };
                    }
                }
                Err(_) => {
                    // Observational refusal only. This never reaches an entry or
                    // dereferences a table on failed privilege admission.
                    unsafe { snapshot_line(table, "refused", 1) };
                }
            }
        }
    } else {
        // CPUID rejection occurs before any owned observation/probe resource.
        // Mailbox attachment has already closed its LoadedImage protocol use.
        inner_result.refusal = u64::from(outcome.diagnostic_code());
        inner_result.cleanup_complete = 1;
    }
    #[cfg(not(feature = "native-returning"))]
    {
        let message = match outcome {
            Outcome::CpuidRejected(_) => {
                b"SVMVISOR native-preflight CPUID refused code=".as_slice()
            }
            Outcome::NativeBoundaryUnavailable => {
                b"SVMVISOR native-preflight native boundary unavailable code=".as_slice()
            }
        };
        // A transient console diagnostic, explicitly not persistent card evidence.
        // Failure to print never changes the refusal or invokes a fallback entry.
        if !table.stdout.is_null() {
            let mut text = [0u16; 96];
            for (dst, src) in text.iter_mut().zip(message) {
                *dst = u16::from(*src);
            }
            let code = outcome.diagnostic_code();
            for i in 0..8 {
                text[message.len() + i] =
                    u16::from(b"0123456789abcdef"[((code >> (28 - i * 4)) & 15) as usize]);
            }
            text[message.len() + 8] = 13;
            text[message.len() + 9] = 10;
            unsafe {
                let _ = ((*table.stdout).output_string)(table.stdout, text.as_ptr());
            }
        }
    }
    #[cfg(feature = "native-returning")]
    if matches!(outcome, Outcome::CpuidRejected(_)) {
        unsafe { snapshot_line(table, "native-returning-cpuid-refused", inner_result.refusal) };
    }
    if let Some(mailbox) = mailbox {
        unsafe { mailbox.complete(inner_result) };
    }
    Status::UNSUPPORTED
}

pub(crate) unsafe fn snapshot_line(table: &SystemTable, field: &str, value: u64) {
    if table.stdout.is_null() {
        return;
    }
    let mut text = [0u16; 96];
    let bytes = b"SVMVISOR snapshot "
        .iter()
        .copied()
        .chain(field.bytes())
        .chain([b'='])
        .chain(
            (0..16).rev().map(|shift| b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]),
        )
        .chain([13, 10]);
    for (dst, byte) in text.iter_mut().zip(bytes) {
        *dst = u16::from(byte);
    }
    unsafe {
        let _ = ((*table.stdout).output_string)(table.stdout, text.as_ptr());
    }
}

/// Read and print retained BSP identity outside the large returning owner frame.
/// Keeping this call out of line releases identity temporaries before the later
/// native preparation/HIGH scope; the original feature evidence and outcome
/// alone survive into that scope.
///
/// # Safety
/// The BSP is at TPL_APPLICATION with live boot services. `table` and any
/// non-null console protocol/output callback it names remain valid throughout
/// this call; firmware console calls are permitted and no guest is running.
/// UEFI 2.11 sections 4.3/12.4 define the system table and text output protocol;
/// AMD PPR 57896 rev.3.00 section2.1.12 defines these read-only CPUID leaves.
#[inline(never)]
unsafe fn collect_boot_identity(table: &SystemTable) -> (CpuidEvidence, Outcome) {
    let report = collect(|leaf| {
        let r = core::arch::x86_64::__cpuid_count(leaf, 0);
        CpuidRegisters { eax: r.eax, ebx: r.ebx, ecx: r.ecx, edx: r.edx }
    });
    // Read-only native BSP boot capture. Diagnostics consume the retained
    // identity, never re-query hardware and never influence admission. This
    // is transient console evidence, not native guest-model installation.
    match report.identity {
        Ok(identity) => {
            unsafe {
                snapshot_line(table, "boot-cpu-identity-available", 1);
                snapshot_line(table, "boot-cpu-signature", u64::from(identity.signature()));
            }
            let brand = identity.brand();
            for (field, bytes) in [
                "boot-cpu-brand-0",
                "boot-cpu-brand-1",
                "boot-cpu-brand-2",
                "boot-cpu-brand-3",
                "boot-cpu-brand-4",
                "boot-cpu-brand-5",
            ]
            .into_iter()
            .zip(brand.chunks_exact(8))
            {
                // Preserve all 48 bytes, including padding and non-UTF8 bytes.
                // Fixed shifts avoid dynamic indexing or fallible conversions
                // in this image's no-panic native observation path.
                let value = bytes
                    .iter()
                    .zip([0, 8, 16, 24, 32, 40, 48, 56])
                    .fold(0u64, |word, (&byte, shift)| word | (u64::from(byte) << shift));
                unsafe { snapshot_line(table, field, value) };
            }
        }
        Err(_) => unsafe { snapshot_line(table, "boot-cpu-identity-unavailable", 1) },
    }
    (report.evidence, report.outcome)
}

/// Table preparation, final comparison and release remain under one NOTIFY
/// scope; only the final comparison executes at HIGH. Diagnostics run afterward.
#[cfg(not(feature = "native-returning"))]
unsafe fn observe_owned_tables(table: &SystemTable, physical_bits: u8, page1gb: bool) -> bool {
    use crate::native_tables::{self, TableError};
    use core::{cell::Cell, convert::Infallible};
    use svmvisor_launcher::native::admission::{cpu as native_cpu, snapshot as native_snapshot};
    let services = unsafe { &*table.boot_services };
    let Ok(mut cpus) = (unsafe { native_cpu::prepare(services) }) else {
        unsafe { snapshot_line(table, "cpu-refused", 1) };
        return false;
    };
    let table_preparation_error = Cell::new(None);
    let completion = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || {
                let prepared = native_tables::prepare(services, physical_bits, page1gb);
                table_preparation_error.set(prepared.as_ref().err().copied());
                Ok::<_, Infallible>(prepared)
            },
            |guard, prepared| {
                let snapshot = native_snapshot::capture();
                let observed = match prepared {
                    Ok(tables) => tables.revalidate(guard).and_then(|report| {
                        Ok((
                            report,
                            tables.retained_entry_count()?,
                            tables.retained_table_page_count()?,
                        ))
                    }),
                    Err(error) => Err(*error),
                };
                (snapshot, observed)
            },
            |prepared| match prepared {
                Ok(tables) => tables.release(),
                Err(_) => Ok(()),
            },
        )
    };
    let table_failure = if completion.as_ref().is_ok_and(|done| done.cleanup.is_err()) {
        Some(TableError::Cleanup)
    } else {
        table_preparation_error.get()
    };
    if cpus.release().is_err() {
        if let Some(error) = table_failure {
            unsafe { snapshot_line(table, "tables-refused", error as u64) };
        }
        unsafe { snapshot_line(table, "cpu-refused", 2) };
        return false;
    }
    let completion = match completion {
        Ok(value) => value,
        Err(native_cpu::PreparedScopeError::Preparation(never)) => match never {},
        Err(native_cpu::PreparedScopeError::Cpu(_)) => {
            if let Some(error) = table_failure {
                unsafe { snapshot_line(table, "tables-refused", error as u64) };
            }
            unsafe { snapshot_line(table, "cpu-refused", 3) };
            return false;
        }
    };
    let Ok((cpus, (Ok(scoped), observed))) = completion.outcome else {
        if let Some(error) = table_failure {
            unsafe { snapshot_line(table, "tables-refused", error as u64) };
        }
        unsafe { snapshot_line(table, "cpu-refused", 4) };
        return false;
    };
    for (field, value) in [
        ("cpu-total", cpus.total_processors as u64),
        ("cpu-enabled", cpus.enabled_processors as u64),
        ("cpu-bsp-number", cpus.bsp_number as u64),
        ("cpu-bsp-id", cpus.bsp_processor_id),
        ("cpu-ap-completed", cpus.completed_ap_callbacks as u64),
        ("cpu-probe-processors", cpus.probe_processors as u64),
        ("cpu-observed", 1),
        ("cpu-scoped-rflags", scoped.rflags),
        ("cpu-scoped-complete", 1),
    ] {
        unsafe { snapshot_line(table, field, value) };
    }
    let observed = if completion.cleanup.is_err() { Err(TableError::Cleanup) } else { observed };
    match observed {
        Ok((observed, entries, pages)) => {
            for (field, value) in [
                ("tables-descriptors", observed.descriptors as u64),
                ("tables-pages", observed.pages as u64),
                ("tables-reads", observed.reads as u64),
                ("tables-gdt-bytes", observed.gdt_bytes as u64),
                ("tables-retained-entries", entries as u64),
                ("tables-retained-pages", pages as u64),
                ("tables-scoped-complete", 1),
                ("tables-complete", 1),
            ] {
                unsafe { snapshot_line(table, field, value) };
            }
            true
        }
        Err(error) => {
            unsafe { snapshot_line(table, "tables-refused", error as u64) };
            false
        }
    }
}
