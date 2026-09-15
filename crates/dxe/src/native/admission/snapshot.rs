//! Opt-in DXE host observations, not a native SVM admission token.
//! The helper runs after compiler entry/CPUID work; this is not a complete
//! original xstate or original image-entry register snapshot. It never reads
//! MSRs/debug registers, dereferences a descriptor table, or enables SVM.

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableSnapshot {
    /// Exact ten-byte SGDT/SIDT image: u16 limit followed by u64 base.
    pub bytes: [u8; 10],
    pub reserved: [u8; 6],
}
impl TableSnapshot {
    pub fn limit(&self) -> u16 {
        u16::from_le_bytes([self.bytes[0], self.bytes[1]])
    }
    pub fn base(&self) -> u64 {
        u64::from_le_bytes(self.bytes[2..10].try_into().unwrap())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeSnapshot {
    pub gdtr: TableSnapshot,
    pub idtr: TableSnapshot,
    pub cs: u16,
    pub ss: u16,
    pub ds: u16,
    pub es: u16,
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub rflags: u64,
}

const _: () = assert!(core::mem::size_of::<TableSnapshot>() == 16);
const _: () = assert!(core::mem::size_of::<NativeSnapshot>() == 72);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, gdtr) == 0);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, idtr) == 16);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, cs) == 32);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, ss) == 34);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, ds) == 36);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, es) == 38);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, cr0) == 40);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, cr3) == 48);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, cr4) == 56);
const _: () = assert!(core::mem::offset_of!(NativeSnapshot, rflags) == 64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError {
    PrivilegeLevel,
    UnexpectedStatus,
}

#[cfg(target_os = "uefi")]
unsafe extern "efiapi" {
    fn svmvisor_native_snapshot(out: *mut NativeSnapshot) -> u32;
}

/// Observe tables/selectors/control state after the caller's CPUID admission.
///
/// # Safety
/// Call only from the native preflight's synchronous firmware context with a
/// valid stack and memory. The helper refuses CPL!=0 before SGDT/SIDT/CR reads,
/// but supplies no fault containment for inaccessible memory or hostile VMM
/// intercepts. No descriptor-table contents or mappings are accessed/proven.
#[cfg(target_os = "uefi")]
pub unsafe fn capture() -> Result<NativeSnapshot, CaptureError> {
    let mut snapshot = NativeSnapshot::default();
    match unsafe { svmvisor_native_snapshot(&mut snapshot) } {
        0 => Ok(snapshot),
        1 => Err(CaptureError::PrivilegeLevel),
        _ => Err(CaptureError::UnexpectedStatus),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_image_decodes_independent_little_endian_fixture() {
        let table = TableSnapshot {
            bytes: [0x34, 0x12, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11],
            reserved: [0; 6],
        };
        assert_eq!(table.limit(), 0x1234);
        assert_eq!(table.base(), 0x1122334455667788);
    }
    #[test]
    fn abi_layout_matches_assembly_storage() {
        assert_eq!(core::mem::align_of::<NativeSnapshot>(), 8);
        assert_eq!(core::mem::offset_of!(TableSnapshot, bytes), 0);
        assert_eq!(core::mem::offset_of!(TableSnapshot, reserved), 10);
        assert_eq!(core::mem::size_of::<NativeSnapshot>(), 72);
        assert_eq!(core::mem::offset_of!(NativeSnapshot, rflags), 64);
    }
}
