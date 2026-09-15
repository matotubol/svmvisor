//! Guest-authored executable workload; no access to monitor structures.
use super::*;
use core::sync::atomic::{AtomicU32, Ordering};
static READY: AtomicU32 = AtomicU32::new(0);
static INJECTOR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static STEP: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static HWCR_STEP: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
const HWCR: u32 = 0xc001_0015;
const IRPERF_EN: u64 = 1 << 30;
#[repr(C, align(4096))]
struct Stack([u8; 16384]);
static mut AP_STACK: Stack = Stack([0; 16384]);
const FIXED: [u32; 11] = [0x250,0x258,0x259,0x268,0x269,0x26a,0x26b,0x26c,0x26d,0x26e,0x26f];
unsafe fn read(index: u32) -> u64 {
    let (low, high): (u32,u32);
    unsafe { asm!("rdmsr", in("ecx") index, out("eax") low, out("edx") high, options(nostack)); }
    (low as u64) | ((high as u64) << 32)
}
unsafe fn write(index: u32, value: u64) {
    unsafe { asm!("wrmsr", in("ecx") index, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack)); }
}
fn query(operation: u32) -> core::arch::x86_64::CpuidResult {
    core::arch::x86_64::__cpuid_count(0x4fff_ca00, operation)
}
fn physical(index: u32) -> u64 {
    let value = query(0x10000 | index);
    require(value.eax == 0x43414348 && value.edx == 0x50485953, "cache-physical-witness-kind");
    (value.ebx as u64) | ((value.ecx as u64) << 32)
}
fn wait(mut predicate: impl FnMut() -> bool) {
    for _ in 0..200_000_000u32 {
        if predicate() { return; }
        core::hint::spin_loop();
    }
    require(false, "cache-guest-barrier-timeout");
}
fn hwcr_write_readback(value: u64) {
    // Both writes execute actual guest WRMSR exits. The repeated write checks
    // idempotent completion, including RDMSR and the post-instruction path.
    for _ in 0..2 {
        unsafe { write(HWCR, value); }
        require(unsafe { read(HWCR) } == value, "cache-hwcr-write-readback");
    }
}
fn hwcr_isolation(slot: usize) -> u64 {
    // fixture_control installs a private modeled HWCR backend per CPU. These
    // instructions exercise the resident handler, not QEMU HWCR/counting.
    let baseline = unsafe { read(HWCR) };
    require(baseline == 0x10, "cache-hwcr-modeled-initial-value");
    hwcr_write_readback(baseline);
    HWCR_STEP[slot].store(1, Ordering::Release);
    wait(|| HWCR_STEP[1-slot].load(Ordering::Acquire) >= 1);
    if slot == 0 {
        hwcr_write_readback(baseline | IRPERF_EN);
        HWCR_STEP[0].store(2, Ordering::Release);
        wait(|| HWCR_STEP[1].load(Ordering::Acquire) >= 2);
        hwcr_write_readback(baseline);
        HWCR_STEP[0].store(3, Ordering::Release);
        wait(|| HWCR_STEP[1].load(Ordering::Acquire) >= 3);
        require(unsafe { read(HWCR) } == baseline, "cache-hwcr-bsp-isolation");
        HWCR_STEP[0].store(4, Ordering::Release);
        wait(|| HWCR_STEP[1].load(Ordering::Acquire) >= 4);
        marker("native-cache-hwcr-modeled-backend\n");
        marker("native-cache-hwcr-isolation-pass\n");
        baseline
    } else {
        wait(|| HWCR_STEP[0].load(Ordering::Acquire) >= 2);
        require(unsafe { read(HWCR) } == baseline, "cache-hwcr-ap-isolation");
        HWCR_STEP[1].store(2, Ordering::Release);
        wait(|| HWCR_STEP[0].load(Ordering::Acquire) >= 3);
        hwcr_write_readback(baseline | IRPERF_EN);
        HWCR_STEP[1].store(3, Ordering::Release);
        wait(|| HWCR_STEP[0].load(Ordering::Acquire) >= 4);
        require(unsafe { read(HWCR) } == baseline | IRPERF_EN, "cache-hwcr-ap-enabled");
        HWCR_STEP[1].store(4, Ordering::Release);
        baseline | IRPERF_EN
    }
}
pub(super) fn start(ap_low: u64, injector: u64) -> ! {
    INJECTOR.store(injector, Ordering::Release);
    require(query(0).eax == 0x4341_4348, "cache-bsp-install");
    unsafe {
        ((ap_low+0x980) as *mut u64).write_volatile(ap as *const () as u64);
        ((ap_low+0x988) as *mut u64).write_volatile(core::ptr::addr_of!(AP_STACK) as u64 + 16384);
        ((ap_low+0x850) as *mut u32).write_volatile(7);
    }
    run(0)
}
unsafe extern "efiapi" fn ap() -> ! {
    require(query(0).eax == 0x4341_4348, "cache-ap-install");
    run(1)
}
fn run(slot: usize) -> ! {
    unsafe { asm!("cli", options(nostack)); }
    let baseline = unsafe { read(0x2ff) };
    let fixed = FIXED.map(|index| unsafe { read(index) });
    let variable: [(u64,u64);8] = core::array::from_fn(|i| unsafe { (read(0x200+i as u32*2), read(0x201+i as u32*2)) });
    let cr0: u64;
    unsafe { asm!("mov {}, cr0", out(reg) cr0, options(nostack)); }
    READY.fetch_or(1 << slot, Ordering::AcqRel);
    wait(|| READY.load(Ordering::Acquire) == 3);
    if slot == 0 { marker("native-cache-paired-before\n"); }
    let expected_hwcr = hwcr_isolation(slot);
    #[cfg(feature = "guest-cache-hwcr")]
    if slot == 0 {
        marker("native-cache-negative-hwcr\n");
        unsafe { write(HWCR, expected_hwcr & !(1 << 4)); }
        require(false, "cache-hwcr-returned");
    }
    #[cfg(feature = "guest-cache-hwcr")]
    if slot == 1 { loop { core::hint::spin_loop(); } }
    #[cfg(feature = "guest-cache-init")]
    if slot == 0 {
        // AP is parked at its own E0. BSP has not disabled caches or entered
        // the transaction: a third CPU's INIT must be refused by the shared-core
        // lifecycle lease, not merely the local cache_active check.
        wait(|| query(1).edx >> 8 == 1);
        marker("native-cache-negative-init-shared-pending\n");
        let injector = INJECTOR.load(Ordering::Acquire);
        require(injector != 0, "cache-init-third-source");
        unsafe { ((injector + 0x850) as *mut u32).write_volatile(8); }
        loop { core::hint::spin_loop(); }
    }
    for generation in 1..=3 {
        unsafe {
            asm!("mov cr0, {}", in(reg) ((cr0 | (1 << 30)) & !(1 << 29)), options(nostack));
            asm!("wbinvd", options(nostack));
            write(0x2ff, baseline & !0x800);
        }
        let witness = query(1);
        require(witness.eax == 0x4341_4348 && witness.ebx as u64 == baseline
            && witness.ecx as u64 == baseline & !0x800 && witness.edx & 7 == 7,
            "cache-real-physical-stable-logical-e0-root-guard");
        require(physical(0x2ff) == baseline, "cache-physical-default");
        require(unsafe { read(HWCR) } == expected_hwcr, "cache-hwcr-preserved-e0");
        for (index, value) in FIXED.into_iter().zip(fixed) {
            require(physical(index) == value, "cache-physical-fixed-bank");
        }
        for (i, (base, mask)) in variable.into_iter().enumerate() {
            require(physical(0x200+i as u32*2) == base && physical(0x201+i as u32*2) == mask,
                "cache-physical-variable-bank");
        }
        STEP[slot].store(generation * 2 - 1, Ordering::Release);
        wait(|| STEP[1-slot].load(Ordering::Acquire) >= generation * 2 - 1);
        #[cfg(any(feature = "guest-cache-low", feature = "guest-cache-cr0", feature = "guest-cache-init"))]
        if slot == 1 { loop { core::hint::spin_loop(); } }
        if slot == 0 {
            #[cfg(feature = "guest-cache-low")]
            unsafe {
                marker("native-cache-negative-low\n");
                core::hint::black_box((0x80000 as *const u64).read_volatile());
                require(false, "cache-low-returned");
            }
            #[cfg(feature = "guest-cache-cr0")]
            unsafe {
                marker("native-cache-negative-cr0\n");
                asm!("mov cr0, {}", in(reg) cr0, options(nostack));
                require(false, "cache-cr0-returned");
            }
        }
        unsafe {
            write(0xc0010010, 1 << 19);
            for (index, value) in FIXED.into_iter().zip(fixed) { write(index, value); }
            write(0xc0010010, 1 << 18);
            for (i, (base, mask)) in variable.into_iter().enumerate() {
                write(0x200+i as u32*2, base); write(0x201+i as u32*2, mask);
            }
            write(0x2ff, baseline);
        }
        let after = query(1);
        require(after.ebx as u64 == baseline && after.ecx as u64 == baseline && after.edx & 7 == 0,
            "cache-e1-local-root-guard-restored");
        unsafe { asm!("mov cr0, {}", in(reg) cr0, options(nostack)); }
        require(unsafe { read(HWCR) } == expected_hwcr, "cache-hwcr-preserved-e1");
        STEP[slot].store(generation * 2, Ordering::Release);
        wait(|| STEP[1-slot].load(Ordering::Acquire) >= generation * 2);
    }
    // AP held bit30 set through three MTRR generations while BSP held it clear.
    // Now both CPUs complete disable and idempotent readback before success.
    hwcr_write_readback(expected_hwcr & !IRPERF_EN);
    require(unsafe { read(HWCR) } == expected_hwcr & !IRPERF_EN, "cache-hwcr-final-disabled");
    HWCR_STEP[slot].store(5, Ordering::Release);
    wait(|| HWCR_STEP[1-slot].load(Ordering::Acquire) >= 5);
    if slot == 0 {
        marker("PASS native-cache-hwcr-three-generations\n");
        marker("PASS native-cache-paired-physical-stable\n"); finish(true);
    }
    loop { core::hint::spin_loop(); }
}
