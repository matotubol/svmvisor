//! CPU-local physical HWCR access for the admitted native cache profile.

use crate::arch::x86_64::msr::{HWCR_CPUID_FLT_EN, HWCR_IRPERF_EN};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwcrError {
    PmcVirtualization,
    PhysicalDrift { observed: u64, baseline: u64 },
    UnsupportedChange { current: u64, requested: u64 },
    Readback { observed: u64, expected: u64 },
}

/// CPU-local physical HWCR ownership for the admitted native cache profile.
/// PPR57896 rev3.00 pp188,203-204: IRPerfEn is a per-thread RW counter enable;
/// it is distinct from cache controls and the counter's read-only lock.
/// APM2 rev3.44 15.39: this direct-counter policy requires PMC virtualization
/// disabled. The native permission map keeps IRPerfCount accesses physical.
///
/// The caller first validates the stopped MSR instruction/CPL/continuation.
/// `baseline` is its immutable admitted HWCR capture; closures access only
/// this CPU's HWCR. Live hardware, including bit30, is authoritative, so no
/// shadow or shared-core bank can make RDMSR disagree with the actual enable.
/// CpuidFltEn (bit35, PPR p203) is also writable only when the caller owns
/// user CPUID fault injection. The advertised capability is Fn80000021.EAX17
/// (PPR p117); actual CPUID handling reads this live bit on the same CPU.
/// All other bits must retain the capture. Unsupported preparation performs
/// no write. A readback error is after a physical side effect and must stop;
/// it is not rollback. Commit guest state only after success.
pub fn access_hwcr(
    baseline: u64,
    requested: Option<u64>,
    inst_ret_counter: bool,
    pmc_virtualization: bool,
    cpuid_fault_owned: bool,
    mut read: impl FnMut() -> u64,
    mut write: impl FnMut(u64),
) -> Result<u64, HwcrError> {
    if pmc_virtualization {
        return Err(HwcrError::PmcVirtualization);
    }
    let allowed = HWCR_IRPERF_EN | if cpuid_fault_owned { HWCR_CPUID_FLT_EN } else { 0 };
    let current = read();
    if (current ^ baseline) & !allowed != 0 {
        return Err(HwcrError::PhysicalDrift { observed: current, baseline });
    }
    let Some(requested) = requested else {
        return Ok(current);
    };
    let changed = requested ^ current;
    if changed & !allowed != 0 || changed & HWCR_IRPERF_EN != 0 && !inst_ret_counter {
        return Err(HwcrError::UnsupportedChange { current, requested });
    }
    if changed == 0 {
        return Ok(current);
    }
    let expected = (current & !allowed) | (requested & allowed);
    write(expected);
    let observed = read();
    if observed != expected {
        return Err(HwcrError::Readback { observed, expected });
    }
    Ok(observed)
}
