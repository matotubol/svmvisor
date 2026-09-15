//! Explicit native loader profile: interpose the in-memory EBS function slot.
//! UEFI2.11 4.2 (table CRC), 7.4.6 (success/failure), 2.3.4.2 (x64 ABI).
//! No firmware/image patch persists across reset. All preparation precedes the
//! first EBS attempt; the assembly success path makes no Boot Services calls.
use super::*;

#[unsafe(no_mangle)]
static mut svmvisor_original_exit_boot_services: usize = 0;
#[unsafe(no_mangle)]
static svmvisor_boot_completed: AtomicU32 = AtomicU32::new(0);
// MAX means the native boundary did not reach Rust; otherwise start's exact
// bounded failure code remains available in retained DXE data.
#[unsafe(no_mangle)]
static mut svmvisor_boot_failure: u64 = u64::MAX;

unsafe extern "efiapi" {
    fn svmvisor_exit_boot_services(image: Handle, key: usize) -> Status;
}

pub(super) struct Prepared;
impl Prepared {
    /// # Safety
    /// BSP driver install, exclusive ownership of this live firmware table's
    /// EBS slot until commit; no other interposer may replace the slot meanwhile.
    /// Refuse non-writable/nonidentity tables without changing page permissions.
    pub(super) unsafe fn new(bs: *mut BootServices) -> Result<Self, Status> {
        let services = unsafe { &*bs };
        if services.header.size as usize != core::mem::size_of::<BootServices>()
            || services.header.reserved != 0
        {
            return Err(Status::UNSUPPORTED);
        }
        let processor = unsafe { cpu() }.map_err(unsupported)?;
        let cfg = unsafe { config(processor) }.map_err(unsupported)?;
        let mt = unsafe { mtrrs(processor.physical_bits) }.map_err(unsupported)?;
        let mut map = unsafe { memory::collect(services) }.map_err(preparation_map_failure)?;
        let checked = unsafe {
            mapped(
                map.descriptors(),
                cfg,
                &mt,
                rdmsr(0x277),
                bs as u64,
                core::mem::size_of::<BootServices>() as u64,
                true,
                false,
            )
        };
        map.release()?;
        checked.map_err(unsupported)?;
        Ok(Self)
    }

    /// # Safety
    /// Same exclusively owned table as new, with all allocation/MP work done.
    /// No shared Rust reference to the table is used across these raw writes.
    pub(super) unsafe fn commit(self, bs: *mut BootServices) {
        unsafe {
            let raise = (*bs).raise_tpl;
            let restore = (*bs).restore_tpl;
            let old = raise(Tpl::HIGH_LEVEL);
            // Snapshot the CURRENT table after all firmware allocation/MP calls.
            // No events can change its fields while this bounded CRC is computed.
            let mut candidate = ptr::read(bs);
            svmvisor_original_exit_boot_services = candidate.exit_boot_services as usize;
            candidate.exit_boot_services = svmvisor_exit_boot_services;
            candidate.header.crc = 0;
            let bytes = core::slice::from_raw_parts(
                ptr::addr_of!(candidate).cast::<u8>(),
                core::mem::size_of::<BootServices>(),
            );
            let mut crc = u32::MAX;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
                }
            }
            ptr::addr_of_mut!((*bs).exit_boot_services).write(svmvisor_exit_boot_services);
            ptr::addr_of_mut!((*bs).header.crc).write(!crc);
            restore(old);
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn svmvisor_boot_inner(
    _: *mut c_void,
    _: *mut c_void,
    _: *const NativeBoundary,
) {
    unsafe { card_boot::stage(2, 0, 0) };
    let result = unsafe { physical::start() };
    unsafe {
        svmvisor_boot_failure = result;
    }
    if result == 0 {
        unsafe { card_boot::stage(5, 0, 0) };
        svmvisor_boot_completed.store(1, Ordering::Release);
    } else {
        unsafe { card_boot::activation_failure(result) };
        trace_detail(&("native-boot-activation-failed", result));
    }
}
