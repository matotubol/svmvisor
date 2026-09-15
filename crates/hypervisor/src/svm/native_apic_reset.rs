//! Read-only admission and bounded writes for target-owned native guest INIT.
//!
//! This owns physical LAPIC reset state, not the synthetic `LocalApic` model.
//! APM2 rev3.44 16.3.2/16.10 and PPR57896 rev3.00 (Family1Ah Model44h B0)
//! 2.1.11.2.1.15/2.1.11.2.2, pp55–65 and MSR registers pp175–185.
use super::{x2apic::ApicMode, ipi::NativeDestinationMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeApicResetError {
    UnsupportedMode,
    UnsupportedLayout { version: u64 },
    UnsupportedSignature { signature: u32 },
    UnavailableRegister(u16),
    RegisterState { offset: u16, value: u64 },
}

/// Read-only admission before hiding the AMD extension interface. The caller
/// has already validated the PPR57896 extended profile. Do not acknowledge or
/// reconfigure inherited sources: every defined IER bit must be enabled and
/// extended LVT entries masked/idle. PPR pp64/65 and MSR848h p184; low IER
/// reserved bits are not compared (MMIO reset table reports all ones).
pub fn admit_hidden_native_apic_state(
    mut read: impl FnMut(u16) -> u64,
) -> Result<(), NativeApicResetError> {
    for offset in (0x480..=0x4f0).step_by(16) {
        let value = read(offset);
        let defined = if offset == 0x480 { 0xffff_0000 } else { u32::MAX as u64 };
        if value > u32::MAX as u64 || value & defined != defined {
            return Err(NativeApicResetError::RegisterState { offset, value });
        }
    }
    for offset in (0x500..=0x530).step_by(16) {
        let value = read(offset);
        if value > u32::MAX as u64 || value & 0x11000 != 0x10000 {
            return Err(NativeApicResetError::RegisterState { offset, value });
        }
    }
    Ok(())
}

/// A preflight for one immediately following INIT commit on the same CPU.
///
/// The caller owns the stopped guest and live physical LAPIC with IF/GIF clear,
/// excludes external nonvectored events and concurrent APIC writers, and must
/// not enable SVR or resume between preparation and commit. Preparation never
/// writes any register. A refused guest INIT therefore preserves stopped state.
/// Software disable holds old IRR/ISR and prevents new fixed/lowest-priority/
/// ExtInt acceptance (PPR p57); merely clearing IF/GIF would not suffice.
#[derive(Debug, PartialEq, Eq)]
pub struct NativeApicReset {
    mode: ApicMode,
    extended: bool,
    extended_control: u32,
}

impl NativeApicReset {
    /// Read only registers proven present by the native version/model before
    /// inspecting the complete supported interrupt state. Register offsets are
    /// shared by xAPIC MMIO and their x2APIC MSR aliases (800h + offset/16).
    ///
    /// The nonextended six-LVT profile retains the emulator contract. Extended
    /// state is specifically the 00B40F40h processor signature and PPR layout;
    /// this is not admission of arbitrary AMD extended APIC implementations.
    pub fn prepare(
        signature: u32,
        mode: ApicMode,
        mut read: impl FnMut(u16) -> Option<u64>,
    ) -> Result<Self, NativeApicResetError> {
        if mode == ApicMode::Disabled {
            return Err(NativeApicResetError::UnsupportedMode);
        }
        let version = read(0x030).ok_or(NativeApicResetError::UnavailableRegister(0x030))?;
        // MLE=5 describes the six standard entries at 320h..370h. Do not
        // silently ignore an additional LVT advertised by another version.
        if version > u32::MAX as u64 || version & 0x7eff_ff00 != 5 << 16 || version & 0xf0 != 0x10 {
            return Err(NativeApicResetError::UnsupportedLayout { version });
        }
        let extended = version & (1 << 31) != 0;
        if extended && version != 0x8105_0010 {
            return Err(NativeApicResetError::UnsupportedLayout { version });
        }
        if extended && signature != 0x00b4_0f40 {
            return Err(NativeApicResetError::UnsupportedSignature { signature });
        }
        let mut extended_control = 0;
        if extended {
            expect(&mut read, 0x400, u64::MAX, 0x0004_0007)?;
            extended_control = expect(&mut read, 0x410, !7, 0)? as u32;
            if mode == ApicMode::XApic && extended_control & 4 == 0 {
                return Err(NativeApicResetError::RegisterState {
                    offset: 0x410, value: extended_control as u64,
                });
            }
        }
        // Check software disable before checking volatile pending state.
        expect(&mut read, 0x0f0, 1 << 8, 0)?;
        // Resettable configuration is not an outstanding interrupt. TPR is
        // writable; APR/PPR derive from TPR and the separately checked empty
        // IRR/ISR (APM2 16.6.2/16.6.4; PPR pp53/56). Clearing TPR resets that
        // priority without EOI. ESR is cleared by the existing double write
        // (PPR p60), even if its visible latch was already nonzero.
        for offset in [0x080, 0x0a0, 0x280] {
            expect(&mut read, offset, !0xff, 0)?;
        }
        // A masked timer cannot generate an interrupt (PPR p55); the LVT
        // mask/idle checks below are still mandatory. Writing initial count
        // zero clears current count and stops it (APM2 16.4.1; PPR pp55/63).
        // Requiring both counts already zero wrongly refused this reset.
        for offset in [0x380, 0x390] {
            expect(&mut read, offset, !u64::from(u32::MAX), 0)?;
        }
        // PPR defines APR also in x2APIC mode. The generic emulator profile
        // does not promise MSR809h, so only sample it when its layout admits it.
        if extended || mode == ApicMode::XApic {
            expect(&mut read, 0x090, !0xff, 0)?;
        }
        if mode == ApicMode::XApic {
            // Pending physical ICR writes must complete before reset (PPR p55).
            expect(&mut read, 0x300, 1 << 12, 0)?;
        }
        for bank in 0..8 {
            for base in [0x100, 0x180, 0x200] {
                expect(&mut read, base + bank * 16, u64::MAX, 0)?;
            }
        }
        for offset in (0x320..=0x370).step_by(16) {
            // Mask=1, delivery status=0, LINT remote IRR=0. Checking bit14
            // on the other LVTs conservatively requires their reserved bit to be zero.
            expect(&mut read, offset, 0x15000, 0x10000)?;
        }
        if extended {
            for offset in (0x500..=0x530).step_by(16) {
                expect(&mut read, offset, 0x11000, 0x10000)?;
            }
        }
        Ok(Self {
            mode,
            extended,
            extended_control,
        })
    }

    /// Physical backing stays in exact-ID mode. The guest exposes no AMD
    /// extension register and therefore has no guest ExtApicIdEn reset bit.
    pub fn destination_mode(&self) -> NativeDestinationMode {
        match (self.mode, self.extended) {
            (ApicMode::X2Apic, _) => NativeDestinationMode::X2Apic,
            (_, true) => NativeDestinationMode::ExtendedXApic8,
            _ => NativeDestinationMode::XApic,
        }
    }

    /// Emit the infallible local register commit after all CPU/auxiliary state
    /// validation succeeds. The callback performs native accesses; it must not
    /// allocate, call firmware, reenable interrupts, or modify the guest VMCB.
    ///
    /// No EOI/SEOI or physical ICR write occurs: those acknowledge a source or
    /// send a new interrupt rather than resetting stored state. The existing
    /// NativeIcr owner resets guest ICR readback. APIC_BASE and ID are retained.
    pub fn writes(&self, mut write: impl FnMut(u16, u32)) {
        write(0x080, 0);
        for offset in (0x320..=0x370).step_by(16) {
            write(offset, 0x10000);
        }
        write(0x380, 0);
        write(0x3e0, 0);
        if self.mode == ApicMode::XApic {
            write(0x0d0, 0);
            // PPR p57 requires reserved DFR bits zero; generic APM Table16-2
            // supplies FFFFFFFFh for the separately admitted legacy profile.
            write(0x0e0, if self.extended { 0xf000_0000 } else { u32::MAX });
        }
        if self.extended {
            for offset in (0x500..=0x530).step_by(16) {
                // PPR pp65/185 takes precedence over generic APM Table16-2's
                // zero for this processor: all four extended LVTs are masked.
                write(offset, 0x10000);
            }
            // PPR pp64/184: IerEn enables IER writes. Keep the other controls
            // stable until defined IER bits have reset. ExtApicIdEn belongs to
            // the physical transport, and must remain set (PPR p64).
            write(0x410, self.extended_control | 1);
            // IER[15:0] are reserved MBZ (APM16.7.2; PPR MSR848h p184).
            write(0x480, 0xffff_0000);
            for offset in (0x490..=0x4f0).step_by(16) {
                write(offset, u32::MAX);
            }
            write(0x410, 4);
        }
        // PPR p60: first write latches/clears internal errors, second clears
        // the visible latch. Reading zero alone does not prove internal zero.
        write(0x280, 0);
        write(0x280, 0);
        write(0x0f0, 0xff);
    }
}

fn expect(
    read: &mut impl FnMut(u16) -> Option<u64>,
    offset: u16,
    mask: u64,
    expected: u64,
) -> Result<u64, NativeApicResetError> {
    let value = read(offset).ok_or(NativeApicResetError::UnavailableRegister(offset))?;
    if value & mask != expected {
        return Err(NativeApicResetError::RegisterState { offset, value });
    }
    Ok(value)
}
