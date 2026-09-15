//! Executed guest contract; only part of the disposable emulator fixture.
use super::*;

#[repr(C, align(4096))]
struct Idt([u8; 4096]);
static mut IDT: Idt = Idt([0; 4096]);
unsafe extern "efiapi" {
    fn svmvisor_fixture_apic_gp();
    fn svmvisor_fixture_apic_fault(index: u32, write: u32) -> u64;
}

fn read_msr(index: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") index, out("eax") low, out("edx") high,
        options(nostack, nomem, preserves_flags)); }
    u64::from(low) | (u64::from(high) << 32)
}

pub(super) fn visibility() {
    require(__cpuid(0x80000001).ecx & (1 << 3) == 0, "apic-contract-cpuid-extension");
    let x2 = read_msr(0x1b) & 0x400 != 0;
    let version = if x2 { read_msr(0x803) as u32 } else {
        read_apic(0x30)
    };
    require(version & 0x80000000 == 0, "apic-contract-version-extension");
    if !x2 {
        let dfr = read_apic(0xe0);
        require(dfr & 0x0fffffff == 0x0fffffff, "apic-contract-dfr-reserved");
    }
}

fn read_apic(offset: usize) -> u32 {
    // Keep the fixture within the supported DWORD MOV MMIO contract; LLVM
    // may fold a Rust volatile read plus mask into a memory TEST instruction.
    let value: u32;
    unsafe { asm!("mov {value:e}, dword ptr [{address}]", value = out(reg) value,
        address = in(reg) (0xfee00000usize + offset), options(nostack, readonly, preserves_flags)); }
    value
}

pub(super) fn run() {
    marker("native-apic-contract-before\n");
    visibility();
    // Like the VM_CR fixture, install a temporary #GP gate before virtual-map
    // reclamation. Accept only our two exact RDMSR/WRMSR sites and error code0.
    let mut old = [0u8; 10];
    let (flags, cs): (u64, u16);
    unsafe {
        asm!("pushfq", "pop {}", "cli", out(reg) flags);
        asm!("sidt [{}]", in(reg) old.as_mut_ptr(), options(nostack, preserves_flags));
        asm!("mov {:x}, cs", out(reg) cs, options(nostack, preserves_flags));
    }
    let length = usize::from(u16::from_le_bytes(old[..2].try_into().unwrap())) + 1;
    require((224..=4096).contains(&length), "apic-contract-idt-size");
    let base = u64::from_le_bytes(old[2..].try_into().unwrap());
    let next = core::ptr::addr_of_mut!(IDT).cast::<u8>();
    let handler = svmvisor_fixture_apic_gp as *const () as u64;
    let mut gate = [0u8; 16];
    gate[..2].copy_from_slice(&(handler as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&cs.to_le_bytes()); gate[5] = 0x8e;
    gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
    gate[8..12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
    let mut descriptor = old; descriptor[2..].copy_from_slice(&(next as u64).to_le_bytes());
    unsafe {
        core::ptr::copy_nonoverlapping(base as *const u8, next, length);
        core::ptr::copy_nonoverlapping(gate.as_ptr(), next.add(13 * 16), 16);
        asm!("lidt [{}]", in(reg) descriptor.as_ptr(), options(nostack, preserves_flags));
        for index in [0x840, 0x841, 0x842, 0x848, 0x84f, 0x850, 0x853] {
            for write in [0, 1] {
                require(svmvisor_fixture_apic_fault(index, write) == 1, "apic-contract-extension-msr-gp");
            }
        }
        require(svmvisor_fixture_apic_fault(0x803, 1) == 1, "apic-contract-version-write-gp");
        if read_msr(0x1b) & 0x400 == 0 {
            require(svmvisor_fixture_apic_fault(0x803, 0) == 1, "apic-contract-version-xapic-gp");
        }
        asm!("lidt [{}]", "push {}", "popfq", in(reg) old.as_ptr(), in(reg) flags);
    }
    visibility();
    marker("native-apic-contract-visibility-and-gp-pass\n");
}
