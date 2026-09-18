//! Evidence emitters: the port-E9h debug text and diagnostic records.

#[cfg(feature = "resident-runtime-test")]
use core::arch::asm;

use crate::host::resident::runtime::diagnostics;
#[cfg(feature = "resident-runtime-test")]
use crate::{arch::x86_64::apic, host::resident::runtime::msr::read_msr};

/// GIF/IF clear, initialized per-CPU runtime; independent of mutable STATE.
pub(super) unsafe fn diagnostic_record(event: u8, fault: bool, context: [u64; 6], aux: u32) {
    unsafe {
        diagnostics::record(event, fault, context, aux);
    }
}

/// Read-only diagnostic observation of the strictly admitted physical x2APIC.
#[cfg(feature = "resident-runtime-test")]
pub(super) unsafe fn read_native_apic(offset: u16) -> u64 {
    unsafe { read_msr(apic::msr(offset)) }
}

pub(super) fn hex(value: u64) {
    for shift in (0..16).rev() {
        debug(&[b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]]);
    }
}

pub(super) fn debug(bytes: &[u8]) {
    #[cfg(feature = "resident-runtime-test")]
    for byte in bytes {
        unsafe {
            asm!("out dx,al",in("dx")0xe9u16,in("al")*byte,options(nomem,nostack));
        }
    }
    #[cfg(not(feature = "resident-runtime-test"))]
    let _ = bytes;
}
