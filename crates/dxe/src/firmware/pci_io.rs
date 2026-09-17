//! PCI I/O ABI prefix and DWORD-only BAR0 access. UEFI 2.10 section 14.4.
use crate::mmio::JournalMapping;
use core::ffi::c_void;
use svmvisor_dxe::diagnostics::journal::JournalIo;
use uefi_raw::Status;
use uefi_raw::table::boot::BootServices;
type MemAccess =
    unsafe extern "efiapi" fn(*const PciIo, u32, u8, u64, usize, *mut c_void) -> Status;
type ConfigRead = unsafe extern "efiapi" fn(*const PciIo, u32, u32, usize, *mut c_void) -> Status;
type GetLocation = unsafe extern "efiapi" fn(*const PciIo, *mut usize, *mut usize, *mut usize, *mut usize) -> Status;
type Attributes = unsafe extern "efiapi" fn(*const PciIo, u32, u64, *mut u64) -> Status;
type GetBarAttributes =
    unsafe extern "efiapi" fn(*const PciIo, u8, *mut u64, *mut *mut u8) -> Status;

#[derive(Clone, Copy)]
pub(crate) struct DecodeState(u32);

const MEMORY: u64 = 0x0200;
const MSE: u32 = 2;

pub(crate) fn status_result(status: Status) -> Result<(), Status> {
    if status.is_error() {
        Err(status)
    } else {
        Ok(())
    }
}

// Prefix through Attributes. UINT32 width is enum value 2.
// Unused slots are pointer-sized; no Map/AllocateBuffer/requester API is exposed.
#[repr(C)]
pub(crate) struct PciIo {
    poll_mem: usize,
    poll_io: usize,
    mem_read: MemAccess,
    mem_write: MemAccess,
    io_read: usize,
    io_write: usize,
    pci_read: ConfigRead,
    pci_write: usize,
    copy_mem: usize,
    map: usize,
    unmap: usize,
    allocate_buffer: usize,
    free_buffer: usize,
    flush: usize,
    get_location: GetLocation,
    attributes: Attributes,
    get_bar_attributes: GetBarAttributes,
}
const _: () = {
    assert!(core::mem::offset_of!(PciIo, mem_read) == 16);
    assert!(core::mem::offset_of!(PciIo, mem_write) == 24);
    assert!(core::mem::offset_of!(PciIo, pci_read) == 48);
    assert!(core::mem::offset_of!(PciIo, get_location) == 112);
    assert!(core::mem::offset_of!(PciIo, attributes) == 120);
    assert!(core::mem::offset_of!(PciIo, get_bar_attributes) == 128);
};

pub(crate) struct Bar0(pub(crate) *const PciIo);
impl Bar0 {
    #[cfg(any(feature = "card-load-only", feature = "card-returning-loader", feature = "card-resident"))]
    pub(crate) fn card_word(&self, offset: u64) -> Result<u32, Status> {
        if offset & 3 != 0 || offset > 0x100000 - 4 {
            return Err(Status::INVALID_PARAMETER);
        }
        let mut value = 0u32;
        // Exactly one DWORD per transaction: the flash bridge caps MRd length.
        status_result(unsafe {
            ((*self.0).mem_read)(self.0, 2, 1, offset, 1, (&mut value as *mut u32).cast())
        })?;
        Ok(value)
    }
    /// UEFI 2.10 §14.4.23 and §14.4.17: drivers enable required decoding in Start.
    /// Change MEMORY only, check the complete command word (including BME),
    /// and retain the original state for failed Start/Stop cleanup.
    pub(crate) fn enable_memory_decode(&mut self) -> Result<DecodeState, Status> {
        let original = self.config(4)? & 0xffff;
        if original & MSE != 0 {
            return Ok(DecodeState(original));
        }
        let mut supported = 0;
        status_result(unsafe { ((*self.0).attributes)(self.0, 4, 0, &mut supported) })?;
        if supported & MEMORY == 0 {
            return Err(Status::UNSUPPORTED);
        }
        let result = status_result(unsafe {
            ((*self.0).attributes)(self.0, 2, MEMORY, core::ptr::null_mut())
        })
        .and_then(|()| {
            if self.config(4)? & 0xffff != original | MSE {
                return Err(Status::DEVICE_ERROR);
            }
            Ok(DecodeState(original))
        });
        // Even an unsuccessful Enable may have partially taken effect.
        if result.is_err() {
            self.restore_memory_decode(DecodeState(original))?;
        }
        result
    }

    pub(crate) fn restore_memory_decode(&mut self, original: DecodeState) -> Result<(), Status> {
        if original.0 & MSE != 0 {
            return Ok(());
        }
        let restored = status_result(unsafe {
            ((*self.0).attributes)(self.0, 3, MEMORY, core::ptr::null_mut())
        });
        // Perform the read even if Disable reports an error; never report a
        // successful operation when cleanup failed or command bits changed.
        let command = self.config(4);
        restored?;
        if command? & 0xffff != original.0 {
            return Err(Status::DEVICE_ERROR);
        }
        Ok(())
    }

    pub(crate) fn journal_mapping(
        &self,
        services: &BootServices,
    ) -> Result<JournalMapping, Status> {
        let mut resource = core::ptr::null_mut();
        status_result(unsafe {
            ((*self.0).get_bar_attributes)(self.0, 0, core::ptr::null_mut(), &mut resource)
        })?;
        // SAFETY: GetBarAttributes returns the allocated descriptor described
        // by UEFI §14.4.18; validate it before any direct access and free now.
        let mapping = unsafe { JournalMapping::from_descriptor(resource) };
        if !resource.is_null() {
            status_result(unsafe { (services.free_pool)(resource) })?;
        }
        mapping
    }

    /// Optional terminal diagnostics for the exact Family 1Ah Model 44h B0
    /// target. All protocol calls finish in the parent's serialized Start path.
    /// PPR57896 rev3.00 pp40-41/210; UEFI2.11 14.4.16 (PDF730, printed646).
    #[cfg(feature = "card-resident")]
    pub(crate) fn terminal_endpoint(&mut self, journal_base: u64, boot_id: u32)
        -> Result<svmvisor_hypervisor::host::resident::terminal::TerminalEndpoint, Status>
    {
        use svmvisor_hypervisor::host::resident::terminal::{self, TerminalEndpoint};
        let vendor = core::arch::x86_64::__cpuid(0);
        if vendor.ebx != 0x6874_7541 || vendor.edx != 0x6974_6e65
            || vendor.ecx != 0x444d_4163 || vendor.eax < 1
            || core::arch::x86_64::__cpuid(1).eax
                != svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE
        { return Err(Status::UNSUPPORTED); }
        let (mut segment, mut bus, mut device, mut function) = (0usize, 0usize, 0usize, 0usize);
        status_result(unsafe { ((*self.0).get_location)(self.0, &mut segment, &mut bus,
            &mut device, &mut function) })?;
        if segment != 0 || bus > 255 || device > 31 || function > 7 {
            return Err(Status::UNSUPPORTED);
        }
        let segment_bdf = ((bus as u32) << 8) | ((device as u32) << 3) | function as u32;
        let low: u32;
        let high: u32;
        // Exact CPU identity above admits this processor-specific, read-only MSR.
        unsafe { core::arch::asm!("rdmsr", in("ecx") 0xc001_0058u32,
            out("eax") low, out("edx") high, options(nostack, preserves_flags)); }
        let mmio_config_msr = u64::from(low) | (u64::from(high) << 32);
        let config_page = TerminalEndpoint::config_page_from_msr(mmio_config_msr, segment_bdf)
            .ok_or(Status::UNSUPPORTED)?;
        if self.config(0)? != terminal::PCI_VENDOR_DEVICE
            || self.config(8)? != terminal::PCI_CLASS_REVISION
            || (self.config(0x0c)? >> 16) & 0xff != 0
        { return Err(Status::UNSUPPORTED); }
        let command = self.config(4)? as u16;
        let bar0_raw = self.config(0x10)?;
        let endpoint = TerminalEndpoint { config_page, bar0_host_page: journal_base,
            fpga_build_id: u64::from(self.read(8)?) | (u64::from(self.read(12)?) << 32),
            rom_build_id: u64::from(self.read(16)?) | (u64::from(self.read(20)?) << 32),
            mmio_config_msr, bar0_raw, segment_bdf, boot_id, command, version: 1, reserved: 0 };
        if !endpoint.valid() { return Err(Status::UNSUPPORTED); }
        Ok(endpoint)
    }

    pub(crate) fn config(&self, offset: u32) -> Result<u32, Status> {
        let mut value = 0u32;
        status_result(unsafe {
            ((*self.0).pci_read)(self.0, 2, offset, 1, (&mut value as *mut u32).cast())
        })?;
        Ok(value)
    }
    pub(crate) fn identity(&self) -> Result<(), Status> {
        if self.config(0)? != 0x066610ee || self.config(8)? != 0xff000003 {
            return Err(Status::UNSUPPORTED);
        }
        Ok(())
    }
}
impl JournalIo for Bar0 {
    fn read(&mut self, offset: u64) -> Result<u32, Status> {
        let mut value = 0u32;
        status_result(unsafe {
            ((*self.0).mem_read)(self.0, 2, 0, offset, 1, (&mut value as *mut u32).cast())
        })?;
        Ok(value)
    }
    fn write(&mut self, offset: u64, mut value: u32) -> Result<(), Status> {
        status_result(unsafe {
            ((*self.0).mem_write)(self.0, 2, 0, offset, 1, (&mut value as *mut u32).cast())
        })
    }
}
