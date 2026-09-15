//! Returning native probe under one retained resource/CPU observation scope.
//!
//! The final linked image must pass tools/native-stack-audit before packaging.
//! Allocation bootstrap, ordinary UEFI identity/lifetime, AP/handler cooperation
//! and the transition's admitted native execution contract remain prerequisites.
use core::cell::Cell;
use svmvisor_dxe::{
    native_boundary::NativeBoundary,
    native_cache_rendezvous::{self, RendezvousError},
    native_cpu::{self, CpuError, PreparedScopeError, QuiescentBsp},
    native_result::{MULTI_EXIT_ENTRIES, MULTI_EXIT_OUTCOME, NativeResult},
    native_transition::{guest_capture, mode, multi, outcome},
    native_transition_canary::svmvisor_native_transition_canary,
};
use uefi_raw::{Handle, table::system::SystemTable};

use crate::{native_resource_cache, native_resources, native_tables::TableError};

/// No firmware calls, allocations or ownership releases occur in this function.
/// The enclosing CPU scope marks the entire HIGH interval, including old-TPL
/// refusal, for the mandatory exact linked-PE stack/call-graph audit.
#[unsafe(no_mangle)]
#[inline(never)]
unsafe fn svmvisor_native_returning_high(
    guard: &QuiescentBsp<'_>,
    prepared: &mut native_resources::Prepared<'_>,
    boundary: &NativeBoundary,
    observed: &Cell<NativeResult>,
) -> Result<(), u64> {
    let tables = prepared.tables.as_ref().ok_or(9u64)?;
    let arena = prepared.arena.as_ref().ok_or(3u64)?;
    let guest = prepared.guest.as_ref().ok_or(19u64)?;
    unsafe { tables.revalidate(guard) }.map_err(|_| 10u64)?;
    unsafe { prepared.cache.capture_bsp_and_compare(guard) }
        .map_err(|error| 11u64 | prepared.cache.diagnostic_bits(error))?;
    if prepared.cache.bsp_cr3().map_err(|_| 18u64)? != boundary.cr3 {
        return Err(18);
    }
    let before = *prepared
        .cache
        .bsp_snapshot()
        .map_err(|error| 11u64 | prepared.cache.diagnostic_bits(error))?;
    native_resource_cache::qualify(&before, tables).map_err(|_| 12u64)?;
    // The shared builder's NPT and guest-table fetches use PAT entry zero.
    // Software aliases can use a different PAT entry, so qualify these actual
    // physical fetch frames separately. All 33 pages is a conservative superset.
    native_resource_cache::qualify_arena_fetches(
        &before,
        arena.base(),
        crate::native_guest_resources::ARENA_BYTES as u64,
    )
    .map_err(|_| 20u64)?;
    unsafe { tables.revalidate(guard) }.map_err(|_| 10u64)?;

    let context = guest.context();
    let canary = guest.canary();
    let failures = unsafe { svmvisor_native_transition_canary(context, canary) };
    let context = unsafe { &*context };
    let canary = unsafe { &*canary };
    // Persist actual observations before any fallible aftercheck or release.
    // A later failure must never replace a real attempt with invented zeros.
    let mut result = observed.get();
    result.outcome = context.journal.outcome;
    result.refusal = context.journal.refusal_code;
    result.attempted_entries = context.journal.vmrun_attempts;
    result.completed_exits = context.journal.completed_exits;
    result.restoration_complete = context.journal.restoration_complete;
    result.canary_failures = failures;
    result.canary_observed = canary.observed_complete;
    result.canary_called = canary.transition_called;
    observed.set(result);
    if failures != 0 || canary.observed_complete != 1 || canary.transition_called != 1 {
        return Err(21);
    }
    if context.journal.restoration_complete == 1 {
        result.adapter_checks =
            unsafe { guest.verify_observations(context) }.map_err(|error| 0x200 + error)?;
        observed.set(result);
    }
    unsafe { tables.revalidate(guard) }.map_err(|_| 22u64)?;
    unsafe { prepared.cache.capture_bsp_and_compare(guard) }
        .map_err(|error| 23u64 | prepared.cache.diagnostic_bits(error))?;
    if prepared.cache.bsp_cr3().map_err(|_| 18u64)? != boundary.cr3 {
        return Err(18);
    }
    native_cache_rendezvous::compare_configuration(
        &before,
        prepared
            .cache
            .bsp_snapshot()
            .map_err(|error| 23u64 | prepared.cache.diagnostic_bits(error))?,
    )
    .map_err(|_| 24u64)?;
    // An assembly admission refusal is a valid returned observation. The parent
    // independently checks its zero counts and consistent restoration evidence.
    if result.outcome == outcome::REFUSED
        && result.refusal != 0
        && result.attempted_entries == 0
        && result.completed_exits == 0
    {
        return Ok(());
    }
    if context.inputs.mode != mode::MULTI_EXIT
        || result.outcome != MULTI_EXIT_OUTCOME
        || result.refusal != 0
        || result.attempted_entries != MULTI_EXIT_ENTRIES
        || result.completed_exits != MULTI_EXIT_ENTRIES
        || result.restoration_complete != 1
        || context.journal.event_release_completed != 1
        || result.adapter_checks != 15
        || context.guest.captured_fields != guest_capture::ALL
    {
        // Preserve an actual in-run protocol failure and its partial counters.
        // Unknown values remain the generic native aftercheck failure.
        let failure = context.guest.reserved[multi::FAILURE];
        if context.inputs.mode == mode::MULTI_EXIT
            && result.outcome == outcome::UNEXPECTED_EXIT
            && failure >= 1
            && failure <= 9
        {
            return Err(0x300 | failure);
        }
        return Err(25);
    }
    Ok(())
}

fn refused(code: u64, cleanup: bool) -> NativeResult {
    let mut result = NativeResult::new();
    result.outcome = outcome::REFUSED;
    result.refusal = 0x4000 | code;
    result.cleanup_complete = u64::from(cleanup);
    result
}

#[inline(always)]
unsafe fn perform(
    image: Handle,
    table: &SystemTable,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
) -> NativeResult {
    let services = unsafe { &*table.boot_services };
    let mut cpus = match unsafe { native_cpu::prepare(services) } {
        Ok(cpus) => cpus,
        Err(error) => return refused(1, !matches!(error, CpuError::Cleanup(_) | CpuError::Layout)),
    };
    let cpu_storage = match cpus.storage_range() {
        Ok(storage) => storage,
        Err(_) => return refused(1, cpus.release().is_ok()),
    };
    let cache = match unsafe { native_cache_rendezvous::prepare(services, &cpus) } {
        Ok(cache) => cache,
        Err(error) => {
            let cpu_clean = cpus.release().is_ok();
            return refused(
                2,
                cpu_clean
                    && !matches!(error, RendezvousError::Cleanup(_) | RendezvousError::Layout),
            );
        }
    };
    // If the initial CPU barrier refuses before preparation is invoked, keep
    // ownership here so that even this cleanup result is explicitly observed.
    let mut unused_cache = Some(cache);
    let resources_clean = Cell::new(true);
    let observed = Cell::new(refused(16, false));
    let completed = unsafe {
        cpus.with_prepared_quiescent_bsp_and_ap_observation(
            || {
                let cache = unused_cache.take().ok_or(2u64)?;
                resources_clean.set(false);
                let prepared = native_resources::prepare(
                    services,
                    image,
                    boundary,
                    physical_bits,
                    page1gb,
                    cpu_storage,
                    cache,
                );
                if let Err(error) = &prepared {
                    resources_clean
                        .set(*error != 13 && *error != 0x100 + TableError::Cleanup as u64);
                }
                prepared
            },
            |prepared| &prepared.cache,
            |guard, prepared| svmvisor_native_returning_high(guard, prepared, boundary, &observed),
            |prepared| {
                let released = prepared.release();
                resources_clean.set(released.is_ok());
                released
            },
        )
    };
    let unused_clean = unused_cache
        .as_mut()
        .is_none_or(|cache| cache.release().is_ok());
    let cpu_clean = cpus.release().is_ok();
    let mut result = observed.get();
    let outer_error = match completed {
        Err(PreparedScopeError::Cpu(_)) => Some(16),
        Err(PreparedScopeError::Preparation(code)) => Some(code),
        Ok(completed) => match completed.outcome {
            Err(_) => Some(17),
            Ok((_, outcome)) => outcome.err(),
        },
    };
    result.cleanup_complete = u64::from(resources_clean.get() && unused_clean && cpu_clean);
    if let Some(code) = outer_error {
        // Preserve the actual journal outcome/counts/restoration on afterfailure.
        result.refusal = 0x4000 | code;
    }
    if result.cleanup_complete == 0 {
        result.refusal = 0x4000 | if !cpu_clean { 14 } else { 13 };
    }
    result
}

#[inline(always)]
pub(crate) unsafe fn run(
    image: Handle,
    table: &SystemTable,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
) -> NativeResult {
    let result = unsafe { perform(image, table, boundary, physical_bits, page1gb) };
    unsafe { print_result(table, &result) };
    result
}

// Diagnostics happen after resource release and TPL restoration. Keep their
// formatting temporaries out of the sampled preparation/HIGH compiler frame.
#[inline(never)]
unsafe fn print_result(table: &SystemTable, result: &NativeResult) {
    for (field, value) in [
        ("native-returning-outcome", result.outcome),
        ("native-returning-refusal", result.refusal),
        ("native-returning-entries", result.attempted_entries),
        ("native-returning-exits", result.completed_exits),
        ("native-returning-restored", result.restoration_complete),
        ("native-returning-cleanup", result.cleanup_complete),
        ("native-returning-adapter", result.adapter_checks),
        ("native-returning-canary", result.canary_failures),
        ("native-returning-observed", result.canary_observed),
        ("native-returning-called", result.canary_called),
    ] {
        unsafe { crate::native_entry::snapshot_line(table, field, value) };
    }
}
