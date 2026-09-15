//! Observe a filtered PAUSE without emulating it. APM2 rev3.44 15.14.4:
//! VMRUN reloads the internal count. Reentry at unchanged RIP executes the
//! interrupted PAUSE with a replenished nonzero budget; TF, RF, instruction
//! bytes and real/protected/long-mode behavior remain hardware-owned.
//! This is not a watchdog: a guest with no PAUSE and no other exits is invisible.
use super::vmcb::Vmcb;

/// Caller owns the stopped native VMCB and its normal pending-event lifecycle.
/// This function changes no guest state. The configuration must have been
/// admitted on this CPU by `Vmcb::configure_native_pause_filter`.
pub fn native_pause_retry_ready(vmcb: &Vmcb) -> bool {
    let b = vmcb.bytes();
    vmcb.exit_snapshot().code == 0x77
        && u32::from_le_bytes(b[0x00c..0x010].try_into().unwrap()) & (1 << 23) != 0
        && u16::from_le_bytes(b[0x03c..0x03e].try_into().unwrap()) == 0
        && u16::from_le_bytes(b[0x03e..0x040].try_into().unwrap()) == 4096
}
