//! Sole finite emulator host LAPIC timer owner, shared by legacy preemption and
//! concurrent scheduling. APM2 rev3.44 7.8.5, 15.13.1, 16.3/16.4.1.
use crate::{hex, host, host_memory, print};
use core::{
    arch::{asm, x86_64::__cpuid},
    ptr,
};
use svmvisor_hypervisor::boot::ownership::OwnershipRecord;
const HOST_COUNT: u32 = 100_000;
pub(crate) const LVT: [usize; 6] = [0x320, 0x330, 0x340, 0x350, 0x360, 0x370];
unsafe extern "C" {
    static mut host_preempt_acks: u64;
}

pub(crate) fn flags() -> u64 {
    let value: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) value, options(preserves_flags));
    }
    value
}
/// # Safety
/// The admitted FEE00000 LAPIC has a supervisor UC mapping; offset is a valid
/// aligned local APIC register and this CPU owns its state. APM2 7.8.5/16.4.
pub(crate) unsafe fn read(offset: usize) -> u32 {
    unsafe { ptr::read_volatile((0xfee00000usize + offset) as *const u32) }
}
/// # Safety
/// Same mapped local-register contract as read; caller additionally owns this
/// register's programming and writes only its admitted bits. APM2 16.4.
pub(crate) unsafe fn write(offset: usize, value: u32) {
    unsafe {
        ptr::write_volatile((0xfee00000usize + offset) as *mut u32, value);
        // Read back the APIC ID to complete the MMIO transaction.
        let _ = read(0x20);
    }
}
unsafe fn input(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}
unsafe fn output(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
/// Read only admitted sources; ISR must be empty outside the integer-only gates.
/// # Safety
/// Local UC LAPIC mapped and exclusively owned, IF=0/GIF=1. APM2 16.4.
pub(crate) unsafe fn pending_sources(allowed: u32) -> u32 {
    let mut pending = 0;
    for word in 0..8 {
        assert_eq!(unsafe { read(0x100 + word * 16) }, 0, "unexpected host ISR");
        let irr = unsafe { read(0x200 + word * 16) };
        if word == 7 {
            assert_eq!(irr & !allowed, 0, "unexpected host IRR");
            pending = irr;
        } else {
            assert_eq!(irr, 0, "unexpected host IRR");
        }
    }
    pending
}
unsafe fn pending() -> bool {
    unsafe { pending_sources(1 << 16) != 0 }
}

/// Arm the same one-shot source used for legacy running preemption. Pending F1
/// remains in the LAPIC; it must never be consumed between empty-check and HLT.
/// # Safety
/// Local timer owned, IF=0/GIF=1, prior one-shot canceled/drained. APM2 16.4.1.
#[cfg(feature = "concurrent-smp")]
pub(crate) unsafe fn arm_concurrent() {
    assert_eq!(flags() & 0x200, 0);
    assert_eq!(unsafe { read(0x380) }, 0);
    assert_eq!(unsafe { read(0x390) }, 0);
    assert_eq!(unsafe { pending_sources(3 << 16) } & (1 << 16), 0);
    unsafe {
        write(0x380, HOST_COUNT);
    }
}

pub(crate) struct HostTimer {
    lvt: [u32; 6],
    tpr: u32,
    svr: u32,
    divide: u32,
    pic: [u8; 2],
    gate: [u8; 16],
    pub(crate) acks: u64,
    pub(crate) rebased: bool,
    concurrent: bool,
    ipi_gate: Option<[u8; 16]>,
}
impl HostTimer {
    /// # Safety
    /// Disposable single-CPU emulator only; no physical/firmware caller. Owned
    /// host tables installed, IF=0/GIF=1, no IRR/ISR state in flight. An inherited
    /// masked timer may be canceled only with retained successful post-EBS
    /// ownership; that terminal path discards its phase and never resumes
    /// firmware. The finite owner then restores the quiescent zero-count baseline.
    pub(crate) unsafe fn acquire(ownership: Option<&OwnershipRecord<'_>>) -> Self {
        unsafe { Self::acquire_inner(ownership, false) }
    }
    /// # Safety
    /// Fixed two-CPU fixture, local private IDT installed, IF=0/GIF=1,
    /// shared UC LAPIC mapping held by BSP until AP restoration. A retained
    /// successful EBS record permits the same masked-timer rebase as acquire.
    /// AP after INIT supplies None. APM2 15.13.1 and chapter16.
    #[cfg(feature = "concurrent-smp")]
    pub(crate) unsafe fn acquire_concurrent(ownership: Option<&OwnershipRecord<'_>>) -> Self {
        unsafe { Self::acquire_inner(ownership, true) }
    }
    unsafe fn acquire_inner(ownership: Option<&OwnershipRecord<'_>>, concurrent: bool) -> Self {
        assert_eq!(flags() & 0x200, 0);
        assert_ne!(__cpuid(1).edx & (1 << 9), 0);
        assert_ne!(__cpuid(1).edx & (1 << 16), 0); // PAT.
        let low: u32;
        let high: u32;
        unsafe {
            asm!("rdmsr", in("ecx") 0x1bu32, out("eax") low, out("edx") high, options(nostack));
        }
        let cpu = if concurrent {
            crate::host_smp::current_cpu()
        } else {
            0
        };
        assert_eq!(
            u64::from(low) | (u64::from(high) << 32),
            0xfee00800 | if cpu == 0 { 0x100 } else { 0 }
        );
        unsafe {
            if !concurrent {
                host_memory::map_timer(true);
            } else {
                host_memory::validate_apic_memory_type();
            }
        }
        assert_eq!(
            (unsafe { read(0x30) } >> 16) & 0xff,
            5,
            "emulator six-LVT admission"
        );
        assert_eq!(unsafe { read(0x20) } >> 24, cpu as u32);
        let inherited_initial = unsafe { read(0x380) };
        let inherited_current = unsafe { read(0x390) };
        let lvt = LVT.map(|offset| unsafe { read(offset) });
        if !concurrent || ownership.is_some() {
            for (label, value) in [
                ("HOST-TIMER-ADMISSION initial=", inherited_initial),
                ("HOST-TIMER-ADMISSION current=", inherited_current),
                ("HOST-TIMER-ADMISSION lvt=", lvt[0]),
            ] {
                print(label);
                hex(u64::from(value));
            }
            for (label, offset) in [
                ("HOST-TIMER-ADMISSION svr=", 0xf0),
                ("HOST-TIMER-ADMISSION tpr=", 0x80),
                ("HOST-TIMER-ADMISSION divide=", 0x3e0),
            ] {
                print(label);
                hex(u64::from(unsafe { read(offset) }));
            }
        }
        assert!(!unsafe { pending() }, "preexisting host timer IRQ");
        assert!(lvt.iter().all(|value| value & (1 << 12) == 0)); // no delivery in flight
        assert!(lvt[0] & (1 << 18) == 0, "TSC-deadline mode unsupported");
        let rebased = inherited_initial != 0;
        if rebased {
            assert!(
                ownership.is_some(),
                "phase discard requires retained post-EBS ownership"
            );
            assert_ne!(
                lvt[0] & (1 << 16),
                0,
                "inherited timer must already be masked"
            );
            assert!(inherited_current <= inherited_initial);
            // APM2 16.4.1: initial zero cancels counting. This is an explicit
            // terminal handoff rebase, not recovery of an inherited timer phase.
            unsafe {
                write(0x380, 0);
            }
        } else {
            assert_eq!(inherited_current, 0);
        }
        assert_eq!(
            unsafe { read(0x380) },
            0,
            "preexisting host timer initial count"
        );
        assert_eq!(
            unsafe { read(0x390) },
            0,
            "preexisting host timer current count"
        );
        assert!(!unsafe { pending() }, "preexisting host timer IRQ");
        let mut owner = Self {
            lvt,
            tpr: unsafe { read(0x80) },
            svr: unsafe { read(0xf0) },
            divide: unsafe { read(0x3e0) },
            pic: if cpu == 0 {
                [unsafe { input(0x21) }, unsafe { input(0xa1) }]
            } else {
                [0; 2]
            },
            gate: [0; 16],
            acks: 0,
            rebased,
            concurrent,
            ipi_gate: None,
        };
        unsafe {
            owner.gate = host::replace_timer_gate(None, concurrent);
            #[cfg(feature = "concurrent-smp")]
            if concurrent {
                owner.ipi_gate = Some(host::replace_ipi_gate(None));
            }
            if cpu == 0 {
                output(0x21, 0xff);
                output(0xa1, 0xff);
            }
            for offset in LVT {
                write(offset, read(offset) | (1 << 16));
            }
            write(0x80, 0xe0);
            write(0xf0, 0x1ff);
            write(0x3e0, 0xb); // divide one, one-shot F0 only
            write(0x320, 0xf0);
            if !concurrent {
                ptr::write_volatile(ptr::addr_of_mut!(host_preempt_acks), 0);
            }
        }
        owner
    }
    /// # Safety
    /// Exclusive legacy timer owner, stopped host IF=0/GIF=1; no pending source.
    /// Paired with cancel_and_drain before any rearm or release. APM2 16.4.1.
    pub(crate) unsafe fn arm(&self) {
        assert_eq!(flags() & 0x200, 0);
        assert!(!unsafe { pending() });
        unsafe {
            write(0x380, HOST_COUNT);
        }
    }
    /// Cancel first, then acknowledge an actual pending F0 through the owned
    /// gate. INTR exit itself neither acknowledges nor identifies the vector.
    /// # Safety
    /// Legacy stopped guest, host state restored, IF=0/GIF=1; intr_exit is the
    /// actual stopped exit classification. Same owner remains live; APM2 15.13.1.
    pub(crate) unsafe fn cancel_and_drain(&mut self, intr_exit: bool) -> bool {
        assert_eq!(flags() & 0x200, 0);
        if intr_exit {
            // Retain independent expiration evidence before cancellation would
            // erase the count: F0 alone cannot attribute an active timer's IRQ.
            assert_eq!(
                unsafe { read(0x390) },
                0,
                "INTR before owned one-shot expiration"
            );
        }
        unsafe {
            write(0x380, 0);
        }
        let ready = unsafe { pending() };
        if intr_exit {
            assert!(ready, "INTR exit without owned timer source");
        }
        if ready {
            // One interrupt-shadow instruction; no polling or unbounded wait.
            unsafe {
                asm!("sti; nop; cli", options(nostack));
            }
            self.acks += 1;
        }
        assert_eq!(
            unsafe { ptr::read_volatile(ptr::addr_of!(host_preempt_acks)) },
            self.acks
        );
        assert!(!unsafe { pending() });
        assert_eq!(unsafe { read(0x390) }, 0);
        ready
    }
    /// # Safety
    /// Local IF=0/GIF=1, timer canceled and every source drained; all remote
    /// senders quiescent. Concurrent mapping stays BSP-owned through AP join.
    /// No guest may resume after restoration. APM2 16.4 and 8.9.
    pub(crate) unsafe fn restore(self) {
        assert_eq!(flags() & 0x200, 0);
        let cpu = if self.concurrent {
            crate::host_smp::current_cpu()
        } else {
            0
        };
        assert!(!unsafe { pending() });
        assert_eq!(unsafe { read(0x380) }, 0);
        assert_eq!(unsafe { read(0x390) }, 0);
        unsafe {
            write(0x320, 0x100f0);
            write(0x3e0, self.divide);
            for (offset, value) in LVT.into_iter().zip(self.lvt) {
                write(offset, value);
            }
            write(0xf0, self.svr);
            write(0x80, self.tpr);
            if cpu == 0 {
                output(0x21, self.pic[0]);
                output(0xa1, self.pic[1]);
            }
        }
        assert_eq!(LVT.map(|offset| unsafe { read(offset) }), self.lvt);
        assert_eq!(unsafe { read(0x80) }, self.tpr);
        assert_eq!(unsafe { read(0xf0) }, self.svr);
        assert_eq!(unsafe { read(0x3e0) }, self.divide);
        if cpu == 0 {
            assert_eq!([unsafe { input(0x21) }, unsafe { input(0xa1) }], self.pic);
        }
        assert_eq!(flags() & 0x200, 0);
        unsafe {
            host::replace_timer_gate(Some(self.gate), self.concurrent);
            // Confirm exact original gate bytes through a no-op replacement.
            assert_eq!(
                host::replace_timer_gate(Some(self.gate), self.concurrent),
                self.gate
            );
            #[cfg(feature = "concurrent-smp")]
            if let Some(gate) = self.ipi_gate {
                host::replace_ipi_gate(Some(gate));
                assert_eq!(host::replace_ipi_gate(Some(gate)), gate);
            }
            if !self.concurrent {
                host_memory::map_timer(false);
            }
        }
    }
}
