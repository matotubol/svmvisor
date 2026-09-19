//! Guest-owned MCAX machine-check MSRs on the admitted native target.
//!
//! APM2 rev3.44 15.11 p518/Table 15-8: the MSRPM covers 0000_0000h-1FFFh,
//! C000_0000h-1FFFh and C001_0000h-1FFFh, and "any attempt to read or write
//! an MSR not covered by the MSRPM will automatically cause an intercept".
//! PPR57896 rev3.00 3.1.2.2.4 p300-301/Table 23 places the MCAX banks at
//! MSRC000_2[3FF:000] (64 banks of 16 registers), outside every covered
//! range, so each access exits although the legacy aliases (0400h-047Fh) run
//! natively (`Msrpm::native_boot`). The guest owns machine-check state (no
//! #MC intercept), so the runtime repeats the access on the same CPU.
//!
//! The host must not fault. PPR p300: unimplemented and unused registers in
//! this space are RAZ/WRIG. PPR 3.1.2.3 p301 and HWCR p203 give the one #GP:
//! a non-zero write to an implemented MCA_STATUS while McStatusWrEn is 0.
//! Decision: that write faults for every bank's STATUS offset, implemented or
//! not (the manual gives no implemented-bank test); an operating system
//! clears STATUS with zero. Table 23 labels register offset 7 "Reserved"
//! without calling it unused, so it is answered here as RAZ/WRIG and never
//! reaches hardware. MSRC000_2[FFF:400] is reserved and stays stopped.

pub const FIRST: u32 = 0xc000_2000;
pub const LAST: u32 = 0xc000_23ff;
/// Register offsets within a bank (Table 23 p301).
const STATUS: u32 = 1;
const RESERVED: u32 = 7;

/// What the runtime does for one intercepted access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// RDMSR on this CPU; the value completes the instruction.
    Read,
    /// Complete the RDMSR with zero, or drop the WRMSR; no hardware access.
    ReadZeroIgnoreWrite,
    /// WRMSR of this value on this CPU.
    Write(u64),
    /// Queue #GP(0) at the unchanged RIP; no hardware access.
    GeneralProtection,
}

/// `None`: not an MCAX MSR. `write` is the WRMSR EDX:EAX, `status_writable`
/// this CPU's HWCR.McStatusWrEn.
pub fn plan(index: u32, write: Option<u64>, status_writable: bool) -> Option<Access> {
    if !(FIRST..=LAST).contains(&index) {
        return None;
    }
    if index & 0xf == RESERVED {
        return Some(Access::ReadZeroIgnoreWrite);
    }
    Some(match write {
        None => Access::Read,
        Some(value) if index & 0xf == STATUS && value != 0 && !status_writable => {
            Access::GeneralProtection
        }
        Some(value) => Access::Write(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_64_bank_window_is_owned_and_only_status_can_fault() {
        for index in [0xc000_1fff, 0xc000_2400, 0xc000_2fff, 0x401] {
            assert_eq!(plan(index, None, false), None);
            assert_eq!(plan(index, Some(0), true), None);
        }
        assert_eq!(plan(FIRST, None, false), Some(Access::Read));
        assert_eq!(plan(LAST, Some(u64::MAX), false), Some(Access::Write(u64::MAX)));
        // Windows enables bank 0 with all ones and clears STATUS with zero.
        assert_eq!(plan(0xc000_2000, Some(u64::MAX), false), Some(Access::Write(u64::MAX)));
        assert_eq!(plan(0xc000_2001, Some(0), false), Some(Access::Write(0)));
        assert_eq!(plan(0xc000_23f1, Some(1 << 63), false), Some(Access::GeneralProtection));
        assert_eq!(plan(0xc000_23f1, Some(1 << 63), true), Some(Access::Write(1 << 63)));
        for write in [None, Some(u64::MAX)] {
            assert_eq!(plan(0xc000_2007, write, true), Some(Access::ReadZeroIgnoreWrite));
        }
        // DESTAT (offset 8) is plain read-write (PPR p323).
        assert_eq!(plan(0xc000_2008, Some(1), false), Some(Access::Write(1)));
    }
}
