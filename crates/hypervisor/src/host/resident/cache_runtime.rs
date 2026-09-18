//! Native core-shared MTRR replay with permanently stable physical controls.
//! See the reviewed register and Windows rendezvous evidence under
//! work/native-raw-result-2026-09-15. Every continuation remains CPU-local.
use super::*;
use crate::{
    arch::x86_64::msr::{HWCR, MTRR_DEF_TYPE, SYS_CFG, SYS_CFG_MTRR_FIX_DRAM_MOD_EN},
    memory::mtrrs::{DEF_TYPE_E, valid_default},
    svm::native_cache::{self, CacheCore, CacheCoreState, CacheOwner, CacheWriteError},
};

pub(super) unsafe fn prepare_root() -> bool {
    unsafe {
        (&mut *ptr::addr_of_mut!(CACHE_NPT))
            .prepare(
                &*ptr::addr_of!(NPT),
                ptr::addr_of!(NPT) as u64,
                ptr::addr_of!(CACHE_NPT) as u64,
            )
            .is_ok()
    }
}

pub(super) unsafe fn core(state: &State) -> &'static CacheCore {
    let owner = unsafe {
        &*((ptr::addr_of!(image_start) as u64 + super::super::CACHE_OWNER_OFFSET)
            as *const CacheOwner)
    };
    &owner.cores[state.cache_core]
}

fn refuse(state: &mut State, vmcb: &Vmcb, detail: u64) -> bool {
    let exit = vmcb.exit_snapshot();
    stop(state, exit.code, exit.rip, 0xf400, detail)
}

/// Bounded stopped-CPU wait. Neither the routing lease nor cache-bank lock is
/// retained. Private INIT acknowledgements cannot discard a held continuation.
unsafe fn wait(
    state: &mut State,
    vmcb: &Vmcb,
    mut predicate: impl FnMut(&mut CacheCoreState) -> Option<bool>,
) -> bool {
    let shared = unsafe { core(state) };
    for spin in 0..100_000_000u32 {
        if spin & 1023 == 0 {
            if unsafe { terminal_requested(state) } {
                return false;
            }
            if unsafe { mailboxes(state.count) }[state.slot].peek().is_some()
                || unsafe { acknowledge_init() }.is_none()
            {
                return refuse(state, vmcb, 1);
            }
        }
        match shared.with(&mut predicate) {
            Some(Some(true)) => return true,
            Some(Some(false)) => return refuse(state, vmcb, 2),
            _ => core::hint::spin_loop(),
        }
    }
    refuse(state, vmcb, 3)
}

fn cd_set(vmcb: &Vmcb) -> bool {
    u64::from_le_bytes(vmcb.bytes()[0x558..0x560].try_into().unwrap()) & 0x6000_0000 == 0x4000_0000
}

pub(super) unsafe fn fetch(state: &State, vmcb: &Vmcb) -> Result<[u8; 2], u16> {
    if state.cache_observation.is_none() || !cd_set(vmcb) {
        return unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) };
    }
    let local = state.cache_observation.unwrap();
    if unsafe { read_msr(MTRR_DEF_TYPE) } != local.default
        || local.default & DEF_TYPE_E == 0
        || (unsafe { physical_syscfg(state, local.sys_cfg) } ^ local.sys_cfg)
            & !SYS_CFG_MTRR_FIX_DRAM_MOD_EN
            != 0
    {
        return Err(terminal::FetchReadFailure::PhysicalMemoryNotWb as u16);
    }
    let mut reader = unsafe { GuestReader::new(vmcb, state.startup_owned, state.count) }
        .map_err(|e| e as u16)?;
    super::super::fetch::cache_disabled_instruction(
        vmcb,
        reader.width,
        reader.guest_pat,
        |address, bytes| unsafe { reader.read(address, bytes) },
    )
    .map_err(|error| {
        reader.failure.map_or_else(|| terminal::fetch_failure_code(error), |e| e as u16)
    })
}

fn root(vmcb: &mut Vmcb, state: &State, disabled: bool) -> bool {
    let Some(caps) = state.capabilities.as_ref() else {
        return false;
    };
    vmcb.set_nested_root(
        if disabled { ptr::addr_of!(CACHE_NPT) as u64 } else { ptr::addr_of!(NPT) as u64 },
        &caps.address_policy(),
    )
    .is_ok()
}

pub(super) unsafe fn handle(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> bool {
    use crate::svm::exit::MsrInstruction;
    let exit = vmcb.exit_snapshot();
    let Some(local) = state.cache_observation else {
        return refuse(state, vmcb, 4);
    };
    // Physical E and routing remain observable on every owned boundary. No
    // software workaround can repair drift caused outside this guest owner.
    let syscfg = unsafe { physical_syscfg(state, local.sys_cfg) };
    if unsafe { read_msr(MTRR_DEF_TYPE) } != local.default
        || (syscfg ^ local.sys_cfg) & !SYS_CFG_MTRR_FIX_DRAM_MOD_EN != 0
    {
        return refuse(state, vmcb, 5);
    }
    let caps = state
        .capabilities
        .filter(|c| c.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_some() {
        None
    } else {
        match unsafe { fetch(state, vmcb) } {
            Ok(bytes) => Some(bytes),
            Err(reason) => return refuse(state, vmcb, 0x10000 | reason as u64),
        }
    };
    let instruction = if let Some(caps) = caps.as_ref() {
        match dispatch::hardware_msr_instruction(vmcb, caps) {
            Ok(value) => value,
            Err(_) => return refuse(state, vmcb, 6),
        }
    } else {
        MsrInstruction::Bytes(bytes.as_ref().unwrap())
    };
    if dispatch::validate_native_msr_boundary(vmcb, instruction, state.startup_owned).is_err()
        || vmcb.guest_rflags() & (1 << 8) != 0
    {
        return refuse(state, vmcb, 7);
    }
    let Ok(next) = instruction.continuation(exit) else {
        return refuse(state, vmcb, 8);
    };
    let index = frame.rcx as u32;
    let write = exit.info1 == 1;
    let requested = (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32);
    if vmcb.bytes()[0x4cb] != 0 {
        if vmcb.queue_validated_msr_general_protection(instruction).is_err() {
            return refuse(state, vmcb, 9);
        }
        state.pending_fault = true;
        return true;
    }
    if index == HWCR {
        // This thread's native counter enable is not part of the shared MTRR
        // replay bank. The validated continuation above remains uncommitted
        // until both the masked physical operation and readback succeed.
        #[cfg(feature = "resident-runtime-test")]
        let modeled = core::cell::Cell::new(state.cache_fixture_hwcr);
        let supported = __cpuid_count(0x8000_0008, 0).ebx & (1 << 1) != 0;
        #[cfg(feature = "resident-runtime-test")]
        let supported = supported || state.cache_fixture;
        let cpuid_fault_owned = state.capabilities.is_some_and(|c| c.optional_features().nrip_save)
            && __cpuid_count(0x8000_0000, 0).eax >= 0x8000_0021
            && __cpuid_count(0x8000_0021, 0).eax & (1 << 17) != 0;
        let result = native_cache::access_hwcr(
            local.hwcr,
            write.then_some(requested),
            supported,
            vmcb.bytes()[0xb8] & (1 << 3) != 0,
            cpuid_fault_owned,
            || {
                #[cfg(feature = "resident-runtime-test")]
                if state.cache_fixture {
                    return modeled.get();
                }
                unsafe { read_msr(HWCR) }
            },
            |value| {
                #[cfg(feature = "resident-runtime-test")]
                if state.cache_fixture {
                    modeled.set(value);
                    return;
                }
                // PPR57896 rev3.00 pp203-204. access_hwcr has checked the
                // admitted capture, features, and owned bit30/35 delta on this CPU.
                unsafe {
                    write_msr(HWCR, value);
                }
            },
        );
        #[cfg(feature = "resident-runtime-test")]
        if state.cache_fixture {
            state.cache_fixture_hwcr = modeled.get();
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                return refuse(
                    state,
                    vmcb,
                    match error {
                        native_cache::HwcrError::UnsupportedChange { .. } => 16,
                        native_cache::HwcrError::PhysicalDrift { .. } => 17,
                        native_cache::HwcrError::Readback { .. } => 18,
                        native_cache::HwcrError::PmcVirtualization => 19,
                    },
                );
            }
        };
        let rax = if write {
            vmcb.guest_rax()
        } else {
            frame.rdx = value >> 32;
            value as u32 as u64
        };
        vmcb.commit_emulated_instruction(rax, next);
        vmcb.complete_native_instruction_state();
        state.msr = state.msr.saturating_add(1);
        return true;
    }
    if !write {
        let mut value = None;
        let state_visibility = state.cache_visibility;
        if !unsafe {
            wait(state, vmcb, |bank| {
                value = bank.read(index, state_visibility);
                Some(value.is_some())
            })
        } {
            return false;
        }
        let value = value.unwrap();
        frame.rdx = value >> 32;
        vmcb.commit_emulated_instruction(value as u32 as u64, next);
        vmcb.complete_native_instruction_state();
        state.msr = state.msr.saturating_add(1);
        return true;
    }
    if index == MTRR_DEF_TYPE && !valid_default(requested) {
        if vmcb.queue_validated_msr_general_protection(instruction).is_err() {
            return refuse(state, vmcb, 10);
        }
        state.pending_fault = true;
        return true;
    }
    if index == MTRR_DEF_TYPE && requested & DEF_TYPE_E == 0 {
        if state.cache_active
            || !cd_set(vmcb)
            || local.default & DEF_TYPE_E == 0
            || requested != local.default & !DEF_TYPE_E
        {
            return refuse(state, vmcb, 11);
        }
        if unsafe { !prepare_root() } || !root(vmcb, state, true) {
            return refuse(state, vmcb, 12);
        }
        vmcb.set_cache_cr0_guard(true);
        state.cache_active = true;
        let bit = 1u32 << state.slot;
        let mut admitted = false;
        let mut generation = 0;
        if !unsafe {
            wait(state, vmcb, |bank| {
                if !admitted {
                    let Ok(value) = bank.enter(bit, requested) else {
                        return Some(false);
                    };
                    generation = value;
                    admitted = true;
                }
                if bank.generation != generation {
                    return Some(false);
                }
                matches!(bank.phase, 2 | 3).then_some(true)
            })
        } {
            return false;
        }
    } else if index == MTRR_DEF_TYPE && state.cache_active {
        if !cd_set(vmcb) {
            return refuse(state, vmcb, 13);
        }
        let bit = 1u32 << state.slot;
        let mut admitted = false;
        let mut generation = 0;
        if !unsafe {
            wait(state, vmcb, |bank| {
                if !admitted {
                    let Ok(value) = bank.leave(bit, requested, &local) else {
                        return Some(false);
                    };
                    generation = value;
                    admitted = true;
                }
                if bank.generation != generation {
                    return Some(false);
                }
                (bank.phase == 4).then_some(true)
            })
        } {
            return false;
        }
        if !root(vmcb, state, false) {
            return refuse(state, vmcb, 14);
        }
        vmcb.set_cache_cr0_guard(false);
        vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
        vmcb.complete_native_instruction_state();
        state.cache_active = false;
        let mut departed = false;
        // Reuse cannot precede both local commits and guard/root restoration.
        if !unsafe {
            wait(state, vmcb, |bank| {
                if !departed {
                    if bank.depart(bit, generation).is_err() {
                        return Some(false);
                    }
                    departed = true;
                }
                (bank.generation != generation).then_some(true)
            })
        } {
            return false;
        }
        state.msr = state.msr.saturating_add(1);
        return true;
    } else {
        let mut result = Err(CacheWriteError::Unsupported);
        let mut visibility = state.cache_visibility;
        if !unsafe {
            wait(state, vmcb, |bank| {
                result = bank.write(index, requested, &mut visibility);
                Some(true)
            })
        } {
            return false;
        }
        match result {
            Ok(()) => state.cache_visibility = visibility,
            Err(CacheWriteError::Fault) => {
                if vmcb.queue_validated_msr_general_protection(instruction).is_err() {
                    return refuse(state, vmcb, 15);
                }
                state.pending_fault = true;
                return true;
            }
            Err(CacheWriteError::Unsupported) => return refuse(state, vmcb, 16),
        }
    }
    vmcb.request_full_tlb_flush();
    vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
    vmcb.complete_native_instruction_state();
    state.msr = state.msr.saturating_add(1);
    true
}

unsafe fn physical_syscfg(_state: &State, _logical: u64) -> u64 {
    #[cfg(feature = "resident-runtime-test")]
    if _state.cache_fixture {
        return _logical;
    }
    unsafe { read_msr(SYS_CFG) }
}

/// Disposable-backend seam, absent from production. QEMU supplies real
/// architectural MTRRs but no target SYS_CFG/shared-core MTRR model. Inject
/// that admission bank only; all instructions execute the actual cache owner.
#[cfg(feature = "resident-runtime-test")]
fn fixture_lease(core: &CacheCore) -> Option<crate::sync::TryLockGuard<'_, CacheCoreState>> {
    for _ in 0..100_000 {
        if let Some(guard) = core.try_lock() {
            return Some(guard);
        }
        core::hint::spin_loop();
    }
    None
}

#[cfg(feature = "resident-runtime-test")]
pub(super) unsafe fn fixture_control(
    state: &mut State,
    vmcb: &mut Vmcb,
    operation: u32,
) -> Option<[u32; 4]> {
    use crate::{
        arch::x86_64::msr::{
            MTRR_CAP, MTRR_FIXED, MTRR_VAR_BASE0, PAT, SYS_CFG_MTRR_FIX_DRAM_EN, TARGET_SIGNATURE,
        },
        svm::native_cache::CacheObservation,
    };
    if !matches!(state.count, 2 | 3)
        || state.slot > 1
        || !state.startup_owned
        || __cpuid_count(1, 0).eax == TARGET_SIGNATURE
    {
        return None;
    }
    if operation & 0xffff_0000 == 0x10000 && state.cache_fixture {
        let index = operation & 0xffff;
        if !matches!(index, MTRR_CAP | PAT | MTRR_DEF_TYPE)
            && !(MTRR_VAR_BASE0..=MTRR_VAR_BASE0 + 15).contains(&index)
            && !MTRR_FIXED.contains(&index)
        {
            return None;
        }
        let physical = unsafe { read_msr(index) };
        return Some([0x4341_4348, physical as u32, (physical >> 32) as u32, 0x5048_5953]);
    }
    if operation == 0 {
        if state.cache_observation.is_some() {
            return None;
        }
        let mut local = CacheObservation::EMPTY;
        local.capability = unsafe { read_msr(MTRR_CAP) };
        if local.capability & 255 != 8 {
            return None;
        }
        local.default = unsafe { read_msr(MTRR_DEF_TYPE) };
        local.pat = unsafe { read_msr(PAT) };
        debug(b"native-cache-fixture-physical slot=");
        hex(state.slot as u64);
        debug(b" cap=");
        hex(local.capability);
        debug(b" def=");
        hex(local.default);
        debug(b" pat=");
        hex(local.pat);
        debug(b"\n");
        local.sys_cfg = SYS_CFG_MTRR_FIX_DRAM_EN;
        local.hwcr = 0x10;
        if local.default & DEF_TYPE_E == 0 || local.pat & 255 != 6 {
            return None;
        }
        for (i, pair) in local.variable.iter_mut().enumerate() {
            let base = MTRR_VAR_BASE0 + i as u32 * 2;
            *pair = unsafe { (read_msr(base), read_msr(base + 1)) };
        }
        for (value, index) in local.fixed.iter_mut().zip(MTRR_FIXED) {
            *value = unsafe { read_msr(index) };
        }
        state.cache_core = 0;
        let mut bank = fixture_lease(unsafe { core(state) })?;
        let admitted = if bank.members == 0 {
            *bank = CacheCoreState::fixture(local);
            true
        } else {
            bank.phase == 0
                && bank.bank.restored_mtrrs(
                    local.default,
                    &local.variable,
                    &local.fixed,
                    local.sys_cfg,
                )
        };
        drop(bank);
        debug(b"native-cache-fixture-bank slot=");
        hex(state.slot as u64);
        debug(b" admitted=");
        hex(u64::from(admitted));
        debug(b"\n");
        if !admitted || unsafe { !prepare_root() } {
            return None;
        }
        let maps = unsafe { &mut *ptr::addr_of_mut!(MSRPM) };
        for index in native_cache::owned_msrs() {
            for access in [MsrAccess::Read, MsrAccess::Write] {
                maps.set(index, access, Permission::Intercept).ok()?;
            }
        }
        state.cache_observation = Some(local);
        state.cache_fixture = true;
    } else if operation != 1 || !state.cache_fixture {
        return None;
    }
    let physical = unsafe { read_msr(MTRR_DEF_TYPE) } as u32;
    let bank = fixture_lease(unsafe { core(state) })?;
    let (logical, phase) = (bank.bank.default as u32, bank.phase);
    drop(bank);
    let flags = u32::from(state.cache_active)
        | (u32::from(vmcb.nested_root() == ptr::addr_of!(CACHE_NPT) as u64) << 1)
        | (u32::from(u32::from_le_bytes(vmcb.bytes()[0..4].try_into().unwrap()) & (1 << 16) != 0)
            << 2)
        | (phase << 8);
    Some([0x4341_4348, physical, logical, flags])
}
