//! Bounded two-host-CPU emulator ownership, flat or retained post-EBS UEFI.
//! APM2 rev3.44 7.2, 14.1/14.1.3, 15.13.1, 15.30.4, 16.5/16.6.
use core::arch::x86_64::__cpuid;

/// Fixture identity from immutable CPUID evidence; no mutable global selector.
pub fn current_cpu() -> usize {
    let id = (__cpuid(1).ebx >> 24) as usize;
    assert!(id < 2, "only emulator APIC ids 0/1 admitted");
    id
}

#[cfg(feature = "concurrent-smp")]
pub use enabled::*;

#[cfg(feature = "concurrent-smp")]
mod enabled {
    use super::current_cpu;
    use core::{
        arch::{asm, x86_64::__cpuid},
        mem::MaybeUninit,
        ptr,
        sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    };
    const LIMIT: u64 = 30_000_000_000;
    use crate::host_lapic::{HostTimer as Owner, pending_sources, read, write};
    use svmvisor_hypervisor::{boot::ownership::{OwnershipRecord,SmpResources,SmpCpuIdentity},
        memory::address::{AddressPolicy,EncryptionState,PhysicalRange}};
    static PHASE: AtomicUsize = AtomicUsize::new(0);
    static ENTRY: AtomicUsize = AtomicUsize::new(0);
    #[unsafe(no_mangle)]
    static host_smp_acks: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    #[unsafe(no_mangle)]
    static host_smp_timer_acks: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    #[unsafe(no_mangle)]
    static host_smp_idle_timer: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    #[unsafe(no_mangle)]
    static host_smp_idle_ipi: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    static HOST_TIMER_CANCEL_RACES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    static HOST_TIMER_CANCEL_ACTIVE_RACES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    static host_smp_idle_returns: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    static mut BSP_OWNER: MaybeUninit<Owner> = MaybeUninit::uninit();
    static mut STARTUP_PAGE: Option<PhysicalRange> = None;
    static mut RESIDENT: Option<SmpResources> = None;
    unsafe extern "C" {
        static resident_handoff: u64;
        static host_smp_trampoline: u8;
        static host_smp_trampoline_end: u8;
        static host_smp_protected_target: u8;
        static host_smp_protected: u8;
        static host_smp_root_value: u8;
        static host_smp_boot_gdt: u8;
        static host_smp_gdt_base: u8;
        static host_ap_save: u8;
        fn host_smp_park() -> !;
        fn host_smp_idle_start();
    }
    /// Serialized raw emulator TSC; never interpreted as physical nanoseconds.
    pub fn now() -> u64 {
        let _ = __cpuid(0);
        let lo: u32;
        let hi: u32;
        unsafe {
            asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack));
        }
        let _ = __cpuid(0);
        u64::from(lo) | (u64::from(hi) << 32)
    }
    fn wait(mut ready: impl FnMut() -> bool) {
        let start = now();
        for _ in 0..100_000_000u64 {
            if ready() {
                return;
            }
            assert!(now().wrapping_sub(start) < LIMIT, "host SMP deadline");
            core::hint::spin_loop();
        }
        panic!("host SMP iteration bound");
    }
    unsafe fn msr(index: u32) -> u64 {
        let lo: u32;
        let hi: u32;
        unsafe {
            asm!("rdmsr",in("ecx") index,out("eax")lo,out("edx")hi,options(nostack));
        }
        u64::from(lo) | (u64::from(hi) << 32)
    }
    unsafe fn set_msr(index: u32, value: u64) {
        unsafe {
            asm!("wrmsr",in("ecx")index,in("eax")value as u32,in("edx")(value>>32) as u32,options(nostack));
        }
    }
    fn assert_if_clear() {
        let flags: u64;
        unsafe {
            asm!("pushfq; pop {}",out(reg)flags,options(preserves_flags));
        }
        assert_eq!(flags & 0x200, 0);
    }
    /// # Safety
    /// Exclusive BSP after private descriptor/memory installation; no other
    /// LAPIC owner. Flat input admits disposable page8000h and reset AP. UEFI
    /// input requires successful EBS, returned MP callbacks and the pinned OVMF
    /// reserved AP transition/park contract; INIT precedes SIPI. The allocated
    /// low page and complete arena remain owned. APM2 14.1/14.1.3 and 16.5; PI1.8A MP.
    pub unsafe fn prepare(ownership:Option<&OwnershipRecord<'_>>) {
        assert_eq!(current_cpu(), 0);
        assert_if_clear();
        let is_resident=unsafe{ptr::read(ptr::addr_of!(resident_handoff))}!=0;
        assert_eq!(is_resident,ownership.is_some(),"resident ownership missing");
        let resources=ownership.map(|o|o.smp().expect("two-CPU UEFI admission missing"));
        if let Some(r)=resources { assert_eq!(identity(),r.cpus()[0],"BSP identity changed after EBS"); }
        let page=resources.map(|r|r.low_page()).unwrap_or_else(||
            AddressPolicy::new(__cpuid(0x80000008).eax as u8,EncryptionState::Unencrypted{encryption_bit:None})
                .unwrap().validate(0x8000,4096,4096).unwrap());
        // PPR57896 rev3.00 CPUID1.EDX[HTT] p69: zero is a single-thread
        // product; EBX[23:16] p67 gives the multithread count. This admits only
        // the fixed emulator package, not arbitrary physical topology.
        let basic = __cpuid(1);
        let logical_cpus = if basic.edx & (1 << 28) == 0 {
            1
        } else {
            (basic.ebx >> 16) & 0xff
        };
        if logical_cpus != 2 {
            crate::print("REFUSE concurrent-host-topology expected=2 actual=");
            crate::hex(u64::from(logical_cpus));
            panic!("requires exactly two emulator host CPUs");
        }
        assert_eq!(
            PHASE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire),
            Ok(0)
        );
        unsafe {
            crate::host_memory::map_timer(true);
        }
        let owner = unsafe { Owner::acquire_concurrent(ownership) };
        unsafe {
            ptr::addr_of_mut!(BSP_OWNER).write(MaybeUninit::new(owner));
            ptr::addr_of_mut!(STARTUP_PAGE).write(Some(page));
            ptr::addr_of_mut!(RESIDENT).write(resources);
        }
        let length = ptr::addr_of!(host_smp_trampoline_end) as usize
            - ptr::addr_of!(host_smp_trampoline) as usize;
        assert!(length > 0 && length < 0xff0);
        let root = unsafe { crate::host_memory::map_startup(page,Some(true)) };
        assert!(root<=u32::MAX as u64);
        let offset=|p:*const u8|p as usize-ptr::addr_of!(host_smp_trampoline) as usize;
        unsafe {
            ptr::write_bytes(page.base() as *mut u8, 0, 4096);
            ptr::copy_nonoverlapping(
                ptr::addr_of!(host_smp_trampoline),
                page.base() as *mut u8,
                length,
            );
            for (field,value) in [
                (ptr::addr_of!(host_smp_protected_target),page.base()+offset(ptr::addr_of!(host_smp_protected)) as u64),
                (ptr::addr_of!(host_smp_gdt_base),page.base()+offset(ptr::addr_of!(host_smp_boot_gdt)) as u64),
                (ptr::addr_of!(host_smp_root_value),root)] {
                let index=offset(field); assert!(index+4<=length && value<=u32::MAX as u64);
                let destination=(page.base() as *mut u8).add(index).cast::<u32>();
                destination.write_unaligned(value as u32);
                assert_eq!(destination.read_unaligned(),value as u32);
            }
            crate::host_memory::map_startup(page,Some(false));
        }
        crate::print("SMP-STARTUP patched=0000000000000003\n");
        crate::print("SMP-STARTUP rdtscp=");crate::hex(u64::from((__cpuid(0x80000001).edx>>27)&1));
        for (key,value) in [("SMP-STARTUP page=",page.base()),("SMP-STARTUP vector=",page.base()>>12),
            ("SMP-STARTUP root=",root),("SMP-STARTUP resident=",u64::from(is_resident))] {crate::print(key);crate::hex(value);}
    }
    fn identity()->SmpCpuIdentity {
        let vendor=__cpuid(0);let basic=__cpuid(1);
        let mut name=[0;12];name[..4].copy_from_slice(&vendor.ebx.to_le_bytes());
        name[4..8].copy_from_slice(&vendor.edx.to_le_bytes());name[8..].copy_from_slice(&vendor.ecx.to_le_bytes());
        SmpCpuIdentity{processor_id:u64::from(basic.ebx>>24),apic_id:basic.ebx>>24,signature:basic.eax,vendor:name}
    }
    unsafe fn send(target: usize, command: u32) {
        assert!(target < 2);
        assert_if_clear();
        wait(|| unsafe { read(0x300) & (1 << 12) == 0 });
        unsafe {
            write(0x310, (target as u32) << 24);
            write(0x300, command);
        }
        wait(|| unsafe { read(0x300) & (1 << 12) == 0 });
    }
    /// # Safety
    /// prepare completed; callback and its backing remain valid until join; CPU0
    /// does not access AP-owned guest or host mutable memory. APM2 14.1/14.1.3 and 16.5.
    pub unsafe fn start(entry: extern "C" fn()) {
        assert_eq!(current_cpu(), 0);
        assert_eq!(PHASE.load(Ordering::Acquire), 1);
        ENTRY.store(entry as usize, Ordering::Release);
        // Table16-4 admits edge INIT; no legacy level-deassert command is needed.
        unsafe {
            send(1, 0x4500);
            let page=ptr::addr_of!(STARTUP_PAGE).read().unwrap();
            send(1, 0x600 | (page.base()>>12) as u32);
        }
        wait(|| PHASE.load(Ordering::Acquire) >= 2);
    }
    /// # Safety
    /// Active concurrent fixture; target admitted, both local LAPIC owners live.
    /// Physical F1 is a host notification only; guest delivery stays guest-owned.
    pub unsafe fn kick(target: usize) {
        assert_ne!(current_cpu(), target);
        assert_eq!(PHASE.load(Ordering::Acquire), 2);
        unsafe {
            send(target, 0xf1);
        }
    }
    /// Source-specific real interrupt-gate acknowledgements, never EXITINFO.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Sources {
        pub timer: u64,
        pub ipi: u64,
    }
    fn sources(cpu: usize) -> Sources {
        Sources {
            timer: host_smp_timer_acks[cpu].load(Ordering::Acquire),
            ipi: host_smp_acks[cpu].load(Ordering::Acquire),
        }
    }
    fn delta(after: Sources, before: Sources) -> Sources {
        Sources {
            timer: after.timer.checked_sub(before.timer).unwrap(),
            ipi: after.ipi.checked_sub(before.ipi).unwrap(),
        }
    }
    /// # Safety
    /// Exclusive stopped local guest, IF=0/GIF=1, prior timer canceled/drained.
    /// The timer and F1 source are privately owned; APM2 16.4.1, 15.13.1.
    pub unsafe fn arm_timer() {
        unsafe {
            crate::host_lapic::arm_concurrent();
        }
    }
    /// Cancel the local one-shot and drain only actual F0/F1 host sources. F1
    /// coalesces; its count is notification telemetry, not guest request count.
    /// # Safety
    /// Stopped guest with host xstate/MSRs/stack restored, IF=0/GIF=1. No sender
    /// may target this CPU after its owner is released. APM2 15.13.1/16.4.
    pub unsafe fn cancel_and_acknowledge() -> Sources {
        assert_if_clear();
        let cpu = current_cpu();
        let before = sources(cpu);
        let pending_before = unsafe { pending_sources(3 << 16) };
        let current_before = unsafe { read(0x390) };
        if pending_before & (1 << 16) != 0 {
            assert_eq!(current_before, 0, "F0 before one-shot expiry");
        }
        // An expiry between the IRR read and cancellation remains a real F0;
        // its vector-specific ISR/EOI gate is the source evidence for that race.
        unsafe {
            write(0x380, 0);
        }
        let start = now();
        for _ in 0..1024 {
            if unsafe { pending_sources(3 << 16) } == 0 {
                assert_eq!(unsafe { read(0x390) }, 0);
                let result = delta(sources(cpu), before);
                if result.timer != 0 && pending_before & (1 << 16) == 0 {
                    HOST_TIMER_CANCEL_RACES[cpu].fetch_add(result.timer, Ordering::AcqRel);
                    if current_before != 0 {
                        HOST_TIMER_CANCEL_ACTIVE_RACES[cpu]
                            .fetch_add(result.timer, Ordering::AcqRel);
                    }
                }
                return result;
            }
            unsafe {
                asm!("sti; nop; cli", options(nostack));
            }
            assert!(
                now().wrapping_sub(start) < LIMIT,
                "host source drain deadline"
            );
        }
        panic!("host source drain bound");
    }
    /// Quiescent control-plane F1 drain. No running/idle timer may be armed.
    /// # Safety
    /// Same stopped-host contract as cancel_and_acknowledge, timer already idle.
    pub unsafe fn acknowledge() -> u64 {
        assert_if_clear();
        assert_eq!(unsafe { read(0x380) }, 0);
        let result = unsafe { cancel_and_acknowledge() };
        assert_eq!(result.timer, 0);
        result.ipi
    }
    /// Execute one host HLT with an armed one-shot watchdog and return to IF=0.
    /// No acknowledgement occurs between arming and HLT: F1 published after a
    /// caller's empty check remains pending. STI's one-instruction shadow makes
    /// HLT the next executed instruction even when an IRQ was already pending.
    /// # Safety
    /// Local timer/gates/mapping owned; stopped guest and fully restored host
    /// state, IF=0/GIF=1. Caller bounds repeated idles and checks work on return.
    /// APM3 rev3.37 HLT p388/STI p477, APM2 15.17, 15.21.5, 16.4.1. A failed
    /// emulator timer requires the runner's outer timeout; this is no NMI watchdog.
    pub unsafe fn idle() -> Sources {
        assert_if_clear();
        let cpu = current_cpu();
        let before = sources(cpu);
        let wake_before = idle_timer_wakes(cpu) + idle_ipi_wakes(cpu);
        unsafe {
            arm_timer();
            host_smp_idle_start();
        }
        assert_if_clear();
        // The gate checks its hardware-stacked RIP, not a software parked flag.
        assert!(
            idle_timer_wakes(cpu) + idle_ipi_wakes(cpu) > wake_before,
            "HLT return without owned source at post-HLT RIP"
        );
        host_smp_idle_returns[cpu].fetch_add(1, Ordering::AcqRel);
        unsafe {
            cancel_and_acknowledge();
        }
        delta(sources(cpu), before)
    }
    pub fn acknowledgements(cpu: usize) -> u64 {
        sources(cpu).ipi
    }
    pub fn timer_acknowledgements(cpu: usize) -> u64 {
        sources(cpu).timer
    }
    pub fn timer_cancel_races(cpu: usize) -> u64 {
        HOST_TIMER_CANCEL_RACES[cpu].load(Ordering::Acquire)
    }
    pub fn timer_cancel_active_races(cpu: usize) -> u64 {
        HOST_TIMER_CANCEL_ACTIVE_RACES[cpu].load(Ordering::Acquire)
    }
    pub fn idle_returns(cpu: usize) -> u64 {
        host_smp_idle_returns[cpu].load(Ordering::Acquire)
    }
    pub fn idle_timer_wakes(cpu: usize) -> u64 {
        host_smp_idle_timer[cpu].load(Ordering::Acquire)
    }
    pub fn idle_ipi_wakes(cpu: usize) -> u64 {
        host_smp_idle_ipi[cpu].load(Ordering::Acquire)
    }
    /// # Safety
    /// BSP guest has stopped and no more notifications can be sent by either CPU.
    /// AP callback must return; bounded wait then restore BSP ownership. AP retains
    /// immutable tables/stack while permanently parked; no firmware resumption.
    pub unsafe fn join() {
        assert_eq!(current_cpu(), 0);
        wait(|| PHASE.load(Ordering::Acquire) == 3);
        unsafe {
            acknowledge();
            ptr::addr_of_mut!(BSP_OWNER).read().assume_init().restore();
            crate::host_memory::map_startup(ptr::addr_of!(STARTUP_PAGE).read().unwrap(),None);
            crate::host_memory::map_timer(false);
        }
        PHASE.store(4, Ordering::Release);
    }
    #[unsafe(no_mangle)]
    extern "C" fn host_smp_ap_main() -> ! {
        assert_eq!(current_cpu(), 1);
        assert_eq!(PHASE.load(Ordering::Acquire), 1);
        // Publish all BSP preparation, including RESIDENT, before AP reads it.
        let entry = ENTRY.load(Ordering::Acquire);
        assert_ne!(entry, 0);
        if let Some(resources)=unsafe{ptr::addr_of!(RESIDENT).read()} {
            assert_eq!(identity(),resources.cpus()[1],"AP identity changed after resident INIT/SIPI");
        }
        unsafe {
            crate::host::install_ap().unwrap();
        }
        assert!(__cpuid(0x80000000).eax >= 0x8000000a);
        assert_ne!(__cpuid(0x80000001).ecx & 4, 0, "AP SVM capability");
        assert_ne!(
            __cpuid(0x8000000a).edx & 1,
            0,
            "AP nested paging capability"
        );
        assert_ne!(__cpuid(0x8000000a).ebx, 0, "AP ASID count");
        assert_eq!(ptr::addr_of!(host_ap_save) as usize & 4095, 0);
        let old_efer = unsafe { msr(0xc0000080) };
        let old_save = unsafe { msr(0xc0010117) };
        assert_eq!(old_efer & (1 << 12), 0);
        unsafe {
            set_msr(0xc0000080, old_efer | (1 << 12));
            set_msr(0xc0010117, ptr::addr_of!(host_ap_save) as u64);
            asm!("stgi", options(nostack));
        }
        let owner = unsafe { Owner::acquire_concurrent(None) };
        PHASE.store(2, Ordering::Release);
        let callback: extern "C" fn() = unsafe { core::mem::transmute(entry) };
        callback();
        unsafe {
            acknowledge();
            owner.restore();
            set_msr(0xc0010117, old_save);
            set_msr(0xc0000080, old_efer);
        }
        assert_eq!(unsafe { msr(0xc0010117) }, old_save);
        assert_eq!(unsafe { msr(0xc0000080) }, old_efer);
        PHASE.store(3, Ordering::Release);
        unsafe { host_smp_park() }
    }

    /// Raw physical LAPIC priority for bounded failure-only evidence.
    /// # Safety
    /// Stopped owned host CPU, active LAPIC mapping, IF=0; read-only local MMIO.
    /// APM2 rev3.44 16.4.2 Task Priority Register. Not guest controller state.
    pub unsafe fn diagnostic_tpr() -> u32 {
        assert_if_clear();
        unsafe { read(0x80) }
    }
}
