//! Pinned native physical IRQ ownership behind x2AVIC.
//!
//! APM2 rev3.44 15.13.1, 15.21.1-2, 15.29.3.1/Table15-22,
//! 15.29.9.2/Tables15-28/29 and 16.6.3/16.6.5. Physical EOI ordering
//! differs from guest EOI ordering; never acknowledge a lower physical ISR
//! while a higher physical source remains in service. No allocations or locks.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqError {
    ReservedVector(u8),
    PhysicalIsrMismatch { vector: u8, highest: Option<u8> },
    DuplicatePhysicalSource(u8),
    AmbiguousLevelSource(u8),
    UnownedLevelCompletion(u8),
    CompletionNotReady(u8),
    UnexpectedPhysicalIsr(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    /// The caller owns a virtual IRR publication, then one physical EOI.
    Edge,
    /// The physical EOI remains held until the corresponding virtual EOI.
    Level,
}

/// One per permanently pinned physical CPU; modified only with host IRQ
/// acceptance closed. Guest fixed edge IPIs do not pass through this ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalIrqLedger {
    held: [u32; 8],
    completed: [u32; 8],
}

impl PhysicalIrqLedger {
    pub const fn new() -> Self {
        Self { held: [0; 8], completed: [0; 8] }
    }

    pub fn is_empty(&self) -> bool { self.held.iter().all(|word| *word == 0) }

    pub fn holds(&self, vector: u8) -> bool {
        self.held[usize::from(vector / 32)] & (1 << (vector % 32)) != 0
    }

    /// Validate before any virtual bitmap publication or physical EOI.
    /// Ambiguous level/edge sharing is unsupported, rather than acknowledging
    /// an earlier virtual interrupt as completion of a newly captured source.
    pub fn prepare_capture(
        &self, vector: u8, level: bool, physical_highest: Option<u8>,
        virtual_pending: bool, virtual_in_service: bool,
    ) -> Result<Capture, IrqError> {
        if vector < 32 { return Err(IrqError::ReservedVector(vector)); }
        if physical_highest != Some(vector) {
            return Err(IrqError::PhysicalIsrMismatch { vector, highest: physical_highest });
        }
        if self.holds(vector) { return Err(IrqError::DuplicatePhysicalSource(vector)); }
        if level && (virtual_pending || virtual_in_service) {
            return Err(IrqError::AmbiguousLevelSource(vector));
        }
        Ok(if level { Capture::Level } else { Capture::Edge })
    }

    /// Commit only after successful prepare_capture and virtual IRR ownership
    /// publication, without opening host acceptance between prepare and commit.
    pub fn commit_level_capture(&mut self, vector: u8) -> Result<(), IrqError> {
        if vector < 32 { return Err(IrqError::ReservedVector(vector)); }
        if self.holds(vector) { return Err(IrqError::DuplicatePhysicalSource(vector)); }
        self.held[usize::from(vector / 32)] |= 1 << (vector % 32);
        Ok(())
    }

    /// The argument is AVIC_NOACCEL EXITINFO2[7:0], not highest virtual ISR:
    /// AVIC has already performed the guest EOI before this trap.
    pub fn complete_level(&mut self, vector: u8) -> Result<(), IrqError> {
        if !self.holds(vector) { return Err(IrqError::UnownedLevelCompletion(vector)); }
        let word = usize::from(vector / 32);
        let mask = 1 << (vector % 32);
        if self.completed[word] & mask != 0 {
            return Err(IrqError::UnownedLevelCompletion(vector));
        }
        self.completed[word] |= mask;
        Ok(())
    }

    /// Read the actual physical ISR anew after each EOI. A higher held source
    /// that the guest has not completed postpones lower completed sources.
    pub fn next_eoi(&self, physical_highest: Option<u8>) -> Result<Option<u8>, IrqError> {
        let owned_highest = highest(&self.held);
        let Some(vector) = physical_highest else {
            return match owned_highest {
                None => Ok(None),
                Some(vector) => Err(IrqError::PhysicalIsrMismatch { vector, highest: None }),
            };
        };
        if !self.holds(vector) { return Err(IrqError::UnexpectedPhysicalIsr(vector)); }
        if let Some(owned) = owned_highest {
            if owned != vector {
                return Err(IrqError::PhysicalIsrMismatch { vector: owned, highest: physical_highest });
            }
        }
        Ok((self.completed[usize::from(vector / 32)] & (1 << (vector % 32)) != 0).then_some(vector))
    }

    /// Call immediately after the physical EOI chosen by next_eoi succeeds.
    pub fn commit_eoi(&mut self, vector: u8) -> Result<(), IrqError> {
        let word = usize::from(vector / 32);
        let mask = 1 << (vector % 32);
        if self.held[word] & self.completed[word] & mask == 0 {
            return Err(IrqError::CompletionNotReady(vector));
        }
        self.held[word] &= !mask;
        self.completed[word] &= !mask;
        Ok(())
    }
}

impl Default for PhysicalIrqLedger { fn default() -> Self { Self::new() } }

pub fn highest(bitmap: &[u32; 8]) -> Option<u8> {
    for index in (0..8).rev() {
        if bitmap[index] != 0 {
            return Some((index * 32 + 31 - bitmap[index].leading_zeros() as usize) as u8);
        }
    }
    None
}

/// Read a physical x2APIC register without guest state aliasing.
///
/// # Safety
/// Ring0, physical x2APIC advertised and enabled, valid readable register MSR,
/// and exclusive same-CPU physical LAPIC ownership are required. This does not
/// validate an arbitrary guest MSR. APM2 rev3.44 16.9-16.11/Table16-6.
#[cfg(target_arch = "x86_64")]
pub unsafe fn read_physical_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe { core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nostack)); }
    u64::from(low) | (u64::from(high) << 32)
}

/// Acknowledge exactly the current highest physical ISR.
///
/// # Safety
/// Same physical-x2APIC requirements as read_physical_msr; host acceptance must
/// be closed, physical ISR highest must have been checked, and that source's
/// completion must be owned. APM2 rev3.44 16.6.5/16.11/Table16-6.
#[cfg(target_arch = "x86_64")]
pub unsafe fn physical_eoi() {
    unsafe { core::arch::asm!("wrmsr", in("ecx") 0x80bu32, in("eax") 0u32, in("edx") 0u32, options(nostack)); }
}

/// # Safety
/// See read_physical_msr; host acceptance remains closed for the complete scan.
#[cfg(target_arch = "x86_64")]
pub unsafe fn physical_highest_in_service() -> Option<u8> {
    let mut bitmap = [0; 8];
    for (index, word) in bitmap.iter_mut().enumerate() {
        *word = unsafe { read_physical_msr(0x810 + index as u32) } as u32;
    }
    highest(&bitmap)
}

/// # Safety
/// See read_physical_msr. Read for the just-accepted vector before physical EOI,
/// with host acceptance closed. APM2 rev3.44 16.6.3/Figure16-25.
#[cfg(target_arch = "x86_64")]
pub unsafe fn physical_level_triggered(vector: u8) -> bool {
    (unsafe { read_physical_msr(0x818 + u32::from(vector / 32)) }) & (1 << (vector % 32)) != 0
}
