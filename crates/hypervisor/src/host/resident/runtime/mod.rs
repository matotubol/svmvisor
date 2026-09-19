//! Executable per-CPU resident host. UEFI allocation/capture stays in DXE.
//! This image has one guest, no migration and no host FP/SIMD use. All linked
//! code (including panic/compiler builtins) needs the no-FP instruction audit.

use core::sync::atomic::{AtomicU32, AtomicU64};

use svmvisor_card_abi::endpoint::TerminalEndpoint;

#[cfg(test)]
use crate::host::resident::runtime::linked_symbols::*;
// The child files name `fetch`, `terminal`, the pool offsets, `is_valid_pool_slot` and
// `captured_register_refusal` as `super::..` inside function bodies that moved here unchanged.
use crate::{
    arch::x86_64::{capabilities::ValidatedCapabilities, registers::GuestRegisters},
    boot::memory::{MAX_DESCRIPTORS, MemoryDescriptor},
    guest::continuation::NativeBootstrapAck,
    host::resident::{
        BridgeContext, CACHE_CAPTURE_OFFSET, CACHE_OWNER_OFFSET, STARTUP_PAGE_OFFSET,
        X2AVIC_BACKING_ALIASES_OFFSET, X2AVIC_TABLE_OFFSET, captured_register_refusal, fetch,
        is_valid_pool_slot, runtime::exit::dispatch, terminal,
    },
    memory::{
        address::PAGE_BYTES,
        npt::{TABLE_COUNT, TableStorage},
    },
    svm::{
        dispatch::NativeEfer,
        permission_maps::{Iopm, Msrpm},
        vmcb::Vmcb,
        x2avic::{
            BackingPage, NativeX2AvicProfile,
            irq::PhysicalIrqLedger,
            registers::GuestX2Apic,
            startup::{NativeIcr, NativeStartupState},
        },
    },
};

pub use crate::host::resident::runtime::arm::prepare;

mod arm;
mod avic;
mod cache;
mod debug;
mod diagnostics;
mod exit;
mod guest_reader;
mod irq;
mod msr;
mod startup;
mod stop;
#[cfg(test)]
mod tests;

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
    cache_observation: Option<crate::svm::cache::CacheObservation>,
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

#[repr(C, align(4096))]
struct Pages<const N: usize>([[u8; 4096]; N]);

#[repr(C, align(4096))]
struct HostTables([[u64; 512]; 4]);
