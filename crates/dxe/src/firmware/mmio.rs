//! Direct access only to the prepared 4 KiB journal, UEFI 2.10 §§2.3.4,14.4.18.
use svmvisor_dxe::diagnostics::journal::JournalIo;
use uefi_raw::Status;

#[derive(Clone, Copy)]
pub(crate) struct JournalMapping(usize);
impl JournalMapping {
    /// Only the resident card loader hands the journal base to its child.
    #[cfg(feature = "card-resident")]
    pub(crate) fn physical_base(self) -> u64 {
        self.0 as u64
    }

    /// # Safety
    /// `descriptor` must point to the allocated resource descriptor returned by
    /// PCI I/O GetBarAttributes(0). The host range must retain its default UC,
    /// identity-mapped UEFI mapping until the ExitBootServices callback returns.
    /// UEFI §14.4.18 returns a QWORD resource descriptor with a host address.
    pub(crate) unsafe fn from_descriptor(descriptor: *const u8) -> Result<Self, Status> {
        if descriptor.is_null() {
            return Err(Status::DEVICE_ERROR);
        }
        // QWORD address-space descriptor: tag, size=43, memory, general flags,
        // specific flags (non-cacheable), granularity, min, max, translation, len.
        let byte = |offset| unsafe { *descriptor.add(offset) };
        let qword = |offset| unsafe { descriptor.add(offset).cast::<u64>().read_unaligned() };
        if byte(0) != 0x8a
            || byte(1) != 43
            || byte(2) != 0
            || byte(3) != 0
            || byte(5) & 6 != 0
            || qword(6) != 32
            || qword(38) != 4096
        {
            return Err(Status::UNSUPPORTED);
        }
        let base = qword(14);
        if base == 0 || base > 0xfffff000 || base & 4095 != 0 {
            return Err(Status::UNSUPPORTED);
        }
        Ok(Self(base as usize))
    }
}
impl JournalIo for JournalMapping {
    fn read(&mut self, offset: u64) -> Result<u32, Status> {
        if offset > 0x09c || offset & 3 != 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        // SAFETY: prepared UC journal mapping; range/alignment checked above.
        Ok(unsafe { ((self.0 + offset as usize) as *const u32).read_volatile() })
    }
    fn write(&mut self, offset: u64, value: u32) -> Result<(), Status> {
        if !(0x040..=0x060).contains(&offset) || offset & 3 != 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        // SAFETY: prepared UC mapping; full DWORD staging/commit offsets only.
        unsafe { ((self.0 + offset as usize) as *mut u32).write_volatile(value) };
        Ok(())
    }
}
