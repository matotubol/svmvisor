//! Disposable guest witness; never included in a delivered driver.
//! Exercises fixed no-SVM policy, ignored RO writes and actual #GP retry.
use super::*;

#[repr(C, align(4096))]
struct Idt([u8; 4096]);
static mut IDT: Idt = Idt([0; 4096]);
unsafe extern "efiapi" {
    fn svmvisor_fixture_vmcr_gp();
    fn svmvisor_fixture_vmcr_mbz() -> u64;
}

fn read() -> u64 {
    let (low, high): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") 0xc0010114u32, out("eax") low, out("edx") high,
        options(nostack, nomem, preserves_flags)); }
    u64::from(low) | (u64::from(high) << 32)
}

pub(super) fn run() {
    marker("native-vmcr-before\n");
    require(__cpuid(0x80000001).ecx & (1 << 2) == 0, "vmcr-no-svm-cpuid");
    require(__cpuid(0x8000000a).edx & (1 << 2) == 0, "vmcr-no-svml-cpuid");
    require(read() == 0x10, "vmcr-initial-policy");
    for value in [0u32, 8, 16, 24] {
        unsafe { asm!("wrmsr", in("ecx") 0xc0010114u32, in("eax") value, in("edx") 0u32,
            options(nostack, nomem, preserves_flags)); }
        require(read() == 0x10, "vmcr-readonly-policy");
    }
    marker("native-vmcr-readonly-pass\n");
    // Copy the existing guest IDT and replace only #GP while local IF is clear.
    // APM2 long-mode exception entry supplies error code then RIP; the tiny
    // assembly gate accepts only our exact two-byte WRMSR and zero error code.
    let mut old = [0u8; 10];
    let (flags, cs): (u64, u16);
    unsafe {
        asm!("pushfq", "pop {}", "cli", out(reg) flags);
        asm!("sidt [{}]", in(reg) old.as_mut_ptr(), options(nostack, preserves_flags));
        asm!("mov {:x}, cs", out(reg) cs, options(nostack, preserves_flags));
    }
    let length = usize::from(u16::from_le_bytes(old[..2].try_into().unwrap())) + 1;
    require((224..=4096).contains(&length), "vmcr-fixture-idt-size");
    let base = u64::from_le_bytes(old[2..].try_into().unwrap());
    let next = core::ptr::addr_of_mut!(IDT).cast::<u8>();
    let mut gate = [0u8; 16];
    let handler = svmvisor_fixture_vmcr_gp as *const () as u64;
    gate[..2].copy_from_slice(&(handler as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&cs.to_le_bytes());
    gate[5] = 0x8e;
    gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
    gate[8..12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
    let mut descriptor = old;
    descriptor[2..].copy_from_slice(&(next as u64).to_le_bytes());
    let count;
    unsafe {
        core::ptr::copy_nonoverlapping(base as *const u8, next, length);
        core::ptr::copy_nonoverlapping(gate.as_ptr(), next.add(13 * 16), 16);
        asm!("lidt [{}]", in(reg) descriptor.as_ptr(), options(nostack, preserves_flags));
        count = svmvisor_fixture_vmcr_mbz();
        asm!("lidt [{}]", "push {}", "popfq", in(reg) old.as_ptr(), in(reg) flags);
    }
    require(count == 1, "vmcr-mbz-did-not-fault-once");
    require(read() == 0x10, "vmcr-fault-changed-policy");
    marker("native-vmcr-gp-retry-pass\n");
}
