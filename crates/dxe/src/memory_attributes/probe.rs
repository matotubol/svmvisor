//! Experimental, opt-in containment for one aligned supervisor u64 load.
//!
//! The pure matcher and registration model do not qualify firmware. Native
//! entry points exist only for UEFI with `memory-attribute-probe`. They require
//! an independently audited dispatcher, stack, handoff, CPU and image lifetime.
//! An unrelated fault or an abandoned owned callback deliberately FAIL-STOPS;
//! this is not a production fallback or a graceful general exception handler.

use uefi_raw::Status;

#[cfg(all(target_os = "uefi", target_arch = "x86_64", feature = "memory-attribute-probe"))]
pub use native::NativeProbe;

pub const PAGE_FAULT_VECTOR: isize = 14;
pub const MAX_RAM_EXTENTS: usize = 128;
pub const MAX_PROBE_READS: usize = 4096;
pub const F7_DISPATCH_CR4_OR: u64 = 0x208;
const PAGE_SIZE: u64 = 4096;
const LOW_CANONICAL_END: u64 = 1 << 47;
const RF: u64 = 1 << 16;
const FORBIDDEN_FLAGS: u64 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 14) | (1 << 17) | (1 << 18);
const REQUIRED_CR0: u64 = (1 << 31) | (1 << 16) | 1;
const FORBIDDEN_CR4: u64 = (1 << 12) | (1 << 17) | (1 << 21) | (1 << 22) | (1 << 23) | (1 << 24);

/// Exact pinned EDK2 EFI_FX_SAVE_STATE_X64 layout. Reserved bytes are opaque.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FxSaveStateX64 {
    pub fcw: u16,
    pub fsw: u16,
    pub ftw: u16,
    pub opcode: u16,
    pub rip: u64,
    pub data_offset: u64,
    pub reserved1: [u8; 8],
    pub st_mm: [[u8; 16]; 8],
    pub xmm: [[u8; 16]; 8],
    pub reserved11: [u8; 14 * 16],
}

const _: () = assert!(core::mem::size_of::<FxSaveStateX64>() == 512);

/// Exact EFI_SYSTEM_CONTEXT_X64 from pinned DebugSupport.h. The FX save image
/// starts at +8; adding Rust/SIMD alignment would silently break this ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemContextX64 {
    pub exception_data: u64,
    pub fx_save_state: FxSaveStateX64,
    pub dr0: u64,
    pub dr1: u64,
    pub dr2: u64,
    pub dr3: u64,
    pub dr6: u64,
    pub dr7: u64,
    pub cr0: u64,
    pub cr1: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub cr8: u64,
    pub rflags: u64,
    pub ldtr: u64,
    pub tr: u64,
    pub gdtr: [u64; 2],
    pub idtr: [u64; 2],
    pub rip: u64,
    pub gs: u64,
    pub fs: u64,
    pub es: u64,
    pub ds: u64,
    pub cs: u64,
    pub ss: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

const _: () = assert!(core::mem::size_of::<SystemContextX64>() == 0x358);
const _: () = assert!(core::mem::align_of::<SystemContextX64>() == 8);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, fx_save_state) == 8);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, cr0) == 0x238);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, cr2) == 0x248);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, cr3) == 0x250);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, cr4) == 0x258);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, rip) == 0x2a0);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, rsp) == 0x2f0);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, rcx) == 0x308);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, r10) == 0x328);
const _: () = assert!(core::mem::offset_of!(SystemContextX64, r11) == 0x330);

#[repr(C)]
#[derive(Clone, Copy)]
pub union SystemContext {
    pub x64: *mut SystemContextX64,
}

const _: () = assert!(core::mem::size_of::<SystemContext>() == 8);

pub type InterruptHandler = unsafe extern "efiapi" fn(isize, SystemContext);
pub type RegisterInterruptHandler =
    unsafe extern "efiapi" fn(*mut CpuArchProtocol, isize, Option<InterruptHandler>) -> Status;

/// PI CPU protocol shape. Unused function slots are opaque machine words.
#[repr(C)]
pub struct CpuArchProtocol {
    pub flush_data_cache: usize,
    pub enable_interrupt: usize,
    pub disable_interrupt: usize,
    pub get_interrupt_state: usize,
    pub init: usize,
    pub register_interrupt_handler: Option<RegisterInterruptHandler>,
    pub get_timer_value: usize,
    pub set_memory_attributes: usize,
    pub number_of_timers: u32,
    pub dma_buffer_alignment: u32,
}

const _: () = assert!(core::mem::size_of::<CpuArchProtocol>() == 0x48);
const _: () = assert!(core::mem::offset_of!(CpuArchProtocol, register_interrupt_handler) == 0x28);

/// All fields consumed by the assembly/matcher; native code supplies its own
/// fixed label addresses. Public construction is only supplied-data modeling.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeFrame {
    pub source: u64,
    pub root: u64,
    pub bsp_apic_id: u64,
    pub cookie: u64,
    pub expected_rsp: u64,
    pub pre_cr2: u64,
    pub pre_cr4: u64,
    pub pre_cr0: u64,
    pub pre_rflags: u64,
    pub physical_bits: u64,
    pub fault_rip: u64,
    pub failure_rip: u64,
    pub value: u64,
    pub pre_cs: u64,
    pub pre_ss: u64,
    pub slot_address: u64,
    pub status: u64,
}

const _: () = assert!(core::mem::size_of::<ProbeFrame>() == 136);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeProfile {
    pub root: u64,
    pub physical_bits: u8,
    pub nxe: bool,
    pub page1gb: bool,
    /// Full ID from a previously validated CPUID.0B topology leaf.
    pub bsp_apic_id: u32,
}

/// Complete allocated-RAM extent supplied by independently qualified metadata.
/// Constructing this data does not attest RAM, caching, ownership or routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamExtent {
    pub base: u64,
    pub length: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationState {
    Unregistered,
    RegisteredIdle,
    /// Unexpected non-error status: this image may now be referenced, but
    /// exclusive ownership was not established. Do not blindly remove a slot.
    Indeterminate(Status),
    RemovalFailed(Status),
    Removed,
}

/// A supplied-data registration seam. Native use supplies the exact CPU method.
/// Installation error statuses must mean this callback was not installed; successful
/// removal plus the caller's quiescence proof must mean no callback can remain.
pub trait HandlerRegistration {
    fn install(&mut self) -> Status;
    fn remove(&mut self) -> Status;
}

/// Pure ownership model. No Drop cleanup is attempted by this model; the native
/// owner adds a no-return Drop contingency while ownership remains uncertain.
pub struct RegistrationMachine {
    state: RegistrationState,
}

impl RegistrationMachine {
    pub const fn new() -> Self {
        Self { state: RegistrationState::Unregistered }
    }

    pub fn state(&self) -> RegistrationState {
        self.state
    }

    pub fn owns_handler(&self) -> bool {
        matches!(
            self.state,
            RegistrationState::RegisteredIdle
                | RegistrationState::RemovalFailed(_)
                | RegistrationState::Indeterminate(_)
        )
    }

    pub fn register(&mut self, backend: &mut impl HandlerRegistration) -> Result<(), ProbeError> {
        if self.state != RegistrationState::Unregistered {
            return Err(ProbeError::Busy);
        }
        let status = backend.install();
        if status != Status::SUCCESS {
            if !status.is_error() {
                self.state = RegistrationState::Indeterminate(status);
            }
            // In particular, ALREADY_STARTED never authorizes any removal.
            return Err(ProbeError::Registration(status));
        }
        self.state = RegistrationState::RegisteredIdle;
        Ok(())
    }

    pub fn remove(&mut self, backend: &mut impl HandlerRegistration) -> Result<(), ProbeError> {
        if !matches!(
            self.state,
            RegistrationState::RegisteredIdle | RegistrationState::RemovalFailed(_)
        ) {
            return Err(ProbeError::Busy);
        }
        let status = backend.remove();
        if status != Status::SUCCESS {
            self.state = if status.is_error() {
                RegistrationState::RemovalFailed(status)
            } else {
                // The slot may already be empty. A later NULL retry could
                // remove a replacement handler whose ownership is not ours.
                RegistrationState::Indeterminate(status)
            };
            return Err(ProbeError::Removal(status));
        }
        self.state = RegistrationState::Removed;
        Ok(())
    }
}

impl Default for RegistrationMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    InvalidProfile,
    InvalidSource,
    SourceOutsideRam,
    Capacity,
    Busy,
    WrongCpu,
    ControlState,
    ReaderFault,
    Registration(Status),
    Removal(Status),
}

/// Bounded metadata validation, with complete containing-page coverage.
pub fn validate_source(
    profile: ProbeProfile,
    extents: &[RamExtent],
    source: u64,
) -> Result<(), ProbeError> {
    if !valid_profile(profile) {
        return Err(ProbeError::InvalidProfile);
    }
    if extents.is_empty() || extents.len() > MAX_RAM_EXTENTS {
        return Err(ProbeError::Capacity);
    }
    if !valid_source_address(source, u64::from(profile.physical_bits)) {
        return Err(ProbeError::InvalidSource);
    }
    let limit = LOW_CANONICAL_END.min(1u64 << profile.physical_bits);
    let page = source & !(PAGE_SIZE - 1);
    let page_end = page + PAGE_SIZE;
    let mut previous_end = 0;
    let mut covered = false;
    for extent in extents {
        let end = extent.base.checked_add(extent.length).ok_or(ProbeError::InvalidProfile)?;
        if extent.length == 0
            || extent.base & (PAGE_SIZE - 1) != 0
            || extent.length & (PAGE_SIZE - 1) != 0
            || extent.base < previous_end
            || end > limit
        {
            return Err(ProbeError::InvalidProfile);
        }
        previous_end = end;
        covered |= page >= extent.base && page_end <= end;
    }
    if covered { Ok(()) } else { Err(ProbeError::SourceOutsideRam) }
}

/// Change only the fixed failure RIP, failure RAX and two control-state fields.
/// Native code additionally pins both RIP values to its own assembly symbols.
#[inline(always)]
pub fn recover_fault(
    armed: bool,
    vector: isize,
    current_apic_id: u32,
    frame: &ProbeFrame,
    context: &mut SystemContextX64,
) -> bool {
    if !matches_fault(armed, vector, current_apic_id, frame, context) {
        return false;
    }
    context.rip = frame.failure_rip;
    context.rax = 0;
    context.cr2 = frame.pre_cr2;
    context.cr4 = frame.pre_cr4;
    true
}

/// Pure, strict matcher. This first profile recovers ONLY error code zero:
/// supervisor, data read, nonpresent translation. Reserved-bit/protection,
/// instruction, user, write, keys, shadow-stack and unknown classes do not match.
/// RF may be inserted by the CPU for the fault; other recorded flags must match.
#[inline(always)]
pub fn matches_fault(
    armed: bool,
    vector: isize,
    current_apic_id: u32,
    frame: &ProbeFrame,
    context: &SystemContextX64,
) -> bool {
    armed
        && vector == PAGE_FAULT_VECTOR
        && context.exception_data == 0
        && valid_source_address(frame.source, frame.physical_bits)
        && frame.root & (PAGE_SIZE - 1) == 0
        && frame.root < LOW_CANONICAL_END
        && frame.root < 1u64.wrapping_shl(frame.physical_bits as u32)
        && frame.fault_rip != 0
        && frame.fault_rip < LOW_CANONICAL_END
        && frame.failure_rip != 0
        && frame.failure_rip < LOW_CANONICAL_END
        && frame.fault_rip != frame.failure_rip
        && frame.cookie != 0
        && frame.slot_address != 0
        && frame.expected_rsp != 0
        && frame.expected_rsp < LOW_CANONICAL_END
        && frame.pre_cs != 0
        && frame.pre_cs & 3 == 0
        && frame.pre_ss & 3 == 0
        && frame.pre_cr0 & REQUIRED_CR0 == REQUIRED_CR0
        && frame.pre_cr0 & 0xc == 0
        && frame.pre_cr4 & ((1 << 5) | (1 << 9)) == ((1 << 5) | (1 << 9))
        && frame.pre_cr4 & FORBIDDEN_CR4 == 0
        && frame.pre_rflags & FORBIDDEN_FLAGS == 0
        && frame.pre_rflags & 2 != 0
        && u64::from(current_apic_id) == frame.bsp_apic_id
        && context.rip == frame.fault_rip
        && context.cr2 == frame.source
        && context.cr3 == frame.root
        && context.cr0 == frame.pre_cr0
        && context.cr4 == frame.pre_cr4 | F7_DISPATCH_CR4_OR
        && context.cs == frame.pre_cs
        && context.ss == frame.pre_ss
        && context.rsp == frame.expected_rsp
        && context.rcx == frame.slot_address
        && context.r10 == frame.cookie
        && context.r11 == frame.source
        && context.rflags & !RF == frame.pre_rflags & !RF
}

fn valid_profile(profile: ProbeProfile) -> bool {
    (32..=52).contains(&profile.physical_bits)
        && profile.root & (PAGE_SIZE - 1) == 0
        && profile.root < LOW_CANONICAL_END
        && profile.root < (1u64 << profile.physical_bits)
}

#[inline(always)]
fn valid_source_address(source: u64, bits: u64) -> bool {
    if !(32..=52).contains(&bits) || source & 7 != 0 {
        return false;
    }
    match source.checked_add(8) {
        Some(end) => end <= LOW_CANONICAL_END && end <= 1u64.wrapping_shl(bits as u32),
        None => false,
    }
}

#[cfg(all(target_os = "uefi", target_arch = "x86_64", feature = "memory-attribute-probe"))]
mod native {
    use super::*;
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    core::arch::global_asm!(include_str!("probe.S"), options(att_syntax));

    const IDLE: u64 = 1;
    const ARMED: u64 = 2;
    const HANDLING: u64 = 3;
    const FAULTED: u64 = 4;

    #[repr(C, align(64))]
    struct Slot {
        stage: AtomicU64,
        frame: UnsafeCell<ProbeFrame>,
    }

    // External qualification excludes concurrent AP/firmware users. The atomic
    // state rejects callback reentry; it does not establish that qualification.
    unsafe impl Sync for Slot {}

    static CLAIMED: AtomicBool = AtomicBool::new(false);
    static SLOT: Slot = Slot {
        stage: AtomicU64::new(IDLE),
        frame: UnsafeCell::new(ProbeFrame {
            source: 0,
            root: 0,
            bsp_apic_id: 0,
            cookie: 0,
            expected_rsp: 0,
            pre_cr2: 0,
            pre_cr4: 0,
            pre_cr0: 0,
            pre_rflags: 0,
            physical_bits: 0,
            fault_rip: 0,
            failure_rip: 0,
            value: 0,
            pre_cs: 0,
            pre_ss: 0,
            slot_address: 0,
            status: 0,
        }),
    };

    unsafe extern "efiapi" {
        fn svmvisor_memory_probe_load(slot: *const Slot, source: u64, cookie: u64) -> u64;
        fn svmvisor_memory_probe_fault_rip();
        fn svmvisor_memory_probe_failure_rip();
        fn svmvisor_memory_probe_fail_stop() -> !;
    }

    #[inline(always)]
    fn fail_stop() -> ! {
        // SAFETY: Explicit opt-in terminal policy. Never returns or unloads.
        unsafe { svmvisor_memory_probe_fail_stop() }
    }

    #[inline(always)]
    fn current_apic_id() -> u32 {
        // SAFETY: register() has validated leaf 0B. The qualified CPU profile
        // and BSP ownership remain stable throughout this object's lifetime.
        core::arch::x86_64::__cpuid_count(0x0b, 0).edx
    }

    fn on_owner(profile: ProbeProfile) -> bool {
        // Validate the full-ID mechanism on the executing CPU before trusting
        // its result; an unsupported-leaf zero is never treated as BSP ID zero.
        let maximum = core::arch::x86_64::__cpuid(0).eax;
        if maximum < 0x0b {
            return false;
        }
        let topology = core::arch::x86_64::__cpuid_count(0x0b, 0);
        topology.ebx & 0xffff != 0
            && (topology.ecx >> 8) & 0xff != 0
            && topology.edx == profile.bsp_apic_id
    }

    #[unsafe(no_mangle)]
    unsafe extern "efiapi" fn svmvisor_memory_probe_handler(vector: isize, system: SystemContext) {
        if SLOT.stage.compare_exchange(ARMED, HANDLING, Ordering::SeqCst, Ordering::SeqCst).is_err()
        {
            fail_stop();
        }
        let context = unsafe { system.x64 };
        if context.is_null() || (context as usize) & 7 != 0 {
            fail_stop();
        }
        let frame = unsafe { &*SLOT.frame.get() };
        if frame.fault_rip != svmvisor_memory_probe_fault_rip as *const () as u64
            || frame.failure_rip != svmvisor_memory_probe_failure_rip as *const () as u64
        {
            fail_stop();
        }
        // SAFETY: the constructor's independently audited dispatcher contract
        // supplies the exact valid, writable, unique context and callback stack.
        // No requested source is touched anywhere in this callback.
        let recovered =
            recover_fault(true, vector, current_apic_id(), frame, unsafe { &mut *context });
        if !recovered {
            fail_stop();
        }
        SLOT.stage.store(FAULTED, Ordering::SeqCst);
    }

    struct CpuRegistration {
        cpu: *mut CpuArchProtocol,
        method: RegisterInterruptHandler,
    }

    impl HandlerRegistration for CpuRegistration {
        fn install(&mut self) -> Status {
            unsafe {
                (self.method)(self.cpu, PAGE_FAULT_VECTOR, Some(svmvisor_memory_probe_handler))
            }
        }

        fn remove(&mut self) -> Status {
            unsafe { (self.method)(self.cpu, PAGE_FAULT_VECTOR, None) }
        }
    }

    /// Temporary reader whose handle can move but whose hardware operations are
    /// restricted to the qualified BSP. It provides no direct-load handoff.
    pub struct NativeProbe<'a> {
        registration: CpuRegistration,
        machine: RegistrationMachine,
        profile: ProbeProfile,
        extents: &'a [RamExtent],
        reads: usize,
    }

    // SAFETY: Metadata and bookkeeping are ordinary owned/shared immutable Rust
    // data. Every slot/firmware access first checks the qualified full BSP ID.
    // Off-BSP calls refuse; Drop never frees retained callback data or invokes
    // firmware, and fail-stops while any callback ownership remains. The unsafe
    // constructor requires unique stable CPU IDs and protocol/image residency.
    unsafe impl Send for NativeProbe<'_> {}

    impl<'a> NativeProbe<'a> {
        /// Register the temporary callback without replacing an occupied slot.
        ///
        /// # Safety
        /// The caller independently qualifies the exact live CPU protocol,
        /// vector-14 IDT/dispatcher, normal 0xffffffff handoff (no old handler),
        /// pre-HIGH registration/removal TPL, context ABI and synchronous return,
        /// writable exception stack, complete GPR/RFLAGS/descriptor/xstate
        /// preservation, and restoration from edited CR2/CR4 context fields.
        /// The audited F7 CR4 OR0x208 path is required; registration success alone
        /// proves none of these. CPU/root/encryption/routing/cache configuration
        /// is fixed, PG/PAE/LMA and CR0.WP are active; SMAP/keys/CET/LA57/PCID are
        /// excluded. NXE/1-GiB capabilities match the supplied profile. CPUID.0B
        /// provides a stable, full and unique CPU ID on every possible caller.
        ///
        /// The caller owns a quiescent BSP interval, excludes AP/incompatible
        /// NMI/SMM/firmware activity and concurrent handler changes, and certifies
        /// every supplied extent as allocated WB RAM with trusted identity
        /// aliases and no MMIO/machine-check/routing/encryption hazard. Source
        /// root and all entries remain stable for each complete reader/bridge
        /// operation, including its setter call, except intended setter changes;
        /// this includes hardware A/D updates and external CPU/firmware writers.
        /// Code, slot, protocol, RAM metadata and all touched stacks remain
        /// resident and independently accessible until successful removal AND
        /// established callback quiescence. No ExitBootServices or image unload
        /// may race this lifetime. The caller explicitly accepts fail-stop for
        /// unmatched/nested faults, corrupted restoration, or dropping an owner
        /// whose callback removal has not succeeded. No final native gate may
        /// retain this object or run firmware services through it.
        /// Apart from the qualified registration/removal calls, no firmware
        /// service may execute while this temporary handler is owned. A bridge
        /// that invokes a setter must close the probe before that call and
        /// independently requalify/register its subsequent observation reads.
        pub unsafe fn register(
            cpu: *mut CpuArchProtocol,
            profile: ProbeProfile,
            extents: &'a [RamExtent],
        ) -> Result<Self, ProbeError> {
            if cpu.is_null() || cpu as usize & 7 != 0 || !valid_profile(profile) {
                return Err(ProbeError::InvalidProfile);
            }
            // This also validates all metadata before any registration/source load.
            validate_source(profile, extents, profile.root)?;
            if !on_owner(profile) {
                return Err(ProbeError::WrongCpu);
            }
            let method =
                unsafe { (*cpu).register_interrupt_handler }.ok_or(ProbeError::InvalidProfile)?;
            CLAIMED
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .map_err(|_| ProbeError::Busy)?;
            let mut registration = CpuRegistration { cpu, method };
            let mut machine = RegistrationMachine::new();
            let frame = ProbeFrame {
                root: profile.root,
                physical_bits: u64::from(profile.physical_bits),
                bsp_apic_id: u64::from(profile.bsp_apic_id),
                fault_rip: svmvisor_memory_probe_fault_rip as *const () as u64,
                failure_rip: svmvisor_memory_probe_failure_rip as *const () as u64,
                slot_address: core::ptr::from_ref(&SLOT) as u64,
                ..ProbeFrame::default()
            };
            unsafe { SLOT.frame.get().write(frame) };
            SLOT.stage.store(IDLE, Ordering::SeqCst);
            if let Err(error) = machine.register(&mut registration) {
                if machine.owns_handler() {
                    // A warning gives no safe uninstall/return/unload proof.
                    fail_stop();
                }
                CLAIMED.store(false, Ordering::SeqCst);
                return Err(error);
            }
            Ok(Self { registration, machine, profile, extents, reads: 0 })
        }

        /// Execute one bounded, aligned load. This requires the constructor's
        /// qualification to remain true; the assembly additionally requires IF,
        /// TF, DF, NT, VM and AC clear. ReaderFault contains only a matched #PF.
        pub fn read_u64(&mut self, source: u64) -> Result<u64, ProbeError> {
            if !on_owner(self.profile) {
                return Err(ProbeError::WrongCpu);
            }
            if self.machine.state() != RegistrationState::RegisteredIdle
                || SLOT.stage.load(Ordering::SeqCst) != IDLE
            {
                return Err(ProbeError::Busy);
            }
            validate_source(self.profile, self.extents, source)?;
            if self.reads >= MAX_PROBE_READS {
                return Err(ProbeError::Capacity);
            }
            self.reads += 1;
            let cookie = self.reads as u64;
            let status =
                unsafe { svmvisor_memory_probe_load(core::ptr::from_ref(&SLOT), source, cookie) };
            if SLOT.stage.load(Ordering::SeqCst) != IDLE {
                fail_stop();
            }
            match status {
                0 => Ok(unsafe { (*SLOT.frame.get()).value }),
                1 => Err(ProbeError::ReaderFault),
                2 => Err(ProbeError::ControlState),
                _ => fail_stop(),
            }
        }

        /// Successful return discharges slot ownership only under the caller's
        /// pre-established synchronous/quiescent removal contract. Failure keeps
        /// ownership and residency obligations; dropping afterward fail-stops.
        pub fn try_close(&mut self) -> Result<(), ProbeError> {
            if !on_owner(self.profile) {
                return Err(ProbeError::WrongCpu);
            }
            if SLOT.stage.load(Ordering::SeqCst) != IDLE {
                return Err(ProbeError::Busy);
            }
            self.machine.remove(&mut self.registration)?;
            CLAIMED.store(false, Ordering::SeqCst);
            Ok(())
        }

        pub fn registration_state(&self) -> RegistrationState {
            self.machine.state()
        }
    }

    impl Drop for NativeProbe<'_> {
        fn drop(&mut self) {
            if self.machine.owns_handler() {
                // Best-effort unregister cannot justify image/state release.
                fail_stop();
            }
        }
    }

    // SAFETY: register() requires the bridge's independent safe-read, identity,
    // complete-operation stability, mode/cache and image contracts.
    // Every actual source operand passes full-page RAM bounds and the guarded
    // assembly load; no successful MAP query is used as the bootstrap premise.
    unsafe impl super::firmware::QualifiedTableReader for NativeProbe<'_> {
        fn config(&self) -> svmvisor_memory_attributes::Config {
            svmvisor_memory_attributes::Config {
                root: self.profile.root,
                physical_bits: self.profile.physical_bits,
                nxe: self.profile.nxe,
                page1gb: self.profile.page1gb,
            }
        }

        fn read_entry(
            &mut self,
            physical_address: u64,
        ) -> Result<u64, svmvisor_memory_attributes::Error> {
            use svmvisor_memory_attributes::Error;
            self.read_u64(physical_address).map_err(|error| match error {
                ProbeError::Capacity => Error::OutOfResources,
                ProbeError::Busy | ProbeError::WrongCpu => Error::AccessDenied,
                ProbeError::ReaderFault => Error::Unsupported,
                ProbeError::Registration(_) | ProbeError::Removal(_) => Error::DeviceError,
                ProbeError::InvalidProfile
                | ProbeError::InvalidSource
                | ProbeError::SourceOutsideRam
                | ProbeError::ControlState => Error::Unsupported,
            })
        }
    }

    const _: () = assert!(core::mem::offset_of!(Slot, frame) == 8);
    const _: () = assert!(core::mem::size_of::<Slot>() == 192);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, source) == 0);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, root) == 8);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, expected_rsp) == 32);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, pre_cr2) == 40);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, pre_cr4) == 48);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, pre_cr0) == 56);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, pre_rflags) == 64);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, value) == 96);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, pre_cs) == 104);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, pre_ss) == 112);
    const _: () = assert!(core::mem::offset_of!(ProbeFrame, status) == 128);
}
