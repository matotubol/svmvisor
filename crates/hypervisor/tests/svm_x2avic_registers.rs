//! x2AVIC guest register owner (phase B D1-D4), EOI/level-source
//! coordination (D6) and the LAPIC half of guest INIT (D9), against a
//! recording model of the physical x2APIC.

use std::collections::BTreeMap;

use svmvisor_hypervisor::{
    arch::x86_64::apic::{self, PhysicalX2Apic},
    memory::address::{AddressPolicy, EncryptionState},
    svm::{
        permission_maps::{MsrAccess, Msrpm},
        x2avic::{
            BackingPage, Error, GUEST_APIC_VERSION,
            irq::{self, Capture, IrqError, PhysicalIrqLedger},
            registers::{
                self, CaptureRefusal, CapturedInterface, Emulation, GuestX2Apic, InitError, Refusal,
            },
        },
    },
};

const GP: Emulation = Emulation::GeneralProtection;
const WRITTEN: Emulation = Emulation::Written;
/// Captured BSP APIC_BASE: enabled x2APIC at FEE0_0000h with BSC set.
const BSP_BASE: u64 = 0xfee0_0d00;
const MASK: u64 = 1 << 16;

fn refused(reason: Refusal, value: u64) -> Emulation {
    Emulation::Refused { reason, value }
}

/// Recording model of one physical x2APIC. A write of MSR 80Bh clears the
/// highest ISR bit (APM2 16.6.4 p652); every other write is stored.
#[derive(Default)]
struct FakeApic {
    registers: BTreeMap<u32, u64>,
    reads: Vec<u32>,
    writes: Vec<(u32, u64)>,
}

impl FakeApic {
    /// Host-owned physical SVR: software-enabled, spurious vector FFh.
    fn host() -> Self {
        let mut apic = Self::default();
        apic.registers.insert(0x80f, 0x1ff);
        apic
    }

    /// Physical acceptance of `vector` (sets ISR and the trigger-mode bit).
    fn raise(&mut self, vector: u8, level: bool) {
        let bit = 1u64 << (vector % 32);
        *self.registers.entry(0x810 + u32::from(vector / 32)).or_default() |= bit;
        let tmr = self.registers.entry(0x818 + u32::from(vector / 32)).or_default();
        if level { *tmr |= bit } else { *tmr &= !bit }
    }

    fn isr(&self) -> Vec<u8> {
        (0..=255u8)
            .filter(|v| {
                self.registers
                    .get(&(0x810 + u32::from(v / 32)))
                    .is_some_and(|bank| bank & (1 << (v % 32)) != 0)
            })
            .collect()
    }

    fn eois(&self) -> usize {
        self.writes.iter().filter(|(msr, _)| *msr == 0x80b).count()
    }
}

impl PhysicalX2Apic for FakeApic {
    fn read(&mut self, msr: u32) -> u64 {
        self.reads.push(msr);
        self.registers.get(&msr).copied().unwrap_or(0)
    }

    fn write(&mut self, msr: u32, value: u64) {
        self.writes.push((msr, value));
        if msr == 0x80b {
            assert_eq!(value, 0, "a physical EOI writes zero");
            for bank in (0x810..=0x817).rev() {
                let bits = self.registers.entry(bank).or_default();
                if *bits != 0 {
                    *bits &= !(1 << (63 - bits.leading_zeros()));
                    return;
                }
            }
        } else {
            self.registers.insert(msr, value);
        }
    }
}

fn offset(msr: u32) -> u16 {
    ((msr - 0x800) << 4) as u16
}

fn snapshot(page: &BackingPage) -> Vec<u32> {
    (0..0x1000u16).step_by(16).map(|offset| page.read_register(offset).unwrap()).collect()
}

fn set_vector(page: &BackingPage, base: u16, vector: u8) {
    let bank = base + u16::from(vector / 32) * 16;
    let bits = page.read_register(bank).unwrap();
    page.write_register_stopped(bank, bits | (1 << (vector % 32))).unwrap();
}

fn vectors(page: &BackingPage, base: u16) -> Vec<u8> {
    (0..=255u8)
        .filter(|v| {
            page.read_register(base + u16::from(v / 32) * 16).unwrap() & (1 << (v % 32)) != 0
        })
        .collect()
}

fn map_bit(map: &Msrpm, msr: u32, write: bool) -> bool {
    let bit = 2 * msr as usize + usize::from(write);
    map.bytes()[bit / 8] & (1 << (bit % 8)) != 0
}

/// One stopped vCPU: register owner, backing page (x2APIC ID 3, software
/// enabled), ledger and physical x2APIC.
struct Env {
    guest: GuestX2Apic,
    page: BackingPage,
    irq: PhysicalIrqLedger,
    apic: FakeApic,
}

impl Env {
    fn new() -> Self {
        let mut page = BackingPage::new();
        page.reset_stopped(3, GUEST_APIC_VERSION).unwrap();
        page.write_register_stopped(apic::SVR, 0x1ff).unwrap();
        Self {
            guest: GuestX2Apic::admit(BSP_BASE, &policy(48)).unwrap(),
            page,
            irq: PhysicalIrqLedger::new(),
            apic: FakeApic::host(),
        }
    }

    fn read(&mut self, msr: u32) -> Emulation {
        self.guest.emulate(msr, None, &self.page, &mut self.irq, &mut self.apic)
    }

    fn write(&mut self, msr: u32, value: u64) -> Emulation {
        self.guest.emulate(msr, Some(value), &self.page, &mut self.irq, &mut self.apic)
    }

    fn reg(&self, offset: u16) -> u32 {
        self.page.read_register(offset).unwrap()
    }

    fn set(&self, offset: u16, value: u32) {
        self.page.write_register_stopped(offset, value).unwrap();
    }

    fn capture(&mut self, vector: u8, level: bool) -> Result<Option<Capture>, IrqError> {
        self.apic.raise(vector, level);
        irq::capture(vector, &self.page, &mut self.irq, &mut self.apic)
    }
}

fn policy(bits: u8) -> AddressPolicy {
    AddressPolicy::new(bits, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

/// Table 16-6 p658 standard x2APIC registers (8-bank registers count 8).
fn implemented(msr: u32) -> bool {
    matches!(msr, 0x802 | 0x803 | 0x808..=0x80b | 0x80d | 0x80f | 0x810..=0x828 | 0x830
        | 0x832..=0x839 | 0x83e | 0x83f)
}

/// Firmware register state on a physical x2APIC: TPR, SVR, the six LVTs,
/// the timer counts and divide configuration.
fn loader_apic(svr: u64, lvts: [u64; 6]) -> FakeApic {
    let mut apic = FakeApic::default();
    apic.registers.insert(0x808, 0x20);
    apic.registers.insert(0x80f, svr);
    for (msr, value) in (0x832..=0x837).zip(lvts) {
        apic.registers.insert(msr, value);
    }
    apic.registers.insert(0x838, 0x1234_5678);
    apic.registers.insert(0x839, 0x1111);
    apic.registers.insert(0x83e, 0xb);
    apic
}

fn installed(apic: &mut FakeApic, icr: u64) -> BackingPage {
    let interface = CapturedInterface::capture(apic, icr).unwrap();
    assert!(apic.writes.is_empty(), "capture is read-only");
    let mut page = BackingPage::new();
    page.reset_stopped(3, GUEST_APIC_VERSION).unwrap();
    interface.install(&page, apic);
    page
}

fn busy_init_env() -> (Env, Msrpm) {
    let mut env = Env::new();
    let mut msrpm = Msrpm::native_boot();
    msrpm.configure_native_x2avic();
    // Three held level sources in capture order; 61h is guest-completed but
    // waits behind 80h.
    for vector in [0x40, 0x61, 0x80] {
        assert_eq!(env.capture(vector, true), Ok(Some(Capture::Level)));
    }
    accept(&env.page, 0x61);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert!(msrpm.update_x2apic_eoi_intercept(&env.irq));
    accept(&env.page, 0x80);
    // Guest register state that INIT resets.
    env.set(apic::TPR, 0x6b);
    for (msr, value) in
        [(0x832, 0x2_00ef), (0x838, 1000), (0x83e, 0xb), (0x836, 0x0400), (0x80f, 0x3f0)]
    {
        assert_eq!(env.write(msr, value), WRITTEN);
    }
    env.set(apic::ICR, 0x4ef);
    env.set(apic::ICR_HIGH, 7);
    env.set(apic::PPR, 0x80);
    env.page.enqueue(0x91, false).unwrap();
    env.apic.registers.insert(0x839, 777);
    env.apic.writes.clear();
    env.apic.reads.clear();
    (env, msrpm)
}

/// AVIC delivery of a pending vector to the guest: IRR to ISR.
fn accept(page: &BackingPage, vector: u8) {
    let bank = u16::from(vector / 32) * 16;
    let bit = 1u32 << (vector % 32);
    let irr = page.read_register(apic::IRR + bank).unwrap();
    assert!(irr & bit != 0, "vector {vector:#x} is not pending");
    page.write_register_stopped(apic::IRR + bank, irr & !bit).unwrap();
    let isr = page.read_register(apic::ISR + bank).unwrap();
    page.write_register_stopped(apic::ISR + bank, isr | bit).unwrap();
}

#[test]
fn unimplemented_reserved_and_extended_x2apic_msrs_fault_both_ways_without_effects() {
    let mut env = Env::new();
    let before = snapshot(&env.page);
    let mut count = 0;
    for msr in (0x800..=0x8ffu32).filter(|msr| !implemented(*msr)) {
        assert_eq!(env.read(msr), GP, "{msr:#x}");
        for value in [0, 1, u64::MAX] {
            assert_eq!(env.write(msr, value), GP, "{msr:#x}");
        }
        count += 1;
    }
    // 80Ch, 80Eh and 831h are eliminated; 840h-8FFh are absent (EAS=0).
    assert!(!implemented(0x80c) && !implemented(0x80e) && !implemented(0x831));
    assert_eq!(count, 256 - 44);
    assert_eq!(snapshot(&env.page), before);
    assert!(env.apic.writes.is_empty() && env.apic.reads.is_empty() && env.irq.is_empty());
}

#[test]
fn read_only_writes_write_only_reads_and_nonzero_eoi_or_esr_writes_fault() {
    let mut env = Env::new();
    env.page.enqueue(0x61, false).unwrap();
    set_vector(&env.page, apic::ISR, 0x42);
    let before = snapshot(&env.page);
    let read_only = [0x802u32, 0x803, 0x809, 0x80a, 0x80d, 0x839].into_iter().chain(0x810..=0x827);
    for msr in read_only {
        for value in [0, 3, 0x42, 0xffff_ffff, u64::MAX] {
            assert_eq!(env.write(msr, value), GP, "{msr:#x} {value:#x}");
        }
    }
    for msr in [0x80b, 0x83f] {
        assert_eq!(env.read(msr), GP, "{msr:#x}");
    }
    for value in [1, 0x42, 0x100, 1 << 32, u64::MAX] {
        assert_eq!(env.write(0x80b, value), GP);
        assert_eq!(env.write(0x828, value), GP);
    }
    assert_eq!(snapshot(&env.page), before);
    assert!(env.apic.writes.is_empty() && env.apic.reads.is_empty());
}

#[test]
fn only_intercepted_accesses_are_emulated_and_others_are_refused() {
    let mut env = Env::new();
    for msr in 0x800..=0x8ffu32 {
        for write in [false, true] {
            let access = if write { MsrAccess::Write } else { MsrAccess::Read };
            let outcome = if write { env.write(msr, 0) } else { env.read(msr) };
            let refused_here = outcome == refused(Refusal::UnownedAccess, 0);
            // EOI writes are emulated whenever they arrive (D6).
            let left_to_hardware = !registers::intercepted(msr, access) && !(write && msr == 0x80b);
            assert_eq!(refused_here, left_to_hardware, "{msr:#x} write={write}");
        }
    }
    for msr in [0, 0x1c, 0x7ff, 0x900, 0xc000_0080] {
        assert_eq!(env.read(msr), refused(Refusal::UnownedAccess, 0));
        assert_eq!(env.write(msr, 7), refused(Refusal::UnownedAccess, 7));
    }
    // Hardware writes carry their value into the refusal evidence.
    for msr in [0x808, 0x830, 0x83f] {
        assert_eq!(env.write(msr, 0x55), refused(Refusal::UnownedAccess, 0x55));
    }
}

#[test]
fn every_reserved_bit_of_every_writable_register_faults() {
    // D2 masks, 64-bit x2APIC view (16.11.3 p659).
    let reserved: [(u32, u64); 9] = [
        (0x80f, !0x3ff),       // SVR 63:10 (bit 12 included)
        (0x832, !0x0003_10ff), // timer 63:18, 15:13, 11:8
        (0x833, !0x0001_17ff), // thermal 63:17, 15:13, 11
        (0x834, !0x0001_17ff), // perf
        (0x835, !0x0001_d7ff), // LINT0 63:17, 13, 11
        (0x836, !0x0001_d7ff), // LINT1
        (0x837, !0x0001_17ff), // error
        (0x838, !0xffff_ffff), // initial count 63:32
        (0x83e, !0xb),         // divide 63:4 and 2
    ];
    for (msr, mask) in reserved {
        for bit in 0..64 {
            let mut env = Env::new();
            let before = snapshot(&env.page);
            let outcome = env.write(msr, 1 << bit);
            if mask & (1 << bit) != 0 {
                assert_eq!(outcome, GP, "{msr:#x} bit {bit}");
                assert_eq!(snapshot(&env.page), before);
                assert!(env.apic.writes.is_empty());
            } else {
                assert_ne!(outcome, GP, "{msr:#x} bit {bit}");
            }
            // A reserved bit faults next to every valid bit as well.
            if mask & (1 << bit) != 0 {
                assert_eq!(Env::new().write(msr, !mask | (1 << bit)), GP, "{msr:#x} bit {bit}");
            }
        }
    }
}

#[test]
fn lvt_writes_drop_read_only_bits_and_mirror_the_stored_entry() {
    for (msr, value, stored) in [
        (0x832u32, 0x2_10efu64, 0x2_00efu32), // periodic timer; DS ignored
        (0x832, 0x1_00ef, 0x1_00ef),          // masked one-shot timer
        (0x835, 0xd030, 0x8030),              // LINT0 level fixed; RIR and DS ignored
        (0x836, 0x0400, 0x0400),              // LINT1 NMI edge (MADT UID FFh, LINT1, 0005h)
        (0x837, 0x10fe, 0x00fe),              // error fixed FEh; DS ignored
        (0x834, 0x0400, 0x0400),              // perf NMI
        (0x833, 0x1_0020, 0x1_0020),          // masked thermal
    ] {
        let mut env = Env::new();
        assert_eq!(env.write(msr, value), WRITTEN, "{msr:#x}");
        assert_eq!(env.reg(offset(msr)), stored, "{msr:#x}");
        assert_eq!(env.apic.writes, [(msr, u64::from(stored))], "{msr:#x}");
    }
}

#[test]
fn lvt_message_type_and_unmasked_delivery_policy_matrix() {
    for msr in 0x833..=0x837u32 {
        let lint = matches!(msr, 0x835 | 0x836);
        for message in 0..8u64 {
            for masked in [false, true] {
                let mut env = Env::new();
                let before = snapshot(&env.page);
                let value = (u64::from(masked) << 16) | (message << 8) | 0x40;
                let expected = match (message, masked) {
                    (0 | 4, _) => WRITTEN,
                    (2, false) => refused(Refusal::UnmaskedSmi, value),
                    (2, true) => WRITTEN,
                    (7, false) if lint => refused(Refusal::UnmaskedExtInt, value),
                    (7, true) if lint => WRITTEN,
                    _ => refused(Refusal::UnsupportedMessageType, value),
                };
                assert_eq!(env.write(msr, value), expected, "{msr:#x} {value:#x}");
                if expected == WRITTEN {
                    assert_eq!(env.reg(offset(msr)), value as u32);
                    assert_eq!(env.apic.writes, [(msr, value)]);
                } else {
                    assert_eq!(snapshot(&env.page), before);
                    assert!(env.apic.writes.is_empty());
                }
            }
        }
    }
    // The timer has no message-type field: bits 10:8 are reserved.
    for message in 1..8u64 {
        assert_eq!(Env::new().write(0x832, (message << 8) | 0x40), GP);
    }
}

#[test]
fn unmasked_fixed_lvt_below_vector_16_is_stored_but_physically_masked() {
    for msr in 0x832..=0x837u32 {
        for vector in (0..=32u64).chain([0xff]) {
            let mut env = Env::new();
            let before = snapshot(&env.page);
            if (16..32).contains(&vector) {
                // Exception vectors: the host IDT cannot accept them, so an
                // unmasked fixed entry is a stopped refusal (review F3).
                assert_eq!(
                    env.write(msr, vector),
                    refused(Refusal::ExceptionVector, vector),
                    "{msr:#x} {vector}"
                );
                assert_eq!(snapshot(&env.page), before);
                assert!(env.apic.writes.is_empty());
            } else {
                assert_eq!(env.write(msr, vector), WRITTEN, "{msr:#x} {vector}");
                assert_eq!(env.reg(offset(msr)), vector as u32);
                let mirror = if vector < 16 { vector | MASK } else { vector };
                assert_eq!(env.apic.writes, [(msr, mirror)], "{msr:#x} {vector}");
            }
            let mut env = Env::new();
            assert_eq!(env.write(msr, vector | MASK), WRITTEN);
            assert_eq!(env.apic.writes, [(msr, vector | MASK)]);
        }
    }
    // NMI ignores its vector (Figure 16-7 p635): the mirror stays unmasked.
    let mut env = Env::new();
    assert_eq!(env.write(0x836, 0x0405), WRITTEN);
    assert_eq!(env.apic.writes, [(0x836, 0x0405)]);
}

#[test]
fn software_disable_forces_every_lvt_mask_and_reenable_keeps_them() {
    let mut env = Env::new();
    let entries = [
        (0x832u32, 0x2_00efu64),
        (0x833, 0x0041),
        (0x834, 0x0400),
        (0x835, 0x8030),
        (0x836, 0x0400),
        (0x837, 0x00fe),
    ];
    for (msr, value) in entries {
        assert_eq!(env.write(msr, value), WRITTEN);
    }
    // A value captured from hardware may carry DS; forcing the mask drops it.
    env.set(apic::LVT_THERMAL, 0x1041);
    env.apic.writes.clear();
    // FCC set, ASE clear, vector F0h.
    assert_eq!(env.write(0x80f, 0x2f0), WRITTEN);
    assert_eq!(env.reg(apic::SVR), 0x2f0);
    let forced: Vec<(u32, u64)> = entries.iter().map(|&(msr, value)| (msr, value | MASK)).collect();
    assert_eq!(env.apic.writes, forced);
    for (msr, value) in entries {
        assert_eq!(u64::from(env.reg(offset(msr))), value | MASK, "{msr:#x}");
    }
    env.apic.writes.clear();
    // While disabled the mask cannot be cleared; a masked SMI is accepted.
    assert_eq!(env.write(0x833, 0x0042), WRITTEN);
    assert_eq!(env.write(0x834, 0x0200), WRITTEN);
    assert_eq!(env.write(0x832, 0x0030), WRITTEN);
    assert_eq!(env.reg(apic::LVT_THERMAL), 0x1_0042);
    assert_eq!(env.reg(apic::LVT_PERFORMANCE), 0x1_0200);
    assert_eq!(env.apic.writes, [(0x833, 0x1_0042), (0x834, 0x1_0200), (0x832, 0x1_0030)]);
    // A second disable keeps everything masked.
    env.apic.writes.clear();
    assert_eq!(env.write(0x80f, 0x0ff), WRITTEN);
    assert_eq!(env.apic.writes.len(), 6);
    assert!(env.apic.writes.iter().all(|(_, value)| value & MASK != 0));
    // Re-enable changes no LVT (decision for APM U13).
    env.apic.writes.clear();
    assert_eq!(env.write(0x80f, 0x1ff), WRITTEN);
    assert!(env.apic.writes.is_empty());
    assert_eq!(env.reg(apic::LVT_TIMER), 0x1_0030);
    // Only a rewrite unmasks; an unmasked SMI is refused again.
    assert_eq!(env.write(0x832, 0x2_00ef), WRITTEN);
    assert_eq!(env.reg(apic::LVT_TIMER), 0x2_00ef);
    assert_eq!(env.write(0x834, 0x0200), refused(Refusal::UnmaskedSmi, 0x200));
    // The physical SVR stays host-owned throughout.
    assert_eq!(env.apic.registers[&0x80f], 0x1ff);
}

#[test]
fn reenable_withdraws_only_interrupts_that_arrived_while_disabled() {
    // 16.3.1 p629: while ASE is clear, pending ISR/IRR are held and further
    // fixed interrupts are not accepted. x2AVIC delivers IPIs without the
    // virtual SVR, so the enable write withdraws what arrived meanwhile.
    let mut env = Env::new();
    assert_eq!(env.capture(0x61, true), Ok(Some(Capture::Level)));
    env.page.enqueue(0x50, false).unwrap();
    assert_eq!(env.write(0x80f, 0x0ff), WRITTEN);
    // Arrivals while disabled: hardware IPIs (70h; 50h again), a stale level
    // trigger with a pending bit (72h), and a level source the bridge still
    // publishes and holds (62h, documented deviation).
    env.page.enqueue(0x70, false).unwrap();
    assert_eq!(env.page.enqueue(0x50, false), Ok(false));
    set_vector(&env.page, apic::IRR, 0x72);
    set_vector(&env.page, apic::TMR, 0x72);
    assert_eq!(env.capture(0x62, true), Ok(Some(Capture::Level)));
    // An edge source is only acknowledged while disabled.
    assert_eq!(env.capture(0x74, false), Ok(Some(Capture::Discarded)));
    let writes = env.apic.writes.len();
    assert_eq!(env.write(0x80f, 0x1ff), WRITTEN);
    assert_eq!(env.apic.writes.len(), writes, "the enable touches no physical register");
    assert_eq!(env.reg(apic::SVR), 0x1ff);
    // Held since the disable or owned by the ledger: kept, with their TMR.
    assert!(env.page.is_pending(0x50) && !env.page.is_level(0x50));
    assert!(env.page.is_pending(0x61) && env.page.is_level(0x61));
    assert!(env.page.is_pending(0x62) && env.page.is_level(0x62));
    // Arrived while disabled: withdrawn with its trigger bit.
    for vector in [0x70u8, 0x72, 0x74] {
        assert!(!env.page.is_pending(vector) && !env.page.is_level(vector), "{vector:#x}");
    }
    // Already enabled: another enable write withdraws nothing.
    env.page.enqueue(0x71, false).unwrap();
    assert_eq!(env.write(0x80f, 0x1ff), WRITTEN);
    assert!(env.page.is_pending(0x71));
    // The next cycle starts from a fresh record: 50h, pending at its
    // disable, survives; 71h is withdrawn only because it was not.
    accept(&env.page, 0x71);
    assert_eq!(env.write(0x80f, 0x0ff), WRITTEN);
    env.page.enqueue(0x71, false).unwrap();
    assert_eq!(env.write(0x80f, 0x1ff), WRITTEN);
    assert!(env.page.is_pending(0x50) && !env.page.is_pending(0x71));
}

#[test]
fn guest_init_forgets_the_irr_held_at_a_software_disable() {
    // Table 16-2 p631: INIT leaves SVR FFh with an empty IRR, so nothing is
    // held; interrupts pending at the old disable are gone, and anything that
    // arrives before the guest enables its APIC again is withdrawn.
    let mut env = Env::new();
    let mut msrpm = Msrpm::native_boot();
    msrpm.configure_native_x2avic();
    env.page.enqueue(0x50, false).unwrap();
    assert_eq!(env.write(0x80f, 0x0ff), WRITTEN);
    registers::prepare_init(&env.page, &env.irq, &mut env.apic).unwrap();
    registers::commit_init(&env.page, &mut env.irq, &mut env.apic, &mut msrpm).unwrap();
    env.guest.reset_after_init();
    env.page.enqueue(0x50, false).unwrap();
    assert_eq!(env.write(0x80f, 0x1ff), WRITTEN);
    assert!(!env.page.is_pending(0x50));
}

#[test]
fn svr_stores_bits_9_to_0_for_every_vector() {
    for value in [0x000u64, 0x0ff, 0x100, 0x1ff, 0x200, 0x3ff, 0x10f] {
        let mut env = Env::new();
        assert_eq!(env.write(0x80f, value), WRITTEN);
        assert_eq!(u64::from(env.reg(apic::SVR)), value);
        assert!(env.apic.writes.iter().all(|(msr, _)| (0x832..=0x837).contains(msr)));
        let masks_forced = value & 0x100 == 0;
        assert_eq!(env.apic.writes.len(), if masks_forced { 6 } else { 0 });
    }
}

#[test]
fn divide_configuration_accepts_exactly_the_table_16_3_encodings() {
    for value in (0..0x40u64).chain([1 << 31, 1 << 32, u64::MAX]) {
        let mut env = Env::new();
        let valid = [0, 1, 2, 3, 8, 9, 0xa, 0xb].contains(&value);
        if valid {
            assert_eq!(env.write(0x83e, value), WRITTEN, "{value:#x}");
            assert_eq!(u64::from(env.reg(apic::TIMER_DIVIDE)), value);
            assert_eq!(env.apic.writes, [(0x83e, value)]);
        } else {
            assert_eq!(env.write(0x83e, value), GP, "{value:#x}");
            assert_eq!(env.reg(apic::TIMER_DIVIDE), 0);
            assert!(env.apic.writes.is_empty());
        }
    }
}

#[test]
fn timer_counts_are_stored_mirrored_and_read_from_the_physical_timer() {
    let mut env = Env::new();
    assert_eq!(env.write(0x838, 0xffff_ffff), WRITTEN);
    assert_eq!(env.reg(apic::TIMER_INITIAL_COUNT), 0xffff_ffff);
    assert_eq!(env.write(0x838, 0), WRITTEN);
    assert_eq!(env.reg(apic::TIMER_INITIAL_COUNT), 0);
    for value in [1 << 32, u64::MAX, 0x8000_0000_0000_0001] {
        assert_eq!(env.write(0x838, value), GP);
    }
    assert_eq!(env.apic.writes, [(0x838, 0xffff_ffff), (0x838, 0)]);
    // Reserved high bits of the physical value never reach the guest.
    env.apic.registers.insert(0x839, 0xdead_0000_1234_5678);
    assert_eq!(env.read(0x839), Emulation::Read(0x1234_5678));
    assert_eq!(env.apic.reads, [0x839]);
    assert_eq!(env.write(0x839, 0), GP);
    assert_eq!(env.apic.writes.len(), 2);
}

#[test]
fn esr_write_accepts_only_zero_and_leaves_an_empty_error_state() {
    let mut env = Env::new();
    env.set(apic::ESR, 0x40);
    assert_eq!(env.write(0x828, 0x40), GP);
    assert_eq!(env.reg(apic::ESR), 0x40);
    assert_eq!(env.write(0x828, 0), WRITTEN);
    assert_eq!(env.reg(apic::ESR), 0);
    assert!(env.apic.writes.is_empty() && env.apic.reads.is_empty());
}

#[test]
fn apr_read_takes_the_highest_class_and_keeps_tps_only_for_the_tpr_class() {
    let cases: [(u32, &[u8], &[u8], u64); 12] = [
        (0x00, &[], &[], 0x00),
        (0x6b, &[], &[], 0x6b),
        (0x6b, &[0x61], &[], 0x6b),
        (0x6b, &[0x70], &[], 0x70),
        (0x6b, &[], &[0x9f], 0x90),
        (0x6b, &[0x80], &[0xa0], 0xa0),
        (0x6b, &[0xa1], &[0x81], 0xa0),
        (0x3f, &[0x20], &[0x31], 0x3f),
        (0x00, &[], &[0xff], 0xf0),
        (0xff, &[0xf0], &[0xff], 0xff),
        (0x25, &[0x21, 0x30], &[0x10], 0x30),
        // TPR bits 31:8 are not part of the priority.
        (0x1_6b, &[0x61], &[0x62], 0x6b),
    ];
    for (tpr, in_service, pending, apr) in cases {
        let mut env = Env::new();
        env.set(apic::TPR, tpr);
        for &vector in in_service {
            set_vector(&env.page, apic::ISR, vector)
        }
        for &vector in pending {
            set_vector(&env.page, apic::IRR, vector)
        }
        let before = snapshot(&env.page);
        assert_eq!(env.read(0x809), Emulation::Read(apr), "{tpr:#x} {in_service:?} {pending:?}");
        assert_eq!(snapshot(&env.page), before);
        assert!(env.apic.reads.is_empty());
    }
    // Reserved IRR/ISR bits 15:0 never name a vector.
    let mut env = Env::new();
    env.set(apic::TPR, 0x3c);
    env.set(apic::IRR, 1 << 15);
    env.set(apic::ISR, 1 << 1);
    assert_eq!(env.read(0x809), Emulation::Read(0x3c));
}

#[test]
fn apic_base_admission_requires_enabled_x2apic_at_the_reset_base() {
    for base in [0xfee0_0c00u64, 0xfee0_0d00] {
        let mut env = Env::new();
        env.guest = GuestX2Apic::admit(base, &policy(48)).unwrap();
        assert_eq!(env.read(0x1b), Emulation::Read(base));
    }
    for base in [
        0xfee0_0800u64,
        0xfee0_0900,
        0xfee0_0400,
        0xfee0_0000,
        0xfed0_0c00,
        0xfee0_0e00,
        0xfee0_0c01,
        0x1_fee0_0c00,
        0xfee0_1c00,
        0,
    ] {
        assert_eq!(
            GuestX2Apic::admit(base, &policy(48)),
            Err(Error::UnsupportedApicBase),
            "{base:#x}"
        );
    }
}

#[test]
fn apic_base_writes_follow_the_x2apic_transition_rules_and_never_change_the_shadow() {
    let mut env = Env::new();
    assert_eq!(env.read(0x1b), Emulation::Read(BSP_BASE));
    let mut cases = vec![
        (0xfee0_0d00u64, WRITTEN), // 11 -> 11
        (0xfee0_0c00, WRITTEN),    // BSC ignored
        (0xfee0_0900, GP),         // 11 -> 10
        (0xfee0_0800, GP),
        (0xfee0_0500, GP), // 01 invalid
        (0xfee0_0400, GP),
        (0xfee0_0100, refused(Refusal::ApicDisable, 0xfee0_0100)), // 11 -> 00
        (0xfee0_0000, refused(Refusal::ApicDisable, 0xfee0_0000)),
        (0xfed0_0d00, refused(Refusal::ApicRelocation, 0xfed0_0d00)), // base change
        (0x0000_0000_0000_0c00, refused(Refusal::ApicRelocation, 0xc00)),
        (0x0000_8000_fee0_0d00, refused(Refusal::ApicRelocation, 0x0000_8000_fee0_0d00)),
        (0x0001_0000_fee0_0d00, GP), // bit 48: beyond width
        (0x000f_0000_fee0_0d00, GP),
        (0x0010_0000_fee0_0d00, GP), // bits 63:52
        (0x8000_0000_fee0_0d00, GP),
        (0xfee0_0f00, GP), // bit 9
        (0xfee0_0101, GP), // reserved before mode
        (0x0001_0000_fee0_0100, GP),
    ];
    for bit in 0..8 {
        cases.push((BSP_BASE | (1 << bit), GP));
    }
    for (value, expected) in cases {
        assert_eq!(env.write(0x1b, value), expected, "{value:#x}");
        assert_eq!(env.read(0x1b), Emulation::Read(BSP_BASE));
    }
    assert!(env.apic.writes.is_empty() && env.apic.reads.is_empty());
    // The reserved field follows the admitted physical-address width.
    let mut irq = PhysicalIrqLedger::new();
    let mut physical = FakeApic::host();
    let mut wide = GuestX2Apic::admit(0xfee0_0c00, &policy(52)).unwrap();
    let value = 0x000f_0000_fee0_0c00;
    assert_eq!(
        wide.emulate(0x1b, Some(value), &env.page, &mut irq, &mut physical),
        refused(Refusal::ApicRelocation, value)
    );
    assert_eq!(
        wide.emulate(0x1b, Some(0x0010_0000_fee0_0c00), &env.page, &mut irq, &mut physical),
        GP
    );
    let mut narrow = GuestX2Apic::admit(0xfee0_0c00, &policy(32)).unwrap();
    assert_eq!(narrow.emulate(0x1b, Some(0x1_fee0_0c00), &env.page, &mut irq, &mut physical), GP);
    assert_eq!(
        narrow.emulate(0x1b, Some(0xfee0_0c00), &env.page, &mut irq, &mut physical),
        WRITTEN
    );
}

#[test]
fn edge_software_eoi_clears_the_highest_isr_and_recomputes_ppr() {
    let mut env = Env::new();
    env.set(apic::TPR, 0x3b);
    for vector in [0x42, 0x61] {
        set_vector(&env.page, apic::ISR, vector)
    }
    env.page.enqueue(0x90, false).unwrap();
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert_eq!(vectors(&env.page, apic::ISR), [0x42]);
    // 16.6.4 p651: ISR class 4 is above TP 3, so PPR is 40h (PPS zero).
    assert_eq!(env.reg(apic::PPR), 0x40);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert!(vectors(&env.page, apic::ISR).is_empty());
    // Nothing in service: PPR equals TPR, including TPS.
    assert_eq!(env.reg(apic::PPR), 0x3b);
    assert!(env.page.is_pending(0x90));
    // An EOI with an empty ISR is a no-op (decision for APM U18).
    let before = snapshot(&env.page);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert_eq!(snapshot(&env.page), before);
    assert!(env.apic.writes.is_empty() && env.irq.is_empty());
    // PPS is kept when the ISR class equals TP.
    env.set(apic::TPR, 0x6b);
    for vector in [0x61, 0x72] {
        set_vector(&env.page, apic::ISR, vector)
    }
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert_eq!(env.reg(apic::PPR), 0x6b);
}

#[test]
fn held_level_eoi_waits_for_a_higher_physical_source_even_when_the_guest_completes_the_lower_first()
{
    let mut env = Env::new();
    // The guest priority blocks 80h for now.
    env.set(apic::TPR, 0x90);
    // The host captured level 40h, then level 80h preempted it; both are held.
    assert_eq!(env.capture(0x40, true), Ok(Some(Capture::Level)));
    assert_eq!(env.capture(0x80, true), Ok(Some(Capture::Level)));
    assert_eq!(env.apic.isr(), [0x40, 0x80]);
    assert_eq!(env.apic.eois(), 0);
    assert!(env.page.is_level(0x40) && env.page.is_level(0x80));
    // AVIC delivered 40h (it was pending before the TPR change).
    accept(&env.page, 0x40);
    // The guest completes the lower vector first. Physically, 80h is still
    // in service above it, so no physical EOI may happen yet.
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert_eq!(env.apic.eois(), 0);
    assert_eq!(env.apic.isr(), [0x40, 0x80]);
    assert!(env.irq.holds(0x40) && env.irq.holds(0x80));
    // Its TMR bit is stale and cleared; 80h keeps its level metadata.
    assert!(!env.page.is_level(0x40));
    assert!(env.page.is_level(0x80) && env.page.is_pending(0x80));
    // Later the guest takes and completes 80h: both physical EOIs follow, in
    // physical ISR order (the model clears the highest bit per EOI).
    env.set(apic::TPR, 0);
    accept(&env.page, 0x80);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert_eq!(env.apic.eois(), 2);
    assert!(env.apic.isr().is_empty() && env.irq.is_empty());
    assert!(!env.page.is_level(0x80));
    assert!(vectors(&env.page, apic::ISR).is_empty());
}

#[test]
fn stale_tmr_is_cleared_unless_the_vector_is_pending_again() {
    let mut env = Env::new();
    set_vector(&env.page, apic::TMR, 0x55);
    set_vector(&env.page, apic::ISR, 0x55);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert!(!env.page.is_level(0x55) && !env.page.is_in_service(0x55));
    set_vector(&env.page, apic::TMR, 0x56);
    set_vector(&env.page, apic::ISR, 0x56);
    set_vector(&env.page, apic::IRR, 0x56);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert!(env.page.is_level(0x56) && env.page.is_pending(0x56) && !env.page.is_in_service(0x56));
    assert!(env.apic.writes.is_empty());
}

#[test]
fn a_second_instance_of_a_completed_held_vector_is_an_edge_eoi() {
    let mut env = Env::new();
    assert_eq!(env.capture(0x40, true), Ok(Some(Capture::Level)));
    assert_eq!(env.capture(0x80, true), Ok(Some(Capture::Level)));
    accept(&env.page, 0x40);
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    // An accelerated edge IPI with vector 40h arrives and is delivered.
    assert_eq!(env.page.enqueue(0x40, false), Ok(true));
    accept(&env.page, 0x40);
    let ledger = env.irq;
    assert_eq!(env.write(0x80b, 0), WRITTEN);
    assert_eq!(env.irq, ledger);
    assert_eq!(env.apic.eois(), 0);
    assert!(!env.page.is_in_service(0x40));
}

#[test]
fn a_failed_level_completion_is_terminal_evidence() {
    let mut env = Env::new();
    // Inconsistent: held, but the physical ISR is empty.
    env.irq.commit_level_capture(0x40).unwrap();
    set_vector(&env.page, apic::ISR, 0x40);
    assert_eq!(
        env.write(0x80b, 0),
        Emulation::EoiFailed(IrqError::PhysicalIsrMismatch { vector: 0x40, highest: None })
    );
    // The virtual EOI had already happened.
    assert!(!env.page.is_in_service(0x40));
    assert!(env.apic.writes.is_empty());
}

#[test]
fn level_eoi_exit_fallback_handles_both_isr_states() {
    // ISR bit still set (fault semantics): cleared here, PPR recomputed.
    let mut env = Env::new();
    env.set(apic::TPR, 0x20);
    assert_eq!(env.capture(0x61, true), Ok(Some(Capture::Level)));
    accept(&env.page, 0x61);
    env.set(apic::PPR, 0x60);
    assert_eq!(irq::level_eoi_exit(0x61, 0x1000, &env.page, &mut env.irq, &mut env.apic), Ok(()));
    assert!(!env.page.is_in_service(0x61) && !env.page.is_level(0x61));
    assert_eq!(env.reg(apic::PPR), 0x20);
    assert_eq!(env.apic.eois(), 1);
    assert!(env.irq.is_empty() && env.apic.isr().is_empty());

    // ISR bit already clear (trap semantics): the ISR and PPR are left alone.
    let mut env = Env::new();
    set_vector(&env.page, apic::ISR, 0x30);
    assert_eq!(env.capture(0x62, true), Ok(Some(Capture::Level)));
    accept(&env.page, 0x62);
    // Hardware already performed the EOI and its PPR update (62h is bank 3, bit 2).
    env.set(apic::ISR + 48, env.reg(apic::ISR + 48) & !(1 << 2));
    assert!(!env.page.is_in_service(0x62) && env.page.is_in_service(0x30));
    env.set(apic::PPR, 0x33);
    assert_eq!(irq::level_eoi_exit(0x62, 0x1000, &env.page, &mut env.irq, &mut env.apic), Ok(()));
    assert_eq!(vectors(&env.page, apic::ISR), [0x30]);
    assert_eq!(env.reg(apic::PPR), 0x33);
    assert!(!env.page.is_level(0x62));
    assert_eq!(env.apic.eois(), 1);
    assert!(env.irq.is_empty());

    // An in-service vector that is not the highest is refused unchanged.
    let mut env = Env::new();
    assert_eq!(env.capture(0x62, true), Ok(Some(Capture::Level)));
    accept(&env.page, 0x62);
    set_vector(&env.page, apic::ISR, 0x70);
    let (before, ledger) = (snapshot(&env.page), env.irq);
    assert_eq!(
        irq::level_eoi_exit(0x62, 0x1000, &env.page, &mut env.irq, &mut env.apic),
        Err(IrqError::VirtualIsrMismatch { vector: 0x62, highest: Some(0x70) })
    );
    assert_eq!((snapshot(&env.page), env.irq), (before, ledger));

    // Not held: a stale TMR bit is cleared unless the vector is pending.
    let mut env = Env::new();
    set_vector(&env.page, apic::TMR, 0x63);
    set_vector(&env.page, apic::TMR, 0x64);
    set_vector(&env.page, apic::IRR, 0x64);
    assert_eq!(irq::level_eoi_exit(0x63, 0x1000, &env.page, &mut env.irq, &mut env.apic), Ok(()));
    assert_eq!(irq::level_eoi_exit(0x64, 0x1000, &env.page, &mut env.irq, &mut env.apic), Ok(()));
    assert!(!env.page.is_level(0x63));
    assert!(env.page.is_level(0x64) && env.page.is_pending(0x64));
    assert!(env.apic.writes.is_empty());
}

#[test]
fn capture_publishes_edges_holds_levels_and_ignores_spurious_interrupts() {
    // Spurious: no physical ISR bit, the host SVR vector.
    let mut env = Env::new();
    assert_eq!(irq::capture(0xff, &env.page, &mut env.irq, &mut env.apic), Ok(None));
    assert!(env.apic.writes.is_empty() && !env.page.is_pending(0xff));

    // Edge: published with TMR clear, then physically acknowledged.
    let mut env = Env::new();
    set_vector(&env.page, apic::TMR, 0x41);
    assert_eq!(env.capture(0x41, false), Ok(Some(Capture::Edge)));
    assert!(env.page.is_pending(0x41) && !env.page.is_level(0x41));
    assert_eq!(env.apic.writes, [(0x80b, 0)]);
    assert!(env.apic.isr().is_empty() && env.irq.is_empty());

    // Level: published with TMR set and held without a physical EOI.
    assert_eq!(env.capture(0x42, true), Ok(Some(Capture::Level)));
    assert!(env.page.is_pending(0x42) && env.page.is_level(0x42) && env.irq.holds(0x42));
    assert_eq!(env.apic.eois(), 1);
    assert_eq!(env.apic.isr(), [0x42]);

    // A higher edge source is acknowledged while the level source stays held.
    assert_eq!(env.capture(0x90, false), Ok(Some(Capture::Edge)));
    assert_eq!(env.apic.isr(), [0x42]);
    assert_eq!(env.apic.eois(), 2);

    // A vector below the physical highest is refused before publication.
    let mut env = Env::new();
    env.apic.raise(0x43, false);
    env.apic.raise(0x50, false);
    let before = snapshot(&env.page);
    assert_eq!(
        irq::capture(0x43, &env.page, &mut env.irq, &mut env.apic),
        Err(IrqError::PhysicalIsrMismatch { vector: 0x43, highest: Some(0x50) })
    );
    assert_eq!(snapshot(&env.page), before);
    assert!(env.apic.writes.is_empty());

    // A level source whose vector is already pending merges into that IRR
    // bit, takes the level type and is held (APM2 16.6.3 p648).
    let mut env = Env::new();
    env.page.enqueue(0x45, false).unwrap();
    assert_eq!(env.capture(0x45, true), Ok(Some(Capture::Level)));
    assert!(env.page.is_pending(0x45) && env.page.is_level(0x45) && env.irq.holds(0x45));
    assert!(env.apic.writes.is_empty());

    // The same with the vector in service: the EOI that ends the earlier
    // interrupt completes the held source, as a local APIC's EOI would
    // reach the I/O APIC; a still-asserted line is then captured again.
    let mut env = Env::new();
    set_vector(&env.page, apic::ISR, 0x47);
    assert_eq!(env.capture(0x47, true), Ok(Some(Capture::Level)));
    assert!(env.page.is_pending(0x47) && env.page.is_level(0x47) && env.irq.holds(0x47));
    assert_eq!(env.write(0x80b, 0), Emulation::Written);
    assert!(!env.page.is_in_service(0x47) && env.page.is_pending(0x47));
    assert!(env.irq.is_empty() && env.apic.eois() == 1);
    assert_eq!(env.capture(0x47, true), Ok(Some(Capture::Level)));
    assert!(env.irq.holds(0x47));

    // An edge source on a vector in service as level is published with the
    // edge type of this acceptance.
    let mut env = Env::new();
    set_vector(&env.page, apic::ISR, 0x46);
    set_vector(&env.page, apic::TMR, 0x46);
    assert_eq!(env.capture(0x46, false), Ok(Some(Capture::Edge)));
    assert!(env.page.is_pending(0x46) && !env.page.is_level(0x46));
    assert_eq!(env.apic.writes, [(0x80b, 0)]);

    // Vectors 0-31 never belong to the bridge. LVT sources cannot produce
    // 16-31 (refused above); a guest-programmed IOAPIC/MSI source can, and
    // the host IDT's window gates hand it here, where it stops.
    let mut env = Env::new();
    assert_eq!(env.capture(0x1f, false), Err(IrqError::ReservedVector(0x1f)));
}

#[test]
fn an_accepted_vector_without_physical_isr_is_the_extint_signature() {
    // An 8259 vector acknowledged through an unmasked ExtINT LINT0 sets no
    // local APIC ISR bit; only the host spurious vector may do that.
    for (vector, busy) in [(0x30u8, None), (0x68, Some(0x41u8)), (0xfe, None)] {
        let mut env = Env::new();
        if let Some(other) = busy {
            env.apic.raise(other, true);
        }
        let before = snapshot(&env.page);
        assert_eq!(
            irq::capture(vector, &env.page, &mut env.irq, &mut env.apic),
            Err(IrqError::NotInService(vector)),
            "{vector:#x}"
        );
        assert_eq!(snapshot(&env.page), before);
        assert!(env.apic.writes.is_empty() && env.irq.is_empty());
    }
    // The spurious vector keeps its no-ISR, no-EOI meaning even while
    // another source is in service.
    let mut env = Env::new();
    env.apic.raise(0x41, false);
    assert_eq!(irq::capture(0xff, &env.page, &mut env.irq, &mut env.apic), Ok(None));
    assert!(env.apic.writes.is_empty());
}

#[test]
fn captured_interface_keeps_loader_state_and_drops_read_only_bits() {
    // EDK2-style BSP: periodic timer with DS pending, LINT0 unmasked ExtINT
    // (virtual wire) with remote IRR, LINT1 NMI, thermal SMI unmasked.
    let lvts = [0x2_10ef, 0x0200, 0x1_0000, 0x5700, 0x0400, 0x1_10fe];
    let mut apic = loader_apic(0x1ff, lvts);
    let icr = 0x0000_0003_0000_14fd;
    let page = installed(&mut apic, icr);
    let interface = CapturedInterface::capture(&mut loader_apic(0x1ff, lvts), icr).unwrap();
    assert_eq!(interface.task_priority(), 0x20);
    assert_eq!(page.read_register(apic::TPR), Ok(0x20));
    // Nothing is in service yet: PPR equals TPR (16.6.4 p651).
    assert_eq!(page.read_register(apic::PPR), Ok(0x20));
    assert_eq!(page.read_register(apic::SVR), Ok(0x1ff));
    let stored = [0x2_00ef, 0x0200, 0x1_0000, 0x0700, 0x0400, 0x1_00fe];
    for (offset, value) in apic::LVTS.into_iter().zip(stored) {
        assert_eq!(page.read_register(offset), Ok(value), "{offset:#x}");
    }
    assert_eq!(page.read_register(apic::TIMER_INITIAL_COUNT), Ok(0x1234_5678));
    assert_eq!(page.read_register(apic::TIMER_DIVIDE), Ok(0xb));
    // The current count stays live; the backing field is not a mirror.
    assert_eq!(page.read_register(apic::TIMER_CURRENT_COUNT), Ok(0));
    // ICR bit 12 is the eliminated delivery status; the rest is kept.
    assert_eq!(page.read_register(apic::ICR), Ok(0x04fd));
    assert_eq!(page.read_register(apic::ICR_HIGH), Ok(3));
    // Faithful loader state needs no physical change, not even for the
    // unmasked ExtINT and SMI entries, and counts are never rewritten.
    assert!(apic.writes.is_empty());
    // A guest write of the same ExtINT or SMI value is still refused.
    let mut env = Env::new();
    assert_eq!(env.write(0x835, 0x0700), refused(Refusal::UnmaskedExtInt, 0x0700));
    assert_eq!(env.write(0x833, 0x0200), refused(Refusal::UnmaskedSmi, 0x0200));
}

#[test]
fn captured_software_disable_and_illegal_vectors_mask_the_physical_mirror() {
    // Software-disabled loader APIC: every stored LVT is masked, and a
    // physical LVT reported unmasked is masked before the host enables its SVR.
    let lvts = [0x0_00ef, 0x1_0000, 0x0_0400, 0x0_0700, 0x1_0400, 0x0_00fe];
    let mut apic = loader_apic(0x0ff, lvts);
    let page = installed(&mut apic, 0);
    for (offset, value) in apic::LVTS.into_iter().zip(lvts) {
        assert_eq!(u64::from(page.read_register(offset).unwrap()), value | MASK, "{offset:#x}");
    }
    assert_eq!(
        apic.writes,
        [(0x832, 0x1_00ef), (0x834, 0x1_0400), (0x835, 0x1_0700), (0x837, 0x1_00fe)]
    );
    assert_eq!(page.read_register(apic::SVR), Ok(0x0ff));
    // An unmasked fixed entry with an illegal vector is stored as captured;
    // only its physical mirror is masked. NMI ignores its vector. Vector 32
    // is the first one the host can accept.
    let lvts = [0x0_0005, 0x1_0000, 0x0_0402, 0x1_0000, 0x0_000f, 0x0_0020];
    let mut apic = loader_apic(0x1ff, lvts);
    let page = installed(&mut apic, 0);
    for (offset, value) in apic::LVTS.into_iter().zip(lvts) {
        assert_eq!(u64::from(page.read_register(offset).unwrap()), value, "{offset:#x}");
    }
    assert_eq!(apic.writes, [(0x832, 0x1_0005), (0x836, 0x1_000f)]);
}

#[test]
fn captured_state_outside_the_register_model_is_refused_without_effects() {
    let clean = [0x1_0000; 6];
    let mut cases: Vec<(u32, u64)> = vec![
        (0x808, 0x100),  // TPR 63:8
        (0x80f, 0x13ff), // SVR bit 12
        (0x80f, 1 << 32),
        (0x838, 1 << 32), // initial count 63:32
        (0x83e, 0x4),     // divide bit 2
        (0x83e, 0x10),
        (0x832, 0x1_0100), // timer message type bits
        (0x832, 0x5_0000), // timer bit 18 (no TSC deadline)
        (0x833, 0x1_0800), // thermal bit 11
        (0x835, 0x1_2000), // LINT0 bit 13 (no polarity bit)
        (0x836, 1 << 32),
    ];
    // Reserved message types: LINT 1/3/5/6, other LVTs also ExtINT.
    for message in [1u64, 3, 5, 6] {
        cases.push((0x835, 0x1_0000 | (message << 8)));
    }
    cases.push((0x837, 0x1_0700));
    cases.push((0x834, 0x0_0300));
    // Live fixed entries with an exception vector (16-31): the host IDT
    // cannot accept them. Masked, or with a non-fixed type, they are kept.
    for (msr, vector) in [(0x832u32, 0x10u64), (0x833, 0x1f), (0x834, 0x11), (0x837, 0x1e)] {
        cases.push((msr, vector));
    }
    let mut kept = loader_apic(0x1ff, [0x1_0010, 0x1_001f, 0x0_0411, 0x0_0710, 0x0_0410, 0x1_0010]);
    assert!(CapturedInterface::capture(&mut kept, 0).is_ok());
    for (msr, value) in cases {
        let mut apic = loader_apic(0x1ff, clean);
        apic.registers.insert(msr, value);
        assert_eq!(
            CapturedInterface::capture(&mut apic, 0),
            Err(CaptureRefusal { msr, value }),
            "{msr:#x}"
        );
        assert!(apic.writes.is_empty());
    }
    // ICR: every reserved bit but the delivery status refuses.
    for bit in [13u64, 16, 17, 20, 31] {
        let icr = 0x0000_0001_0000_10ef | (1 << bit);
        assert_eq!(
            CapturedInterface::capture(&mut loader_apic(0x1ff, clean), icr),
            Err(CaptureRefusal { msr: 0x830, value: icr }),
            "bit {bit}"
        );
    }
    assert!(
        CapturedInterface::capture(&mut loader_apic(0x1ff, clean), 0xffff_ffff_000c_16ff).is_ok()
    );
}

#[test]
fn init_preparation_is_read_only_and_commit_resets_physical_sources_msrpm_and_backing() {
    let (mut env, mut msrpm) = busy_init_env();
    let (before, ledger) = (snapshot(&env.page), env.irq);
    assert_eq!(registers::prepare_init(&env.page, &env.irq, &mut env.apic), Ok(()));
    assert!(env.apic.writes.is_empty());
    assert!(env.apic.reads.iter().all(|msr| (0x810..=0x817).contains(msr)));
    assert_eq!((snapshot(&env.page), env.irq), (before, ledger));
    assert!(map_bit(&msrpm, 0x80b, true));

    assert_eq!(registers::commit_init(&env.page, &mut env.irq, &mut env.apic, &mut msrpm), Ok(()));
    // Step 1: timer masked and stopped, divide 0, every other LVT masked.
    assert_eq!(
        env.apic.writes[..8],
        [
            (0x832, MASK),
            (0x838, 0),
            (0x83e, 0),
            (0x833, MASK),
            (0x834, MASK),
            (0x835, MASK),
            (0x836, MASK),
            (0x837, MASK)
        ]
    );
    // Step 2: three physical EOIs, highest first, and nothing held.
    assert_eq!(env.apic.writes[8..], [(0x80b, 0); 3]);
    assert!(env.apic.isr().is_empty() && env.irq.is_empty());
    assert_eq!(env.apic.registers[&0x80f], 0x1ff);
    // Step 3: EOI acceleration restored.
    assert!(!map_bit(&msrpm, 0x80b, true));
    let mut reference = Msrpm::native_boot();
    reference.configure_native_x2avic();
    assert_eq!(msrpm.bytes(), reference.bytes());
    // Step 4: Table 16-2 values; ID and version kept; LDR derived for ID 3.
    let mut expected: BTreeMap<u16, u32> = BTreeMap::new();
    for offset in (0..0x1000u16).step_by(16) {
        expected.insert(offset, 0);
    }
    expected.insert(apic::ID, 3);
    expected.insert(apic::VERSION, GUEST_APIC_VERSION);
    expected.insert(apic::LDR, 1 << 3);
    expected.insert(apic::SVR, 0xff);
    for offset in apic::LVTS {
        expected.insert(offset, 0x1_0000);
    }
    let after: BTreeMap<u16, u32> = (0..0x1000u16).step_by(16).zip(snapshot(&env.page)).collect();
    assert_eq!(after, expected);
    // INIT keeps the APIC_BASE shadow.
    assert_eq!(env.read(0x1b), Emulation::Read(BSP_BASE));
}

#[test]
fn init_preparation_refuses_foreign_or_missing_sources_and_foreign_identity() {
    let mut env = Env::new();
    env.apic.raise(0x70, false);
    assert_eq!(
        registers::prepare_init(&env.page, &env.irq, &mut env.apic),
        Err(InitError::Irq(IrqError::UnexpectedPhysicalIsr(0x70)))
    );

    let mut env = Env::new();
    env.irq.commit_level_capture(0x41).unwrap();
    assert_eq!(
        registers::prepare_init(&env.page, &env.irq, &mut env.apic),
        Err(InitError::Irq(IrqError::PhysicalIsrMismatch { vector: 0x41, highest: None }))
    );

    let mut env = Env::new();
    env.set(apic::ID, 512);
    assert_eq!(
        registers::prepare_init(&env.page, &env.irq, &mut env.apic),
        Err(InitError::Backing(Error::InvalidId))
    );
    env.set(apic::ID, 3);
    env.set(apic::VERSION, 0x8005_0010);
    assert_eq!(
        registers::prepare_init(&env.page, &env.irq, &mut env.apic),
        Err(InitError::Backing(Error::UnsupportedVersion))
    );
    assert!(env.apic.writes.is_empty());
}

#[test]
fn an_init_commit_without_preparation_stops_after_its_physical_reset() {
    let mut env = Env::new();
    let mut msrpm = Msrpm::native_boot();
    msrpm.configure_native_x2avic();
    env.irq.commit_level_capture(0x41).unwrap();
    assert!(msrpm.update_x2apic_eoi_intercept(&env.irq));
    env.set(apic::TPR, 0x20);
    assert_eq!(
        registers::commit_init(&env.page, &mut env.irq, &mut env.apic, &mut msrpm),
        Err(InitError::Irq(IrqError::PhysicalIsrMismatch { vector: 0x41, highest: None }))
    );
    // The physical reset happened; the backing reset and MSRPM change did not.
    assert_eq!(env.apic.writes.len(), 8);
    assert_eq!(env.reg(apic::TPR), 0x20);
    assert!(map_bit(&msrpm, 0x80b, true));
}
