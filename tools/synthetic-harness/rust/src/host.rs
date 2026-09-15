//! Disposable emulator host tables and terminal fault reporting.
//! This is not a native host transition or a recovery/resume implementation.

use core::{arch::asm, ptr};
use svmvisor_hypervisor::host_descriptors::{HostDescriptorError, HostDescriptorRequest};

#[repr(C, align(4096))]
struct TablePage([u8; 4096]);

static mut HOST_GDT: TablePage = TablePage([0; 4096]);
static mut HOST_TSS: TablePage = TablePage([0; 4096]);
static mut HOST_IDT: TablePage = TablePage([0; 4096]);
#[cfg(feature = "concurrent-smp")]
static mut AP_GDT: TablePage = TablePage([0; 4096]);
#[cfg(feature = "concurrent-smp")]
static mut AP_TSS: TablePage = TablePage([0; 4096]);
#[cfg(feature = "concurrent-smp")]
static mut AP_IDT: TablePage = TablePage([0; 4096]);

/// Replace a private host interrupt gate, retaining its exact prior bytes.
/// # Safety
/// IF=0/GIF=1, installed private IDT, no handler executing. The replacement
/// handler must preserve all interrupted state. APM2 rev3.44 8.9/16.4.
unsafe fn replace_gate(vector: usize, address: u64, replacement: Option<[u8; 16]>) -> [u8; 16] {
    #[cfg(feature = "concurrent-smp")]
    let idt = if crate::host_smp::current_cpu() == 0 {
        ptr::addr_of_mut!(HOST_IDT)
    } else {
        ptr::addr_of_mut!(AP_IDT)
    };
    #[cfg(not(feature = "concurrent-smp"))]
    let idt = ptr::addr_of_mut!(HOST_IDT);
    let gate = unsafe { idt.cast::<[u8; 16]>().add(vector) };
    let old = unsafe { ptr::read_volatile(gate) };
    let new = replacement.unwrap_or_else(|| {
        let mut bytes = [0; 16];
        bytes[..2].copy_from_slice(&(address as u16).to_le_bytes());
        bytes[2..4].copy_from_slice(&8u16.to_le_bytes());
        bytes[5] = 0x8e;
        bytes[6..8].copy_from_slice(&((address >> 16) as u16).to_le_bytes());
        bytes[8..12].copy_from_slice(&((address >> 32) as u32).to_le_bytes());
        bytes
    });
    unsafe {
        ptr::write_volatile(gate, new);
    }
    old
}
/// # Safety
/// Same private-IDT contract as replace_gate; F1 is exclusively host-owned.
#[cfg(feature = "concurrent-smp")]
pub unsafe fn replace_ipi_gate(replacement: Option<[u8; 16]>) -> [u8; 16] {
    unsafe extern "C" {
        static host_smp_ipi_handler: u8;
    }
    unsafe {
        replace_gate(
            0xf1,
            ptr::addr_of!(host_smp_ipi_handler) as u64,
            replacement,
        )
    }
}
/// # Safety
/// Same private-IDT contract as replace_gate; F0 is exclusively host-timer-owned.
/// Restore the returned bytes before releasing the timer. APM2 8.9/16.4.
pub unsafe fn replace_timer_gate(replacement: Option<[u8; 16]>, concurrent: bool) -> [u8; 16] {
    unsafe extern "C" {
        static host_preempt_handler: u8;
    }
    let mut address = ptr::addr_of!(host_preempt_handler) as u64;
    #[cfg(feature = "concurrent-smp")]
    if concurrent {
        unsafe extern "C" {
            static host_smp_timer_handler: u8;
        }
        address = ptr::addr_of!(host_smp_timer_handler) as u64;
    }
    #[cfg(not(feature = "concurrent-smp"))]
    {
        assert!(!concurrent);
        let _ = &mut address;
    }
    unsafe { replace_gate(0xf0, address, replacement) }
}

unsafe extern "C" {
    static host_fault_handlers: [u64; 256];
    static host_stack_top: u8;
    static host_double_fault_stack_end: u8;
    pub fn host_fault_probe() -> !;
    fn host_double_fault_trigger() -> !;
    fn host_stack_overflow_trigger() -> !;
}

/// Exhaust the ordinary host stack against its unmapped low guard, requiring
/// #DF to switch to the separately guarded IST1 stack. Requires installed host
/// tables, unmapped stack guards, IF clear and exclusive single-CPU execution.
/// This terminal emulator probe destroys the current stack and cannot return.
pub unsafe fn stack_overflow_probe() -> ! {
    unsafe { host_stack_overflow_trigger() }
}

/// Cause a genuine #GP -> #NP during delivery -> #DF chain in the disposable
/// emulator. Requires successful `install()`, IF clear, one CPU and exclusive
/// IDT ownership. The modified #GP gate is never restored: this probe exits.
pub unsafe fn double_fault_probe() -> ! {
    unsafe {
        let attributes = ptr::addr_of_mut!(HOST_IDT).cast::<u8>().add(13 * 16 + 5);
        let previous = ptr::read_volatile(attributes);
        ptr::write_volatile(attributes, previous & !0x80);
        host_double_fault_trigger()
    }
}

/// Install tables only in the single-CPU, identity-mapped emulator bootstrap.
/// Caller must already have IF clear, CS=8 with matching flat long-mode code,
/// CET disabled, writable static RAM and a working stack. Call exactly once,
/// before guest entry and before enabling any asynchronous event delivery.
/// Static backing remains valid for the lifetime of this disposable machine.
pub unsafe fn install() -> Result<(), HostDescriptorError> {
    unsafe {
        install_tables(
            ptr::addr_of_mut!(HOST_GDT).cast(),
            ptr::addr_of_mut!(HOST_TSS).cast(),
            ptr::addr_of_mut!(HOST_IDT).cast(),
            ptr::addr_of!(host_stack_top) as u64,
            ptr::addr_of!(host_double_fault_stack_end) as u64,
        )
    }
}

/// # Safety
/// Emulator AP only, private guarded stack, IF=0, identity paging and CS=8.
/// Called once before AP guest entry; APM2 4.8/8.9 and TSS layout in chapter12.
#[cfg(feature = "concurrent-smp")]
pub unsafe fn install_ap() -> Result<(), HostDescriptorError> {
    unsafe extern "C" {
        static host_ap_stack_top: u8;
        static host_ap_df_stack_end: u8;
    }
    unsafe {
        install_tables(
            ptr::addr_of_mut!(AP_GDT).cast(),
            ptr::addr_of_mut!(AP_TSS).cast(),
            ptr::addr_of_mut!(AP_IDT).cast(),
            ptr::addr_of!(host_ap_stack_top) as u64,
            ptr::addr_of!(host_ap_df_stack_end) as u64,
        )
    }
}
// Common descriptor owner with concrete BSP and feature-gated AP callers.
unsafe fn install_tables(
    gdt: *mut u8,
    tss: *mut u8,
    idt: *mut u8,
    rsp0: u64,
    ist1: u64,
) -> Result<(), HostDescriptorError> {
    let handlers = unsafe { ptr::read(ptr::addr_of!(host_fault_handlers)) };
    let image = HostDescriptorRequest {
        gdt_base: gdt as u64,
        tss_base: tss as u64,
        idt_base: idt as u64,
        rsp0,
        ist1,
        handlers,
    }
    .validate()?;
    unsafe {
        ptr::copy_nonoverlapping(image.gdt().as_ptr(), gdt, image.gdt().len());
        ptr::copy_nonoverlapping(image.tss().as_ptr(), tss, image.tss().len());
        ptr::copy_nonoverlapping(image.idt().as_ptr(), idt, image.idt().len());
    }
    let mut gdtr = [0_u8; 10];
    gdtr[..2].copy_from_slice(&image.gdtr().limit.to_le_bytes());
    gdtr[2..].copy_from_slice(&image.gdtr().base.to_le_bytes());
    let mut idtr = [0_u8; 10];
    idtr[..2].copy_from_slice(&image.idtr().limit.to_le_bytes());
    idtr[2..].copy_from_slice(&image.idtr().base.to_le_bytes());
    unsafe {
        asm!(
            "lgdt [{gdt}]",
            "mov ax, 16",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov ax, 24",
            "ltr ax",
            "lidt [{idt}]",
            gdt = in(reg) gdtr.as_ptr(),
            idt = in(reg) idtr.as_ptr(),
            out("ax") _,
            options(nostack),
        );
    }
    Ok(())
}
