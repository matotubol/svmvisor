//! Disposable TCG-only integration boundary. No firmware entry point.
#![no_std]
mod clock;
#[cfg(feature = "amd-cpu-model")]
mod amd_cpu;
#[cfg(feature = "concurrent-smp")]
mod concurrent;
mod continuation;
mod event_overlap;
mod exceptions;
mod execution;
#[cfg(feature = "io-intercept")]
mod io_intercept;
mod host;
mod host_lapic;
mod host_memory;
mod host_smp;
mod interrupted_delivery;
mod interrupts;
mod memory;
mod multicore;
mod ownership;
mod preemption;
mod scheduled_timer;
mod session;
mod svr;
mod timing;
mod x2apic;
mod xstate;
use core::{
    arch::{asm, x86_64::__cpuid},
    panic::PanicInfo,
    ptr,
};
use svmvisor_hypervisor::{
    address::EncryptionState,
    capabilities::{CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures},
    vmcb::Vmcb,
};

unsafe extern "C" {
    static host_stack_guard: u8;
    static host_df_guard: u8;
    static mut vmcb: Vmcb;
    static guest_entry: u8;
    static guest_end: u8;
}

fn print(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn hex(value: u64) {
    for shift in (0..16).rev() {
        let digit = ((value >> (shift * 4)) & 15) as usize;
        let byte = b"0123456789abcdef"[digit];
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
    print("\n");
}
fn finish(pass: bool) -> ! {
    print(if pass {
        "PASS rust-dispatch\n"
    } else {
        "FAIL rust-dispatch\n"
    });
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4u16, in("eax") if pass { 0x10u32 } else { 0x11u32 }, options(nomem, nostack));
    }
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        print(location.file());
        print(":line=");
        hex(location.line() as u64);
    }
    finish(false)
}

// Only this disposable harness fills fields not yet modeled by the safe core.
// The storage is aligned, identity mapped, exclusively owned and stopped.
unsafe fn field<const N: usize>(control: *mut Vmcb, offset: usize, bytes: [u8; N]) {
    assert!(offset + N <= 4096);
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), control.cast::<u8>().add(offset), N);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn harness_main() -> ! {
    print("rust-core\n");
    let ownership = unsafe { ownership::capture() };
    unsafe {
        host::install().unwrap();
    }
    print("host-idt-installed\n");
    unsafe {
        host_memory::install();
    }
    print("host-mappings-restricted\n");
    if let Some(record) = ownership.as_ref() {
        ownership::verify_private(record);
    }
    // Runner selects an AMD TCG CPU with its hypervisor CPUID bit disabled.
    // Unencrypted RAM is an explicit emulator fixture, not physical evidence.
    let (vendor, svm, features, width, basic) = {
        (
            __cpuid(0),
            __cpuid(0x80000001),
            __cpuid(0x8000000a),
            __cpuid(0x80000008),
            __cpuid(1),
        )
    };
    let mut name = [0u8; 12];
    name[..4].copy_from_slice(&vendor.ebx.to_le_bytes());
    name[4..8].copy_from_slice(&vendor.edx.to_le_bytes());
    name[8..].copy_from_slice(&vendor.ecx.to_le_bytes());
    let vm_cr: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0xc0010114u32, out("eax") vm_cr, out("edx") _, options(nomem, nostack));
    }
    let flag = |set| {
        if set {
            EvidenceFlag::Set
        } else {
            EvidenceFlag::Clear
        }
    };
    let caps = CapabilityEvidence {
        vendor: if name == *b"AuthenticAMD" {
            CpuVendor::Amd
        } else {
            CpuVendor::Other
        },
        svm: flag(svm.ecx & 4 != 0),
        nested_paging: flag(features.edx & 1 != 0),
        svm_revision: Some(features.eax as u8),
        asid_count: Some(features.ebx),
        physical_address_bits: Some(width.eax as u8),
        vm_cr_svmdis: flag(vm_cr & 16 != 0),
        hypervisor_present: flag(basic.ecx & (1 << 31) != 0),
        encryption: EncryptionState::Unencrypted {
            encryption_bit: None,
        },
        optional: OptionalFeatures {
            nrip_save: features.edx & 8 != 0,
            ..OptionalFeatures::default()
        },
    }
    .validate()
    .unwrap();
    assert!(svm.edx & (1 << 20) != 0);
    #[cfg(feature = "io-intercept")]
    unsafe {
        assert!(ownership.is_none(), "I/O fixture is disposable flat Multiboot only");
        let clock = clock::State::capture();
        let state = xstate::State::install();
        io_intercept::run(&caps, &state, &clock);
        finish(true);
    }
    #[cfg(feature = "concurrent-smp")]
    unsafe {
        #[cfg(feature = "uefi-smp")]
        if ownership.as_ref().and_then(|o|o.smp()).is_none() {
            print("REFUSE uefi-smp-missing-ownership\n"); finish(false);
        }
        let clock = clock::State::capture();
        let state = xstate::State::install();
        #[cfg(feature = "amd-cpu-model")]
        amd_cpu::run(&caps, &state, &clock);
        #[cfg(not(feature = "amd-cpu-model"))]
        concurrent::run(ownership.as_ref(), &caps, &state, &clock);
        finish(true);
    }
    #[cfg(not(any(feature = "concurrent-smp", feature = "io-intercept")))]
    {
    let policy = caps.address_policy();
    let code_start = ptr::addr_of!(guest_entry) as usize;
    let code_len = ptr::addr_of!(guest_end) as usize - code_start;
    let code = unsafe { core::slice::from_raw_parts(code_start as *const u8, code_len) };
    let prepared = unsafe {
        memory::prepare(
            policy,
            code,
            ownership.as_ref(),
            svmvisor_hypervisor::npt::NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
    };
    print("checked-mappings-installed\n");
    let control = ptr::addr_of_mut!(vmcb);
    unsafe {
        let clock = clock::State::capture();
        let state = xstate::State::install();
        session::run_sessions(
            control, policy, &prepared, code, code_start, &caps, &state, &clock,
        );
        exceptions::run(control, &prepared, code_start, &caps, &state, &clock);
        interrupts::run(control, &prepared, code, code_start, &caps, &state, &clock);
        interrupts::run_controller(control, &prepared, code, code_start, &caps, &state, &clock);
        x2apic::run(control, &prepared, code, code_start, &caps, &state, &clock);
        timing::run(control, &prepared, code, code_start, &caps, &state, &clock);
        if let Some(record) = ownership.as_ref() {
            continuation::run(control, record, &caps, &state, &clock);
        }
        // All prior guest-code and mapping users have returned. Retire their
        // provenance token before rebuilding the same owned pages; each next
        // VMCB initialization requests an ASID/TLB flush.
        drop(prepared);
        scheduled_timer::run(control, ownership.as_ref(), &caps, &state, &clock);
        preemption::run(control, ownership.as_ref(), &caps, &state, &clock);
        event_overlap::run(control, ownership.as_ref(), &caps, &state, &clock);
        interrupted_delivery::run(control, ownership.as_ref(), &caps, &state, &clock);
        multicore::run(ownership.as_ref(), &caps, &state, &clock);
    }
    if cfg!(feature = "double-fault") {
        unsafe {
            host::double_fault_probe();
        }
    }
    if cfg!(feature = "stack-overflow") {
        print("host-stack-overflow-probe\n");
        unsafe {
            host::stack_overflow_probe();
        }
    }
    if cfg!(feature = "df-guard") {
        print("host-guard-target=");
        hex(ptr::addr_of!(host_df_guard) as u64);
        unsafe {
            asm!("mov rax, [{address}]", address = in(reg) ptr::addr_of!(host_df_guard), out("rax") _, options(nostack));
        }
        finish(false);
    }
    if cfg!(feature = "host-guard") {
        print("host-guard-target=");
        hex(ptr::addr_of!(host_stack_guard) as u64);
        unsafe {
            asm!("mov rax, [{address}]", address = in(reg) ptr::addr_of!(host_stack_guard), out("rax") _, options(nostack));
        }
        finish(false);
    }
    if cfg!(feature = "host-write-protect") {
        print("host-write-target=");
        hex(ptr::addr_of!(guest_entry) as u64);
        // This store must fault before changing host text.
        unsafe {
            asm!("mov byte ptr [{address}], 0", address = in(reg) ptr::addr_of!(guest_entry), options(nostack));
        }
        finish(false);
    }
    if cfg!(feature = "host-fault") {
        unsafe {
            host::host_fault_probe();
        }
    }
    finish(true)
    }
}
