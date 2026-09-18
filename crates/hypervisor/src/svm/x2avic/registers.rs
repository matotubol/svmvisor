//! Guest x2APIC register owner of the exclusive x2AVIC profile.
//!
//! This module is the single source of truth for which x2APIC MSR accesses
//! the per-vCPU MSRPM intercepts before AVIC, and it emulates every
//! intercepted access. AMD APM2 rev3.44: Table 15-22 pp566-568 makes the
//! SVR/LVT/timer/ESR writes AVIC traps that follow the backing-page store,
//! too late for the #GP(0) cases of chapter 16; 15.11 p518 checks the MSRPM
//! before MSR-specific exceptions, and 15.29.10 p583 checks x2APIC MSR
//! intercepts before AVIC permissions. An intercepted RDMSR/WRMSR is
//! VMEXIT_MSR with a valid nRIP (15.7.1 p509).
//!
//! Register rules: Table 16-6 p658 and 16.11 pp657-659, with the figures of
//! 16.3-16.6. Product facts: PPR 57896 rev3.00 (Family 1Ah Model 44h B0)
//! pp52-65 and pp174-185.
//!
//! The physical LAPIC stays host-owned. Guest-visible values live in the AVIC
//! backing page; validated timer and LVT values are mirrored to the same
//! physical register (read-only bits clear). The same rules admit the
//! register state the loader left behind (`CapturedInterface`). Every
//! operation runs with the guest stopped on its own CPU and host interrupt
//! acceptance closed.
use super::{
    BackingPage, Error,
    irq::{self, IrqError, PhysicalIrqLedger},
};
use crate::{
    arch::x86_64::apic::{
        self, LVT_MASKED, MESSAGE_EXTERNAL, MESSAGE_FIXED, MESSAGE_NMI, MESSAGE_SMI,
        PhysicalX2Apic, SVR_SOFTWARE_ENABLE,
    },
    memory::address::AddressPolicy,
    svm::permission_maps::{MsrAccess, Msrpm},
};

const ID: u32 = apic::ID_MSR;
const VERSION: u32 = apic::msr(apic::VERSION);
const TPR: u32 = apic::msr(apic::TPR);
const APR: u32 = apic::msr(apic::APR);
const PPR: u32 = apic::msr(apic::PPR);
const EOI: u32 = apic::msr(apic::EOI);
const LDR: u32 = apic::msr(apic::LDR);
const SVR: u32 = apic::msr(apic::SVR);
const ISR_FIRST: u32 = apic::msr(apic::ISR);
const IRR_LAST: u32 = apic::msr(apic::IRR) + 7;
const ESR: u32 = apic::msr(apic::ESR);
const ICR: u32 = apic::ICR_MSR;
const LVT_TIMER: u32 = apic::msr(apic::LVT_TIMER);
const LVT_ERROR: u32 = apic::msr(apic::LVT_ERROR);
const INITIAL_COUNT: u32 = apic::msr(apic::TIMER_INITIAL_COUNT);
const CURRENT_COUNT: u32 = apic::TIMER_CURRENT_COUNT_MSR;
const DIVIDE: u32 = apic::msr(apic::TIMER_DIVIDE);
const SELF_IPI: u32 = apic::SELF_IPI_MSR;

/// TPR bits 7:0 (Figure 16-26 p650); bits 63:8 are reserved.
const TPR_VALID: u64 = 0xff;
/// Writable SVR bits 9:0 (Figure 16-17 p641; PPR p176 marks 63:10 reserved,
/// so bit 12 is reserved too).
const SVR_VALID: u64 = 0x3ff;
/// Divide Value bits 3 and 1:0; bit 2 and bits 31:4 are MBZ (Figure 16-11
/// p637, Table 16-3 p638).
const DIVIDE_VALID: u64 = 0xb;
/// Delivery status (bit 12), read-only in every LVT (Figure 16-7 p635).
const LVT_DELIVERY_STATUS: u32 = 1 << 12;
/// Remote IRR (bit 14), read-only in LINT0/LINT1 (Figure 16-12 p638).
const LVT_REMOTE_IRR: u32 = 1 << 14;
/// APIC_BASE bit 9 and bits 7:0 are MBZ (Figure 16-2 p630, Figure 16-31 p655).
const APIC_BASE_RESERVED_LOW: u64 = (1 << 9) | 0xff;

/// One standard LVT layout in the 64-bit x2APIC view. Bits 63:32 are
/// reserved in every non-ICR register (16.11.3 p659).
#[derive(Clone, Copy)]
struct Lvt {
    /// Bits that are neither reserved nor read-only.
    valid: u32,
    /// Read-only bits: ignored on write and stored as zero. The APM does not
    /// say whether writing 1 faults; PPR p27 Table 8 has "Read-only: writes
    /// are ignored" (decision).
    read_only: u32,
    /// Message types admitted for this source (Table 16-1 p628); `None` for
    /// the timer, whose bits 11:8 are reserved (Figure 16-8 p636; PPR p180).
    messages: Option<&'static [u8]>,
}

impl Lvt {
    fn of(offset: u16) -> Self {
        match offset {
            // Figure 16-8: 17 TMM, 16 M, 12 DS, 7:0 VEC. The timer-specific
            // figure, not the generic Figure 16-7, governs bits 11:8 (decision).
            // Bit 18 is reserved: no TSC-deadline mode exists.
            apic::LVT_TIMER => {
                Self { valid: 0x0003_10ff, read_only: LVT_DELIVERY_STATUS, messages: None }
            }
            // Figure 16-12: 16 M, 15 TGM, 14 RIR, 12 DS, 10:8 MT, 7:0 VEC.
            // LINT has no Table 16-1 row; Figure 16-7's legal types apply.
            apic::LVT_LINT0 | apic::LVT_LINT1 => Self {
                valid: 0x0001_d7ff,
                read_only: LVT_DELIVERY_STATUS | LVT_REMOTE_IRR,
                messages: Some(&[MESSAGE_FIXED, MESSAGE_SMI, MESSAGE_NMI, MESSAGE_EXTERNAL]),
            },
            // Figures 16-13/16-14/16-15: 16 M, 12 DS, 10:8 MT, 7:0 VEC. Table
            // 16-1 admits Fixed, SMI or NMI for these sources.
            _ => Self {
                valid: 0x0001_17ff,
                read_only: LVT_DELIVERY_STATUS,
                messages: Some(&[MESSAGE_FIXED, MESSAGE_SMI, MESSAGE_NMI]),
            },
        }
    }

    const fn writable(self) -> u32 {
        self.valid & !self.read_only
    }

    /// D2/D3 for one value of the LVT at `offset`, with the virtual APIC
    /// software-enabled or not. `None`: a reserved bit is set, so a WRMSR of
    /// the value raises #GP(0) (16.11.3 p659). Otherwise the read-only bits
    /// are dropped, a software-disabled APIC forces the mask, and the value
    /// carries the refusal a guest write of it receives:
    /// - a message type Table 16-1 p628 or Figure 16-7 p635 does not admit
    ///   for the source (masked or not); the manual gives no fault or ignore
    ///   rule (decision);
    /// - an unmasked SMI or ExtINT entry;
    /// - an unmasked fixed entry with vector 16-31.
    ///
    /// An unmasked fixed entry with vector 0-15 is an illegal-vector APIC
    /// error, not #GP (Figure 16-7 p635): it is kept, but its physical mirror
    /// is masked because the virtual APIC never delivers it.
    fn check(offset: u16, value: u64, software_enabled: bool) -> Option<LvtValue> {
        let lvt = Self::of(offset);
        if value & !u64::from(lvt.valid) != 0 {
            return None;
        }
        let mut entry = value as u32 & lvt.writable();
        if !software_enabled {
            entry |= LVT_MASKED;
        }
        let masked = entry & LVT_MASKED != 0;
        // The timer has no message-type field (its bits 11:8 are reserved and
        // refused above), so it is always fixed.
        let message = ((entry >> 8) & 7) as u8;
        let refusal = match lvt.messages {
            Some(admitted) if !admitted.contains(&message) => Some(Refusal::UnsupportedMessageType),
            _ if masked => None,
            _ if message == MESSAGE_SMI => Some(Refusal::UnmaskedSmi),
            _ if message == MESSAGE_EXTERNAL => Some(Refusal::UnmaskedExtInt),
            _ if message == MESSAGE_FIXED && (16..32).contains(&(entry & 0xff)) => {
                Some(Refusal::ExceptionVector)
            }
            _ => None,
        };
        let illegal = !masked && message == MESSAGE_FIXED && entry & 0xff < 16;
        let mirror = if illegal { entry | LVT_MASKED } else { entry };
        Some(LvtValue { entry, mirror, refusal })
    }
}

/// One LVT value admitted by `Lvt::check`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LvtValue {
    /// The guest-visible (backing-page) register value.
    entry: u32,
    /// The value of the same physical LVT.
    mirror: u32,
    /// Set when a guest write of this value is a stopped refusal.
    refusal: Option<Refusal>,
}

/// Outcome of one intercepted guest RDMSR/WRMSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emulation {
    /// Complete the RDMSR with EDX:EAX = value and continue at nRIP.
    Read(u64),
    /// Complete the WRMSR and continue at nRIP.
    Written,
    /// Queue #GP(0) at the unchanged RIP. Nothing changed.
    GeneralProtection,
    /// Stop with evidence (`value` is the requested WRMSR value, 0 for a
    /// read). Nothing changed.
    Refused { reason: Refusal, value: u64 },
    /// Stop: a software EOI cleared the virtual ISR bit, then its host
    /// level-source completion failed.
    EoiFailed(IrqError),
}

/// Stopped unsupported access, with a stable wire code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Refusal {
    /// An access that `intercepted` leaves to hardware, or an MSR outside
    /// APIC_BASE and 800h-8FFh: the intercept profile and the exit disagree.
    UnownedAccess = 1,
    /// An LVT message type that Table 16-1 or Figure 16-7 does not admit for
    /// that source. The manual does not say fault or ignore.
    UnsupportedMessageType = 2,
    /// Unmasked SMI LVT. With HWCR SmmLock set, SMIs are not intercepted in
    /// SVM (PPR p204), so the guest would reach platform SMM.
    UnmaskedSmi = 3,
    /// Unmasked ExtINT LINT: the vector comes from the 8259 and sets no
    /// physical ISR bit, so the IRQ bridge cannot own the source.
    UnmaskedExtInt = 4,
    /// APIC_BASE AE=EXTD=0 from x2APIC mode. Valid per Figure 16-32 p656,
    /// but the exclusive x2APIC profile cannot leave x2APIC mode (documented
    /// deviation).
    ApicDisable = 5,
    /// APIC_BASE base change in x2APIC mode; Figure 16-32 has no such
    /// transition and the manual gives no rule (decision).
    ApicRelocation = 6,
    /// Unmasked fixed LVT with vector 16-31. The APIC accepts these vectors
    /// (Figure 16-7 p635 names only 0-15 illegal), but vectors 0-31 are
    /// reserved for exceptions (APM2 8.2 p245, Table 8-1 p246), so the host
    /// IDT cannot accept such an interrupt for the IRQ bridge.
    ExceptionVector = 7,
}

/// Whether the MSRPM intercepts one x2APIC MSR access (D1) before AVIC.
/// Accesses left to hardware are the reads that Table 15-22 allows and the
/// writes it accelerates: TPR, EOI, ICR and SELF IPI. EOI writes are also
/// intercepted while a level source is held (`Msrpm::
/// update_x2apic_eoi_intercept`). Every other access in 800h-8FFh, and any
/// MSR outside that range, is intercepted.
pub const fn intercepted(msr: u32, access: MsrAccess) -> bool {
    let hardware = match access {
        MsrAccess::Read => matches!(msr,
            ID | VERSION | TPR | PPR | LDR | SVR | ISR_FIRST..=ESR | ICR
                | LVT_TIMER..=INITIAL_COUNT | DIVIDE),
        MsrAccess::Write => matches!(msr, TPR | EOI | ICR | SELF_IPI),
    };
    !hardware
}

/// Implemented x2APIC registers of the presented guest APIC (Table 16-6
/// p658). The AMD extended registers 840h-853h are absent because the guest
/// version register has bit 31 (extended space) clear; the APM gives no rule
/// for 840h-853h in that case (decision).
const fn implemented(msr: u32) -> bool {
    matches!(msr, ID | VERSION | TPR | APR | PPR | EOI | LDR | SVR | ISR_FIRST..=ESR | ICR
        | LVT_TIMER..=CURRENT_COUNT | DIVIDE | SELF_IPI)
}

const fn x2apic_msr(msr: u32) -> bool {
    msr >= apic::X2APIC_MSR_FIRST && msr <= apic::X2APIC_MSR_LAST
}

const fn refused(reason: Refusal, value: u64) -> Emulation {
    Emulation::Refused { reason, value }
}

/// Read a constant, aligned backing-page register.
fn load(backing: &BackingPage, offset: u16) -> u32 {
    backing.read_register(offset).unwrap_or(0)
}

/// Store a constant, aligned backing-page register; that store cannot fail.
fn store(backing: &BackingPage, offset: u16, value: u32) {
    let _ = backing.write_register_stopped(offset, value);
}

/// APR (MSR 809h) from the backing page, Figure 16-22 p647: AP is the
/// highest priority class of TP, the highest ISR bit and the highest IRR
/// bit; APS equals TPS when AP equals TP, and is zero otherwise.
fn arbitration_priority(backing: &BackingPage) -> u32 {
    let tpr = load(backing, apic::TPR) & 0xff;
    let class = |vector: Option<u8>| vector.map_or(0, |vector| u32::from(vector) & 0xf0);
    let priority =
        (tpr & 0xf0).max(class(backing.highest_in_service())).max(class(backing.highest_pending()));
    if priority == tpr & 0xf0 { tpr } else { priority }
}

/// Guest-visible x2APIC state outside the backing page: the APIC_BASE shadow,
/// the physical-address width that bounds its base field, and the IRR held
/// when the virtual APIC was last software-disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestX2Apic {
    apic_base: u64,
    apic_base_reserved: u64,
    /// IRR banks pending when SVR bit 8 was last cleared (16.3.1 p629:
    /// "Pending interrupts in the ISR and IRR are held").
    held_at_disable: [u32; 8],
}

impl GuestX2Apic {
    /// Admit the physical APIC_BASE captured at arm, which must be enabled
    /// x2APIC at FEE0_0000h (BSC may be set). Base bits at or above the
    /// admitted physical-address width are reserved (Figure 16-2 p630 notes
    /// shorter implementations; PPR p121 reserves bits 63:48 on this product).
    /// A captured software-disabled APIC holds nothing: its never-entered
    /// backing page has no pending interrupt of its own.
    pub fn admit(apic_base: u64, policy: &AddressPolicy) -> Result<Self, Error> {
        if apic_base & !(apic::APIC_BASE_BSP | apic::APIC_BASE_X2APIC)
            != apic::APIC_BASE_DEFAULT_ADDRESS
            || apic_base & apic::APIC_BASE_X2APIC != apic::APIC_BASE_X2APIC
        {
            return Err(Error::UnsupportedApicBase);
        }
        let above_width = !0u64 << policy.physical_bits();
        Ok(Self {
            apic_base,
            apic_base_reserved: above_width | APIC_BASE_RESERVED_LOW,
            held_at_disable: [0; 8],
        })
    }

    /// Guest INIT, together with `commit_init`: the reset backing page is
    /// software-disabled (SVR FFh, Table 16-2 p631) with an empty IRR, so
    /// nothing is held.
    pub fn reset_after_init(&mut self) {
        self.held_at_disable = [0; 8];
    }

    /// Emulate one intercepted RDMSR (`write` is `None`) or WRMSR of `msr`
    /// (APIC_BASE or 800h-8FFh) with EDX:EAX `write`.
    pub fn emulate(
        &mut self,
        msr: u32,
        write: Option<u64>,
        backing: &BackingPage,
        irq: &mut PhysicalIrqLedger,
        physical: &mut impl PhysicalX2Apic,
    ) -> Emulation {
        match write {
            None => self.read(msr, backing, physical),
            Some(value) => self.write(msr, value, backing, irq, physical),
        }
    }

    fn read(
        &self,
        msr: u32,
        backing: &BackingPage,
        physical: &mut impl PhysicalX2Apic,
    ) -> Emulation {
        match msr {
            // Every guest read returns the shadow; INIT leaves it unchanged
            // (16.10 p657: AE and EXTD; Figure 16-2 p630: ABA).
            apic::APIC_BASE => Emulation::Read(self.apic_base),
            _ if !x2apic_msr(msr) => refused(Refusal::UnownedAccess, 0),
            // Unimplemented and reserved MSRs, including 80Ch/80Eh/831h and
            // 840h-8FFh (16.11.1 pp657-659).
            _ if !implemented(msr) => Emulation::GeneralProtection,
            // Write-only: SELF IPI (16.15 p662); EOI is "WO" in Table 16-6
            // and Error-on-read in PPR p175 (decision).
            EOI | SELF_IPI => Emulation::GeneralProtection,
            APR => Emulation::Read(u64::from(arbitration_priority(backing))),
            // The live mirrored physical timer (Figure 16-9 p637); RDMSR
            // returns zero for reserved bits 63:32 (16.11.3 p659). PPR p43:
            // the count may step by 1 to 8.
            CURRENT_COUNT => Emulation::Read(physical.read(CURRENT_COUNT) & 0xffff_ffff),
            _ => refused(Refusal::UnownedAccess, 0),
        }
    }

    fn write(
        &mut self,
        msr: u32,
        value: u64,
        backing: &BackingPage,
        irq: &mut PhysicalIrqLedger,
        physical: &mut impl PhysicalX2Apic,
    ) -> Emulation {
        match msr {
            apic::APIC_BASE => self.write_apic_base(value),
            _ if !x2apic_msr(msr) => refused(Refusal::UnownedAccess, value),
            _ if !implemented(msr) => Emulation::GeneralProtection,
            // 802h: 16.12 p660. The other registers are only "RO" in Table
            // 16-6; PPR pp174-182 marks them Error-on-write (decision).
            ID | VERSION | APR | PPR | LDR | ISR_FIRST..=IRR_LAST | CURRENT_COUNT => {
                Emulation::GeneralProtection
            }
            // Table 16-6 p658: #GP(0) if a non-zero value is written.
            EOI if value != 0 => Emulation::GeneralProtection,
            EOI => match irq::software_eoi(backing, irq, physical) {
                Ok(_) => Emulation::Written,
                Err(error) => Emulation::EoiFailed(error),
            },
            // 16.11.3 p659: a non-zero ESR write faults. The x2APIC write
            // semantics are otherwise unstated; the virtual error state is
            // always empty because virtual APIC error generation is not
            // modeled (decision).
            ESR if value != 0 => Emulation::GeneralProtection,
            ESR => {
                store(backing, apic::ESR, 0);
                Emulation::Written
            }
            SVR => self.write_svr(value, backing, irq, physical),
            LVT_TIMER..=LVT_ERROR => {
                let offset = ((msr - apic::X2APIC_MSR_FIRST) << 4) as u16;
                self.write_lvt(offset, value, backing, physical)
            }
            // Figure 16-10 p637: 31:0 R/W. A non-zero value restarts the
            // physical timer, zero stops it (16.4.1 p636).
            INITIAL_COUNT if value >> 32 != 0 => Emulation::GeneralProtection,
            INITIAL_COUNT => {
                store(backing, apic::TIMER_INITIAL_COUNT, value as u32);
                physical.write(INITIAL_COUNT, value);
                Emulation::Written
            }
            // All eight bit-3,1:0 encodings are defined (Table 16-3 p638).
            DIVIDE if value & !DIVIDE_VALID != 0 => Emulation::GeneralProtection,
            DIVIDE => {
                store(backing, apic::TIMER_DIVIDE, value as u32);
                physical.write(DIVIDE, value);
                Emulation::Written
            }
            // TPR, ICR and SELF IPI writes belong to hardware.
            _ => refused(Refusal::UnownedAccess, value),
        }
    }

    /// Figure 16-17 p641 and 16.3.1 p629: with ASE (bit 8) clear, "Pending
    /// interrupts in the ISR and IRR are held. Further fixed, lowest-priority,
    /// and ExtInt interrupts are not accepted. All LVT entry mask bits are set
    /// and cannot be cleared" (PPR p57 agrees). The physical SVR stays
    /// host-owned.
    ///
    /// - Disable: the pending IRR is recorded as held, and the masks are
    ///   forced in the backing LVTs and their physical mirrors.
    /// - Re-enable: the masks stay set until the guest rewrites each LVT
    ///   (decision; the APM gives no rule). Before the enable is stored,
    ///   every IRR bit that appeared while disabled is withdrawn with its TMR
    ///   bit, except the level sources this CPU's IRQ ledger still holds. The
    ///   software fan-out and the IRQ bridge publish no edge interrupt into a
    ///   disabled page, so these bits come from hardware-accelerated IPIs:
    ///   x2AVIC delivers them without consulting the virtual SVR. The guest
    ///   is stopped, so it cannot take them between the withdrawal and the
    ///   enable, and a real APIC would never have accepted them. A vector
    ///   pending at the disable stays pending even if it arrived again.
    fn write_svr(
        &mut self,
        value: u64,
        backing: &BackingPage,
        irq: &PhysicalIrqLedger,
        physical: &mut impl PhysicalX2Apic,
    ) -> Emulation {
        if value & !SVR_VALID != 0 {
            return Emulation::GeneralProtection;
        }
        let svr = value as u32;
        let enabled = backing.software_enabled();
        if svr & SVR_SOFTWARE_ENABLE == 0 {
            if enabled {
                self.held_at_disable = backing.pending_banks();
            }
            for offset in apic::LVTS {
                let entry = (load(backing, offset) & Lvt::of(offset).writable()) | LVT_MASKED;
                store(backing, offset, entry);
                physical.write(apic::msr(offset), u64::from(entry));
            }
        } else if !enabled {
            let pending = backing.pending_banks();
            let owned = irq.held();
            let arrived: [u32; 8] = core::array::from_fn(|index| {
                pending[index] & !self.held_at_disable[index] & !owned[index]
            });
            backing.discard_pending(&arrived);
            self.held_at_disable = [0; 8];
        }
        store(backing, apic::SVR, svr);
        Emulation::Written
    }

    /// Validate, store and mirror one LVT (D2/D3, `Lvt::check`). Reserved
    /// bits fault and refused values stop, both without effects. Unmasked NMI
    /// entries are mirrored; physical NMIs reach the guest in guest mode, and
    /// an NMI during a host GIF window remains terminal.
    fn write_lvt(
        &self,
        offset: u16,
        value: u64,
        backing: &BackingPage,
        physical: &mut impl PhysicalX2Apic,
    ) -> Emulation {
        let enabled = load(backing, apic::SVR) & SVR_SOFTWARE_ENABLE != 0;
        let Some(lvt) = Lvt::check(offset, value, enabled) else {
            return Emulation::GeneralProtection;
        };
        if let Some(reason) = lvt.refusal {
            return refused(reason, value);
        }
        store(backing, offset, lvt.entry);
        physical.write(apic::msr(offset), u64::from(lvt.mirror));
        Emulation::Written
    }

    /// APIC_BASE write (D4). Reserved bits fault first; BSC is read-only
    /// (Figure 16-2 p630) and the effect of writing it is unstated, so the
    /// written value is ignored (decision).
    /// From x2APIC mode (Table 16-5 p655, Figure 16-32 p656): AE=0 with
    /// EXTD=1 is invalid and x2APIC to xAPIC is not a transition, so both
    /// fault; x2APIC to disabled is valid but unsupported here; staying in
    /// x2APIC mode completes only with an unchanged base.
    fn write_apic_base(&self, value: u64) -> Emulation {
        if value & self.apic_base_reserved != 0 {
            return Emulation::GeneralProtection;
        }
        match value & apic::APIC_BASE_X2APIC {
            apic::APIC_BASE_X2APIC if (value ^ self.apic_base) & apic::APIC_BASE_ADDRESS == 0 => {
                Emulation::Written
            }
            apic::APIC_BASE_X2APIC => refused(Refusal::ApicRelocation, value),
            0 => refused(Refusal::ApicDisable, value),
            _ => Emulation::GeneralProtection,
        }
    }
}

/// A captured physical register value the guest register model cannot
/// present: `msr` held `value`, which has a reserved bit set (a guest WRMSR of
/// it raises #GP(0), and 16.11.3 p659 makes RDMSR return zero for reserved
/// bits) or, for an LVT, a message type with no defined meaning or a live
/// fixed vector 16-31.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRefusal {
    pub msr: u32,
    pub value: u64,
}

impl CaptureRefusal {
    /// Whether `msr` is one of the registers `CapturedInterface::capture`
    /// reads, the only MSRs a refusal can name (arm evidence decoding).
    pub const fn captured(msr: u32) -> bool {
        matches!(msr, TPR | SVR | ICR | INITIAL_COUNT | DIVIDE | LVT_TIMER..=LVT_ERROR)
    }
}

/// The x2APIC register interface the loader left on this CPU, admitted as
/// the guest's initial virtual state when the resident host takes the
/// physical LAPIC over: TPR, SVR, the six LVTs, the timer initial count and
/// divide configuration, and the command register readback. Every value
/// passes the guest-write rules (D2/D3) before `install` changes anything.
///
/// Faithful state is kept where the guest-write model would only refuse to
/// *reprogram* it: an unmasked ExtINT LINT (common for firmware virtual-wire
/// mode on the BSP) or an unmasked SMI entry stays as captured and stays live
/// physically. A later guest write of the same value is still refused, and a
/// source accepted through such an ExtINT LINT stops the IRQ bridge as
/// `IrqError::NotInService`. Refused here: a reserved LVT message type, which
/// has no defined meaning (Figure 16-7 p635; PPR 57896 rev3.00 p55 "all other
/// message types are Reserved"; decision), and a live fixed entry with vector
/// 16-31, which the host could never accept (`Refusal::ExceptionVector`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturedInterface {
    tpr: u32,
    svr: u32,
    /// Stored LVT entry and, when the physical LVT must change, its mirror.
    lvts: [(u32, Option<u32>); 6],
    initial_count: u32,
    divide: u32,
    icr: u64,
}

impl CapturedInterface {
    /// Read-only capture from `physical`, with `icr` as the command readback
    /// (the physical ICR, or the BSP's value saved before the physical
    /// bootstrap overwrote it).
    ///
    /// - Read-only LVT bits (delivery status, remote IRR) are dropped, as for
    ///   a guest write (PPR p27 Table 8: "writes are ignored"; decision U14).
    /// - A captured SVR with bit 8 clear forces every LVT mask (16.3.1 p629:
    ///   "All LVT entry mask bits are set and cannot be cleared").
    /// - The physical LVT changes only where the mirror masks a source the
    ///   captured value leaves unmasked (software-disabled APIC, or a fixed
    ///   entry with an illegal vector).
    /// - ICR bit 12 is the eliminated delivery status and is dropped rather
    ///   than refused, the same rule the AVIC_INCOMPLETE_IPI handler applies
    ///   to the backing ICR (decision); the other ICR reserved bits refuse.
    /// - The counts and divide configuration are already the physical values
    ///   and are never rewritten here (a count write would restart the timer,
    ///   16.4.1 p636).
    pub fn capture(physical: &mut impl PhysicalX2Apic, icr: u64) -> Result<Self, CaptureRefusal> {
        let mut read = |msr: u32, valid: u64| {
            let value = physical.read(msr);
            if value & !valid == 0 { Ok(value as u32) } else { Err(CaptureRefusal { msr, value }) }
        };
        let tpr = read(TPR, TPR_VALID)?;
        let svr = read(SVR, SVR_VALID)?;
        let initial_count = read(INITIAL_COUNT, u32::MAX.into())?;
        let divide = read(DIVIDE, DIVIDE_VALID)?;
        let enabled = svr & SVR_SOFTWARE_ENABLE != 0;
        let mut lvts = [(0, None); 6];
        for (slot, offset) in lvts.iter_mut().zip(apic::LVTS) {
            let msr = apic::msr(offset);
            let value = physical.read(msr);
            let lvt = match Lvt::check(offset, value, enabled) {
                Some(lvt)
                    if !matches!(
                        lvt.refusal,
                        Some(Refusal::UnsupportedMessageType | Refusal::ExceptionVector)
                    ) =>
                {
                    lvt
                }
                _ => return Err(CaptureRefusal { msr, value }),
            };
            let current = value as u32 & Lvt::of(offset).writable();
            *slot = (lvt.entry, (lvt.mirror != current).then_some(lvt.mirror));
        }
        if icr & apic::ICR_RESERVED & !apic::ICR_DELIVERY_STATUS != 0 {
            return Err(CaptureRefusal { msr: ICR, value: icr });
        }
        Ok(Self { tpr, svr, lvts, initial_count, divide, icr: icr & !apic::ICR_DELIVERY_STATUS })
    }

    /// The captured task priority (TPR bits 7:0, Figure 16-26 p650).
    pub const fn task_priority(&self) -> u8 {
        self.tpr as u8
    }

    /// Store the admitted interface into this vCPU's backing page before its
    /// first guest entry, then apply the physical LVT mask changes. The
    /// caller owns the physical LAPIC, sets the host-owned physical TPR and
    /// SVR afterwards, and excludes every other writer of these backing
    /// registers (remote publishers change only IRR and TMR).
    ///
    /// The never-entered page has an empty ISR, so PPR equals the captured
    /// TPR (16.6.4 p651: PP is the higher of TP and the in-service class, and
    /// PPS equals TPS when PP equals TP). AVIC uses the backing PPR to gate
    /// delivery (15.29.3.1 p569), so it must not stay at its reset zero.
    pub fn install(&self, backing: &BackingPage, physical: &mut impl PhysicalX2Apic) {
        store(backing, apic::TPR, self.tpr);
        store(backing, apic::PPR, self.tpr);
        store(backing, apic::SVR, self.svr);
        store(backing, apic::TIMER_INITIAL_COUNT, self.initial_count);
        store(backing, apic::TIMER_DIVIDE, self.divide);
        store(backing, apic::ICR, self.icr as u32);
        store(backing, apic::ICR_HIGH, (self.icr >> 32) as u32);
        for ((entry, mirror), offset) in self.lvts.into_iter().zip(apic::LVTS) {
            store(backing, offset, entry);
            if let Some(mirror) = mirror {
                physical.write(apic::msr(offset), u64::from(mirror));
            }
        }
    }
}

/// Failure of the LAPIC half of a guest INIT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    Backing(Error),
    Irq(IrqError),
}

/// Guest INIT preparation (D9), read-only: the backing page keeps an admitted
/// identity, and the physical ISR holds exactly the ledger's level sources,
/// so the retirement drain in `commit_init` cannot fail.
pub fn prepare_init(
    backing: &BackingPage,
    irq: &PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
) -> Result<(), InitError> {
    backing.check_init_identity().map_err(InitError::Backing)?;
    irq.check_retirement(&apic::in_service_banks(physical)).map_err(InitError::Irq)
}

/// Guest INIT LAPIC commit (D9 steps 1-4), after `prepare_init` succeeded and
/// the other INIT preparation of the runtime. A failure is terminal: earlier
/// steps have taken effect.
///
/// 1. The physical timer and mirrored LVTs take their INIT values (Table
///    16-2 p631); a zero initial count stops the timer (16.4.1 p636).
/// 2. Every held level source is completed and physically acknowledged in
///    physical ISR order.
/// 3. The now empty ledger restores EOI acceleration in `msrpm`. The caller
///    clears the VMCB clean bits (the CPU INIT commit does).
/// 4. The backing page takes its INIT values
///    (`BackingPage::reset_after_init_stopped`).
///
/// The APIC_BASE shadow is unchanged (16.10 p657).
pub fn commit_init(
    backing: &BackingPage,
    irq: &mut PhysicalIrqLedger,
    physical: &mut impl PhysicalX2Apic,
    msrpm: &mut Msrpm,
) -> Result<(), InitError> {
    physical.write(LVT_TIMER, u64::from(LVT_MASKED));
    physical.write(INITIAL_COUNT, 0);
    physical.write(DIVIDE, 0);
    for offset in [
        apic::LVT_THERMAL,
        apic::LVT_PERFORMANCE,
        apic::LVT_LINT0,
        apic::LVT_LINT1,
        apic::LVT_ERROR,
    ] {
        physical.write(apic::msr(offset), u64::from(LVT_MASKED));
    }
    irq::retire(irq, physical).map_err(InitError::Irq)?;
    msrpm.update_x2apic_eoi_intercept(irq);
    backing.reset_after_init_stopped().map_err(InitError::Backing)
}
