//! Retained native cache-control observations for the Family1Ah Model44h host.
//! PPR57896 rev3.00 pp123,126-130,173,202-206,210; APM2 rev3.44 7.7/7.9.
//! These records do not grant permission to write physical cache controls.

use crate::{
    arch::x86_64::msr::{
        HWCR, IORR_BASE0, MMIO_CFG_BASE_ADDR, MTRR_CAP, MTRR_DEF_TYPE, MTRR_FIXED, MTRR_VAR_BASE0,
        PAT, SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN,
        SYS_CFG_MTRR_FIX_DRAM_MOD_EN, TARGET_PHYSICAL_BITS, TARGET_SIGNATURE, TOM2, TOP_MEM,
    },
    memory::mtrrs::{DEF_TYPE_E, DEF_TYPE_FE, VARIABLE_VALID, valid_type},
    sync::TryLock,
};

pub use crate::svm::cache::{
    hwcr::{HwcrError, access_hwcr},
    survey::CacheSurvey,
};

mod hwcr;
mod survey;
#[cfg(test)]
mod tests;

pub(crate) const MAX_CACHE_CPUS: usize = 32;
/// Last owned MtrrVarMask (eight pairs) and IORR_MASK (two pairs).
const VAR_LAST: u32 = MTRR_VAR_BASE0 + 15;
const IORR_LAST: u32 = IORR_BASE0 + 3;

/// Complete local physical observation. Fixed bytes include their otherwise
/// hidden RdMem/WrMem bits; the capture owner temporarily exposes and restores
/// that thread's SYS_CFG19. Physical scope differs by field (notably PAT/HWCR).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheObservation {
    pub capability: u64,
    pub default: u64,
    pub sys_cfg: u64,
    pub top_mem: u64,
    pub top_mem2: u64,
    pub iorr: [u64; 4],
    pub hwcr: u64,
    pub mmconfig: u64,
    pub pat: u64,
    pub variable: [(u64, u64); 8],
    pub fixed: [u64; 11],
    /// Raw Fn8000001E EAX/EBX/ECX and Fn80000008 ECX. Interpretation and
    /// membership admission belong to the target topology owner.
    pub topology: [u32; 4],
}

const _: () = {
    assert!(core::mem::size_of::<CacheObservation>() == 328);
    assert!(core::mem::align_of::<CacheObservation>() == 8);
    assert!(core::mem::size_of::<CacheCapture>() == 3 * 4096);
};

impl CacheObservation {
    pub(crate) const EMPTY: Self = Self {
        capability: 0,
        default: 0,
        sys_cfg: 0,
        top_mem: 0,
        top_mem2: 0,
        iorr: [0; 4],
        hwcr: 0,
        mmconfig: 0,
        pat: 0,
        variable: [(0, 0); 8],
        fixed: [0; 11],
        topology: [0; 4],
    };

    /// Bounded target-specific sampling, used by the actual MP observer and
    /// local pre-entry revalidation. Closures own permitted physical MSR
    /// access on the current CPU. SYS_CFG19 is restored before every return
    /// after its temporary change; no address-routing field is modified.
    pub fn capture(
        signature: u32,
        width: u8,
        topology: Option<[u32; 4]>,
        read: impl FnMut(u32) -> u64,
        write: impl FnMut(u32, u64),
    ) -> Option<Self> {
        Self::capture_detailed(
            signature,
            width,
            topology.ok_or(CacheAdmissionFailure::new(1, 0x8000001e, 0, 1)),
            read,
            write,
        )
        .ok()
    }
    pub fn capture_detailed(
        signature: u32,
        width: u8,
        topology: Result<[u32; 4], CacheAdmissionFailure>,
        mut read: impl FnMut(u32) -> u64,
        mut write: impl FnMut(u32, u64),
    ) -> Result<Self, CacheAdmissionFailure> {
        if signature != TARGET_SIGNATURE {
            return Err(CacheAdmissionFailure::new(
                3,
                1,
                signature as u64,
                TARGET_SIGNATURE as u64,
            ));
        }
        if width != TARGET_PHYSICAL_BITS {
            return Err(CacheAdmissionFailure::new(
                4,
                0x80000008,
                width as u64,
                TARGET_PHYSICAL_BITS as u64,
            ));
        }
        let topology = topology?;
        let capability = read(MTRR_CAP);
        if capability != 0x508 {
            return Err(CacheAdmissionFailure::new(5, MTRR_CAP, capability, 0x508));
        }
        let sys_cfg = read(SYS_CFG);
        if sys_cfg & !SYS_CFG_DEFINED != 0 || sys_cfg & SYS_CFG_ENCRYPTION != 0 {
            return Err(CacheAdmissionFailure::new(
                6,
                SYS_CFG,
                sys_cfg,
                SYS_CFG_DEFINED & !SYS_CFG_ENCRYPTION,
            ));
        }
        let mut result = Self { capability, sys_cfg, topology, ..Self::EMPTY };
        result.default = read(MTRR_DEF_TYPE);
        result.top_mem = read(TOP_MEM);
        result.top_mem2 = read(TOM2);
        for (index, value) in result.iorr.iter_mut().enumerate() {
            *value = read(IORR_BASE0 + index as u32);
        }
        result.hwcr = read(HWCR);
        // Preserve the current bounded profile's HWCR3=0 restriction while
        // diagnosing admission. HWCR4 retains INVD-to-WBINVD conversion.
        if result.hwcr & 0x18 != 0x10 {
            return Err(CacheAdmissionFailure::new(7, HWCR, result.hwcr, 0x10));
        }
        result.mmconfig = read(MMIO_CFG_BASE_ADDR);
        result.pat = read(PAT);
        for (index, pair) in result.variable.iter_mut().enumerate() {
            let base = MTRR_VAR_BASE0 + index as u32 * 2;
            *pair = (read(base), read(base + 1));
        }
        let visible = sys_cfg | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        if visible != sys_cfg {
            write(SYS_CFG, visible);
        }
        let observed_visible = read(SYS_CFG);
        let changed = observed_visible == visible;
        if changed {
            for (value, index) in result.fixed.iter_mut().zip(MTRR_FIXED) {
                *value = read(index);
            }
        }
        if visible != sys_cfg {
            write(SYS_CFG, sys_cfg);
        }
        let observed_restored = read(SYS_CFG);
        if !changed {
            return Err(CacheAdmissionFailure::new(8, SYS_CFG, observed_visible, visible));
        }
        if observed_restored != sys_cfg {
            return Err(CacheAdmissionFailure::new(9, SYS_CFG, observed_restored, sys_cfg));
        }
        let final_default = read(MTRR_DEF_TYPE);
        if final_default != result.default {
            return Err(CacheAdmissionFailure::new(
                10,
                MTRR_DEF_TYPE,
                final_default,
                result.default,
            ));
        }
        result.validate_active_fixed_types()?;
        Ok(result)
    }

    /// APM2 rev3.44 7.9.1/Table7-13, PDF295-296: valid type bits alone do
    /// not establish a supported active extended tuple. Dormant fixed type
    /// fields are not interpreted as though E/FE and extended attrs were on.
    pub(crate) fn validate_active_fixed_types(&self) -> Result<(), CacheAdmissionFailure> {
        if self.default & (DEF_TYPE_E | DEF_TYPE_FE) != DEF_TYPE_E | DEF_TYPE_FE {
            return Ok(());
        }
        // The bounded native bootstrap profile requires enabled DRAM attrs;
        // SYS_CFG18=0 would instead route low ranges as attr00. Do not silently
        // interpret the hidden raw fields as active in that configuration.
        if self.sys_cfg & SYS_CFG_MTRR_FIX_DRAM_EN == 0 {
            return Err(CacheAdmissionFailure::new(
                21,
                SYS_CFG,
                self.sys_cfg,
                SYS_CFG_MTRR_FIX_DRAM_EN,
            ));
        }
        for (&index, &value) in MTRR_FIXED.iter().zip(&self.fixed) {
            if value.to_le_bytes().iter().any(|b| {
                !matches!(
                    b,
                    0x00 | 0x01
                        | 0x04
                        | 0x05
                        | 0x08
                        | 0x09
                        | 0x10
                        | 0x15
                        | 0x18
                        | 0x19
                        | 0x1c
                        | 0x1e
                )
            }) {
                return Err(CacheAdmissionFailure::new(20, index, value, 0));
            }
        }
        Ok(())
    }

    /// Compare raw physical state from two observations on the SAME thread.
    /// Revalidation must not hide an intervening physical control change.
    pub fn same_physical_state(&self, other: &Self) -> bool {
        self == other
    }

    /// Exact effective-bank restoration for the bounded replay owner. Windows
    /// stores only valid variable pairs and may replay zero into disabled slots.
    /// A disabled pair contributes no match, irrespective of its base/mask bits.
    /// All enabled pairs, fixed attributes, default and routing stay identical.
    /// This intentionally does not accept arbitrary equivalent range rewrites.
    pub fn restored_mtrrs(
        &self,
        default: u64,
        variable: &[(u64, u64); 8],
        fixed: &[u64; 11],
        sys_cfg: u64,
    ) -> bool {
        self.default == default
            && self.fixed == *fixed
            && (self.sys_cfg ^ sys_cfg) & !SYS_CFG_MTRR_FIX_DRAM_MOD_EN == 0
            && self.variable.iter().zip(variable).all(|(&(base, mask), &(new_base, new_mask))| {
                if mask & VARIABLE_VALID == 0 && new_mask & VARIABLE_VALID == 0 {
                    true
                } else {
                    base == new_base && mask == new_mask
                }
            })
    }

    fn domain(&self) -> Option<(u32, u32, u32)> {
        let shift = (self.topology[3] >> 12) & 15;
        let threads = ((self.topology[1] >> 8) & 255) + 1;
        if shift == 0
            || self.topology[1] & 0xffff_0000 != 0
            || threads > 2
            || (self.topology[3] & 0xfff) + 1 > 1 << shift
        {
            return None;
        }
        Some((self.topology[0] >> shift, self.topology[1] & 255, threads))
    }

    fn bank_difference(&self, peer: &Self, ignore_disabled: bool) -> Option<CacheAdmissionFailure> {
        let fail =
            |index, observed, expected| CacheAdmissionFailure::new(12, index, observed, expected);
        let visible = SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        for (index, expected, observed) in [
            (MTRR_DEF_TYPE, self.default, peer.default),
            (SYS_CFG, self.sys_cfg & !visible, peer.sys_cfg & !visible),
            (TOP_MEM, self.top_mem, peer.top_mem),
            (TOM2, self.top_mem2, peer.top_mem2),
            (MMIO_CFG_BASE_ADDR, self.mmconfig, peer.mmconfig),
        ] {
            if observed != expected {
                return Some(fail(index, observed, expected));
            }
        }
        if !ignore_disabled && self.capability != peer.capability {
            return Some(fail(MTRR_CAP, peer.capability, self.capability));
        }
        for (i, (&expected, &observed)) in self.iorr.iter().zip(&peer.iorr).enumerate() {
            if observed != expected {
                return Some(fail(IORR_BASE0 + i as u32, observed, expected));
            }
        }
        for (i, (&(base, mask), &(other_base, other_mask))) in
            self.variable.iter().zip(&peer.variable).enumerate()
        {
            if ignore_disabled && mask & VARIABLE_VALID == 0 && other_mask & VARIABLE_VALID == 0 {
                continue;
            }
            if base != other_base {
                return Some(fail(MTRR_VAR_BASE0 + 2 * i as u32, other_base, base));
            }
            if mask != other_mask {
                return Some(fail(MTRR_VAR_BASE0 + 1 + 2 * i as u32, other_mask, mask));
            }
        }
        for ((&expected, &observed), index) in self.fixed.iter().zip(&peer.fixed).zip(MTRR_FIXED) {
            if expected != observed {
                return Some(fail(index, observed, expected));
            }
        }
        None
    }
}

/// Immutable pre-guest evidence shared by every private root. Owned CPUs sample
/// serially after successful EBS return, before publication/first guest entry.
#[repr(C, align(4096))]
pub struct CacheCapture {
    count: u32,
    valid: u32,
    observations: [CacheObservation; MAX_CACHE_CPUS],
}

impl CacheCapture {
    pub const fn empty() -> Self {
        Self { count: 0, valid: 0, observations: [CacheObservation::EMPTY; MAX_CACHE_CPUS] }
    }
    pub fn initialize(&mut self, count: usize) -> bool {
        if self.count != 0 || !(1..=MAX_CACHE_CPUS).contains(&count) {
            return false;
        }
        self.count = count as u32;
        true
    }
    pub fn seed(&mut self, slot: usize, observation: CacheObservation) -> bool {
        if slot >= self.count as usize || self.valid & (1 << slot) != 0 {
            return false;
        }
        self.observations[slot] = observation;
        self.valid |= 1 << slot;
        true
    }
    pub fn complete(&self, count: usize) -> bool {
        (1..=MAX_CACHE_CPUS).contains(&count)
            && self.count as usize == count
            && self.valid == u32::MAX >> (MAX_CACHE_CPUS - count)
    }
    pub(crate) fn observation(&self, slot: usize, count: usize) -> Option<&CacheObservation> {
        if !self.complete(count) || slot >= count {
            return None;
        }
        Some(&self.observations[slot])
    }
    pub const fn enabled(&self) -> bool {
        self.count != 0
    }
    pub fn capture_state(&self) -> (u32, u32) {
        (self.count, self.valid)
    }

    /// The audited Windows initialization saves the BSP bank and replays it on
    /// every active processor. Admit that replay before any guest starts,
    /// allowing only irrelevant contents in disabled variable slots to differ.
    pub fn agrees_with_bsp(&self, bsp: usize, count: usize) -> bool {
        self.agrees_with_bsp_detailed(bsp, count).is_ok()
    }
    pub fn agrees_with_bsp_detailed(
        &self,
        bsp: usize,
        count: usize,
    ) -> Result<(), (usize, CacheAdmissionFailure)> {
        let baseline = self.observation(bsp, count).ok_or((
            bsp,
            CacheAdmissionFailure::new(11, 0, self.valid as u64, self.count as u64),
        ))?;
        for slot in 0..count {
            let peer = self.observation(slot, count).ok_or((
                slot,
                CacheAdmissionFailure::new(11, 0, self.valid as u64, self.count as u64),
            ))?;
            if let Some(f) = baseline.bank_difference(peer, true) {
                return Err((slot, f));
            }
        }
        Ok(())
    }

    /// PPR57896 Fn8000001E: CoreId is per socket and threads/core is EBX15:8+1.
    /// Fn80000008 ECX15:12 supplies the nonzero initial-APIC package width.
    /// Dense firmware slots are never interpreted as hardware core numbers.
    pub fn domain_mask(&self, slot: usize, ids: &[u32]) -> Option<u32> {
        self.domain_mask_detailed(slot, ids).ok()
    }
    pub fn domain_mask_detailed(
        &self,
        slot: usize,
        ids: &[u32],
    ) -> Result<u32, (usize, CacheAdmissionFailure)> {
        let missing =
            |s| (s, CacheAdmissionFailure::new(11, 0, self.valid as u64, self.count as u64));
        let current = self.observation(slot, ids.len()).ok_or_else(|| missing(slot))?;
        let (package, core, threads) = current.domain().ok_or((
            slot,
            CacheAdmissionFailure::new(
                13,
                0x80000008,
                (current.topology[3] as u64) << 32 | current.topology[1] as u64,
                2,
            ),
        ))?;
        let mut members = 0u32;
        for (index, &id) in ids.iter().enumerate() {
            let peer = self.observation(index, ids.len()).ok_or_else(|| missing(index))?;
            if peer.topology[0] != id {
                return Err((
                    index,
                    CacheAdmissionFailure::new(14, 0x8000001e, peer.topology[0] as u64, id as u64),
                ));
            }
            if ids[..index].contains(&id) {
                return Err((
                    index,
                    CacheAdmissionFailure::new(15, 0x8000001e, id as u64, index as u64),
                ));
            }
            if peer.topology[3] != current.topology[3] {
                return Err((
                    index,
                    CacheAdmissionFailure::new(
                        16,
                        0x80000008,
                        peer.topology[3] as u64,
                        current.topology[3] as u64,
                    ),
                ));
            }
            let (peer_package, peer_core, peer_threads) = peer.domain().ok_or((
                index,
                CacheAdmissionFailure::new(
                    13,
                    0x80000008,
                    (peer.topology[3] as u64) << 32 | peer.topology[1] as u64,
                    2,
                ),
            ))?;
            if (package, core) == (peer_package, peer_core) {
                if peer_threads != threads {
                    return Err((
                        index,
                        CacheAdmissionFailure::new(
                            17,
                            0x8000001e,
                            peer_threads as u64,
                            threads as u64,
                        ),
                    ));
                }
                if peer.topology[2] != current.topology[2] {
                    return Err((
                        index,
                        CacheAdmissionFailure::new(
                            18,
                            0x8000001e,
                            peer.topology[2] as u64,
                            current.topology[2] as u64,
                        ),
                    ));
                }
                if let Some(f) = current.bank_difference(peer, false) {
                    return Err((index, f));
                }
                members |= 1 << index;
            }
        }
        if members.count_ones() != threads {
            return Err((
                slot,
                CacheAdmissionFailure::new(19, 0x8000001e, members as u64, threads as u64),
            ));
        }
        Ok(members)
    }
}

#[derive(Clone, Copy)]
pub struct CacheCoreState {
    pub bank: CacheObservation,
    pub members: u32,
    pub entering: u32,
    pub leaving: u32,
    pub departed: u32,
    /// 0 idle, 1 collecting E0, 2 active, 3 collecting E1, 4 releasing.
    pub phase: u32,
    pub generation: u64,
}
impl CacheCoreState {
    const EMPTY: Self = Self {
        bank: CacheObservation::EMPTY,
        members: 0,
        entering: 0,
        leaving: 0,
        departed: 0,
        phase: 0,
        generation: 0,
    };

    #[cfg(feature = "resident-runtime-test")]
    pub(crate) fn fixture(bank: CacheObservation) -> Self {
        Self { bank, members: 3, ..Self::EMPTY }
    }

    pub fn read(&self, index: u32, visibility: bool) -> Option<u64> {
        let visible = SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        Some(match index {
            MTRR_CAP => self.bank.capability,
            MTRR_DEF_TYPE => self.bank.default,
            SYS_CFG => (self.bank.sys_cfg & !visible) | if visibility { visible } else { 0 },
            MTRR_VAR_BASE0..=VAR_LAST => {
                let pair = self.bank.variable[((index - MTRR_VAR_BASE0) / 2) as usize];
                if index & 1 == 0 { pair.0 } else { pair.1 }
            }
            IORR_BASE0..=IORR_LAST => self.bank.iorr[(index - IORR_BASE0) as usize],
            TOP_MEM => self.bank.top_mem,
            TOM2 => self.bank.top_mem2,
            MMIO_CFG_BASE_ADDR => self.bank.mmconfig,
            _ => {
                let slot = MTRR_FIXED.iter().position(|&v| v == index)?;
                self.bank.fixed[slot] & if visibility { u64::MAX } else { 0x0707_0707_0707_0707 }
            }
        })
    }

    pub fn enter(&mut self, bit: u32, requested: u64) -> Result<u64, CacheWriteError> {
        if !matches!(self.phase, 0 | 1)
            || bit.count_ones() != 1
            || self.entering & bit != 0
            || self.members & bit == 0
        {
            return Err(CacheWriteError::Unsupported);
        }
        self.phase = 1;
        self.entering |= bit;
        if self.entering == self.members {
            self.bank.default = requested;
            self.phase = 2;
        }
        Ok(self.generation)
    }
    pub fn leave(
        &mut self,
        bit: u32,
        requested: u64,
        baseline: &CacheObservation,
    ) -> Result<u64, CacheWriteError> {
        if !matches!(self.phase, 2 | 3)
            || bit.count_ones() != 1
            || self.members & bit == 0
            || self.leaving & bit != 0
            || requested != baseline.default
        {
            return Err(CacheWriteError::Unsupported);
        }
        let complete = self.leaving | bit == self.members;
        if complete
            && !baseline.restored_mtrrs(
                requested,
                &self.bank.variable,
                &self.bank.fixed,
                self.bank.sys_cfg,
            )
        {
            return Err(CacheWriteError::Unsupported);
        }
        self.phase = 3;
        self.leaving |= bit;
        if complete {
            self.bank.default = requested;
            self.phase = 4;
        }
        Ok(self.generation)
    }
    /// Called only after this CPU committed E1 and restored its root/guard.
    pub fn depart(&mut self, bit: u32, generation: u64) -> Result<(), CacheWriteError> {
        if self.phase != 4
            || self.generation != generation
            || bit.count_ones() != 1
            || self.members & bit == 0
            || self.departed & bit != 0
        {
            return Err(CacheWriteError::Unsupported);
        }
        self.departed |= bit;
        if self.departed == self.members {
            self.entering = 0;
            self.leaving = 0;
            self.departed = 0;
            self.phase = 0;
            self.generation = self.generation.wrapping_add(1);
        }
        Ok(())
    }

    /// Ordinary logical writes during CD-constrained replay. Boundary E0/E1
    /// transitions are owned separately by the paired continuation barrier.
    pub fn write(
        &mut self,
        index: u32,
        value: u64,
        visibility: &mut bool,
    ) -> Result<(), CacheWriteError> {
        use CacheWriteError::{Fault, Unsupported};
        let current = self.read(index, *visibility).ok_or(Unsupported)?;
        if index == MTRR_CAP {
            return Err(Fault);
        }
        if index == SYS_CFG {
            let visible = SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
            if value & !SYS_CFG_DEFINED != 0 {
                return Err(Fault);
            }
            let allowed =
                visible | if matches!(self.phase, 2 | 3) { SYS_CFG_MTRR_FIX_DRAM_EN } else { 0 };
            if (value ^ current) & !allowed != 0 {
                return Err(Unsupported);
            }
            self.bank.sys_cfg = value & !visible;
            *visibility = value & visible != 0;
            return Ok(());
        }
        if let Some(slot) = MTRR_FIXED.iter().position(|&v| v == index) {
            for byte in value.to_le_bytes() {
                if byte & !0x1f != 0 || !valid_type(byte & 7) || !*visibility && byte & 0x18 != 0 {
                    return Err(Fault);
                }
            }
            let merged = if *visibility {
                value
            } else {
                value | self.bank.fixed[slot] & 0x1818_1818_1818_1818
            };
            if !matches!(self.phase, 2 | 3) && merged != self.bank.fixed[slot] {
                return Err(Unsupported);
            }
            self.bank.fixed[slot] = merged;
            return Ok(());
        }
        if (MTRR_VAR_BASE0..=VAR_LAST).contains(&index) {
            let mask = if index & 1 == 0 { 0x0000_ffff_ffff_f007 } else { 0x0000_ffff_ffff_f800 };
            if value & !mask != 0 || index & 1 == 0 && !valid_type(value as u8) {
                return Err(Fault);
            }
            if !matches!(self.phase, 2 | 3) && value != current {
                return Err(Unsupported);
            }
            let pair = &mut self.bank.variable[((index - MTRR_VAR_BASE0) / 2) as usize];
            if index & 1 == 0 {
                pair.0 = value;
            } else {
                pair.1 = value;
            }
            return Ok(());
        }
        // Routing and MMIO controls retain physical values. HWCR belongs to
        // the CPU-local physical access owner, never this shared replay bank.
        if value == current { Ok(()) } else { Err(Unsupported) }
    }
}

/// Shared physical-core bank. Access is serialized only while software copies
/// or changes state; no caller may retain this guard while waiting for a peer.
pub(crate) type CacheCore = TryLock<CacheCoreState>;

#[repr(C, align(4096))]
pub struct CacheOwner {
    pub cores: [CacheCore; MAX_CACHE_CPUS],
}

const _: () = assert!(core::mem::size_of::<CacheOwner>() == 3 * 4096);

impl CacheOwner {
    pub const fn empty() -> Self {
        Self { cores: [const { TryLock::new(CacheCoreState::EMPTY) }; MAX_CACHE_CPUS] }
    }
    /// Sole post-EBS BSP writer; all captures complete and no guest has entered.
    pub fn initialize(&mut self, capture: &CacheCapture, ids: &[u32]) -> bool {
        self.initialize_detailed(capture, ids).is_ok()
    }
    pub fn initialize_detailed(
        &mut self,
        capture: &CacheCapture,
        ids: &[u32],
    ) -> Result<(), (usize, CacheAdmissionFailure)> {
        for slot in 0..ids.len() {
            let mask = capture.domain_mask_detailed(slot, ids)?;
            if mask.trailing_zeros() as usize != slot {
                continue;
            }
            let bank = capture.observation(slot, ids.len()).ok_or((
                slot,
                CacheAdmissionFailure::new(11, 0, capture.valid as u64, capture.count as u64),
            ))?;
            *self.cores[slot].get_mut() =
                CacheCoreState { bank: *bank, members: mask, ..CacheCoreState::EMPTY };
        }
        Ok(())
    }
}

/// First failed native cache admission predicate, retaining the existing sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheAdmissionFailure {
    pub predicate: u32,
    pub index: u32,
    pub observed: u64,
    pub expected: u64,
}
impl CacheAdmissionFailure {
    pub const fn new(predicate: u32, index: u32, observed: u64, expected: u64) -> Self {
        Self { predicate, index, observed, expected }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheWriteError {
    Fault,
    Unsupported,
}

/// Enumerated AMD topology observations; missing leaves are not invented.
pub fn native_topology() -> Option<[u32; 4]> {
    native_topology_detailed().ok()
}
pub fn native_topology_detailed() -> Result<[u32; 4], CacheAdmissionFailure> {
    use core::arch::x86_64::__cpuid_count;
    let maximum = __cpuid_count(0x8000_0000, 0).eax;
    if maximum < 0x8000_001e {
        return Err(CacheAdmissionFailure::new(1, 0x80000000, maximum as u64, 0x8000001e));
    }
    let features = __cpuid_count(0x8000_0001, 0).ecx;
    if features & (1 << 22) == 0 {
        return Err(CacheAdmissionFailure::new(2, 0x80000001, features as u64, 1 << 22));
    }
    let leaf = __cpuid_count(0x8000_001e, 0);
    Ok([leaf.eax, leaf.ebx, leaf.ecx, __cpuid_count(0x8000_0008, 0).ecx])
}

/// Complete register ownership inventory. Guest PAT keeps its VMCB owner.
pub fn owned_msr(index: u32) -> bool {
    owned_msrs().any(|owned| owned == index)
}
pub(crate) fn owned_msrs() -> impl Iterator<Item = u32> {
    (MTRR_VAR_BASE0..=VAR_LAST)
        .chain(MTRR_FIXED)
        .chain([MTRR_CAP, MTRR_DEF_TYPE, SYS_CFG, HWCR])
        .chain(IORR_BASE0..=IORR_LAST)
        .chain([TOP_MEM, TOM2, MMIO_CFG_BASE_ADDR])
}
