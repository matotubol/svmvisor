//! Independent manual-conformance tests for the x2APIC/x2AVIC guest interrupt
//! controller. Every expected value below is derived from the manuals, never
//! from the implementation:
//!
//! - APM2 = AMD64 Architecture Programmer's Manual Vol. 2, pub. 24593 rev 3.44
//!   (SHA256 3d9dcb3f...c48c). Citations give the printed page (= PDF page - 62)
//!   and the table or figure; the pages were read as rendered images.
//! - PPR = AMD PPR for Family 1Ah Model 44h B0, pub. 57896 rev 3.00
//!   (product-specific; supplementary only).
//! - ACPI = ACPI 6.6 and this machine's captured MADT (enabled x2APIC IDs
//!   {0-11, 16-27}; UIDs 12-23 map to IDs 16-27).
//! - Dn / Un = the Phase B design decisions (work/x2avic-batch-2026-09-16/
//!   phase-b-brief.md) and the unresolved manual points they settle. A test that
//!   depends on such a decision rather than on the manual says so.
//!
//! Only the public API is used. Implementation constants are deliberately not
//! used for expected values; register offsets, MSR numbers and masks are
//! restated here from the manual.
use std::cell::RefCell;
use std::collections::BTreeMap;

use svmvisor_hypervisor::{
    arch::x86_64::{
        apic::{DoorbellTarget, PhysicalX2Apic},
        registers::GuestRegisters,
    },
    memory::address::{AddressPolicy, EncryptionState},
    svm::{
        permission_maps::{MsrAccess, Msrpm, Permission},
        vmcb::Vmcb,
        x2avic::{
            AvicExit, BackingPage, ENABLE_BITS, GUEST_APIC_VERSION, MAX_ID, NATIVE_CONTROL,
            NativeX2AvicProfile, PhysicalIdTable, X2AvicCapabilities,
            ipi::{FanOutError, IpiAction, IpiDrop, IpiRefusal},
            irq::{self, Capture, IrqError, PhysicalIrqLedger},
            registers::{self, CaptureRefusal, CapturedInterface, Emulation, GuestX2Apic, Refusal},
            startup::{
                NativeIcr, NativeStartupCommand, NativeStartupEffect, NativeStartupMailbox,
                NativeStartupState, NativeStartupTarget,
            },
        },
    },
};

// ---------------------------------------------------------------------------
// Manual constants
// ---------------------------------------------------------------------------

/// APIC register offsets, APM2 Table 16-2 p631. The AVIC backing page uses the
/// same offsets (APM2 15.29.3.1, Table 15-22 pp566-568).
mod off {
    pub const ID: u16 = 0x20;
    pub const VERSION: u16 = 0x30;
    pub const TPR: u16 = 0x80;
    pub const APR: u16 = 0x90;
    pub const PPR: u16 = 0xa0;
    pub const EOI: u16 = 0xb0;
    pub const RRR: u16 = 0xc0;
    pub const LDR: u16 = 0xd0;
    pub const SVR: u16 = 0xf0;
    pub const ISR: u16 = 0x100;
    pub const TMR: u16 = 0x180;
    pub const IRR: u16 = 0x200;
    pub const ESR: u16 = 0x280;
    pub const ICR_LOW: u16 = 0x300;
    pub const ICR_HIGH: u16 = 0x310;
    pub const LVT_TIMER: u16 = 0x320;
    pub const LVT_THERMAL: u16 = 0x330;
    pub const LVT_PERF: u16 = 0x340;
    pub const LVT_LINT0: u16 = 0x350;
    pub const LVT_LINT1: u16 = 0x360;
    pub const LVT_ERROR: u16 = 0x370;
    pub const INITIAL_COUNT: u16 = 0x380;
    pub const CURRENT_COUNT: u16 = 0x390;
    pub const DIVIDE: u16 = 0x3e0;
    pub const LVTS: [u16; 6] = [LVT_TIMER, LVT_THERMAL, LVT_PERF, LVT_LINT0, LVT_LINT1, LVT_ERROR];
}

/// x2APIC MSR of an offset: 800h + (offset >> 4) (APM2 16.11.1 p657).
fn msr_of(offset: u16) -> u32 {
    0x800 + u32::from(offset >> 4)
}

const APIC_BASE_MSR: u32 = 0x1b; // APM2 Figure 16-2 p630
const EOI_MSR: u32 = 0x80b;
const APR_MSR: u32 = 0x809;
const SVR_MSR: u32 = 0x80f;
const ESR_MSR: u32 = 0x828;
const LVT_TIMER_MSR: u32 = 0x832;
const LVT_THERMAL_MSR: u32 = 0x833;
const LVT_PERF_MSR: u32 = 0x834;
const LVT_LINT0_MSR: u32 = 0x835;
const LVT_LINT1_MSR: u32 = 0x836;
const LVT_ERROR_MSR: u32 = 0x837;
const INITIAL_COUNT_MSR: u32 = 0x838;
const CURRENT_COUNT_MSR: u32 = 0x839;
const DIVIDE_MSR: u32 = 0x83e;
const SELF_IPI_MSR: u32 = 0x83f;
const LVT_MSRS: [u32; 6] =
    [LVT_TIMER_MSR, LVT_THERMAL_MSR, LVT_PERF_MSR, LVT_LINT0_MSR, LVT_LINT1_MSR, LVT_ERROR_MSR];

/// LVT mask bit 16 (APM2 Figure 16-7 p635); every LVT resets to exactly this
/// value (Table 16-2 p631).
const MASK: u64 = 1 << 16;

/// Reserved (MBZ) bits of each writable x2APIC register, 64-bit view. Bits 63:32
/// of every non-ICR register are reserved in x2APIC mode (APM2 16.11.3 p659).
fn range(high: u32, low: u32) -> u64 {
    let width = high - low + 1;
    let ones = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
    ones << low
}
/// Timer LVT: 31:18, 15:13, 11:8 (Figure 16-8 p636) plus 63:32.
fn timer_reserved() -> u64 {
    range(63, 18) | range(15, 13) | range(11, 8)
}
/// Thermal, perf and error LVTs: 31:17, 15:13, 11 (Figures 16-13 p638,
/// 16-14 and 16-15 p639) plus 63:32.
fn thermal_reserved() -> u64 {
    range(63, 17) | range(15, 13) | range(11, 11)
}
/// LINT0/LINT1 LVTs: 31:17, 13, 11 (Figure 16-12 p638) plus 63:32.
fn lint_reserved() -> u64 {
    range(63, 17) | range(13, 13) | range(11, 11)
}
/// SVR: 31:10 (Figure 16-17 p641) plus 63:32.
fn svr_reserved() -> u64 {
    range(63, 10)
}
/// Divide configuration: 31:4 and 2 (Figure 16-11 p637) plus 63:32.
fn divide_reserved() -> u64 {
    range(63, 4) | range(2, 2)
}
/// Initial count: 31:0 is the count (Figure 16-10 p637); 63:32 reserved.
fn initial_count_reserved() -> u64 {
    range(63, 32)
}
/// Read-only LVT bits a guest write may carry but that are never stored:
/// DS bit 12 on every LVT, RIR bit 14 on LINT (Figure 16-7 p635; D2/U14).
const DS: u64 = 1 << 12;
const RIR: u64 = 1 << 14;

/// Logical x2APIC ID (APM2 16.14 p662; 15.29.5.3 p574):
/// cluster_id[15:0] = x2APIC_ID[19:4], logical_id[15:0] = 1 << x2APIC_ID[3:0].
fn logical_id(id: u32) -> u32 {
    (((id >> 4) & 0xffff) << 16) | (1 << (id & 0xf))
}

/// Enabled x2APIC IDs of the captured MADT in table order (ACPI 6.6 5.2.12.2;
/// madt-decoded.txt entries 0-23): primary threads, then second threads.
const MADT_IDS: [u32; 24] = [
    0x00, 0x02, 0x04, 0x06, 0x08, 0x0a, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1a, 0x01, 0x03, 0x05, 0x07,
    0x09, 0x0b, 0x11, 0x13, 0x15, 0x17, 0x19, 0x1b,
];

fn policy(bits: u8) -> AddressPolicy {
    AddressPolicy::new(bits, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

// ---------------------------------------------------------------------------
// Recording model of one physical x2APIC
// ---------------------------------------------------------------------------

/// Physical x2APIC model. It enforces the manual's WRMSR/RDMSR legality for the
/// host register interface, so any access that would #GP on real hardware
/// (APM2 16.11 p657, 16.11.3 p659, Table 16-6 p658) is recorded as a
/// violation, and models the side effects the tests rely on:
/// - a write of MSR 80Bh clears the highest ISR bit (16.6.4 p652);
/// - a write of the initial count reloads the current count (16.4.1 p636).
struct Lapic {
    reg: BTreeMap<u32, u64>,
    writes: Vec<(u32, u64)>,
    violations: Vec<String>,
}

impl Lapic {
    /// Host-owned physical state: SVR software-enabled with spurious vector FFh
    /// (D3: the physical SVR stays host-owned at 1FFh), LVTs at their reset
    /// value (Table 16-2 p631).
    fn host() -> Self {
        let mut reg = BTreeMap::new();
        reg.insert(SVR_MSR, 0x1ff);
        reg.insert(0x803, 0x8105_0010);
        for msr in LVT_MSRS {
            reg.insert(msr, MASK);
        }
        Self { reg, writes: Vec::new(), violations: Vec::new() }
    }

    fn get(&self, msr: u32) -> u64 {
        self.reg.get(&msr).copied().unwrap_or(0)
    }

    /// Physical acceptance of `vector` (16.6.3 p647-648): the ISR bit is set when
    /// the core takes the interrupt; TMR is set for level, cleared for edge.
    fn accept(&mut self, vector: u8, level: bool) {
        let bit = 1u64 << (vector % 32);
        *self.reg.entry(0x810 + u32::from(vector / 32)).or_default() |= bit;
        let tmr = self.reg.entry(0x818 + u32::from(vector / 32)).or_default();
        if level {
            *tmr |= bit;
        } else {
            *tmr &= !bit;
        }
    }

    fn in_service(&self) -> Vec<u8> {
        (0..=255u8).filter(|v| self.get(0x810 + u32::from(v / 32)) & (1 << (v % 32)) != 0).collect()
    }

    fn eoi_count(&self) -> usize {
        self.writes.iter().filter(|(msr, _)| *msr == EOI_MSR).count()
    }

    fn wrote(&self, msr: u32) -> bool {
        self.writes.iter().any(|(m, _)| *m == msr)
    }

    fn assert_legal(&self) {
        assert!(
            self.violations.is_empty(),
            "illegal physical x2APIC accesses: {:?}",
            self.violations
        );
    }
}

/// Table 16-6 p658 standard registers (840h-853h excluded: the guest exposes
/// no extended space, so the host owners have no reason to touch it).
fn physical_implemented(msr: u32) -> bool {
    matches!(msr, 0x802 | 0x803 | 0x808..=0x80b | 0x80d | 0x80f | 0x810..=0x828 | 0x830
        | 0x832..=0x839 | 0x83e | 0x83f)
}

impl PhysicalX2Apic for Lapic {
    fn read(&mut self, msr: u32) -> u64 {
        if !physical_implemented(msr) {
            self.violations.push(format!("RDMSR of unimplemented {msr:#x} (p659)"));
        }
        if msr == EOI_MSR || msr == SELF_IPI_MSR {
            self.violations.push(format!("RDMSR of write-only {msr:#x} (p662, PPR p175)"));
        }
        self.get(msr)
    }

    fn write(&mut self, msr: u32, value: u64) {
        self.writes.push((msr, value));
        let reserved = match msr {
            LVT_TIMER_MSR => Some(timer_reserved() | DS),
            LVT_THERMAL_MSR | LVT_PERF_MSR | LVT_ERROR_MSR => Some(thermal_reserved() | DS),
            LVT_LINT0_MSR | LVT_LINT1_MSR => Some(lint_reserved() | DS | RIR),
            INITIAL_COUNT_MSR => Some(initial_count_reserved()),
            DIVIDE_MSR => Some(divide_reserved()),
            EOI_MSR => Some(u64::MAX),
            _ => None,
        };
        match reserved {
            Some(mask) if value & mask != 0 => self
                .violations
                .push(format!("WRMSR {msr:#x} = {value:#x} sets reserved/RO bits (p659)")),
            Some(_) => {}
            None => self
                .violations
                .push(format!("WRMSR {msr:#x} = {value:#x}: not a guest-mirrored register")),
        }
        match msr {
            EOI_MSR => {
                for bank in (0x810..=0x817u32).rev() {
                    let bits = self.reg.entry(bank).or_default();
                    if *bits != 0 {
                        *bits &= !(1u64 << (63 - bits.leading_zeros()));
                        return;
                    }
                }
            }
            INITIAL_COUNT_MSR => {
                self.reg.insert(INITIAL_COUNT_MSR, value);
                self.reg.insert(CURRENT_COUNT_MSR, value);
            }
            _ => {
                self.reg.insert(msr, value);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Backing page helpers
// ---------------------------------------------------------------------------

fn reg(page: &BackingPage, offset: u16) -> u32 {
    page.read_register(offset).unwrap()
}

fn set(page: &BackingPage, offset: u16, value: u32) {
    page.write_register_stopped(offset, value).unwrap();
}

fn snapshot(page: &BackingPage) -> Vec<u32> {
    (0..0x1000u16).step_by(16).map(|o| reg(page, o)).collect()
}

fn set_vector(page: &BackingPage, base: u16, vector: u8) {
    let bank = base + u16::from(vector / 32) * 16;
    set(page, bank, reg(page, bank) | (1 << (vector % 32)));
}

fn clear_vector(page: &BackingPage, base: u16, vector: u8) {
    let bank = base + u16::from(vector / 32) * 16;
    set(page, bank, reg(page, bank) & !(1 << (vector % 32)));
}

fn vectors(page: &BackingPage, base: u16) -> Vec<u8> {
    (0..=255u8)
        .filter(|v| reg(page, base + u16::from(v / 32) * 16) & (1 << (v % 32)) != 0)
        .collect()
}

/// The guest core takes a pending interrupt: IRR bit to ISR bit (16.6.4 p651).
fn deliver_to_guest(page: &BackingPage, vector: u8) {
    assert!(vectors(page, off::IRR).contains(&vector), "{vector:#x} not pending");
    clear_vector(page, off::IRR, vector);
    set_vector(page, off::ISR, vector);
}

fn map_bit(map: &Msrpm, msr: u32, write: bool) -> bool {
    // APM2 15.11 p518, Table 15-8: MSRs 0-1FFFh at byte offsets 0-7FFh, two
    // bits per MSR, the lsb for read and the msb for write; 1 intercepts.
    let bit = 2 * msr as usize + usize::from(write);
    map.bytes()[bit / 8] & (1 << (bit % 8)) != 0
}

// ---------------------------------------------------------------------------
// One stopped vCPU
// ---------------------------------------------------------------------------

/// AP APIC_BASE: enabled x2APIC (AE=EXTD=1) at the reset base FEE0_0000h
/// (Figure 16-2 p630, Table 16-5 p655).
const AP_BASE: u64 = 0xfee0_0c00;
/// BSP APIC_BASE: the same with BSC (bit 8) set.
const BSP_BASE: u64 = 0xfee0_0d00;

struct Vcpu {
    guest: GuestX2Apic,
    page: BackingPage,
    irq: PhysicalIrqLedger,
    lapic: Lapic,
}

impl Vcpu {
    /// x2APIC ID 0x13 (MADT UID 15), APIC_BASE as an AP, backing page at its
    /// reset state, then software-enabled (SVR 1FFh).
    fn new() -> Self {
        let vcpu = Self::with(0x13, AP_BASE, 48);
        set(&vcpu.page, off::SVR, 0x1ff);
        vcpu
    }

    fn with(id: u32, base: u64, bits: u8) -> Self {
        let mut page = BackingPage::new();
        page.reset_stopped(id, GUEST_APIC_VERSION).unwrap();
        Self {
            guest: GuestX2Apic::admit(base, &policy(bits)).unwrap(),
            page,
            irq: PhysicalIrqLedger::new(),
            lapic: Lapic::host(),
        }
    }

    fn read(&mut self, msr: u32) -> Emulation {
        self.guest.emulate(msr, None, &self.page, &mut self.irq, &mut self.lapic)
    }

    fn write(&mut self, msr: u32, value: u64) -> Emulation {
        self.guest.emulate(msr, Some(value), &self.page, &mut self.irq, &mut self.lapic)
    }

    /// Hold one physical level source for `vector`, published into the guest
    /// IRR with TMR set (D6; 16.6.3 p648), so that guest EOI writes are
    /// intercepted and emulated.
    fn hold_level(&mut self, vector: u8) {
        self.lapic.accept(vector, true);
        let captured = irq::capture(vector, &self.page, &mut self.irq, &mut self.lapic);
        assert_eq!(captured, Ok(Some(Capture::Level)), "level capture of {vector:#x}");
        assert!(self.irq.holds(vector));
    }

    /// Asserts that `outcome` changed nothing: backing page, ledger and the
    /// physical write log are as they were.
    fn unchanged<T>(&mut self, what: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        let page = snapshot(&self.page);
        let writes = self.lapic.writes.len();
        let irq = self.irq;
        let result = f(self);
        assert_eq!(snapshot(&self.page), page, "{what}: backing page changed");
        assert_eq!(self.lapic.writes.len(), writes, "{what}: physical x2APIC written");
        assert_eq!(self.irq, irq, "{what}: level-source ledger changed");
        result
    }
}

const GP: Emulation = Emulation::GeneralProtection;
const WRITTEN: Emulation = Emulation::Written;

fn refused(reason: Refusal, value: u64) -> Emulation {
    Emulation::Refused { reason, value }
}

// ---------------------------------------------------------------------------
// 1. Table 16-6 row by row: interception profile and emulation outcome class
// ---------------------------------------------------------------------------

/// Table 16-6 p658 rows (840h-853h are listed there but absent for this guest:
/// its version register has EAS=0, D2/U10).
fn listed(msr: u32) -> bool {
    matches!(msr, 0x802 | 0x803 | 0x808 | 0x809 | 0x80a | 0x80b | 0x80d | 0x80f
        | 0x810..=0x827 | 0x828 | 0x830 | 0x832..=0x839 | 0x83e | 0x83f)
}

/// Reads the x2AVIC hardware serves from the backing page: "Read: Allowed" in
/// Table 15-22 pp566-567, limited to readable Table 16-6 rows. EOI (80Bh) is
/// write-only in Table 16-6, so its read is owned by the VMM (D1/U2).
fn hardware_read(msr: u32) -> bool {
    matches!(msr, 0x802 | 0x803 | 0x808 | 0x80a | 0x80d | 0x80f | 0x810..=0x827 | 0x828
        | 0x830 | 0x832..=0x838 | 0x83e)
}

/// Writes Table 15-22 accelerates: TPR, EOI (edge), ICRL and SELF IPI
/// ("Write: Accelerated by AVIC" / "Allowed (x2AVIC)"). Every trap/fault write
/// is intercepted before the access so #GP(0) stays possible (15.11 p518 check
/// order; 15.29.10 p583; D1).
fn hardware_write(msr: u32) -> bool {
    matches!(msr, 0x808 | 0x80b | 0x830 | 0x83f)
}

#[test]
fn table_16_6_interception_profile_follows_table_15_22() {
    // APM2 Table 16-6 p658, Table 15-22 pp566-568, 15.29.10 p583; decision D1.
    for msr in 0x800..=0x8ffu32 {
        assert_eq!(
            registers::intercepted(msr, MsrAccess::Read),
            !hardware_read(msr),
            "read of {msr:#x}"
        );
        assert_eq!(
            registers::intercepted(msr, MsrAccess::Write),
            !hardware_write(msr),
            "write of {msr:#x}"
        );
        if !listed(msr) {
            // Unimplemented MSRs must reach the owner in both directions to
            // raise #GP(0) (p659), although Table 15-22 p568 would let AVIC
            // read and write the backing page there (U12).
            assert!(registers::intercepted(msr, MsrAccess::Read), "{msr:#x}");
            assert!(registers::intercepted(msr, MsrAccess::Write), "{msr:#x}");
        }
    }
    // APIC_BASE (1Bh) is not an x2APIC MSR (16.11 p657) and its transition
    // rules (p655) need the VMM: both directions intercepted (D1/D4).
    assert!(registers::intercepted(APIC_BASE_MSR, MsrAccess::Read));
    assert!(registers::intercepted(APIC_BASE_MSR, MsrAccess::Write));
}

#[test]
fn private_msrpm_matches_the_profile_from_any_prior_state() {
    // APM2 15.11 p518 (MSRPM layout, lsb read / msb write, 1 = intercept) and
    // Table 15-8; the profile of Table 15-22 / D1. The per-vCPU map must end up
    // with exactly the profile whatever the x2APIC bits held before.
    let mut all_allowed = Msrpm::native_boot();
    for msr in (0x800..=0x8ffu32).chain([APIC_BASE_MSR]) {
        for access in [MsrAccess::Read, MsrAccess::Write] {
            all_allowed.set(msr, access, Permission::Allow).unwrap();
        }
    }
    let mut all_intercepted = Msrpm::native_boot();
    for msr in 0x800..=0x8ffu32 {
        for access in [MsrAccess::Read, MsrAccess::Write] {
            all_intercepted.set(msr, access, Permission::Intercept).unwrap();
        }
    }
    for (name, mut map) in [
        ("new", Msrpm::new()),
        ("native_boot", Msrpm::native_boot()),
        ("all allowed", all_allowed),
        ("all intercepted", all_intercepted),
    ] {
        map.configure_native_x2avic();
        for msr in 0x800..=0x8ffu32 {
            assert_eq!(map_bit(&map, msr, false), !hardware_read(msr), "{name}: read {msr:#x}");
            assert_eq!(map_bit(&map, msr, true), !hardware_write(msr), "{name}: write {msr:#x}");
        }
        assert!(map_bit(&map, APIC_BASE_MSR, false), "{name}: APIC_BASE read");
        assert!(map_bit(&map, APIC_BASE_MSR, true), "{name}: APIC_BASE write");
    }
}

#[test]
fn eoi_write_interception_tracks_held_level_sources() {
    // Table 15-22 p566: EOI writes are accelerated for edge-triggered
    // interrupts; D6 intercepts them exactly while a level source is held.
    let mut vcpu = Vcpu::new();
    let mut map = Msrpm::new();
    map.configure_native_x2avic();
    let before: Vec<u8> = map.bytes().to_vec();
    assert!(!map.update_x2apic_eoi_intercept(&vcpu.irq), "empty ledger: no change");
    assert!(!map_bit(&map, EOI_MSR, true));

    vcpu.hold_level(0x62);
    assert!(map.update_x2apic_eoi_intercept(&vcpu.irq), "held source: map changes");
    assert!(map_bit(&map, EOI_MSR, true), "EOI write intercepted");
    assert!(!map.update_x2apic_eoi_intercept(&vcpu.irq), "idempotent");
    let changed: Vec<usize> = map
        .bytes()
        .iter()
        .zip(&before)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    let eoi_write_bit = 2 * EOI_MSR as usize + 1;
    assert_eq!(changed, vec![eoi_write_bit / 8], "only the EOI write bit moves");
    assert_eq!(map.bytes()[changed[0]] ^ before[changed[0]], 1 << (eoi_write_bit % 8));

    // The guest takes the interrupt and completes it with EOI (16.6.4 p652).
    deliver_to_guest(&vcpu.page, 0x62);
    assert_eq!(vcpu.write(EOI_MSR, 0), WRITTEN);
    assert!(vcpu.irq.is_empty(), "EOI completed the level source");
    assert!(map.update_x2apic_eoi_intercept(&vcpu.irq), "acceleration restored");
    assert!(!map_bit(&map, EOI_MSR, true));
    assert_eq!(map.bytes().to_vec(), before);
    vcpu.lapic.assert_legal();
}

/// Representative WRMSR values for writes that must fault whatever is written.
const ANY_VALUES: [u64; 8] = [0, 1, 0x10, 0xff, 0xffff_ffff, 1 << 32, 1 << 63, u64::MAX];

#[test]
fn unlisted_x2apic_msrs_fault_on_read_and_write() {
    // APM2 p659: MSRs in 800h-8FFh that are not listed "are unimplemented and
    // reserved. A #GP(0) exception is generated if a WRMSR or an RDMSR
    // instruction attempts to access" them. 80Ch/80Eh "are not used and are
    // reserved" and there is no 831h because the ICR is merged into 830h
    // (16.11.1 p657).
    let unlisted: Vec<u32> = (0x800..=0x801)
        .chain(0x804..=0x807)
        .chain([0x80c, 0x80e])
        .chain(0x829..=0x82f)
        .chain([0x831])
        .chain(0x83a..=0x83d)
        .collect();
    assert_eq!(unlisted.len(), 2 + 4 + 2 + 7 + 1 + 4);
    let mut vcpu = Vcpu::new();
    for msr in unlisted {
        assert!(!listed(msr));
        vcpu.unchanged("unlisted read", |v| assert_eq!(v.read(msr), GP, "read {msr:#x}"));
        for value in ANY_VALUES {
            vcpu.unchanged("unlisted write", |v| {
                assert_eq!(v.write(msr, value), GP, "write {msr:#x} = {value:#x}")
            });
        }
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn amd_extended_space_is_absent_and_faults_on_read_and_write() {
    // Table 16-6 p658 lists 840h-853h, but the guest version register reports
    // EAS=0 (no extended APIC register space, Figure 16-4 p632), so the whole
    // 840h-8FFh range is treated as unimplemented (p659): decision D2/U10.
    let mut vcpu = Vcpu::new();
    for msr in 0x840..=0x8ffu32 {
        vcpu.unchanged("extended read", |v| assert_eq!(v.read(msr), GP, "read {msr:#x}"));
        for value in ANY_VALUES {
            vcpu.unchanged("extended write", |v| {
                assert_eq!(v.write(msr, value), GP, "write {msr:#x} = {value:#x}")
            });
        }
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn x2apic_id_write_faults() {
    // APM2 16.12 p660: "Attempting to write MSR 802h ... causes a #GP(0)".
    let mut vcpu = Vcpu::new();
    let id = u64::from(reg(&vcpu.page, off::ID));
    assert_eq!(id, 0x13);
    for value in ANY_VALUES.into_iter().chain([id]) {
        vcpu.unchanged("802h write", |v| assert_eq!(v.write(0x802, value), GP, "{value:#x}"));
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn read_only_register_writes_fault() {
    // Table 16-6 p658 marks 803h, 809h, 80Ah, 80Dh (also "read-only", 16.14
    // p661), 810h-827h and 839h RO. The APM does not name the write fault; the
    // PPR marks them Error-on-write (p175-p182). Decision D2/U1: #GP(0).
    let ro: Vec<u32> = [0x803, APR_MSR, 0x80a, 0x80d]
        .into_iter()
        .chain(0x810..=0x827)
        .chain([CURRENT_COUNT_MSR])
        .collect();
    let mut vcpu = Vcpu::new();
    for msr in ro {
        let current = match msr {
            0x803 => u64::from(GUEST_APIC_VERSION),
            0x80d => u64::from(reg(&vcpu.page, off::LDR)),
            _ => 0,
        };
        for value in ANY_VALUES.into_iter().chain([current]) {
            vcpu.unchanged("RO write", |v| {
                assert_eq!(v.write(msr, value), GP, "write {msr:#x} = {value:#x}")
            });
        }
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn write_only_register_reads_fault() {
    // SELF IPI: "This register is write-only and attempts to read it cause a
    // #GP(0) exception" (16.15 p662). EOI is WO (Table 16-6 p658, Figure 16-28
    // p652); its read fault is decision D2/U2 (PPR p175: Error-on-read).
    let mut vcpu = Vcpu::new();
    vcpu.unchanged("SELF IPI read", |v| assert_eq!(v.read(SELF_IPI_MSR), GP));
    vcpu.unchanged("EOI read", |v| assert_eq!(v.read(EOI_MSR), GP));
    vcpu.lapic.assert_legal();
}

#[test]
fn esr_accepts_only_a_zero_write() {
    // Table 16-6 p658 and 16.11.3 p659: "A WRMSR of a non-zero value causes a
    // #GP(0) exception." A zero write stores zero (D2: the virtual error state
    // is always empty, U15/U16).
    let mut vcpu = Vcpu::new();
    for bit in 0..64 {
        vcpu.unchanged("ESR non-zero", |v| assert_eq!(v.write(ESR_MSR, 1 << bit), GP, "bit {bit}"));
    }
    vcpu.unchanged("ESR all ones", |v| assert_eq!(v.write(ESR_MSR, u64::MAX), GP));
    set(&vcpu.page, off::ESR, 0x40);
    assert_eq!(vcpu.write(ESR_MSR, 0), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::ESR), 0);
    vcpu.lapic.assert_legal();
}

#[test]
fn eoi_nonzero_write_faults_when_intercepted() {
    // Table 16-6 p658: EOI "#GP(0) if non-zero value is written". The write is
    // intercepted while a level source is held (D6).
    let mut vcpu = Vcpu::new();
    vcpu.hold_level(0x62);
    deliver_to_guest(&vcpu.page, 0x62);
    for value in [1, 0x62, 0xffff_ffff, 1 << 32, u64::MAX] {
        vcpu.unchanged("EOI non-zero", |v| assert_eq!(v.write(EOI_MSR, value), GP, "{value:#x}"));
    }
    assert!(vcpu.irq.holds(0x62));
    assert_eq!(vectors(&vcpu.page, off::ISR), vec![0x62]);
    assert_eq!(vcpu.lapic.eoi_count(), 0);
    vcpu.lapic.assert_legal();
}

#[test]
fn intercepted_reads_of_apr_and_current_count_are_emulated() {
    // Table 15-22 pp566-567 faults both reads, so the VMM serves them: APR from
    // TPR/ISR/IRR (Figure 16-22 p647; D2) and the current count from the
    // mirrored physical timer (Figure 16-9 p637; D2).
    let mut vcpu = Vcpu::new();
    set(&vcpu.page, off::TPR, 0x35);
    assert_eq!(vcpu.read(APR_MSR), Emulation::Read(0x35));
    vcpu.lapic.reg.insert(CURRENT_COUNT_MSR, 0x1234_5678);
    let page = snapshot(&vcpu.page);
    assert_eq!(vcpu.read(CURRENT_COUNT_MSR), Emulation::Read(0x1234_5678));
    assert_eq!(snapshot(&vcpu.page), page);
    assert!(vcpu.lapic.writes.is_empty());
    vcpu.lapic.assert_legal();
}

#[test]
fn hardware_owned_accesses_reaching_the_owner_are_refused() {
    // Doc contract of `Refusal::UnownedAccess` (D1): an access the profile
    // leaves to AVIC, or an MSR outside APIC_BASE and 800h-8FFh, means the
    // intercept profile and the exit disagree, so the owner stops.
    let mut vcpu = Vcpu::new();
    for msr in (0x800..=0x8ffu32).filter(|m| hardware_read(*m)) {
        vcpu.unchanged("hw read", |v| {
            assert_eq!(v.read(msr), refused(Refusal::UnownedAccess, 0), "read {msr:#x}")
        });
    }
    for msr in [0x808u32, 0x830, SELF_IPI_MSR] {
        vcpu.unchanged("hw write", |v| {
            assert_eq!(v.write(msr, 0x30), refused(Refusal::UnownedAccess, 0x30), "write {msr:#x}")
        });
    }
    for msr in [0x0u32, 0x1a, 0x1c, 0x7ff, 0x900, 0xc000_0080, 0xc001_011b] {
        assert!(registers::intercepted(msr, MsrAccess::Read), "{msr:#x}");
        assert!(registers::intercepted(msr, MsrAccess::Write), "{msr:#x}");
        vcpu.unchanged("foreign read", |v| {
            assert_eq!(v.read(msr), refused(Refusal::UnownedAccess, 0), "read {msr:#x}")
        });
        vcpu.unchanged("foreign write", |v| {
            assert_eq!(v.write(msr, 5), refused(Refusal::UnownedAccess, 5), "write {msr:#x}")
        });
    }
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// 2. Reserved-bit masks, one test per boundary bit
// ---------------------------------------------------------------------------

fn offset_of(msr: u32) -> u16 {
    ((msr - 0x800) << 4) as u16
}

/// Read-only bits of each register that a write drops (D2/U14).
fn read_only_bits(msr: u32) -> u64 {
    match msr {
        LVT_LINT0_MSR | LVT_LINT1_MSR => DS | RIR,
        LVT_TIMER_MSR | LVT_THERMAL_MSR | LVT_PERF_MSR | LVT_ERROR_MSR => DS,
        _ => 0,
    }
}

/// Writing `base` with the reserved bit set faults without side effects
/// (16.11.3 p659); writing `base` with the legal neighbour bit completes and
/// stores the value without its read-only bits.
fn check_boundary(msr: u32, base: u64, reserved: u32, legal: u32) {
    let mut vcpu = Vcpu::new();
    let bad = base | (1u64 << reserved);
    vcpu.unchanged("reserved bit", |v| assert_eq!(v.write(msr, bad), GP, "{msr:#x} = {bad:#x}"));
    let good = base | (1u64 << legal);
    assert_eq!(vcpu.write(msr, good), WRITTEN, "{msr:#x} = {good:#x}");
    assert_eq!(
        u64::from(reg(&vcpu.page, offset_of(msr))),
        good & !read_only_bits(msr),
        "stored {msr:#x}"
    );
    vcpu.lapic.assert_legal();
}

macro_rules! boundary {
    ($name:ident, $msr:expr, $base:expr, reserved $bad:expr, legal $good:expr) => {
        #[test]
        fn $name() {
            check_boundary($msr, $base, $bad, $good);
        }
    };
}

/// An unmasked, edge, fixed entry with a legal vector (Figure 16-7 p635).
const LVT_BASE: u64 = 0x30;

// Timer LVT, Figure 16-8 p636: reserved 31:18, 15:13, 11:8; TMM 17, M 16,
// DS 12 (RO), VEC 7:0; bits 63:32 reserved by 16.11.3 p659. A read-only
// neighbour (DS) is accepted and not stored (D2/U14).
boundary!(timer_bit18_reserved_bit17_timer_mode, LVT_TIMER_MSR, LVT_BASE, reserved 18, legal 17);
boundary!(timer_bit15_reserved_bit16_mask, LVT_TIMER_MSR, LVT_BASE, reserved 15, legal 16);
boundary!(timer_bit13_reserved_bit12_delivery_status, LVT_TIMER_MSR, LVT_BASE, reserved 13, legal 12);
boundary!(timer_bit11_reserved_bit12_delivery_status, LVT_TIMER_MSR, LVT_BASE, reserved 11, legal 12);
boundary!(timer_bit8_reserved_bit7_vector, LVT_TIMER_MSR, LVT_BASE, reserved 8, legal 7);
boundary!(timer_bit31_reserved_bit17_timer_mode, LVT_TIMER_MSR, LVT_BASE, reserved 31, legal 17);
boundary!(timer_bit32_reserved_bit16_mask, LVT_TIMER_MSR, LVT_BASE, reserved 32, legal 16);
boundary!(timer_bit63_reserved_bit16_mask, LVT_TIMER_MSR, LVT_BASE, reserved 63, legal 16);

// Thermal LVT, Figure 16-14 p639: reserved 31:17, 15:13 ("Res"), 11; M 16,
// DS 12 (RO), MT 10:8, VEC 7:0. Bit 10 alone selects NMI, which Table 16-1
// p628 allows for the thermal source. DS is accepted and dropped (D2/U14).
boundary!(thermal_bit17_reserved_bit16_mask, LVT_THERMAL_MSR, LVT_BASE, reserved 17, legal 16);
boundary!(thermal_bit15_reserved_bit16_mask, LVT_THERMAL_MSR, LVT_BASE, reserved 15, legal 16);
boundary!(thermal_bit13_reserved_bit12_delivery_status, LVT_THERMAL_MSR, LVT_BASE, reserved 13, legal 12);
boundary!(thermal_bit11_reserved_bit12_delivery_status, LVT_THERMAL_MSR, LVT_BASE, reserved 11, legal 12);
boundary!(thermal_bit11_reserved_bit10_message_type, LVT_THERMAL_MSR, LVT_BASE, reserved 11, legal 10);
boundary!(thermal_bit32_reserved_bit16_mask, LVT_THERMAL_MSR, LVT_BASE, reserved 32, legal 16);
boundary!(thermal_bit63_reserved_bit16_mask, LVT_THERMAL_MSR, LVT_BASE, reserved 63, legal 16);

// Performance counter LVT, Figure 16-13 p638: same layout as thermal; NMI is
// allowed for this source (Table 16-1 p628). DS: D2/U14.
boundary!(perf_bit17_reserved_bit16_mask, LVT_PERF_MSR, LVT_BASE, reserved 17, legal 16);
boundary!(perf_bit15_reserved_bit16_mask, LVT_PERF_MSR, LVT_BASE, reserved 15, legal 16);
boundary!(perf_bit13_reserved_bit12_delivery_status, LVT_PERF_MSR, LVT_BASE, reserved 13, legal 12);
boundary!(perf_bit11_reserved_bit12_delivery_status, LVT_PERF_MSR, LVT_BASE, reserved 11, legal 12);
boundary!(perf_bit11_reserved_bit10_message_type, LVT_PERF_MSR, LVT_BASE, reserved 11, legal 10);
boundary!(perf_bit32_reserved_bit16_mask, LVT_PERF_MSR, LVT_BASE, reserved 32, legal 16);

// Error LVT, Figure 16-15 p639: same layout as thermal; NMI is allowed for
// this source (Table 16-1 p628). DS: D2/U14.
boundary!(error_bit17_reserved_bit16_mask, LVT_ERROR_MSR, LVT_BASE, reserved 17, legal 16);
boundary!(error_bit15_reserved_bit16_mask, LVT_ERROR_MSR, LVT_BASE, reserved 15, legal 16);
boundary!(error_bit13_reserved_bit12_delivery_status, LVT_ERROR_MSR, LVT_BASE, reserved 13, legal 12);
boundary!(error_bit11_reserved_bit12_delivery_status, LVT_ERROR_MSR, LVT_BASE, reserved 11, legal 12);
boundary!(error_bit11_reserved_bit10_message_type, LVT_ERROR_MSR, LVT_BASE, reserved 11, legal 10);
boundary!(error_bit32_reserved_bit16_mask, LVT_ERROR_MSR, LVT_BASE, reserved 32, legal 16);

// LINT0/LINT1 LVTs, Figure 16-12 p638: reserved 31:17, 13, 11; M 16, TGM 15,
// RIR 14 (RO), DS 12 (RO), MT 10:8, VEC 7:0. NMI (bit 10) is a legal LINT
// message type (Figure 16-7 p635). RIR/DS are accepted and dropped (D2/U14).
boundary!(lint0_bit17_reserved_bit16_mask, LVT_LINT0_MSR, LVT_BASE, reserved 17, legal 16);
boundary!(lint0_bit13_reserved_bit12_delivery_status, LVT_LINT0_MSR, LVT_BASE, reserved 13, legal 12);
boundary!(lint0_bit13_reserved_bit14_remote_irr, LVT_LINT0_MSR, LVT_BASE, reserved 13, legal 14);
boundary!(lint0_bit11_reserved_bit12_delivery_status, LVT_LINT0_MSR, LVT_BASE, reserved 11, legal 12);
boundary!(lint0_bit11_reserved_bit10_message_type, LVT_LINT0_MSR, LVT_BASE, reserved 11, legal 10);
boundary!(lint0_bit32_reserved_bit15_trigger_mode, LVT_LINT0_MSR, LVT_BASE, reserved 32, legal 15);
boundary!(lint1_bit17_reserved_bit16_mask, LVT_LINT1_MSR, LVT_BASE, reserved 17, legal 16);
boundary!(lint1_bit13_reserved_bit12_delivery_status, LVT_LINT1_MSR, LVT_BASE, reserved 13, legal 12);
boundary!(lint1_bit13_reserved_bit14_remote_irr, LVT_LINT1_MSR, LVT_BASE, reserved 13, legal 14);
boundary!(lint1_bit11_reserved_bit12_delivery_status, LVT_LINT1_MSR, LVT_BASE, reserved 11, legal 12);
boundary!(lint1_bit11_reserved_bit10_message_type, LVT_LINT1_MSR, LVT_BASE, reserved 11, legal 10);
boundary!(lint1_bit63_reserved_bit15_trigger_mode, LVT_LINT1_MSR, LVT_BASE, reserved 63, legal 15);

// SVR, Figure 16-17 p641: reserved 31:10 (bit 12 included: no EOI-broadcast
// suppression bit exists, D2), FCC 9, ASE 8, VEC 7:0; 63:32 by p659.
boundary!(svr_bit10_reserved_bit9_focus_checking, SVR_MSR, 0x1ff, reserved 10, legal 9);
boundary!(svr_bit12_reserved_bit9_focus_checking, SVR_MSR, 0x1ff, reserved 12, legal 9);
boundary!(svr_bit31_reserved_bit9_focus_checking, SVR_MSR, 0x1ff, reserved 31, legal 9);
boundary!(svr_bit32_reserved_bit9_focus_checking, SVR_MSR, 0x1ff, reserved 32, legal 9);
boundary!(svr_bit63_reserved_bit9_focus_checking, SVR_MSR, 0x1ff, reserved 63, legal 9);

// Divide configuration, Figure 16-11 p637: reserved 31:4 and 2; DV 3 and 1:0.
boundary!(divide_bit2_reserved_bit1_divide_value, DIVIDE_MSR, 0, reserved 2, legal 1);
boundary!(divide_bit2_reserved_bit3_divide_value, DIVIDE_MSR, 0, reserved 2, legal 3);
boundary!(divide_bit4_reserved_bit3_divide_value, DIVIDE_MSR, 0, reserved 4, legal 3);
boundary!(divide_bit31_reserved_bit0_divide_value, DIVIDE_MSR, 0, reserved 31, legal 0);
boundary!(divide_bit32_reserved_bit0_divide_value, DIVIDE_MSR, 0, reserved 32, legal 0);

// Initial count, Figure 16-10 p637: 31:0 is the count; 63:32 reserved (p659).
boundary!(initial_count_bit32_reserved_bit31_count, INITIAL_COUNT_MSR, 0, reserved 32, legal 31);
boundary!(initial_count_bit63_reserved_bit0_count, INITIAL_COUNT_MSR, 0, reserved 63, legal 0);

#[test]
fn reserved_masks_restate_the_figures() {
    // Self-check of this file's masks against the figures cited above.
    assert_eq!(timer_reserved(), 0xffff_ffff_fffc_ef00); // Figure 16-8 p636
    assert_eq!(thermal_reserved(), 0xffff_ffff_fffe_e800); // Figures 16-13/14/15
    assert_eq!(lint_reserved(), 0xffff_ffff_fffe_2800); // Figure 16-12 p638
    assert_eq!(svr_reserved(), 0xffff_ffff_ffff_fc00); // Figure 16-17 p641
    assert_eq!(divide_reserved(), 0xffff_ffff_ffff_fff4); // Figure 16-11 p637
    assert_eq!(initial_count_reserved(), 0xffff_ffff_0000_0000); // p659
    assert_eq!(logical_id(0x1b), 0x0001_0800); // 16.14 p662
}

#[test]
fn msr_numbers_follow_the_16_11_1_formula() {
    // Self-check: 16.11.1 p657, "x2APIC MSR address = 800h + ((APIC MMIO
    // offset) >> 4)", against the Table 16-6 p658 rows used in this file.
    let pairs = [
        (off::ID, 0x802),
        (off::VERSION, 0x803),
        (off::TPR, 0x808),
        (off::APR, APR_MSR),
        (off::PPR, 0x80a),
        (off::EOI, EOI_MSR),
        (off::LDR, 0x80d),
        (off::SVR, SVR_MSR),
        (off::ISR, 0x810),
        (off::TMR, 0x818),
        (off::IRR, 0x820),
        (off::ESR, ESR_MSR),
        (off::ICR_LOW, 0x830),
        (off::LVT_TIMER, LVT_TIMER_MSR),
        (off::LVT_THERMAL, LVT_THERMAL_MSR),
        (off::LVT_PERF, LVT_PERF_MSR),
        (off::LVT_LINT0, LVT_LINT0_MSR),
        (off::LVT_LINT1, LVT_LINT1_MSR),
        (off::LVT_ERROR, LVT_ERROR_MSR),
        (off::INITIAL_COUNT, INITIAL_COUNT_MSR),
        (off::CURRENT_COUNT, CURRENT_COUNT_MSR),
        (off::DIVIDE, DIVIDE_MSR),
    ];
    for (offset, msr) in pairs {
        assert_eq!(msr_of(offset), msr, "offset {offset:#x}");
        assert_eq!(offset_of(msr), offset, "MSR {msr:#x}");
    }
    // The exceptions: ICR high (310h) is merged into 830h, and the eliminated
    // RRR (C0h) and DFR (E0h) leave 80Ch and 80Eh unused.
    assert_eq!(msr_of(off::ICR_HIGH), 0x831);
    assert_eq!(msr_of(off::RRR), 0x80c);
    assert!(!listed(0x831) && !listed(0x80c) && !listed(0x80e));
}

/// Every single bit on top of `base`: a reserved bit faults without side
/// effects; any other bit is not a fault (16.11.3 p659).
fn sweep(msr: u32, base: u64, reserved: u64) {
    let mut vcpu = Vcpu::new();
    for bit in 0..64 {
        let value = base | (1u64 << bit);
        if reserved & (1u64 << bit) != 0 {
            vcpu.unchanged("sweep", |v| assert_eq!(v.write(msr, value), GP, "{msr:#x} bit {bit}"));
        } else {
            assert_ne!(vcpu.write(msr, value), GP, "{msr:#x} bit {bit}");
        }
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn lvt_reserved_bit_sweeps() {
    // Figures 16-8, 16-12, 16-13, 16-14, 16-15 (pp636-639); 16.11.3 p659.
    sweep(LVT_TIMER_MSR, LVT_BASE, timer_reserved());
    sweep(LVT_THERMAL_MSR, LVT_BASE, thermal_reserved());
    sweep(LVT_PERF_MSR, LVT_BASE, thermal_reserved());
    sweep(LVT_ERROR_MSR, LVT_BASE, thermal_reserved());
    sweep(LVT_LINT0_MSR, LVT_BASE, lint_reserved());
    sweep(LVT_LINT1_MSR, LVT_BASE, lint_reserved());
    // Masked bases: the same masks apply whatever the mask state.
    sweep(LVT_TIMER_MSR, LVT_BASE | MASK, timer_reserved());
    sweep(LVT_LINT1_MSR, 0x400 | MASK, lint_reserved());
}

#[test]
fn svr_divide_and_initial_count_reserved_bit_sweeps() {
    // Figure 16-17 p641, Figure 16-11 p637, Figure 16-10 p637; 16.11.3 p659.
    sweep(SVR_MSR, 0x1ff, svr_reserved());
    sweep(SVR_MSR, 0x0ff, svr_reserved());
    sweep(DIVIDE_MSR, 0, divide_reserved());
    sweep(INITIAL_COUNT_MSR, 0, initial_count_reserved());
}

// ---------------------------------------------------------------------------
// LVT message types and physical mirroring (D2/D3)
// ---------------------------------------------------------------------------

/// LVT value with message type `mt` (bits 10:8) and vector 0 (SMI "should be
/// set to 00h", NMI ignores it: Figure 16-7 p635).
fn lvt_type(mt: u64, masked: bool) -> u64 {
    (mt << 8) | if masked { MASK } else { 0 }
}

#[test]
fn thermal_perf_and_error_message_types_follow_table_16_1() {
    // Table 16-1 p628: the performance counter, thermal and APIC internal error
    // sources take "Fixed, SMI, or NMI". Figure 16-7 p635 defines only 000b,
    // 010b, 100b and 111b. The manual does not say fault or ignore for the
    // others (U3): D2 stops them as unsupported; ExtINT is not allowed for
    // these sources. D3: an unmasked SMI entry is refused (PPR p204: SMIs are
    // not intercepted once SmmLock is set); masked entries are mirrored.
    for msr in [LVT_THERMAL_MSR, LVT_PERF_MSR, LVT_ERROR_MSR] {
        let mut vcpu = Vcpu::new();
        for masked in [false, true] {
            assert_eq!(vcpu.write(msr, lvt_type(0, masked) | 0x30), WRITTEN, "{msr:#x} fixed");
            assert_eq!(vcpu.write(msr, lvt_type(4, masked)), WRITTEN, "{msr:#x} NMI");
            for mt in [1, 3, 5, 6, 7] {
                let value = lvt_type(mt, masked) | 0x30;
                vcpu.unchanged("reserved LVT type", |v| {
                    assert_eq!(
                        v.write(msr, value),
                        refused(Refusal::UnsupportedMessageType, value),
                        "{msr:#x} = {value:#x}"
                    )
                });
            }
        }
        assert_eq!(vcpu.write(msr, lvt_type(2, true)), WRITTEN, "{msr:#x} masked SMI");
        let smi = lvt_type(2, false);
        vcpu.unchanged("unmasked SMI", |v| {
            assert_eq!(v.write(msr, smi), refused(Refusal::UnmaskedSmi, smi), "{msr:#x}")
        });
        vcpu.lapic.assert_legal();
    }
}

#[test]
fn lint_message_types_follow_figure_16_7() {
    // Figure 16-7 p635: Fixed, SMI, NMI and External interrupt are the legal
    // LINT message types; TGM (bit 15) is meaningful for fixed (p638). D3:
    // unmasked SMI and unmasked ExtINT are stopped refusals; other encodings are
    // unsupported (D2/U3); NMI is mirrored (ACPI 6.6 5.2.12.7 and the captured
    // MADT: LINT1 carries NMI on every processor).
    for msr in [LVT_LINT0_MSR, LVT_LINT1_MSR] {
        let mut vcpu = Vcpu::new();
        for masked in [false, true] {
            for trigger in [0, 1 << 15] {
                let fixed = lvt_type(0, masked) | trigger | 0x31;
                assert_eq!(vcpu.write(msr, fixed), WRITTEN, "{msr:#x} = {fixed:#x}");
            }
            assert_eq!(vcpu.write(msr, lvt_type(4, masked)), WRITTEN, "{msr:#x} NMI");
            for mt in [1, 3, 5, 6] {
                let value = lvt_type(mt, masked) | 0x31;
                vcpu.unchanged("reserved LINT type", |v| {
                    assert_eq!(
                        v.write(msr, value),
                        refused(Refusal::UnsupportedMessageType, value),
                        "{msr:#x} = {value:#x}"
                    )
                });
            }
        }
        assert_eq!(vcpu.write(msr, lvt_type(2, true)), WRITTEN, "{msr:#x} masked SMI");
        assert_eq!(vcpu.write(msr, lvt_type(7, true)), WRITTEN, "{msr:#x} masked ExtINT");
        let smi = lvt_type(2, false);
        vcpu.unchanged("unmasked SMI", |v| {
            assert_eq!(v.write(msr, smi), refused(Refusal::UnmaskedSmi, smi), "{msr:#x}")
        });
        let extint = lvt_type(7, false);
        vcpu.unchanged("unmasked ExtINT", |v| {
            assert_eq!(v.write(msr, extint), refused(Refusal::UnmaskedExtInt, extint), "{msr:#x}")
        });
        vcpu.lapic.assert_legal();
    }
}

#[test]
fn accepted_lvt_writes_are_stored_and_mirrored_without_read_only_bits() {
    // D3: the validated guest value goes to the same physical MSR with the
    // read-only DS (bit 12) and RIR (bit 14) cleared (Figure 16-7 p635; D2/U14).
    // Values: periodic timer (TMM, Figure 16-8 p636), fixed thermal, NMI perf,
    // level-triggered fixed LINT0 (Figure 16-12 p638), LINT1 NMI as the captured
    // MADT describes (ACPI 5.2.12.7), fixed error.
    let mut vcpu = Vcpu::new();
    let cases = [
        (LVT_TIMER_MSR, 0x0002_0040 | DS, 0x0002_0040),
        (LVT_THERMAL_MSR, 0x41 | DS, 0x41),
        (LVT_PERF_MSR, 0x400 | DS, 0x400),
        (LVT_LINT0_MSR, 0x8043 | DS | RIR, 0x8043),
        (LVT_LINT1_MSR, 0x400 | RIR, 0x400),
        (LVT_ERROR_MSR, 0x45, 0x45),
    ];
    for (msr, written, stored) in cases {
        assert_eq!(vcpu.write(msr, written), WRITTEN, "{msr:#x}");
        assert_eq!(u64::from(reg(&vcpu.page, offset_of(msr))), stored, "virtual {msr:#x}");
        assert_eq!(vcpu.lapic.get(msr), stored, "physical {msr:#x}");
    }
    // Masked SMI / ExtINT entries are mirrored masked (D3).
    assert_eq!(vcpu.write(LVT_THERMAL_MSR, 0x1_0200), WRITTEN);
    assert_eq!(vcpu.lapic.get(LVT_THERMAL_MSR), 0x1_0200);
    assert_eq!(vcpu.write(LVT_LINT0_MSR, 0x1_0700), WRITTEN);
    assert_eq!(vcpu.lapic.get(LVT_LINT0_MSR), 0x1_0700);
    // The guest never programs the host-owned physical SVR (D3).
    assert!(!vcpu.lapic.wrote(SVR_MSR));
    vcpu.lapic.assert_legal();
}

#[test]
fn fixed_lvt_with_vector_below_16_is_not_a_fault() {
    // Figure 16-7 p635: "A value of 0 to 15 when the message type is fixed
    // results in an illegal vector APIC error" -- an APIC error, not #GP(0).
    // D3: the virtual entry keeps the value; the physical mirror is masked so
    // the host never takes the illegal vector.
    for msr in LVT_MSRS {
        for vector in [0u64, 1, 15] {
            let mut vcpu = Vcpu::new();
            assert_eq!(vcpu.write(msr, vector), WRITTEN, "{msr:#x} vector {vector}");
            assert_eq!(u64::from(reg(&vcpu.page, offset_of(msr))), vector, "virtual {msr:#x}");
            assert_eq!(vcpu.lapic.get(msr), vector | MASK, "physical {msr:#x}");
            vcpu.lapic.assert_legal();
        }
    }
    // Vector 16 is the first APIC-valid vector, but vectors 16-31 are
    // exception vectors (APM2 8.2 p245) that the host cannot accept: an
    // unmasked fixed entry with one is a stopped refusal (review F3). Vector
    // 32 is mirrored as written.
    let mut vcpu = Vcpu::new();
    assert_eq!(vcpu.write(LVT_TIMER_MSR, 16), refused(Refusal::ExceptionVector, 16));
    assert_eq!(vcpu.write(LVT_TIMER_MSR, 32), WRITTEN);
    assert_eq!(vcpu.lapic.get(LVT_TIMER_MSR), 32);
}

#[test]
fn timer_counts_and_divide_are_stored_and_mirrored_exactly() {
    // Figures 16-10/16-11 p637; 16.4.1 p636 (a count write starts the timer).
    // D2/D3: store and mirror exactly.
    let mut vcpu = Vcpu::new();
    for count in [0x1234_5678u64, 0xffff_ffff, 1, 0] {
        assert_eq!(vcpu.write(INITIAL_COUNT_MSR, count), WRITTEN);
        assert_eq!(u64::from(reg(&vcpu.page, off::INITIAL_COUNT)), count);
        assert_eq!(vcpu.lapic.get(INITIAL_COUNT_MSR), count);
        // The live count comes back from the physical timer (D2).
        assert_eq!(vcpu.read(CURRENT_COUNT_MSR), Emulation::Read(count));
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn all_eight_divide_encodings_are_accepted() {
    // Table 16-3 p638, "Bits 3, 1:0": 000b..111b = divide by 2, 4, 8, 16, 32,
    // 64, 128, 1, i.e. register values 0h-3h and 8h-Bh (bit 2 MBZ, Figure 16-11
    // p637).
    let mut vcpu = Vcpu::new();
    for (encoding, divisor) in [
        (0b000u64, 2),
        (0b001, 4),
        (0b010, 8),
        (0b011, 16),
        (0b100, 32),
        (0b101, 64),
        (0b110, 128),
        (0b111, 1),
    ] {
        let _: u32 = divisor;
        let value = ((encoding >> 2) << 3) | (encoding & 0b11);
        assert_eq!(vcpu.write(DIVIDE_MSR, value), WRITTEN, "encoding {encoding:03b}");
        assert_eq!(u64::from(reg(&vcpu.page, off::DIVIDE)), value);
        assert_eq!(vcpu.lapic.get(DIVIDE_MSR), value, "mirror of {value:#x}");
    }
    // The other eight 4-bit values set the reserved bit 2.
    for value in [0x4u64, 0x5, 0x6, 0x7, 0xc, 0xd, 0xe, 0xf] {
        vcpu.unchanged("divide bit 2", |v| {
            assert_eq!(v.write(DIVIDE_MSR, value), GP, "{value:#x}")
        });
    }
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// 3. SVR software disable forces the LVT masks
// ---------------------------------------------------------------------------

/// Unmasked guest LVT programming, one per entry.
const UNMASKED_LVTS: [(u32, u64); 6] = [
    (LVT_TIMER_MSR, 0x0002_0040),
    (LVT_THERMAL_MSR, 0x41),
    (LVT_PERF_MSR, 0x42),
    (LVT_LINT0_MSR, 0x43),
    (LVT_LINT1_MSR, 0x400),
    (LVT_ERROR_MSR, 0x45),
];

#[test]
fn software_disable_masks_every_virtual_and_physical_lvt() {
    // APM2 16.3.1 p629 and Figure 16-17 p641: with ASE (SVR bit 8) clear, "All
    // LVT entry mask bits are set and cannot be cleared." D2/D3: the backing LVTs
    // and the mirrored physical LVTs are masked; the SVR is stored as written.
    let mut vcpu = Vcpu::new();
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(vcpu.write(msr, value), WRITTEN);
        assert_eq!(vcpu.lapic.get(msr), value);
    }
    assert_eq!(vcpu.write(SVR_MSR, 0xff), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::SVR), 0xff);
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(u64::from(reg(&vcpu.page, offset_of(msr))), value | MASK, "virtual {msr:#x}");
        assert_eq!(vcpu.lapic.get(msr), value | MASK, "physical {msr:#x}");
    }
    assert!(!vcpu.lapic.wrote(SVR_MSR), "physical SVR stays host-owned (D3)");
    assert_eq!(vcpu.lapic.get(SVR_MSR), 0x1ff);
    vcpu.lapic.assert_legal();
}

#[test]
fn lvt_writes_while_software_disabled_stay_masked() {
    // 16.3.1 p629: the masks "cannot be cleared" while ASE=0. The other fields
    // stay writable (the manual is silent, U13; D2 forces only bit 16).
    let mut vcpu = Vcpu::new();
    assert_eq!(vcpu.write(SVR_MSR, 0x2ff), WRITTEN, "FCC set, ASE clear");
    assert_eq!(reg(&vcpu.page, off::SVR), 0x2ff);
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(vcpu.write(msr, value), WRITTEN, "{msr:#x}");
        assert_eq!(u64::from(reg(&vcpu.page, offset_of(msr))), value | MASK, "virtual {msr:#x}");
        assert_eq!(vcpu.lapic.get(msr), value | MASK, "physical {msr:#x}");
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn software_disable_leaves_timer_counts_and_divide_writable() {
    // 16.3.1 p629 and Figure 16-17 p641 list what ASE=0 changes: accepted
    // interrupt types and the LVT masks. The count and divide registers keep
    // their R/W behaviour (Figures 16-10/16-11 p637; D2 store and mirror).
    let mut vcpu = Vcpu::new();
    assert_eq!(vcpu.write(SVR_MSR, 0xff), WRITTEN);
    assert_eq!(vcpu.write(DIVIDE_MSR, 0xa), WRITTEN);
    assert_eq!(vcpu.write(INITIAL_COUNT_MSR, 0x0765_4321), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::DIVIDE), 0xa);
    assert_eq!(reg(&vcpu.page, off::INITIAL_COUNT), 0x0765_4321);
    assert_eq!(vcpu.lapic.get(DIVIDE_MSR), 0xa);
    assert_eq!(vcpu.lapic.get(INITIAL_COUNT_MSR), 0x0765_4321);
    assert_eq!(vcpu.write(ESR_MSR, 0), WRITTEN);
    vcpu.lapic.assert_legal();
}

#[test]
fn software_enable_keeps_masks_until_the_guest_rewrites_them() {
    // Setting ASE "enables the local APIC" (Figure 16-17 p641); whether masks
    // clear automatically is not stated (U13). Decision D2: they stay set until
    // the guest rewrites the entry.
    let mut vcpu = Vcpu::new();
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(vcpu.write(msr, value), WRITTEN);
    }
    assert_eq!(vcpu.write(SVR_MSR, 0xff), WRITTEN);
    assert_eq!(vcpu.write(SVR_MSR, 0x1ff), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::SVR), 0x1ff);
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(u64::from(reg(&vcpu.page, offset_of(msr))), value | MASK, "virtual {msr:#x}");
        assert_eq!(vcpu.lapic.get(msr), value | MASK, "physical {msr:#x}");
    }
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(vcpu.write(msr, value), WRITTEN);
        assert_eq!(u64::from(reg(&vcpu.page, offset_of(msr))), value, "virtual {msr:#x}");
        assert_eq!(vcpu.lapic.get(msr), value, "physical {msr:#x}");
    }
    vcpu.lapic.assert_legal();
}

#[test]
fn svr_vector_and_focus_bits_are_stored() {
    // Figure 16-17 p641: FCC (9), ASE (8) and VEC (7:0) are R/W; the APM puts
    // no restriction on the spurious vector value.
    let mut vcpu = Vcpu::new();
    for value in [0x100u64, 0x1ff, 0x3ff, 0x10f, 0x2ff, 0x000] {
        assert_eq!(vcpu.write(SVR_MSR, value), WRITTEN, "{value:#x}");
        assert_eq!(u64::from(reg(&vcpu.page, off::SVR)), value);
    }
    assert!(!vcpu.lapic.wrote(SVR_MSR));
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// 4. APIC_BASE (MSR 1Bh)
// ---------------------------------------------------------------------------

#[test]
fn apic_base_admission_requires_enabled_x2apic_at_the_reset_base() {
    // Figure 16-2 p630 (reset base FEE0_0000h, BSC is RO) and Table 16-5 p655
    // (AE=EXTD=1 is x2APIC mode); D4: the captured value must be enabled
    // x2APIC at FEE0_0000h.
    for base in [AP_BASE, BSP_BASE] {
        assert!(GuestX2Apic::admit(base, &policy(48)).is_ok(), "{base:#x}");
        assert_eq!(Vcpu::with(0x13, base, 48).read(APIC_BASE_MSR), Emulation::Read(base));
    }
    for base in [
        0xfee0_0800u64, // xAPIC mode
        0xfee0_0000,    // disabled
        0xfee0_0400,    // invalid AE=0, EXTD=1
        0xfed0_0c00,    // another base
        0x1_fee0_0c00,  // another base above 4 GiB
        0xfee0_0c01,    // reserved bit 0
        0xfee0_0e00,    // reserved bit 9
    ] {
        assert!(GuestX2Apic::admit(base, &policy(48)).is_err(), "{base:#x}");
    }
}

/// One intercepted APIC_BASE WRMSR on a fresh vCPU. Whatever the outcome, the
/// shadow, the backing page, the ledger and the physical x2APIC stay as they
/// were: the only completing writes are no-ops (D4).
fn base_write(base: u64, bits: u8, value: u64) -> Emulation {
    let mut vcpu = Vcpu::with(0x13, base, bits);
    let outcome = vcpu.unchanged("APIC_BASE write", |v| v.write(APIC_BASE_MSR, value));
    assert_eq!(vcpu.read(APIC_BASE_MSR), Emulation::Read(base), "shadow after {value:#x}");
    vcpu.lapic.assert_legal();
    outcome
}

#[test]
fn apic_base_read_returns_the_shadow() {
    // D4: RDMSR(1Bh) returns the captured enabled-x2APIC value (Figure 16-2).
    for base in [AP_BASE, BSP_BASE] {
        let mut vcpu = Vcpu::with(0x13, base, 48);
        vcpu.unchanged("APIC_BASE read", |v| {
            assert_eq!(v.read(APIC_BASE_MSR), Emulation::Read(base))
        });
    }
}

#[test]
fn apic_base_write_of_the_same_x2apic_state_completes() {
    // Figure 16-32 p656 has no self-loop; decision D4/U7: AE:EXTD=11 with an
    // unchanged base completes as a no-op. BSC (bit 8) is RO (Figure 16-2
    // p630): the written BSC is ignored (D4/U7).
    assert_eq!(base_write(AP_BASE, 48, AP_BASE), WRITTEN);
    assert_eq!(base_write(BSP_BASE, 48, BSP_BASE), WRITTEN);
    assert_eq!(base_write(AP_BASE, 48, AP_BASE | 0x100), WRITTEN, "AP writes BSC=1");
    assert_eq!(base_write(BSP_BASE, 48, BSP_BASE & !0x100), WRITTEN, "BSP writes BSC=0");
}

#[test]
fn apic_base_transition_to_xapic_or_the_invalid_mode_faults() {
    // Table 16-5 p655: AE=0/EXTD=1 "is invalid and causes the WRMSR instruction
    // to generate a #GP(0)". Figure 16-32 p656 / p655: from x2APIC mode the only
    // valid transition is to disabled; any other gives #GP(0), whatever the base.
    assert_eq!(base_write(AP_BASE, 48, 0xfee0_0800), GP, "11 -> 10");
    assert_eq!(base_write(BSP_BASE, 48, 0xfee0_0900), GP, "11 -> 10, BSC kept");
    assert_eq!(base_write(AP_BASE, 48, 0xfee0_0400), GP, "11 -> 01");
    assert_eq!(base_write(BSP_BASE, 48, 0xfee0_0500), GP, "11 -> 01, BSC kept");
    assert_eq!(base_write(AP_BASE, 48, 0xfed0_0800), GP, "11 -> 10 with another base");
    assert_eq!(base_write(AP_BASE, 48, 0x0400), GP, "11 -> 01 with base 0");
}

#[test]
fn apic_base_reserved_bits_fault() {
    // Figure 16-2 p630 / Figure 16-31 p655: bits 63:52, 9 and 7:0 are MBZ;
    // writing 1 to an MBZ bit makes WRMSR raise #GP (V3 WRMSR p511; D4). The
    // check applies whatever mode the value selects.
    for bit in (0..=7).chain([9, 52, 53, 62, 63]) {
        let value = AP_BASE | (1u64 << bit);
        assert_eq!(base_write(AP_BASE, 52, value), GP, "bit {bit}");
        assert_eq!(base_write(AP_BASE, 48, 0xfee0_0000 | (1u64 << bit)), GP, "disable + bit {bit}");
    }
}

#[test]
fn apic_base_bits_at_or_above_the_physical_width_fault() {
    // Figure 16-2 p630: "a given processor may implement a physical address
    // less than 52 bits in length"; D4 faults base bits at or above the
    // admitted width. The bit just below the width is a (refused) relocation.
    for bits in [40u8, 44, 48] {
        for bit in u32::from(bits)..52 {
            let value = AP_BASE | (1u64 << bit);
            assert_eq!(base_write(AP_BASE, bits, value), GP, "width {bits}, bit {bit}");
        }
        let below = AP_BASE | (1u64 << (bits - 1));
        assert_eq!(
            base_write(AP_BASE, bits, below),
            refused(Refusal::ApicRelocation, below),
            "width {bits}, bit {}",
            bits - 1
        );
    }
}

#[test]
fn apic_base_disable_is_a_documented_refusal() {
    // Figure 16-32 p656 and 16.9.1 p656 allow x2APIC -> disabled (AE=EXTD=0);
    // the exclusive profile stops instead (D4, documented deviation).
    assert_eq!(base_write(AP_BASE, 48, 0xfee0_0000), refused(Refusal::ApicDisable, 0xfee0_0000));
    assert_eq!(base_write(BSP_BASE, 48, 0xfee0_0100), refused(Refusal::ApicDisable, 0xfee0_0100));
    assert!(
        matches!(base_write(AP_BASE, 48, 0), Emulation::Refused { value: 0, .. }),
        "disable with base 0 is refused"
    );
}

#[test]
fn apic_base_relocation_is_a_documented_refusal() {
    // Figure 16-32 p656 has no x2APIC base-change transition and the manual
    // gives no rule (U7); decision D4: stopped unsupported refusal.
    for value in [0xfed0_0c00u64, 0x1_fee0_0c00, 0xfee0_1c00, 0x0000_1c00, 0xffff_f000_0c00] {
        assert_eq!(
            base_write(AP_BASE, 48, value),
            refused(Refusal::ApicRelocation, value),
            "{value:#x}"
        );
    }
    assert_eq!(base_write(BSP_BASE, 48, 0x1d00), refused(Refusal::ApicRelocation, 0x1d00));
}

// ---------------------------------------------------------------------------
// 5. Reset and INIT values
// ---------------------------------------------------------------------------

#[test]
fn guest_version_register_fields_match_figure_16_4() {
    // Figure 16-4 p632 and p633: VER (7:0) identifies a local APIC as 1Xh
    // (Table 16-2 p631: 10h); MLE (23:16) is the LVT entry count minus one; bits
    // 30:24 and 15:8 are MBZ; EAS (31) announces the extended space. The guest
    // has the six LVTs of Table 16-6 (832h-837h) and no extended space (U10).
    let v = GUEST_APIC_VERSION;
    assert_eq!(v & 0xff, 0x10, "VER");
    assert_eq!((v >> 16) & 0xff, 6 - 1, "MLE");
    assert_eq!(v & 0x7f00_ff00, 0, "reserved 30:24 and 15:8");
    assert_eq!(v >> 31, 0, "EAS");
}

/// Table 16-2 p631 ("the value of each register after reset and INIT") in
/// backing-page form for x2APIC ID `id`. ID and version are preserved (PPR p55:
/// INIT leaves ApicId unaffected); LDR is the derived logical x2APIC ID
/// (16.14 p661: hardware initializes it whenever x2APIC mode is enabled, and
/// INIT keeps the mode, 16.10 p657; decision D9/U5).
fn table_16_2(id: u32) -> Vec<(u16, u32, String)> {
    let mut v = vec![
        (off::ID, id, "x2APIC ID".to_string()),
        (off::VERSION, GUEST_APIC_VERSION, "version".into()),
        (off::TPR, 0, "TPR".into()),
        (off::PPR, 0, "PPR".into()),
        (off::LDR, logical_id(id), "LDR".into()),
        (off::SVR, 0xff, "SVR".into()),
        (off::ESR, 0, "ESR".into()),
        (off::ICR_LOW, 0, "ICR low".into()),
        (off::ICR_HIGH, 0, "ICR high".into()),
        (off::INITIAL_COUNT, 0, "initial count".into()),
        (off::DIVIDE, 0, "divide".into()),
    ];
    for lvt in off::LVTS {
        v.push((lvt, 0x0001_0000, format!("LVT {lvt:#x}")));
    }
    for bank in 0..8u16 {
        v.push((off::ISR + bank * 16, 0, format!("ISR bank {bank}")));
        v.push((off::TMR + bank * 16, 0, format!("TMR bank {bank}")));
        v.push((off::IRR + bank * 16, 0, format!("IRR bank {bank}")));
    }
    v
}

fn assert_table_16_2(page: &BackingPage, id: u32) {
    for (offset, value, name) in table_16_2(id) {
        assert_eq!(reg(page, offset), value, "{name} (offset {offset:#x}) of ID {id:#x}");
    }
}

#[test]
fn backing_page_reset_matches_table_16_2() {
    // Table 16-2 p631 (reset values), 16.14 p662 (derived LDR, e.g. ID 1Bh ->
    // cluster 1, logical bit 11), for every enabled MADT ID.
    for id in MADT_IDS {
        let mut page = BackingPage::new();
        page.reset_stopped(id, GUEST_APIC_VERSION).unwrap();
        assert_table_16_2(&page, id);
    }
}

/// Dirties every guest-visible register except the identity (ID, version and
/// LDR cannot be changed by the guest: their writes fault).
fn dirty(page: &BackingPage) {
    set(page, off::TPR, 0x5a);
    set(page, off::PPR, 0x7a);
    set(page, off::SVR, 0x3ff);
    set(page, off::ESR, 0x40);
    set(page, off::ICR_LOW, 0x000c_4055);
    set(page, off::ICR_HIGH, 0x1b);
    for (i, lvt) in off::LVTS.into_iter().enumerate() {
        set(page, lvt, 0x0002_0040 + i as u32);
    }
    set(page, off::INITIAL_COUNT, 0x10_0000);
    set(page, off::DIVIDE, 0xb);
    // Vectors 15:0 are reserved (16.6.3 p647), so every pattern keeps bank 0's
    // low 16 bits clear.
    for bank in 0..8u16 {
        set(page, off::ISR + bank * 16, 0x8001_0000);
        set(page, off::TMR + bank * 16, 0x4002_0000);
        set(page, off::IRR + bank * 16, 0x2004_0000);
    }
}

#[test]
fn init_reset_restores_table_16_2_on_a_dirty_page() {
    // Table 16-2 p631 and 16.10 p657 (INIT reinitializes every register except
    // APIC_BASE AE/EXTD); ID/version preserved and LDR derived (D9/U5).
    for id in MADT_IDS {
        let mut page = BackingPage::new();
        page.reset_stopped(id, GUEST_APIC_VERSION).unwrap();
        dirty(&page);
        page.reset_after_init_stopped().unwrap();
        assert_table_16_2(&page, id);
    }
}

#[test]
fn init_reset_zeroes_the_emulated_and_write_only_slots() {
    // Table 16-2 p631 lists APR (90h), Remote Read (C0h) and the current count
    // (390h) as 0 after INIT; EOI (B0h) has no Table 16-2 value, and the PPR
    // x2APIC EOI resets to 0 (PPR p175). The guest reads APR and the current
    // count through the VMM (D2), so those slots are not guest-visible; D9 step
    // 4 still lists "counts 0".
    let mut page = BackingPage::new();
    page.reset_stopped(0x1b, GUEST_APIC_VERSION).unwrap();
    for offset in [off::APR, off::EOI, off::RRR, off::CURRENT_COUNT] {
        set(&page, offset, 0x5a5a_0001);
    }
    page.reset_after_init_stopped().unwrap();
    for offset in [off::APR, off::EOI, off::RRR, off::CURRENT_COUNT] {
        assert_eq!(reg(&page, offset), 0, "slot {offset:#x}");
    }
}

#[test]
fn guest_init_resets_the_physical_timer_lvts_and_level_sources() {
    // D9 commit steps 1-4 against Table 16-2 p631: physical timer LVT 10000h,
    // initial count 0 (a zero count stops the timer, 16.4.1 p636), divide 0,
    // other mirrored LVTs 10000h; held level sources are retired with physical
    // EOIs; EOI acceleration returns (Table 15-22 p566); the backing page takes
    // its INIT values; APIC_BASE is untouched (16.10 p657).
    let mut vcpu = Vcpu::new();
    let mut map = Msrpm::new();
    map.configure_native_x2avic();
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(vcpu.write(msr, value), WRITTEN);
    }
    assert_eq!(vcpu.write(DIVIDE_MSR, 0xb), WRITTEN);
    assert_eq!(vcpu.write(INITIAL_COUNT_MSR, 0x10_0000), WRITTEN);
    set(&vcpu.page, off::TPR, 0x5a);
    set(&vcpu.page, off::PPR, 0x62);
    set(&vcpu.page, off::ICR_LOW, 0x000c_4055);
    set(&vcpu.page, off::ICR_HIGH, 0x1b);
    vcpu.hold_level(0x62);
    deliver_to_guest(&vcpu.page, 0x62);
    set_vector(&vcpu.page, off::ISR, 0x33);
    assert_eq!(vcpu.page.enqueue(0x71, false), Ok(true));
    assert!(map.update_x2apic_eoi_intercept(&vcpu.irq));
    let Emulation::Read(base) = vcpu.read(APIC_BASE_MSR) else { panic!("APIC_BASE read") };

    registers::prepare_init(&vcpu.page, &vcpu.irq, &mut vcpu.lapic).unwrap();
    registers::commit_init(&vcpu.page, &mut vcpu.irq, &mut vcpu.lapic, &mut map).unwrap();

    assert_table_16_2(&vcpu.page, 0x13);
    assert_eq!(vcpu.read(APR_MSR), Emulation::Read(0), "APR after INIT");
    assert_eq!(vcpu.read(CURRENT_COUNT_MSR), Emulation::Read(0), "current count after INIT");
    for msr in LVT_MSRS {
        assert_eq!(vcpu.lapic.get(msr), MASK, "physical {msr:#x}");
    }
    assert_eq!(vcpu.lapic.get(INITIAL_COUNT_MSR), 0);
    assert_eq!(vcpu.lapic.get(DIVIDE_MSR), 0);
    assert!(vcpu.lapic.in_service().is_empty(), "held level source acknowledged");
    assert_eq!(vcpu.lapic.eoi_count(), 1, "exactly one physical EOI per held source");
    assert!(vcpu.irq.is_empty());
    assert!(!map_bit(&map, EOI_MSR, true), "EOI accelerated again");
    assert!(!vcpu.lapic.wrote(SVR_MSR), "physical SVR stays host-owned");
    assert_eq!(vcpu.read(APIC_BASE_MSR), Emulation::Read(base));
    vcpu.lapic.assert_legal();
}

#[test]
fn guest_init_preparation_is_read_only_and_refuses_foreign_physical_in_service() {
    // D9 preparation: read-only, and the physical ISR (Figure 16-24 p649) must
    // hold only the ledger's level sources, or the retirement drain could
    // acknowledge a host interrupt it does not own.
    let mut vcpu = Vcpu::new();
    vcpu.hold_level(0x62);
    vcpu.unchanged("prepare_init", |v| {
        assert!(registers::prepare_init(&v.page, &v.irq, &mut v.lapic).is_ok())
    });
    vcpu.lapic.accept(0x50, false);
    vcpu.unchanged("prepare_init foreign ISR", |v| {
        assert!(registers::prepare_init(&v.page, &v.irq, &mut v.lapic).is_err())
    });
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// 7. ICR and AVIC_INCOMPLETE_IPI policy
// ---------------------------------------------------------------------------

/// The sending vCPU (MADT UID 15).
const SOURCE: u32 = 0x13;
const VECTOR: u8 = 0x55;
const MT_FIXED: u64 = 0;
const MT_SMI: u64 = 2;
const MT_NMI: u64 = 4;
const MT_INIT: u64 = 5;
const MT_STARTUP: u64 = 6;
const DSH_NONE: u64 = 0;
const DSH_SELF: u64 = 1;
const DSH_ALL: u64 = 2;
const DSH_OTHERS: u64 = 3;

/// x2APIC ICR, Figure 16-34 p661: DEST 63:32, DSH 19:18, TGM 15, L 14,
/// DM 11, MT 10:8, VEC 7:0.
#[derive(Clone, Copy)]
struct Icr {
    dest: u32,
    shorthand: u64,
    level_trigger: bool,
    assert: bool,
    logical: bool,
    mt: u64,
    vector: u8,
}

impl Icr {
    fn fixed(dest: u32) -> Self {
        Self {
            dest,
            shorthand: DSH_NONE,
            level_trigger: false,
            assert: false,
            logical: false,
            mt: MT_FIXED,
            vector: VECTOR,
        }
    }
    fn logical(dest: u32) -> Self {
        Self { logical: true, ..Self::fixed(dest) }
    }
    fn shorthand(self, shorthand: u64) -> Self {
        Self { shorthand, ..self }
    }
    fn mt(self, mt: u64, vector: u8) -> Self {
        Self { mt, vector, ..self }
    }
    fn value(self) -> u64 {
        (u64::from(self.dest) << 32)
            | (self.shorthand << 18)
            | (u64::from(self.level_trigger) << 15)
            | (u64::from(self.assert) << 14)
            | (u64::from(self.logical) << 11)
            | (self.mt << 8)
            | u64::from(self.vector)
    }
}

fn madt_owner() -> NativeIcr {
    NativeIcr::admit(SOURCE, &MADT_IDS).unwrap()
}

fn classify(owner: &NativeIcr, icr: u64, reason: u32) -> Result<IpiAction, IpiRefusal> {
    owner.inventory().classify(icr, reason)
}

fn is_fixed(action: Result<IpiAction, IpiRefusal>) -> bool {
    matches!(action, Ok(IpiAction::Fixed(ipi)) if ipi.vector() == VECTOR && ipi.targets() != 0)
}

#[test]
fn incomplete_ipi_id0_routes_by_message_type() {
    // Table 15-27 p581 ID 0 ("trigger mode ... level or the destination type is
    // unsupported"); D5: INIT/STARTUP go to the startup router, fixed edge and
    // NMI are delivered in software, level fixed and SMI are stopped (D10).
    // Table 16-4 p644: fixed edge ignores Level; INIT may be level with assert;
    // STARTUP ignores trigger and level; NMI takes any trigger with
    // "destination or all excluding self"; all take that destination form.
    let owner = madt_owner();
    let init = Icr::fixed(0x1b).mt(MT_INIT, 0);
    let sipi = Icr::fixed(0x1b).mt(MT_STARTUP, 0x9a);
    for shorthand in [DSH_NONE, DSH_OTHERS] {
        for logical in [false, true] {
            for icr in [init, sipi] {
                let icr = Icr { logical, ..icr.shorthand(shorthand) };
                assert_eq!(
                    classify(&owner, icr.value(), 0),
                    Ok(IpiAction::Startup),
                    "{:#x}",
                    icr.value()
                );
            }
        }
        let level_init = Icr { level_trigger: true, assert: true, ..init.shorthand(shorthand) };
        assert_eq!(classify(&owner, level_init.value(), 0), Ok(IpiAction::Startup));
        for (level_trigger, assert) in [(false, true), (true, false), (true, true)] {
            let s = Icr { level_trigger, assert, ..sipi.shorthand(shorthand) };
            assert_eq!(classify(&owner, s.value(), 0), Ok(IpiAction::Startup), "{:#x}", s.value());
        }
    }
    for assert in [false, true] {
        let edge = Icr { assert, ..Icr::fixed(0x1b) };
        assert!(is_fixed(classify(&owner, edge.value(), 0)), "fixed edge, L={assert}");
        let level = Icr { level_trigger: true, ..edge };
        assert_eq!(
            classify(&owner, level.value(), 0),
            Err(IpiRefusal::LevelTriggered),
            "L={assert}"
        );
    }
    assert_eq!(classify(&owner, Icr::fixed(0x1b).mt(MT_SMI, 0).value(), 0), Err(IpiRefusal::Smi));
    // NMI IPIs are delivered (Table 16-4 p644): a physical destination and the
    // all-excluding-self shorthand both resolve to V_NMI targets.
    assert_eq!(
        nmi_targets(classify(&owner, Icr::fixed(0x1b).mt(MT_NMI, 0).value(), 0)),
        madt_mask([0x1b])
    );
    assert_eq!(
        nmi_targets(classify(
            &owner,
            Icr::fixed(0x1b).mt(MT_NMI, 0).shorthand(DSH_OTHERS).value(),
            0
        )),
        madt_mask(MADT_IDS.into_iter().filter(|id| *id != SOURCE))
    );
    // Self and all-including-self shorthands are not admitted for NMI.
    assert_eq!(
        classify(&owner, Icr::fixed(0).mt(MT_NMI, 0).shorthand(DSH_SELF).value(), 0),
        Err(IpiRefusal::Nmi)
    );
    assert_eq!(
        classify(&owner, Icr::fixed(0).mt(MT_NMI, 0).shorthand(DSH_ALL).value(), 0),
        Err(IpiRefusal::Nmi)
    );
}

/// Target mask of a delivered NMI IPI classification.
fn nmi_targets(action: Result<IpiAction, IpiRefusal>) -> u32 {
    match action {
        Ok(IpiAction::Nmi(nmi)) => nmi.targets(),
        other => panic!("expected an NMI IPI: {other:?}"),
    }
}

#[test]
fn eliminated_icr_message_types_stop() {
    // 16.13 p661: "Message Type field (bits 10:8). Encodings 1, 3 and 7 are
    // eliminated and the encodings are reserved." The WRMSR has already
    // completed, so D5 stops with the ICR instead of #GP.
    let owner = madt_owner();
    for reason in [0, 2] {
        for mt in [1, 3, 7] {
            for icr in [Icr::fixed(0x1b), Icr::logical(1).shorthand(DSH_OTHERS)] {
                let value = icr.mt(mt, VECTOR).value();
                assert_eq!(
                    classify(&owner, value, reason),
                    Err(IpiRefusal::ReservedMessageType),
                    "{value:#x}"
                );
            }
        }
    }
}

#[test]
fn reserved_icr_bits_stop() {
    // Figure 16-34 p661: bits 31:20, 17:16 (eliminated remote read status) and
    // 13 are MBZ; D5 stops because #GP is impossible after completion. The DEST
    // half (63:32) is exempt from the 63:32 rule (16.11.3 p659).
    let owner = madt_owner();
    for reason in [0, 2] {
        for bit in (20..=31).chain([16, 17, 13]) {
            for icr in [Icr::fixed(0x1b), Icr::fixed(0x1b).mt(MT_INIT, 0)] {
                let value = icr.value() | (1u64 << bit);
                assert_eq!(
                    classify(&owner, value, reason),
                    Err(IpiRefusal::ReservedBits),
                    "bit {bit}"
                );
            }
        }
    }
}

#[test]
fn icr_delivery_status_bit_stops() {
    // 16.13 p661: "The Delivery Status field (bit 12) is eliminated and must be
    // zero" (Figure 16-34: 13:12 MBZ). D5 lists 13:12 among the reserved bits
    // that stop the incomplete IPI.
    let owner = madt_owner();
    for reason in [0, 2] {
        for icr in [Icr::fixed(0x1b), Icr::fixed(0x1b).mt(MT_INIT, 0)] {
            let value = icr.value() | (1 << 12);
            assert_eq!(
                classify(&owner, value, reason),
                Err(IpiRefusal::ReservedBits),
                "{value:#x}"
            );
        }
    }
}

#[test]
fn incomplete_ipi_id1_never_redelivers() {
    // 15.29.6.1 steps 5-6 pp576-577: hardware already set IRR in every valid
    // target and doorbelled the running ones before the ID 1 exit (Table 15-27
    // p581). D5: the target that is not running evaluates IRR at its first
    // VMRUN (15.29.8.3 p579), so the exit resumes and publishes nothing.
    let owner = madt_owner();
    for icr in [Icr::fixed(0x1b), Icr::logical(u32::MAX), Icr::fixed(0).shorthand(DSH_ALL)] {
        assert_eq!(
            classify(&owner, icr.value(), 1),
            Ok(IpiAction::Published),
            "{:#x}",
            icr.value()
        );
    }
    // Step 5 is reached only by a fixed IPI: any other ID 1 exit is refused.
    for icr in [Icr::fixed(0x1b).mt(MT_INIT, 0), Icr::fixed(0x1b).mt(MT_NMI, 0)] {
        assert_eq!(
            classify(&owner, icr.value(), 1),
            Err(IpiRefusal::TargetNotRunning),
            "{:#x}",
            icr.value()
        );
    }
}

#[test]
fn incomplete_ipi_id2_is_delivered_in_software() {
    // Table 15-27 p581 ID 2 ("Target is not covered by the physical or logical
    // ID table"); steps 3-4 exit before any IRR write of step 5 (p577). D5: full
    // software handling by message type.
    let owner = madt_owner();
    assert!(is_fixed(classify(&owner, Icr::fixed(0x1b).value(), 2)));
    assert_eq!(
        classify(&owner, Icr::fixed(0x1b).mt(MT_INIT, 0).value(), 2),
        Ok(IpiAction::Startup)
    );
    assert_eq!(
        classify(&owner, Icr::fixed(0x1b).mt(MT_STARTUP, 0x9a).value(), 2),
        Ok(IpiAction::Startup)
    );
    assert_eq!(
        nmi_targets(classify(&owner, Icr::fixed(0x1b).mt(MT_NMI, 0).value(), 2)),
        madt_mask([0x1b])
    );
    assert_eq!(classify(&owner, Icr::fixed(0x1b).mt(MT_SMI, 0).value(), 2), Err(IpiRefusal::Smi));
    let level = Icr { level_trigger: true, assert: true, ..Icr::fixed(0x1b) };
    assert_eq!(classify(&owner, level.value(), 2), Err(IpiRefusal::LevelTriggered));
}

#[test]
fn incomplete_ipi_id3_stops() {
    // Table 15-27 p581 ID 3: invalid backing page pointer in the physical ID
    // table. D5: stopped refusal.
    let owner = madt_owner();
    for icr in [Icr::fixed(0x1b), Icr::fixed(0).shorthand(DSH_ALL), Icr::fixed(0x1b).mt(MT_INIT, 0)]
    {
        assert_eq!(
            classify(&owner, icr.value(), 3),
            Err(IpiRefusal::InvalidBackingPage),
            "{:#x}",
            icr.value()
        );
    }
}

#[test]
fn incomplete_ipi_id4_drops_illegal_vectors() {
    // Table 15-27 p581 ID 4: "The vector for the specified IPI was set to an
    // illegal value (VEC < 16)"; an illegal vector is an APIC error, not a
    // fault (Figure 16-7 p635). D5: drop and record. INIT (vector field 0 by
    // definition) and STARTUP (a start routine below 10h) take the startup
    // router, as for IDs 0 and 2 (review F4). An ID 4 exit for any other ICR
    // contradicts the table: stop.
    let owner = madt_owner();
    for vector in 0..16u8 {
        for icr in [
            Icr::fixed(0x1b),
            Icr::logical(0x0001_0800),
            Icr::fixed(0).shorthand(DSH_SELF),
            Icr::fixed(0).shorthand(DSH_ALL),
            Icr::fixed(0).shorthand(DSH_OTHERS),
        ] {
            let value = icr.mt(MT_FIXED, vector).value();
            assert_eq!(
                classify(&owner, value, 4),
                Ok(IpiAction::Dropped(IpiDrop::IllegalVector)),
                "{value:#x}"
            );
        }
    }
    for icr in [Icr::fixed(0x1b).mt(MT_INIT, 0), Icr::fixed(0x1b).mt(MT_STARTUP, 0x09)] {
        assert_eq!(classify(&owner, icr.value(), 4), Ok(IpiAction::Startup), "{:#x}", icr.value());
    }
    for icr in [
        Icr::fixed(0x1b).mt(MT_FIXED, 16),
        Icr::fixed(0x1b).mt(MT_FIXED, 0xff),
        Icr::fixed(0x1b).mt(MT_NMI, 0),
    ] {
        assert_eq!(
            classify(&owner, icr.value(), 4),
            Err(IpiRefusal::InconsistentVectorExit),
            "{:#x}",
            icr.value()
        );
    }
}

#[test]
fn fixed_ipi_with_vector_below_16_is_dropped_in_software_delivery() {
    // Figure 16-7 p635 / Table 15-27 ID 4: vectors 0-15 are illegal; D5 drops
    // them in the software fan-out as for ID 4.
    let owner = madt_owner();
    for reason in [0, 2] {
        for vector in [0u8, 1, 15] {
            let value = Icr::fixed(0x1b).mt(MT_FIXED, vector).value();
            assert_eq!(
                classify(&owner, value, reason),
                Ok(IpiAction::Dropped(IpiDrop::IllegalVector)),
                "ID {reason} vector {vector}"
            );
        }
    }
}

#[test]
fn reserved_incomplete_ipi_ids_stop() {
    // Table 15-27 p581: ID 5 is Secure AVIC only; IDs above 5 are reserved.
    let owner = madt_owner();
    for reason in [5, 6, 7, 0x100, u32::MAX] {
        assert_eq!(
            classify(&owner, Icr::fixed(0x1b).value(), reason),
            Err(IpiRefusal::UnknownReason),
            "ID {reason}"
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Destination matching over the captured MADT inventory
// ---------------------------------------------------------------------------

fn slot_mask(ids: &[u32], wanted: impl IntoIterator<Item = u32>) -> u32 {
    wanted.into_iter().fold(0, |mask, id| mask | 1 << ids.iter().position(|x| *x == id).unwrap())
}

fn madt_mask(wanted: impl IntoIterator<Item = u32>) -> u32 {
    slot_mask(&MADT_IDS, wanted)
}

/// Target slots of a fixed edge IPI (software delivery, ID 2), or the drop.
fn fan(owner: &NativeIcr, icr: Icr) -> Result<u32, IpiDrop> {
    match classify(owner, icr.value(), 2) {
        Ok(IpiAction::Fixed(ipi)) => {
            assert_eq!(ipi.vector(), VECTOR);
            Ok(ipi.targets())
        }
        Ok(IpiAction::Dropped(reason)) => Err(reason),
        other => panic!("{:#x}: {other:?}", icr.value()),
    }
}

#[test]
fn physical_destination_matches_the_32_bit_id_exactly() {
    // 16.6.1 p645: the destination is compared with each APIC ID; 16.8 p654:
    // destinations are 32 bits in x2APIC mode (Figure 16-34 DEST 63:32).
    let owner = madt_owner();
    for id in MADT_IDS {
        assert_eq!(fan(&owner, Icr::fixed(id)), Ok(madt_mask([id])), "ID {id:#x}");
    }
    // The sender itself is a valid physical destination.
    assert_eq!(fan(&owner, Icr::fixed(SOURCE)), Ok(madt_mask([SOURCE])));
    // IDs 12-15, 28+ and 32-bit IDs whose low byte aliases an inventory ID.
    for dest in [0x0c, 0x0f, 0x1c, 0x20, 0x100, 0x11b, 0x0001_0013, 0x8000_0000] {
        assert_eq!(fan(&owner, Icr::fixed(dest)), Err(IpiDrop::NoTarget), "dest {dest:#x}");
    }
    // Slot order follows the admitted ID order.
    let ids = [0x1b, 0x02, 0x10];
    let small = NativeIcr::admit(0x1b, &ids).unwrap();
    assert_eq!(fan(&small, Icr::fixed(0x10)), Ok(1 << 2));
    assert_eq!(fan(&small, Icr::fixed(0x02)), Ok(1 << 1));
}

#[test]
fn physical_ffffffff_is_a_broadcast_including_self() {
    // 16.13 p660: "A DEST value of FFFF_FFFFh is used to broadcast IPIs to all
    // local APICs."
    let owner = madt_owner();
    assert_eq!(fan(&owner, Icr::fixed(u32::MAX)), Ok(madt_mask(MADT_IDS)));
}

#[test]
fn destination_ff_is_not_a_broadcast() {
    // FFh is the xAPIC broadcast (16.6.1 p645); whether it stays one in x2APIC
    // mode is not stated (U19). D5: only FFFF_FFFFh broadcasts; FFh is ID 255.
    let owner = madt_owner();
    assert_eq!(fan(&owner, Icr::fixed(0xff)), Err(IpiDrop::NoTarget));
    let ids = [0x00, 0x01, 0xff];
    let with_255 = NativeIcr::admit(0x00, &ids).unwrap();
    assert_eq!(fan(&with_255, Icr::fixed(0xff)), Ok(slot_mask(&ids, [0xff])));
}

#[test]
fn logical_destination_uses_cluster_and_logical_bits() {
    // 16.14 p662: a logical destination matches when bits 31:16 equal
    // LDR[31:16] (cluster) and any bit of 15:0 matches LDR[15:0]; LDR is
    // (ID[19:4] << 16) | (1 << ID[3:0]); flat logical mode does not exist.
    let owner = madt_owner();
    let cases: [(u32, Vec<u32>); 8] = [
        (0x0000_0001, vec![0x00]),
        (0x0000_0003, vec![0x00, 0x01]),
        (0x0000_ffff, (0x00..=0x0b).collect()),
        (0x0001_ffff, (0x10..=0x1b).collect()),
        (0x0001_0800, vec![0x1b]),
        (0x0001_0008, vec![SOURCE]),
        (0x0001_0101, vec![0x10, 0x18]),
        // FFh has no special meaning in x2APIC logical mode: cluster 0, bits 7:0.
        (0x0000_00ff, (0x00..=0x07).collect()),
    ];
    for (dest, ids) in cases {
        assert_eq!(
            fan(&owner, Icr::logical(dest)),
            Ok(madt_mask(ids.iter().copied())),
            "dest {dest:#x}"
        );
        for id in &ids {
            assert_eq!(logical_id(*id) >> 16, dest >> 16);
            assert_ne!(logical_id(*id) & dest & 0xffff, 0);
        }
    }
    for dest in [
        0x0000_0000u32, // no logical bit
        0x0001_0000,    // cluster 1, no logical bit
        0x0000_1000,    // cluster 0, bit 12: ID 12 is not enabled
        0x0000_f000,    // IDs 12-15 are not enabled
        0x0001_f000,    // IDs 28-31 are not enabled
        0x0002_ffff,    // cluster 2 is empty
        0x0100_0001,    // cluster 100h
        0xffff_fffe,    // cluster FFFFh, not the broadcast value
        0xfffe_ffff,    // cluster FFFEh
    ] {
        assert_eq!(fan(&owner, Icr::logical(dest)), Err(IpiDrop::NoTarget), "dest {dest:#x}");
    }
}

#[test]
fn logical_ffffffff_is_a_broadcast() {
    // 16.14 p662: "A DEST value of FFFF_FFFFh in the ICR is used to broadcast
    // IPIs to all local APICs."
    let owner = madt_owner();
    assert_eq!(fan(&owner, Icr::logical(u32::MAX)), Ok(madt_mask(MADT_IDS)));
}

#[test]
fn destination_shorthands_select_self_all_or_others() {
    // Figure 16-18 pp643-644: DSH 01b = self, 10b = all including self, 11b =
    // all excluding self; with 1xb "the destination mode is ignored and
    // physical is automatically used", and DEST is used only for 00b.
    let owner = madt_owner();
    let others = madt_mask(MADT_IDS.into_iter().filter(|id| *id != SOURCE));
    for dest in [0x1b, 0x00, u32::MAX, 0x0001_0800] {
        for base in [Icr::fixed(dest), Icr::logical(dest)] {
            assert_eq!(
                fan(&owner, base.shorthand(DSH_SELF)),
                Ok(madt_mask([SOURCE])),
                "self {dest:#x}"
            );
            assert_eq!(
                fan(&owner, base.shorthand(DSH_ALL)),
                Ok(madt_mask(MADT_IDS)),
                "all {dest:#x}"
            );
            assert_eq!(fan(&owner, base.shorthand(DSH_OTHERS)), Ok(others), "others {dest:#x}");
        }
    }
}

#[test]
fn empty_target_sets_are_dropped() {
    // A destination matching no APIC delivers nothing; the send-accept error
    // (Figure 16-16 p640) is not modeled (D5).
    let ids = [0x05];
    let alone = NativeIcr::admit(0x05, &ids).unwrap();
    assert_eq!(fan(&alone, Icr::fixed(0).shorthand(DSH_OTHERS)), Err(IpiDrop::NoTarget));
    assert_eq!(fan(&alone, Icr::fixed(0x06)), Err(IpiDrop::NoTarget));
    assert_eq!(fan(&alone, Icr::logical(0x0000_0040)), Err(IpiDrop::NoTarget));
    assert_eq!(fan(&alone, Icr::fixed(0).shorthand(DSH_SELF)), Ok(1));
    assert_eq!(fan(&alone, Icr::logical(0x0000_0020)), Ok(1));
}

// ---------------------------------------------------------------------------
// Software fan-out, doorbells and startup routing
// ---------------------------------------------------------------------------

fn madt_pages(ids: &[u32]) -> Vec<BackingPage> {
    ids.iter()
        .map(|id| {
            let mut page = BackingPage::new();
            page.reset_stopped(*id, GUEST_APIC_VERSION).unwrap();
            set(&page, off::SVR, 0x1ff);
            page
        })
        .collect()
}

/// Classifies `icr` (ID 2), fans it out and returns the doorbelled IDs.
fn deliver(owner: &NativeIcr, pages: &[BackingPage], icr: Icr) -> Result<Vec<u32>, FanOutError> {
    let ipi = match classify(owner, icr.value(), 2) {
        Ok(IpiAction::Fixed(ipi)) => ipi,
        other => panic!("{:#x}: {other:?}", icr.value()),
    };
    let rung = RefCell::new(Vec::new());
    owner.inventory().deliver_fixed(
        ipi,
        |slot| &pages[slot],
        |target| rung.borrow_mut().push(target.apic_id()),
    )?;
    Ok(rung.into_inner())
}

fn pending(pages: &[BackingPage], vector: u8) -> Vec<u32> {
    MADT_IDS
        .iter()
        .zip(pages)
        .filter(|(_, page)| page.is_pending(vector))
        .map(|(id, _)| *id)
        .collect()
}

#[test]
fn software_fan_out_sets_irr_clears_tmr_and_doorbells_remote_targets() {
    // 15.29.6.1 step 5 p577 (atomic IRR set per valid destination, doorbell to
    // the running ones); 16.6.3 p648 (TMR is reset for edge-triggered
    // interrupts); 15.29.8.3 p579 (VMRUN evaluates IRR, so the sender needs no
    // doorbell); D5.
    let owner = madt_owner();
    let pages = madt_pages(&MADT_IDS);
    let target = MADT_IDS.iter().position(|id| *id == 0x1b).unwrap();
    // A stale level TMR bit from a completed interrupt on one target.
    set_vector(&pages[target], off::TMR, VECTOR);

    let rung = deliver(&owner, &pages, Icr::fixed(0).shorthand(DSH_ALL)).unwrap();
    assert_eq!(pending(&pages, VECTOR), MADT_IDS.to_vec(), "every target pending");
    for page in &pages {
        assert!(!page.is_level(VECTOR), "edge publication clears TMR");
        assert!(!page.is_in_service(VECTOR));
    }
    let expected: Vec<u32> = MADT_IDS.into_iter().filter(|id| *id != SOURCE).collect();
    assert_eq!(rung, expected, "one doorbell per remote target, in slot order");
}

#[test]
fn self_directed_and_logical_fan_out_touch_only_their_targets() {
    // Figure 16-18 p644 (DSH self), 16.6.1 p645 (physical), 16.14 p662
    // (logical); 15.29.8.3 p579 (no doorbell to self); D5.
    let owner = madt_owner();

    let pages = madt_pages(&MADT_IDS);
    let rung = deliver(&owner, &pages, Icr::fixed(0x1b).shorthand(DSH_SELF)).unwrap();
    assert_eq!(pending(&pages, VECTOR), vec![SOURCE]);
    assert!(rung.is_empty(), "no doorbell for self");

    let pages = madt_pages(&MADT_IDS);
    let rung = deliver(&owner, &pages, Icr::fixed(0x1b)).unwrap();
    assert_eq!(pending(&pages, VECTOR), vec![0x1b]);
    assert_eq!(rung, vec![0x1b]);

    let pages = madt_pages(&MADT_IDS);
    let rung = deliver(&owner, &pages, Icr::logical(0x0001_0109)).unwrap();
    let mut expected = vec![0x10, 0x13, 0x18];
    assert_eq!(
        {
            let mut p = pending(&pages, VECTOR);
            p.sort();
            p
        },
        expected
    );
    expected.retain(|id| *id != SOURCE);
    assert_eq!(
        {
            let mut r = rung;
            r.sort();
            r
        },
        expected
    );

    let pages = madt_pages(&MADT_IDS);
    let rung = deliver(&owner, &pages, Icr::fixed(0).shorthand(DSH_OTHERS)).unwrap();
    let others: Vec<u32> = MADT_IDS.into_iter().filter(|id| *id != SOURCE).collect();
    assert_eq!(pending(&pages, VECTOR), others);
    assert_eq!(rung, others);
}

#[test]
fn fixed_ipi_is_not_accepted_by_a_software_disabled_target() {
    // 16.3.1 p629 (and ASE, Figure 16-17 p641): while ASE is clear, "Further
    // fixed, lowest-priority, and ExtInt interrupts are not accepted." A target
    // whose SVR bit 8 is clear (e.g. between INIT, Table 16-2 SVR=FFh, and the
    // OS enabling its APIC) must not get the fixed IPI in IRR. Brief D5 does not
    // address the target's software-enable state.
    let owner = madt_owner();
    let pages = madt_pages(&MADT_IDS);
    let disabled = MADT_IDS.iter().position(|id| *id == 0x1b).unwrap();
    set(&pages[disabled], off::SVR, 0xff);
    let _ = deliver(&owner, &pages, Icr::fixed(0).shorthand(DSH_ALL));
    assert!(!pages[disabled].is_pending(VECTOR), "software-disabled target accepted a fixed IPI");
    assert_eq!(
        pending(&pages, VECTOR).len(),
        MADT_IDS.len() - 1,
        "enabled targets still receive it"
    );
}

#[test]
fn device_interrupt_is_not_accepted_by_a_software_disabled_guest_apic() {
    // 16.3.1 p629: a software-disabled local APIC does not accept further fixed
    // interrupts; the bridged physical interrupt (D1) must not appear in the
    // guest IRR while the guest SVR has ASE clear.
    let mut vcpu = Vcpu::new();
    set(&vcpu.page, off::SVR, 0xff);
    vcpu.lapic.accept(0x41, false);
    let _ = irq::capture(0x41, &vcpu.page, &mut vcpu.irq, &mut vcpu.lapic);
    assert!(
        !vcpu.page.is_pending(0x41),
        "software-disabled guest APIC accepted a device interrupt"
    );
}

#[test]
fn doorbell_targets_are_bounded_to_254() {
    // Figure 15-22 p579: Doorbell Register bits 7:0 hold the physical APIC ID,
    // bits 63:8 are MBZ. PPR p216 has a 32-bit field; D8 admits IDs <= 254,
    // which satisfies both and excludes the unresolved entry-255 reservation
    // (Figure 15-18 p573, U7).
    for id in 0..=254u32 {
        assert_eq!(DoorbellTarget::new(id).map(DoorbellTarget::apic_id), Some(id), "ID {id}");
    }
    for id in [255u32, 256, 0x1fe, 0x0100_00fe, 0xffff_ff00, u32::MAX] {
        assert_eq!(DoorbellTarget::new(id), None, "ID {id:#x}");
    }
}

#[test]
fn fan_out_to_an_unringable_target_publishes_nothing() {
    // Figure 15-22 p579 / D8: a remote target whose ID cannot be written to
    // the doorbell must not be published to (either admission refuses the
    // inventory, or fan-out refuses before any IRR write).
    for ids in [[0x00u32, 0x13, 0xff], [0x00, 0x13, 0x113]] {
        let Ok(owner) = NativeIcr::admit(0x00, &ids) else { continue };
        let pages = madt_pages(&ids);
        let ipi = match classify(&owner, Icr::fixed(0).shorthand(DSH_ALL).value(), 2) {
            Ok(IpiAction::Fixed(ipi)) => ipi,
            other => panic!("{other:?}"),
        };
        let rung = RefCell::new(0);
        let result =
            owner.inventory().deliver_fixed(ipi, |slot| &pages[slot], |_| *rung.borrow_mut() += 1);
        assert_eq!(result, Err(FanOutError::DoorbellTarget { slot: 2, id: ids[2] }), "{ids:?}");
        assert!(pages.iter().all(|page| !page.is_pending(VECTOR)), "nothing published");
        assert_eq!(*rung.borrow(), 0, "nothing rung");
    }
}

fn madt_mailboxes() -> Vec<NativeStartupMailbox> {
    let boxes: Vec<NativeStartupMailbox> =
        MADT_IDS.iter().map(|id| NativeStartupMailbox::new(*id)).collect();
    for mailbox in &boxes {
        mailbox.mark_running();
    }
    boxes
}

#[test]
fn init_and_startup_ipis_reach_only_their_destinations() {
    // Figure 16-18 pp642-643 (INIT 101b, STARTUP 110b with the vector as the
    // start routine); Table 16-4 p644: "Destination or all excluding self";
    // D5: routed through the startup mailboxes.
    let boxes = madt_mailboxes();
    let mut owner = madt_owner();
    let target = MADT_IDS.iter().position(|id| *id == 0x1b).unwrap();
    let source = MADT_IDS.iter().position(|id| *id == SOURCE).unwrap();

    let init = Icr::fixed(0x1b).mt(MT_INIT, 0);
    assert_eq!(classify(&owner, init.value(), 0), Ok(IpiAction::Startup));
    let kicks = RefCell::new(0);
    owner.route_x2avic_startup(init.value(), &boxes, |_| *kicks.borrow_mut() += 1).unwrap();
    assert_eq!(*kicks.borrow(), 1);
    for (slot, mailbox) in boxes.iter().enumerate() {
        let expected = (slot == target).then_some(NativeStartupCommand::Init);
        assert_eq!(mailbox.peek(), expected, "slot {slot}");
    }
    boxes[target].complete(NativeStartupCommand::Init).unwrap();

    let sipi = Icr::fixed(0x1b).mt(MT_STARTUP, 0x9a);
    owner.route_x2avic_startup(sipi.value(), &boxes, |_| {}).unwrap();
    assert_eq!(boxes[target].peek(), Some(NativeStartupCommand::Sipi(0x9a)));
    assert!(boxes.iter().enumerate().all(|(s, b)| s == target || b.peek().is_none()));
    boxes[target].complete(NativeStartupCommand::Sipi(0x9a)).unwrap();

    let broadcast = Icr::fixed(0x1b).mt(MT_INIT, 0).shorthand(DSH_OTHERS);
    owner.route_x2avic_startup(broadcast.value(), &boxes, |_| {}).unwrap();
    for (slot, mailbox) in boxes.iter().enumerate() {
        let expected = (slot != source).then_some(NativeStartupCommand::Init);
        assert_eq!(mailbox.peek(), expected, "slot {slot}");
        if expected.is_some() {
            mailbox.complete(NativeStartupCommand::Init).unwrap();
        }
    }
}

#[test]
fn init_and_startup_with_self_or_all_including_self_are_not_delivered() {
    // Table 16-4 p644: INIT and STARTUP are valid only with "Destination or all
    // excluding self"; self (01b) and all-including-self (10b) are not valid
    // combinations (consequence unstated, U4), so nothing may be published.
    let boxes = madt_mailboxes();
    let mut owner = madt_owner();
    for icr in [Icr::fixed(0x1b).mt(MT_INIT, 0), Icr::fixed(0x1b).mt(MT_STARTUP, 0x9a)] {
        for shorthand in [DSH_SELF, DSH_ALL] {
            let value = icr.shorthand(shorthand).value();
            let result =
                owner.route_x2avic_startup(value, &boxes, |_| panic!("kick for {value:#x}"));
            assert!(result.is_err(), "{value:#x}");
            assert!(boxes.iter().all(|b| b.peek().is_none()), "{value:#x} published");
        }
    }
}

// ---------------------------------------------------------------------------
// 9. EOI and PPR
// ---------------------------------------------------------------------------

/// Checks PPR against 16.6.4 p651 / Figure 16-27: PP is the higher of TP and
/// the highest in-service priority class; PPS equals TPS when PP equals TP.
/// PPS for PP != TP is unstated (U20); the documented decision
/// (`BackingPage::eoi_stopped`) is zero.
fn assert_ppr(page: &BackingPage, what: &str) {
    let tpr = reg(page, off::TPR);
    let ppr = reg(page, off::PPR);
    let tp = tpr >> 4;
    let isr_class = vectors(page, off::ISR).last().map_or(0, |v| u32::from(*v) >> 4);
    let pp = tp.max(isr_class);
    assert_eq!(ppr >> 4, pp, "{what}: PP (TPR {tpr:#x}, PPR {ppr:#x})");
    if pp == tp {
        assert_eq!(ppr & 0xf, tpr & 0xf, "{what}: PPS = TPS");
    } else {
        assert_eq!(ppr & 0xf, 0, "{what}: PPS for PP != TP (U20 decision)");
    }
    assert_eq!(ppr >> 8, 0, "{what}: PPR 31:8 reserved (Figure 16-27)");
}

#[test]
fn intercepted_eoi_completes_only_the_highest_in_service_vector() {
    // 16.6.4 p652: an EOI write of zero "causes the local APIC to reset the
    // associated ISR bit" (the highest in service, 15.29.3.1 p569); PPR follows
    // pp650-651. D6: the level source behind that vector is completed, its
    // physical EOI is drained and its stale TMR bit cleared.
    let mut vcpu = Vcpu::new();
    set(&vcpu.page, off::TPR, 0x45);
    set_vector(&vcpu.page, off::ISR, 0x30);
    set_vector(&vcpu.page, off::ISR, 0x51);
    vcpu.hold_level(0x62);
    deliver_to_guest(&vcpu.page, 0x62);
    assert_eq!(vcpu.page.enqueue(0x20, false), Ok(true));

    assert_eq!(vcpu.write(EOI_MSR, 0), WRITTEN);
    assert_eq!(vectors(&vcpu.page, off::ISR), vec![0x30, 0x51]);
    assert_eq!(vectors(&vcpu.page, off::IRR), vec![0x20], "IRR untouched");
    assert_ppr(&vcpu.page, "after the level EOI");
    assert!(vcpu.irq.is_empty(), "level source completed");
    assert!(vcpu.lapic.in_service().is_empty(), "physical EOI drained");
    assert_eq!(vcpu.lapic.eoi_count(), 1);
    assert!(!vcpu.page.is_level(0x62), "stale TMR bit cleared (D6)");
    vcpu.lapic.assert_legal();
}

#[test]
fn intercepted_eoi_of_an_edge_vector_leaves_the_level_source_held() {
    // 16.6.4 p652: EOI resets the highest in-service bit only. A higher edge
    // vector nested over the level vector is completed first; the level source
    // stays held until its own EOI (D6).
    let mut vcpu = Vcpu::new();
    set(&vcpu.page, off::TPR, 0x00);
    vcpu.hold_level(0x62);
    deliver_to_guest(&vcpu.page, 0x62);
    set_vector(&vcpu.page, off::ISR, 0x70);

    assert_eq!(vcpu.write(EOI_MSR, 0), WRITTEN);
    assert_eq!(vectors(&vcpu.page, off::ISR), vec![0x62]);
    assert_ppr(&vcpu.page, "after the edge EOI");
    assert!(vcpu.irq.holds(0x62), "level source still held");
    assert_eq!(vcpu.lapic.eoi_count(), 0, "no physical EOI yet");
    assert!(vcpu.page.is_level(0x62));

    assert_eq!(vcpu.write(EOI_MSR, 0), WRITTEN);
    assert!(vectors(&vcpu.page, off::ISR).is_empty());
    assert_ppr(&vcpu.page, "after the level EOI");
    assert_eq!(reg(&vcpu.page, off::PPR), 0x00, "PPR equals TPR when nothing is in service");
    assert!(vcpu.irq.is_empty());
    assert_eq!(vcpu.lapic.eoi_count(), 1);
    vcpu.lapic.assert_legal();
}

#[test]
fn eoi_with_an_empty_isr_changes_nothing() {
    // The effect of an EOI with nothing in service is unstated (U18). D6: the
    // emulated EOI clears nothing and completes; the pending level source (not
    // yet taken by the guest) stays pending and held.
    let mut vcpu = Vcpu::new();
    set(&vcpu.page, off::TPR, 0x45);
    set(&vcpu.page, off::PPR, 0x45);
    vcpu.hold_level(0x62);
    let outcome = vcpu.unchanged("EOI with empty ISR", |v| v.write(EOI_MSR, 0));
    assert_eq!(outcome, WRITTEN);
    assert!(vcpu.page.is_pending(0x62));
    assert!(vcpu.irq.holds(0x62));
    vcpu.lapic.assert_legal();
}

#[test]
fn software_eoi_sequences_recompute_ppr() {
    // 16.6.4 pp650-652 via the backing-page EOI primitive: each EOI clears the
    // highest ISR bit and PPR follows TPR and the new highest in-service class.
    let cases: [(u32, &[u8], &[u8]); 5] = [
        // (TPR, ISR before, expected EOI order)
        (0x45, &[0x30, 0x51, 0x62], &[0x62, 0x51, 0x30]),
        (0x2a, &[0x21, 0x2f], &[0x2f, 0x21]),
        (0x7f, &[0x10, 0x80], &[0x80, 0x10]),
        (0x00, &[0xfe], &[0xfe]),
        (0x00, &[0x20, 0x9f, 0xff], &[0xff, 0x9f, 0x20]),
    ];
    for (tpr, isr, order) in cases {
        let mut page = BackingPage::new();
        page.reset_stopped(0x13, GUEST_APIC_VERSION).unwrap();
        set(&page, off::TPR, tpr);
        for v in isr {
            set_vector(&page, off::ISR, *v);
        }
        set_vector(&page, off::TMR, isr[isr.len() - 1]);
        assert_eq!(page.enqueue(0x1f, false), Ok(true));
        for (i, expected) in order.iter().enumerate() {
            assert_eq!(page.eoi_stopped(), Some(*expected), "TPR {tpr:#x}, EOI {i}");
            // The EOI leaves TMR to the level-source ledger.
            assert_eq!(page.is_level(*expected), i == 0, "TPR {tpr:#x}, EOI {i}");
            assert!(!page.is_in_service(*expected));
            assert_eq!(vectors(&page, off::ISR), {
                let mut rest: Vec<u8> = order[i + 1..].to_vec();
                rest.sort();
                rest
            });
            assert_ppr(&page, &format!("TPR {tpr:#x}, EOI {i}"));
            assert_eq!(vectors(&page, off::IRR), vec![0x1f], "IRR untouched");
        }
        let before = snapshot(&page);
        assert_eq!(page.eoi_stopped(), None, "EOI with empty ISR");
        assert_eq!(snapshot(&page), before);
    }
}

#[test]
fn level_eoi_exit_completes_whether_or_not_the_isr_bit_is_still_set() {
    // Table 15-29 p582: EXITINFO2[7:0] is the highest in-service vector of the
    // EOI write. Table 15-22 p566 calls the exit a trap, 15.29.9.2 p581 a fault
    // (U11), so the ISR bit may or may not still be set (D6 fallback).
    for hardware_cleared in [false, true] {
        let mut vcpu = Vcpu::new();
        set(&vcpu.page, off::TPR, 0x20);
        set_vector(&vcpu.page, off::ISR, 0x40);
        vcpu.hold_level(0x62);
        deliver_to_guest(&vcpu.page, 0x62);
        if hardware_cleared {
            clear_vector(&vcpu.page, off::ISR, 0x62);
        }
        irq::level_eoi_exit(0x62, 0x1000, &vcpu.page, &mut vcpu.irq, &mut vcpu.lapic).unwrap();
        assert_eq!(vectors(&vcpu.page, off::ISR), vec![0x40], "trap={hardware_cleared}");
        assert!(vcpu.irq.is_empty(), "trap={hardware_cleared}");
        assert_eq!(vcpu.lapic.eoi_count(), 1, "trap={hardware_cleared}");
        assert!(vcpu.lapic.in_service().is_empty());
        if !hardware_cleared {
            assert_ppr(&vcpu.page, "level EOI exit");
        }
        vcpu.lapic.assert_legal();
        // Only a still-set ISR bit leaves the fault reading open: the WRMSR at
        // this RIP may run again, so EOI writes stay intercepted until the
        // next one, which is a replay only at the same RIP.
        assert_eq!(vcpu.irq.intercepts_eoi(), !hardware_cleared);
        let mut other = vcpu.irq;
        assert_eq!(vcpu.irq.take_eoi_replay(0x1000), !hardware_cleared);
        assert!(!other.take_eoi_replay(0x2000), "another RIP is a nested handler's own EOI");
        assert!(!vcpu.irq.intercepts_eoi() && !other.intercepts_eoi(), "one EOI write disarms");
        assert!(!vcpu.irq.take_eoi_replay(0x1000));
    }
}

#[test]
fn level_eoi_exit_for_a_vector_that_is_not_the_highest_in_service_is_refused() {
    // Table 15-29 p582: the reported vector is "the highest in-service vector";
    // an exit naming a lower in-service vector contradicts the table.
    let mut vcpu = Vcpu::new();
    vcpu.hold_level(0x62);
    deliver_to_guest(&vcpu.page, 0x62);
    set_vector(&vcpu.page, off::ISR, 0x70);
    assert!(irq::level_eoi_exit(0x62, 0x1000, &vcpu.page, &mut vcpu.irq, &mut vcpu.lapic).is_err());
    assert!(vcpu.page.is_in_service(0x70), "the higher vector is not completed");
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// 10. APR
// ---------------------------------------------------------------------------

/// Figure 16-22 p647: AP is the highest of the TPR task priority, the highest
/// ISR class and the highest IRR class; APS is TPS "if the APR is equal to the
/// TPR, and zero otherwise" (read as AP == TP, D2).
fn expected_apr(tpr: u32, isr: Option<u8>, irr: Option<u8>) -> u32 {
    let tp = tpr >> 4;
    let class = |v: Option<u8>| v.map_or(0, |v| u32::from(v) >> 4);
    let ap = tp.max(class(isr)).max(class(irr));
    (ap << 4) | if ap == tp { tpr & 0xf } else { 0 }
}

#[test]
fn apr_reads_follow_figure_16_22() {
    // Worked cases (Figure 16-22 p647; vector class = vector / 16, p650).
    let cases: [(u32, &[u8], &[u8], u32); 11] = [
        (0x35, &[], &[], 0x35),
        (0x35, &[], &[0x61], 0x60),
        (0x35, &[0x42], &[0x21], 0x40),
        (0x72, &[0x70], &[], 0x72),
        (0x72, &[], &[0x7f], 0x72),
        (0x00, &[], &[], 0x00),
        (0x0f, &[], &[0x10], 0x10),
        (0x0f, &[], &[], 0x0f),
        (0xf3, &[0xe0], &[0xff], 0xf3),
        (0x10, &[0x2a], &[0x9b, 0x20], 0x90),
        (0x44, &[0x21, 0x3f], &[0x1f, 0x33], 0x44),
    ];
    for (tpr, isr, irr, expected) in cases {
        let mut vcpu = Vcpu::new();
        set(&vcpu.page, off::TPR, tpr);
        for v in isr {
            set_vector(&vcpu.page, off::ISR, *v);
        }
        for v in irr {
            set_vector(&vcpu.page, off::IRR, *v);
        }
        let outcome = vcpu.unchanged("APR read", |v| v.read(APR_MSR));
        assert_eq!(
            outcome,
            Emulation::Read(u64::from(expected)),
            "TPR {tpr:#x} ISR {isr:x?} IRR {irr:x?}"
        );
    }
}

#[test]
fn apr_sweep_over_priority_classes() {
    // Figure 16-22 p647 over TPR values and single ISR/IRR vectors in every
    // 32-bit bank.
    let vectors_under_test =
        [None, Some(0x10u8), Some(0x3f), Some(0x40), Some(0x72), Some(0xa5), Some(0xff)];
    for tpr in [0x00u32, 0x0f, 0x35, 0x72, 0xa9, 0xf3] {
        for isr in vectors_under_test {
            for irr in vectors_under_test {
                let mut vcpu = Vcpu::new();
                set(&vcpu.page, off::TPR, tpr);
                if let Some(v) = isr {
                    set_vector(&vcpu.page, off::ISR, v);
                }
                if let Some(v) = irr {
                    set_vector(&vcpu.page, off::IRR, v);
                }
                assert_eq!(
                    vcpu.read(APR_MSR),
                    Emulation::Read(u64::from(expected_apr(tpr, isr, irr))),
                    "TPR {tpr:#x} ISR {isr:x?} IRR {irr:x?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 11. Physical APIC ID table entries
// ---------------------------------------------------------------------------

const ENTRY_VALID: u64 = 1 << 63; // Table 15-23 p572, V
const ENTRY_RUNNING: u64 = 1 << 62; // Table 15-23 p572, IR
const ENTRY_RESERVED: u64 = 0x3ff << 52; // 61:52, "Reserved, SBZ"
const ENTRY_BACKING: u64 = 0x000f_ffff_ffff_f000; // 51:12, 4K-aligned HPA
const ENTRY_HOST_ID: u64 = 0xfff; // 11:0, host physical APIC ID

#[test]
fn physical_id_table_entries_follow_figure_15_17() {
    // Figure 15-17 / Table 15-23 p572. Guest APIC ID = host x2APIC ID (one
    // table per VM, indexed by the guest physical APIC ID, 15.29.5.2 p571).
    let policy = policy(48);
    let mut table = PhysicalIdTable::new();
    let backing = |slot: usize| 0x7fff_f000_0000u64 + (slot as u64) * 0x1000;
    for (slot, id) in MADT_IDS.into_iter().enumerate() {
        table.insert_stopped(id as u16, backing(slot), &policy).unwrap();
    }
    for (slot, id) in MADT_IDS.into_iter().enumerate() {
        let entry = table.entry(id as u16).unwrap();
        assert_ne!(entry & ENTRY_VALID, 0, "V of {id:#x}");
        assert_eq!(entry & ENTRY_RESERVED, 0, "reserved 61:52 of {id:#x}");
        assert_eq!(entry & ENTRY_BACKING, backing(slot), "backing of {id:#x}");
        assert_eq!(entry & ENTRY_HOST_ID, u64::from(id), "host APIC ID of {id:#x}");
    }
    // IDs 12-15 have no processor: V clear, "no information".
    for id in 12..16u16 {
        assert_eq!(table.entry(id).unwrap() & ENTRY_VALID, 0, "ID {id}");
    }
}

#[test]
fn is_running_toggles_only_bit_62_of_its_entry() {
    // Table 15-23 p572: IR (bit 62) marks the vCPU as scheduled on a physical
    // core; it does not alter the other fields.
    let policy = policy(48);
    let mut table = PhysicalIdTable::new();
    table.insert_stopped(0x1b, 0x1234_5000, &policy).unwrap();
    table.insert_stopped(0x13, 0x1234_6000, &policy).unwrap();
    let other = table.entry(0x13).unwrap();
    let base = table.entry(0x1b).unwrap() & !ENTRY_RUNNING;
    table.set_running(0x1b, true).unwrap();
    assert_eq!(table.entry(0x1b).unwrap(), base | ENTRY_RUNNING);
    table.set_running(0x1b, false).unwrap();
    assert_eq!(table.entry(0x1b).unwrap(), base);
    table.set_running(0x1b, true).unwrap();
    assert_eq!(table.entry(0x13).unwrap(), other, "other entries unchanged");
}

#[test]
fn physical_id_table_holds_512_entries_and_12_bit_host_ids() {
    // 15.29.5.2 p571: without x2AVIC_EXT the table is one 4-Kbyte page, "a
    // maximum of 512 virtual processors"; the host APIC ID field is 11:0
    // (Table 15-23 p572).
    let policy = policy(48);
    let mut table = PhysicalIdTable::new();
    table.insert_stopped(511, 0x2000_0000, &policy).unwrap();
    let entry = table.entry(511).unwrap();
    assert_eq!(entry & ENTRY_HOST_ID, 0x1ff);
    assert_eq!(entry & ENTRY_BACKING, 0x2000_0000);
    assert!(table.entry(512).is_err());
    assert!(table.insert_stopped(512, 0x2000_1000, &policy).is_err());
    assert!(table.set_running(512, true).is_err());
}

#[test]
fn physical_id_table_backing_pointer_must_be_a_legal_4k_address() {
    // Table 15-23 p572: "4-Kbyte aligned HPA"; 15.29.4.3 p571: pointers must be
    // inside the implemented physical address range; bits 61:52 SBZ.
    let policy = policy(40);
    for backing in [0x1234_5001u64, 0x1234_5800, 1 << 40, 1 << 51, 1 << 52, 1 << 62, 1 << 63] {
        let mut table = PhysicalIdTable::new();
        assert!(table.insert_stopped(0x1b, backing, &policy).is_err(), "{backing:#x}");
        assert_eq!(table.entry(0x1b).unwrap() & ENTRY_VALID, 0, "{backing:#x} left no entry");
    }
    let mut table = PhysicalIdTable::new();
    table.insert_stopped(0x1b, (1 << 40) - 0x1000, &policy).unwrap();
    assert_eq!(table.entry(0x1b).unwrap() & ENTRY_BACKING, (1 << 40) - 0x1000);
}

// ---------------------------------------------------------------------------
// 12. AVIC exit decoding
// ---------------------------------------------------------------------------

const AVIC_INCOMPLETE_IPI: u64 = 0x401; // Table C-1 p758
const AVIC_NOACCEL: u64 = 0x402; // Table C-1 p758

#[test]
fn incomplete_ipi_exit_fields_follow_tables_15_25_to_15_27() {
    // Table 15-25 p580: EXITINFO1 63:32 = ICRH, 31:0 = ICRL. Table 15-26 p580:
    // EXITINFO2 63:32 = ID, 11:0 = index "For ID = 1 - 3 ... Reserved for all
    // other ID values."
    for icr in [0x0000_001b_000c_4055u64, 0xffff_ffff_0000_08ff, 0x0000_0000_0000_0500, u64::MAX] {
        for id in 1..=3u64 {
            for index in [0u64, 0x1b, 0x7ff, 0xfff] {
                assert_eq!(
                    AvicExit::decode(AVIC_INCOMPLETE_IPI, icr, (id << 32) | index),
                    Ok(AvicExit::IncompleteIpi {
                        icr,
                        reason: id as u32,
                        index: Some(index as u16)
                    }),
                    "ICR {icr:#x} ID {id} index {index:#x}"
                );
            }
        }
        for id in [0u64, 4] {
            assert_eq!(
                AvicExit::decode(AVIC_INCOMPLETE_IPI, icr, (id << 32) | 0xabc),
                Ok(AvicExit::IncompleteIpi { icr, reason: id as u32, index: None }),
                "ICR {icr:#x} ID {id}"
            );
        }
    }
}

#[test]
fn incomplete_ipi_index_is_bits_11_to_0_only() {
    // Figure 15-24 p580: EXITINFO2 bits 31:12 are reserved; the index is 11:0.
    assert_eq!(
        AvicExit::decode(AVIC_INCOMPLETE_IPI, 0x1b_0000_0055, (2 << 32) | 0xffff_f123),
        Ok(AvicExit::IncompleteIpi { icr: 0x1b_0000_0055, reason: 2, index: Some(0x123) })
    );
}

#[test]
fn secure_avic_and_reserved_incomplete_ipi_ids_are_not_decoded() {
    // Table 15-27 p581: ID 5 is "Un-accelerated IPI ... (Secure AVIC)" and IDs
    // above 5 are reserved; neither applies to this non-SEV profile.
    for id in [5u64, 6, 0x100, 0xffff_ffff] {
        assert!(AvicExit::decode(AVIC_INCOMPLETE_IPI, 0x55, id << 32).is_err(), "ID {id}");
    }
}

#[test]
fn noaccel_exit_fields_follow_tables_15_28_and_15_29() {
    // Table 15-28 p582: bit 32 = R/W (set for write), 11:4 = APIC_Offset[11:4]
    // with APIC_Offset[3:0] = 0. Table 15-29 p582: for a write to EOI (B0h),
    // EXITINFO2[7:0] is the highest in-service vector; otherwise undefined.
    for offset in (0..0x1000u16).step_by(16) {
        let info1 = u64::from(offset);
        assert_eq!(
            AvicExit::decode(AVIC_NOACCEL, info1, u64::MAX),
            Ok(AvicExit::NoAcceleration { offset, write: false, eoi_vector: None }),
            "read of {offset:#x}"
        );
        let eoi_vector = (offset == off::EOI).then_some(0x62);
        assert_eq!(
            AvicExit::decode(AVIC_NOACCEL, (1 << 32) | info1, 0x62),
            Ok(AvicExit::NoAcceleration { offset, write: true, eoi_vector }),
            "write of {offset:#x}"
        );
    }
}

#[test]
fn noaccel_eoi_vector_is_bits_7_to_0_only() {
    // Figure 15-26 / Table 15-29 p582: EXITINFO2 63:8 are reserved. (Vectors
    // 15:0 cannot be in service, 16.6.3 p647, so only 16-255 are used here.)
    for (info2, vector) in
        [(0xffff_ffff_ffff_ff62u64, 0x62u8), (0x1_0000_00ff, 0xff), (0x110, 0x10)]
    {
        assert_eq!(
            AvicExit::decode(AVIC_NOACCEL, (1 << 32) | 0xb0, info2),
            Ok(AvicExit::NoAcceleration { offset: 0xb0, write: true, eoi_vector: Some(vector) }),
            "{info2:#x}"
        );
    }
}

#[test]
fn other_exit_codes_are_not_avic_exits() {
    // Table C-1 pp756-758: only 401h and 402h are AVIC exits (400h is
    // VMEXIT_NPF, 403h VMEXIT_VMGEXIT, 7Ch VMEXIT_MSR, -1 VMEXIT_INVALID).
    for code in [0x400u64, 0x403, 0x7c, 0x0, u64::MAX] {
        assert!(AvicExit::decode(code, (1 << 32) | 0xb0, 0x62).is_err(), "code {code:#x}");
    }
}

// ---------------------------------------------------------------------------
// Adjacent rules: physical IRQ bridge, arm-time capture, admission, VMCB
// ---------------------------------------------------------------------------

#[test]
fn physical_spurious_interrupt_is_neither_published_nor_acknowledged() {
    // 16.4.7 p640: a spurious interrupt carries the SVR vector (Figure 16-17
    // p641; the host SVR is 1FFh, D3); "The ISR is unaffected by the spurious
    // interrupt, so the interrupt handler completes without sending an EOI".
    let mut vcpu = Vcpu::new();
    let result =
        vcpu.unchanged("spurious", |v| irq::capture(0xff, &v.page, &mut v.irq, &mut v.lapic));
    assert_eq!(result, Ok(None));
    vcpu.lapic.assert_legal();
}

#[test]
fn physical_edge_interrupt_is_published_then_acknowledged() {
    // 16.6.3 p648: an accepted edge interrupt sets IRR and resets its TMR bit;
    // the physical EOI (16.6.4 p652) completes it at the host (D1 IRQ bridge).
    let mut vcpu = Vcpu::new();
    set_vector(&vcpu.page, off::TMR, 0x41); // stale bit of a completed level source
    vcpu.lapic.accept(0x41, false);
    assert_eq!(
        irq::capture(0x41, &vcpu.page, &mut vcpu.irq, &mut vcpu.lapic),
        Ok(Some(Capture::Edge))
    );
    assert!(vcpu.page.is_pending(0x41));
    assert!(!vcpu.page.is_level(0x41));
    assert_eq!(vcpu.lapic.eoi_count(), 1);
    assert!(vcpu.lapic.in_service().is_empty());
    assert!(vcpu.irq.is_empty());
    vcpu.lapic.assert_legal();
}

#[test]
fn physical_level_interrupt_is_published_with_tmr_and_held() {
    // 16.6.3 p648: TMR is set for level-sensitive interrupts, and the EOI of a
    // TMR-marked interrupt completes it at the source; D6 holds the physical EOI
    // until the guest EOI.
    let mut vcpu = Vcpu::new();
    vcpu.lapic.accept(0x62, true);
    assert_eq!(
        irq::capture(0x62, &vcpu.page, &mut vcpu.irq, &mut vcpu.lapic),
        Ok(Some(Capture::Level))
    );
    assert!(vcpu.page.is_pending(0x62));
    assert!(vcpu.page.is_level(0x62));
    assert!(vcpu.irq.holds(0x62));
    assert_eq!(vcpu.lapic.eoi_count(), 0);
    assert_eq!(vcpu.lapic.in_service(), vec![0x62]);
    vcpu.lapic.assert_legal();
}

#[test]
fn accepted_vector_without_physical_in_service_state_is_refused() {
    // 16.6.3 p647: ExtINT (and other non-fixed types) go "directly to the CPU
    // core" without ISR state, so a non-spurious vector with no physical ISR
    // bit cannot be owned by the bridge (IrqError::NotInService contract).
    let mut vcpu = Vcpu::new();
    let result =
        vcpu.unchanged("no ISR bit", |v| irq::capture(0x41, &v.page, &mut v.irq, &mut v.lapic));
    assert_eq!(result, Err(IrqError::NotInService(0x41)));
    vcpu.lapic.assert_legal();
}

/// Loader-programmed physical x2APIC as the resident host finds it.
fn loader_lapic() -> Lapic {
    let mut lapic = Lapic::host();
    lapic.reg.insert(0x808, 0x2a);
    lapic.reg.insert(LVT_TIMER_MSR, 0x0002_00ef);
    lapic.reg.insert(LVT_THERMAL_MSR, MASK);
    lapic.reg.insert(LVT_PERF_MSR, 0x400);
    lapic.reg.insert(LVT_LINT0_MSR, MASK | 0x700);
    lapic.reg.insert(LVT_LINT1_MSR, 0x400 | DS);
    lapic.reg.insert(LVT_ERROR_MSR, MASK | 0xfe);
    lapic.reg.insert(INITIAL_COUNT_MSR, 0x0001_2345);
    lapic.reg.insert(CURRENT_COUNT_MSR, 0x0000_2345);
    lapic.reg.insert(DIVIDE_MSR, 0xb);
    lapic
}

#[test]
fn captured_loader_state_is_installed_with_ppr_equal_to_tpr() {
    // 16.6.4 p651 / Figures 16-26, 16-27: with nothing in service PP = TP and
    // PPS = TPS, so PPR equals TPR. LVT read-only bits are not stored (D2/U14).
    // Counts are not rewritten: a count write restarts the timer (16.4.1 p636).
    let mut lapic = loader_lapic();
    let icr = 0x0000_001b_0000_40fd;
    let captured = CapturedInterface::capture(&mut lapic, icr).unwrap();
    assert_eq!(captured.task_priority(), 0x2a);
    let mut page = BackingPage::new();
    page.reset_stopped(0x13, GUEST_APIC_VERSION).unwrap();
    captured.install(&page, &mut lapic);

    assert_eq!(reg(&page, off::TPR), 0x2a);
    assert_eq!(reg(&page, off::PPR), 0x2a);
    assert_eq!(reg(&page, off::SVR), 0x1ff);
    for (offset, value) in [
        (off::LVT_TIMER, 0x0002_00ef),
        (off::LVT_THERMAL, 0x0001_0000),
        (off::LVT_PERF, 0x400),
        (off::LVT_LINT0, 0x0001_0700),
        (off::LVT_LINT1, 0x400),
        (off::LVT_ERROR, 0x0001_00fe),
    ] {
        assert_eq!(reg(&page, offset), value, "LVT {offset:#x}");
    }
    assert_eq!(reg(&page, off::INITIAL_COUNT), 0x0001_2345);
    assert_eq!(reg(&page, off::DIVIDE), 0xb);
    assert_eq!(reg(&page, off::ICR_LOW), 0x40fd);
    assert_eq!(reg(&page, off::ICR_HIGH), 0x1b);
    for msr in [0x808, SVR_MSR, INITIAL_COUNT_MSR, DIVIDE_MSR] {
        assert!(!lapic.wrote(msr), "capture/install wrote {msr:#x}");
    }
    assert_eq!(lapic.get(CURRENT_COUNT_MSR), 0x2345, "timer not restarted");
    lapic.assert_legal();
}

#[test]
fn captured_software_disabled_state_masks_every_lvt() {
    // 16.3.1 p629: with ASE (SVR bit 8) clear, "All LVT entry mask bits are set
    // and cannot be cleared" - virtually and in the physical mirror (D3).
    let mut lapic = Lapic::host();
    lapic.reg.insert(SVR_MSR, 0xff);
    for (msr, value) in UNMASKED_LVTS {
        lapic.reg.insert(msr, value);
    }
    let captured = CapturedInterface::capture(&mut lapic, 0).unwrap();
    let mut page = BackingPage::new();
    page.reset_stopped(0x13, GUEST_APIC_VERSION).unwrap();
    captured.install(&page, &mut lapic);
    assert_eq!(reg(&page, off::SVR), 0xff);
    for (msr, value) in UNMASKED_LVTS {
        assert_eq!(u64::from(reg(&page, offset_of(msr))), value | MASK, "virtual {msr:#x}");
        assert_eq!(lapic.get(msr), value | MASK, "physical {msr:#x}");
    }
    lapic.assert_legal();
}

#[test]
fn captured_values_with_reserved_bits_are_refused() {
    // 16.11.3 p659: "The RDMSR instruction returns a zero for any reserved bit",
    // and a guest WRMSR of such a value faults, so it cannot be presented.
    for (msr, value) in [
        (LVT_TIMER_MSR, 0x40 | (1u64 << 18)),
        (LVT_THERMAL_MSR, 0x41 | (1 << 11)),
        (LVT_LINT0_MSR, 0x43 | (1 << 13)),
        (SVR_MSR, 0x1ff | (1 << 12)),
        (DIVIDE_MSR, 0x4),
    ] {
        let mut lapic = Lapic::host();
        lapic.reg.insert(msr, value);
        assert_eq!(
            CapturedInterface::capture(&mut lapic, 0),
            Err(CaptureRefusal { msr, value }),
            "{msr:#x}"
        );
        assert!(lapic.writes.is_empty());
    }
}

#[test]
fn captured_icr_never_presents_eliminated_bits() {
    // 16.13 p661: ICR bits 17:16 and 12 are eliminated and must be zero; bits
    // 31:20 and 13 are reserved (Figure 16-34); RDMSR returns zero for reserved
    // bits (p659). The delivery-status bit is dropped (documented decision);
    // the other reserved bits refuse the capture.
    let mut lapic = loader_lapic();
    let captured = CapturedInterface::capture(&mut lapic, 0x1b_0000_10fd).unwrap();
    let mut page = BackingPage::new();
    page.reset_stopped(0x13, GUEST_APIC_VERSION).unwrap();
    captured.install(&page, &mut lapic);
    assert_eq!(reg(&page, off::ICR_LOW) & (1 << 12), 0);
    assert_eq!(reg(&page, off::ICR_LOW), 0xfd);
    for bit in [13u32, 16, 17, 20, 31] {
        let mut lapic = loader_lapic();
        assert!(CapturedInterface::capture(&mut lapic, 0xfd | (1u64 << bit)).is_err(), "bit {bit}");
    }
}

/// CPUID values of this machine (ppr-lapic facts Q3/Q4; acpi-madt facts):
/// Fn0000_0001 ECX (with the BIOS x2APIC bit 21 now set) and Fn8000_000A EDX.
const CPUID1_ECX: u32 = 0x7ed8_320b | (1 << 21);
const SVM_EDX: u32 = 0xfebf_bdff;

#[test]
fn x2avic_admission_requires_the_cpuid_feature_bits() {
    // 16.9 p654: x2APIC support is CPUID Fn0000_0001_ECX[x2APIC] (bit 21).
    // 15.29.7 p578: AVIC is Fn8000_000A_EDX bit 13, x2AVIC is EDX bit 18.
    // 15.21.10 p536 / PPR57896 p101: NmiVirt/VNMI is EDX bit 25, required
    // because the armed profile always enables V_NMI_ENABLE.
    assert!(X2AvicCapabilities::admit(CPUID1_ECX, SVM_EDX).is_ok());
    assert!(X2AvicCapabilities::admit(CPUID1_ECX & !(1 << 21), SVM_EDX).is_err(), "no x2APIC");
    assert!(X2AvicCapabilities::admit(CPUID1_ECX, SVM_EDX & !(1 << 13)).is_err(), "no AVIC");
    assert!(X2AvicCapabilities::admit(CPUID1_ECX, SVM_EDX & !(1 << 18)).is_err(), "no x2AVIC");
    assert!(X2AvicCapabilities::admit(CPUID1_ECX, SVM_EDX & !(1 << 25)).is_err(), "no VNMI");
}

#[test]
fn vmcb_avic_control_bits_follow_table_b_1() {
    // Table B-1 pp740-741, offset 060h: bit 31 AVIC Enable, bit 30 x2AVIC
    // Enable (15.29.10 p583 sets both), bit 24 V_INTR_MASKING; bits 15:13,
    // 23:21, 29:27 and 63:40 are SBZ.
    assert_eq!(ENABLE_BITS, (1 << 31) | (1 << 30));
    let required = (1u64 << 31) | (1 << 30) | (1 << 24);
    assert_eq!(NATIVE_CONTROL & required, required);
    let sbz = range(15, 13) | range(23, 21) | range(29, 27) | range(63, 40);
    assert_eq!(NATIVE_CONTROL & sbz, 0);
}

#[test]
fn physical_table_vmcb_field_follows_table_b_1() {
    // Table B-1 p742: 0E0h is the 52-bit backing page pointer; 0F8h holds the
    // physical table pointer in 51:12 and AVIC_PHYSICAL_MAX_INDEX in 11:0.
    // 15.29.4.3 p571: 4-Kbyte aligned addresses; MAX_INDEX above 511 fails
    // VMRUN in x2AVIC mode without x2AVIC_EXT, and one table page holds 512
    // entries (15.29.5.2 p571). The backing page and the table are distinct
    // 4-Kbyte structures (15.29.5, p571).
    let caps = X2AvicCapabilities::admit(CPUID1_ECX, SVM_EDX).unwrap();
    let p = policy(48);
    let profile = NativeX2AvicProfile::new(caps, 0x1234_5000, 0x6789_a000, 27, &p).unwrap();
    assert_eq!(profile.backing_address(), 0x1234_5000);
    assert_eq!(profile.table_address(), 0x6789_a000);
    assert_eq!(profile.maximum_id(), 27);
    assert_eq!(profile.table_control(), 0x6789_a000 | 27);
    assert_eq!(MAX_ID, 511);
    let max = NativeX2AvicProfile::new(caps, 0x1234_5000, 0x6789_a000, 511, &p).unwrap();
    assert_eq!(max.table_control() & 0xfff, 511);
    for (backing, table, maximum) in [
        (0x1234_5000u64, 0x6789_a000u64, 512u16),
        (0x1234_5001, 0x6789_a000, 27),
        (0x1234_5000, 0x6789_a800, 27),
        (1 << 48, 0x6789_a000, 27),
        (0x1234_5000, 1 << 48, 27),
        (0x1234_5000, 0x1234_5000, 27),
    ] {
        assert!(
            NativeX2AvicProfile::new(caps, backing, table, maximum, &p).is_err(),
            "backing {backing:#x} table {table:#x} max {maximum}"
        );
    }
}

// ---------------------------------------------------------------------------
// Further INIT/SIPI, INIT-state and capture interactions
// ---------------------------------------------------------------------------

#[test]
fn startup_ipi_to_all_excluding_self_reaches_every_other_cpu() {
    // Table 16-4 p644: STARTUP takes "Destination or all excluding self";
    // Figure 16-18 p643: the vector names the start routine.
    let boxes = madt_mailboxes();
    let mut owner = madt_owner();
    let source = MADT_IDS.iter().position(|id| *id == SOURCE).unwrap();
    let sipi = Icr::fixed(0).mt(MT_STARTUP, 0x9a).shorthand(DSH_OTHERS);
    owner.route_x2avic_startup(sipi.value(), &boxes, |_| {}).unwrap();
    for (slot, mailbox) in boxes.iter().enumerate() {
        let expected = (slot != source).then_some(NativeStartupCommand::Sipi(0x9a));
        assert_eq!(mailbox.peek(), expected, "slot {slot}");
    }
}

#[test]
fn init_to_a_logical_destination_reaches_the_matching_cpu() {
    // Table 16-4 p644: INIT with "Destination" shorthand is valid in either
    // destination mode (DM, Figure 16-18 p643); 16.14 p662: logical cluster 1,
    // bit 11 selects x2APIC ID 1Bh. D10 does not list logical INIT/SIPI among
    // the unsupported cases.
    let boxes = madt_mailboxes();
    let mut owner = madt_owner();
    let target = MADT_IDS.iter().position(|id| *id == 0x1b).unwrap();
    let init = Icr::logical(0x0001_0800).mt(MT_INIT, 0);
    let result = owner.route_x2avic_startup(init.value(), &boxes, |_| {});
    assert_eq!(result, Ok(()), "logical INIT to cluster 1 bit 11");
    for (slot, mailbox) in boxes.iter().enumerate() {
        let expected = (slot == target).then_some(NativeStartupCommand::Init);
        assert_eq!(mailbox.peek(), expected, "slot {slot}");
    }
}

#[test]
fn init_to_an_absent_apic_publishes_nothing() {
    // 16.6.1 p645: only an APIC whose ID matches the destination accepts the
    // message; IDs 0Ch and 1Ch have no processor in the captured MADT.
    let boxes = madt_mailboxes();
    let mut owner = madt_owner();
    for dest in [0x0c, 0x1c, 0x11b] {
        let init = Icr::fixed(dest).mt(MT_INIT, 0);
        let _ = owner.route_x2avic_startup(init.value(), &boxes, |_| {});
        assert!(boxes.iter().all(|b| b.peek().is_none()), "dest {dest:#x}");
    }
}

#[test]
fn lvt_writes_after_guest_init_are_masked_until_software_enable() {
    // Table 16-2 p631: INIT leaves SVR = FFh (ASE clear), so per 16.3.1 p629
    // LVT masks cannot be cleared until the guest sets ASE again.
    let mut vcpu = Vcpu::new();
    let mut map = Msrpm::new();
    map.configure_native_x2avic();
    registers::prepare_init(&vcpu.page, &vcpu.irq, &mut vcpu.lapic).unwrap();
    registers::commit_init(&vcpu.page, &mut vcpu.irq, &mut vcpu.lapic, &mut map).unwrap();
    assert_eq!(vcpu.write(LVT_TIMER_MSR, 0x40), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::LVT_TIMER), 0x0001_0040);
    assert_eq!(vcpu.lapic.get(LVT_TIMER_MSR), 0x0001_0040);
    assert_eq!(vcpu.write(SVR_MSR, 0x1ff), WRITTEN);
    assert_eq!(vcpu.write(LVT_TIMER_MSR, 0x40), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::LVT_TIMER), 0x40);
    assert_eq!(vcpu.lapic.get(LVT_TIMER_MSR), 0x40);
    vcpu.lapic.assert_legal();
}

#[test]
fn apic_base_bits_below_a_52_bit_width_are_base_bits() {
    // Figure 16-2 p630: ABA is 51:12; with a 52-bit implementation bits 51:48
    // are ordinary base bits, so setting one is a relocation (refused, D4/U7),
    // not a reserved-bit fault.
    for bit in 48..52 {
        let value = AP_BASE | (1u64 << bit);
        assert_eq!(
            base_write(AP_BASE, 52, value),
            refused(Refusal::ApicRelocation, value),
            "bit {bit}"
        );
    }
}

#[test]
fn software_disable_virtually_masks_a_captured_live_extint_lint0() {
    // 16.3.1 p629: with ASE clear "All LVT entry mask bits are set" and
    // "further ... ExtInt interrupts are not accepted". The captured unmasked
    // ExtINT LINT0 (firmware virtual-wire mode) must read back masked.
    let mut lapic = Lapic::host();
    lapic.reg.insert(LVT_LINT0_MSR, 0x700);
    let captured = CapturedInterface::capture(&mut lapic, 0).unwrap();
    let mut vcpu = Vcpu::with(0x00, BSP_BASE, 48);
    vcpu.lapic = lapic;
    captured.install(&vcpu.page, &mut vcpu.lapic);
    assert_eq!(reg(&vcpu.page, off::LVT_LINT0), 0x700, "captured as live");
    assert_eq!(vcpu.write(SVR_MSR, 0xff), WRITTEN);
    assert_eq!(reg(&vcpu.page, off::LVT_LINT0), 0x0001_0700, "virtual LINT0 masked");
    vcpu.lapic.assert_legal();
}

#[test]
fn software_disable_physically_masks_a_captured_live_extint_lint0() {
    // D2/D3: a software disable writes masked values to all mirrored physical
    // LVTs; the captured ExtINT LINT0 is the live physical entry of the guest,
    // and an unmasked physical ExtINT would reach the core while the virtual
    // APIC refuses ExtInt interrupts (16.3.1 p629).
    let mut lapic = Lapic::host();
    lapic.reg.insert(LVT_LINT0_MSR, 0x700);
    let captured = CapturedInterface::capture(&mut lapic, 0).unwrap();
    let mut vcpu = Vcpu::with(0x00, BSP_BASE, 48);
    vcpu.lapic = lapic;
    captured.install(&vcpu.page, &mut vcpu.lapic);
    assert_eq!(vcpu.write(SVR_MSR, 0xff), WRITTEN);
    assert_eq!(vcpu.lapic.get(LVT_LINT0_MSR), 0x0001_0700, "physical LINT0 masked");
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// Backing-page layout and doorbell MSR
// ---------------------------------------------------------------------------

#[test]
fn backing_page_registers_are_32_bit_slots_at_16_byte_offsets() {
    // 15.29.3.1 p568: "All vAPIC registers are 32-bits wide and are located at
    // 16-byte aligned offsets"; the backing page is one 4-Kbyte page (15.29.5
    // p571).
    let page = BackingPage::new();
    for offset in (0..0x1000u16).step_by(16) {
        page.write_register_stopped(offset, 0xa5a5_0000 | u32::from(offset)).unwrap();
    }
    for offset in (0..0x1000u16).step_by(16) {
        assert_eq!(reg(&page, offset), 0xa5a5_0000 | u32::from(offset), "slot {offset:#x}");
    }
    for offset in [0x21u16, 0x28, 0x2f, 0xff8, 0x1000, 0xffff] {
        assert!(page.read_register(offset).is_err(), "read {offset:#x}");
        assert!(page.write_register_stopped(offset, 1).is_err(), "write {offset:#x}");
    }
}

#[test]
fn interrupt_bitmaps_have_no_vectors_below_16() {
    // 16.6.3 p647: "Bits 255:16 correspond to interrupt vectors 255:16 ...;
    // bits 15:0 are reserved." The TMR bank map of p650 puts vectors 31-16 in
    // the first bank, 63-32 in the second, and so on.
    let page = BackingPage::new();
    for vector in 0..16u8 {
        assert!(page.enqueue(vector, false).is_err(), "vector {vector}");
        assert!(page.enqueue(vector, true).is_err(), "vector {vector}");
    }
    assert!(snapshot(&page).iter().all(|slot| *slot == 0));
    for (vector, bank, bit) in
        [(16u8, 0u16, 16u32), (31, 0, 31), (32, 1, 0), (0x62, 3, 2), (255, 7, 31)]
    {
        assert_eq!(page.enqueue(vector, true), Ok(true), "vector {vector:#x}");
        assert_eq!(page.enqueue(vector, true), Ok(false), "already pending {vector:#x}");
        assert_eq!(reg(&page, off::IRR + bank * 16), 1 << bit, "IRR bank of {vector:#x}");
        assert_eq!(reg(&page, off::TMR + bank * 16), 1 << bit, "TMR bank of {vector:#x}");
        assert!(page.is_pending(vector) && page.is_level(vector) && !page.is_in_service(vector));
        set(&page, off::IRR + bank * 16, 0);
        set(&page, off::TMR + bank * 16, 0);
    }
}

#[test]
fn highest_in_service_is_the_highest_isr_bit() {
    // 16.6.4 pp650-651: 255 is the highest priority; EOI and PPR use the
    // highest in-service vector.
    let page = BackingPage::new();
    assert_eq!(page.highest_in_service(), None);
    for (vector, highest) in
        [(0x21u8, 0x21u8), (0x80, 0x80), (0x7f, 0x80), (0xff, 0xff), (0x10, 0xff)]
    {
        set_vector(&page, off::ISR, vector);
        assert_eq!(page.highest_in_service(), Some(highest), "after {vector:#x}");
    }
}

#[test]
fn doorbell_msr_is_c001_011b_and_hidden_from_the_guest() {
    // 15.29.8.2 p579 / Figure 15-22: Doorbell Register, MSR C001_011Bh; the
    // mechanism "must be protected from access from non-privileged software"
    // (p578). Table 15-8 p518: MSRs C001_0000h-C001_1FFFh use MSRPM bytes
    // 1000h-17FFh. Msrpm::native_boot documents that C00101xx stay protected.
    assert_eq!(svmvisor_hypervisor::arch::x86_64::msr::AVIC_DOORBELL, 0xc001_011b);
    let mut map = Msrpm::native_boot();
    map.configure_native_x2avic();
    let bit = 0x1000 * 8 + 2 * (0xc001_011b_usize - 0xc001_0000);
    assert_ne!(map.bytes()[bit / 8] & (1 << (bit % 8)), 0, "doorbell read intercepted");
    assert_ne!(
        map.bytes()[(bit + 1) / 8] & (1 << ((bit + 1) % 8)),
        0,
        "doorbell write intercepted"
    );
}

// ---------------------------------------------------------------------------
// Consolidated Table 16-6 outcome matrix
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// Served or accelerated by AVIC; the owner refuses it as unowned.
    Hardware,
    /// #GP(0).
    Fault,
    /// RDMSR completes with a value.
    Value,
    /// WRMSR completes.
    Completes,
    /// Stopped unsupported refusal (LVT message-type policy, D2/D3).
    Refused,
}

fn class_of(outcome: Emulation) -> Class {
    match outcome {
        Emulation::Read(_) => Class::Value,
        Emulation::Written => Class::Completes,
        Emulation::GeneralProtection => Class::Fault,
        Emulation::Refused { reason: Refusal::UnownedAccess, .. } => Class::Hardware,
        Emulation::Refused { .. } => Class::Refused,
        other => panic!("unexpected outcome {other:?}"),
    }
}

/// LVT message-type policy for a value without reserved bits: Table 16-1 p628
/// (thermal/perf/error: Fixed, SMI, NMI), Figure 16-7 p635 (LINT: Fixed, SMI,
/// NMI, ExtINT); unmasked SMI/ExtINT and other encodings are refused (D2/D3).
fn lvt_policy(msr: u32, value: u64) -> Class {
    let masked = value & MASK != 0;
    let lint = matches!(msr, LVT_LINT0_MSR | LVT_LINT1_MSR);
    match (value >> 8) & 7 {
        0 | 4 => Class::Completes,
        2 if masked => Class::Completes,
        7 if lint && masked => Class::Completes,
        _ => Class::Refused,
    }
}

/// Expected class of an RDMSR (Table 16-6 p658, Table 15-22 pp566-568, p659,
/// p662, D1/D2).
fn expected_read(msr: u32) -> Class {
    if hardware_read(msr) {
        Class::Hardware
    } else if msr == APR_MSR || msr == CURRENT_COUNT_MSR {
        Class::Value
    } else {
        Class::Fault
    }
}

/// Expected class of a WRMSR of `value` (Table 16-6 p658; 16.11.3 p659 and
/// the per-register figures; D1/D2/U1/U10).
fn expected_write(msr: u32, value: u64) -> Class {
    let reserved = match msr {
        // EOI interception is dynamic (D6); whenever the write reaches the
        // owner it follows Table 16-6: zero completes, non-zero faults.
        EOI_MSR => u64::MAX,
        _ if hardware_write(msr) => return Class::Hardware,
        SVR_MSR => svr_reserved(),
        LVT_TIMER_MSR => timer_reserved(),
        LVT_THERMAL_MSR | LVT_PERF_MSR | LVT_ERROR_MSR => thermal_reserved(),
        LVT_LINT0_MSR | LVT_LINT1_MSR => lint_reserved(),
        INITIAL_COUNT_MSR => initial_count_reserved(),
        DIVIDE_MSR => divide_reserved(),
        ESR_MSR => u64::MAX,
        _ => return Class::Fault,
    };
    if value & reserved != 0 {
        Class::Fault
    } else if matches!(
        msr,
        LVT_THERMAL_MSR | LVT_PERF_MSR | LVT_ERROR_MSR | LVT_LINT0_MSR | LVT_LINT1_MSR
    ) {
        lvt_policy(msr, value)
    } else {
        Class::Completes
    }
}

#[test]
fn table_16_6_outcome_matrix_for_every_msr() {
    // Every MSR 800h-8FFh, both directions, with representative values: 0,
    // a masked fixed LVT (10030h), a count/SVR-sized value (1FFh), a divide
    // value (Bh), bit 32 and all ones.
    let mut vcpu = Vcpu::new();
    let mut rows = 0;
    for msr in 0x800..=0x8ffu32 {
        let got = vcpu.read(msr);
        assert_eq!(class_of(got), expected_read(msr), "RDMSR {msr:#x}: {got:?}");
        for value in [0u64, 0x1_0030, 0x1ff, 0xb, 1 << 32, u64::MAX] {
            let got = vcpu.write(msr, value);
            assert_eq!(
                class_of(got),
                expected_write(msr, value),
                "WRMSR {msr:#x} = {value:#x}: {got:?}"
            );
        }
        rows += 1;
    }
    assert_eq!(rows, 256);
    vcpu.lapic.assert_legal();
}

// ---------------------------------------------------------------------------
// CPU side of guest INIT (D9 step 5)
// ---------------------------------------------------------------------------

/// Test-fixture edit of the plain-byte VMCB image (as the existing suites do)
/// to emulate fields that hardware or other owners write.
fn poke(vmcb: &mut Vmcb, offset: usize, value: u64) {
    assert!(offset + 8 <= vmcb.bytes().len());
    // SAFETY: the VMCB is exclusively owned plain data; the write stays in bounds.
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

fn peek(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}

#[test]
fn guest_init_keeps_x2avic_enabled_and_resets_v_tpr() {
    // 16.10 p657: INIT does not change AE/EXTD, so the guest stays in x2APIC
    // mode: VMCB 060h bits 31 (AVIC) and 30 (x2AVIC) stay set (Table B-1
    // pp740-741; 15.29.10 p583), and the backing page (0E0h) and physical table
    // (0F8h, Table B-1 p742) keep their identity. Table 16-2 p631 resets TPR to
    // 0, and V_TPR mirrors TPR[7:4] (15.29.3.1 p568), so V_TPR (060h 3:0) is 0.
    // D9 step 5 invalidates the clean bits (15.15 p526, Figure 15-4 p527).
    // 16.5 p643: an INIT'ed APIC waits for STARTUP.
    let caps = X2AvicCapabilities::admit(CPUID1_ECX, SVM_EDX).unwrap();
    let profile = NativeX2AvicProfile::new(caps, 0x2000, 0x3000, 27, &policy(48)).unwrap();
    let mut vmcb = Vmcb::new();
    poke(&mut vmcb, 0x90, 1); // NP_ENABLE: AVIC requires nested paging (15.29.4.1 p570)
    vmcb.set_virtual_interrupt_tpr(6).unwrap();
    vmcb.enable_native_x2avic(&profile).unwrap();
    let control = vmcb.virtual_interrupt_control();
    assert_eq!(control & (3 << 30), 3 << 30, "x2AVIC enabled before INIT");
    // The guest later wrote CR8 = 9; hardware copied it to V_TPR, and the
    // processor cached every field.
    poke(&mut vmcb, 0x60, (control & !0xf) | 9);
    poke(&mut vmcb, 0xc0, 0xffff_ffff);
    let mut frame = GuestRegisters::default();
    let mut state = NativeStartupState::Running;
    let mut target = NativeStartupTarget {
        vmcb: &mut vmcb,
        frame: &mut frame,
        state: &mut state,
        signature: 0x00b4_0f40,
    };
    assert_eq!(
        target.apply_x2avic(NativeStartupCommand::Init, &profile),
        Ok(NativeStartupEffect::Init)
    );
    let control = vmcb.virtual_interrupt_control();
    assert_eq!(control & 0xf, 0, "V_TPR after INIT");
    assert_eq!(control & (3 << 30), 3 << 30, "AVIC and x2AVIC stay enabled");
    assert_ne!(control & (1 << 24), 0, "V_INTR_MASKING stays enabled");
    assert_eq!(peek(&vmcb, 0xc0) & 0xffff_ffff, 0, "clean bits invalidated");
    assert_eq!(peek(&vmcb, 0xe0), 0x2000, "backing page pointer");
    assert_eq!(peek(&vmcb, 0xf8), 0x3000 | 27, "physical table pointer and MAX_INDEX");
    assert_eq!(state, NativeStartupState::AwaitSipi);

    let mut target = NativeStartupTarget {
        vmcb: &mut vmcb,
        frame: &mut frame,
        state: &mut state,
        signature: 0x00b4_0f40,
    };
    assert_eq!(
        target.apply_x2avic(NativeStartupCommand::Sipi(0x9a), &profile),
        Ok(NativeStartupEffect::Started)
    );
    assert_eq!(
        target.apply_x2avic(NativeStartupCommand::Sipi(0x9b), &profile),
        Ok(NativeStartupEffect::Ignored)
    );
    assert_eq!(state, NativeStartupState::Running);
    let control = vmcb.virtual_interrupt_control();
    assert_eq!(control & (3 << 30), 3 << 30, "x2AVIC still enabled after SIPI");
}
