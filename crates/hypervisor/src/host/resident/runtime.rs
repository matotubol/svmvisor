//! Executable per-CPU resident host. UEFI allocation/capture stays in DXE.
//! This image has one guest, no migration and no host FP/SIMD use. All linked
//! code (including panic/compiler builtins) needs the no-FP instruction audit.
#[path = "diagnostic_runtime.rs"]
mod diagnostics;
#[path = "cache_runtime.rs"]
mod cache;
use super::{BridgeContext, DIRECTORY_VERSION, ResidentDirectory, terminal::{self, TerminalEndpoint, TerminalControl}};
use crate::{
    address::EncryptionState,
    arch::x86_64::{
        encryption::NativeEncryptionPlan,
        msr::{
            HWCR, HWCR_CPUID_FLT_EN, MMIO_CFG_BASE_ADDR, MTRR_CAP, PAT, SYS_CFG, SYS_CFG_DEFINED,
            SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN, SYS_CFG_MTRR_FIX_DRAM_MOD_EN,
            TARGET_SIGNATURE,
        },
    },
    boot::memory::{MAX_DESCRIPTORS, MemoryDescriptor, ValidatedMemoryMap},
    capabilities::{
        CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
    },
    guest::continuation::NativeBootstrapAck,
    host::descriptors::HostDescriptorRequest,
    memory::npt::{PAGE_BYTES, TABLE_COUNT, TableStorage},
    registers::GuestRegisters,
    svm::{
        dispatch::{self, NativeEfer, NativeMsrOutcome},
        ipi::{
            NativeDestinationMode, NativeIcr, NativeIcrError, NativeStartupCommand,
            NativeStartupEffect, NativeStartupMailbox, NativeStartupState, NativeStartupTarget,
            try_lock_routes,
        },
        native_irq::{self, PhysicalIrqLedger},
        x2avic::{BackingPage, NativeX2AvicProfile, X2AvicCapabilities, PhysicalIdTable, AvicExit},
        permission_maps::{Iopm, MsrAccess, Msrpm, Permission},
        vmcb::Vmcb,
    },
};
use core::{
    arch::{asm, x86_64::__cpuid_count},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

#[unsafe(no_mangle)]
static svmvisor_resident_init_acks: AtomicU64 = AtomicU64::new(0);

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
#[unsafe(no_mangle)]
static mut svmvisor_resident_raw_vmexit: RawVmexitCapture = unsafe { core::mem::zeroed() };
// CPU-local circular history; no PCI traffic, heap, guest reads or global lock.
// Updated only on the ordinary stopped-guest stack, before dispatcher mutation.
static mut EXIT_HISTORY: [[u64;6];5] = [[0;6];5];
static mut EXIT_HISTORY_NEXT: usize = 0;
static mut EXIT_HISTORY_COUNT: usize = 0;


impl RawVmexitCapture {
    fn stop_record(self, expected_vmcb: u64, assigned_apic_id: u32,
        reason: u64, detail: u64) -> Option<(u8, [u64; 6])> {
        // Assembly captures GPR/VMCB provenance and physical APIC identity on
        // every MSR exit. Only the optional physical cache sample is SYS_CFG-
        // specific; other owned registers must not depend on that sample.
        let index = self.guest_rcx as u32;
        if self.code != 0x7c || !crate::svm::native_cache::owned_msr(index)
            || index == SYS_CFG && self.physical_cache_valid != 1 { return None; }
        if self.entry_sequence == 0 || self.entry_sequence != self.exit_sequence
            || self.entry_vmcb_pa != expected_vmcb || self.exit_vmcb_pa != expected_vmcb
            || self.context_vmcb_pa != expected_vmcb
            || self.physical_apic_id != u64::from(assigned_apic_id) {
            return Some((11, [self.exit_vmcb_pa, expected_vmcb,
                (self.physical_apic_id << 32) | u64::from(assigned_apic_id),
                self.exit_rip, self.nrip, self.entry_rip]));
        }
        if index == SYS_CFG {
            Some((10, [self.exit_rip, self.nrip, self.entry_rip, self.guest_cr0,
                self.mtrr_def_type, self.host_cr0]))
        } else {
            // EDX:EAX uses the low DWORD of each register. Preserve raw RCX
            // separately from its architectural low-DWORD MSR index. These
            // are boundary operands, not a claim the access was completed.
            let operand = (self.guest_rax as u32 as u64)
                | ((self.guest_rdx as u32 as u64) << 32);
            Some((13, [self.exit_rip, self.guest_rcx, operand, self.nrip, reason, detail]))
        }
    }
}

#[cfg(test)]
mod raw_capture_tests {
    use super::RawVmexitCapture;

    #[test]
    fn raw_boundary_requires_same_generation_vmcb_and_physical_cpu() {
        let mut raw: RawVmexitCapture = unsafe { core::mem::zeroed() };
        raw.entry_sequence = 19; raw.exit_sequence = 19;
        raw.entry_vmcb_pa = 0x123000; raw.exit_vmcb_pa = 0x123000;
        raw.context_vmcb_pa = 0x123000; raw.physical_apic_id = 21;
        raw.code = 0x7c; raw.guest_rcx = 0xc001_0010; raw.physical_cache_valid = 1;
        raw.exit_rip = 0xffff800000001111; raw.nrip = 0xffff800000002222;
        raw.entry_rip = 0xffff800000003333; raw.guest_cr0 = 0xe0000011;
        raw.mtrr_def_type = 0xc06; raw.host_cr0 = 0x80010011;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), Some((10,
            [0xffff800000001111, 0xffff800000002222, 0xffff800000003333,
             0xe0000011, 0xc06, 0x80010011])));
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
            assert_eq!(record.1, [bad.exit_vmcb_pa, 0x123000,
                (bad.physical_apic_id << 32) | 21, raw.exit_rip, raw.nrip, raw.entry_rip]);
        }
        raw.physical_cache_valid = 0;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
        raw.physical_cache_valid = 1; raw.guest_rcx = 0xc0000080;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
    }

    #[test]
    fn raw_cache_operands_require_provenance_but_no_syscfg_physical_sample() {
        let mut raw: RawVmexitCapture = unsafe { core::mem::zeroed() };
        raw.entry_sequence = 29; raw.exit_sequence = 29;
        raw.entry_vmcb_pa = 0x123000; raw.exit_vmcb_pa = 0x123000;
        raw.context_vmcb_pa = 0x123000; raw.physical_apic_id = 21;
        raw.code = 0x7c; raw.exit_rip = 0xffff800000001111;
        raw.nrip = raw.exit_rip + 2;
        raw.guest_rax = 0xfeedface76543210; raw.guest_rdx = 0xdeadc0defedcba98;
        for index in crate::svm::native_cache::owned_msrs().filter(|&index| index != 0xc001_0010) {
            raw.guest_rcx = 0x1234567800000000 | u64::from(index);
            assert_eq!(raw.stop_record(0x123000, 21, 0x12345678_f400, 0xfedcba98_00000010),
                Some((13, [raw.exit_rip, raw.guest_rcx, 0xfedcba9876543210,
                    raw.nrip, 0x12345678_f400, 0xfedcba98_00000010])));
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
        raw.guest_rcx = 0xc001_0015; raw.code = 0x72;
        assert_eq!(raw.stop_record(0x123000, 21, 0xf400, 16), None);
    }
}

#[repr(C, align(4096))]
struct Pages<const N: usize>([[u8; 4096]; N]);
#[repr(C, align(4096))]
struct HostTables([[u64; 512]; 4]);
static mut TABLES: HostTables = HostTables([[0; 512]; 4]);
static mut NPT: TableStorage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
static mut CACHE_NPT: crate::memory::npt::LowMemoryNptStorage = crate::memory::npt::LowMemoryNptStorage::empty();
static mut VMCB: Vmcb = Vmcb::new();
static mut AVIC_BACKING: BackingPage = BackingPage::new();
static mut AUX: Vmcb = Vmcb::new();
static mut HOST_EXTRA: Vmcb = Vmcb::new();
static mut HSAVE: Pages<1> = Pages([[0; 4096]; 1]);
static mut STACK: Pages<18> = Pages([[0; 4096]; 18]);
static mut FAULT_STACK: Pages<6> = Pages([[0; 4096]; 6]);
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
    apic_base: u64,
    avic: Option<NativeX2AvicProfile>,
    irq: PhysicalIrqLedger,
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
    terminal_endpoint: Option<TerminalEndpoint>,
    stopped_valid: bool,
}
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
    apic_base: 0,
    avic: None,
    irq: PhysicalIrqLedger::new(),
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
    terminal_endpoint: None,
    stopped_valid: false,
};
static mut STATE: State = INITIAL_STATE;
static mut RAM: [MemoryDescriptor; MAX_DESCRIPTORS] = [MemoryDescriptor {
    memory_type: 0,
    physical_start: 0,
    page_count: 0,
    attributes: 0,
}; MAX_DESCRIPTORS];
static mut RAM_COUNT: usize = 0;
static mut PHYSICAL_BITS: u8 = 0;
static mut POOL: (u64, u64) = (0, 0);
static mut ASSIGNED_APIC_ID: u32 = u32::MAX;

unsafe extern "C" {
    static image_start: u8;
    static text_end: u8;
    static data_start: u8;
    static image_bss_end: u8;
    static svmvisor_resident_fault_offsets: [i32; 256];
    static svmvisor_resident_irq_offsets: [i32; 224];
    fn svmvisor_resident_accept_irq() -> u32;
    fn svmvisor_resident_sx();
}
unsafe extern "win64" {
    fn svmvisor_resident_enter(context: *mut BridgeContext) -> !;
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
    unsafe { diagnostic_record(4, true, [rip, error, cr2, cr3, rsp, flags], vector); diagnostics::flush_fault(); }
    unsafe { asm!("cli", "2: hlt", "jmp 2b", options(noreturn, nostack)); }
}

/// # Safety
/// Call once in a legal loaded raw runtime allocation under live native CPL0
/// identity mappings. Caller has validated the entire 1MiB backing RW/X/WB,
/// original stack and this exact linked package. No CPU control is changed.
/// `output` is disjoint writable caller storage. Firmware owns allocation.
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
        || end > base + super::SOURCE_ROUTES_OFFSET
        || (text_limit | data | end) & 4095 != 0
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
    let gdt = ptr::addr_of_mut!(GDT_TSS) as u64;
    let idt = ptr::addr_of_mut!(IDT) as u64;
    let stack_top = stack + 17 * 4096;
    let fault_top = fault_stack + 5 * 4096;
    let fault_offsets = ptr::addr_of!(svmvisor_resident_fault_offsets);
    let mut handlers = core::array::from_fn(|vector| {
        (fault_offsets as u64).wrapping_add(unsafe { (*fault_offsets)[vector] } as i64 as u64)
    });
    handlers[30] = svmvisor_resident_sx as *const () as u64;
    let irq_offsets = ptr::addr_of!(svmvisor_resident_irq_offsets);
    for vector in 32..256 {
        handlers[vector] = (irq_offsets as u64).wrapping_add(unsafe { (*irq_offsets)[vector - 32] } as i64 as u64);
    }
    let request = HostDescriptorRequest {
        gdt_base: gdt,
        tss_base: gdt + 128,
        idt_base: idt,
        rsp0: stack_top,
        ist1: fault_top,
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
        // Returning IRQ gates use the current private host stack, not terminal IST1.
        for vector in 32..256 { (idt as *mut u8).add(vector * 16 + 4).write(0); }
        let gdtr = &mut *ptr::addr_of_mut!(GDTR);
        gdtr[..2].copy_from_slice(&descriptors.gdtr().limit.to_le_bytes());
        gdtr[2..].copy_from_slice(&gdt.to_le_bytes());
        let idtr = &mut *ptr::addr_of_mut!(IDTR);
        idtr[..2].copy_from_slice(&4095u16.to_le_bytes());
        idtr[2..].copy_from_slice(&idt.to_le_bytes());
    }
    let tables = unsafe { &mut (*ptr::addr_of_mut!(TABLES)).0 };
    tables[0][0] = root + 4096 | 3;
    tables[1][0] = root + 8192 | 3;
    tables[2][((base >> 21) & 511) as usize] = root + 12288 | 3;
    for page in (base..end).step_by(4096) {
        if [stack, stack_top, fault_stack, fault_top].contains(&page) {
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
        pool_base + super::X2AVIC_TABLE_OFFSET | 3 | (1 << 63);
    for offset in (super::SOURCE_ROUTES_OFFSET..super::X2AVIC_TABLE_OFFSET).step_by(4096) {
        tables[3][(((base + offset) >> 12) & 511) as usize] = pool_base + offset | 3 | (1 << 63);
    }
    let shared_alias = base + super::STARTUP_PAGE_OFFSET;
    tables[3][((shared_alias >> 12) & 511) as usize] =
        pool_base + super::STARTUP_PAGE_OFFSET | 3 | (1 << 63);
    for offset in (super::CACHE_OWNER_OFFSET..super::CACHE_CAPTURE_OFFSET).step_by(4096) {
        tables[3][(((base + offset) >> 12) & 511) as usize] = pool_base + offset | 3 | (1 << 63);
    }
    for offset in (super::CACHE_CAPTURE_OFFSET..super::CACHE_CAPTURE_OFFSET
        + core::mem::size_of::<crate::svm::native_cache::CacheCapture>() as u64).step_by(4096)
    {
        tables[3][(((base + offset) >> 12) & 511) as usize] =
            pool_base + offset | 1 | (1 << 63);
    }
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    if vmcb.configure_native_boot_intercepts().is_err() {
        return 4;
    }
    // Initialize before DXE publishes this address in the shared AVIC table.
    if unsafe { &mut *ptr::addr_of_mut!(AVIC_BACKING) }
        .reset_stopped(apic_id as u32, 0x0005_0010).is_err() { return 4; }

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
        avic_backing: ptr::addr_of!(AVIC_BACKING) as u64,
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
        asm!("rdmsr",in("ecx")0xc0010114u32,out("eax")low,out("edx")high,options(nostack));
    }
    if low & (1 << 4) != 0 || high != 0 {
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
        optional: OptionalFeatures {
            nrip_save: svm.edx & 8 != 0,
            ..Default::default()
        },
    }
    .validate()
    .ok()
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
    let endpoint = if terminal_endpoint.is_null() { None } else {
        // Caller validated this immutable input mapping for this arm invocation.
        // PPR57896 applies only to Family1Ah Model44h B0, signature00B40F40h.
        if !startup_owned || terminal_endpoint as usize & 7 != 0
            || __cpuid_count(1,0).eax != TARGET_SIGNATURE { return 10; }
        let value = unsafe { terminal_endpoint.read() };
        if !value.valid() || unsafe { read_msr(MMIO_CFG_BASE_ADDR) } != value.mmio_config_msr { return 10; }
        Some(value)
    };
    if ids.is_null()
        || ids as usize % core::mem::align_of::<u32>() != 0
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
            let Some(slot) = ids.iter().position(|&id| id == assigned_id) else { return 12; };
            let Some(original) = capture.observation(slot, id_count) else { return 12; };
            let Some(members) = capture.domain_mask(slot, ids) else { return 12; };
            let current = CacheObservation::capture(__cpuid_count(1, 0).eax,
                caps.address_policy().physical_bits(), crate::svm::native_cache::native_topology(),
                |index| unsafe { read_msr(index) }, |index, value| unsafe { write_msr(index, value) });
            let Some(current) = current else { return 12; };
            if current.topology[0] != assigned_id || !original.same_physical_state(&current) { return 12; }
            state.cache_observation = Some(current);
            state.cache_core = members.trailing_zeros() as usize;
            state.cache_visibility = current.sys_cfg & SYS_CFG_MTRR_FIX_DRAM_MOD_EN != 0;
        }
    }
    let Ok(icr_owner) = NativeIcr::admit(assigned_id, ids) else {
        return 7;
    };
    // Exclusive handoff: the loader must already use x2APIC. An xAPIC
    // continuation is refused, never promoted.
    let apic_base = unsafe { read_msr(0x1b) };
    let Ok(avic_caps) = X2AvicCapabilities::admit(__cpuid_count(1, 0).ecx,
        __cpuid_count(0x8000_000a, 0).edx) else { return 8; };
    if apic_base & !0xd00 != 0xfee0_0000 || apic_base & 0xc00 != 0xc00
        || ids.iter().any(|&id| id > 511)
        || unsafe { read_msr(0x802) } != assigned_id as u64 { return 8; }
    let Ok(avic) = NativeX2AvicProfile::new(avic_caps,
        ptr::addr_of!(AVIC_BACKING) as u64,
        unsafe { POOL.0 } + super::X2AVIC_TABLE_OFFSET,
        *ids.iter().max().unwrap() as u16, &caps.address_policy()) else { return 8; };
    // Before table publication/guest entry, physical sources must have no
    // inherited in-service ownership. Pending physical IRR is captured later.
    if unsafe { native_irq::physical_highest_in_service() }.is_some() { return 8; }
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    // Preserve the captured guest interface; ISR/IRR belong to the bridge.
    for offset in [0x80, 0xf0, 0x320, 0x330, 0x340, 0x350, 0x360, 0x370, 0x380, 0x3e0] {
        let value = unsafe { read_msr(0x800 + u32::from(offset / 16)) };
        if value > u32::MAX as u64 || backing.write_register_stopped(offset, value as u32).is_err() { return 8; }
    }
    // The physical bootstrap changed the BSP's ICR. Preserve its captured
    // logical readback in the virtual page without sending a second command.
    let captured_icr = if initial_icr.is_null() { unsafe { read_msr(0x830) } } else {
        if initial_icr as usize & 7 != 0
            || caps.address_policy().validate(initial_icr as u64, 8, 8).is_err() { return 8; }
        unsafe { initial_icr.read() }
    };
    if backing.write_register_stopped(0x300, captured_icr as u32).is_err()
        || backing.write_register_stopped(0x310, (captured_icr >> 32) as u32).is_err() { return 8; }
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    let frame = unsafe { &*ptr::addr_of!(FRAME) };
    if let Some(endpoint) = endpoint {
        let slot = ids.iter().position(|&id| id == assigned_id).unwrap();
        if !unsafe { diagnostics::prepare(endpoint,slot,id_count) } { return 10; }
        let io = unsafe { &mut *ptr::addr_of_mut!(IOPM) };
        if io.set_range(0,65536,Permission::Allow).is_err()
            || io.set_range(0xcf8,8,Permission::Intercept).is_err()
            || unsafe { &mut *ptr::addr_of_mut!(MSRPM) }.set(
                MMIO_CFG_BASE_ADDR,MsrAccess::Write,Permission::Intercept).is_err() { return 10; }
        vmcb.set_instruction_intercept(crate::svm::vmcb::InstructionIntercept::Ioio,true);
    }
    // Capture only enumerated same-CPU features, before the first guest entry.
    // Guest FXSR and INVLPG execute directly; VMRUN/VMEXIT owns EFER switching.
    let extended = __cpuid_count(0x8000_0001, 0);
    let extended21 = if __cpuid_count(0x8000_0000, 0).eax >= 0x8000_0021 {
        Some(__cpuid_count(0x8000_0021, 0).eax)
    } else {
        None
    };
    let Ok(mut owner) = NativeEfer::admit_native(efer, extended.ecx, extended.edx,
        __cpuid_count(0x8000_0008, 0).ebx, extended21) else {
        return 3;
    };
    let Ok(ack_owner) = NativeBootstrapAck::new(vmcb, frame, resume, ack, after) else {
        return 4;
    };
    let policy = caps.address_policy();
    if map.is_null()
        || map as usize % core::mem::align_of::<MemoryDescriptor>() != 0
        || count == 0
        || count > MAX_DESCRIPTORS
    {
        return 6;
    }
    let descriptors = unsafe { core::slice::from_raw_parts(map, count) };
    if ValidatedMemoryMap::new(descriptors, policy.physical_bits()).is_err() {
        return 6;
    }
    if id_count > 1 {
        unsafe {
            (&mut *ptr::addr_of_mut!(MSRPM)).configure_native_x2avic();
        }
    }
    if state.cache_observation.is_some() {
        if !startup_owned || unsafe { !cache::prepare_root() } { return 12; }
        let maps = unsafe { &mut *ptr::addr_of_mut!(MSRPM) };
        for index in crate::svm::native_cache::owned_msrs() {
            for access in [MsrAccess::Read, MsrAccess::Write] {
                if maps.set(index, access, Permission::Intercept).is_err() { return 12; }
            }
        }
    }
    if vmcb
        .set_permission_maps(
            ptr::addr_of!(IOPM) as u64,
            ptr::addr_of!(MSRPM) as u64,
            &policy,
        )
        .is_err()
        || vmcb.set_guest_asid(1, &caps).is_err()
        || vmcb
            .set_nested_root(ptr::addr_of!(NPT) as u64, &policy)
            .is_err()
        || vmcb.enable_native_nested_paging(&policy).is_err()
    {
        return 5;
    }
    if !startup_owned || id_count < 2 { return 9; }
    let shared = unsafe { core::slice::from_raw_parts(
        (POOL.0 + super::STARTUP_PAGE_OFFSET) as *const NativeStartupMailbox, id_count) };
    // APM2 15.21.2/15.29.5: VMRUN loads V_TPR and AVIC CR8 reads use it.
    // Seed the priority class as well as backing TPR before enabling AVIC.
    let Ok(captured_tpr) = backing.read_register(0x80) else { return 9; };
    if captured_tpr > 0xff
        || vmcb.set_virtual_interrupt_tpr((captured_tpr >> 4) as u8).is_err() { return 9; }
    if shared.iter().zip(ids).any(|(mailbox, &id)| mailbox.identity() != id)
        || vmcb.enable_native_x2avic(&avic).is_err() { return 9; }
    owner.enable_guest_startup();
    // Software startup commands retain the existing target-owned mailbox.
    // Ordinary fixed IPIs use x2AVIC, never a physical guest ICR write.
    let Ok(routes) = try_lock_routes(shared) else { return 9; };
    let slot = ids.iter().position(|&id| id == assigned_id).unwrap();
    let Ok(commit) = routes.prepare_destination_mode(slot, NativeDestinationMode::X2Apic) else { return 9; };
    unsafe {
        let original = read_msr(0xc0010114);
        write_msr(0xc0010114, original | 2);
        if read_msr(0xc0010114) != original | 2 { write_msr(0xc0010114, original); return 9; }
    }
    commit.commit_destination_mode();
    // Host physical TPR must not inherit a guest priority threshold. Guest CR8
    // and TPR now use AVIC; the physical LAPIC is a source capture backend.
    unsafe {
        write_msr(0x808, 0);
        // Host capture owns physical software-enable and spurious vector.
        // The captured guest SVR remains in its separate backing register.
        write_msr(0x80f, 0x1ff);
    }
    let table = unsafe { &*((POOL.0 + super::X2AVIC_TABLE_OFFSET) as *const PhysicalIdTable) };
    if table.set_running(assigned_id as u16, true).is_err() { return 9; }
    state.avic = Some(avic);
    unsafe {
        ptr::copy_nonoverlapping(map, ptr::addr_of_mut!(RAM).cast(), count);
        RAM_COUNT = count;
        PHYSICAL_BITS = policy.physical_bits();
    }
    state.ack = Some(ack_owner);
    state.efer = Some(owner);
    state.icr = if id_count > 1 { Some(icr_owner) } else { None };
    state.apic_base = apic_base;
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

const _: super::ArmRuntime = arm;

/// GIF/IF clear, initialized per-CPU runtime; independent of mutable STATE.
unsafe fn diagnostic_record(event:u8,fault:bool,context:[u64;6],aux:u32) {
    unsafe { diagnostics::record(event,fault,context,aux); }
}

unsafe fn handle_diagnostic_ecam(state:&mut State,vmcb:&mut Vmcb,base:u64,bytes:u64) -> bool {
    let exit=vmcb.exit_snapshot();
    if exit.info1 & 0x1f != 7 { return stop(state,exit.code,exit.rip,exit.info1,exit.info2); }
    let Some(guard) = (unsafe { terminal_control() }).diagnostic_lock() else { return retry_routing(state,vmcb); };
    // Remove only our write restriction after permanently revoking publication.
    // Hardware retries the original instruction; no GPR/RIP/event is emulated.
    unsafe { diagnostics::revoke(exit.info2,0,0,1); }
    let result=crate::memory::npt::restore_identity_write_range(
        unsafe { &mut *ptr::addr_of_mut!(NPT) },ptr::addr_of!(NPT) as u64,base,bytes);
    drop(guard);
    if result.is_err() { return stop(state,exit.code,exit.rip,0xf203,exit.info2); }
    vmcb.request_full_tlb_flush(); state.routing_retries=0; true
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
        ptr::addr_of_mut!(EXIT_HISTORY).cast::<[u64;6]>().add(index).write(
            [before.rip,before.code,before.info1,before.info2,frame.rcx,
             (rax as u32 as u64)|((frame.rdx as u32 as u64)<<32)]);
        EXIT_HISTORY_NEXT = (index+1)%5;
        EXIT_HISTORY_COUNT = (EXIT_HISTORY_COUNT+1).min(5);
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
        let vmcb=unsafe { &*ptr::addr_of!(VMCB) };
        unsafe { diagnostic_record(2,false,[vmcb.guest_rip(),before.code,before.info1,
            before.info2,vmcb.guest_cr3(),state.exits],0); }
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
        let exit = vmcb.exit_snapshot();
        return stop(state, exit.code, exit.rip, 0xf10b, 0);
    }
    if state.avic.as_ref().is_none_or(|profile| vmcb.validate_native_x2avic(profile).is_err()) {
        let exit = vmcb.exit_snapshot();
        return stop(state, exit.code, exit.rip, 0xf520, 2);
    }
    let observed = vmcb.exit_snapshot();
    // Every unusual exit and MSR boundary; common CPUID/PAUSE samples are
    // bounded to avoid making diagnostic PCI traffic dominate guest execution.
    if !matches!(observed.code,0x72|0x77) || state.exits & 0xff == 1 {
        unsafe { diagnostic_record(1,false,[observed.rip,observed.code,observed.info1,
            observed.info2,vmcb.guest_cr3(),state.exits],
            if observed.code == 0x7c { frame.rcx as u32 } else { 0 }); }
    }
    // Audited assembly has returned from VMRUN on this CPU. Consume its old
    // flush before INIT, EFER or any other dispatcher mutation can re-arm it.
    // An invalid entry does not establish that the requested flush occurred.
    unsafe { vmcb.consume_tlb_flush_after_exit(); }
    if state.startup_owned {
        #[cfg(feature = "resident-runtime-test")]
        let irq_witness = unsafe {
            (
                read_native_apic(0x80),
                read_native_apic(0x270),
                read_native_apic(0x170),
            )
        };
        let Some(acknowledged) = (unsafe { acknowledge_init() }) else {
            return stop(
                state,
                vmcb.exit_snapshot().code,
                vmcb.guest_rip(),
                0xf102,
                0,
            );
        };
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
            && (unsafe { mailboxes(state.count) })[state.slot]
                .peek()
                .is_some()
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
                hex(unsafe { read_native_apic(0x80) });
                debug(b" irr-f1-after=");
                hex(unsafe { read_native_apic(0x270) } & (1 << 17));
                debug(b" isr-f1-after=");
                hex(unsafe { read_native_apic(0x170) } & (1 << 17));
            }
            debug(b"\n");
        }
    }
    if !check_exit_event(state, vmcb) { return false; }
    let exit = vmcb.exit_snapshot();
    // A physical source can arrive before the bootstrap VMMCALL. Capture it
    // without falsely treating that asynchronous exit as a failed guest ACK.
    if exit.code == 0x60 { return unsafe { capture_physical_irq(state, vmcb) }; }
    if let Some(ack) = state.ack.as_mut() {
        if !ack.acknowledged() {
            if ack.acknowledge(vmcb, frame).is_ok() {
                if state.startup_owned {
                    (unsafe { mailboxes(state.count) })[state.slot].mark_running();
                    if terminal_enabled(state) {
                        // Separate monotonic initial-ACK mask, not mutable guest
                        // startup readiness. Last initial guest ACK opens export.
                        unsafe { terminal_control() }.initial_ack(state.slot,state.count);
                        unsafe { diagnostic_record(9,false,[vmcb.guest_rip(),vmcb.guest_cr3(),
                            state.slot as u64,state.count as u64,0,0],0); }
                    }
                }
                debug(b"resident-ack\n");
                return true;
            }
            return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
        }
    } else {
        return stop(state, exit.code, exit.rip, 0xf10d, 0);
    }
    if state.startup_owned {
        match unsafe { service_startup(state, vmcb, frame) } {
            Some(true) => return true,
            Some(false) if unsafe { terminal_requested(state) } => return false,
            Some(false) if state.stopped_valid => return false,
            Some(false) => return stop(state, exit.code, exit.rip, 0xf103, 0),
            None => {}
        }
    }
    #[cfg(feature = "resident-runtime-test")]
    if exit.code == 0x400 && state.cache_fixture && state.cache_active {
        debug(b"native-cache-fixture-low-npf gpa="); hex(exit.info2);
        debug(b" root="); hex(vmcb.nested_root()); debug(b"\n");
    }
    match exit.code {
        0x77 => {
            // Reenter unchanged: VMRUN replenishes the nonzero PAUSE count,
            // and hardware executes this instruction, including debug state.
            unsafe { diagnostic_record(5,false,[exit.rip,vmcb.guest_cr3(),state.exits,0,0,0],0); }
            if crate::svm::native_pause::native_pause_retry_ready(vmcb) { return true; }
            return stop(state,exit.code,exit.rip,exit.info1,exit.info2);
        }
        0x63 if state.startup_owned => {
            // Actual host #SX(error1) acknowledgment was checked above. A
            // notification is only a wakeup: commands live in the mailbox,
            // and multiple notifications may coalesce without losing commands.
            if vmcb.validate_external_interrupt_conflicts().is_ok() {
                return true;
            }
        }
        0x72 => {
            // The per-CPU arm observation is retained in this private runtime.
            // APM2 15.7.1 makes hardware nRIP authoritative for instruction
            // intercepts. Select this path before any guest-memory access;
            // invalid nRIP must stop, never fall back to rereading the opcode.
            let hardware_nrip = state.capabilities.filter(|caps| {
                caps.optional_features().nrip_save
            });
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
                    let mut reader = match unsafe { GuestReader::new(vmcb, state.startup_owned, state.count) } {
                        Ok(reader) => reader,
                        Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
                    };
                    let bytes = super::fetch::cpuid_instruction(vmcb, reader.width,
                        reader.guest_pat, length, state.startup_owned,
                        |address, bytes| unsafe { reader.read(address, bytes) });
                    match bytes {
                        Ok(bytes) => prefixed = Some((bytes, length)),
                        Err(error) => {
                            let reason = reader.failure.map_or_else(
                                || terminal::fetch_failure_code(error), |e| e as u16);
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
                    Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => return retry_routing(state, vmcb),
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
            } else { response };
            #[cfg(feature = "resident-runtime-test")]
            let response = if leaf == 0x4fff0000 {
                [
                    0x53564d52,
                    state.exits as u32,
                    state.cpuid as u32,
                    state.msr as u32,
                ]
            } else {
                response
            };
            state.cpuid = state.cpuid.saturating_add(1);
            let cpuid_user_disabled = vmcb.bytes()[0x4cb] != 0
                && unsafe { read_msr(HWCR) } & HWCR_CPUID_FLT_EN != 0;
            let result = if let Some(caps) = hardware_nrip {
                dispatch::handle_native_cpuid_with_nrip(
                    vmcb, frame, &caps, response, state.startup_owned,
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
        0x7b if diagnostics::available() => return unsafe { diagnostics::handle_io(state,vmcb) },
        0x7c => {
            if state.cache_observation.is_some() && crate::svm::native_cache::owned_msr(frame.rcx as u32) {
                return unsafe { cache::handle(state, vmcb, frame) };
            }
            if frame.rcx as u32 == MMIO_CFG_BASE_ADDR && diagnostics::available() {
                // Dynamic ECAM relocation is not yet an owned instruction path.
                // Keep the actual stopped operands before any native write.
                let requested = (vmcb.guest_rax() as u32 as u64) | ((frame.rdx as u32 as u64)<<32);
                return stop(state,exit.code,exit.rip,0xf202,requested);
            }
            if frame.rcx as u32 == SYS_CFG {
                return unsafe { handle_syscfg(state, vmcb, frame) };
            }
            if frame.rcx as u32 == 0x1b || (0x800..=0x8ff).contains(&(frame.rcx as u32)) {
                return unsafe { handle_avic_msr(state, vmcb, frame) };
            }
            // Actual same-CPU MSR exit plus NRIPS owns the decoded instruction
            // length, including prefixes. Route owned MSRs before guest-byte fetch.
            // Other MSRs and non-long64 profiles retain their existing byte owner.
            let hardware_nrip = state.capabilities.filter(|caps| {
                caps.optional_features().nrip_save && vmcb.guest_in_64_bit_code()
                    && vmcb.bytes()[0x4cb] == 0 && matches!(frame.rcx as u32, 0xc000_0080 | 0xc001_0114)
            });
            if frame.rcx as u32 == 0xc001_0114 {
                if let Some(caps) = hardware_nrip {
                    match dispatch::handle_native_vmcr_with_nrip(vmcb, frame, &caps, state.startup_owned) {
                        Ok(NativeMsrOutcome::Completed) => { state.routing_retries = 0; return true; }
                        Ok(NativeMsrOutcome::GeneralProtectionPrepared) => { state.pending_fault = true; return true; }
                        Err(error) => {
                            let (reason, value) = super::terminal::vmcr_nrip_failure(error, vmcb, dispatch::NATIVE_VM_CR_VALUE);
                            return stop(state, exit.code, exit.rip, reason, value);
                        }
                    }
                }
            }
            if let (Some(caps), Some(efer)) = (hardware_nrip, state.efer.as_mut()) {
                match dispatch::handle_native_efer_with_nrip(efer, vmcb, frame, &caps) {
                    Ok(NativeMsrOutcome::Completed) => { state.routing_retries = 0; return true; }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => { state.pending_fault = true; return true; }
                    Err(error) => {
                        let (reason, value) = super::terminal::efer_nrip_failure(error, vmcb, efer.logical());
                        return stop(state, exit.code, exit.rip, reason, value);
                    }
                }
            }
            let instruction = match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
                Ok(bytes) => bytes,
                Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => return retry_routing(state, vmcb),
                Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
            };
            state.msr = state.msr.saturating_add(1);
            if frame.rcx as u32 == 0xc001_0114 {
                match dispatch::handle_native_vmcr(vmcb, frame, &instruction, state.startup_owned) {
                    Ok(NativeMsrOutcome::Completed) => { state.routing_retries = 0; return true; }
                    Ok(NativeMsrOutcome::GeneralProtectionPrepared) => { state.pending_fault = true; return true; }
                    Err(error) => {
                        let (reason, value) = super::terminal::vmcr_failure(error, vmcb, dispatch::NATIVE_VM_CR_VALUE, instruction);
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
                        let (reason, value) = super::terminal::efer_failure(error, vmcb, efer.logical(), instruction);
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
            if let Some(endpoint) = unsafe { diagnostics::endpoint() } {
                if let Some((base,bytes)) = endpoint.config_aperture() {
                    if let Ok((start,end)) = crate::memory::npt::identity_protection_range(base,bytes) {
                        if (start..end).contains(&exit.info2) {
                            return unsafe { handle_diagnostic_ecam(state,vmcb,base,bytes) };
                        }
                    }
                }
            }
            return stop(state, exit.code, exit.rip, exit.info1, exit.info2);
        }
        0x60 => return unsafe { capture_physical_irq(state, vmcb) },
        0x401 | 0x402 => return unsafe { handle_avic_exit(state, vmcb, frame) },
        _ => {}
    }
    stop(state, exit.code, exit.rip, exit.info1, exit.info2)
}

/// Accept one physical source through the bounded assembly mailbox. Its gate
/// touches no Rust owner; IF/GIF are clear before this function reads state.
unsafe fn capture_physical_irq(state: &mut State, vmcb: &mut Vmcb) -> bool {
    let exit = vmcb.exit_snapshot();
    let vector = unsafe { svmvisor_resident_accept_irq() };
    if vector == u32::MAX { return retry_routing(state, vmcb); }
    if vector > 255 { return stop(state, exit.code, exit.rip, 0xf500, vector as u64); }
    let vector = vector as u8;
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    let highest = unsafe { native_irq::physical_highest_in_service() };
    // A physical spurious interrupt has no ISR bit and needs no EOI.
    if highest != Some(vector) && vector == unsafe { read_msr(0x80f) } as u8 {
        return true;
    }
    // A physical capture cannot share a vector with a direct-device owner.
    // Keep the guard until publication, so route installation cannot race it.
    let Ok(routes) = (unsafe { source_routes() }).try_lock() else {
        return stop(state, exit.code, exit.rip, 0xf505, vector as u64);
    };
    if routes.has_route(unsafe { ASSIGNED_APIC_ID } as u16, vector) != Ok(false) {
        return stop(state, exit.code, exit.rip, 0xf505, vector as u64);
    }
    let level = unsafe { native_irq::physical_level_triggered(vector) };
    let capture = match state.irq.prepare_capture(vector, level, highest,
        backing.is_pending(vector), backing.is_in_service(vector)) {
        Ok(capture) => capture,
        Err(_) => return stop(state, exit.code, exit.rip, 0xf501, vector as u64),
    };
    if backing.enqueue(vector, level).is_err() {
        return stop(state, exit.code, exit.rip, 0xf502, vector as u64);
    }
    match capture {
        native_irq::Capture::Edge => unsafe { native_irq::physical_eoi() },
        native_irq::Capture::Level => if state.irq.commit_level_capture(vector).is_err() {
            return stop(state, exit.code, exit.rip, 0xf503, vector as u64);
        },
    }
    state.intr = state.intr.saturating_add(1);
    state.routing_retries = 0;
    unsafe { drain_physical_eoi(state, vmcb) }
}

unsafe fn drain_physical_eoi(state: &mut State, vmcb: &Vmcb) -> bool {
    for _ in 0..256 {
        let highest = unsafe { native_irq::physical_highest_in_service() };
        match state.irq.next_eoi(highest) {
            Ok(None) => return true,
            Ok(Some(vector)) => {
                unsafe { native_irq::physical_eoi() };
                if state.irq.commit_eoi(vector).is_err() { break; }
            }
            Err(_) => break,
        }
    }
    let exit = vmcb.exit_snapshot();
    stop(state, exit.code, exit.rip, 0xf504, 0)
}

/// Only register operations deliberately preintercepted by the x2AVIC MSRPM.
/// APIC_BASE is a guest shadow; no guest access here executes a physical mode
/// change. Invalid architectural access prepares #GP; unsupported mode changes
/// retain the stopped instruction rather than inventing a guest fault.
unsafe fn handle_avic_msr(state: &mut State, vmcb: &mut Vmcb,
    frame: &mut GuestRegisters) -> bool {
    use crate::svm::exit::MsrInstruction;
    let exit = vmcb.exit_snapshot();
    let Some(profile) = state.avic else { return stop(state, exit.code, exit.rip, 0xf510, 0); };
    if vmcb.validate_native_x2avic(&profile).is_err()
        || vmcb.validate_external_interrupt_conflicts().is_err() {
        return stop(state, exit.code, exit.rip, 0xf510, 1);
    }
    let caps = state.capabilities.filter(|caps| caps.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_none() {
        match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
            Ok(bytes) => Some(bytes),
            Err(reason) if reason == terminal::FetchReadFailure::MemoryControlBusy as u16 => return retry_routing(state, vmcb),
            Err(reason) => return stop(state, exit.code, exit.rip, 0xf001, reason as u64),
        }
    } else { None };
    let evidence = match caps {
        Some(caps) => match MsrInstruction::hardware(exit, &caps) {
            Ok(evidence) => evidence,
            Err(_) => return stop(state, exit.code, exit.rip, 0xf510, 2),
        },
        None => MsrInstruction::Bytes(bytes.as_ref().unwrap()),
    };
    let Ok(next) = evidence.continuation(exit) else {
        return stop(state, exit.code, exit.rip, 0xf510, 2);
    };
    if !dispatch::native_startup_instruction_mode(vmcb, evidence.length())
        || vmcb.guest_rflags() & (1 << 8) != 0 {
        return stop(state, exit.code, exit.rip, 0xf510, 3);
    }
    let index = frame.rcx as u32;
    let write = exit.info1 == 1;
    if vmcb.bytes()[0x4cb] != 0 || (0x840..=0x8ff).contains(&index) {
        if vmcb.queue_native_x2avic_general_protection(&profile).is_err() {
            return stop(state, exit.code, exit.rip, 0xf510, 4);
        }
        state.pending_fault = true;
        return true;
    }
    let value = match (index, write) {
        (0x1b, false) => state.apic_base,
        (0x1b, true) => {
            let requested = vmcb.guest_rax() as u32 as u64 | ((frame.rdx as u32 as u64) << 32);
            if requested != state.apic_base {
                return stop(state, exit.code, exit.rip, 0xf511, requested);
            }
            vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
            vmcb.complete_native_instruction_state();
            return true;
        }
        (0x839, false) => unsafe { read_msr(0x839) },
        _ => return stop(state, exit.code, exit.rip, 0xf510, index as u64),
    };
    frame.rdx = value >> 32;
    vmcb.commit_emulated_instruction(value as u32 as u64, next);
    vmcb.complete_native_instruction_state();
    state.routing_retries = 0;
    true
}

/// AVIC writes in Table15-22 are traps: the backing write and guest RIP have
/// already committed. They must not pass through ordinary MSR completion.
unsafe fn handle_avic_exit(state: &mut State, vmcb: &mut Vmcb,
    _frame: &mut GuestRegisters) -> bool {
    let exit = vmcb.exit_snapshot();
    let Some(profile) = state.avic else { return stop(state, exit.code, exit.rip, 0xf520, 0); };
    if vmcb.validate_native_x2avic(&profile).is_err() {
        return stop(state, exit.code, exit.rip, 0xf520, 1);
    }
    match AvicExit::decode(exit.code, exit.info1, exit.info2) {
        Ok(AvicExit::IncompleteIpi { icr, reason: 0, .. }) => {
            let result = state.icr.as_mut().unwrap().route_x2avic_startup(icr,
                unsafe { mailboxes(state.count) }, |_| unsafe { notify_native_startup() });
            match result {
                Ok(()) => true,
                // The hardware instruction already completed. Never reenter
                // as an instruction retry or resend a partially delivered IPI.
                Err(_) => stop(state, exit.code, exit.rip, 0xf521, icr),
            }
        }
        Ok(AvicExit::NoAcceleration { offset: 0xb0, write: true, eoi_vector: Some(vector) }) => {
            let Ok(mut routes) = (unsafe { source_routes() }).try_lock() else {
                // The EOI instruction already completed; it cannot be retried.
                return stop(state, exit.code, exit.rip, 0xf523, vector as u64);
            };
            match routes.has_level(unsafe { ASSIGNED_APIC_ID } as u16, vector) {
                Ok(true) => {
                    let result = routes.complete_level(unsafe { ASSIGNED_APIC_ID } as u16,
                        vector, |eoi| unsafe { write_directed_eoi(state, eoi) });
                    return result.is_ok() || stop(state, exit.code, exit.rip, 0xf524, vector as u64);
                }
                Ok(false) => {},
                Err(_) => return stop(state, exit.code, exit.rip, 0xf523, vector as u64),
            }
            if state.irq.complete_level(vector).is_err() {
                return stop(state, exit.code, exit.rip, 0xf522, vector as u64);
            }
            unsafe { drain_physical_eoi(state, vmcb) }
        }
        Ok(AvicExit::NoAcceleration { offset, write: true, .. }) => {
            unsafe { apply_avic_register_backend(state, vmcb, offset) }
        }
        _ => stop(state, exit.code, exit.rip, 0xf520, exit.info2),
    }
}

/// Shared reverse routing has one excluded backing and per-image RW/NX aliases.
unsafe fn source_routes() -> &'static crate::svm::native_sources::SharedRoutes {
    unsafe { &*((ptr::addr_of!(image_start) as u64 + super::SOURCE_ROUTES_OFFSET)
        as *const crate::svm::native_sources::SharedRoutes) }
}

/// Platform-qualified directed EOI only, under the retained source-route guard.
/// APM2 5.4/7.8.5: private UC alias and local invalidation, IF/GIF clear. This
/// does not qualify a chipset register; the route publisher must do that before
/// enabling its source. No physical LAPIC ISR/EOI is involved.
unsafe fn write_directed_eoi(state: &State, eoi: crate::svm::native_sources::DirectedEoi)
    -> Result<(), ()>
{
    let Some(caps) = state.capabilities else { return Err(()); };
    let page = eoi.register & !4095;
    let policy = caps.address_policy();
    if eoi.register & 3 != 0 || policy.validate(page,4096,4096).is_err() { return Err(()); }
    let (pool, bytes) = unsafe { POOL };
    if page < pool+bytes && pool < page+4096 { return Err(()); }
    let pat = unsafe { read_msr(PAT) };
    let Some(uc) = (0..8).find(|i| (pat >> (i*8)) & 255 == 0) else { return Err(()); };
    let Some(mt) = (unsafe { native_mtrrs(policy.physical_bits()) }) else { return Err(()); };
    if !mt.terminal_page_is_uc(page,0) { return Err(()); }
    let window = ptr::addr_of!(image_start) as u64 + 0xfd000;
    let entry = unsafe { ptr::addr_of_mut!((*ptr::addr_of_mut!(TABLES)).0[3][((window>>12)&511) as usize]) };
    if unsafe { entry.read() } != 0 { return Err(()); }
    let flags = 3 | (1u64<<63) | ((uc&1)<<3) | ((uc&2)<<3) | ((uc&4)<<5);
    unsafe {
        entry.write(page|flags);
        asm!("invlpg [{}]",in(reg)window,options(nostack,preserves_flags));
        ((window+(eoi.register&4095)) as *mut u32).write_volatile(eoi.source_vector as u32);
        entry.write(0);
        asm!("invlpg [{}]",in(reg)window,options(nostack,preserves_flags));
    }
    Ok(())
}

/// Physical timer/LVT source backend. Guest register storage is the AVIC page;
/// the physical LAPIC remains host-owned and its TPR/EOI/ICR are never forwarded.
unsafe fn apply_avic_register_backend(state: &mut State, vmcb: &Vmcb, offset: u16) -> bool {
    let exit = vmcb.exit_snapshot();
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    let value = backing.read_register(offset).unwrap_or(u32::MAX);
    match offset {
        0xf0 => {
            // Physical capture must remain enabled even when the guest's
            // virtual LAPIC is disabled. Timer delivery follows guest SVR.
            let timer = backing.read_register(0x320).unwrap();
            let mask = if value & (1 << 8) == 0 { 1 << 16 } else { 0 };
            unsafe { write_msr(0x832, u64::from(timer | mask)); }
        }
        0x320 => {
            // Only native count-driven one-shot/periodic modes are admitted.
            if value & (3 << 17) > 1 << 17 {
                return stop(state, exit.code, exit.rip, 0xf530, value as u64);
            }
            let mask = if backing.read_register(0xf0).unwrap() & (1 << 8) == 0 { 1 << 16 } else { 0 };
            unsafe { write_msr(0x832, u64::from(value | mask)); }
        }
        0x380 | 0x3e0 => unsafe { write_msr(0x800 + u32::from(offset / 16), value as u64) },
        // Masked standard LVTs own no live source. Active LINT/performance/
        // thermal/error delivery needs a separately admitted source contract.
        0x330 | 0x340 | 0x350 | 0x360 | 0x370 if value & (1 << 16) != 0 => unsafe {
            write_msr(0x800 + u32::from(offset / 16), value as u64);
        },
        0x280 => {}, // ESR's backing access completed; no physical ESR owner.
        _ => return stop(state, exit.code, exit.rip, 0xf531, (u64::from(offset) << 32) | u64::from(value)),
    }
    true
}

/// Complete the reviewed fixed-MTRR control transaction on the owning CPU.
/// Runtime preparation retains every host object above1MiB. Shared routing
/// lock excludes low-RAM readers and other core-shared SYS_CFG writes.
unsafe fn handle_syscfg(state: &mut State, vmcb: &mut Vmcb, frame: &GuestRegisters) -> bool {
    use crate::svm::native_syscfg::{self, SyscfgInstruction, SyscfgPreparation};
    let exit = vmcb.exit_snapshot();
    let caps = state.capabilities.filter(|c| c.optional_features().nrip_save && vmcb.guest_in_64_bit_code());
    let bytes = if caps.is_some() { None } else {
        match unsafe { fetch_instruction(vmcb, state.startup_owned, state.count) } {
            Ok(bytes) => Some(bytes),
            Err(r) if r == terminal::FetchReadFailure::MemoryControlBusy as u16 => return retry_routing(state, vmcb),
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
    let result = native_syscfg::prepare(vmcb, frame, evidence, state.startup_owned,
        __cpuid_count(1, 0).eax, unsafe { PHYSICAL_BITS }, || unsafe { read_msr(SYS_CFG) });
    let failure = match result {
        Ok(SyscfgPreparation::GeneralProtectionPrepared) => {
            state.pending_fault = true; return true;
        }
        Ok(SyscfgPreparation::Write(prepared)) => {
            let current=prepared.current(); let requested=prepared.requested(); let delta=prepared.delta();
            unsafe { diagnostic_record(8,false,[exit.rip,current,requested,current,delta,0],1); }
            if let Some(requested) = prepared.write_value() {
                unsafe { write_msr(SYS_CFG, requested); }
                let observed = unsafe { read_msr(SYS_CFG) };
                if observed != requested {
                    drop(prepared);
                    let (tag, value) = terminal::syscfg_operands(0x85, true, requested, observed);
                    drop(routes);
                    return stop(state, exit.code, exit.rip, tag, value);
                }
            }
            prepared.commit(); state.msr = state.msr.saturating_add(1);
            unsafe { diagnostic_record(8,false,[exit.rip,current,requested,requested,delta,0],2); }
            state.routing_retries = 0; return true;
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
    stop(
        state,
        exit.code,
        exit.rip,
        0xf107,
        state.routing_retries as u64,
    )
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

/// APM2 15.21.8/Table15-12 and15.28: consume held INIT through private #SX,
/// with IF=0 throughout. No physical IRQ is acknowledged and no CR8/APIC state
/// is changed. Nonmaskable events opened by STGI still use the terminal host
/// gates. Counts may coalesce; only the mailbox owns guest startup commands.
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

/// Service bounded FIFO work on the destination only. AwaitSipi uses a bounded
/// stopped-host poll; it never enters a reset-vector guest or calls firmware.
/// This is a diagnostic iteration bound, not calibrated physical time.
unsafe fn service_startup(
    state: &mut State,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> Option<bool> {
    let mailbox = &unsafe { mailboxes(state.count) }[state.slot];
    let mut changed = false;
    for _ in 0..20_000_000 {
        if unsafe { terminal_requested(state) } {
            #[cfg(feature = "resident-runtime-test")]
            if state.startup == NativeStartupState::AwaitSipi {
                debug(b"resident-terminal await-sipi-peer="); hex(state.slot as u64); debug(b"\n");
            }
            return Some(false);
        }
        let Some(command) = mailbox.peek() else {
            if state.startup == NativeStartupState::Running {
                return changed.then_some(true);
            }
            core::hint::spin_loop();
            continue;
        };
        if state.cache_active { return startup_stage_stop(state, vmcb, 13); }
        if unsafe { acknowledge_init() }.is_none() {
            return startup_stage_stop(state, vmcb, 1);
        }
        // Serialize hardware destination changes and queue completion against
        // source selection/publication. Never hold this lease while waiting.
        let routes = match try_lock_routes(unsafe { mailboxes(state.count) }) {
            Ok(routes) => routes,
            Err(NativeIcrError::RoutingBusy) => {
                core::hint::spin_loop();
                continue;
            }
            Err(_) => return startup_stage_stop(state, vmcb, 2),
        };
        // Route lease -> core lease is the only nested order. E0 publication
        // uses the core lease alone. Hold this through the bounded local
        // startup commit so a sibling cannot begin E0 between check and reset.
        let cache_lease = if state.cache_observation.is_some() {
            let Some(lease) = (unsafe { cache::core(state) }).try_lock() else {
                drop(routes); core::hint::spin_loop(); continue;
            };
            if lease.phase != 0 { return startup_stage_stop(state, vmcb, 13); }
            Some(lease)
        } else { None };
        let mut target = NativeStartupTarget {
            vmcb,
            frame,
            state: &mut state.startup,
            signature: __cpuid_count(1, 0).eax,
        };
        let Some(profile) = state.avic else { return startup_stage_stop(state, vmcb, 14); };
        let effect = match target.validate_x2avic(command, &profile) {
            Ok(effect) => effect,
            Err(NativeIcrError::PendingState(error)) => {
                let exit = target.vmcb.exit_snapshot();
                let (tag, value) = terminal::startup_pending_failure(error, unsafe { ASSIGNED_APIC_ID });
                stop(state, exit.code, exit.rip, tag, value);
                return Some(false);
            }
            Err(_) => return startup_stage_stop(state, vmcb, 5),
        };
        // A live AVIC backing page can be written by another CPU's accelerated
        // IPI or the IOMMU. A route lock does not drain those hardware writers.
        // Until the global producer-quiescence owner supplies that proof, retain
        // the exact stopped state and command rather than racing a page reset.
        if effect == NativeStartupEffect::Init {
            drop(target);
            return startup_stage_stop(state, vmcb, 14);
        }
        if target.apply_x2avic(command, &profile).is_err() {
            return startup_stage_stop(state, vmcb, 5);
        }
        if effect == NativeStartupEffect::Started {
            debug(b"resident-guest-sipi cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b" vector=");
            if let NativeStartupCommand::Sipi(vector) = command {
                hex(vector as u64);
            }
            debug(b"\n");
            changed = true;
        } else if effect == NativeStartupEffect::Ignored {
            debug(b"resident-guest-sipi-ignored cpu=");
            hex(unsafe { ASSIGNED_APIC_ID } as u64);
            debug(b"\n");
        }
        if mailbox.complete(command).is_err() {
            return startup_stage_stop(state, vmcb, 8);
        }
        drop(cache_lease);
        drop(routes);
    }
    startup_stage_stop(state, vmcb, 9)
}

fn startup_stage_stop(state: &mut State, vmcb: &Vmcb, stage: u8) -> Option<bool> {
    let exit = vmcb.exit_snapshot();
    let value = u64::from(state.startup == NativeStartupState::AwaitSipi);
    let (tag, value) = terminal::startup_failure(7, stage, value, unsafe { ASSIGNED_APIC_ID });
    stop(state, exit.code, exit.rip, tag, value);
    Some(false)
}

/// Reset only the stopped target's guest-live breakpoint addresses.
/// APM2 rev3.44 Table14-1 p482,15.5.1/15.7: INIT resets DR0-3, which are
/// not part of the ordinary VMCB state switch. DR6/7 reset in the guest VMCB.
/// # Safety
/// The sole successful target-owned INIT commit calls this with IF/GIF clear,
/// after all fallible preparation, on the same nonmigrating guest CPU. Host
/// breakpoints/GD are disabled after VMEXIT; no external debugger or host DR
/// owner exists. Never call for a private wake, refused INIT or SIPI. Keep
/// this out-of-line symbol for the exact linked debug-write audit.
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
unsafe fn fetch_instruction(vmcb: &Vmcb, startup_owned: bool, count: usize) -> Result<[u8; 2], u16> {
    let mut reader = unsafe { GuestReader::new(vmcb, startup_owned, count) }.map_err(|e| e as u16)?;
    let result = if startup_owned {
        super::fetch::startup_instruction(
            vmcb,
            reader.width,
            reader.guest_pat,
            |address, bytes| unsafe { reader.read(address, bytes) },
        )
    } else {
        super::fetch::instruction(
            vmcb,
            reader.width,
            reader.guest_pat,
            |address, bytes| unsafe { reader.read(address, bytes) },
        )
    };
    result.map_err(|error| reader.failure.map_or_else(|| terminal::fetch_failure_code(error), |e| e as u16))
}

/// Existing temporary RAM alias shared by CPUID/MSR fetch and native MMIO
/// decoding. Owns one stopped CPU; each read removes its alias before returning.
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
    unsafe fn new(vmcb: &Vmcb, startup_owned: bool, count: usize) -> Result<Self, terminal::FetchReadFailure> {
        use terminal::FetchReadFailure as R;
        use crate::memory::address::AddressPolicy;
        let width = unsafe { PHYSICAL_BITS };
        let map = unsafe { core::slice::from_raw_parts(ptr::addr_of!(RAM).cast(), RAM_COUNT) };
        let map = ValidatedMemoryMap::new(map, width).map_err(|_| R::MemoryMap)?;
        let policy = AddressPolicy::new(
            width,
            EncryptionState::Unencrypted {
                encryption_bit: None,
            },
        )
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
                Err(_) => { self.failure = Some(R::MemoryControlBusy); return None; }
            }
        } else { None };
        let wb = if self.startup_owned && physical < 0x100000 {
            use crate::memory::mtrrs::{CAP_FIX, DEF_TYPE_DEFINED, DEF_TYPE_E, DEF_TYPE_FE};
            if self.mt.default & !DEF_TYPE_DEFINED != 0
                || self.mt.default & (DEF_TYPE_E | DEF_TYPE_FE) != DEF_TYPE_E | DEF_TYPE_FE
                || unsafe { read_msr(MTRR_CAP) } & CAP_FIX == 0
            {
                self.failure = Some(R::FixedMtrrControl);
                return None;
            }
            let Some((index, shift)) = crate::memory::mtrrs::Mtrrs::fixed_range_register(physical) else {
                self.failure = Some(R::FixedMtrrRange);
                return None;
            };
            if crate::memory::mtrrs::Tom2Default::supported_profile(__cpuid_count(1, 0).eax, self.width) {
                unsafe { native_fixed_page_is_wb(index, shift) }
            } else { (unsafe { read_msr(index) } >> shift) & 255 == 6 }
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

/// PPR57896 p202/43, APM2 7.9.1: bit19 is thread-private visibility;
/// bit18 is core-shared routing. Caller holds the shared route gate through
/// this sample and the eventual low RAM read. Fixed writes retain the native
/// trusted OS rendezvous contract. All executing monitor storage is >=1MiB.
unsafe fn native_fixed_page_is_wb(index: u32, shift: u8) -> bool {
    use crate::memory::mtrrs::Mtrrs;
    let original = unsafe { read_msr(SYS_CFG) };
    if original & !SYS_CFG_DEFINED != 0 || original & SYS_CFG_ENCRYPTION != 0
        || original & SYS_CFG_MTRR_FIX_DRAM_EN == 0 { return false; }
    let visible = original | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
    if visible != original { unsafe { write_msr(SYS_CFG, visible); } }
    let matched = unsafe { read_msr(SYS_CFG) } == visible;
    let byte = if matched { (unsafe { read_msr(index) } >> shift) as u8 } else { 0 };
    if visible != original { unsafe { write_msr(SYS_CFG, original); } }
    let restored = unsafe { read_msr(SYS_CFG) } == original;
    matched && restored && Mtrrs::native_fixed_page_is_wb(visible, byte)
}

/// Enumerated architectural MTRRs, captured boundedly on the owning CPU.
/// Shared by device UC admission and stopped instruction/MMIO reading.
unsafe fn native_mtrrs(width: u8) -> Option<crate::memory::mtrrs::Mtrrs> {
    crate::memory::mtrrs::Mtrrs::read(width, __cpuid_count(1, 0).eax,
        |index| unsafe { read_msr(index) }).ok()
}

/// Read-only diagnostic observation of the strictly admitted physical x2APIC.
#[cfg(feature = "resident-runtime-test")]
unsafe fn read_native_apic(offset: u16) -> u64 {
    unsafe { read_msr(0x800 + u32::from(offset >> 4)) }
}

/// Private notification only after the routing owner has proved every assigned
/// CPU ready and published a target queue. APM2 16.5/Table16-4: shorthand11
/// ignores destination width and excludes the source. R_INIT/#SX consumes every
/// hardware wake; CPUs with empty queues resume unchanged. No guest INIT is
/// inferred from a wake, and no guest interrupt is acknowledged here.
unsafe fn notify_native_startup() {
    unsafe { send_native_notification() };
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

/// Same owning CPU; either admitted VM_CR.R_INIT preserving every other bit,
/// validated native ICR, or reset-register values after quiescent INIT preflight.
/// APM2 rev3.44 15.30.1/16.11.2/16.13 and PPR57896 p215. No MSR may fault.
unsafe fn write_msr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32,
            in("edx") (value >> 32) as u32, options(nostack, preserves_flags));
    }
}

/// All accesses use the existing pool-excluded shared alias, initialized before
/// any arm call. No guest reference or original DXE pointer survives here.
unsafe fn terminal_control() -> &'static TerminalControl {
    unsafe { &*((ptr::addr_of!(image_start) as u64 + super::STARTUP_PAGE_OFFSET
        + terminal::CONTROL_OFFSET) as *const TerminalControl) }
}
unsafe fn terminal_requested(state: &State) -> bool {
    state.armed && terminal_enabled(state)
        && unsafe { terminal_control() }.ready(state.count)
        && unsafe { terminal_control() }.requested()
}
fn terminal_enabled(state: &State) -> bool {
    state.startup_owned && state.count >= 2
        && (state.terminal_endpoint.is_some() || cfg!(feature = "resident-runtime-test"))
}

/// Terminal-only: no route guard is held, no resume follows. APM2 Table15-10
/// holds external SMI/NMI/INIT while GIF=0. All CPUs acknowledge only in that
/// state and never reopen GIF afterward, excluding their firmware/config writes.
/// Reset/machine-check or a nonparticipating CPU can lose evidence; no write is
/// permitted for the legacy aggregate without the complete ack mask. The live
/// guarded per-CPU transport exports failure evidence independently of that mask.
/// Iteration caps are not time bounds.
unsafe fn terminal_finish(state: &mut State) {
    if !state.armed || !terminal_enabled(state)
        || state.terminal_endpoint.is_some_and(|endpoint| !endpoint.valid()) { return; }
    unsafe { diagnostics::flush_fault(); }
    let shared = unsafe { terminal_control() };
    if !shared.ready(state.count) { return; }
    let winner = state.stopped_valid && shared.claim(state.slot,state.count);
    if !shared.requested() { return; }
    if !shared.acknowledge(state.slot,state.count) { return; }
    unsafe { record_barrier(shared); }
    if !winner { return; }
    // The terminal request establishes sole spare-bank ownership; export before
    // notification/preflight can fail and without waiting for another CPU.
    unsafe { export_stop_context(state); }
    // The published ready gate follows every target's armed/guest ACK. The
    // dedicated terminal request is authoritative; no guest INIT is enqueued.
    let apic = unsafe { read_msr(0x1b) };
    // x2APIC has no software-polled ICR delivery status to wait for.
    if apic != state.apic_base || apic & 0xc00 != 0xc00
        || unsafe { read_msr(0xc0010114) } & 2 == 0
        || unsafe { mailboxes(state.count) }.iter().any(|m| !m.is_ready()) {
        shared.finish(2); unsafe { record_barrier(shared); } return;
    }
    // Same already admitted INIT-to-#SX wire operation as startup notification,
    // with a separate irreversible terminal publication instead of a queue.
    unsafe { send_native_notification() };
    for _ in 0..20_000_000 {
        if shared.all_acknowledged(state.count) {
            #[cfg(feature = "resident-runtime-test")]
            if state.terminal_endpoint.is_none() {
                debug(b"resident-terminal barrier=complete owner="); hex(state.slot as u64);
                debug(b" count="); hex(state.count as u64); debug(b" card=disabled\n");
                shared.finish(1); return;
            }
            let words = terminal::stop_words(state.slot,state.stopped,state.stopped_rip,
                state.stopped_info1,state.stopped_info2);
            let result = words.is_some_and(|words| unsafe { diagnostics::export_terminal(words) });
            shared.finish(if result { 1 } else { 4 });
            unsafe { record_barrier(shared); }
            return;
        }
        core::hint::spin_loop();
    }
    shared.finish(3);
    unsafe { record_barrier(shared); }
    debug(b"resident-terminal barrier=incomplete\n");
}

unsafe fn record_barrier(shared:&TerminalControl) {
    let mut context = shared.diagnostic_snapshot();
    context[5] = diagnostics::fault_status();
    unsafe { diagnostic_record(7,false,context,0); }
}

/// Stable raw stopped state; reading bytes does not change VMCB/GPRs. These are
/// software observations, not a claim that invalid-entry save fields are valid.
unsafe fn export_stop_context(state:&State) {
    let aux = (state.slot as u32)<<8 | (state.count as u32)<<24;
    unsafe {
        diagnostics::export_context(0,[state.stopped_rip,state.stopped,state.stopped_info1,
            state.stopped_info2,state.exits,diagnostics::fault_status()],aux);
        let bytes = ptr::addr_of!(VMCB).cast::<u8>();
        let read = |offset:usize| ptr::read_volatile(bytes.add(offset).cast::<u64>());
        diagnostics::export_context(1,[read(0x78),read(0x80),read(0x88),read(0xa8),
            read(0xc8),read(0x550)],aux|1);
        let frame = ptr::read_volatile(ptr::addr_of!(FRAME));
        diagnostics::export_context(2,[read(0x5f8),frame.rcx,frame.rdx,read(0x558),
            read(0x4d0),read(0x410)],aux|2|((ptr::read_volatile(bytes.add(0x4cb)) as u32)<<13));
        let count = EXIT_HISTORY_COUNT;
        for n in 0..count {
            let index = (EXIT_HISTORY_NEXT+5-count+n)%5;
            let context = ptr::addr_of!(EXIT_HISTORY).cast::<[u64;6]>().add(index).read();
            diagnostics::export_context(3+n,context,aux|3|((n as u32)<<16));
        }
    }
}

/// Same validated physical bus and already idle ICR as terminal_finish. The
/// all-ready target set has R_INIT/#SX installed. IF/GIF stay zero on sender.
unsafe fn send_native_notification() {
    unsafe {
        asm!("mfence", options(nostack,preserves_flags));
        write_msr(0x830, 0x000c_0500);
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
    // APM2 rev3.44 15.7.2-3 / 15.20: EXITINTINFO.V means delivery did not
    // finish, even for an event not injected by us. A bare NPF/INIT retry can
    // lose an acknowledged IRQ. Native delivery recovery is not implemented;
    // preserve the stopped state instead of entering without the event.
    if interrupted & (1 << 31) != 0 {
        let exit = vmcb.exit_snapshot();
        return stop(state, exit.code, exit.rip, 0xf10f, interrupted);
    }
    true
}

fn record_unexplained_stop(state: &mut State, resume: bool, exit: crate::svm::exit::ExitSnapshot) {
    if !resume && !state.stopped_valid {
        stop(state, exit.code, exit.rip, 0xf10e, exit.info1);
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
                unsafe { ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset), 8); }
            }
            let before = *vmcb.bytes();
            let mut state = INITIAL_STATE;
            state.pending_fault = true;
            assert!(!check_exit_event(&mut state, &mut vmcb));
            assert!(state.pending_fault && state.stopped_valid);
            assert_eq!((state.stopped, state.stopped_info1, state.stopped_info2),
                (code, if code == 0x400 { 0xf10c } else { 0xf110 }, interrupted));
            assert_eq!(*vmcb.bytes(), before);
        }
        let mut vmcb = Vmcb::new();
        let mut state = INITIAL_STATE;
        state.pending_fault = true;
        assert!(check_exit_event(&mut state, &mut vmcb));
        assert!(!state.pending_fault && !state.stopped_valid);
    }

    #[test]
    fn hardware_delivery_cannot_escape_through_an_unchanged_retry() {
        for code in [0x400_u64, 0x63] {
            let mut vmcb = Vmcb::new();
            let interrupted = 0x8000_0051_u64;
            for (offset, value) in [(0x70, code), (0x88, interrupted), (0x578, 0x1234)] {
                unsafe { ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset), 8); }
            }
            let before = *vmcb.bytes();
            let mut state = INITIAL_STATE;
            assert!(!check_exit_event(&mut state, &mut vmcb));
            assert!(!state.pending_fault && state.stopped_valid);
            assert_eq!((state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
                (code, 0x1234, 0xf10f, interrupted));
            assert_eq!(*vmcb.bytes(), before);
        }
    }

    #[test]
    fn shutdown_and_invalid_entry_do_not_interpret_poisoned_saved_event_or_rip() {
        for code in [0x7f_u64, u64::MAX] {
            let mut vmcb = Vmcb::new();
            for (offset, value) in [(0x70, code), (0x88, u64::MAX), (0x578, u64::MAX)] {
                unsafe { ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset), 8); }
            }
            let before = *vmcb.bytes();
            let mut state = INITIAL_STATE;
            assert!(!check_exit_event(&mut state, &mut vmcb));
            assert_eq!((state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
                (code, 0, 0xf110, 0));
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
        assert_eq!((state.stopped, state.stopped_rip, state.stopped_info1, state.stopped_info2),
            (exit.code, exit.rip, 0xf10e, exit.info1));
        state.stopped_info1 = 0xf400;
        state.stopped_info2 = 16;
        record_unexplained_stop(&mut state, false, exit);
        assert_eq!((state.stopped_info1, state.stopped_info2), (0xf400, 16));
        assert_eq!(*vmcb.bytes(), before);
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
            if let Some((event, context)) = raw.stop_record(ptr::addr_of!(VMCB) as u64, ASSIGNED_APIC_ID, info1, info2) {
                // Win the sticky first-fault bank with the immutable boundary;
                // keep event3 as the ordinary latest stopped-state record.
                diagnostic_record(event, true, context, info1 as u32);
            }
        }
        diagnostic_record(3,true,[rip,code,info1,info2,state.exits,0],0);
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
fn hex(value: u64) {
    for shift in (0..16).rev() {
        debug(&[b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]]);
    }
}
