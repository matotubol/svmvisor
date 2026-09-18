//! Executable per-CPU resident host. UEFI allocation/capture stays in DXE.
//! This image has one guest, no migration and no host FP/SIMD use. All linked
//! code (including panic/compiler builtins) needs the no-FP instruction audit.

use core::{
    arch::{asm, x86_64::__cpuid_count},
    ptr,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

#[cfg(test)]
use crate::host::resident::runtime::linked_symbols::*;
use crate::{
    arch::x86_64::{
        apic::{self, DoorbellTarget, HostX2Apic, PhysicalX2Apic},
        capabilities::{
            CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
        },
        encryption::NativeEncryptionPlan,
        msr::{
            HWCR, HWCR_CPUID_FLT_EN, HWCR_MC_STATUS_WR_EN, MMIO_CFG_BASE_ADDR, MTRR_CAP, PAT,
            SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN,
            SYS_CFG_MTRR_FIX_DRAM_MOD_EN, TARGET_SIGNATURE, VM_CR, VM_CR_R_INIT, VM_CR_SVMDIS,
        },
        registers::GuestRegisters,
    },
    boot::memory::{MAX_DESCRIPTORS, MemoryDescriptor, ValidatedMemoryMap},
    guest::continuation::NativeBootstrapAck,
    host::{
        descriptors::HostDescriptorRequest,
        resident::{
            BridgeContext, DIRECTORY_VERSION, ResidentDirectory,
            terminal::{
                self, IrqSite, StartupStage, TerminalControl, TerminalEndpoint, X2AvicStop,
            },
        },
    },
    memory::{
        address::EncryptionState,
        npt::{PAGE_BYTES, TABLE_COUNT, TableStorage},
    },
    svm::{
        dispatch::{self, NativeEfer, NativeMsrOutcome},
        events::ExternalInterruptError,
        exit::{ExitSnapshot, ResumeCandidate},
        native_cache::CacheCore,
        permission_maps::{Iopm, MsrAccess, Msrpm, Permission},
        vmcb::{ReinjectOutcome, Vmcb},
        x2avic::{
            AvicExit, BackingPage, GUEST_APIC_VERSION, NativeX2AvicProfile, PhysicalIdTable,
            X2AvicCapabilities,
            ipi::{self, FixedIpi, Inventory, IpiAction, IpiDrop, NmiIpi},
            irq::{self, Capture, PhysicalIrqLedger},
            registers::{self, CapturedInterface, Emulation, GuestX2Apic},
            startup::{
                NativeDestinationCause, NativeDestinationMode, NativeIcr, NativeIcrError,
                NativeRoutePredicate, NativeStartupCommand, NativeStartupEffect,
                NativeStartupMailbox, NativeStartupState, NativeStartupTarget, ROUTE_WAIT_ATTEMPTS,
                lock_routes_within, try_lock_routes, validate_destination_slot,
            },
        },
    },
};

#[path = "cache_runtime.rs"]
mod cache;
#[path = "diagnostic_runtime.rs"]
mod diagnostics;

const INITIAL_STATE: State = State {
    prepared: false,
    armed: false,
    capabilities: None,
    cache_observation: None,
    cache_core: 0,
    cache_visibility: false,
    cache_active: false,
    #[cfg(feature = "resident-runtime-test")]
    cache_fixture: false,
    #[cfg(feature = "resident-runtime-test")]
    cache_fixture_hwcr: 0x10,
    ack: None,
    efer: None,
    icr: None,
    guest_apic: None,
    host_apic_base: 0,
    avic: None,
    irq: PhysicalIrqLedger::new(),
    ipi_drops: 0,
    irq_discards: 0,
    startup_owned: false,
    startup: NativeStartupState::Running,
    slot: 0,
    count: 0,
    pending_fault: false,
    exits: 0,
    cpuid: 0,
    msr: 0,
    intr: 0,
    stopped: 0,
    stopped_rip: 0,
    stopped_info1: 0,
    stopped_info2: 0,
    routing_retries: 0,
    nmi_drain_misses: 0,
    terminal_endpoint: None,
    stopped_valid: false,
};

/// Entries of `svmvisor_resident_irq_offsets` (irq.S), for vectors 16-255.
const IRQ_GATES: usize = 240;

/// Stop tag (`info1`) of a stalled physical-NMI drain; `info2` = the
/// consecutive misses. `stop_words` exports it as an unhandled exit 61h with
/// the guest RIP; the tag stays in the stop record and the context export.
const NMI_DRAIN_STALL: u64 = 0xf113;
/// Consecutive undrained VMEXIT_NMI exits that stop this CPU. One miss is
/// tolerated (the NMI may already have been consumed) and a second is margin;
/// a real stall repeats at every VMRUN without guest progress, and three
/// exits still leave two earlier entries in the five-entry exit history.
const NMI_DRAIN_MISS_LIMIT: u8 = 3;

/// Polls of an AwaitSipi wait between two GIF windows (`acknowledge_init`).
const AWAIT_SIPI_POLLS: u32 = 1 << 16;
/// Attempts of one pending command while this core's cache lease is busy.
const STARTUP_LEASE_ATTEMPTS: u32 = 1 << 20;
/// Commands a Running destination applies in one exit before it resumes its
/// guest; the rest are serviced at its next exit (every exit services them).
const STARTUP_COMMANDS_PER_EXIT: u32 = 64;

#[unsafe(no_mangle)]
static svmvisor_resident_init_acks: AtomicU64 = AtomicU64::new(0);

/// Set to 1 by the host NMI gate (vector 2, irq.S) when a physical NMI is
/// taken in a host GIF window, and swapped back to 0 by the dispatcher, which
/// re-presents it to the guest as a virtual NMI (V_NMI). APM2 rev3.44 15.21.10
/// p536: platform NMIs are re-presented under NMI virtualization. This CPU is
/// the only writer (its own NMI) and reader (its own dispatcher).
#[unsafe(no_mangle)]
static svmvisor_resident_nmi_pending: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
static mut svmvisor_resident_raw_vmexit: RawVmexitCapture = unsafe { core::mem::zeroed() };
// CPU-local circular history; no PCI traffic, heap, guest reads or global lock.
// Updated only on the ordinary stopped-guest stack, before dispatcher mutation.
static mut EXIT_HISTORY: [[u64; 6]; 5] = [[0; 6]; 5];
static mut EXIT_HISTORY_NEXT: usize = 0;
static mut EXIT_HISTORY_COUNT: usize = 0;

static mut TABLES: HostTables = HostTables([[0; 512]; 4]);
static mut NPT: TableStorage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
static mut CACHE_NPT: crate::memory::npt::LowMemoryNptStorage =
    crate::memory::npt::LowMemoryNptStorage::empty();
static mut VMCB: Vmcb = Vmcb::new();
static mut AVIC_BACKING: BackingPage = BackingPage::new();
static mut AUX: Vmcb = Vmcb::new();
static mut HOST_EXTRA: Vmcb = Vmcb::new();
static mut HSAVE: Pages<1> = Pages([[0; 4096]; 1]);
static mut STACK: Pages<18> = Pages([[0; 4096]; 18]);
static mut FAULT_STACK: Pages<6> = Pages([[0; 4096]; 6]);
// Dedicated IST2 stack for the returning host NMI gate: one usable page
// between two guard pages, like FAULT_STACK.
static mut NMI_STACK: Pages<3> = Pages([[0; 4096]; 3]);
static mut GDT_TSS: Pages<1> = Pages([[0; 4096]; 1]);
static mut IDT: Pages<1> = Pages([[0; 4096]; 1]);
static mut GDTR: [u8; 10] = [0; 10];
static mut IDTR: [u8; 10] = [0; 10];
static mut FRAME: GuestRegisters = GuestRegisters {
    rcx: 0,
    rdx: 0,
    rbx: 0,
    rbp: 0,
    rsi: 0,
    rdi: 0,
    r8: 0,
    r9: 0,
    r10: 0,
    r11: 0,
    r12: 0,
    r13: 0,
    r14: 0,
    r15: 0,
};
static mut IOPM: Iopm = Iopm::new();
static mut MSRPM: Msrpm = Msrpm::new();
static mut CONTEXT: BridgeContext = BridgeContext {
    host_stack_top: 0,
    host_cr3: 0,
    guest_vmcb_pa: 0,
    guest_vmcb_va: 0,
    host_extra_pa: 0,
    guest_frame_va: 0,
    hsave_pa: 0,
    host_gdtr_va: 0,
    host_idtr_va: 0,
    host_tr_selector: 24,
    host_data_selector: 16,
    host_code_selector: 8,
    dispatch,
    owner_context: 0,
    reserved: 0,
};

static mut STATE: State = INITIAL_STATE;
static mut RAM: [MemoryDescriptor; MAX_DESCRIPTORS] =
    [MemoryDescriptor { memory_type: 0, physical_start: 0, page_count: 0, attributes: 0 };
        MAX_DESCRIPTORS];
static mut RAM_COUNT: usize = 0;
static mut PHYSICAL_BITS: u8 = 0;
static mut POOL: (u64, u64) = (0, 0);
static mut ASSIGNED_APIC_ID: u32 = u32::MAX;

// Defined by crates/resident-payload/payload.ld, runtime.S, irq.S and fault.S.
#[cfg(not(test))]
unsafe extern "C" {
    static image_start: u8;
    static text_end: u8;
    static data_start: u8;
    static image_bss_end: u8;
    static svmvisor_resident_fault_offsets: [i32; 256];
    static svmvisor_resident_irq_offsets: [i32; IRQ_GATES];
    fn svmvisor_resident_accept_irq() -> u32;
    fn svmvisor_resident_sx();
    fn svmvisor_resident_nmi();
}

#[cfg(not(test))]
unsafe extern "win64" {
    fn svmvisor_resident_enter(context: *mut BridgeContext) -> !;
}

/// Host unit tests link this module without the payload linker script and
/// resident assembly. These inert stand-ins only satisfy references from code
/// the tests never execute; no test may reach them.
#[cfg(test)]
#[allow(non_upper_case_globals)]
mod linked_symbols {
    use crate::host::resident::BridgeContext;

    pub(super) static image_start: u8 = 0;
    pub(super) static text_end: u8 = 0;
    pub(super) static data_start: u8 = 0;
    pub(super) static image_bss_end: u8 = 0;
    pub(super) static svmvisor_resident_fault_offsets: [i32; 256] = [0; 256];
    pub(super) static svmvisor_resident_irq_offsets: [i32; super::IRQ_GATES] =
        [0; super::IRQ_GATES];
    pub(super) unsafe extern "C" fn svmvisor_resident_accept_irq() -> u32 {
        unreachable!("host unit tests never accept a physical IRQ")
    }
    pub(super) unsafe extern "C" fn svmvisor_resident_sx() {
        unreachable!("host unit tests never take #SX")
    }
    pub(super) unsafe extern "C" fn svmvisor_resident_nmi() {
        unreachable!("host unit tests never take a host NMI")
    }
    pub(super) unsafe extern "win64" fn svmvisor_resident_enter(_: *mut BridgeContext) -> ! {
        unreachable!("host unit tests never enter the resident runtime")
    }
}

struct State {
    prepared: bool,
    armed: bool,
    capabilities: Option<ValidatedCapabilities>,
    cache_observation: Option<crate::svm::native_cache::CacheObservation>,
    cache_core: usize,
    cache_visibility: bool,
    cache_active: bool,
    #[cfg(feature = "resident-runtime-test")]
    cache_fixture: bool,
    #[cfg(feature = "resident-runtime-test")]
    cache_fixture_hwcr: u64,
    ack: Option<NativeBootstrapAck>,
    efer: Option<NativeEfer>,
    icr: Option<NativeIcr>,
    /// Guest APIC_BASE shadow (D4), admitted from the captured physical value.
    guest_apic: Option<GuestX2Apic>,
    /// Physical APIC_BASE captured at arm. The host keeps this interface; the
    /// terminal notification rechecks it. Never a guest-visible value.
    host_apic_base: u64,
    avic: Option<NativeX2AvicProfile>,
    irq: PhysicalIrqLedger,
    /// AVIC_INCOMPLETE_IPI exits that delivered nothing (D5: illegal vector
    /// or no admitted target). Every stop record exports it, saturated to 32
    /// bits, in the low half of its last context word.
    ipi_drops: u64,
    /// Physical edge interrupts acknowledged without publication because the
    /// guest APIC was software-disabled (`Capture::Discarded`). Exported like
    /// `ipi_drops`, in the high half.
    irq_discards: u64,
    startup_owned: bool,
    startup: NativeStartupState,
    slot: usize,
    count: usize,
    pending_fault: bool,
    exits: u64,
    cpuid: u64,
    msr: u64,
    intr: u64,
    stopped: u64,
    stopped_rip: u64,
    stopped_info1: u64,
    stopped_info2: u64,
    routing_retries: u16,
    /// Consecutive VMEXIT_NMI exits whose host GIF window did not take the
    /// pending physical NMI (`nmi_drain_stalled`).
    nmi_drain_misses: u8,
    terminal_endpoint: Option<TerminalEndpoint>,
    stopped_valid: bool,
}

/// Single-writer host-private provenance, written by runtime.S. The entry half
/// is recorded before loading guest GPRs; the exit half before VMSAVE/VMLOAD or
/// Rust dispatch. The exit sequence commits last. It is retained unchanged
/// through a stop and overwritten only by the next world switch, never by SMM
/// acknowledgment or instruction emulation in this runtime.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct RawVmexitCapture {
    entry_sequence: u64,
    exit_sequence: u64,
    entry_vmcb_pa: u64,
    entry_rip: u64,
    exit_vmcb_pa: u64,
    exit_rip: u64,
    nrip: u64,
    code: u64,
    guest_cr0: u64,
    guest_cr3: u64,
    guest_rax: u64,
    guest_rdx: u64,
    mtrr_def_type: u64,
    sys_cfg: u64,
    host_cr0: u64,
    physical_apic_id: u64,
    guest_rcx: u64,
    entry_cr0: u64,
    context_vmcb_pa: u64,
    physical_cache_valid: u64,
}

const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    assert!(size_of::<RawVmexitCapture>() == 160);
    assert!(align_of::<RawVmexitCapture>() == 8);
    assert!(offset_of!(RawVmexitCapture, entry_sequence) == 0);
    assert!(offset_of!(RawVmexitCapture, exit_sequence) == 8);
    assert!(offset_of!(RawVmexitCapture, entry_vmcb_pa) == 16);
    assert!(offset_of!(RawVmexitCapture, entry_rip) == 24);
    assert!(offset_of!(RawVmexitCapture, exit_vmcb_pa) == 32);
    assert!(offset_of!(RawVmexitCapture, exit_rip) == 40);
    assert!(offset_of!(RawVmexitCapture, nrip) == 48);
    assert!(offset_of!(RawVmexitCapture, code) == 56);
    assert!(offset_of!(RawVmexitCapture, guest_cr0) == 64);
    assert!(offset_of!(RawVmexitCapture, guest_cr3) == 72);
    assert!(offset_of!(RawVmexitCapture, guest_rax) == 80);
    assert!(offset_of!(RawVmexitCapture, guest_rdx) == 88);
    assert!(offset_of!(RawVmexitCapture, mtrr_def_type) == 96);
    assert!(offset_of!(RawVmexitCapture, sys_cfg) == 104);
    assert!(offset_of!(RawVmexitCapture, host_cr0) == 112);
    assert!(offset_of!(RawVmexitCapture, physical_apic_id) == 120);
    assert!(offset_of!(RawVmexitCapture, guest_rcx) == 128);
    assert!(offset_of!(RawVmexitCapture, entry_cr0) == 136);
    assert!(offset_of!(RawVmexitCapture, context_vmcb_pa) == 144);
    assert!(offset_of!(RawVmexitCapture, physical_cache_valid) == 152);
};

impl RawVmexitCapture {
    fn stop_record(
        self,
        expected_vmcb: u64,
        assigned_apic_id: u32,
        reason: u64,
        detail: u64,
    ) -> Option<(u8, [u64; 6])> {
        // Assembly captures GPR/VMCB provenance and physical APIC identity on
        // every MSR exit. Only the optional physical cache sample is SYS_CFG-
        // specific; other owned registers must not depend on that sample.
        let index = self.guest_rcx as u32;
        if self.code != 0x7c
            || !crate::svm::native_cache::owned_msr(index)
            || index == SYS_CFG && self.physical_cache_valid != 1
        {
            return None;
        }
        if self.entry_sequence == 0
            || self.entry_sequence != self.exit_sequence
            || self.entry_vmcb_pa != expected_vmcb
            || self.exit_vmcb_pa != expected_vmcb
            || self.context_vmcb_pa != expected_vmcb
            || self.physical_apic_id != u64::from(assigned_apic_id)
        {
            return Some((
                11,
                [
                    self.exit_vmcb_pa,
                    expected_vmcb,
                    (self.physical_apic_id << 32) | u64::from(assigned_apic_id),
                    self.exit_rip,
                    self.nrip,
                    self.entry_rip,
                ],
            ));
        }
        if index == SYS_CFG {
            Some((
                10,
                [
                    self.exit_rip,
                    self.nrip,
                    self.entry_rip,
                    self.guest_cr0,
                    self.mtrr_def_type,
                    self.host_cr0,
                ],
            ))
        } else {
            // EDX:EAX uses the low DWORD of each register. Preserve raw RCX
            // separately from its architectural low-DWORD MSR index. These
            // are boundary operands, not a claim the access was completed.
            let operand = (self.guest_rax as u32 as u64) | ((self.guest_rdx as u32 as u64) << 32);
            Some((13, [self.exit_rip, self.guest_rcx, operand, self.nrip, reason, detail]))
        }
    }
}

#[repr(C, align(4096))]
struct Pages<const N: usize>([[u8; 4096]; N]);

#[repr(C, align(4096))]
struct HostTables([[u64; 512]; 4]);

/// The stopped exit that `dispatch_body` is handling.
struct ExitContext<'a> {
    state: &'a mut State,
    vmcb: &'a mut Vmcb,
    frame: &'a mut GuestRegisters,
    exit: ExitSnapshot,
}

/// Where queued startup commands are serviced relative to an exit's own
/// handler, once the guest has acknowledged its bootstrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitOrder {
    /// The exit reports completed guest work (Table 15-22 p567 lists the ICRL
    /// write and the level EOI write as traps) or a physical interrupt at an
    /// instruction boundary. Its effect belongs before a later INIT, so the
    /// handler runs first; the startup service then runs as for any exit.
    TrapThenStartup,
    /// An instruction intercept or other fault-style exit: the guest
    /// instruction has not run. A queued INIT resets the guest first and the
    /// intercepted instruction is then never completed.
    StartupThenExit,
}

/// How one register-owner outcome completes an intercepted RDMSR/WRMSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MsrCompletion {
    /// Continue at nRIP. A RDMSR loads EDX:EAX with `read`; in 64-bit mode
    /// the upper halves of RAX and RDX become zero.
    Complete { read: Option<u64> },
    /// Queue #GP(0) at the unchanged RIP.
    Fault,
    /// Stop with this reason and value; the instruction is not completed.
    Stop(u64, u64),
}

/// What one AVIC exit asks of the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AvicPlan {
    /// AVIC_INCOMPLETE_IPI with an ID of 0-4 (Table 15-27 p581).
    IncompleteIpi,
    /// AVIC_NOACCEL for a level-triggered EOI write (Table 15-29 p582).
    LevelEoi(u8),
    /// Stop with this reason and value.
    Stop(u64, u64),
}

/// Runtime action for one AVIC_INCOMPLETE_IPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IncompleteIpi {
    /// Route this ICR (delivery status clear) through the startup mailbox.
    Startup(u64),
    /// Publish to the target slots and doorbell the remote ones.
    Fixed(FixedIpi),
    /// Set V_NMI on each target (the sender directly, remote targets through
    /// the NMI mailbox command and the private kick).
    Nmi(NmiIpi),
    /// Deliver nothing; count the drop and resume.
    Dropped(IpiDrop),
    /// Hardware already published the IPI (ID 1); resume.
    Published,
    /// Stop with this reason and value.
    Stop(u64, u64),
}

/// Outcome of one startup command (`startup_step`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupStep {
    /// Committed and removed from the mailbox.
    Applied(NativeStartupEffect),
    /// The cache lease was busy; nothing changed and the command stays queued.
    Busy,
    /// A pending guest event refused the command before any change.
    PendingEvent(ExternalInterruptError),
    /// Stop at this stage with this value (`None`: the AwaitSipi flag).
    Failed(StartupStage, Option<u64>),
}

/// Hardware owners that a guest INIT changes on its own CPU.
struct InitOwners<'a, P> {
    backing: &'a BackingPage,
    physical: P,
    msrpm: &'a mut Msrpm,
    /// CPUID Fn0000_0001 EAX, the INIT value of EDX (APM2 Table 14-2).
    signature: u32,
}

impl InitOwners<'static, HostX2Apic> {
    /// # Safety
    /// This CPU's armed dispatcher with its guest stopped and IF/GIF clear:
    /// arm admitted the enabled physical x2APIC, and no other reference to
    /// the private MSRPM is live.
    unsafe fn local(signature: u32) -> Self {
        unsafe {
            Self {
                backing: &*ptr::addr_of!(AVIC_BACKING),
                physical: HostX2Apic::new(),
                msrpm: &mut *ptr::addr_of_mut!(MSRPM),
                signature,
            }
        }
    }
}

/// Existing temporary RAM alias shared by CPUID, MSR and cache-owner fetches.
/// Owns one stopped CPU; each read removes its alias before returning.
struct GuestReader {
    deny_low: bool,
    map: ValidatedMemoryMap<'static>,
    monitor: crate::memory::address::PhysicalRange,
    mt: crate::memory::mtrrs::Mtrrs,
    width: u8,
    guest_pat: u64,
    startup_owned: bool,
    failure: Option<terminal::FetchReadFailure>,
    count: usize,
}

impl GuestReader {
    unsafe fn new(
        vmcb: &Vmcb,
        startup_owned: bool,
        count: usize,
    ) -> Result<Self, terminal::FetchReadFailure> {
        use crate::memory::address::AddressPolicy;
        use terminal::FetchReadFailure as R;
        let width = unsafe { PHYSICAL_BITS };
        let map = unsafe { core::slice::from_raw_parts(ptr::addr_of!(RAM).cast(), RAM_COUNT) };
        let map = ValidatedMemoryMap::new(map, width).map_err(|_| R::MemoryMap)?;
        let policy =
            AddressPolicy::new(width, EncryptionState::Unencrypted { encryption_bit: None })
                .map_err(|_| R::AddressPolicy)?;
        let (pool_base, pool_bytes) = unsafe { POOL };
        let monitor = policy.validate(pool_base, pool_bytes, 4096).map_err(|_| R::MonitorRange)?;
        let host_pat = unsafe { read_msr(PAT) };
        if host_pat & 255 != 6 {
            return Err(R::HostPat);
        }
        let mt = unsafe { native_mtrrs(width) }.ok_or(R::MtrrCapture)?;
        let guest_pat = u64::from_le_bytes(vmcb.bytes()[0x668..0x670].try_into().unwrap());
        Ok(Self {
            deny_low: vmcb.nested_root() == ptr::addr_of!(CACHE_NPT) as u64,
            map,
            monitor,
            mt,
            width,
            guest_pat,
            startup_owned,
            failure: None,
            count,
        })
    }

    /// Same stopped/private-root WB RAM admission as fetch_instruction.
    unsafe fn read(&mut self, address: u64, bytes: usize) -> Option<u64> {
        use terminal::FetchReadFailure as R;
        if !matches!(bytes, 1 | 8)
            || (address & 4095) + bytes as u64 > 4096
            || (bytes == 8 && address & 7 != 0)
        {
            self.failure = Some(R::ReadShape);
            return None;
        }
        let physical = address & !4095;
        if self.deny_low && physical < 0x100000 {
            self.failure = Some(R::RamAdmission);
            return None;
        }
        if self.map.permit_guest_ram(physical, 4096, self.monitor).is_err() {
            self.failure = Some(R::RamAdmission);
            return None;
        }
        // Serialize core-shared SYS_CFG18 with every low-RAM sample and access.
        // Fetch completes before the APIC operation takes this same route gate.
        let _memory_controls = if self.startup_owned && physical < 0x100000 {
            match try_lock_routes(unsafe { mailboxes(self.count) }) {
                Ok(guard) => Some(guard),
                Err(_) => {
                    self.failure = Some(R::MemoryControlBusy);
                    return None;
                }
            }
        } else {
            None
        };
        let wb = if self.startup_owned && physical < 0x100000 {
            use crate::memory::mtrrs::{CAP_FIX, DEF_TYPE_DEFINED, DEF_TYPE_E, DEF_TYPE_FE};
            if self.mt.default & !DEF_TYPE_DEFINED != 0
                || self.mt.default & (DEF_TYPE_E | DEF_TYPE_FE) != DEF_TYPE_E | DEF_TYPE_FE
                || unsafe { read_msr(MTRR_CAP) } & CAP_FIX == 0
            {
                self.failure = Some(R::FixedMtrrControl);
                return None;
            }
            let Some((index, shift)) = crate::memory::mtrrs::Mtrrs::fixed_range_register(physical)
            else {
                self.failure = Some(R::FixedMtrrRange);
                return None;
            };
            if crate::memory::mtrrs::Tom2Default::supported_profile(
                __cpuid_count(1, 0).eax,
                self.width,
            ) {
                unsafe { native_fixed_page_is_wb(index, shift) }
            } else {
                (unsafe { read_msr(index) } >> shift) & 255 == 6
            }
        } else {
            self.mt.page_is_wb(physical)
        };
        if !wb {
            self.failure = Some(R::PhysicalMemoryNotWb);
            return None;
        }
        let window = ptr::addr_of!(image_start) as u64 + 0xff000;
        let slot = unsafe {
            ptr::addr_of_mut!((*ptr::addr_of_mut!(TABLES)).0[3][((window >> 12) & 511) as usize])
        };
        // The final page has no backing object and starts absent. Never replace
        // an unexpected retained mapping or leave an alias across guest entry.
        if unsafe { ptr::read_volatile(slot) } != 0 {
            self.failure = Some(R::ScratchAliasOccupied);
            return None;
        }
        unsafe {
            ptr::write_volatile(slot, physical | 1 | (1 << 63));
            asm!("invlpg [{}]",in(reg)window,options(nostack,preserves_flags));
        }
        let pointer = (window + (address & 4095)) as *const u8;
        let value = unsafe {
            if bytes == 1 {
                ptr::read_volatile(pointer) as u64
            } else {
                ptr::read_volatile(pointer.cast::<u64>())
            }
        };
        unsafe {
            ptr::write_volatile(slot, 0);
            asm!("invlpg [{}]",in(reg)window,options(nostack,preserves_flags));
        }
        Some(value)
    }
}

/// # Safety
/// Call once in a legal loaded raw runtime allocation under live native CPL0
/// identity mappings. Caller has validated the entire 1MiB backing RW/X/WB,
/// original stack and this exact linked package. No CPU control is changed.
/// `output` is disjoint writable caller storage. Firmware owns allocation.
/// Every dense pool slot holds a relocated copy of this same package; the
/// private root's remote backing aliases depend on that before arm.
pub unsafe extern "win64" fn prepare(
    base: u64,
    output: *mut ResidentDirectory,
    pool_base: u64,
    pool_bytes: u64,
    cpu_slot: u64,
    apic_id: u64,
) -> u64 {
    let start = ptr::addr_of!(image_start) as u64;
    let text_limit = ptr::addr_of!(text_end) as u64;
    let data = ptr::addr_of!(data_start) as u64;
    let end = ptr::addr_of!(image_bss_end) as u64;
    let backing = ptr::addr_of!(AVIC_BACKING) as u64;
    if output.is_null()
        || !super::valid_pool_slot(base, pool_base, pool_bytes, cpu_slot, apic_id)
        || base != start
        || base < 0x100000
        || base & 4095 != 0
        || base + 0x100000 > 0x40000000
        || base >> 21 != (base + 0xfffff) >> 21
        || text_limit <= base
        || text_limit > data
        || data > end
        || end > base + super::X2AVIC_BACKING_ALIASES_OFFSET
        || (text_limit | data | end | backing) & 4095 != 0
        || backing < data
        || backing + 4096 > end
    {
        return 1;
    }
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if state.prepared {
        return 2;
    }
    let root = ptr::addr_of_mut!(TABLES) as u64;
    let stack = ptr::addr_of_mut!(STACK) as u64;
    let fault_stack = ptr::addr_of_mut!(FAULT_STACK) as u64;
    let nmi_stack = ptr::addr_of_mut!(NMI_STACK) as u64;
    let gdt = ptr::addr_of_mut!(GDT_TSS) as u64;
    let idt = ptr::addr_of_mut!(IDT) as u64;
    let stack_top = stack + 17 * 4096;
    let fault_top = fault_stack + 5 * 4096;
    let nmi_top = nmi_stack + 2 * 4096;
    let fault_offsets = ptr::addr_of!(svmvisor_resident_fault_offsets);
    let mut handlers = core::array::from_fn(|vector| {
        (fault_offsets as u64).wrapping_add(unsafe { (*fault_offsets)[vector] } as i64 as u64)
    });
    handlers[30] = svmvisor_resident_sx as *const () as u64;
    // Vector 2: the returning host NMI gate (irq.S) on IST2, replacing the
    // terminal fault stub, so a platform NMI in a host GIF window is swallowed
    // and re-presented to the guest as V_NMI (15.21.10 p536) instead of
    // stopping this CPU.
    handlers[2] = svmvisor_resident_nmi as *const () as u64;
    let irq_offsets = ptr::addr_of!(svmvisor_resident_irq_offsets);
    for vector in (0..256).filter(|&vector| window_gate(vector)) {
        handlers[vector] =
            (irq_offsets as u64).wrapping_add(unsafe { (*irq_offsets)[vector - 16] } as i64 as u64);
    }
    let request = HostDescriptorRequest {
        gdt_base: gdt,
        tss_base: gdt + 128,
        idt_base: idt,
        rsp0: stack_top,
        ist1: fault_top,
        ist2: nmi_top,
        handlers,
    };
    let Ok(descriptors) = request.validate_terminal_ist() else {
        return 3;
    };
    unsafe {
        ptr::copy_nonoverlapping(
            descriptors.gdt().as_ptr(),
            gdt as *mut u8,
            descriptors.gdt().len(),
        );
        ptr::copy_nonoverlapping(
            descriptors.tss().as_ptr(),
            (gdt + 128) as *mut u8,
            descriptors.tss().len(),
        );
        ptr::copy_nonoverlapping(descriptors.idt().as_ptr(), idt as *mut u8, 4096);
        // Returning IRQ gates at 32-255 use the current private host stack,
        // not terminal IST1; 16-31 keep IST1 (`window_gate`).
        for vector in 32..256 {
            (idt as *mut u8).add(vector * 16 + 4).write(0);
        }
        let gdtr = &mut *ptr::addr_of_mut!(GDTR);
        gdtr[..2].copy_from_slice(&descriptors.gdtr().limit.to_le_bytes());
        gdtr[2..].copy_from_slice(&gdt.to_le_bytes());
        let idtr = &mut *ptr::addr_of_mut!(IDTR);
        idtr[..2].copy_from_slice(&4095u16.to_le_bytes());
        idtr[2..].copy_from_slice(&idt.to_le_bytes());
    }
    let tables = unsafe { &mut (*ptr::addr_of_mut!(TABLES)).0 };
    tables[0][0] = (root + 4096) | 3;
    tables[1][0] = (root + 8192) | 3;
    tables[2][((base >> 21) & 511) as usize] = (root + 12288) | 3;
    for page in (base..end).step_by(4096) {
        if [stack, stack_top, fault_stack, fault_top, nmi_stack, nmi_top].contains(&page) {
            continue;
        }
        let flags = if page < text_limit {
            1
        } else if page < data {
            1 | (1 << 63)
        } else {
            3 | (1 << 63)
        };
        tables[3][((page >> 12) & 511) as usize] = page | flags;
    }
    // Every private root maps one shared RW/NX page through a fixed local
    // alias. Its physical backing remains inside the excluded monitor pool.
    let avic_alias = base + super::X2AVIC_TABLE_OFFSET;
    tables[3][((avic_alias >> 12) & 511) as usize] =
        (pool_base + super::X2AVIC_TABLE_OFFSET) | 3 | (1 << 63);
    let shared_alias = base + super::STARTUP_PAGE_OFFSET;
    tables[3][((shared_alias >> 12) & 511) as usize] =
        (pool_base + super::STARTUP_PAGE_OFFSET) | 3 | (1 << 63);
    for offset in (super::CACHE_OWNER_OFFSET..super::CACHE_CAPTURE_OFFSET).step_by(4096) {
        tables[3][(((base + offset) >> 12) & 511) as usize] = (pool_base + offset) | 3 | (1 << 63);
    }
    for offset in (super::CACHE_CAPTURE_OFFSET
        ..super::CACHE_CAPTURE_OFFSET
            + core::mem::size_of::<crate::svm::native_cache::CacheCapture>() as u64)
        .step_by(4096)
    {
        tables[3][(((base + offset) >> 12) & 511) as usize] = (pool_base + offset) | 1 | (1 << 63);
    }
    // RW/NX alias of every dense slot's retained backing page, including this
    // slot's own; later alias pages stay absent. The target reuses this image's
    // backing offset for every slot: each slot is a relocated copy of the same
    // linked image, and DXE refuses directories whose offsets differ.
    for slot in 0..pool_bytes / 0x100000 {
        tables[3][((backing_alias(base, slot) >> 12) & 511) as usize] =
            backing_alias_pte(pool_base, slot, backing - base);
    }
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    if vmcb.configure_native_boot_intercepts().is_err() {
        return 4;
    }
    // Initialize before DXE publishes this address in the shared AVIC table.
    if unsafe { &mut *ptr::addr_of_mut!(AVIC_BACKING) }
        .reset_stopped(apic_id as u32, GUEST_APIC_VERSION)
        .is_err()
    {
        return 4;
    }

    unsafe {
        ptr::write(ptr::addr_of_mut!(MSRPM), Msrpm::native_boot());
    }
    let context = unsafe { &mut *ptr::addr_of_mut!(CONTEXT) };
    context.host_stack_top = stack_top;
    context.host_cr3 = root;
    context.guest_vmcb_pa = ptr::addr_of_mut!(VMCB) as u64;
    context.guest_vmcb_va = context.guest_vmcb_pa;
    context.host_extra_pa = ptr::addr_of_mut!(HOST_EXTRA) as u64;
    context.guest_frame_va = ptr::addr_of_mut!(FRAME) as u64;
    context.hsave_pa = ptr::addr_of_mut!(HSAVE) as u64;
    context.host_gdtr_va = ptr::addr_of_mut!(GDTR) as u64;
    context.host_idtr_va = ptr::addr_of_mut!(IDTR) as u64;
    context.owner_context = ptr::addr_of_mut!(STATE) as u64;
    let directory = ResidentDirectory {
        version: DIRECTORY_VERSION,
        arena_base: base,
        arena_bytes: 0x100000,
        context: ptr::addr_of_mut!(CONTEXT) as u64,
        vmcb: context.guest_vmcb_pa,
        auxiliary: ptr::addr_of_mut!(AUX) as u64,
        registers: context.guest_frame_va,
        npt: ptr::addr_of_mut!(NPT) as u64,
        arm: arm as *const () as u64,
        enter: svmvisor_resident_enter as *const () as u64,
        text_end: text_limit,
        data_start: data,
        memory_end: end,
        pool_base,
        pool_bytes,
        cpu_slot,
        apic_id,
        avic_backing: backing,
        reserved: [0; 2],
    };
    unsafe {
        ptr::write(output, directory);
        POOL = (pool_base, pool_bytes);
        ASSIGNED_APIC_ID = apic_id as u32;
    }
    state.prepared = true;
    0
}

/// Vectors whose IDT gate checks the acceptance window (irq.S): 16-255
/// except the #MC gate (18) and the #SX gate (30), which checks the window
/// itself when its error code is not the INIT redirection's 1. External
/// interrupts push no error code (APM2 rev3.44 8.2.24 p261); every other
/// exception vector below 32 cannot be raised inside the window, which runs
/// only NOPs (Table 8-1 p246), so a window event on those vectors is a
/// physical interrupt. Vectors 16-31 keep IST1: outside the window they
/// remain host exceptions.
const fn window_gate(vector: usize) -> bool {
    vector >= 16 && vector < 16 + IRQ_GATES && vector != 18 && vector != 30
}

/// Terminal assembly has copied the normalized first host exception frame to
/// private retained storage and blocked recursive callback entry. No STATE
/// reference may be formed here: a host fault can interrupt a live mutable
/// runtime borrow. Export is a bounded best effort, then this CPU stays stopped.
#[unsafe(no_mangle)]
unsafe extern "C" fn svmvisor_resident_host_fault(frame: *const u64, cr2: u64, cr3: u64) -> ! {
    let vector = unsafe { ptr::read(frame) } as u32;
    let error = unsafe { ptr::read(frame.add(1)) };
    let rip = unsafe { ptr::read(frame.add(2)) };
    let flags = unsafe { ptr::read(frame.add(4)) };
    let rsp = unsafe { ptr::read(frame.add(5)) };
    unsafe {
        diagnostic_record(4, true, [rip, error, cr2, cr3, rsp, flags], vector);
        diagnostics::flush_fault();
    }
    unsafe {
        asm!("cli", "2: hlt", "jmp 2b", options(noreturn, nostack));
    }
}

/// RW/NX leaf of that alias: slot `slot`'s image starts `slot` MiB into the
/// pool and, being a relocated copy of this image, holds its backing page at
/// the same `backing_offset`.
const fn backing_alias_pte(pool_base: u64, slot: u64, backing_offset: u64) -> u64 {
    (pool_base + slot * 0x100000 + backing_offset) | 3 | (1 << 63)
}

/// # Safety
/// Owning CPU, IF=0, native capture/memory admission complete, never-entered
/// exclusively stopped VMCB/frame/NPT already prepared in this owned image.
/// Callback sites identify the audited immutable callback until its guest RET.
/// `map` and `ids` are disjoint valid immutable caller arrays for this call;
/// IDs bind the complete retained pool to admitted native processors. x2APIC
/// must already be enabled on every CPU.
/// A nonnull `initial_icr` is aligned, readable and immutable for this call,
/// revalidated by DXE under the current mapping; its value is copied only.
/// The optional terminal endpoint has the same copied-input lifetime and its
/// complete 56-byte aligned mapping has been validated by the DXE caller.
///
/// Returns 0 when armed, otherwise the refused step: 1 runtime state or CPU
/// identity, 2 CPU capabilities, 3 EFER, 4 bootstrap ACK sites, 5 VMCB
/// controls, 6 memory map, 7 CPU inventory, 8 x2APIC/x2AVIC admission
/// (capabilities, APIC_BASE, host IDs above 254, profile, inherited physical
/// ISR, initial ICR pointer), 9 startup ownership and its commit, 10 terminal
/// endpoint, 11 the loader's x2APIC register state is outside the guest
/// register model (`CapturedInterface`; returned as the typed
/// `captured_register_refusal`, which names the MSR and value), 12 cache
/// replay. Every refusal precedes the first visible change (VM_CR, the
/// destination record, the LAPIC, IsRunning): this CPU's physical-ID table
/// entry is checked first, so the final `set_running` refuses only if the
/// published table changed meanwhile. DXE never enters a runtime whose arm
/// failed.
unsafe extern "win64" fn arm(
    efer: u64,
    resume: u64,
    ack: u64,
    after: u64,
    map: *const MemoryDescriptor,
    count: usize,
    ids: *const u32,
    id_count: usize,
    startup_owned: bool,
    initial_icr: *const u64,
    terminal_endpoint: *const TerminalEndpoint,
) -> u64 {
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if !state.prepared
        || state.armed
        || (!startup_owned && !initial_icr.is_null())
        || __cpuid_count(1, 0).ebx >> 24 != unsafe { ASSIGNED_APIC_ID }
    {
        return 1;
    }
    let Some(caps) = (unsafe { capabilities() }) else {
        return 2;
    };
    let endpoint = if terminal_endpoint.is_null() {
        None
    } else {
        // Caller validated this immutable input mapping for this arm invocation.
        // PPR57896 applies only to Family1Ah Model44h B0, signature00B40F40h.
        if !startup_owned
            || terminal_endpoint as usize & 7 != 0
            || __cpuid_count(1, 0).eax != TARGET_SIGNATURE
        {
            return 10;
        }
        let value = unsafe { terminal_endpoint.read() };
        if !value.valid() || unsafe { read_msr(MMIO_CFG_BASE_ADDR) } != value.mmio_config_msr {
            return 10;
        }
        Some(value)
    };
    if ids.is_null()
        || !(ids as usize).is_multiple_of(core::mem::align_of::<u32>())
        || id_count == 0
        || id_count > 32
        || id_count as u64 != unsafe { POOL.1 } / 0x100000
    {
        return 7;
    }
    let ids = unsafe { core::slice::from_raw_parts(ids, id_count) };
    let assigned_id = unsafe { ASSIGNED_APIC_ID };
    {
        use crate::svm::native_cache::{CacheCapture, CacheObservation};
        // Arm still runs under the admitted caller root, so use the physical
        // pool address. The private read-only alias is used only after entry.
        let capture = unsafe { &*((POOL.0 + super::CACHE_CAPTURE_OFFSET) as *const CacheCapture) };
        if capture.enabled() {
            let Some(slot) = ids.iter().position(|&id| id == assigned_id) else {
                return 12;
            };
            let Some(original) = capture.observation(slot, id_count) else {
                return 12;
            };
            let Some(members) = capture.domain_mask(slot, ids) else {
                return 12;
            };
            let current = CacheObservation::capture(
                __cpuid_count(1, 0).eax,
                caps.address_policy().physical_bits(),
                crate::svm::native_cache::native_topology(),
                |index| unsafe { read_msr(index) },
                |index, value| unsafe { write_msr(index, value) },
            );
            let Some(current) = current else {
                return 12;
            };
            if current.topology[0] != assigned_id || !original.same_physical_state(&current) {
                return 12;
            }
            state.cache_observation = Some(current);
            state.cache_core = members.trailing_zeros() as usize;
            state.cache_visibility = current.sys_cfg & SYS_CFG_MTRR_FIX_DRAM_MOD_EN != 0;
        }
    }
    let Ok(icr_owner) = NativeIcr::admit(assigned_id, ids) else {
        return 7;
    };
    // Exclusive handoff: the loader must already use x2APIC. An xAPIC
    // continuation is refused, never promoted. The captured physical
    // APIC_BASE stays the host interface and seeds the guest shadow (D4).
    let host_apic_base = unsafe { read_msr(apic::APIC_BASE) };
    let Ok(avic_caps) =
        X2AvicCapabilities::admit(__cpuid_count(1, 0).ecx, __cpuid_count(0x8000_000a, 0).edx)
    else {
        return 8;
    };
    let Ok(guest_apic) = GuestX2Apic::admit(host_apic_base, &caps.address_policy()) else {
        return 8;
    };
    // D8: every host ID must be a doorbell target (at most 254), which also
    // bounds the physical-ID table index (15.29.5.2 p571).
    if ids.iter().any(|&id| DoorbellTarget::new(id).is_none())
        || unsafe { read_msr(apic::ID_MSR) } != assigned_id as u64
    {
        return 8;
    }
    let Ok(avic) = NativeX2AvicProfile::new(
        avic_caps,
        ptr::addr_of!(AVIC_BACKING) as u64,
        unsafe { POOL.0 } + super::X2AVIC_TABLE_OFFSET,
        *ids.iter().max().unwrap() as u16,
        &caps.address_policy(),
    ) else {
        return 8;
    };
    // SAFETY: CPL0 callback on its owning CPU with IF=0 (this function's
    // contract); x2APIC is enumerated (X2AvicCapabilities) and enabled
    // (GuestX2Apic); nothing else uses this CPU's LAPIC until its guest runs.
    let mut host = unsafe { HostX2Apic::new() };
    // Before table publication/guest entry, physical sources must have no
    // inherited in-service ownership. Pending physical IRR is captured later.
    if apic::highest_in_service(&mut host).is_some() {
        return 8;
    }
    // The physical bootstrap changed the BSP's ICR. Preserve its captured
    // logical readback in the virtual page without sending a second command.
    let captured_icr = if initial_icr.is_null() {
        host.read(apic::ICR_MSR)
    } else {
        if initial_icr as usize & 7 != 0
            || caps.address_policy().validate(initial_icr as u64, 8, 8).is_err()
        {
            return 8;
        }
        unsafe { initial_icr.read() }
    };
    // The loader's register interface must be representable by the guest
    // register owner before anything changes (D2/D3). ISR/IRR belong to the
    // bridge, which starts empty.
    let interface = match CapturedInterface::capture(&mut host, captured_icr) {
        Ok(interface) => interface,
        Err(refusal) => return super::captured_register_refusal(refusal),
    };
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    let frame = unsafe { &*ptr::addr_of!(FRAME) };
    if let Some(endpoint) = endpoint {
        let slot = ids.iter().position(|&id| id == assigned_id).unwrap();
        if !unsafe { diagnostics::prepare(endpoint, slot, id_count) } {
            return 10;
        }
        let io = unsafe { &mut *ptr::addr_of_mut!(IOPM) };
        if io.set_range(0, 65536, Permission::Allow).is_err()
            || io.set_range(0xcf8, 8, Permission::Intercept).is_err()
            || unsafe { &mut *ptr::addr_of_mut!(MSRPM) }
                .set(MMIO_CFG_BASE_ADDR, MsrAccess::Write, Permission::Intercept)
                .is_err()
        {
            return 10;
        }
        vmcb.set_instruction_intercept(crate::svm::vmcb::InstructionIntercept::Ioio, true);
    }
    // Capture only enumerated same-CPU features, before the first guest entry.
    // Guest FXSR and INVLPG execute directly; VMRUN/VMEXIT owns EFER switching.
    let extended = __cpuid_count(0x8000_0001, 0);
    let extended21 = if __cpuid_count(0x8000_0000, 0).eax >= 0x8000_0021 {
        Some(__cpuid_count(0x8000_0021, 0).eax)
    } else {
        None
    };
    let Ok(mut owner) = NativeEfer::admit_native(
        efer,
        extended.ecx,
        extended.edx,
        __cpuid_count(0x8000_0008, 0).ebx,
        extended21,
    ) else {
        return 3;
    };
    let Ok(ack_owner) = NativeBootstrapAck::new(vmcb, frame, resume, ack, after) else {
        return 4;
    };
    let policy = caps.address_policy();
    if map.is_null()
        || !(map as usize).is_multiple_of(core::mem::align_of::<MemoryDescriptor>())
        || count == 0
        || count > MAX_DESCRIPTORS
    {
        return 6;
    }
    let descriptors = unsafe { core::slice::from_raw_parts(map, count) };
    if ValidatedMemoryMap::new(descriptors, policy.physical_bits()).is_err() {
        return 6;
    }
    // D1 guest x2APIC interception profile. Arm refuses fewer than two CPUs
    // below, so every armed runtime uses it.
    unsafe {
        (&mut *ptr::addr_of_mut!(MSRPM)).configure_native_x2avic();
    }
    if state.cache_observation.is_some() {
        if !startup_owned || unsafe { !cache::prepare_root() } {
            return 12;
        }
        let maps = unsafe { &mut *ptr::addr_of_mut!(MSRPM) };
        for index in crate::svm::native_cache::owned_msrs() {
            for access in [MsrAccess::Read, MsrAccess::Write] {
                if maps.set(index, access, Permission::Intercept).is_err() {
                    return 12;
                }
            }
        }
    }
    if vmcb
        .set_permission_maps(ptr::addr_of!(IOPM) as u64, ptr::addr_of!(MSRPM) as u64, &policy)
        .is_err()
        || vmcb.set_guest_asid(1, &caps).is_err()
        || vmcb.set_nested_root(ptr::addr_of!(NPT) as u64, &policy).is_err()
        || vmcb.enable_native_nested_paging(&policy).is_err()
    {
        return 5;
    }
    if !startup_owned || id_count < 2 {
        return 9;
    }
    let shared = unsafe {
        core::slice::from_raw_parts(
            (POOL.0 + super::STARTUP_PAGE_OFFSET) as *const NativeStartupMailbox,
            id_count,
        )
    };
    // APM2 15.21.2/15.29.5: VMRUN loads V_TPR and AVIC CR8 reads use it.
    // Seed the priority class as well as backing TPR before enabling AVIC.
    if vmcb.set_virtual_interrupt_tpr(interface.task_priority() >> 4).is_err() {
        return 9;
    }
    if shared.iter().zip(ids).any(|(mailbox, &id)| mailbox.identity() != id)
        || vmcb.enable_native_x2avic(&avic).is_err()
    {
        return 9;
    }
    // DXE published this CPU's entry stopped (valid, this backing page, host
    // ID = guest ID) before any arm; check it before anything visible
    // changes, so the final `set_running` only adds IsRunning.
    let table = unsafe { &*((POOL.0 + super::X2AVIC_TABLE_OFFSET) as *const PhysicalIdTable) };
    if !table.is_stopped_entry(assigned_id as u16, ptr::addr_of!(AVIC_BACKING) as u64) {
        return 9;
    }
    owner.enable_guest_startup();
    // Software startup commands retain the existing target-owned mailbox.
    // Ordinary fixed IPIs use x2AVIC, never a physical guest ICR write.
    let Ok(routes) = try_lock_routes(shared) else {
        return 9;
    };
    let slot = ids.iter().position(|&id| id == assigned_id).unwrap();
    let Ok(commit) = routes.prepare_destination_mode(slot, NativeDestinationMode::X2Apic) else {
        return 9;
    };
    unsafe {
        let original = read_msr(VM_CR);
        write_msr(VM_CR, original | VM_CR_R_INIT);
        if read_msr(VM_CR) != original | VM_CR_R_INIT {
            write_msr(VM_CR, original);
            return 9;
        }
    }
    commit.commit_destination_mode();
    // The captured interface becomes this vCPU's backing state. A physical
    // LVT changes only where the virtual APIC masks its source (D3), before
    // the host enables its own physical SVR below.
    interface.install(backing, &mut host);
    // Host physical TPR must not inherit a guest priority threshold. Guest CR8
    // and TPR now use AVIC; the physical LAPIC is a source capture backend.
    host.write(apic::msr(apic::TPR), 0);
    // Host capture owns physical software-enable and spurious vector FFh.
    // The captured guest SVR remains in its separate backing register.
    host.write(apic::msr(apic::SVR), u64::from(apic::SVR_SOFTWARE_ENABLE | 0xff));
    if table.set_running(assigned_id as u16, true).is_err() {
        return 9;
    }
    state.avic = Some(avic);
    unsafe {
        ptr::copy_nonoverlapping(map, ptr::addr_of_mut!(RAM).cast(), count);
        RAM_COUNT = count;
        PHYSICAL_BITS = policy.physical_bits();
    }
    state.ack = Some(ack_owner);
    state.efer = Some(owner);
    state.icr = Some(icr_owner);
    state.guest_apic = Some(guest_apic);
    state.host_apic_base = host_apic_base;
    state.startup_owned = startup_owned;
    state.slot = ids.iter().position(|&id| id == assigned_id).unwrap();
    state.count = id_count;
    state.terminal_endpoint = endpoint;
    // PAUSE filtering is optional; unsupported CPUs retain direct PAUSE.
    vmcb.configure_native_pause_filter(__cpuid_count(0x8000_000a, 0).edx);
    state.capabilities = Some(caps);
    state.armed = true;
    0
}

const _: crate::host::resident::ArmRuntime = arm;

/// Capability gating precedes every SVM-related MSR access in the caller.
/// Here VM_CR is read only after enumerated native AMD SVM. The shared native
/// encryption plan gates control reads and refuses unsupported enabled modes.
unsafe fn capabilities() -> Option<ValidatedCapabilities> {
    let basic = __cpuid_count(0, 0);
    let ext = __cpuid_count(0x80000000, 0);
    if basic.ebx != 0x68747541
        || basic.edx != 0x69746e65
        || basic.ecx != 0x444d4163
        || basic.eax < 1
        || ext.eax < 0x8000000a
    {
        return None;
    }
    let one = __cpuid_count(1, 0);
    let extone = __cpuid_count(0x80000001, 0);
    let svm = __cpuid_count(0x8000000a, 0);
    if one.ecx >> 31 != 0
        || extone.ecx & 4 == 0
        || extone.edx & ((1 << 20) | (1 << 26)) != ((1 << 20) | (1 << 26))
        || svm.edx & 1 != 1
        || svm.eax != 1
    {
        return None;
    }
    let width = __cpuid_count(0x80000008, 0).eax as u8;
    let encryption_leaf = if ext.eax >= 0x8000001f {
        let leaf = __cpuid_count(0x8000001f, 0);
        Some([leaf.eax, leaf.ebx, leaf.ecx, leaf.edx])
    } else {
        None
    };
    let plan = NativeEncryptionPlan::new(one.eax, width, encryption_leaf).ok()?;
    let encryption = plan
        .validate(
            plan.sys_cfg_msr().map(|msr| unsafe { read_msr(msr) }),
            plan.sev_status_msr().map(|msr| unsafe { read_msr(msr) }),
        )
        .ok()?;
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr",in("ecx")VM_CR,out("eax")low,out("edx")high,options(nostack));
    }
    if u64::from(low) & VM_CR_SVMDIS != 0 || high != 0 {
        return None;
    }
    CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(svm.eax as u8),
        asid_count: Some(svm.ebx),
        physical_address_bits: Some(width),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption,
        optional: OptionalFeatures { nrip_save: svm.edx & 8 != 0, ..Default::default() },
    }
    .validate()
    .ok()
}

/// # Safety
/// Called only by the audited integer assembly after VMEXIT, host auxiliary
/// restore, private stack and GIF/IF clear. The one stopped guest is exclusive.
unsafe extern "win64" fn dispatch(context: *mut BridgeContext) -> bool {
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if context != ptr::addr_of_mut!(CONTEXT) {
        let exit = unsafe { &*ptr::addr_of!(VMCB) }.exit_snapshot();
        stop(state, exit.code, exit.rip, 0xf10a, context as u64);
        unsafe { terminal_finish(state) };
        return false;
    }
    if unsafe { terminal_requested(state) } {
        unsafe { terminal_finish(state) };
        return false;
    }
    let before = unsafe { &*ptr::addr_of!(VMCB) }.exit_snapshot();
    unsafe {
        let frame = ptr::read_volatile(ptr::addr_of!(FRAME));
        let rax = (&*ptr::addr_of!(VMCB)).guest_rax();
        let index = EXIT_HISTORY_NEXT;
        ptr::addr_of_mut!(EXIT_HISTORY).cast::<[u64; 6]>().add(index).write([
            before.rip,
            before.code,
            before.info1,
            before.info2,
            frame.rcx,
            (rax as u32 as u64) | ((frame.rdx as u32 as u64) << 32),
        ]);
        EXIT_HISTORY_NEXT = (index + 1) % 5;
        EXIT_HISTORY_COUNT = (EXIT_HISTORY_COUNT + 1).min(5);
    }

    // The inner body's RAII route guards must be gone before terminal work.
    let resume = unsafe { dispatch_body(context) };
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    // Every terminal return needs evidence, including an unforeseen callee
    // refusal. Route guards have unwound before publishing the stop/barrier.
    if !unsafe { terminal_requested(state) } {
        record_unexplained_stop(state, resume, before);
    }
    if unsafe { terminal_requested(state) } || (!resume && state.stopped_valid) {
        unsafe { terminal_finish(state) };
        return false;
    }
    if resume && before.code != 0x77 && (before.code != 0x72 || state.exits & 0xff == 1) {
        let vmcb = unsafe { &*ptr::addr_of!(VMCB) };
        unsafe {
            diagnostic_record(
                2,
                false,
                [
                    vmcb.guest_rip(),
                    before.code,
                    before.info1,
                    before.info2,
                    vmcb.guest_cr3(),
                    state.exits,
                ],
                0,
            );
        }
    }
    resume
}

unsafe fn dispatch_body(context: *mut BridgeContext) -> bool {
    // The outer dispatcher validated context without dereferencing it.
    debug_assert!(context == ptr::addr_of_mut!(CONTEXT));
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    let frame = unsafe { &mut *ptr::addr_of_mut!(FRAME) };
    state.exits = state.exits.saturating_add(1);
    if !state.armed {
        // F10Bh without detail: an exit before arm. With the startup-route
        // record retired, `stop_words` exports it as an unhandled exit.
        let exit = vmcb.exit_snapshot();
        return stop(state, exit.code, exit.rip, 0xf10b, 0);
    }
    if state.avic.as_ref().is_none_or(|profile| vmcb.validate_native_x2avic(profile).is_err()) {
        let exit = vmcb.exit_snapshot();
        let (tag, value) = terminal::profile_mismatch_at_entry(vmcb.virtual_interrupt_control());
        return stop(state, exit.code, exit.rip, tag, value);
    }
    let observed = vmcb.exit_snapshot();
    // Every unusual exit and MSR boundary; common CPUID/PAUSE samples are
    // bounded to avoid making diagnostic PCI traffic dominate guest execution.
    if !matches!(observed.code, 0x72 | 0x77) || state.exits & 0xff == 1 {
        unsafe {
            diagnostic_record(
                1,
                false,
                [
                    observed.rip,
                    observed.code,
                    observed.info1,
                    observed.info2,
                    vmcb.guest_cr3(),
                    state.exits,
                ],
                if observed.code == 0x7c { frame.rcx as u32 } else { 0 },
            );
        }
    }
    // Audited assembly has returned from VMRUN on this CPU. Consume its old
    // flush before INIT, EFER or any other dispatcher mutation can re-arm it.
    // An invalid entry does not establish that the requested flush occurred.
    unsafe {
        vmcb.consume_tlb_flush_after_exit();
    }
    let mut nmi_drained = false;
    if state.startup_owned {
        #[cfg(feature = "resident-runtime-test")]
        let irq_witness = unsafe {
            (
                read_native_apic(apic::TPR),
                read_native_apic(apic::IRR + 7 * 16),
                read_native_apic(apic::ISR + 7 * 16),
            )
        };
        let Some(acknowledged) = (unsafe { acknowledge_init() }) else {
            return stop(state, vmcb.exit_snapshot().code, vmcb.guest_rip(), 0xf102, 0);
        };
        // A physical NMI held pending in a host GIF window (including the one
        // acknowledge_init just opened, and the one still pending after a
        // VMEXIT_NMI) is taken by the host vector-2 gate, which sets the NMI
        // flag. Re-present it to the guest as V_NMI before the next VMRUN
        // (15.21.10 p536), so a physical NMI never re-fires on entry.
        nmi_drained = unsafe { route_physical_nmi_to_guest(state, vmcb) };
        if vmcb.exit_snapshot().code == 0x63 && acknowledged == 0 {
            return stop(state, 0x63, vmcb.guest_rip(), 0xf105, 0);
        }
        if vmcb.exit_snapshot().code == 0x63 {
            state.intr = state.intr.saturating_add(1);
        }
        // Broadcast notifications also reach unrelated CPUs. Keep their actual
        // wake count, but emit command evidence only on the queued target so
        // simultaneous empty wakes cannot interleave diagnostic port writes.
        if vmcb.exit_snapshot().code == 0x63
            && (unsafe { mailboxes(state.count) })[state.slot].peek().is_some()
        {
            debug(b"resident-physical-init cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b" rip=");
            hex(vmcb.guest_rip());
            debug(b" count=");
            hex(state.intr);
            debug(b" sx-acks=");
            hex(svmvisor_resident_init_acks.load(Ordering::Acquire));
            #[cfg(feature = "resident-runtime-test")]
            {
                debug(b" tpr=");
                hex(irq_witness.0);
                debug(b" irr-f1=");
                hex(irq_witness.1 & (1 << 17));
                debug(b" isr-f1=");
                hex(irq_witness.2 & (1 << 17));
                // Evidence only: asynchronous device arrivals may legitimately
                // change IRR. The controlled fixture checks its own stable case.
                debug(b" tpr-after=");
                hex(unsafe { read_native_apic(apic::TPR) });
                debug(b" irr-f1-after=");
                hex(unsafe { read_native_apic(apic::IRR + 7 * 16) } & (1 << 17));
                debug(b" isr-f1-after=");
                hex(unsafe { read_native_apic(apic::ISR + 7 * 16) } & (1 << 17));
            }
            debug(b"\n");
        }
    }
    // After the only host GIF window before this exit's handler (above): a
    // VMEXIT_NMI whose pending NMI that window did not take would exit again
    // at the next VMRUN.
    if nmi_drain_stalled(&mut state.nmi_drain_misses, observed.code, nmi_drained) {
        return stop(
            state,
            observed.code,
            observed.rip,
            NMI_DRAIN_STALL,
            u64::from(state.nmi_drain_misses),
        );
    }
    if !check_exit_event(state, vmcb) {
        return false;
    }
    let exit = vmcb.exit_snapshot();
    let Some(ack) = state.ack.as_mut() else {
        return stop(state, exit.code, exit.rip, 0xf10d, 0);
    };
    if !ack.acknowledged() {
        // A physical source can arrive before the bootstrap VMMCALL. Capture
        // it without falsely treating that asynchronous exit as a failed
        // guest ACK; no startup command is serviced before the ACK.
        if exit.code == 0x60 {
            return unsafe { capture_physical_irq(state, vmcb) };
        }
        // Likewise a physical NMI held pending while arm ran with GIF=0
        // (Table 15-10 p530) exits at the first VMRUN (Table 15-13 p536). The
        // window above drained it and set V_NMI; the drain watchdog ran.
        if exit.code == 0x61 {
            return true;
        }
        if ack.acknowledge(vmcb, frame).is_ok() {
            if state.startup_owned {
                (unsafe { mailboxes(state.count) })[state.slot].mark_running();
                if terminal_enabled(state) {
                    // Separate monotonic initial-ACK mask, not mutable guest
                    // startup readiness. Last initial guest ACK opens export.
                    unsafe { terminal_control() }.initial_ack(state.slot, state.count);
                    unsafe {
                        diagnostic_record(
                            9,
                            false,
                            [
                                vmcb.guest_rip(),
                                vmcb.guest_cr3(),
                                state.slot as u64,
                                state.count as u64,
                                0,
                                0,
                            ],
                            0,
                        );
                    }
                }
            }
            debug(b"resident-ack\n");
            return true;
        }
        return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
    }
    let mut context = ExitContext { state, vmcb, frame, exit };
    sequence_exit(
        &mut context,
        exit_order(exit.code),
        // SAFETY: this CPU's armed dispatcher with its guest stopped and
        // IF/GIF clear, the contract of every exit handler below.
        |context| unsafe { handle_exit(context) },
        |context| unsafe { startup_service(context) },
    )
}

const fn exit_order(code: u64) -> ExitOrder {
    match code {
        // 0x61 (physical NMI) is an asynchronous event at an instruction
        // boundary, like INTR/AVIC: its V_NMI re-presentation is complete
        // before a later queued INIT, so handle it first.
        0x60 | 0x61 | 0x401 | 0x402 => ExitOrder::TrapThenStartup,
        _ => ExitOrder::StartupThenExit,
    }
}

/// Run an exit's handler and the startup service in `order`. The service
/// returns `Some(resume)` when it decided the exit (a changed guest or a
/// stop) and `None` when there was nothing to service.
fn sequence_exit<C>(
    context: &mut C,
    order: ExitOrder,
    handle: impl FnOnce(&mut C) -> bool,
    service: impl FnOnce(&mut C) -> Option<bool>,
) -> bool {
    match order {
        ExitOrder::TrapThenStartup => handle(context) && service(context).unwrap_or(true),
        ExitOrder::StartupThenExit => match service(context) {
            Some(resume) => resume,
            None => handle(context),
        },
    }
}

/// `service_startup` for one exit, with its stop fallback.
/// # Safety
/// As `service_startup`.
unsafe fn startup_service(context: &mut ExitContext<'_>) -> Option<bool> {
    let ExitContext { state, vmcb, frame, exit } = context;
    let (state, vmcb, frame, exit) = (&mut **state, &mut **vmcb, &mut **frame, *exit);
    if !state.startup_owned {
        return None;
    }
    match unsafe { service_startup(state, vmcb, frame) } {
        Some(true) => Some(true),
        Some(false) if unsafe { terminal_requested(state) } => Some(false),
        Some(false) if state.stopped_valid => Some(false),
        Some(false) => Some(stop(state, exit.code, exit.rip, 0xf103, 0)),
        None => None,
    }
}

/// The acknowledged guest's exit handlers.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
unsafe fn handle_exit(context: &mut ExitContext<'_>) -> bool {
    let ExitContext { state, vmcb, frame, exit } = context;
    let (state, vmcb, frame, exit) = (&mut **state, &mut **vmcb, &mut **frame, *exit);
    #[cfg(feature = "resident-runtime-test")]
    if exit.code == 0x400 && state.cache_fixture && state.cache_active {
        debug(b"native-cache-fixture-low-npf gpa=");
        hex(exit.info2);
        debug(b" root=");
        hex(vmcb.nested_root());
        debug(b"\n");
    }
    match exit.code {
        0x60 => return unsafe { capture_physical_irq(state, vmcb) },
        0x61 => {
            // Physical NMI intercept under NMI virtualization (Table 15-13
            // p536, 15.21.10 p536): the NMI is still pending after the exit
            // and was drained by acknowledge_init's GIF window (host vector-2
            // gate) before this handler runs. Re-present it to the guest as a
            // virtual NMI so Windows still receives platform NMIs. Setting
            // V_NMI is idempotent with the flag routing that already ran.
            return match state.avic {
                Some(profile) => match vmcb.set_guest_v_nmi_pending(&profile) {
                    Ok(()) => {
                        state.routing_retries = 0;
                        true
                    }
                    Err(_) => stop(state, exit.code, exit.rip, exit.info1, exit.info2),
                },
                None => stop(state, exit.code, exit.rip, X2AvicStop::ProfileMismatch as u64, 0),
            };
        }
        0x401 | 0x402 => return unsafe { handle_avic_exit(state, vmcb, frame) },
        0x77 => {
            // Reenter unchanged: VMRUN replenishes the nonzero PAUSE count,
            // and hardware executes this instruction, including debug state.
            unsafe {
                diagnostic_record(5, false, [exit.rip, vmcb.guest_cr3(), state.exits, 0, 0, 0], 0);
            }
            if crate::svm::native_pause::native_pause_retry_ready(vmcb) {
                return true;
            }
            return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
        }
        0x63 if state.startup_owned => {
            // Actual host #SX(error1) acknowledgment was checked above. A
            // notification is only a wakeup: commands live in the mailbox,
            // and multiple notifications may coalesce without losing commands.
            // `check_exit_event` owns the pending-event state: like every
            // intercept, this one may report an interrupted delivery (15.7.2
            // p509), which it has re-injected, so EVENTINJ.V is not a refusal.
            return true;
        }
        0x72 => {
            // The per-CPU arm observation is retained in this private runtime.
            // APM2 15.7.1 makes hardware nRIP authoritative for instruction
            // intercepts. Select this path before any guest-memory access;
            // invalid nRIP must stop, never fall back to rereading the opcode.
            let hardware_nrip =
                state.capabilities.filter(|caps| caps.optional_features().nrip_save);
            let mut prefixed = None;
            if let Some(caps) = hardware_nrip.as_ref() {
                let next = match exit.resume_candidate(caps) {
                    Ok(next) if next.instruction_bytes() >= 2 => next,
                    _ => return stop(state, exit.code, exit.rip, 0xf001, 0x100),
                };
                if !dispatch::native_cpuid_mode(vmcb, next.address(), state.startup_owned) {
                    return stop(state, exit.code, exit.rip, 0xf001, 0x101);
                }
                if next.instruction_bytes() > 2 {
                    let length = next.instruction_bytes() as usize;
                    let mut reader =
                        match unsafe { GuestReader::new(vmcb, state.startup_owned, state.count) } {
                            Ok(reader) => reader,
                            Err(reason) => {
                                return stop(state, exit.code, exit.rip, 0xf001, reason as u64);
                            }
                        };
                    let bytes = super::fetch::cpuid_instruction(
                        vmcb,
                        reader.width,
                        reader.guest_pat,
                        length,
                        state.startup_owned,
                        |address, bytes| unsafe { reader.read(address, bytes) },
                    );
                    match bytes {
                        Ok(bytes) => prefixed = Some((bytes, length)),
                        Err(error) => {
                            let reason = reader
                                .failure
                                .map_or_else(|| terminal::fetch_failure_code(error), |e| e as u16);
                            if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 {
                                return retry_routing(state, vmcb);
                            }
                            return stop(state, exit.code, exit.rip, 0xf001, reason as u64);
                        }
                    }
                }
            }
            let instruction = if hardware_nrip.is_none() {
                let bytes = match unsafe { cache::fetch(state, vmcb) } {
                    Ok(bytes) => bytes,
                    Err(reason)
                        if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 =>
                    {
                        return retry_routing(state, vmcb);
                    }
                    Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
                };
                Some(bytes)
            } else {
                None
            };
            let leaf = vmcb.guest_rax() as u32;
            let native = __cpuid_count(leaf, frame.rcx as u32);
            let response = [native.eax, native.ebx, native.ecx, native.edx];
            #[cfg(feature = "resident-runtime-test")]
            let response = if leaf == 0x4fff_ca00 {
                match unsafe { cache::fixture_control(state, vmcb, frame.rcx as u32) } {
                    Some(value) => value,
                    None => return stop(state, exit.code, exit.rip, 0xf4ff, frame.rcx),
                }
            } else {
                response
            };
            #[cfg(feature = "resident-runtime-test")]
            let response = if leaf == 0x4fff0000 {
                [0x53564d52, state.exits as u32, state.cpuid as u32, state.msr as u32]
            } else {
                response
            };
            state.cpuid = state.cpuid.saturating_add(1);
            let cpuid_user_disabled =
                vmcb.bytes()[0x4cb] != 0 && unsafe { read_msr(HWCR) } & HWCR_CPUID_FLT_EN != 0;
            let result = if let Some(caps) = hardware_nrip {
                dispatch::handle_native_cpuid_with_nrip(
                    vmcb,
                    frame,
                    &caps,
                    response,
                    state.startup_owned,
                    cpuid_user_disabled,
                    prefixed.as_ref().map(|(bytes, length)| &bytes[..*length]),
                )
            } else if cpuid_user_disabled {
                // The byte fallback has no owned CPUID-fault injection path.
                return stop(state, exit.code, exit.rip, 0xf111, 1 << 35);
            } else if state.startup_owned {
                dispatch::handle_native_startup_cpuid(vmcb, frame, &instruction.unwrap(), response)
            } else {
                dispatch::handle_native_cpuid(vmcb, frame, &instruction.unwrap(), response)
            };
            if result.is_ok() {
                if result == Ok(dispatch::DispatchOutcome::GeneralProtectionPrepared) {
                    state.pending_fault = true;
                }
                state.routing_retries = 0;
                return true;
            }
        }
        0x7b if diagnostics::available() => return unsafe { diagnostics::handle_io(state, vmcb) },
        0x7c => {
            if state.cache_observation.is_some()
                && crate::svm::native_cache::owned_msr(frame.rcx as u32)
            {
                return unsafe { cache::handle(state, vmcb, frame) };
            }
            if frame.rcx as u32 == MMIO_CFG_BASE_ADDR && diagnostics::available() {
                // Dynamic ECAM relocation is not yet an owned instruction path.
                // Keep the actual stopped operands before any native write.
                let requested =
                    (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32);
                return stop(state, exit.code, exit.rip, 0xf202, requested);
            }
            if frame.rcx as u32 == SYS_CFG {
                return unsafe { handle_syscfg(state, vmcb, frame) };
            }
            if frame.rcx as u32 == apic::APIC_BASE
                || (apic::X2APIC_MSR_FIRST..=apic::X2APIC_MSR_LAST).contains(&(frame.rcx as u32))
            {
                return unsafe { handle_avic_msr(state, vmcb, frame) };
            }
            if let Some(resume) = unsafe { handle_mcax_msr(state, vmcb, frame) } {
                return resume;
            }
            // Actual same-CPU MSR exit plus NRIPS owns the decoded instruction
            // length, including prefixes. Route owned MSRs before guest-byte fetch.
            // Other MSRs and non-long64 profiles retain their existing byte owner.
            let hardware_nrip = state.capabilities.filter(|caps| {
                caps.optional_features().nrip_save
                    && vmcb.guest_in_64_bit_code()
                    && vmcb.bytes()[0x4cb] == 0
                    && matches!(frame.rcx as u32, 0xc000_0080 | VM_CR)
            });
            if frame.rcx as u32 == VM_CR
                && let Some(caps) = hardware_nrip
            {
                match dispatch::handle_native_vmcr_with_nrip(
                    vmcb,
                    frame,
                    &caps,
                    state.startup_owned,
                ) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) => {
                        let (reason, value) = super::terminal::vmcr_nrip_failure(
                            error,
                            vmcb,
                            dispatch::NATIVE_VM_CR_VALUE,
                        );
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }
            if let (Some(caps), Some(efer)) = (hardware_nrip, state.efer.as_mut()) {
                match dispatch::handle_native_efer_with_nrip(efer, vmcb, frame, &caps) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) => {
                        let (reason, value) =
                            super::terminal::efer_nrip_failure(error, vmcb, efer.logical());
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }
            let instruction = match unsafe {
                fetch_instruction(vmcb, state.startup_owned, state.count)
            } {
                Ok(bytes) => bytes,
                Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => {
                    return retry_routing(state, vmcb);
                }
                Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
            };
            state.msr = state.msr.saturating_add(1);
            if frame.rcx as u32 == VM_CR {
                match dispatch::handle_native_vmcr(vmcb, frame, &instruction, state.startup_owned) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) => {
                        let (reason, value) = super::terminal::vmcr_failure(
                            error,
                            vmcb,
                            dispatch::NATIVE_VM_CR_VALUE,
                            instruction,
                        );
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }

            if let Some(efer) = state.efer.as_mut() {
                match dispatch::handle_native_efer(efer, vmcb, frame, &instruction) {
                    Ok(NativeMsrOutcome::Completed) => {
                        state.routing_retries = 0;
                        return true;
                    }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => {
                        state.pending_fault = true;
                        return true;
                    }
                    Err(error) if frame.rcx as u32 == 0xc000_0080 => {
                        let (reason, value) =
                            super::terminal::efer_failure(error, vmcb, efer.logical(), instruction);
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                    Err(_) => {}
                }
            }
            // A decoded but unsupported/failed MSR exit remains stopped. Keep
            // its operand index in the typed terminal record; VMCB/GPR state
            // and the raw architectural exit fields remain untouched.
            return stop(state, exit.code, exit.rip, 0xf104, frame.rcx);
        }
        0x400 if state.startup_owned => {
            if let Some(endpoint) = unsafe { diagnostics::endpoint() }
                && let Some((base, bytes)) = endpoint.config_aperture()
                && let Ok((start, end)) = crate::memory::npt::identity_protection_range(base, bytes)
                && (start..end).contains(&exit.info2)
            {
                return unsafe { handle_diagnostic_ecam(state, vmcb, base, bytes) };
            }
            return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
        }
        _ => {}
    }
    stop(state, exit.code, exit.rip, exit.info1, exit.info2)
}

unsafe fn handle_diagnostic_ecam(
    state: &mut State,
    vmcb: &mut Vmcb,
    base: u64,
    bytes: u64,
) -> bool {
    let exit = vmcb.exit_snapshot();
    if exit.info1 & 0x1f != 7 {
        return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
    }
    let Some(guard) = (unsafe { terminal_control() }).diagnostic_lock() else {
        return retry_routing(state, vmcb);
    };
    // Remove only our write restriction after noting the first such write.
    // Hardware retries the original instruction; no GPR/RIP/event is emulated.
    unsafe {
        diagnostics::note_config_write(exit.info2, 0, 0, 1);
    }
    let result = crate::memory::npt::restore_identity_write_range(
        unsafe { &mut *ptr::addr_of_mut!(NPT) },
        ptr::addr_of!(NPT) as u64,
        base,
        bytes,
    );
    drop(guard);
    if result.is_err() {
        return stop(state, exit.code, exit.rip, 0xf203, exit.info2);
    }
    vmcb.request_full_tlb_flush();
    state.routing_retries = 0;
    true
}

/// Accept one physical source through the bounded assembly mailbox and hand
/// it to the host IRQ bridge (`capture_accepted`). The gate touches no Rust
/// owner; IF/GIF are clear before this function reads state.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped.
unsafe fn capture_physical_irq(state: &mut State, vmcb: &mut Vmcb) -> bool {
    let exit = vmcb.exit_snapshot();
    let vector = unsafe { svmvisor_resident_accept_irq() };
    if vector == u32::MAX {
        return retry_routing(state, vmcb);
    }
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    // SAFETY: armed dispatcher on its own CPU with IF/GIF clear; arm admitted
    // this CPU's enabled x2APIC, and only this runtime uses it.
    let mut host = unsafe { HostX2Apic::new() };
    // SAFETY: this function's contract; no other MSRPM reference is live.
    let msrpm = unsafe { local_msrpm() };
    match capture_accepted(vector, backing, &mut state.irq, &mut host, msrpm, vmcb) {
        // A physical spurious interrupt: nothing to publish or acknowledge.
        Ok(None) => {}
        Ok(Some(Capture::Discarded)) => state.irq_discards = state.irq_discards.saturating_add(1),
        Ok(Some(_)) => state.intr = state.intr.saturating_add(1),
        Err((tag, value)) => return stop(state, exit.code, exit.rip, tag, value),
    }
    state.routing_retries = 0;
    true
}

/// Bridge one vector that the acceptance helper returned (`irq::capture`),
/// then resynchronize the guest EOI intercept, since a newly held level
/// source needs its guest EOI intercepted (D6). `Err` is a stop reason and
/// value: a helper result above 255, or a bridge failure (vectors 16-31
/// included, which the host IDT reports through the window gates).
fn capture_accepted(
    vector: u32,
    backing: &BackingPage,
    ledger: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
    msrpm: &mut Msrpm,
    vmcb: &mut Vmcb,
) -> Result<Option<Capture>, (u64, u64)> {
    let Ok(vector) = u8::try_from(vector) else {
        return Err((X2AvicStop::AcceptedVector as u64, u64::from(vector)));
    };
    let capture = irq::capture(vector, backing, ledger, physical)
        .map_err(|error| terminal::irq_failure(IrqSite::Capture, error))?;
    sync_eoi_intercept(ledger, msrpm, vmcb);
    Ok(capture)
}

/// Intercepted guest x2APIC (800h-8FFh) and APIC_BASE accesses (D1). The
/// register owner emulates each one (D2-D4, D6); this boundary owns the
/// instruction evidence, the continuation and the fault/stop mapping. Every
/// fallible check precedes the emulation, whose only fallible effect (a level
/// completion after a software EOI) is terminal.
unsafe fn handle_avic_msr(state: &mut State, vmcb: &mut Vmcb, frame: &mut GuestRegisters) -> bool {
    use crate::svm::exit::MsrInstruction;
    let exit = vmcb.exit_snapshot();
    let boundary = X2AvicStop::MsrBoundary as u64;
    let Some(profile) = state.avic.filter(|_| state.guest_apic.is_some()) else {
        return stop(state, exit.code, exit.rip, boundary, 0);
    };
    if vmcb.validate_native_x2avic(&profile).is_err()
        || vmcb.validate_external_interrupt_conflicts().is_err()
    {
        return stop(state, exit.code, exit.rip, boundary, 1);
    }
    // APM2 15.7.1 p509: MSR intercepts save nRIP. Guest code outside 64-bit
    // mode (an AP startup trampoline) keeps the byte-owned continuation of
    // the startup instruction fetch.
    let caps = state
        .capabilities
        .filter(|caps| caps.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_none() {
        match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
            Ok(bytes) => Some(bytes),
            Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => {
                return retry_routing(state, vmcb);
            }
            Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
        }
    } else {
        None
    };
    let evidence = match caps {
        Some(caps) => match MsrInstruction::hardware(exit, &caps) {
            Ok(evidence) => evidence,
            Err(_) => return stop(state, exit.code, exit.rip, boundary, 2),
        },
        None => MsrInstruction::Bytes(bytes.as_ref().unwrap()),
    };
    let Ok(next) = evidence.continuation(exit) else {
        return stop(state, exit.code, exit.rip, boundary, 2);
    };
    if !dispatch::native_startup_instruction_mode(vmcb, evidence.length())
        || vmcb.guest_rflags() & (1 << 8) != 0
    {
        return stop(state, exit.code, exit.rip, boundary, 3);
    }
    let index = frame.rcx as u32;
    let write = (exit.info1 == 1)
        .then(|| (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32));
    // RDMSR/WRMSR above CPL0 fault before the MSRPM check (15.11 p518); keep
    // that #GP(0) should such an exit ever be observed.
    let outcome = if vmcb.bytes()[0x4cb] != 0 {
        Emulation::GeneralProtection
    } else if index == apic::msr(apic::EOI)
        && write.is_some()
        && state.irq.take_eoi_replay(exit.rip)
        && write == Some(0)
    {
        // The re-execution of an EOI write that a level-EOI AVIC_NOACCEL
        // exit already completed (`irq::level_eoi_exit`): no second EOI.
        Emulation::Written
    } else {
        let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
        // SAFETY: armed dispatcher on its own CPU with IF/GIF clear; arm
        // admitted this CPU's enabled x2APIC, and only this runtime uses it.
        let mut host = unsafe { HostX2Apic::new() };
        match state.guest_apic.as_mut() {
            Some(guest) => guest.emulate(index, write, backing, &mut state.irq, &mut host),
            None => return stop(state, exit.code, exit.rip, boundary, 0),
        }
    };
    let completion = msr_completion(outcome, index, write.is_some());
    match apply_msr_completion(vmcb, frame, &profile, completion, next) {
        Ok(fault) => state.pending_fault |= fault,
        Err((tag, value)) => return stop(state, exit.code, exit.rip, tag, value),
    }
    // A software EOI can release the last held level source (D6).
    // SAFETY: this function's contract; no other MSRPM reference is live.
    sync_eoi_intercept(&state.irq, unsafe { local_msrpm() }, vmcb);
    state.routing_retries = 0;
    true
}

/// Intercepted MCAX machine-check MSR (`native_mcax`): the MSRPM cannot cover
/// C000_2000h-23FFh (APM2 rev3.44 Table 15-8 p518), so every guest access
/// exits and is repeated here on its own CPU. `None`: not an MCAX MSR, or a
/// boundary this path does not own (no NRIPS, outside 64-bit code, TF, a
/// changed profile or a pending event); the caller keeps its F104h stop.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
unsafe fn handle_mcax_msr(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> Option<bool> {
    use crate::svm::{
        exit::MsrInstruction,
        native_mcax::{self, Access},
    };
    let exit = vmcb.exit_snapshot();
    let index = frame.rcx as u32;
    if !(native_mcax::FIRST..=native_mcax::LAST).contains(&index) {
        return None;
    }
    let write = (exit.info1 == 1)
        .then(|| (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64) << 32));
    let status_writable = unsafe { read_msr(HWCR) } & HWCR_MC_STATUS_WR_EN != 0;
    let access = native_mcax::plan(index, write, status_writable)?;
    let profile = state.avic?;
    // RDMSR/WRMSR above CPL0 fault before the MSRPM check (15.11 p518).
    let caps = state.capabilities.filter(|caps| {
        caps.optional_features().nrip_save
            && vmcb.guest_in_64_bit_code()
            && vmcb.bytes()[0x4cb] == 0
    })?;
    let evidence = MsrInstruction::hardware(exit, &caps).ok()?;
    let next = evidence.continuation(exit).ok()?;
    if vmcb.validate_native_x2avic(&profile).is_err()
        || vmcb.validate_external_interrupt_conflicts().is_err()
        || !dispatch::native_startup_instruction_mode(vmcb, evidence.length())
        || vmcb.guest_rflags() & (1 << 8) != 0
    {
        return None;
    }
    let completion = match access {
        // PPR57896 rev3.00 p300: unimplemented and unused registers in this
        // space are RAZ/WRIG, so neither host access can fault.
        Access::Read => MsrCompletion::Complete { read: Some(unsafe { read_msr(index) }) },
        Access::Write(value) => {
            unsafe {
                write_msr(index, value);
            }
            MsrCompletion::Complete { read: None }
        }
        Access::ReadZeroIgnoreWrite => {
            MsrCompletion::Complete { read: write.is_none().then_some(0) }
        }
        Access::GeneralProtection => MsrCompletion::Fault,
    };
    Some(match apply_msr_completion(vmcb, frame, &profile, completion, next) {
        Ok(fault) => {
            state.pending_fault |= fault;
            state.msr = state.msr.saturating_add(1);
            state.routing_retries = 0;
            true
        }
        Err((tag, value)) => stop(state, exit.code, exit.rip, tag, value),
    })
}

/// Apply one completion to the stopped guest. A completed RDMSR loads
/// EDX:EAX (in 64-bit mode the upper halves of RAX and RDX become zero), and
/// every completed access continues at `next` with the interrupt shadow and
/// RF consumed. A fault queues #GP(0) at the unchanged RIP (`Ok(true)`).
/// `Err` is a stop reason and value; the guest is then unchanged.
fn apply_msr_completion(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    profile: &NativeX2AvicProfile,
    completion: MsrCompletion,
    next: ResumeCandidate,
) -> Result<bool, (u64, u64)> {
    match completion {
        MsrCompletion::Complete { read } => {
            let rax = match read {
                Some(value) => {
                    frame.rdx = value >> 32;
                    value & 0xffff_ffff
                }
                None => vmcb.guest_rax(),
            };
            vmcb.commit_emulated_instruction(rax, next);
            vmcb.complete_native_instruction_state();
            Ok(false)
        }
        MsrCompletion::Fault => match vmcb.queue_native_x2avic_general_protection(profile) {
            Ok(()) => Ok(true),
            Err(_) => Err((X2AvicStop::MsrBoundary as u64, 4)),
        },
        MsrCompletion::Stop(tag, value) => Err((tag, value)),
    }
}

fn msr_completion(outcome: Emulation, index: u32, write: bool) -> MsrCompletion {
    match outcome {
        Emulation::Read(value) => MsrCompletion::Complete { read: Some(value) },
        Emulation::Written => MsrCompletion::Complete { read: None },
        Emulation::GeneralProtection => MsrCompletion::Fault,
        Emulation::Refused { reason, value } => {
            let (tag, value) = terminal::register_refusal(reason, index, write, value);
            MsrCompletion::Stop(tag, value)
        }
        Emulation::EoiFailed(error) => {
            let (tag, value) = terminal::irq_failure(IrqSite::SoftwareEoi, error);
            MsrCompletion::Stop(tag, value)
        }
    }
}

/// AVIC exits (D5/D6). Both are handled as traps: Table 15-22 pp566-567
/// lists the ICRL write and the level-triggered EOI write as "#VMEXIT
/// (trap)", so the write has completed and RIP has advanced. 15.29.9.2 p581
/// calls the EOI exit a fault instead; `irq::level_eoi_exit` stays correct if
/// that WRMSR runs again. nRIP is not used (15.7.1 p509 saves it only for
/// instruction, MSR and IOIO intercepts). This handler never changes RIP or
/// retries a write. WRMSR leaves RCX unchanged, so the
/// frame still names the written MSR.
unsafe fn handle_avic_exit(state: &mut State, vmcb: &mut Vmcb, frame: &GuestRegisters) -> bool {
    let exit = vmcb.exit_snapshot();
    let mismatch = X2AvicStop::ProfileMismatch as u64;
    let Some(profile) = state.avic else {
        return stop(state, exit.code, exit.rip, mismatch, 0);
    };
    if vmcb.validate_native_x2avic(&profile).is_err() {
        return stop(state, exit.code, exit.rip, mismatch, 1);
    }
    match avic_exit_plan(exit) {
        AvicPlan::IncompleteIpi => unsafe {
            handle_incomplete_ipi(state, vmcb, exit, frame.rcx as u32)
        },
        AvicPlan::LevelEoi(vector) => {
            // D6 fallback for an EOI write that was not intercepted (U11:
            // Table 15-22 p566 calls it a trap, 15.29.9.2 p581 a fault).
            // `level_eoi_exit` accepts the virtual ISR bit still set (it must
            // be the highest) or clear, and records this RIP so that a
            // re-executed WRMSR cannot EOI a second vector (`handle_avic_msr`).
            let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
            // SAFETY: armed dispatcher on its own CPU with IF/GIF clear; arm
            // admitted this CPU's enabled x2APIC, and only this runtime uses it.
            let mut host = unsafe { HostX2Apic::new() };
            if let Err(error) =
                irq::level_eoi_exit(vector, exit.rip, backing, &mut state.irq, &mut host)
            {
                let (tag, value) = terminal::irq_failure(IrqSite::LevelEoiExit, error);
                return stop(state, exit.code, exit.rip, tag, value);
            }
            // SAFETY: armed dispatcher; no other MSRPM reference is live.
            sync_eoi_intercept(&state.irq, unsafe { local_msrpm() }, vmcb);
            state.routing_retries = 0;
            true
        }
        AvicPlan::Stop(tag, value) => stop(state, exit.code, exit.rip, tag, value),
    }
}

/// D6: guest EOI writes are intercepted exactly while this CPU's ledger holds
/// a level source or expects a re-executed EOI write
/// (`PhysicalIrqLedger::intercepts_eoi`). APM2 rev3.44 Figure 15-4 p527 does not say whether VMRUN
/// caches the map contents, so a change also clears every VMCB clean bit.
fn sync_eoi_intercept(ledger: &PhysicalIrqLedger, msrpm: &mut Msrpm, vmcb: &mut Vmcb) {
    if msrpm.update_x2apic_eoi_intercept(ledger) {
        vmcb.invalidate_all();
    }
}

/// This CPU's private MSRPM, which only its own VMCB names.
/// # Safety
/// The armed dispatcher (or arm) of this CPU with its guest stopped, holding
/// no other reference to the map.
unsafe fn local_msrpm() -> &'static mut Msrpm {
    unsafe { &mut *ptr::addr_of_mut!(MSRPM) }
}

/// D1 intercepts every other access that Table 15-22 makes a trap or fault
/// before AVIC sees it (15.29.10 p583), so AVIC_NOACCEL is expected only for
/// a level-triggered EOI write. Anything else is a profile mismatch.
fn avic_exit_plan(exit: ExitSnapshot) -> AvicPlan {
    match AvicExit::decode(exit.code, exit.info1, exit.info2) {
        Ok(AvicExit::IncompleteIpi { .. }) => AvicPlan::IncompleteIpi,
        Ok(AvicExit::NoAcceleration {
            offset: apic::EOI,
            write: true,
            eoi_vector: Some(vector),
        }) => AvicPlan::LevelEoi(vector),
        Ok(AvicExit::NoAcceleration { .. }) => {
            let (tag, value) =
                terminal::avic_exit_refusal(X2AvicStop::NoAcceleration, exit.info1, exit.info2);
            AvicPlan::Stop(tag, value)
        }
        Err(_) => {
            let (tag, value) = terminal::avic_exit_refusal(
                X2AvicStop::UndecodableAvicExit,
                exit.info1,
                exit.info2,
            );
            AvicPlan::Stop(tag, value)
        }
    }
}

/// D5 for one AVIC_INCOMPLETE_IPI exit. The ICR write has completed, so a
/// refusal is a stop, never #GP or a retry, and nothing is republished.
/// `msr` is the guest's RCX: the ICR (830h) or SELF IPI (83Fh) just written.
unsafe fn handle_incomplete_ipi(
    state: &mut State,
    vmcb: &mut Vmcb,
    exit: ExitSnapshot,
    msr: u32,
) -> bool {
    let Some(owner) = state.icr.as_mut() else {
        return stop(state, exit.code, exit.rip, X2AvicStop::ProfileMismatch as u64, 0);
    };
    match incomplete_ipi_plan(owner.inventory(), exit, msr) {
        IncompleteIpi::Startup(icr) => {
            let result =
                owner.route_x2avic_startup(icr, unsafe { mailboxes(state.count) }, |_| unsafe {
                    notify_native_startup()
                });
            if result.is_err() {
                // The hardware instruction already completed. Never reenter
                // as an instruction retry; a refusal published nothing. An
                // INIT/SIPI whose destination matches no admitted CPU is
                // ignored by real hardware (16.5 p643), so count it as a drop
                // and resume; every other malformed form still stops.
                if owner.route_failure().map(|failure| failure.predicate)
                    == Some(NativeRoutePredicate::NoMatch)
                {
                    state.ipi_drops = state.ipi_drops.saturating_add(1);
                    debug(b"resident-ipi-nomatch cpu=");
                    hex(unsafe { ASSIGNED_APIC_ID } as u64);
                    debug(b" icr=");
                    hex(exit.info1);
                    debug(b" count=");
                    hex(state.ipi_drops);
                    debug(b"\n");
                } else {
                    let (tag, value) =
                        terminal::startup_route_refusal(owner.route_failure(), exit.info1);
                    return stop(state, exit.code, exit.rip, tag, value);
                }
            }
        }
        IncompleteIpi::Nmi(nmi) => {
            // Guest NMI IPI (Table 16-4 p644 allowed it here). Remote targets
            // get the NMI mailbox command and the private kick; the sender, if
            // its own explicit destination selects it, sets V_NMI directly
            // (15.21.10 p536). NMI queues no destination record.
            let source = owner.inventory().source_slot();
            let targets = nmi.targets();
            let remote = targets & !(1 << source);
            let result =
                owner.route_x2avic_nmi(remote, unsafe { mailboxes(state.count) }, |_| unsafe {
                    notify_native_startup()
                });
            if result.is_err() {
                let (tag, value) =
                    terminal::startup_route_refusal(owner.route_failure(), exit.info1);
                return stop(state, exit.code, exit.rip, tag, value);
            }
            if targets & (1 << source) != 0 {
                match state.avic {
                    Some(profile) => {
                        if vmcb.set_guest_v_nmi_pending(&profile).is_err() {
                            return stop(
                                state,
                                exit.code,
                                exit.rip,
                                X2AvicStop::ProfileMismatch as u64,
                                3,
                            );
                        }
                    }
                    None => {
                        return stop(
                            state,
                            exit.code,
                            exit.rip,
                            X2AvicStop::ProfileMismatch as u64,
                            0,
                        );
                    }
                }
            }
        }
        IncompleteIpi::Fixed(ipi) => {
            let result = owner.inventory().deliver_fixed(
                ipi,
                // SAFETY: dispatcher under this CPU's private root; the
                // inventory resolves only slots below its admitted count,
                // which arm bound to the pool (`state.count`).
                |slot| unsafe { remote_backing(slot) },
                // SAFETY: CPL0 with AVIC admitted at arm (CPUID Fn8000_000A
                // EDX[13]); `DoorbellTarget` bounds the ID, so the WRMSR
                // cannot fault whatever the receiver does. The receiver need
                // not be armed: AP guests run before the BSP arms. DXE
                // published every table entry (valid, backing page, host ID)
                // and every slot's prepared backing page before the first
                // arm, nothing clears V or IsRunning, and each CPU sets its
                // own IsRunning at the end of its arm, so every published
                // target page stays valid. A doorbell to a core in host mode
                // has no defined effect (15.29.8.2 p579 defines guest-mode
                // receipt only); that core evaluates the page's IRR at its
                // next VMRUN (15.29.8.3 p579).
                |target| unsafe { apic::ring_avic_doorbell(target) },
            );
            if let Err(error) = result {
                let (tag, value) = terminal::fan_out_failure(error, exit.info1);
                return stop(state, exit.code, exit.rip, tag, value);
            }
        }
        IncompleteIpi::Dropped(_drop) => {
            state.ipi_drops = state.ipi_drops.saturating_add(1);
            debug(b"resident-ipi-drop cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b" icr=");
            hex(exit.info1);
            debug(b" count=");
            hex(state.ipi_drops);
            debug(b"\n");
        }
        // ID 1: hardware published every valid target (15.29.6.1 step 5
        // p577); a target that is not running evaluates IRR at its first
        // VMRUN (15.29.8.3 p579).
        IncompleteIpi::Published => {}
        IncompleteIpi::Stop(tag, value) => return stop(state, exit.code, exit.rip, tag, value),
    }
    clear_icr_delivery_status(unsafe { &*ptr::addr_of!(AVIC_BACKING) });
    state.routing_retries = 0;
    true
}

/// D5 policy (`Inventory::classify`) applied to EXITINFO1, with one tolerance:
/// ICR bit 12 is ignored. Decision: 16.13 p661 makes the eliminated delivery
/// status must-be-zero for x2APIC ICR writes and 15.29.9.1 p580 calls
/// EXITINFO1 the value written, yet the hardware may leave its busy flag set
/// on an incomplete IPI (informative only: Linux KVM avic.c
/// avic_incomplete_ipi_interception). Every other reserved bit still refuses.
/// A SELF IPI write (`msr` 83Fh) is first made its to-self ICR command
/// (`ipi::written_command`).
fn incomplete_ipi_plan(inventory: &Inventory, exit: ExitSnapshot, msr: u32) -> IncompleteIpi {
    let icr = ipi::written_command(msr, exit.info1) & !apic::ICR_DELIVERY_STATUS;
    match inventory.classify(icr, (exit.info2 >> 32) as u32) {
        Ok(IpiAction::Startup) => IncompleteIpi::Startup(icr),
        Ok(IpiAction::Fixed(ipi)) => IncompleteIpi::Fixed(ipi),
        Ok(IpiAction::Nmi(nmi)) => IncompleteIpi::Nmi(nmi),
        Ok(IpiAction::Dropped(drop)) => IncompleteIpi::Dropped(drop),
        Ok(IpiAction::Published) => IncompleteIpi::Published,
        Err(refusal) => {
            let (tag, value) = terminal::ipi_refusal(refusal, exit.info1, exit.info2);
            IncompleteIpi::Stop(tag, value)
        }
    }
}

/// Drop a delivery-status residue (bit 12) from the backing ICR low word
/// after a handled incomplete IPI, so guest ICR reads stay x2APIC-conformant
/// (16.11.3 p659: reserved bits read as zero). Only this CPU's guest, now
/// stopped, writes its ICR; remote publishers change only IRR and TMR.
fn clear_icr_delivery_status(backing: &BackingPage) {
    let busy = apic::ICR_DELIVERY_STATUS as u32;
    if let Ok(low) = backing.read_register(apic::ICR)
        && low & busy != 0
    {
        let _ = backing.write_register_stopped(apic::ICR, low & !busy);
    }
}

/// Backing page of dense slot `slot` through this CPU's private-root alias
/// (D7): `prepare` maps one RW/NX alias per pool slot, this CPU's included,
/// and DXE walks all of them before arm.
/// # Safety
/// This CPU's private root is loaded (a dispatcher path after
/// `svmvisor_resident_enter`; arm still runs on the caller's root), `slot` is
/// below the armed pool slot count, and the caller uses only the page's
/// atomic operations, as for any shared backing page.
unsafe fn remote_backing(slot: usize) -> &'static BackingPage {
    debug_assert!((slot as u64) < unsafe { POOL.1 } >> 20);
    unsafe {
        &*(backing_alias(ptr::addr_of!(image_start) as u64, slot as u64) as *const BackingPage)
    }
}

/// Private-root address of dense slot `slot`'s backing-page alias (D7).
const fn backing_alias(base: u64, slot: u64) -> u64 {
    base + super::X2AVIC_BACKING_ALIASES_OFFSET + slot * 4096
}

/// Complete the reviewed fixed-MTRR control transaction on the owning CPU.
/// Runtime preparation retains every host object above1MiB. Shared routing
/// lock excludes low-RAM readers and other core-shared SYS_CFG writes.
unsafe fn handle_syscfg(state: &mut State, vmcb: &mut Vmcb, frame: &GuestRegisters) -> bool {
    use crate::svm::native_syscfg::{self, SyscfgInstruction, SyscfgPreparation};
    let exit = vmcb.exit_snapshot();
    let caps = state
        .capabilities
        .filter(|c| c.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_some() {
        None
    } else {
        match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
            Ok(bytes) => Some(bytes),
            Err(r) if r == terminal::FetchReadFailure::MemoryControlBusy as u16 => {
                return retry_routing(state, vmcb);
            }
            Err(r) => return stop(state, exit.code, exit.rip, 0xf001, r as u64),
        }
    };
    let routes = match try_lock_routes(unsafe { mailboxes(state.count) }) {
        Ok(guard) => guard,
        Err(_) => return retry_routing(state, vmcb),
    };
    let evidence = match caps.as_ref() {
        Some(c) => SyscfgInstruction::Hardware(c),
        None => SyscfgInstruction::Bytes(bytes.as_ref().unwrap()),
    };
    let result = native_syscfg::prepare(
        vmcb,
        frame,
        evidence,
        state.startup_owned,
        __cpuid_count(1, 0).eax,
        unsafe { PHYSICAL_BITS },
        || unsafe { read_msr(SYS_CFG) },
    );
    let failure = match result {
        Ok(SyscfgPreparation::GeneralProtectionPrepared) => {
            state.pending_fault = true;
            return true;
        }
        Ok(SyscfgPreparation::Write(prepared)) => {
            let current = prepared.current();
            let requested = prepared.requested();
            let delta = prepared.delta();
            unsafe {
                diagnostic_record(8, false, [exit.rip, current, requested, current, delta, 0], 1);
            }
            if let Some(requested) = prepared.write_value() {
                unsafe {
                    write_msr(SYS_CFG, requested);
                }
                let observed = unsafe { read_msr(SYS_CFG) };
                if observed != requested {
                    let (tag, value) = terminal::syscfg_operands(0x85, true, requested, observed);
                    drop(routes);
                    return stop(state, exit.code, exit.rip, tag, value);
                }
            }
            prepared.commit();
            state.msr = state.msr.saturating_add(1);
            unsafe {
                diagnostic_record(8, false, [exit.rip, current, requested, requested, delta, 0], 2);
            }
            state.routing_retries = 0;
            return true;
        }
        Err(error) => error,
    };
    let (tag, value) = terminal::syscfg_failure(failure, vmcb, bytes);
    drop(routes);
    stop(state, exit.code, exit.rip, tag, value)
}

/// Contention performed no guest/register/queue/hardware commit. Re-enter at
/// the unchanged faulting instruction, bounded to1024 attempts; do not turn a
/// normal concurrent AP mode transition into a falsely completed instruction.
fn retry_routing(state: &mut State, vmcb: &Vmcb) -> bool {
    state.routing_retries = state.routing_retries.saturating_add(1);
    if state.routing_retries <= 1024 {
        return true;
    }
    let exit = vmcb.exit_snapshot();
    stop(state, exit.code, exit.rip, 0xf107, state.routing_retries as u64)
}

/// Re-present a physical NMI the host vector-2 gate swallowed (irq.S set
/// `svmvisor_resident_nmi_pending`) to the guest as a virtual NMI. APM2
/// rev3.44 15.21.10 p536: platform NMIs are re-presented under NMI
/// virtualization, so Windows still receives them. Virtual NMIs coalesce, so
/// several drained physical NMIs become one V_NMI. A failure only happens on a
/// shutdown/non-armed VMCB that is already terminal, so the NMI is dropped.
/// Returns whether the gate had taken an NMI since the previous call.
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
unsafe fn route_physical_nmi_to_guest(state: &mut State, vmcb: &mut Vmcb) -> bool {
    if svmvisor_resident_nmi_pending.swap(0, Ordering::AcqRel) == 0 {
        return false;
    }
    if let Some(profile) = state.avic {
        let _ = vmcb.set_guest_v_nmi_pending(&profile);
    }
    true
}

/// A VMEXIT_NMI leaves the physical NMI pending (APM2 rev3.44 Table 15-13
/// p536) and the host GIF window must take it (Table 15-10 p530), or the next
/// VMRUN exits again at once, without end. Count the consecutive 61h exits
/// whose window did not set the gate's flag; any other exit is guest progress
/// and a drained one is the design working, so both reset the count. Returns
/// whether this CPU must stop.
fn nmi_drain_stalled(misses: &mut u8, code: u64, drained: bool) -> bool {
    if code != 0x61 || drained {
        *misses = 0;
        return false;
    }
    *misses = misses.saturating_add(1);
    *misses >= NMI_DRAIN_MISS_LIMIT
}

/// Service startup commands on this stopped destination only; it never
/// enters a reset-vector guest or calls firmware. Running returns when its
/// queue is empty (`Some(true)` after a guest change, `None` when nothing
/// changed) or after `STARTUP_COMMANDS_PER_EXIT` commands.
///
/// AwaitSipi has no guest to run, so it waits for its SIPI or a terminal
/// request without a bound (a CPU parked in wait-for-SIPI), polling with
/// PAUSE. Every `AWAIT_SIPI_POLLS` polls it opens a GIF window: INIT
/// notifications, NMI and external SMI are held pending while GIF=0 (APM2
/// rev3.44 Table 15-10 p530), and firmware SMM needs its SMIs. A physical NMI
/// taken in any host GIF window, this one included, reaches the returning host
/// vector-2 gate and is re-presented to the guest as V_NMI at its next VMRUN
/// (15.21.10 p536), not stopped. This AwaitSipi guest has no VMRUN until its
/// SIPI, so a physical NMI here waits in `svmvisor_resident_nmi_pending` until
/// the guest starts. A pending command whose cache lease stays busy is retried
/// `STARTUP_LEASE_ATTEMPTS` times, then stops (stage 9).
/// # Safety
/// This CPU's armed dispatcher with its guest stopped and IF/GIF clear.
unsafe fn service_startup(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> Option<bool> {
    let shared = unsafe { mailboxes(state.count) };
    let mut changed = false;
    let (mut polls, mut busy, mut applied) = (0u32, 0u32, 0u32);
    loop {
        if unsafe { terminal_requested(state) } {
            #[cfg(feature = "resident-runtime-test")]
            if state.startup == NativeStartupState::AwaitSipi {
                debug(b"resident-terminal await-sipi-peer=");
                hex(state.slot as u64);
                debug(b"\n");
            }
            return Some(false);
        }
        let Some(command) = shared[state.slot].peek() else {
            if state.startup == NativeStartupState::Running {
                return changed.then_some(true);
            }
            polls = polls.wrapping_add(1);
            if polls % AWAIT_SIPI_POLLS == 0 && unsafe { acknowledge_init() }.is_none() {
                return startup_stage_stop(state, vmcb, StartupStage::InitAcknowledgment);
            }
            core::hint::spin_loop();
            continue;
        };
        if state.startup == NativeStartupState::Running && applied == STARTUP_COMMANDS_PER_EXIT {
            return changed.then_some(true);
        }
        if state.cache_active {
            return startup_stage_stop(state, vmcb, StartupStage::CacheReplay);
        }
        if unsafe { acknowledge_init() }.is_none() {
            return startup_stage_stop(state, vmcb, StartupStage::InitAcknowledgment);
        }
        let Some(profile) = state.avic else {
            return startup_stage_stop(state, vmcb, StartupStage::OwnerMissing);
        };
        if command == NativeStartupCommand::Nmi {
            // A guest NMI IPI (`ipi::NmiIpi`) queued by another vCPU. Set this
            // stopped guest's V_NMI directly (15.21.10 p536); no LAPIC, cache
            // or route-record work. `complete` is a lock-free FIFO removal, so
            // it needs no route lease. A target in AwaitSipi has V_NMI cleared
            // by its INIT reset and the pending NMI is delivered once it starts
            // (16.5 p643: NMI held pending in the INIT state until STARTUP).
            if vmcb.set_guest_v_nmi_pending(&profile).is_err() {
                return startup_stage_stop(state, vmcb, StartupStage::TargetApplication);
            }
            if shared[state.slot].complete(command).is_err() {
                return startup_stage_stop(state, vmcb, StartupStage::MailboxCompletion);
            }
            (busy, applied) = (0, applied + 1);
            changed = true;
            debug(b"resident-guest-nmi cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b"\n");
            continue;
        }
        let signature = __cpuid_count(1, 0).eax;
        let core = state.cache_observation.is_some().then(|| unsafe { cache::core(state) });
        // SAFETY: this function's contract.
        let owners = unsafe { InitOwners::local(signature) };
        let step = startup_step(
            state,
            vmcb,
            frame,
            shared,
            &profile,
            command,
            core,
            owners,
            // SAFETY: called once by a successful INIT commit, after every
            // fallible step, on this stopped guest's CPU.
            || unsafe { svmvisor_resident_reset_guest_debug() },
        );
        match step {
            StartupStep::Applied(effect) => {
                (busy, applied) = (0, applied + 1);
                changed |= effect != NativeStartupEffect::Ignored;
                startup_debug(command, effect);
            }
            StartupStep::Busy if busy < STARTUP_LEASE_ATTEMPTS => {
                busy += 1;
                core::hint::spin_loop();
            }
            StartupStep::Busy => {
                return startup_stage_stop(state, vmcb, StartupStage::WaitExhausted);
            }
            StartupStep::PendingEvent(error) => {
                let exit = vmcb.exit_snapshot();
                let (tag, value) =
                    terminal::startup_pending_failure(error, unsafe { ASSIGNED_APIC_ID });
                stop(state, exit.code, exit.rip, tag, value);
                return Some(false);
            }
            StartupStep::Failed(stage, value) => {
                return startup_value_stop(state, vmcb, stage, value);
            }
        }
    }
}

/// APM2 15.21.8/Table15-12 and15.28: consume held INIT through private #SX,
/// with IF=0 throughout. No physical IRQ is acknowledged and no CR8/APIC state
/// is changed. The window also takes held external SMIs (firmware SMM) and
/// NMIs (Table 15-10 p530); a held physical NMI now reaches the returning host
/// vector-2 gate (irq.S), which sets `svmvisor_resident_nmi_pending` and
/// returns, so the dispatcher re-presents it to the guest as V_NMI
/// (`route_physical_nmi_to_guest`) instead of stopping this CPU (15.21.10
/// p536). Counts may coalesce; only the mailbox owns guest startup commands.
unsafe fn acknowledge_init() -> Option<u64> {
    let before = svmvisor_resident_init_acks.load(Ordering::Acquire);
    for _ in 0..1024 {
        let previous = svmvisor_resident_init_acks.load(Ordering::Acquire);
        unsafe {
            asm!("stgi", "nop", "clgi", options(nostack));
        }
        let current = svmvisor_resident_init_acks.load(Ordering::Acquire);
        if current == previous {
            return Some(current.wrapping_sub(before));
        }
    }
    None
}

fn startup_debug(command: NativeStartupCommand, effect: NativeStartupEffect) {
    let cpu = u64::from(unsafe { ASSIGNED_APIC_ID });
    match (command, effect) {
        (_, NativeStartupEffect::Init) => {
            debug(b"resident-guest-init cpu=");
            hex(cpu);
            debug(b" kick-acks=");
            hex(svmvisor_resident_init_acks.load(Ordering::Acquire));
        }
        (NativeStartupCommand::Sipi(vector), NativeStartupEffect::Started) => {
            debug(b"resident-guest-sipi cpu=");
            hex(cpu);
            debug(b" vector=");
            hex(u64::from(vector));
        }
        _ => {
            debug(b"resident-guest-sipi-ignored cpu=");
            hex(cpu);
        }
    }
    debug(b"\n");
}

/// One peeked startup command on its stopped destination (D9). "Route lease,
/// then core lease" stays the only nesting; this step holds one at a time:
///
/// 1. Lease-free checks: no pending guest event and the armed profile
///    (`validate_x2avic`), and a destination slot that names an admitted CPU.
/// 2. Under this core's cache lease when cache replay is owned (`core`),
///    which must be idle so no sibling starts E0 between check and reset:
///    the INIT commit (`guest_init`) or the SIPI commit. The lease is
///    released before the route lease is requested.
/// 3. Under the route lease, waited for up to `ROUTE_WAIT_ATTEMPTS`: the
///    guest INIT's destination record (D9 step 7, now after step 8), then the
///    mailbox completion (step 9), which must not interleave with a source's
///    FIFO preflight and store.
///
/// A failure once step 2 has begun is terminal and leaves the command
/// queued; stages 2, 4 and 8 then follow an applied command.
#[allow(clippy::too_many_arguments)]
fn startup_step<P: PhysicalX2Apic>(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    mailboxes: &[NativeStartupMailbox],
    profile: &NativeX2AvicProfile,
    command: NativeStartupCommand,
    core: Option<&CacheCore>,
    owners: InitOwners<'_, P>,
    reset_guest_debug: impl FnOnce(),
) -> StartupStep {
    let signature = owners.signature;
    let target = NativeStartupTarget {
        vmcb: &mut *vmcb,
        frame: &mut *frame,
        state: &mut state.startup,
        signature,
    };
    let effect = match target.validate_x2avic(command, profile) {
        Ok(effect) => effect,
        Err(NativeIcrError::PendingState(error)) => return StartupStep::PendingEvent(error),
        Err(_) => return StartupStep::Failed(StartupStage::TargetApplication, None),
    };
    let Ok(mailbox) = validate_destination_slot(mailboxes, state.slot) else {
        return StartupStep::Failed(StartupStage::ModeCommitPreparation, None);
    };
    let lease = match core.map(|core| core.try_lock()) {
        None => None,
        Some(None) => return StartupStep::Busy,
        Some(Some(lease)) if lease.phase != 0 => {
            return StartupStep::Failed(StartupStage::CacheReplay, None);
        }
        Some(lease) => lease,
    };
    if effect == NativeStartupEffect::Init {
        if let Err((stage, value)) =
            guest_init(state, vmcb, frame, profile, owners, reset_guest_debug)
        {
            return StartupStep::Failed(stage, value);
        }
    } else if (NativeStartupTarget { vmcb, frame, state: &mut state.startup, signature })
        .apply_x2avic(command, profile)
        .is_err()
    {
        return StartupStep::Failed(StartupStage::TargetApplication, None);
    }
    drop(lease);
    let Ok(routes) = lock_routes_within(mailboxes, ROUTE_WAIT_ATTEMPTS) else {
        return StartupStep::Failed(StartupStage::RouteTable, None);
    };
    if effect == NativeStartupEffect::Init {
        let Ok(destination) =
            routes.prepare_destination_mode(state.slot, NativeDestinationMode::X2Apic)
        else {
            return StartupStep::Failed(StartupStage::ModeCommitPreparation, None);
        };
        // The destination stays x2APIC; record the guest INIT.
        destination.commit_destination_mode_from(NativeDestinationCause::GuestInit);
    }
    if mailbox.complete(command).is_err() {
        return StartupStep::Failed(StartupStage::MailboxCompletion, None);
    }
    drop(routes);
    StartupStep::Applied(effect)
}

/// D9 guest INIT on its stopped destination CPU, after `validate_x2avic`
/// returned Init, with the cache lease (if owned) held by `startup_step`.
///
/// Preparation is read-only: the backing identity and the physical ISR
/// (`registers::prepare_init`), and the guest APIC and startup-owned EFER
/// owners (the INIT EFER is computed on a copy). The commit then follows D9:
/// LAPIC (physical timer/LVT reset, level-source retirement, EOI
/// acceleration, backing reset, and the guest APIC's held-IRR record), CPU
/// state (V_TPR 0, every clean bit clear), logical EFER, live DR0-3. A
/// failure after the first commit step is terminal: the caller stops and
/// never resumes. The caller then records the INIT at the destination and
/// completes the mailbox command under the route lease. `Err` carries the
/// startup stage and its value (`None`: the AwaitSipi flag).
fn guest_init<P: PhysicalX2Apic>(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    profile: &NativeX2AvicProfile,
    mut owners: InitOwners<'_, P>,
    reset_guest_debug: impl FnOnce(),
) -> Result<(), (StartupStage, Option<u64>)> {
    let lapic =
        |stage: StartupStage, error| (stage, Some(u64::from(terminal::init_error_code(error))));
    registers::prepare_init(owners.backing, &state.irq, &mut owners.physical)
        .map_err(|error| lapic(StartupStage::InitPreparation, error))?;
    let (Some(mut efer), true) = (state.efer, state.guest_apic.is_some()) else {
        return Err((StartupStage::OwnerMissing, None));
    };
    efer.reset_after_init().map_err(|_| (StartupStage::EferReset, None))?;
    // Steps 1-4 (`registers::commit_init`). The reset page is
    // software-disabled with an empty IRR, so the guest APIC holds nothing.
    registers::commit_init(owners.backing, &mut state.irq, &mut owners.physical, owners.msrpm)
        .map_err(|error| lapic(StartupStage::InitLapicCommit, error))?;
    if let Some(guest) = state.guest_apic.as_mut() {
        guest.reset_after_init();
    }
    // Step 5: CPU INIT state (`Vmcb::initialize_ap_after_init`).
    NativeStartupTarget { vmcb, frame, state: &mut state.startup, signature: owners.signature }
        .apply_x2avic(NativeStartupCommand::Init, profile)
        .map_err(|_| (StartupStage::InitCpuCommit, None))?;
    // Step 6: INIT clears the logical EFER (`NativeEfer::reset_after_init`).
    state.efer = Some(efer);
    // Step 8: DR0-3 are live guest state (APM2 Table 14-1 p482).
    reset_guest_debug();
    Ok(())
}

fn startup_stage_stop(state: &mut State, vmcb: &Vmcb, stage: StartupStage) -> Option<bool> {
    startup_value_stop(state, vmcb, stage, None)
}

/// Startup service stop. `value` defaults to 1 for AwaitSipi, 0 for Running.
fn startup_value_stop(
    state: &mut State,
    vmcb: &Vmcb,
    stage: StartupStage,
    value: Option<u64>,
) -> Option<bool> {
    let exit = vmcb.exit_snapshot();
    let value = value.unwrap_or(u64::from(state.startup == NativeStartupState::AwaitSipi));
    let (tag, value) =
        terminal::startup_failure(7, stage as u8, value, unsafe { ASSIGNED_APIC_ID });
    stop(state, exit.code, exit.rip, tag, value);
    Some(false)
}

/// Reset only the stopped target's guest-live breakpoint addresses.
/// APM2 rev3.44 Table14-1 p482,15.5.1/15.7: INIT resets DR0-3, which are
/// not part of the ordinary VMCB state switch. DR6/7 reset in the guest VMCB.
/// # Safety
/// The sole successful target-owned INIT commit (`guest_init`) calls this
/// with IF/GIF clear, after all fallible preparation and its LAPIC and CPU
/// commits, on the same nonmigrating guest CPU. Host breakpoints/GD are
/// disabled after VMEXIT; no external debugger or host DR owner exists.
/// Never call for a private wake, refused INIT or SIPI. Keep this
/// out-of-line symbol for the exact linked debug-write audit.
#[unsafe(no_mangle)]
#[inline(never)]
unsafe extern "C" fn svmvisor_resident_reset_guest_debug() {
    unsafe {
        asm!(
            "xor eax, eax",
            "mov dr0, rax",
            "mov dr1, rax",
            "mov dr2, rax",
            "mov dr3, rax",
            out("rax") _,
            options(nostack),
        );
    }
}

/// Read only one qualified physical page through the final, otherwise absent
/// arena PTE. The alias is supervisor RO/NX and is removed before returning.
/// Physical source pages are admitted RAM, effective WB and outside the monitor.
/// Single CPU, stopped guest and GIF/IF clear make the temporary PTE exclusive.
unsafe fn fetch_instruction(
    vmcb: &Vmcb,
    startup_owned: bool,
    count: usize,
) -> Result<[u8; 2], u16> {
    let mut reader =
        unsafe { GuestReader::new(vmcb, startup_owned, count) }.map_err(|e| e as u16)?;
    let result = if startup_owned {
        super::fetch::startup_instruction(
            vmcb,
            reader.width,
            reader.guest_pat,
            |address, bytes| unsafe { reader.read(address, bytes) },
        )
    } else {
        super::fetch::instruction(vmcb, reader.width, reader.guest_pat, |address, bytes| unsafe {
            reader.read(address, bytes)
        })
    };
    result.map_err(|error| {
        reader.failure.map_or_else(|| terminal::fetch_failure_code(error), |e| e as u16)
    })
}

/// PPR57896 p202/43, APM2 7.9.1: bit19 is thread-private visibility;
/// bit18 is core-shared routing. Caller holds the shared route gate through
/// this sample and the eventual low RAM read. Fixed writes retain the native
/// trusted OS rendezvous contract. All executing monitor storage is >=1MiB.
unsafe fn native_fixed_page_is_wb(index: u32, shift: u8) -> bool {
    use crate::memory::mtrrs::Mtrrs;
    let original = unsafe { read_msr(SYS_CFG) };
    if original & !SYS_CFG_DEFINED != 0
        || original & SYS_CFG_ENCRYPTION != 0
        || original & SYS_CFG_MTRR_FIX_DRAM_EN == 0
    {
        return false;
    }
    let visible = original | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
    if visible != original {
        unsafe {
            write_msr(SYS_CFG, visible);
        }
    }
    let matched = unsafe { read_msr(SYS_CFG) } == visible;
    let byte = if matched { (unsafe { read_msr(index) } >> shift) as u8 } else { 0 };
    if visible != original {
        unsafe {
            write_msr(SYS_CFG, original);
        }
    }
    let restored = unsafe { read_msr(SYS_CFG) } == original;
    matched && restored && Mtrrs::native_fixed_page_is_wb(visible, byte)
}

/// Enumerated architectural MTRRs, captured boundedly on the owning CPU.
/// Shared by diagnostic UC admission and stopped instruction reading.
unsafe fn native_mtrrs(width: u8) -> Option<crate::memory::mtrrs::Mtrrs> {
    crate::memory::mtrrs::Mtrrs::read(width, __cpuid_count(1, 0).eax, |index| unsafe {
        read_msr(index)
    })
    .ok()
}

/// Read-only diagnostic observation of the strictly admitted physical x2APIC.
#[cfg(feature = "resident-runtime-test")]
unsafe fn read_native_apic(offset: u16) -> u64 {
    unsafe { read_msr(apic::msr(offset)) }
}

/// Private notification only after the routing owner has proved every assigned
/// CPU ready and published a target queue. APM2 16.5/Table16-4: shorthand11
/// ignores destination width and excludes the source. R_INIT/#SX consumes every
/// hardware wake; CPUs with empty queues resume unchanged. No guest INIT is
/// inferred from a wake, and no guest interrupt is acknowledged here.
unsafe fn notify_native_startup() {
    unsafe { send_native_notification() };
}

unsafe fn terminal_requested(state: &State) -> bool {
    state.armed
        && terminal_enabled(state)
        && unsafe { terminal_control() }.ready(state.count)
        && unsafe { terminal_control() }.requested()
}

/// Terminal-only: no route guard is held, no resume follows. APM2 Table15-10
/// holds external SMI/NMI/INIT while GIF=0. All CPUs acknowledge only in that
/// state and never reopen GIF afterward, excluding their firmware/config writes.
/// Reset/machine-check or a nonparticipating CPU can lose evidence; no write is
/// permitted for the legacy aggregate without the complete ack mask. The live
/// guarded per-CPU transport exports failure evidence independently of that mask.
/// Iteration caps are not time bounds.
unsafe fn terminal_finish(state: &mut State) {
    if !state.armed
        || !terminal_enabled(state)
        || state.terminal_endpoint.is_some_and(|endpoint| !endpoint.valid())
    {
        return;
    }
    unsafe {
        diagnostics::flush_fault();
    }
    let shared = unsafe { terminal_control() };
    if !shared.ready(state.count) {
        return;
    }
    let winner = state.stopped_valid && shared.claim(state.slot, state.count);
    if !shared.requested() {
        return;
    }
    if !shared.acknowledge(state.slot, state.count) {
        return;
    }
    unsafe {
        record_barrier(shared);
    }
    if !winner {
        return;
    }
    // The terminal request establishes sole spare-bank ownership; export before
    // notification/preflight can fail and without waiting for another CPU.
    unsafe {
        export_stop_context(state);
    }
    // The published ready gate follows every target's armed/guest ACK. The
    // dedicated terminal request is authoritative; no guest INIT is enqueued.
    let base = unsafe { read_msr(apic::APIC_BASE) };
    // x2APIC has no software-polled ICR delivery status to wait for.
    if base != state.host_apic_base
        || base & apic::APIC_BASE_X2APIC != apic::APIC_BASE_X2APIC
        || unsafe { read_msr(VM_CR) } & VM_CR_R_INIT == 0
        || unsafe { mailboxes(state.count) }.iter().any(|m| !m.is_ready())
    {
        shared.finish(2);
        unsafe {
            record_barrier(shared);
        }
        return;
    }
    // Same already admitted INIT-to-#SX wire operation as startup notification,
    // with a separate irreversible terminal publication instead of a queue.
    unsafe { send_native_notification() };
    for _ in 0..20_000_000 {
        if shared.all_acknowledged(state.count) {
            #[cfg(feature = "resident-runtime-test")]
            if state.terminal_endpoint.is_none() {
                debug(b"resident-terminal barrier=complete owner=");
                hex(state.slot as u64);
                debug(b" count=");
                hex(state.count as u64);
                debug(b" card=disabled\n");
                shared.finish(1);
                return;
            }
            let words = terminal::stop_words(
                state.slot,
                state.stopped,
                state.stopped_rip,
                state.stopped_info1,
                state.stopped_info2,
            );
            let result = words.is_some_and(|words| unsafe { diagnostics::export_terminal(words) });
            shared.finish(if result { 1 } else { 4 });
            unsafe {
                record_barrier(shared);
            }
            return;
        }
        core::hint::spin_loop();
    }
    shared.finish(3);
    unsafe {
        record_barrier(shared);
    }
    debug(b"resident-terminal barrier=incomplete\n");
}

/// Same retained shared backing and inventory as arm; called only under the
/// private root. Sole target owns local CPU state, all shared writes are atomic.
unsafe fn mailboxes(count: usize) -> &'static [NativeStartupMailbox] {
    unsafe {
        core::slice::from_raw_parts(
            (ptr::addr_of!(image_start) as u64 + super::STARTUP_PAGE_OFFSET)
                as *const NativeStartupMailbox,
            count,
        )
    }
}

/// Owning native CPU with the selected MSR enumerated/admitted: x2APIC
/// registers require the fixed enabled bus; VM_CR requires admitted AMD SVM.
/// APM2 15.30.1/16.11 and applicable PPR57896 register definitions.
unsafe fn read_msr(index: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr",in("ecx")index,out("eax")low,out("edx")high,
        options(nomem,nostack,preserves_flags));
    }
    low as u64 | ((high as u64) << 32)
}

/// All accesses use the existing pool-excluded shared alias, initialized before
/// any arm call. No guest reference or original DXE pointer survives here.
unsafe fn terminal_control() -> &'static TerminalControl {
    unsafe {
        &*((ptr::addr_of!(image_start) as u64
            + super::STARTUP_PAGE_OFFSET
            + terminal::CONTROL_OFFSET) as *const TerminalControl)
    }
}

fn terminal_enabled(state: &State) -> bool {
    state.startup_owned
        && state.count >= 2
        && (state.terminal_endpoint.is_some() || cfg!(feature = "resident-runtime-test"))
}

unsafe fn record_barrier(shared: &TerminalControl) {
    let mut context = shared.diagnostic_snapshot();
    context[5] = diagnostics::fault_status();
    unsafe {
        diagnostic_record(7, false, context, 0);
    }
}

/// Stable raw stopped state; reading bytes does not change VMCB/GPRs. These are
/// software observations, not a claim that invalid-entry save fields are valid.
unsafe fn export_stop_context(state: &State) {
    let aux = (state.slot as u32) << 8 | (state.count as u32) << 24;
    unsafe {
        diagnostics::export_context(
            0,
            [
                state.stopped_rip,
                state.stopped,
                state.stopped_info1,
                state.stopped_info2,
                state.exits,
                diagnostics::fault_status(),
            ],
            aux,
        );
        let bytes = ptr::addr_of!(VMCB).cast::<u8>();
        let read = |offset: usize| ptr::read_volatile(bytes.add(offset).cast::<u64>());
        diagnostics::export_context(
            1,
            [read(0x78), read(0x80), read(0x88), read(0xa8), read(0xc8), read(0x550)],
            aux | 1,
        );
        let frame = ptr::read_volatile(ptr::addr_of!(FRAME));
        diagnostics::export_context(
            2,
            [read(0x5f8), frame.rcx, frame.rdx, read(0x558), read(0x4d0), read(0x410)],
            aux | 2 | ((ptr::read_volatile(bytes.add(0x4cb)) as u32) << 13),
        );
        let count = EXIT_HISTORY_COUNT;
        for n in 0..count {
            let index = (EXIT_HISTORY_NEXT + 5 - count + n) % 5;
            let context = ptr::addr_of!(EXIT_HISTORY).cast::<[u64; 6]>().add(index).read();
            diagnostics::export_context(3 + n, context, aux | 3 | ((n as u32) << 16));
        }
    }
}

/// Same validated physical bus and already idle ICR as terminal_finish. The
/// all-ready target set has R_INIT/#SX installed. IF/GIF stay zero on sender.
unsafe fn send_native_notification() {
    unsafe {
        asm!("mfence", options(nostack, preserves_flags));
        write_msr(apic::ICR_MSR, 0x000c_0500);
    }
}

/// Same owning CPU; an admitted non-APIC MSR value checked by its owner
/// (VM_CR.R_INIT preserving every other bit, SYS_CFG, HWCR, cache replay, a
/// guest MCAX write that `native_mcax::plan` admitted) or
/// the fixed private INIT notification ICR. Guest x2APIC state reaches the
/// physical LAPIC only through `HostX2Apic`. APM2 rev3.44 15.30.1/16.13 and
/// PPR57896 p215. No MSR may fault.
unsafe fn write_msr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32,
            in("edx") (value >> 32) as u32, options(nostack, preserves_flags));
    }
}

fn check_exit_event(state: &mut State, vmcb: &mut Vmcb) -> bool {
    let code = vmcb.exit_snapshot().code;
    // APM2 15.14.3: shutdown leaves saved guest state undefined. Invalid entry
    // likewise cannot establish event delivery. Neither authorizes a retry.
    if matches!(code, 0x7f | u64::MAX) {
        return stop(state, code, 0, 0xf110, 0);
    }
    let interrupted = u64::from_le_bytes(vmcb.bytes()[0x088..0x090].try_into().unwrap());
    if state.pending_fault {
        if vmcb.clear_event_injection_after_exit().is_err() {
            let exit = vmcb.exit_snapshot();
            return stop(state, exit.code, exit.rip, 0xf10c, interrupted);
        }
        state.pending_fault = false;
    }
    // APM2 rev3.44 15.7.2-3 p509-511 / 15.20 p531: EXITINTINFO.V means the
    // guest was delivering an event through the IDT when this intercept fired
    // and delivery did not finish. A bare NPF/INTR/NMI/AVIC retry would lose
    // an acknowledged interrupt, so complete delivery by re-injecting the
    // recorded event through EVENTINJ (`Vmcb::reinject_interrupted_delivery`).
    // This runs before the exit's own handler; the exits that carry
    // EXITINTINFO.V here (0x60 INTR, 0x61 NMI, 0x401/0x402 AVIC, an interrupted
    // NPF) do not themselves write EVENTINJ, so re-injection is not clobbered.
    // A physical INIT does not reset this guest (it is redirected to #SX), so
    // no interrupted event is discarded here. TYPE 4 software interrupts and
    // reserved types stay terminal (15.20 p531-532 needs nRIP emulation this
    // path does not implement); a conflicting queued event stays terminal too.
    if interrupted & (1 << 31) != 0 {
        let exit = vmcb.exit_snapshot();
        return match vmcb.reinject_interrupted_delivery() {
            ReinjectOutcome::Reinjected { .. } | ReinjectOutcome::NoEvent => true,
            ReinjectOutcome::Unsupported { interrupted } => {
                stop(state, exit.code, exit.rip, 0xf10f, interrupted)
            }
            ReinjectOutcome::Conflict => stop(state, exit.code, exit.rip, 0xf112, interrupted),
        };
    }
    true
}

fn record_unexplained_stop(state: &mut State, resume: bool, exit: crate::svm::exit::ExitSnapshot) {
    if !resume && !state.stopped_valid {
        stop(state, exit.code, exit.rip, 0xf10e, exit.info1);
    }
}

fn stop(state: &mut State, code: u64, rip: u64, info1: u64, info2: u64) -> bool {
    // Retain the exact terminal reason even in production, where the diagnostic
    // port is absent. The dedicated stopped owner never resumes after this.
    unsafe {
        ptr::write_volatile(&mut state.stopped_rip, rip);
        ptr::write_volatile(&mut state.stopped_info1, info1);
        ptr::write_volatile(&mut state.stopped_info2, info2);
        ptr::write_volatile(&mut state.stopped, code);
        ptr::write_volatile(&mut state.stopped_valid, true);
        if code == 0x7c {
            let raw = ptr::read_volatile(ptr::addr_of!(svmvisor_resident_raw_vmexit));
            if let Some((event, context)) =
                raw.stop_record(ptr::addr_of!(VMCB) as u64, ASSIGNED_APIC_ID, info1, info2)
            {
                // Win the sticky first-fault bank with the immutable boundary;
                // keep event3 as the ordinary latest stopped-state record.
                diagnostic_record(event, true, context, info1 as u32);
            }
        }
        diagnostic_record(3, true, [rip, code, info1, info2, state.exits, stop_counters(state)], 0);
    }
    debug(b"resident-stop code=");
    hex(code);
    debug(b" rip=");
    hex(rip);
    debug(b" info1=");
    hex(info1);
    debug(b" info2=");
    hex(info2);
    debug(b"\n");
    false
}

/// GIF/IF clear, initialized per-CPU runtime; independent of mutable STATE.
unsafe fn diagnostic_record(event: u8, fault: bool, context: [u64; 6], aux: u32) {
    unsafe {
        diagnostics::record(event, fault, context, aux);
    }
}

/// Last context word of a stop record: incomplete-IPI drops in bits 31:0 and
/// software-disabled edge discards in bits 63:32, each saturated.
fn stop_counters(state: &State) -> u64 {
    state.ipi_drops.min(u64::from(u32::MAX)) | (state.irq_discards.min(u64::from(u32::MAX)) << 32)
}

fn hex(value: u64) {
    for shift in (0..16).rev() {
        debug(&[b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]]);
    }
}

fn debug(bytes: &[u8]) {
    #[cfg(feature = "resident-runtime-test")]
    for byte in bytes {
        unsafe {
            asm!("out dx,al",in("dx")0xe9u16,in("al")*byte,options(nomem,nostack));
        }
    }
    #[cfg(not(feature = "resident-runtime-test"))]
    let _ = bytes;
}

#[cfg(test)]
mod raw_capture_tests {
    use crate::host::resident::runtime::RawVmexitCapture;

    #[test]
    fn raw_boundary_requires_same_generation_vmcb_and_physical_cpu() {
        let mut raw: RawVmexitCapture = unsafe { core::mem::zeroed() };
        raw.entry_sequence = 19;
        raw.exit_sequence = 19;
        raw.entry_vmcb_pa = 0x123000;
        raw.exit_vmcb_pa = 0x123000;
        raw.context_vmcb_pa = 0x123000;
        raw.physical_apic_id = 21;
        raw.code = 0x7c;
        raw.guest_rcx = 0xc001_0010;
        raw.physical_cache_valid = 1;
        raw.exit_rip = 0xffff800000001111;
        raw.nrip = 0xffff800000002222;
        raw.entry_rip = 0xffff800000003333;
        raw.guest_cr0 = 0xe0000011;
        raw.mtrr_def_type = 0xc06;
        raw.host_cr0 = 0x80010011;
        assert_eq!(
            raw.stop_record(0x123000, 21, 0xf400, 16),
            Some((
                10,
                [
                    0xffff800000001111,
                    0xffff800000002222,
                    0xffff800000003333,
                    0xe0000011,
                    0xc06,
                    0x80010011
                ]
            ))
        );
        for failure in 0..6 {
            let mut bad = raw;
            match failure {
                0 => bad.entry_sequence = 0,
                1 => bad.exit_sequence = 18,
                2 => bad.entry_vmcb_pa += 4096,
                3 => bad.exit_vmcb_pa += 4096,
                4 => bad.context_vmcb_pa += 4096,
                _ => bad.physical_apic_id = 13,
            }
            let record = bad.stop_record(0x123000, 21, 0xf400, 16).unwrap();
            assert_eq!(record.0, 11);
            assert_eq!(
                record.1,
                [
                    bad.exit_vmcb_pa,
                    0x123000,
                    (bad.physical_apic_id << 32) | 21,
                    raw.exit_rip,
                    raw.nrip,
                    raw.entry_rip
                ]
            );
        }
        raw.physical_cache_valid = 0;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
        raw.physical_cache_valid = 1;
        raw.guest_rcx = 0xc0000080;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
    }

    #[test]
    fn raw_cache_operands_require_provenance_but_no_syscfg_physical_sample() {
        let mut raw: RawVmexitCapture = unsafe { core::mem::zeroed() };
        raw.entry_sequence = 29;
        raw.exit_sequence = 29;
        raw.entry_vmcb_pa = 0x123000;
        raw.exit_vmcb_pa = 0x123000;
        raw.context_vmcb_pa = 0x123000;
        raw.physical_apic_id = 21;
        raw.code = 0x7c;
        raw.exit_rip = 0xffff800000001111;
        raw.nrip = raw.exit_rip + 2;
        raw.guest_rax = 0xfeedface76543210;
        raw.guest_rdx = 0xdeadc0defedcba98;
        for index in crate::svm::native_cache::owned_msrs().filter(|&index| index != 0xc001_0010) {
            raw.guest_rcx = 0x1234567800000000 | u64::from(index);
            assert_eq!(
                raw.stop_record(0x123000, 21, 0x12345678_f400, 0xfedcba98_00000010),
                Some((
                    13,
                    [
                        raw.exit_rip,
                        raw.guest_rcx,
                        0xfedcba9876543210,
                        raw.nrip,
                        0x12345678_f400,
                        0xfedcba98_00000010
                    ]
                ))
            );
        }
        for failure in 0..6 {
            let mut bad = raw;
            match failure {
                0 => bad.entry_sequence = 0,
                1 => bad.exit_sequence -= 1,
                2 => bad.entry_vmcb_pa += 4096,
                3 => bad.exit_vmcb_pa += 4096,
                4 => bad.context_vmcb_pa += 4096,
                _ => bad.physical_apic_id += 1,
            }
            assert_eq!(bad.stop_record(0x123000, 21, 0xf400, 16).unwrap().0, 11);
        }
        raw.guest_rcx = 0xc000_00e9;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
        raw.guest_rcx = 0xc001_0015;
        raw.code = 0x72;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
    }
}
#[cfg(all(test, not(feature = "resident-runtime-test")))]
mod terminal_return_tests {
    use super::*;

    #[test]
    fn interrupted_fault_refusal_preserves_request_and_records_terminal_reason() {
        for (code, interrupted) in [(0x400, 0x8000_0b0d_u64), (u64::MAX, 0), (0x7f, 0)] {
            let mut vmcb = Vmcb::new();
            for (offset, value) in [(0x70, code), (0x88, interrupted), (0xa8, 0x8000_0b0d)] {
                unsafe {
                    ptr::copy_nonoverlapping(
                        value.to_le_bytes().as_ptr(),
                        (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                        8,
                    );
                }
            }
            let before = *vmcb.bytes();
            let mut state = INITIAL_STATE;
            state.pending_fault = true;
            assert!(!check_exit_event(&mut state, &mut vmcb));
            assert!(state.pending_fault && state.stopped_valid);
            assert_eq!(
                (state.stopped, state.stopped_info1, state.stopped_info2),
                (code, if code == 0x400 { 0xf10c } else { 0xf110 }, interrupted)
            );
            assert_eq!(*vmcb.bytes(), before);
        }
        let mut vmcb = Vmcb::new();
        let mut state = INITIAL_STATE;
        state.pending_fault = true;
        assert!(check_exit_event(&mut state, &mut vmcb));
        assert!(!state.pending_fault && !state.stopped_valid);
    }

    #[test]
    fn interrupted_delivery_is_reinjected_through_eventinj_before_resume() {
        // An interrupted external interrupt (TYPE 0, vector 51h) on an INTR or
        // AVIC exit completes by re-injection, not by an unchanged retry.
        for code in [0x400_u64, 0x60, 0x61, 0x401] {
            let mut vmcb = Vmcb::new();
            let interrupted = 0x8000_0051_u64;
            for (offset, value) in
                [(0x70, code), (0x88, interrupted), (0x578, 0x1234), (0xc0, u64::from(u32::MAX))]
            {
                unsafe {
                    ptr::copy_nonoverlapping(
                        value.to_le_bytes().as_ptr(),
                        (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                        8,
                    );
                }
            }
            let mut state = INITIAL_STATE;
            assert!(check_exit_event(&mut state, &mut vmcb));
            assert!(!state.pending_fault && !state.stopped_valid);
            // EXITINTINFO is retained; EVENTINJ now carries the same event and
            // the clean bits are cleared.
            assert_eq!(vmcb.event_injection(), interrupted);
            assert_eq!(
                u64::from_le_bytes(vmcb.bytes()[0x88..0x90].try_into().unwrap()),
                interrupted
            );
            assert_eq!(vmcb.bytes()[0xc0..0xc4], [0; 4]);
        }
    }

    #[test]
    fn software_interrupt_and_conflicting_delivery_stay_terminal() {
        // TYPE 4 (INTn) is not re-injectable and stays a terminal stop.
        let mut vmcb = Vmcb::new();
        for (offset, value) in [(0x70, 0x400u64), (0x88, 0x8000_0451), (0x578, 0x1234)] {
            unsafe {
                ptr::copy_nonoverlapping(
                    value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                    8,
                );
            }
        }
        let before = *vmcb.bytes();
        let mut state = INITIAL_STATE;
        assert!(!check_exit_event(&mut state, &mut vmcb));
        assert_eq!(
            (state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
            (0x400, 0x1234, 0xf10f, 0x8000_0451)
        );
        assert_eq!(*vmcb.bytes(), before);
        // A different pending EVENTINJ conflicts with re-injection: terminal.
        let mut vmcb = Vmcb::new();
        for (offset, value) in [
            (0x70, 0x400u64),
            (0x88, 0x8000_0051),
            (0xa8, (1 << 31) | (3 << 8) | 13),
            (0x578, 0x1234),
        ] {
            unsafe {
                ptr::copy_nonoverlapping(
                    value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                    8,
                );
            }
        }
        let before = *vmcb.bytes();
        let mut state = INITIAL_STATE;
        assert!(!check_exit_event(&mut state, &mut vmcb));
        assert_eq!(
            (state.stopped, state.stopped_info1, state.stopped_info2),
            (0x400, 0xf112, 0x8000_0051)
        );
        assert_eq!(*vmcb.bytes(), before);
    }

    #[test]
    fn shutdown_and_invalid_entry_do_not_interpret_poisoned_saved_event_or_rip() {
        for code in [0x7f_u64, u64::MAX] {
            let mut vmcb = Vmcb::new();
            for (offset, value) in [(0x70, code), (0x88, u64::MAX), (0x578, u64::MAX)] {
                unsafe {
                    ptr::copy_nonoverlapping(
                        value.to_le_bytes().as_ptr(),
                        (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                        8,
                    );
                }
            }
            let before = *vmcb.bytes();
            let mut state = INITIAL_STATE;
            assert!(!check_exit_event(&mut state, &mut vmcb));
            assert_eq!(
                (state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
                (code, 0, 0xf110, 0)
            );
            assert!(state.stopped_valid);
            assert_eq!(*vmcb.bytes(), before);
        }
    }

    #[test]
    fn unexplained_refusal_records_exit_without_changing_guest_or_existing_reason() {
        let vmcb = Vmcb::new();
        let before = *vmcb.bytes();
        let exit = vmcb.exit_snapshot();
        let mut state = INITIAL_STATE;
        record_unexplained_stop(&mut state, true, exit);
        assert!(!state.stopped_valid);
        record_unexplained_stop(&mut state, false, exit);
        assert!(state.stopped_valid);
        assert_eq!(
            (state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
            (exit.code, exit.rip, 0xf10e, exit.info1)
        );
        state.stopped_info1 = 0xf400;
        state.stopped_info2 = 16;
        record_unexplained_stop(&mut state, false, exit);
        assert_eq!((state.stopped_info1, state.stopped_info2), (0xf400, 16));
        assert_eq!(*vmcb.bytes(), before);
    }
}
/// Host models of the x2AVIC exit glue. Hardware effects go through the
/// owners' `PhysicalX2Apic` seam; nothing here reaches `stop` or `debug`.
#[cfg(test)]
mod x2avic_glue_tests {
    extern crate std;

    use super::*;
    use crate::{
        memory::address::{AddressPolicy, EncryptionState},
        svm::{
            exit::MsrInstruction,
            native_cache::{CacheCoreState, CacheObservation},
            x2avic::{
                ipi::IpiRefusal,
                irq::IrqError,
                registers::{InitError, Refusal},
            },
        },
    };
    use core::{
        cell::Cell,
        sync::atomic::{AtomicBool, Ordering},
    };

    fn exit(code: u64, info1: u64, info2: u64) -> ExitSnapshot {
        ExitSnapshot { code, info1, info2, rip: 0x1000, nrip: 0 }
    }

    fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
        assert!(offset + 8 <= 4096);
        // SAFETY: in-bounds write into the 4 KiB VMCB byte image.
        unsafe {
            ptr::write_unaligned((vmcb as *mut Vmcb).cast::<u8>().add(offset).cast::<u64>(), value)
        };
    }

    fn get(vmcb: &Vmcb, offset: usize) -> u64 {
        u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
    }

    fn policy() -> AddressPolicy {
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
    }

    fn eoi_intercepted(msrpm: &Msrpm) -> bool {
        let bit = 2 * 0x80b + 1;
        msrpm.bytes()[bit / 8] & (1 << (bit % 8)) != 0
    }

    #[test]
    fn undrained_nmi_exits_stop_only_when_consecutive() {
        let mut misses = 0;
        // One miss, or two, never stops; a drained 61h exit resets the count.
        assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
        assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
        assert_eq!(misses, 2);
        assert!(!nmi_drain_stalled(&mut misses, 0x61, true));
        assert_eq!(misses, 0);
        // Any other exit is guest progress and resets it, drained or not.
        for (code, drained) in [(0x60, false), (0x72, true), (0x400, false)] {
            assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
            assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
            assert!(!nmi_drain_stalled(&mut misses, code, drained));
            assert_eq!(misses, 0);
        }
        // The third consecutive miss stops, and the count then saturates.
        for expected in [false, false, true, true] {
            assert_eq!(nmi_drain_stalled(&mut misses, 0x61, false), expected);
        }
        assert_eq!(misses, 4);
        misses = u8::MAX;
        assert!(nmi_drain_stalled(&mut misses, 0x61, false));
        assert_eq!(misses, u8::MAX);
        assert_eq!(NMI_DRAIN_MISS_LIMIT, 3);
        // A unique tag that exports as an unhandled exit 61h with its RIP.
        assert_eq!(NMI_DRAIN_STALL, 0xf113);
        let words =
            terminal::stop_words(5, 0x61, 0xffff_f800_0000_2000, NMI_DRAIN_STALL, 3).unwrap();
        assert_eq!(
            (
                (words[0] >> 24) & 15,
                (words[0] >> 13) & 0x7ff,
                (words[0] >> 8) & 31,
                words[1],
                words[2]
            ),
            (0, 0x61, 5, 0x2000, 0xffff_f800)
        );
    }

    #[test]
    fn register_outcomes_complete_fault_or_stop_with_typed_evidence() {
        assert_eq!(
            msr_completion(Emulation::Read(0x1234_5678_9abc_def0), 0x809, false),
            MsrCompletion::Complete { read: Some(0x1234_5678_9abc_def0) }
        );
        assert_eq!(
            msr_completion(Emulation::Written, 0x80b, true),
            MsrCompletion::Complete { read: None }
        );
        assert_eq!(msr_completion(Emulation::GeneralProtection, 0x802, true), MsrCompletion::Fault);
        // APIC_BASE disable: reason 5, MSR 1Bh, WRMSR, the requested value.
        let refused = Emulation::Refused { reason: Refusal::ApicDisable, value: 0xfee0_0000 };
        assert_eq!(
            msr_completion(refused, 0x1b, true),
            MsrCompletion::Stop(0xf545 | (0x1b << 16) | (1 << 48), 0xfee0_0000)
        );
        let refused = Emulation::Refused { reason: Refusal::UnownedAccess, value: 0 };
        assert_eq!(
            msr_completion(refused, 0x830, false),
            MsrCompletion::Stop(0xf541 | (0x830 << 16), 0)
        );
        let refused = Emulation::Refused { reason: Refusal::ExceptionVector, value: 0x11 };
        assert_eq!(
            msr_completion(refused, 0x832, true),
            MsrCompletion::Stop(0xf547 | (0x832 << 16) | (1 << 48), 0x11)
        );
        let failed =
            Emulation::EoiFailed(IrqError::PhysicalIsrMismatch { vector: 0x40, highest: None });
        assert_eq!(
            msr_completion(failed, 0x80b, true),
            MsrCompletion::Stop(0xf572 | (1 << 16), (2 << 17) | (0x100 << 8) | 0x40)
        );
    }

    #[test]
    fn msr_completions_continue_fault_or_stop_the_stopped_guest() {
        let mut f = Fixture::new();
        let armed = |vmcb: &mut Vmcb| {
            put(vmcb, 0x578, 0x1000); // RIP
            put(vmcb, 0x5f8, 0xdead_beef_0000_0001); // RAX
            put(vmcb, 0x570, (1 << 16) | 2); // RFLAGS with RF
            put(vmcb, 0x068, 1); // interrupt shadow
            put(vmcb, 0xc0, u64::from(u32::MAX)); // clean bits
        };
        armed(&mut f.vmcb);
        let rdmsr = exit(0x7c, 0, 0);
        let next = MsrInstruction::Bytes(&[0x0f, 0x32]).continuation(rdmsr).unwrap();
        // RDMSR: EDX:EAX loaded with zero-extended halves, then nRIP.
        let read = MsrCompletion::Complete { read: Some(0x1234_5678_9abc_def0) };
        assert_eq!(
            apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, read, next),
            Ok(false)
        );
        assert_eq!((f.vmcb.guest_rax(), f.frame.rdx, f.frame.rcx), (0x9abc_def0, 0x1234_5678, 7));
        assert_eq!(
            (f.vmcb.guest_rip(), get(&f.vmcb, 0x570), get(&f.vmcb, 0x068) & 1),
            (0x1002, 2, 0)
        );
        assert_eq!(f.vmcb.bytes()[0xc0..0xc4], [0; 4]);
        // WRMSR: RAX and RDX keep the written value.
        armed(&mut f.vmcb);
        let wrmsr = exit(0x7c, 1, 0);
        let next = MsrInstruction::Bytes(&[0x0f, 0x30]).continuation(wrmsr).unwrap();
        let written = MsrCompletion::Complete { read: None };
        assert_eq!(
            apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, written, next),
            Ok(false)
        );
        assert_eq!((f.vmcb.guest_rax(), f.frame.rdx), (0xdead_beef_0000_0001, 0x1234_5678));
        assert_eq!(f.vmcb.guest_rip(), 0x1002);
        // #GP(0) at the unchanged RIP.
        armed(&mut f.vmcb);
        assert_eq!(
            apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, MsrCompletion::Fault, next),
            Ok(true)
        );
        assert_eq!(
            (get(&f.vmcb, 0xa8), f.vmcb.guest_rip(), get(&f.vmcb, 0x570)),
            (0x8000_0b0d, 0x1000, 0x1_0002)
        );
        // A fault that cannot be queued, and a stop, change nothing.
        let (vmcb, frame) = (*f.vmcb.bytes(), f.frame);
        assert_eq!(
            apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, MsrCompletion::Fault, next),
            Err((0xf510, 4))
        );
        let stop = MsrCompletion::Stop(0xf541 | (0x830 << 16), 0);
        assert_eq!(
            apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, stop, next),
            Err((0xf541 | (0x830 << 16), 0))
        );
        assert_eq!((*f.vmcb.bytes(), f.frame), (vmcb, frame));
    }

    #[test]
    fn incomplete_ipi_tolerates_only_the_delivery_status_bit() {
        let owner = NativeIcr::admit(0x10, &[0x10, 0x11, 0x12]).unwrap();
        let inventory = owner.inventory();
        let busy = apic::ICR_DELIVERY_STATUS;
        let edge = 0x0000_0012_0000_00ef;
        for reason in [0u64, 2] {
            for icr in [edge, edge | busy] {
                let plan =
                    incomplete_ipi_plan(inventory, exit(0x401, icr, reason << 32), apic::ICR_MSR);
                assert!(
                    matches!(plan, IncompleteIpi::Fixed(ipi)
                    if ipi.vector() == 0xef && ipi.targets() == 1 << 2),
                    "{icr:#x} {reason}"
                );
            }
        }
        // INIT/SIPI are routed with bit 12 clear, for IDs 0, 2 and 4.
        let init = 0x0000_0011_0000_0500;
        for reason in [0u64, 2, 4] {
            assert_eq!(
                incomplete_ipi_plan(
                    inventory,
                    exit(0x401, init | busy, reason << 32),
                    apic::ICR_MSR
                ),
                IncompleteIpi::Startup(init)
            );
        }
        // Every other reserved bit still refuses; the raw EXITINFO1 survives.
        for bit in [13, 16, 17, 20, 31] {
            let icr = edge | busy | (1 << bit);
            assert_eq!(
                incomplete_ipi_plan(inventory, exit(0x401, icr, 0), apic::ICR_MSR),
                IncompleteIpi::Stop(0xf550 | IpiRefusal::ReservedBits as u64, icr)
            );
        }
        // ID 1: hardware published the fixed edge IPI; resume, nothing to do.
        for icr in [edge, edge | busy] {
            assert_eq!(
                incomplete_ipi_plan(
                    inventory,
                    exit(0x401, icr, (1 << 32) | 0xfff_f012),
                    apic::ICR_MSR
                ),
                IncompleteIpi::Published
            );
        }
        // An ID 1 that hardware cannot have published keeps its ID and table
        // index as detail.
        assert_eq!(
            incomplete_ipi_plan(
                inventory,
                exit(0x401, init, (1 << 32) | 0xfff_f012),
                apic::ICR_MSR
            ),
            IncompleteIpi::Stop(0xf551 | (0x1012 << 16), init)
        );
        assert_eq!(
            incomplete_ipi_plan(
                inventory,
                exit(0x401, 0x0000_0012_0000_0005, 4 << 32),
                apic::ICR_MSR
            ),
            IncompleteIpi::Dropped(IpiDrop::IllegalVector)
        );
        assert_eq!(
            incomplete_ipi_plan(
                inventory,
                exit(0x401, 0x0000_0020_0000_00ef | busy, 2 << 32),
                apic::ICR_MSR
            ),
            IncompleteIpi::Dropped(IpiDrop::NoTarget)
        );
    }

    #[test]
    fn self_ipi_exits_never_name_another_cpu() {
        // Source 12h is slot 2 and slot 0 is ID 0. EXITINFO1 of a SELF IPI
        // write may be the bare vector or the to-self ICR (16.15 p663).
        let owner = NativeIcr::admit(0x12, &[0, 0x11, 0x12]).unwrap();
        let inventory = owner.inventory();
        for info1 in [0xefu64, 0x0004_00ef, 0x0000_0011_0004_00ef] {
            for reason in [0u64, 2] {
                let plan = incomplete_ipi_plan(
                    inventory,
                    exit(0x401, info1, reason << 32),
                    apic::SELF_IPI_MSR,
                );
                assert!(
                    matches!(plan, IncompleteIpi::Fixed(ipi)
                    if ipi.vector() == 0xef && ipi.targets() == 1 << 2),
                    "{info1:#x} {reason}"
                );
            }
        }
        assert_eq!(
            incomplete_ipi_plan(inventory, exit(0x401, 0x05, 4 << 32), apic::SELF_IPI_MSR),
            IncompleteIpi::Dropped(IpiDrop::IllegalVector)
        );
        // The same bare value written to the ICR is a physical IPI to ID 0.
        let plan = incomplete_ipi_plan(inventory, exit(0x401, 0xef, 0), apic::ICR_MSR);
        assert!(matches!(plan, IncompleteIpi::Fixed(ipi) if ipi.targets() == 1));
    }

    #[test]
    fn handled_incomplete_ipi_clears_only_the_backing_delivery_status() {
        let page = BackingPage::new();
        page.write_register_stopped(apic::ICR, 0x000c_14ef).unwrap();
        page.write_register_stopped(apic::ICR_HIGH, 0x12).unwrap();
        clear_icr_delivery_status(&page);
        assert_eq!(page.read_register(apic::ICR), Ok(0x000c_04ef));
        assert_eq!(page.read_register(apic::ICR_HIGH), Ok(0x12));
        clear_icr_delivery_status(&page);
        assert_eq!(page.read_register(apic::ICR), Ok(0x000c_04ef));
    }

    #[test]
    fn avic_exits_outside_the_level_eoi_fallback_stop_with_raw_exit_information() {
        let eoi = (1u64 << 32) | 0xb0;
        assert_eq!(avic_exit_plan(exit(0x402, eoi, 0x61)), AvicPlan::LevelEoi(0x61));
        assert_eq!(avic_exit_plan(exit(0x401, 0x4ef, 2 << 32)), AvicPlan::IncompleteIpi);
        // Timer LVT write, APR read, EOI read, divide write: D1 intercepts all.
        for (info1, info2) in [
            ((1u64 << 32) | 0x320, 0xdead_beef_0000_0001),
            (0x90, 0),
            (0xb0, 0x61),
            ((1 << 32) | 0x3e0, 7),
        ] {
            assert_eq!(
                avic_exit_plan(exit(0x402, info1, info2)),
                AvicPlan::Stop(0xf580 | ((info2 & 0xffff_ffff) << 16), info1)
            );
        }
        // An EOI vector below 16 or an ID above 4 does not decode.
        assert_eq!(
            avic_exit_plan(exit(0x402, eoi, 0x0f)),
            AvicPlan::Stop(0xf581 | (0x0f << 16), eoi)
        );
        assert_eq!(avic_exit_plan(exit(0x401, 0x4ef, 5 << 32)), AvicPlan::Stop(0xf581, 0x4ef));
    }

    #[test]
    fn traps_are_handled_before_the_startup_service_and_instructions_after() {
        for code in [0x60, 0x401, 0x402] {
            assert_eq!(exit_order(code), ExitOrder::TrapThenStartup, "{code:#x}");
        }
        for code in [0x63, 0x72, 0x77, 0x7b, 0x7c, 0x81, 0x400, u64::MAX] {
            assert_eq!(exit_order(code), ExitOrder::StartupThenExit, "{code:#x}");
        }
        type Log = ([u8; 2], usize);
        let run = |order: ExitOrder, handled: bool, serviced: Option<bool>| {
            let mut log: Log = ([0; 2], 0);
            let resume = sequence_exit(
                &mut log,
                order,
                |log| {
                    log.0[log.1] = b'h';
                    log.1 += 1;
                    handled
                },
                |log| {
                    log.0[log.1] = b's';
                    log.1 += 1;
                    serviced
                },
            );
            (log, resume)
        };
        use ExitOrder::{StartupThenExit as Instruction, TrapThenStartup as Trap};
        // A trap's effect is complete before any INIT/SIPI; a stopped trap
        // services nothing.
        assert_eq!(run(Trap, true, None), ((*b"hs", 2), true));
        assert_eq!(run(Trap, true, Some(true)), ((*b"hs", 2), true));
        assert_eq!(run(Trap, true, Some(false)), ((*b"hs", 2), false));
        assert_eq!(run(Trap, false, Some(true)), ((*b"h\0", 1), false));
        // An instruction runs only if no startup command changed the guest.
        assert_eq!(run(Instruction, true, None), ((*b"sh", 2), true));
        assert_eq!(run(Instruction, false, None), ((*b"sh", 2), false));
        assert_eq!(run(Instruction, false, Some(true)), ((*b"s\0", 1), true));
        assert_eq!(run(Instruction, true, Some(false)), ((*b"s\0", 1), false));
    }

    #[test]
    fn window_gates_cover_16_to_255_except_mc_and_sx() {
        assert_eq!(IRQ_GATES, 256 - 16);
        for vector in 0..=256 {
            assert_eq!(
                window_gate(vector),
                (16..256).contains(&vector) && vector != 18 && vector != 30,
                "{vector}"
            );
        }
    }

    #[test]
    fn stop_records_pack_saturated_drop_and_discard_counts() {
        let mut state = INITIAL_STATE;
        assert_eq!(stop_counters(&state), 0);
        (state.ipi_drops, state.irq_discards) = (7, 3);
        assert_eq!(stop_counters(&state), (3 << 32) | 7);
        (state.ipi_drops, state.irq_discards) = (u64::MAX, 1 << 40);
        assert_eq!(stop_counters(&state), u64::MAX);
    }

    #[test]
    fn backing_aliases_map_every_slot_page_below_the_shared_table() {
        use crate::host::resident::{X2AVIC_BACKING_ALIASES_OFFSET, X2AVIC_TABLE_OFFSET};
        // Slot 1 of a 2 MiB-aligned pool; the backing page sits 34000h into
        // every image.
        let (pool, base, offset) = (0x2000_0000u64, 0x2010_0000u64, 0x3_4000u64);
        for slot in 0..32u64 {
            let alias = backing_alias(base, slot);
            assert_eq!(alias, base + X2AVIC_BACKING_ALIASES_OFFSET + slot * 4096);
            // Inside the image's own last-level table, below every shared page.
            assert!(
                alias + 4096 <= base + X2AVIC_TABLE_OFFSET && alias >> 21 == base >> 21,
                "{slot}"
            );
            let pte = backing_alias_pte(pool, slot, offset);
            // Slot s's image is s MiB into the pool: its directory's
            // `avic_backing`, which DXE publishes in the table.
            assert_eq!(pte & 0x000f_ffff_ffff_f000, pool + slot * 0x10_0000 + offset, "{slot}");
            assert_eq!(pte & !0x000f_ffff_ffff_f000, (1 << 63) | 3, "RW/NX present");
        }
        assert_eq!(backing_alias(base, 31) + 4096, base + X2AVIC_TABLE_OFFSET);
    }

    /// Physical x2APIC model over MSRs 800h-8FFh. An EOI clears the highest
    /// ISR bit (APM2 16.6.4 p652) unless `broken_eoi` is set.
    struct Apic<'a> {
        registers: &'a mut [u64; 256],
        writes: &'a Cell<usize>,
        broken_eoi: bool,
    }

    impl PhysicalX2Apic for Apic<'_> {
        fn read(&mut self, msr: u32) -> u64 {
            self.registers[(msr - 0x800) as usize]
        }

        fn write(&mut self, msr: u32, value: u64) {
            self.writes.set(self.writes.get() + 1);
            if msr != 0x80b {
                self.registers[(msr - 0x800) as usize] = value;
            } else if let Some(bank) = (0x10..0x18).rev().find(|&bank| self.registers[bank] != 0)
                && !self.broken_eoi
            {
                let bits = &mut self.registers[bank];
                *bits &= !(1 << (63 - bits.leading_zeros()));
            }
        }
    }

    #[test]
    fn level_eoi_exit_intercepts_eoi_writes_until_the_next_one() {
        // A stale TMR bit with nothing held: the AVIC_NOACCEL fallback. The
        // ISR bit is still set, so the WRMSR at 1000h may run again (Table
        // 15-22 p566 trap, 15.29.9.2 p581 fault).
        let mut page = BackingPage::new();
        page.reset_stopped(1, GUEST_APIC_VERSION).unwrap();
        for base in [apic::ISR, apic::TMR] {
            page.write_register_stopped(base + 0x20, 1).unwrap(); // 40h
        }
        page.write_register_stopped(apic::ISR + 0x10, 1).unwrap(); // 20h
        let (mut ledger, mut msrpm, mut vmcb) =
            (PhysicalIrqLedger::new(), Msrpm::native_boot(), Vmcb::new());
        msrpm.configure_native_x2avic();
        let (mut registers, writes) = ([0u64; 256], Cell::new(0));
        let mut host = Apic { registers: &mut registers, writes: &writes, broken_eoi: false };
        irq::level_eoi_exit(0x40, 0x1000, &page, &mut ledger, &mut host).unwrap();
        sync_eoi_intercept(&ledger, &mut msrpm, &mut vmcb);
        assert!(!page.is_in_service(0x40) && page.is_in_service(0x20) && eoi_intercepted(&msrpm));
        // The re-executed write is recognized once and leaves 20h in service.
        assert!(ledger.take_eoi_replay(0x1000) && !ledger.take_eoi_replay(0x1000));
        sync_eoi_intercept(&ledger, &mut msrpm, &mut vmcb);
        assert!(page.is_in_service(0x20) && !eoi_intercepted(&msrpm) && writes.get() == 0);
    }

    #[test]
    fn accepted_vectors_publish_and_resynchronize_the_eoi_intercept() {
        let mut page = BackingPage::new();
        page.reset_stopped(1, GUEST_APIC_VERSION).unwrap();
        page.write_register_stopped(apic::SVR, 0x1ff).unwrap();
        let (mut ledger, mut msrpm, mut vmcb) =
            (PhysicalIrqLedger::new(), Msrpm::native_boot(), Vmcb::new());
        msrpm.configure_native_x2avic();
        let mut registers = [0u64; 256];
        registers[0x0f] = 0x1ff; // host-owned physical SVR
        let writes = Cell::new(0);
        let capture = |vector: u32,
                       registers: &mut [u64; 256],
                       page: &BackingPage,
                       ledger: &mut PhysicalIrqLedger,
                       msrpm: &mut Msrpm,
                       vmcb: &mut Vmcb| {
            put(vmcb, 0xc0, u64::from(u32::MAX));
            let mut apic = Apic { registers, writes: &writes, broken_eoi: false };
            capture_accepted(vector, page, ledger, &mut apic, msrpm, vmcb)
        };
        let clean = |vmcb: &Vmcb| get(vmcb, 0xc0) as u32;
        // Level 40h: published with TMR, held, EOI now intercepted, clean
        // bits cleared, no physical EOI.
        (registers[0x12], registers[0x1a]) = (1, 1);
        assert_eq!(
            capture(0x40, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Ok(Some(Capture::Level))
        );
        assert!(page.is_pending(0x40) && page.is_level(0x40) && ledger.holds(0x40));
        assert!(eoi_intercepted(&msrpm) && clean(&vmcb) == 0 && writes.get() == 0);
        // Edge 50h above it: published and acknowledged; the intercept and
        // the clean bits stay.
        registers[0x12] |= 1 << 16;
        assert_eq!(
            capture(0x50, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Ok(Some(Capture::Edge))
        );
        assert!(page.is_pending(0x50) && !page.is_level(0x50));
        assert_eq!((registers[0x12], writes.get(), clean(&vmcb)), (1, 1, u32::MAX));
        assert!(eoi_intercepted(&msrpm));
        // Software-disabled guest APIC: an edge source is only acknowledged,
        // a level source is still published and held.
        page.write_register_stopped(apic::SVR, 0xff).unwrap();
        registers[0x12] |= 1 << 1;
        assert_eq!(
            capture(0x41, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Ok(Some(Capture::Discarded))
        );
        assert!(!page.is_pending(0x41));
        assert_eq!((registers[0x12], writes.get()), (1, 2));
        (registers[0x13], registers[0x1b]) = (1, 1);
        assert_eq!(
            capture(0x60, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Ok(Some(Capture::Level))
        );
        assert!(page.is_pending(0x60) && ledger.holds(0x60) && writes.get() == 2);
        // The host spurious vector needs nothing.
        assert_eq!(
            capture(0xff, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Ok(None)
        );
        // Vectors 16-31 reach the bridge through the window gates and stop
        // with the vector; a helper result above 255 stops as well.
        registers[0x10] = 1 << 17;
        assert_eq!(
            capture(17, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Err((0xf571, (1 << 17) | (0x100 << 8) | 17))
        );
        assert_eq!(
            capture(0x100, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
            Err((0xf500, 0x100))
        );
        assert_eq!(writes.get(), 2);
    }

    /// Guest 1 of two, INIT pending: level source 40h held and physically in
    /// service, a busy guest register state, EOI writes intercepted.
    struct Fixture {
        profile: NativeX2AvicProfile,
        vmcb: Vmcb,
        frame: GuestRegisters,
        state: State,
        page: BackingPage,
        msrpm: Msrpm,
        registers: [u64; 256],
    }

    fn mailboxes() -> [NativeStartupMailbox; 2] {
        let mailboxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(1)];
        for mailbox in &mailboxes {
            mailbox.mark_running();
        }
        mailboxes
    }

    fn cache_core(phase: u32) -> CacheCore {
        CacheCore::new(CacheCoreState {
            bank: CacheObservation::EMPTY,
            members: 0b11,
            entering: 0,
            leaving: 0,
            departed: 0,
            phase,
            generation: 0,
        })
    }

    /// The destination history of slot 1, as a refused route to its filled
    /// queue reports it: (cause, INIT count). Leaves that queue full.
    fn history(mailboxes: &[NativeStartupMailbox; 2]) -> (NativeDestinationCause, u32) {
        while mailboxes[1].publish(NativeStartupCommand::Sipi(9)).is_ok() {}
        let mut source = NativeIcr::admit(0, &[0, 1]).unwrap();
        assert!(source.route_x2avic_startup(0x0000_0001_0000_0500, mailboxes, |_| {}).is_err());
        let recipient = source.route_failure().unwrap().recipient.unwrap();
        (recipient.cause, recipient.init_count)
    }

    impl Fixture {
        fn new() -> Self {
            let capabilities =
                X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18) | (1 << 25)).unwrap();
            let profile =
                NativeX2AvicProfile::new(capabilities, 0x2000, 0x3000, 1, &policy()).unwrap();
            let mut vmcb = Vmcb::new();
            // NP_ENABLE, as native preparation leaves it (Table B-1 090h).
            put(&mut vmcb, 0x90, 1);
            vmcb.set_virtual_interrupt_tpr(6).unwrap();
            vmcb.enable_native_x2avic(&profile).unwrap();
            let mut efer = NativeEfer::admit(0xd01, true).unwrap();
            efer.enable_guest_startup();
            let mut state = INITIAL_STATE;
            state.efer = Some(efer);
            state.guest_apic = Some(GuestX2Apic::admit(0xfee0_0c00, &policy()).unwrap());
            state.slot = 1;
            state.count = 2;
            state.irq.commit_level_capture(0x40).unwrap();
            let mut page = BackingPage::new();
            page.reset_stopped(1, GUEST_APIC_VERSION).unwrap();
            page.write_register_stopped(apic::TPR, 0x6b).unwrap();
            page.write_register_stopped(apic::LVT_TIMER, 0x2_00ef).unwrap();
            page.enqueue(0x40, true).unwrap();
            let mut msrpm = Msrpm::native_boot();
            msrpm.configure_native_x2avic();
            assert!(msrpm.update_x2apic_eoi_intercept(&state.irq));
            let mut registers = [0; 256];
            registers[0x0f] = 0x1ff; // host-owned physical SVR
            registers[0x12] = 1; // physical ISR 40h
            registers[0x32] = 0x2_00ef;
            registers[0x38] = 5000;
            let frame = GuestRegisters { rcx: 7, rdx: 9, ..GuestRegisters::default() };
            Self { profile, vmcb, frame, state, page, msrpm, registers }
        }

        /// `startup_step` for the command at the head of slot 1's queue.
        fn step(
            &mut self,
            mailboxes: &[NativeStartupMailbox; 2],
            writes: &Cell<usize>,
            broken_eoi: bool,
            core: Option<&CacheCore>,
            reset: impl FnOnce(),
        ) -> StartupStep {
            let command = mailboxes[1].peek().unwrap();
            let owners = InitOwners {
                backing: &self.page,
                physical: Apic { registers: &mut self.registers, writes, broken_eoi },
                msrpm: &mut self.msrpm,
                signature: 0x00b4_0f40,
            };
            startup_step(
                &mut self.state,
                &mut self.vmcb,
                &mut self.frame,
                mailboxes,
                &self.profile,
                command,
                core,
                owners,
                reset,
            )
        }

        fn backing(&self) -> [u32; 256] {
            core::array::from_fn(|index| self.page.read_register(index as u16 * 16).unwrap())
        }

        fn snapshot(&self) -> impl PartialEq + core::fmt::Debug + use<> {
            (
                *self.vmcb.bytes(),
                self.backing(),
                self.registers,
                self.state.irq,
                self.state.efer,
                self.state.guest_apic,
                *self.msrpm.bytes(),
                self.frame,
                self.state.startup,
            )
        }
    }

    #[test]
    fn guest_init_commits_d9_then_records_and_completes_under_the_route_lease() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        boxes[1].publish(NativeStartupCommand::Init).unwrap();
        // The guest enabled and then disabled its APIC with 40h pending, so
        // its APIC owner records 40h as held.
        {
            let unused = Cell::new(0);
            let mut apic = Apic { registers: &mut f.registers, writes: &unused, broken_eoi: false };
            let guest = f.state.guest_apic.as_mut().unwrap();
            for svr in [0x1ff, 0xff] {
                assert_eq!(
                    guest.emulate(0x80f, Some(svr), &f.page, &mut f.state.irq, &mut apic),
                    Emulation::Written
                );
            }
        }
        assert_ne!(f.state.guest_apic, Some(GuestX2Apic::admit(0xfee0_0c00, &policy()).unwrap()));
        let core = cache_core(0);
        let (writes, resets) = (Cell::new(0), Cell::new(0));
        let result = f.step(&boxes, &writes, false, Some(&core), || {
            // Steps 1-4 precede the debug reset: eight register resets and
            // the physical EOI of the retired source. The whole commit holds
            // the cache lease and not the route lease.
            assert_eq!(writes.get(), 9);
            assert!(core.try_lock().is_none());
            assert!(try_lock_routes(&boxes).is_ok());
            assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
            resets.set(resets.get() + 1);
        });
        assert_eq!(result, StartupStep::Applied(NativeStartupEffect::Init));
        assert_eq!(resets.get(), 1);
        // Both leases are free again and the command left the queue.
        assert!(core.try_lock().is_some() && try_lock_routes(&boxes).is_ok());
        assert_eq!(boxes[1].peek(), None);
        // Physical LAPIC: timer stopped, every LVT masked, source retired.
        for msr in 0x832..=0x837 {
            assert_eq!(f.registers[msr - 0x800], 0x1_0000, "{msr:#x}");
        }
        assert_eq!((f.registers[0x38], f.registers[0x3e], f.registers[0x12]), (0, 0, 0));
        assert_eq!(f.registers[0x0f], 0x1ff);
        assert!(f.state.irq.is_empty() && !eoi_intercepted(&f.msrpm));
        // Backing page: Table 16-2 values, ID kept, LDR derived for ID 1.
        let backing = f.backing();
        assert_eq!(backing[(apic::TPR / 16) as usize], 0);
        assert_eq!(backing[(apic::SVR / 16) as usize], 0xff);
        assert_eq!(backing[(apic::LVT_TIMER / 16) as usize], 0x1_0000);
        assert_eq!(backing[(apic::LDR / 16) as usize], 1 << 1);
        assert!(!f.page.is_pending(0x40) && !f.page.is_level(0x40));
        // CPU INIT state with V_TPR 0 and every clean bit clear.
        assert_eq!(f.vmcb.guest_rip(), 0xfff0);
        assert_eq!(f.vmcb.virtual_interrupt_control(), crate::svm::x2avic::NATIVE_CONTROL);
        assert_eq!(f.vmcb.bytes()[0xc0..0xc4], [0; 4]);
        assert!(f.vmcb.validate_native_x2avic(&f.profile).is_ok());
        assert_eq!((f.frame.rcx, f.frame.rdx), (0, 0x00b4_0f40));
        assert_eq!(f.state.startup, NativeStartupState::AwaitSipi);
        assert_eq!(f.state.efer.map(|efer| efer.logical()), Some(0));
        // The reset page holds nothing; APIC_BASE is unchanged (16.10 p657).
        assert_eq!(f.state.guest_apic, Some(GuestX2Apic::admit(0xfee0_0c00, &policy()).unwrap()));
        // The destination record names one guest INIT.
        assert_eq!(history(&boxes), (NativeDestinationCause::GuestInit, 1));
    }

    #[test]
    fn init_then_sipi_starts_the_guest_once() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        for command in [
            NativeStartupCommand::Init,
            NativeStartupCommand::Sipi(0x9a),
            NativeStartupCommand::Sipi(0x9b),
        ] {
            boxes[1].publish(command).unwrap();
        }
        let writes = Cell::new(0);
        assert_eq!(
            f.step(&boxes, &writes, false, None, || {}),
            StartupStep::Applied(NativeStartupEffect::Init)
        );
        assert_eq!(f.state.startup, NativeStartupState::AwaitSipi);
        // SIPI 9Ah: real-mode CS 9A00h based at 9A000h, IP 0 (APM2 15.27.8).
        assert_eq!(
            f.step(&boxes, &writes, false, None, || panic!("SIPI resets no debug state")),
            StartupStep::Applied(NativeStartupEffect::Started)
        );
        assert_eq!(f.state.startup, NativeStartupState::Running);
        assert_eq!(
            (get(&f.vmcb, 0x410) as u16, get(&f.vmcb, 0x418), f.vmcb.guest_rip()),
            (0x9a00, 0x9a000, 0)
        );
        // One start per INIT: the next SIPI is completed without an effect.
        let before = f.snapshot();
        assert_eq!(
            f.step(&boxes, &writes, false, None, || panic!("ignored SIPI")),
            StartupStep::Applied(NativeStartupEffect::Ignored)
        );
        assert_eq!(f.snapshot(), before);
        assert_eq!(boxes[1].peek(), None);
        assert_eq!(history(&boxes), (NativeDestinationCause::GuestInit, 1));
    }

    #[test]
    fn refused_startup_commands_change_nothing() {
        type Case = (fn(&mut Fixture), Option<u32>, StartupStep);
        let foreign = InitError::Irq(IrqError::UnexpectedPhysicalIsr(0x70));
        let cases: [Case; 7] = [
            // Physical ISR 70h (bank 3, bit 16) is not a held source.
            (
                |f| f.registers[0x13] = 1 << 16,
                None,
                StartupStep::Failed(
                    StartupStage::InitPreparation,
                    Some(u64::from(terminal::init_error_code(foreign))),
                ),
            ),
            (
                |f| f.state.efer = NativeEfer::admit(0xd01, true).ok(),
                None,
                StartupStep::Failed(StartupStage::EferReset, None),
            ),
            (|f| f.state.efer = None, None, StartupStep::Failed(StartupStage::OwnerMissing, None)),
            (
                |f| f.state.guest_apic = None,
                None,
                StartupStep::Failed(StartupStage::OwnerMissing, None),
            ),
            (
                |f| f.state.slot = 2,
                None,
                StartupStep::Failed(StartupStage::ModeCommitPreparation, None),
            ),
            (
                |f| put(&mut f.vmcb, 0xa8, 0x8000_0b0d),
                None,
                StartupStep::PendingEvent(ExternalInterruptError::PendingInjection),
            ),
            // Cache replay in progress on this core.
            (|_| {}, Some(1), StartupStep::Failed(StartupStage::CacheReplay, None)),
        ];
        for (index, (edit, phase, expected)) in cases.into_iter().enumerate() {
            let mut f = Fixture::new();
            let boxes = mailboxes();
            boxes[1].publish(NativeStartupCommand::Init).unwrap();
            edit(&mut f);
            let core = phase.map(cache_core);
            let before = f.snapshot();
            let writes = Cell::new(0);
            assert_eq!(
                f.step(&boxes, &writes, false, core.as_ref(), || panic!(
                    "debug reset after a refusal"
                )),
                expected,
                "case {index}"
            );
            assert_eq!(writes.get(), 0, "case {index}");
            assert_eq!(f.snapshot(), before, "case {index}");
            assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init), "case {index}");
            assert!(try_lock_routes(&boxes).is_ok());
            assert!(core.as_ref().is_none_or(|core| core.try_lock().is_some()));
            assert_eq!(history(&boxes), (NativeDestinationCause::Observed, 0), "case {index}");
        }
    }

    #[test]
    fn a_failed_lapic_commit_is_terminal_before_any_cpu_state_change() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        boxes[1].publish(NativeStartupCommand::Init).unwrap();
        let (vmcb, efer) = (*f.vmcb.bytes(), f.state.efer);
        let writes = Cell::new(0);
        // The physical EOI does not clear the ISR: the drain cannot finish.
        let result =
            f.step(&boxes, &writes, true, None, || panic!("debug reset after a failed commit"));
        let failure = InitError::Irq(IrqError::UnexpectedPhysicalIsr(0x40));
        assert_eq!(
            result,
            StartupStep::Failed(
                StartupStage::InitLapicCommit,
                Some(u64::from(terminal::init_error_code(failure)))
            )
        );
        // The physical reset happened; the CPU, EFER and backing did not
        // change, and the command stays queued without a destination record.
        assert_eq!(writes.get(), 9);
        assert_eq!((*f.vmcb.bytes(), f.state.efer), (vmcb, efer));
        assert_eq!(f.page.read_register(apic::TPR), Ok(0x6b));
        assert_eq!(f.state.startup, NativeStartupState::Running);
        assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
        assert_eq!(history(&boxes), (NativeDestinationCause::Observed, 0));
    }

    #[test]
    fn a_busy_cache_lease_defers_the_command_unchanged() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        boxes[1].publish(NativeStartupCommand::Init).unwrap();
        let core = cache_core(0);
        let sibling = core.try_lock().unwrap();
        let (before, writes) = (f.snapshot(), Cell::new(0));
        assert_eq!(
            f.step(&boxes, &writes, false, Some(&core), || panic!("busy")),
            StartupStep::Busy
        );
        assert_eq!((f.snapshot() == before, writes.get()), (true, 0));
        assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
        assert!(try_lock_routes(&boxes).is_ok());
        drop(sibling);
        assert_eq!(
            f.step(&boxes, &writes, false, Some(&core), || {}),
            StartupStep::Applied(NativeStartupEffect::Init)
        );
    }

    #[test]
    fn the_commit_runs_while_another_cpu_holds_the_route_lease() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        boxes[1].publish(NativeStartupCommand::Init).unwrap();
        let Fixture { profile, vmcb, frame, state, page, msrpm, registers } = &mut f;
        let page: &BackingPage = page;
        let (held, timed_out, writes) =
            (AtomicBool::new(false), AtomicBool::new(false), Cell::new(0));
        let step = std::thread::scope(|scope| {
            scope.spawn(|| {
                let routes = try_lock_routes(&boxes).unwrap();
                held.store(true, Ordering::SeqCst);
                // Release only once the destination has reset its backing
                // page (TPR 6Bh becomes 0), i.e. inside its INIT commit.
                let start = std::time::Instant::now();
                while page.read_register(apic::TPR) != Ok(0) {
                    if start.elapsed() > std::time::Duration::from_secs(20) {
                        timed_out.store(true, Ordering::SeqCst);
                        break;
                    }
                    std::thread::yield_now();
                }
                drop(routes);
            });
            while !held.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            let owners = InitOwners {
                backing: page,
                physical: Apic { registers, writes: &writes, broken_eoi: false },
                msrpm,
                signature: 0x00b4_0f40,
            };
            startup_step(
                state,
                vmcb,
                frame,
                &boxes,
                profile,
                NativeStartupCommand::Init,
                None,
                owners,
                || {},
            )
        });
        assert!(!timed_out.load(Ordering::SeqCst), "the INIT commit waited for the route lease");
        assert_eq!(step, StartupStep::Applied(NativeStartupEffect::Init));
        assert_eq!(boxes[1].peek(), None);
        assert_eq!(history(&boxes), (NativeDestinationCause::GuestInit, 1));
    }

    #[test]
    fn a_lost_route_lease_stops_after_the_bounded_wait_with_the_command_queued() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        boxes[1].publish(NativeStartupCommand::Init).unwrap();
        let writes = Cell::new(0);
        let routes = try_lock_routes(&boxes).unwrap();
        assert_eq!(
            f.step(&boxes, &writes, false, None, || {}),
            StartupStep::Failed(StartupStage::RouteTable, None)
        );
        drop(routes);
        // Applied but neither recorded nor completed: the stop is terminal.
        assert_eq!(f.state.startup, NativeStartupState::AwaitSipi);
        assert_eq!(f.page.read_register(apic::TPR), Ok(0));
        assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
        assert_eq!(history(&boxes), (NativeDestinationCause::Observed, 0));
    }
}
