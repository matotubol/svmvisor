//! Guest-side independent restart witness, using only disposable retained RAM.
use super::*;
unsafe extern "C" {
    static guest_start: u8;
    static guest_end: u8;
    static guest_protected_target: u8;
    static guest_protected: u8;
    static guest_root: u8;
    #[cfg(feature = "guest-cpuid-nrip")]
    static guest_cpuid_base: u8;
    static guest_long_target: u8;
    static guest_long: u8;
    static guest_gdt_base: u8;
    static guest_irq_handler: u8;
}
#[derive(Clone, Copy)]
pub(super) struct Startup {
    base: u64,
}
impl Startup {
    pub(super) fn prepare_all(count: usize) -> [Self; 32] {
        require((2..=32).contains(&count), "guest-startup-count");
        let mut pages = [Self { base: 0 }; 32];
        for page in pages.iter_mut().take(count).skip(1) {
            *page = Self::prepare();
        }
        pages
    }
    fn prepare() -> Self {
        let page = boot::allocate_pages(
            AllocateType::MaxAddress(0x9ffff),
            MemoryType::LOADER_CODE,
            2,
        )
        .expect("guest startup low RAM");
        let base = page.as_ptr() as u64;
        let start = core::ptr::addr_of!(guest_start) as u64;
        let size = core::ptr::addr_of!(guest_end) as u64 - start;
        require(
            base != 0 && base & 4095 == 0 && size < 0x800,
            "guest-startup-shape",
        );
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, 8192);
            core::ptr::copy_nonoverlapping(start as *const u8, base as *mut u8, size as usize);
        }
        let handler = base + core::ptr::addr_of!(guest_irq_handler) as u64 - start;
        let gate = (handler & 0xffff) | (8 << 16) | (0x8e << 40) | ((handler & 0xffff0000) << 32);
        unsafe {
            ((base + 0x1000 + 0xf1 * 16) as *mut u64).write(gate);
            ((base + 0x1008 + 0xf1 * 16) as *mut u64).write(handler >> 32);
        }
        #[cfg(not(feature = "loader-boot"))]
        let root = unsafe {
            let current: u64;
            asm!("mov {}, cr3", out(reg) current, options(nomem, nostack, preserves_flags));
            current
        };
        #[cfg(feature = "loader-boot")]
        let root = super::loader::root();
        let gdt = core::ptr::addr_of!(guest_gdt_base) as u64 - 34;
        for (field, value) in [
            (
                core::ptr::addr_of!(guest_protected_target) as u64,
                base + core::ptr::addr_of!(guest_protected) as u64 - start,
            ),
            (core::ptr::addr_of!(guest_root) as u64, root),
            #[cfg(feature = "guest-cpuid-nrip")]
            (core::ptr::addr_of!(guest_cpuid_base) as u64, base),
            (
                core::ptr::addr_of!(guest_long_target) as u64,
                base + core::ptr::addr_of!(guest_long) as u64 - start,
            ),
            (
                core::ptr::addr_of!(guest_gdt_base) as u64,
                base + gdt - start,
            ),
        ] {
            require(
                value <= u32::MAX as u64 && field >= start && field - start + 4 <= size,
                "guest-startup-patch",
            );
            unsafe {
                ((base + field - start) as *mut u32).write_unaligned(value as u32);
            }
        }
        marker("native-guest-startup-page=");
        hex(base);
        marker("\n");
        Self { base }
    }
    fn read(&self, offset: u64) -> u32 {
        unsafe { ((self.base + offset) as *const u32).read_volatile() }
    }
    pub(super) fn run_all(pages: &[Self; 32], count: usize) -> ! {
        #[cfg(feature = "guest-xapic")]
        unsafe {
            require(read_msr(0x1b) & 0xc00 == 0x800, "guest-xapic-native-mode");
            let before = __cpuid(TEST_LEAF).ebx;
            write_msr(0x80e, u32::MAX as u64);
            write_msr(0x80d, 0x01000000);
            require(
                apic_read(0x80e) == u32::MAX as u64 && apic_read(0x80d) == 0x01000000,
                "xapic-native-logical-registers",
            );
            require(
                __cpuid(TEST_LEAF).ebx.wrapping_sub(before) == 5,
                "xapic-four-mmio-exits-plus-query",
            );
            marker("native-xapic-four-npf-witness\n");
            marker("native-xapic-ldr-dfr-readback\n");
        }
        require(
            unsafe { apic_read(0x802) } == 0,
            "guest-startup-fixture-bsp0",
        );
        for (target, page) in pages.iter().enumerate().take(count).skip(1) {
            page.run_target(target as u32);
        }
        #[cfg(feature = "guest-xapic")]
        marker("PASS native-xapic-mmio-startup\n");
        #[cfg(feature = "guest-xapic-upgrade")]
        unsafe {
            let base = read_msr(0x1b);
            require(base & 0xc00 == 0x800, "upgrade-starts-xapic");
            write_msr(0x1b, base | 0x400);
            require(read_msr(0x1b) == base | 0x400, "upgrade-readback");
            require(apic_read(0x802) == 0, "upgrade-native-id");
            marker("PASS native-xapic-to-x2apic\n");
        }
        #[cfg(feature = "guest-irq-wakeup")]
        pages[1].run_irq_wakeup(1);
        #[cfg(feature = "guest-apic-contract")]
        {
            super::apic_contract::visibility();
            marker("native-apic-contract-after-restart-pass\n");
        }
        #[cfg(feature = "guest-terminal-await")]
        {
            // Disposable fixture: leave AP1 awaiting SIPI, then provoke an
            // unowned MSR exit on BSP. The harness requires a host-side witness
            // that AP1 actually left its AwaitSipi poll through terminal request.
            marker("native-terminal-before-await-init\n");
            unsafe { write_msr(0x830, (1u64 << 32) | 0x4500); }
            for _ in 0..256 { core::hint::black_box(__cpuid(TEST_LEAF)); }
            marker("native-terminal-before-msr-refusal\n");
            unsafe { write_msr(0xc0010100, 0); }
            marker("FAIL native-terminal-msr-returned\n");
            finish(false);
        }
        marker("PASS native-guest-startup-repeat\n");
        #[cfg(feature = "guest-cache")]
        {
            require(count == if cfg!(feature = "guest-cache-init") { 3 } else { 2 }, "cache-fixture-cpu-count");
            super::cache::start(pages[1].base, if count == 3 { pages[2].base } else { 0 });
        }
        #[cfg(not(feature = "guest-cache"))]
        finish(true);
    }
    #[cfg(feature = "guest-irq-wakeup")]
    fn run_irq_wakeup(&self, target: u32) {
        let destination = u64::from(target) << 32;
        let command = |value: u32| unsafe {
            ((self.base + 0x850) as *mut u32).write_volatile(value);
        };
        let wait = |offset, value, reason| {
            for _ in 0..200_000_000u32 {
                if self.read(offset) == value {
                    return;
                }
                core::hint::spin_loop();
            }
            require(false, reason);
        };
        let wake = || {
            // Duplicate SIPI is a wake-only mailbox command for a running CPU.
            unsafe {
                write_msr(0x830, destination | 0x4600 | (self.base >> 12));
            }
            let progress = self.read(0x848);
            for _ in 0..200_000_000u32 {
                if self.read(0x848).wrapping_sub(progress) >= 1_000_000 {
                    return;
                }
                core::hint::spin_loop();
            }
            require(false, "wake-nonexiting-progress");
        };
        command(1);
        wait(0x854, 1, "irq-priority-setup");
        #[cfg(feature = "guest-irq-level")]
        let rtc = RtcLevel::arm(target);
        #[cfg(not(feature = "guest-irq-level"))]
        unsafe {
            write_msr(0x830, destination | 0xf1);
        }
        wait(0x864, 1 << 17, "irq-pending-irr");
        #[cfg(feature = "guest-irq-level")]
        wait(0x878, 1 << 17, "irq-level-tmr");
        wake();
        require(
            self.read(0x860) == 0xf0 && self.read(0x858) == 0,
            "wake-preserves-blocked-irq",
        );
        #[cfg(feature = "guest-irq-level")]
        require(
            RtcLevel::io_read(0x20) & (1 << 14) != 0,
            "wake-preserves-remote-irr-pending",
        );
        command(2);
        wait(0x85c, 1, "irq-handler-entry");
        wake();
        require(
            self.read(0x868) == 1 << 17 && self.read(0x858) == 1,
            "wake-preserves-in-service-irq",
        );
        #[cfg(feature = "guest-irq-reset-refusal")]
        {
            command(5);
            wait(0x85c, 5, "held-irq-software-disabled");
            require(
                self.read(0x86c) == 0xff && self.read(0x868) == 1 << 17,
                "held-irq-retained-after-software-disable",
            );
            marker("native-guest-held-irq-svr-disabled\n");
            marker("native-guest-reset-with-held-irq\n");
            unsafe {
                write_msr(0x830, destination | 0x4500);
            }
            // The target must stop before applying a reset to its active ISR.
            loop {
                core::hint::spin_loop();
            }
        }
        #[cfg(feature = "guest-irq-level")]
        {
            require(
                RtcLevel::io_read(0x20) & (1 << 14) != 0,
                "wake-preserves-remote-irr-isr",
            );
            rtc.acknowledge();
        }
        command(3);
        wait(0x854, 3, "irq-handler-eoi-return");
        require(
            self.read(0x868) == 0 && self.read(0x864) == 0,
            "irq-native-eoi-clears-state",
        );
        #[cfg(feature = "guest-irq-level")]
        {
            require(
                RtcLevel::io_read(0x20) & (1 << 14) == 0,
                "native-eoi-clears-remote-irr",
            );
            rtc.restore();
            marker("PASS native-level-irq-remote-irr-eoi\n");
        }
        command(4);
        wait(0x854, 4, "irq-software-disable");
        wake();
        require(self.read(0x86c) == 0xff, "wake-preserves-software-disable");
        require(
            self.read(0x870) == 0x9abcdef0 && self.read(0x874) == 0x12345678,
            "wake-preserves-live-xmm0",
        );
        marker("PASS native-irq-wakeup-priority-isr-disabled\n");
    }
    fn run_target(&self, target: u32) {
        let vector = self.base >> 12;
        let signature = __cpuid(1).eax;
        let destination = u64::from(target) << 32;
        #[cfg(feature = "guest-startup-broadcast")]
        let destination = destination | (3 << 18);
        for generation in 1..=2u32 {
            // Exercise both documented assertion encodings across repetitions.
            let assertion = if generation == 1 { 0xc500 } else { 0x4500 };
            marker("native-guest-before-init cpu=");
            hex(target.into());
            marker(" generation=");
            hex(generation.into());
            marker("\n");
            unsafe {
                write_msr(0x830, destination | assertion);
            }
            require(
                unsafe { apic_read(0x830) } == destination | assertion,
                "guest-init-icr-shadow",
            );
            // Legacy INIT assertion/deassertion pair: deassert must neither
            // reset the target again nor consume a startup FIFO entry.
            unsafe { write_msr(0x830, destination | 0x8500); }
            require(unsafe { apic_read(0x830) } == destination | 0x8500,
                "guest-init-deassert-icr-shadow");
            unsafe {
                write_msr(0x830, destination | 0x4600 | vector);
            }
            // Duplicate SIPI must not restart an already runnable target.
            unsafe {
                write_msr(0x830, destination | 0x4600 | vector);
            }
            let mut arrived = false;
            for _ in 0..200_000_000u32 {
                if self.read(0x804) == generation {
                    arrived = true;
                    break;
                }
                core::hint::spin_loop();
            }
            require(arrived, "guest-startup-long-timeout");
            require(
                self.read(0x800) == generation,
                "duplicate-sipi-restarted-guest",
            );
            require(
                self.read(0x810) | self.read(0x814) | self.read(0x818) == 0,
                "init-gpr-reset",
            );
            require(self.read(0x81c) == signature, "init-rdx-signature");
            require(
                self.read(0x820) & !0x60000000 == 0x10
                    && self.read(0x824) == 0
                    && self.read(0x828) == 0,
                "init-controls",
            );
            require(self.read(0x82c) == 2, "init-rflags");
            require(self.read(0x830) == 0xd00, "startup-logical-efer");
            #[cfg(feature = "guest-vmcr")]
            require(self.read(0x880) == 0x10 && self.read(0x884) == 0, "startup-vmcr-fixed-policy");
            require(
                self.read(0x834) == 0xff
                    && self.read(0x838) == 0
                    && self.read(0x83c) == 0
                    && self.read(0x840) == 0,
                "init-guest-apic-reset",
            );
            require(self.read(0x844) == TEST_MAGIC, "startup-resident-missing");
            #[cfg(feature = "guest-cpuid-nrip")]
            {
            require(self.read(0x970) == 24 && self.read(0x974) == signature
                && self.read(0x978) == signature && self.read(0x97c) == 8,
                "guest-prefixed-cpuid-compat32-long64");
            marker("native-guest-prefixed-cpuid-pass cpu=");
            hex(target.into()); marker(" generation="); hex(generation.into()); marker("\n");
            }
            require(self.read(0x960) == 0, "init-cleared-timer-initial-count");
            require(self.read(0x964) == 0xf0 && self.read(0x968) == 0x7fff_ffff,
                "guest-resettable-priority-timer-seed");
            let read64 = |offset| u64::from(self.read(offset)) | (u64::from(self.read(offset + 4)) << 32);
            for slot in 0..4 {
                require(read64(0x8b0 + slot * 8) == 0, "init-live-debug-address-reset");
            }
            require(read64(0x8d0) == 0xffff_0ff0 && read64(0x8d8) == 0x400,
                "init-vmcb-debug-reset");
            let debug_seed = [0x1111_1000u64, 0x2222_2000, 0x3333_3000, 0x4444_4000,
                0xffff_0ff1, 0x1_0400];
            for (slot, value) in debug_seed.iter().enumerate() {
                require(read64(0x900 + slot as u64 * 8) == *value, "guest-debug-seed-readback");
            }
            // Deliver another private notification only after the target has
            // seeded debug state. A duplicate SIPI must not clear live DR0-3
            // or the VMCB's DR6/7 merely because a physical INIT carried it.
            // Also deassert once the target is running with seeded live state.
            // A mistaken INIT would park it; the progress/debug witnesses below
            // require continuing execution and unchanged debug registers.
            unsafe { write_msr(0x830, destination | 0x8500); }
            require(unsafe { apic_read(0x830) } == destination | 0x8500,
                "guest-running-deassert-icr-shadow");
            unsafe { write_msr(0x830, destination | 0x4600 | vector); }
            let progress = self.read(0x848);
            let mut advanced = false;
            for _ in 0..20_000_000u32 {
                if self.read(0x848).wrapping_sub(progress) >= 1024 {
                    advanced = true;
                    break;
                }
                core::hint::spin_loop();
            }
            require(advanced, "startup-nonexiting-guest-loop");
            for (slot, value) in debug_seed.iter().enumerate() {
                require(read64(0x930 + slot as u64 * 8) == *value,
                    "duplicate-sipi-preserves-debug-state");
            }
            require(self.read(0x800) == generation,
                "deassert-did-not-restart-guest");
            marker("native-guest-init-deassert-pass cpu=");
            hex(target.into());
            marker(" generation=");
            hex(generation.into());
            marker("\n");
            marker("native-guest-debug-reset-pass cpu=");
            hex(target.into());
            marker(" generation=");
            hex(generation.into());
            marker("\n");
            marker("native-guest-restart-pass cpu=");
            hex(target.into());
            marker(" generation=");
            hex(generation.into());
            marker("\n");
            if generation == 2 {
                marker("native-guest-resettable-state-pass cpu=");
                hex(target.into());
                marker("\n");
            }
        }
    }
}

/// Disposable Q35 fixture only: MC146818 IRQ8 into IOAPIC pin8, level F1.
/// The device holds its line until the guest acknowledges it; the guest owns
/// both that acknowledgment and LAPIC EOI. QEMU10.1 hw/rtc/mc146818rtc.c and
/// hw/intc/ioapic.c define the emulator device behavior exercised here.
#[cfg(feature = "guest-irq-level")]
struct RtcLevel {
    a: u8,
    b: u8,
    low: u32,
    high: u32,
}
#[cfg(feature = "guest-irq-level")]
impl RtcLevel {
    fn cmos_read(index: u8) -> u8 {
        let value: u8;
        unsafe {
            asm!("out dx, al", in("dx") 0x70u16, in("al") index, options(nostack));
            asm!("in al, dx", in("dx") 0x71u16, out("al") value, options(nostack));
        }
        value
    }
    fn cmos_write(index: u8, value: u8) {
        unsafe {
            asm!("out dx, al", in("dx") 0x70u16, in("al") index, options(nostack));
            asm!("out dx, al", in("dx") 0x71u16, in("al") value, options(nostack));
        }
    }
    fn io_read(index: u32) -> u32 {
        unsafe {
            (0xfec00000 as *mut u32).write_volatile(index);
            (0xfec00010 as *const u32).read_volatile()
        }
    }
    fn io_write(index: u32, value: u32) {
        unsafe {
            (0xfec00000 as *mut u32).write_volatile(index);
            (0xfec00010 as *mut u32).write_volatile(value);
        }
    }
    fn arm(target: u32) -> Self {
        let owner = Self {
            a: Self::cmos_read(0xa),
            b: Self::cmos_read(0xb),
            low: Self::io_read(0x20),
            high: Self::io_read(0x21),
        };
        require(
            owner.b & 0x70 == 0 && owner.low & (1 << 14) == 0,
            "rtc-fixture-unowned-source",
        );
        Self::io_write(0x20, 0x180f1);
        Self::io_write(0x21, target << 24);
        Self::cmos_read(0xc);
        Self::cmos_write(0xa, (owner.a & 0xf0) | 0xf); // 2Hz, latched IRQ.
        Self::io_write(0x20, 0x80f1);
        Self::cmos_write(0xb, owner.b | 0x40);
        owner
    }
    fn acknowledge(&self) {
        Self::cmos_write(0xb, self.b); // Stop further periodic IRQs and lower line.
        require(
            Self::cmos_read(0xc) & 0x40 != 0,
            "rtc-periodic-source-witness",
        );
    }
    fn restore(&self) {
        Self::io_write(0x20, 0x180f1);
        Self::io_write(0x21, self.high);
        Self::io_write(0x20, self.low);
        Self::cmos_write(0xa, self.a);
    }
}

unsafe fn read_msr(index: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") index, out("eax") low, out("edx") high, options(nomem,nostack,preserves_flags));
    }
    low as u64 | ((high as u64) << 32)
}
unsafe fn write_msr(index: u32, value: u64) {
    // These volatile DWORD MOVs are actual trapped xAPIC decoder witnesses.
    if (0x800..=0x83f).contains(&index) && unsafe { read_msr(0x1b) } & 0x400 == 0 {
        unsafe {
            if index == 0x830 {
                (0xfee00310 as *mut u32).write_volatile(((value >> 32) as u32) << 24);
            }
            ((0xfee00000u64 + u64::from(index - 0x800) * 16) as *mut u32)
                .write_volatile(value as u32);
        }
        return;
    }
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack,preserves_flags));
    }
}

pub(super) fn quiesce_bsp() {
    // Start the disposable restart witness with quiescent local sources.
    // The IRQ fixture enables and owns its sources after both resets.
    unsafe {
        asm!("cli", options(nomem, nostack));
        let base = read_msr(0x1b);
        require(base & 0x800 != 0, "startup-bsp-apic-disabled");
        #[cfg(not(feature = "guest-xapic"))]
        write_msr(0x1b, base | 0x400);
        #[cfg(feature = "guest-xapic")]
        {
            require(base & 0xc00 == 0x800, "startup-bsp-native-xapic");
            marker("native-fixture-initial-xapic\n");
        }
        for index in [0x832, 0x833, 0x834, 0x835, 0x836, 0x837] {
            write_msr(index, 0x10000);
        }
        write_msr(0x838, 0);
        write_msr(0x83e, 0);
        write_msr(0x808, 0);
        write_msr(0x80f, 0xff);
    }
}

unsafe fn apic_read(index: u32) -> u64 {
    unsafe {
        if read_msr(0x1b) & 0x400 != 0 {
            read_msr(index)
        } else {
            let value =
                ((0xfee00000u64 + u64::from(index - 0x800) * 16) as *const u32).read_volatile();
            if index == 0x830 {
                u64::from(value)
                    | (u64::from((0xfee00310 as *const u32).read_volatile() >> 24) << 32)
            } else if index == 0x802 {
                u64::from(value >> 24)
            } else {
                u64::from(value)
            }
        }
    }
}
