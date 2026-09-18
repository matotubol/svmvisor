//! Debug-port trace output and the admission-hint shims shared by every profile.
#[cfg(feature = "native-resident-test")]
use core::arch::asm;

use svmvisor_hypervisor::host::paging::{self, PagingConfig};
use uefi_raw::Status;

#[cfg(feature = "native-resident-smp-activate")]
use super::physical;

// Shared qualification helpers also serve profiles without MP admission.
pub(super) fn admission_hint(predicate: u32, item: u64, observed: u64, expected: u64) {
    #[cfg(feature = "native-resident-smp-activate")]
    physical::admission_hint(predicate, item, observed, expected);
    #[cfg(not(feature = "native-resident-smp-activate"))]
    let _ = (predicate, item, observed, expected);
}

pub(super) fn admission_walk(
    error: paging::WalkError,
    cfg: PagingConfig,
    address: u64,
    last: Option<(u64, u64)>,
) {
    #[cfg(feature = "native-resident-smp-activate")]
    physical::admission_walk(error, cfg, address, last);
    #[cfg(not(feature = "native-resident-smp-activate"))]
    let _ = (error, cfg, address, last);
}

pub(super) fn unsupported(code: u64) -> Status {
    trace_error(code);
    Status::UNSUPPORTED
}

pub(super) fn trace_error(code: u64) {
    trace(b'!');
    for shift in [4, 0] {
        let nibble = ((code >> shift) & 15) as u8;
        trace(if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 });
    }
}

pub(super) fn trace_detail(error: &impl core::fmt::Debug) {
    #[cfg(feature = "native-resident-test")]
    {
        use core::fmt::Write;
        struct Bounded(usize);
        impl Write for Bounded {
            fn write_str(&mut self, value: &str) -> core::fmt::Result {
                if value.len() > self.0 {
                    return Err(core::fmt::Error);
                }
                self.0 -= value.len();
                for byte in value.bytes() {
                    trace(byte);
                }
                Ok(())
            }
        }
        trace(b'[');
        let _ = write!(&mut Bounded(192), "{error:?}");
        trace(b']');
    }
    #[cfg(not(feature = "native-resident-test"))]
    let _ = error;
}

pub(super) fn trace(code: u8) {
    #[cfg(feature = "native-resident-test")]
    unsafe {
        asm!("out dx, al", in("dx") 0xe9u16, in("al") code, options(nomem, nostack, preserves_flags));
    }
    #[cfg(not(feature = "native-resident-test"))]
    let _ = code;
}
