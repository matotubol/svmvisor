//! Local APIC register address space and physical host x2APIC primitives.
//!
//! AMD APM2 rev3.44: APIC_BASE is Figure 16-2. Register offsets are the xAPIC
//! MMIO offsets of 16.3.2/Table 16-2; the AVIC backing page uses the same
//! offsets (15.29.3.1/Table 15-22). In x2APIC mode a register with offset `o`
//! is MSR `800h + (o >> 4)` (16.11.1/Table 16-6), except that ICR is one 64-bit
//! MSR at 830h, SELF IPI exists only at 83Fh, and RRR/DFR have no MSR.
//! Offsets 400h and above (MSRs 840h and above) are AMD extended registers.
//!
//! The host primitives read or write only the executing CPU's physical x2APIC,
//! or signal another core through the AVIC doorbell (15.29.8.2). They never
//! validate or forward a guest register access; the pure x2AVIC owners in
//! `svm::x2avic` do that and reach hardware only through [`PhysicalX2Apic`].

/// APIC Base Address Register, MSR 0000_001Bh (Figure 16-2).
pub const APIC_BASE: u32 = 0x1b;
/// Boot-strap CPU core flag (BSC, bit 8, read-only).
pub const APIC_BASE_BSP: u64 = 1 << 8;
/// x2APIC mode enable (EXTD, bit 10).
pub(crate) const APIC_BASE_EXTD: u64 = 1 << 10;
/// APIC enable (AE, bit 11).
pub(crate) const APIC_BASE_AE: u64 = 1 << 11;
/// AE and EXTD together: enabled x2APIC, the only admitted interface.
pub const APIC_BASE_X2APIC: u64 = APIC_BASE_AE | APIC_BASE_EXTD;
/// APIC base address field (ABA, bits 51:12).
pub const APIC_BASE_ADDRESS: u64 = 0x000f_ffff_ffff_f000;
/// Reset value of ABA; the only admitted base address.
pub const APIC_BASE_DEFAULT_ADDRESS: u64 = 0xfee0_0000;

// Register offsets (Table 16-2). Multi-bank registers name their first bank.
pub const ID: u16 = 0x20;
pub const VERSION: u16 = 0x30;
pub const TPR: u16 = 0x80;
/// Arbitration priority; reset to zero by INIT. RO MSR 809h (Table 16-6).
pub const APR: u16 = 0x90;
pub const PPR: u16 = 0xa0;
pub const EOI: u16 = 0xb0;
/// Remote read register; xAPIC only, eliminated in x2APIC mode.
pub const RRR: u16 = 0xc0;
pub const LDR: u16 = 0xd0;
pub const SVR: u16 = 0xf0;
/// Eight 32-bit in-service banks, 100h-170h.
pub const ISR: u16 = 0x100;
/// Eight 32-bit trigger-mode banks, 180h-1F0h.
pub const TMR: u16 = 0x180;
/// Eight 32-bit request banks, 200h-270h.
pub const IRR: u16 = 0x200;
pub const ESR: u16 = 0x280;
/// ICR low half in xAPIC/backing layout; the whole 64-bit ICR in x2APIC.
pub const ICR: u16 = 0x300;
/// ICR high half; xAPIC/backing layout only.
pub const ICR_HIGH: u16 = 0x310;
pub const LVT_TIMER: u16 = 0x320;
pub const LVT_THERMAL: u16 = 0x330;
pub const LVT_PERFORMANCE: u16 = 0x340;
pub(crate) const LVT_LINT0: u16 = 0x350;
pub(crate) const LVT_LINT1: u16 = 0x360;
pub const LVT_ERROR: u16 = 0x370;
pub const TIMER_INITIAL_COUNT: u16 = 0x380;
pub const TIMER_CURRENT_COUNT: u16 = 0x390;
pub const TIMER_DIVIDE: u16 = 0x3e0;
/// First AMD extended register (Extended APIC Feature).
pub const EXTENDED: u16 = 0x400;
/// The six standard LVT entries, in offset order.
pub const LVTS: [u16; 6] =
    [LVT_TIMER, LVT_THERMAL, LVT_PERFORMANCE, LVT_LINT0, LVT_LINT1, LVT_ERROR];

/// SVR APIC software enable (ASE, bit 8; Figure 16-17 p641).
pub(crate) const SVR_SOFTWARE_ENABLE: u32 = 1 << 8;
/// LVT mask (bit 16); every standard LVT resets to exactly this value.
pub const LVT_MASKED: u32 = 1 << 16;

/// x2APIC ICR (MSR 830h) bits that must be zero: 31:20, 17:16 and 13:12
/// (16.13 and Figure 16-34, p661; the figure's "55:20" row is a stale copy of
/// Figure 16-18 whose bit diagram shows 31:20).
pub(crate) const ICR_RESERVED: u64 = 0xfff0_0000 | (3 << 16) | (3 << 12);
/// ICR bit 12, the xAPIC delivery status (Figure 16-18 p642). 16.13 p661:
/// "eliminated and must be zero" in x2APIC mode.
pub(crate) const ICR_DELIVERY_STATUS: u64 = 1 << 12;

/// LVT (Figure 16-7 p635) and ICR (Figure 16-18 pp642-643) message types,
/// bits 10:8. x2APIC eliminates ICR encodings 1, 3 and 7 (16.13 p661).
pub(crate) const MESSAGE_FIXED: u8 = 0;
pub(crate) const MESSAGE_LOWEST_PRIORITY: u8 = 1;
pub(crate) const MESSAGE_SMI: u8 = 2;
pub(crate) const MESSAGE_REMOTE_READ: u8 = 3;
pub(crate) const MESSAGE_NMI: u8 = 4;
pub(crate) const MESSAGE_INIT: u8 = 5;
pub(crate) const MESSAGE_STARTUP: u8 = 6;
pub(crate) const MESSAGE_EXTERNAL: u8 = 7;

/// First and last MSR of the dedicated x2APIC range (16.11.1).
pub(crate) const X2APIC_MSR_FIRST: u32 = 0x800;
pub(crate) const X2APIC_MSR_LAST: u32 = 0x8ff;

/// Named MSRs used where a constant pattern is required.
pub(crate) const ID_MSR: u32 = msr(ID);
pub const ICR_MSR: u32 = msr(ICR);
pub(crate) const TIMER_CURRENT_COUNT_MSR: u32 = msr(TIMER_CURRENT_COUNT);
/// SELF IPI, x2APIC only (Table 16-6).
pub(crate) const SELF_IPI_MSR: u32 = 0x83f;

const _: () = {
    assert!(msr(TPR) == 0x808 && msr(EOI) == 0x80b && msr(SVR) == 0x80f);
    assert!(msr(ISR) == 0x810 && msr(TMR) == 0x818 && msr(IRR) == 0x820);
    assert!(ID_MSR == 0x802 && ICR_MSR == 0x830 && msr(LVT_TIMER) == 0x832);
    assert!(TIMER_CURRENT_COUNT_MSR == 0x839 && msr(TIMER_DIVIDE) == 0x83e);
    assert!(msr(EXTENDED) == 0x840 && APIC_BASE_X2APIC == 0xc00);
    assert!(ICR_RESERVED == 0xfff3_3000 && ICR_RESERVED & ICR_DELIVERY_STATUS != 0);
    assert!(msr(APR) == 0x809 && msr(ESR) == 0x828 && msr(LVT_ERROR) == 0x837);
};

/// One CPU's physical x2APIC register interface (APM2 16.11, Table 16-6).
///
/// The `svm::x2avic` owners use it for mirrored LVT/timer state, physical ISR
/// and TMR inspection, and physical EOI. They pass only implemented register
/// MSRs of Table 16-6 and values whose reserved bits are clear. [`HostX2Apic`]
/// implements it on hardware; host tests implement recording fakes.
pub trait PhysicalX2Apic {
    /// RDMSR of one implemented x2APIC register MSR.
    fn read(&mut self, msr: u32) -> u64;
    /// WRMSR of one implemented x2APIC register MSR.
    fn write(&mut self, msr: u32, value: u64);
}

/// The executing CPU's own physical x2APIC. It is neither `Send` nor `Sync`.
/// It reaches only the dedicated x2APIC MSR range 800h-8FFh (16.11.1 p657);
/// every other MSR has its own owner.
#[cfg(target_arch = "x86_64")]
pub struct HostX2Apic {
    _local: core::marker::PhantomData<*mut ()>,
}

#[cfg(target_arch = "x86_64")]
impl HostX2Apic {
    /// # Safety
    /// For the whole lifetime of the value, on the CPU that created it: CPL0;
    /// physical x2APIC advertised (CPUID Fn0000_0001 ECX[21]) and enabled
    /// (APIC_BASE AE=EXTD=1); host interrupt acceptance closed (IF=0); and
    /// exclusive ownership of this CPU's physical LAPIC registers. The value is
    /// used only through [`PhysicalX2Apic`] by the `svm::x2avic` owners, which
    /// access implemented registers of 800h-8FFh with valid values, so no
    /// RDMSR or WRMSR faults. APM2 rev3.44 16.9-16.11, Table 16-6.
    pub unsafe fn new() -> Self {
        Self { _local: core::marker::PhantomData }
    }
}

#[cfg(target_arch = "x86_64")]
impl PhysicalX2Apic for HostX2Apic {
    fn read(&mut self, msr: u32) -> u64 {
        debug_assert!((X2APIC_MSR_FIRST..=X2APIC_MSR_LAST).contains(&msr), "not an x2APIC MSR");
        // SAFETY: the constructor's contract covers every owner access.
        unsafe { read_physical_msr(msr) }
    }

    fn write(&mut self, msr: u32, value: u64) {
        debug_assert!((X2APIC_MSR_FIRST..=X2APIC_MSR_LAST).contains(&msr), "not an x2APIC MSR");
        // SAFETY: the constructor's contract covers every owner access.
        unsafe { write_physical_msr(msr, value) }
    }
}

/// Host physical APIC ID accepted by the AVIC doorbell in both documented
/// formats: APM2 rev3.44 Figure 15-22 p579 has an 8-bit field with bits 63:8
/// MBZ, PPR 57896 rev3.00 p216 a 32-bit field. ID 255 is excluded as well:
/// Figure 15-18 p573 reserves physical-table entry 255 in xAVIC mode, and
/// whether that reservation also applies in x2AVIC mode is not stated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoorbellTarget(u8);

impl DoorbellTarget {
    pub const fn new(host_apic_id: u32) -> Option<Self> {
        if host_apic_id <= 254 { Some(Self(host_apic_id as u8)) } else { None }
    }

    pub const fn apic_id(self) -> u32 {
        self.0 as u32
    }
}

/// Highest physical in-service vector. Host acceptance must stay closed for
/// the whole scan. APM2 16.6.3 p647-648.
pub(crate) fn highest_in_service(apic: &mut impl PhysicalX2Apic) -> Option<u8> {
    highest_vector(&in_service_banks(apic))
}

/// All eight physical ISR banks (MSRs 810h-817h, Figure 16-24).
pub(crate) fn in_service_banks(apic: &mut impl PhysicalX2Apic) -> [u32; 8] {
    let mut banks = [0; 8];
    for (index, bank) in banks.iter_mut().enumerate() {
        *bank = apic.read(msr(ISR) + index as u32) as u32;
    }
    banks
}

/// Highest set vector of an eight-bank 256-bit register image.
pub fn highest_vector(bitmap: &[u32; 8]) -> Option<u8> {
    for index in (0..8).rev() {
        if bitmap[index] != 0 {
            return Some((index * 32 + 31 - bitmap[index].leading_zeros() as usize) as u8);
        }
    }
    None
}

/// Physical TMR bit of a just-accepted vector (Figure 16-25, 16.6.3 p648):
/// set for a level-sensitive interrupt, clear for an edge one.
pub(crate) fn level_triggered(apic: &mut impl PhysicalX2Apic, vector: u8) -> bool {
    apic.read(msr(TMR) + u32::from(vector / 32)) & (1 << (vector % 32)) != 0
}

/// x2APIC MSR for a Table 16-2 offset. `ICR` maps to the merged 64-bit ICR;
/// callers must not pass offsets without an x2APIC register (RRR, ICR_HIGH
/// or the DFR).
pub const fn msr(offset: u16) -> u32 {
    X2APIC_MSR_FIRST + (offset as u32 >> 4)
}

/// Signal the core that owns `target` to evaluate its running guest's vAPIC
/// backing page. APM2 rev3.44 15.29.8.2 p578-579: a doorbell received in
/// guest mode makes that core evaluate IRR; WRMSR serialization is relaxed.
/// What a doorbell does to a core that is not in guest mode (in its host, or
/// not yet in its resident runtime at all) is not documented (U5); the
/// caller relies on it being harmless because that core's next VMRUN
/// evaluates IRR (15.29.8.3 p579). Informative only: Linux KVM avic.c treats
/// such a spurious doorbell as harmless.
///
/// # Safety
/// CPL0 with SVM enabled on an AMD CPU with CPUID Fn8000_000A EDX[13]
/// (AVIC) = 1, which enables this MSR (PPR 57896 rev3.00 p216). `target`
/// already satisfies both value formats, so the WRMSR cannot fault; the
/// target core's state does not affect the sender. The MSR is write-only and
/// is never read.
#[cfg(target_arch = "x86_64")]
pub unsafe fn ring_avic_doorbell(target: DoorbellTarget) {
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") super::msr::AVIC_DOORBELL,
            in("eax") u32::from(target.0), in("edx") 0u32, options(nostack));
    }
}

/// RDMSR of one physical x2APIC register; only `HostX2Apic` calls it.
///
/// # Safety
/// Ring0, physical x2APIC advertised and enabled, `msr` an implemented
/// readable register of Table 16-6, and exclusive same-CPU physical LAPIC
/// ownership. APM2 rev3.44 16.9-16.11/Table16-6.
#[cfg(target_arch = "x86_64")]
unsafe fn read_physical_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nostack));
    }
    u64::from(low) | (u64::from(high) << 32)
}

/// WRMSR of one physical x2APIC register; only `HostX2Apic` calls it.
///
/// # Safety
/// The requirements of `read_physical_msr`, plus: `msr` is a writable
/// register of Table 16-6 and `value` sets no reserved bit (16.11.3 p659
/// makes that WRMSR raise #GP(0)). Serialization is relaxed for x2APIC
/// writes (16.11.2 p659).
#[cfg(target_arch = "x86_64")]
unsafe fn write_physical_msr(msr: u32, value: u64) {
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") value as u32,
            in("edx") (value >> 32) as u32, options(nostack));
    }
}
