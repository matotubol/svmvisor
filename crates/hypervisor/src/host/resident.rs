//! Native callback and resident assembly seam, AMD APM2 rev3.44 15.5/15.7.
//!
//! Pointer fields are inert ABI operands, not admission or allocation evidence.
//! The callback object stays in the original DXE image; runtime.S and its entire
//! dispatch call graph must reside in independently retained monitor memory.

pub use crate::guest::continuation::NATIVE_BOOTSTRAP_ACK as BOOTSTRAP_ACK;
pub mod fetch;
pub mod terminal;

/// Nonreturning resident entry uses the Windows x64 ABI, also when linked into
/// an ELF payload. The dispatcher uses that same ABI and returns AL=1 to resume.
/// No borrowed reference may alias guest writes or survive guest execution.
pub type Dispatch = unsafe extern "win64" fn(*mut BridgeContext) -> bool;
pub type Enter = unsafe extern "win64" fn(*mut BridgeContext) -> !;

/// One physical CPU and one guest. All fields and referenced storage remain
/// resident, host-accessible and exclusively owned until reset. Descriptor
/// pointers identify packed ten-byte GDTR/IDTR images; selectors are GDT CPL0.
/// The host TSS descriptor must be available before the one-time LTR.
///
/// # Entry safety contract
/// Native CPL0 long64, IF=0, admitted AMD SVM enabled and guest auxiliary state
/// already captured/composed into guest_vmcb. The guest/native root is retained
/// in that VMCB; host_cr3 is a different, privately owned root. Current and new
/// host roots map the complete runtime transition instructions and context.
/// Every host pointer remains mapped after switching CR3. Host stack is aligned,
/// bounded, disjoint from guest stack/backing, and large enough for dispatch.
/// VMCB/HSAVE/auxiliary pages are distinct aligned WB physical operands; their
/// virtual aliases and the 112-byte GuestRegisters frame are prevalidated.
///
/// Guest x87/MMX/SIMD and XCR0 remain live on this CPU. The complete persistent
/// host call graph must contain no x87/MMX/SIMD, XSAVE/XRSTOR, XSETBV, debug
/// register mutation outside the successful target-owned INIT reset, or migration.
/// Compiler target settings alone are not
/// proof: linked-image instruction audit is mandatory. DR0–3 and clock MSRs
/// remain guest-owned. The audited INIT commit clears live DR0-3 and resets
/// VMCB DR6/7; ordinary exits and private notifications leave them untouched.
/// Clock MSRs are untouched by host. Admit disabled hardware breakpoints
/// and debug recording. All guest GPRs are captured before host scratch use.
/// The dispatcher may update only validated stopped guest state and keeps
/// IF clear throughout and GIF clear except the opt-in redirected-INIT window.
/// VM_CR.R_INIT and a private #SX(error1) gate own that notification. Native
/// guest IF/CR8 and physical LAPIC state own ordinary IRQ delivery unchanged.
/// Host descriptors remain stable. No firmware calls,
/// allocation, unwinding, unbounded work, or resumable synchronous transport.
/// The optional terminal exporter runs only after guards drop and every admitted
/// CPU irreversibly acknowledges the shared stop request with IF/GIF clear.
/// False dispatch is terminal; no post-entry native restoration is provided.
#[repr(C, align(16))]
pub struct BridgeContext {
    pub host_stack_top: u64,
    pub host_cr3: u64,
    pub guest_vmcb_pa: u64,
    pub guest_vmcb_va: u64,
    pub host_extra_pa: u64,
    pub guest_frame_va: u64,
    pub hsave_pa: u64,
    pub host_gdtr_va: u64,
    pub host_idtr_va: u64,
    pub host_tr_selector: u64,
    pub host_data_selector: u64,
    pub host_code_selector: u64,
    pub dispatch: Dispatch,
    pub owner_context: u64,
    /// Reserved ABI slot; zero. Native ordinary interrupts are never host-owned.
    pub reserved: u64,
}

const _: () = {
    assert!(core::mem::size_of::<BridgeContext>() == 128);
    assert!(core::mem::align_of::<BridgeContext>() == 16);
    assert!(core::mem::offset_of!(BridgeContext, host_stack_top) == 0);
    assert!(core::mem::offset_of!(BridgeContext, host_cr3) == 8);
    assert!(core::mem::offset_of!(BridgeContext, guest_vmcb_pa) == 16);
    assert!(core::mem::offset_of!(BridgeContext, guest_vmcb_va) == 24);
    assert!(core::mem::offset_of!(BridgeContext, host_extra_pa) == 32);
    assert!(core::mem::offset_of!(BridgeContext, guest_frame_va) == 40);
    assert!(core::mem::offset_of!(BridgeContext, hsave_pa) == 48);
    assert!(core::mem::offset_of!(BridgeContext, host_gdtr_va) == 56);
    assert!(core::mem::offset_of!(BridgeContext, host_idtr_va) == 64);
    assert!(core::mem::offset_of!(BridgeContext, host_tr_selector) == 72);
    assert!(core::mem::offset_of!(BridgeContext, host_data_selector) == 80);
    assert!(core::mem::offset_of!(BridgeContext, host_code_selector) == 88);
    assert!(core::mem::offset_of!(BridgeContext, dispatch) == 96);
    assert!(core::mem::offset_of!(BridgeContext, owner_context) == 104);
    assert!(core::mem::offset_of!(BridgeContext, reserved) == 112);
};

unsafe extern "efiapi" {
    pub fn svmvisor_resident_callback(
        event: *mut core::ffi::c_void,
        context: *mut core::ffi::c_void,
    );
}
unsafe extern "C" {
    pub static svmvisor_resident_guest_resume: u8;
    pub static svmvisor_resident_guest_ack: u8;
    pub static svmvisor_resident_guest_after_ack: u8;
}

/// Numeric directory returned by the separately loaded raw runtime. It owns no
/// references into firmware. All storage addresses identify aligned, disjoint
/// objects within the declared arena; the delivery adapter verifies containment
/// and actual mappings before use. Function addresses are linked runtime code.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ResidentDirectory {
    pub version: u64,
    pub arena_base: u64,
    pub arena_bytes: u64,
    pub context: u64,
    pub vmcb: u64,
    pub auxiliary: u64,
    pub registers: u64,
    pub npt: u64,
    pub arm: u64,
    pub enter: u64,
    pub text_end: u64,
    pub data_start: u64,
    pub memory_end: u64,
    pub pool_base: u64,
    pub pool_bytes: u64,
    pub cpu_slot: u64,
    pub apic_id: u64,
    pub reserved: [u64; 3],
}
pub const DIRECTORY_VERSION: u64 = 8;
/// Reserved shared transport page, wholly inside the guest-excluded pool.
pub const STARTUP_PAGE_OFFSET: u64 = 0xfe000;
/// Three immutable initial-cache observation pages, shared inside the excluded
/// pool. The adjacent fb000/fc000 space and LAPIC/startup/scratch aliases remain
/// separate; linked runtime data must end before this range.
pub const CACHE_CAPTURE_OFFSET: u64 = 0xf8000;
pub const CACHE_OWNER_OFFSET: u64 = 0xf5000;
pub const MAX_RESIDENT_CPUS: usize = 32;
/// One 1MiB image per dense slot; larger pools are 2MiB aligned below1GiB.
/// Numeric layout checks are not allocation, caching or CPU ownership proof.
pub fn valid_pool_slot(
    base: u64,
    pool_base: u64,
    pool_bytes: u64,
    slot: u64,
    apic_id: u64,
) -> bool {
    let Some(end) = pool_base.checked_add(pool_bytes) else {
        return false;
    };
    pool_base >= 0x100000
        && pool_base & 4095 == 0
        && end <= 0x40000000
        && pool_bytes >= 0x100000
        && pool_bytes <= MAX_RESIDENT_CPUS as u64 * 0x100000
        && pool_bytes & 0xfffff == 0
        && slot < pool_bytes / 0x100000
        && apic_id <= 255
        && (pool_bytes == 0x100000 || pool_base & 0x1fffff == 0)
        && base == pool_base + slot * 0x100000
        && base >> 21 == (base + 0xfffff) >> 21
}
pub type PrepareRuntime =
    unsafe extern "win64" fn(u64, *mut ResidentDirectory, u64, u64, u64, u64) -> u64;
/// The optional penultimate pointer supplies the BSP's canonical ICR before physical
/// bootstrap overwrites its readable command. Null selects current native ICR.
/// A nonnull operand is allowed only for startup ownership and identifies an
/// aligned immutable u64 in the caller's validated current mapping for this
/// call. The callee copies the value before guest entry and retains no pointer.
/// xAPIC destination is canonicalized to bits63:32; command remains bits31:0.
/// The final optional pointer identifies one aligned immutable TerminalEndpoint
/// in the caller's validated current mapping for this call. It is copied before
/// entry; no pointer is retained. Null disables production terminal export.
pub type ArmRuntime = unsafe extern "win64" fn(
    u64,
    u64,
    u64,
    u64,
    *const crate::boot::memory::MemoryDescriptor,
    usize,
    *const u32,
    usize,
    bool,
    *const u64,
    *const terminal::TerminalEndpoint,
) -> u64;
const _: () = {
    assert!(core::mem::size_of::<ResidentDirectory>() == 160);
    assert!(core::mem::offset_of!(ResidentDirectory, context) == 24);
    assert!(core::mem::offset_of!(ResidentDirectory, arm) == 64);
    assert!(core::mem::offset_of!(ResidentDirectory, pool_base) == 104);
    assert!(core::mem::offset_of!(ResidentDirectory, reserved) == 136);
};

#[cfg(feature = "resident-runtime")]
mod runtime;
#[cfg(feature = "resident-runtime")]
pub use runtime::prepare;
