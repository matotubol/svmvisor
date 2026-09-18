//! Explicit TCG-only integration fixture for the exact native transition object.
//! This module is never a native admission provider or physical card payload.

use core::{arch::x86_64::__cpuid_count, ptr};

use svmvisor_dxe::native::{
    admission::{boundary::NativeBoundary, cpu as native_cpu},
    transition::{canary::svmvisor_native_transition_canary, state::*},
};
use uefi_raw::{
    Status,
    table::{
        boot::{AllocateType, BootServices, MemoryType},
        system::SystemTable,
    },
};

#[cfg(feature = "native-transition-event-test")]
use crate::native_guest_resources::VMMCALL_RIP;
use crate::{
    native_guest_resources::{self, BoundGuest, changed_canary_components},
    native_tables::{self, PreparedTables},
};

const PAGES: usize = native_guest_resources::ARENA_PAGES;

#[cfg(feature = "native-transition-event-test")]
unsafe extern "efiapi" {
    fn svmvisor_native_stgi();
}

struct Fixture<'a> {
    services: &'a BootServices,
    base: u64,
    owned: bool,
}

impl Fixture<'_> {
    #[cfg(feature = "native-transition-event-test")]
    fn page(&self, index: usize) -> *mut u8 {
        (self.base + index as u64 * 4096) as *mut u8
    }

    fn release(&mut self) -> Result<(), u64> {
        if !self.owned {
            return Ok(());
        }
        if unsafe { (self.services.free_pages)(self.base, PAGES) } != Status::SUCCESS {
            return Err(5);
        }
        self.base = 0;
        self.owned = false;
        Ok(())
    }
}

impl Drop for Fixture<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

struct Prepared<'a> {
    fixture: Fixture<'a>,
    tables: PreparedTables<'a>,
    guest: BoundGuest,
}

struct Observation {
    journal: TransitionJournal,
    guest_exit: u64,
    guest_rip: u64,
    guest_rax: u64,
    guest_captured: u64,
    adapter_checks: u64,
    canary_failures: u64,
    canary_observed: u64,
    canary_called: u64,
    canary_changed: u64,
    multi: [u64; 9],
    multi_completion: [u64; 2],
}

pub unsafe fn run(
    table: &SystemTable,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
) -> svmvisor_dxe::diagnostics::native_result::NativeResult {
    let mut result = svmvisor_dxe::diagnostics::native_result::NativeResult::new();
    let hv = __cpuid_count(0x40000000, 0);
    if [hv.ebx, hv.ecx, hv.edx] != [0x54474354, 0x43544743, 0x47435447] {
        unsafe {
            crate::native_entry::snapshot_line(table, "transition-fixture-refused", 1);
        }
        result.outcome = outcome::REFUSED;
        result.refusal = 0x2001;
        result.cleanup_complete = 1;
        return result;
    }
    // Qualify the literal NPT construction against this emulator's actual CPU
    // and firmware profile; the fixture requires the standard AVX offset.
    if boundary.efer & (1 << 11) == 0
        || boundary.cr4 & (1 << 12) != 0
        || __cpuid_count(0x80000001, 0).edx & (1 << 20) == 0
        || (boundary.profile == 7 && boundary.avx_offset != 576)
    {
        unsafe {
            crate::native_entry::snapshot_line(table, "transition-fixture-refused", 2);
        }
        result.outcome = outcome::REFUSED;
        result.refusal = 0x2002;
        result.cleanup_complete = 1;
        return result;
    }
    let observed = unsafe { perform(table, boundary, physical_bits, page1gb) };
    match observed {
        Ok(observation) => {
            let journal = observation.journal;
            result.outcome = journal.outcome;
            result.refusal = journal.refusal_code;
            result.attempted_entries = journal.vmrun_attempts;
            result.completed_exits = journal.completed_exits;
            result.restoration_complete = journal.restoration_complete;
            result.cleanup_complete = 1;
            result.adapter_checks = observation.adapter_checks;
            result.canary_failures = observation.canary_failures;
            result.canary_observed = observation.canary_observed;
            result.canary_called = observation.canary_called;
            for (field, value) in [
                ("transition-outcome", journal.outcome),
                ("transition-refusal", journal.refusal_code),
                ("transition-progress", journal.progress),
                ("transition-vmruns", journal.vmrun_attempts),
                ("transition-exits", journal.completed_exits),
                ("transition-events-released", journal.event_release_completed),
                ("transition-restored", journal.restoration_complete),
                ("transition-gdt-accessed-restores", journal.gdt_accessed_restores),
                ("transition-guest-exit", observation.guest_exit),
                ("transition-guest-rip", observation.guest_rip),
                ("transition-guest-rax", observation.guest_rax),
                ("transition-guest-captured", observation.guest_captured),
                ("transition-adapter-checks", observation.adapter_checks),
                ("transition-canary-failures", observation.canary_failures),
                ("transition-canary-observed", observation.canary_observed),
                ("transition-canary-called", observation.canary_called),
                ("transition-canary-changed", observation.canary_changed),
                ("transition-multi-cpuid", observation.multi[0]),
                ("transition-multi-query", observation.multi[1]),
                ("transition-multi-resume", observation.multi[2]),
                ("transition-multi-failure", observation.multi[3]),
                ("transition-multi-nrip", observation.multi[4]),
                ("transition-multi-phase", observation.multi[5]),
                ("transition-multi-nrip-checked", observation.multi[6]),
                ("transition-multi-completed-rounds", observation.multi_completion[0]),
                ("transition-multi-completed-proof", observation.multi_completion[1]),
            ] {
                unsafe {
                    crate::native_entry::snapshot_line(table, field, value);
                }
            }
        }
        Err(code) => unsafe {
            crate::native_entry::snapshot_line(table, "transition-fixture-refused", code);
            // Failure may follow a transition or a failed release. Preserve
            // uncertainty instead of claiming zero entries or complete cleanup.
            result.refusal = code;
            result.attempted_entries = u64::MAX;
            result.completed_exits = u64::MAX;
        },
    }
    result
}

unsafe fn perform(
    table: &SystemTable,
    boundary: &NativeBoundary,
    physical_bits: u8,
    page1gb: bool,
) -> Result<Observation, u64> {
    let services = unsafe { &*table.boot_services };
    let mut cpus = unsafe { native_cpu::prepare(services) }.map_err(|_| 20u64)?;
    let result = unsafe {
        cpus.with_prepared_quiescent_bsp(
            || {
                let mut fixture = allocate(services)?;
                // Build/touch all owned pages before retained host mappings
                // settle A/D bits. This shared constructor grants no admission.
                let initialize = if cfg!(feature = "native-transition-multi-exit") {
                    native_guest_resources::initialize_multi_exit
                } else {
                    native_guest_resources::initialize
                };
                let guest = match initialize(
                    fixture.base as *mut u8,
                    fixture.base,
                    boundary,
                    native_guest_resources::read_cpuid_inputs(physical_bits),
                ) {
                    Ok(guest) => guest,
                    Err(error) => {
                        fixture.release()?;
                        return Err(error);
                    }
                };
                // Deliberately broken guest bytes exist only in explicitly
                // selected TCG fixture builds. Native-returning cannot link any
                // transition-test feature. Patches happen before retained maps.
                #[cfg(feature = "native-transition-multi-exit-unexpected")]
                {
                    // After one successful CPUID resume, HLT instead of QUERY.
                    // The multi constructor intercepts HLT, so this is finite.
                    ptr::copy_nonoverlapping(
                        [0xf4u8, 0x90, 0x90].as_ptr(),
                        (fixture.base + 7 * 4096 + 0xbf) as *mut u8,
                        3,
                    );
                }
                #[cfg(feature = "native-transition-multi-exit-mismatch")]
                {
                    // QUERY's full 64-bit opcode becomes 1<<32 while low EAX
                    // stays zero. A truncating handler must not accept it.
                    ((fixture.base + 7 * 4096 + 0xb6) as *mut u8).write(1);
                }
                let mut tables = match native_tables::prepare(services, physical_bits, page1gb) {
                    Ok(tables) => tables,
                    Err(_) => {
                        fixture.release()?;
                        return Err(21u64);
                    }
                };
                let guest = match tables.captured_gdt().map_err(|_| 12u64).and_then(|gdt| {
                    guest.bind(
                        boundary,
                        gdt,
                        if cfg!(feature = "native-transition-roundtrip") {
                            mode::BIND_ONLY
                        } else if cfg!(feature = "native-transition-multi-exit") {
                            mode::MULTI_EXIT
                        } else {
                            mode::ONE_ENTRY
                        },
                    )
                }) {
                    Ok(guest) => guest,
                    Err(error) => {
                        let table_release = tables.release();
                        let fixture_release = fixture.release();
                        if table_release.is_err() || fixture_release.is_err() {
                            return Err(23);
                        }
                        return Err(error);
                    }
                };
                #[cfg(feature = "native-transition-multi-exit-bad-mode")]
                {
                    (*guest.context()).inputs.mode = 3;
                }
                #[cfg(feature = "native-transition-event-test")]
                for (field, value) in [
                    ("event-arena", fixture.base),
                    ("event-ready-pa", fixture.page(8) as u64),
                    ("event-context-pa", fixture.page(30) as u64),
                    ("event-stgi-va", svmvisor_native_stgi as *const () as u64),
                    ("event-spin-rip", 0x10b4),
                    ("event-spin-end-rip", 0x10be),
                    ("event-vmmcall-rip", VMMCALL_RIP),
                ] {
                    crate::native_entry::snapshot_line(table, field, value);
                }
                Ok::<_, u64>(Prepared { fixture, tables, guest })
            },
            |guard, prepared| {
                prepared.tables.revalidate(guard).map_err(|_| 22u64)?;
                let context = prepared.guest.context();
                let canary = prepared.guest.canary();
                let canary_failures = svmvisor_native_transition_canary(context, canary);
                let adapter_checks = if (*context).journal.restoration_complete != 0 {
                    prepared.guest.verify_observations(&*context)?
                } else {
                    0
                };
                prepared.tables.revalidate(guard).map_err(|_| 27u64)?;
                Ok::<_, u64>(Observation {
                    journal: (*context).journal,
                    guest_exit: (*context).guest.exit_code,
                    guest_rip: (*context).guest.rip,
                    guest_rax: (*context).guest.rax,
                    guest_captured: (*context).guest.captured_fields,
                    adapter_checks,
                    canary_failures,
                    canary_observed: (*canary).observed_complete,
                    canary_called: (*canary).transition_called,
                    canary_changed: changed_canary_components(&*canary),
                    multi: (*context).guest.reserved,
                    multi_completion: prepared.guest.multi_completion(),
                })
            },
            |prepared| {
                let tables = prepared.tables.release();
                let fixture = prepared.fixture.release();
                if tables.is_err() || fixture.is_err() { Err(23u64) } else { Ok(()) }
            },
        )
    };
    let released = cpus.release();
    if released.is_err() {
        return Err(24);
    }
    let done = match result {
        Ok(done) => done,
        Err(native_cpu::PreparedScopeError::Cpu(_)) => return Err(25),
        Err(native_cpu::PreparedScopeError::Preparation(code)) => return Err(code),
    };
    done.cleanup?;
    let (_, result) = done.outcome.map_err(|_| 26u64)?;
    result
}

unsafe fn allocate(services: &BootServices) -> Result<Fixture<'_>, u64> {
    let mut base = 0;
    if unsafe {
        (services.allocate_pages)(
            AllocateType::ANY_PAGES,
            MemoryType::BOOT_SERVICES_DATA,
            PAGES,
            &mut base,
        )
    } != Status::SUCCESS
    {
        return Err(3);
    }
    let mut owned = Fixture { services, base, owned: true };
    // Pinned 256MiB OVMF RAM fixture only, never native accessibility evidence.
    if base < 0x100000 || base > 0x10000000 - (PAGES * 4096) as u64 || base & 4095 != 0 {
        owned.release()?;
        return Err(4);
    }
    unsafe {
        ptr::write_bytes(base as *mut u8, 0, PAGES * 4096);
    }
    Ok(owned)
}
