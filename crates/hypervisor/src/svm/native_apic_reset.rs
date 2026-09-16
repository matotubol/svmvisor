//! Admission of host AMD extended LAPIC state before hiding guest extensions.
//!
//! This is host validation only. Guest INIT resets the virtual APIC and must
//! never reset physical device-interrupt ownership. PPR57896 rev3.00 pp64–65.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeApicResetError {
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
