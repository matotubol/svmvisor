//! Retained native cache-control observations for the Family1Ah Model44h host.
//! PPR57896 rev3.00 pp123,126-130,173,202-206,210; APM2 rev3.44 7.7/7.9.
//! These records do not grant permission to write physical cache controls.

use crate::{
    arch::x86_64::msr::{
        HWCR, HWCR_CPUID_FLT_EN, HWCR_IRPERF_EN, IORR_BASE0, MMIO_CFG_BASE_ADDR, MTRR_CAP,
        MTRR_DEF_TYPE, MTRR_FIXED, MTRR_VAR_BASE0, PAT, SYS_CFG, SYS_CFG_DEFINED,
        SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN, SYS_CFG_MTRR_FIX_DRAM_MOD_EN,
        TARGET_PHYSICAL_BITS, TARGET_SIGNATURE, TOM2, TOP_MEM,
    },
    memory::mtrrs::{DEF_TYPE_E, DEF_TYPE_FE, VARIABLE_VALID, valid_type},
    sync::TryLock,
};

pub const MAX_CACHE_CPUS: usize = 32;
/// Last owned MtrrVarMask (eight pairs) and IORR_MASK (two pairs).
const VAR_LAST: u32 = MTRR_VAR_BASE0 + 15;
const IORR_LAST: u32 = IORR_BASE0 + 3;

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
    baseline: u64, requested: Option<u64>, inst_ret_counter: bool,
    pmc_virtualization: bool, cpuid_fault_owned: bool, mut read: impl FnMut() -> u64,
    mut write: impl FnMut(u64),
) -> Result<u64, HwcrError> {
    if pmc_virtualization { return Err(HwcrError::PmcVirtualization); }
    let allowed = HWCR_IRPERF_EN | if cpuid_fault_owned { HWCR_CPUID_FLT_EN } else { 0 };
    let current = read();
    if (current ^ baseline) & !allowed != 0 {
        return Err(HwcrError::PhysicalDrift { observed: current, baseline });
    }
    let Some(requested) = requested else { return Ok(current); };
    let changed = requested ^ current;
    if changed & !allowed != 0 || changed & HWCR_IRPERF_EN != 0 && !inst_ret_counter {
        return Err(HwcrError::UnsupportedChange { current, requested });
    }
    if changed == 0 { return Ok(current); }
    let expected = (current & !allowed) | (requested & allowed);
    write(expected);
    let observed = read();
    if observed != expected { return Err(HwcrError::Readback { observed, expected }); }
    Ok(observed)
}

/// Post-EBS collection gate. Firmware may synchronize MTRRs in its final
/// callbacks, so no guest may consume the bank until every owned CPU sampled
/// it and the BSP admitted the complete capture. This gate never grants MSR
/// write permission or replaces the bank/domain checks.
pub struct CacheSurvey {
    sampled: core::sync::atomic::AtomicU32,
    admitted: core::sync::atomic::AtomicBool,
    failed: core::sync::atomic::AtomicBool,
}
impl Default for CacheSurvey {
    fn default() -> Self { Self::new() }
}
impl CacheSurvey {
    pub const fn new() -> Self { Self {
        sampled: core::sync::atomic::AtomicU32::new(0),
        admitted: core::sync::atomic::AtomicBool::new(false),
        failed: core::sync::atomic::AtomicBool::new(false),
    } }
    pub fn sampled(&self, slot: usize) -> bool {
        use core::sync::atomic::Ordering;
        slot < MAX_CACHE_CPUS && self.sampled.load(Ordering::Acquire) & (1 << slot) != 0
    }
    /// Sole serial capture writer publishes after its complete bank write.
    pub fn complete_sample(&self, slot: usize) -> bool {
        use core::sync::atomic::Ordering;
        slot < MAX_CACHE_CPUS && !self.failed.load(Ordering::Acquire)
            && self.sampled.fetch_or(1 << slot, Ordering::AcqRel) & (1 << slot) == 0
    }
    /// BSP only, after full bank/topology/owner admission succeeded.
    pub fn admit(&self, count: usize) -> bool {
        use core::sync::atomic::Ordering;
        if !(1..=MAX_CACHE_CPUS).contains(&count) || self.failed.load(Ordering::Acquire)
            || self.sampled.load(Ordering::Acquire) != u32::MAX >> (MAX_CACHE_CPUS-count) {
            return false;
        }
        self.admitted.store(true, Ordering::Release);
        true
    }
    pub fn abort(&self) { self.failed.store(true, core::sync::atomic::Ordering::Release); }
    pub fn failed(&self) -> bool { self.failed.load(core::sync::atomic::Ordering::Acquire) }
    pub fn admitted(&self) -> bool {
        !self.failed() && self.admitted.load(core::sync::atomic::Ordering::Acquire)
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
    pub const fn new(predicate:u32,index:u32,observed:u64,expected:u64)->Self {
        Self { predicate,index,observed,expected }
    }
}

/// Enumerated AMD topology observations; missing leaves are not invented.
pub fn native_topology() -> Option<[u32; 4]> {
    native_topology_detailed().ok()
}
pub fn native_topology_detailed() -> Result<[u32;4],CacheAdmissionFailure> {
    use core::arch::x86_64::__cpuid_count;
    let maximum=__cpuid_count(0x8000_0000,0).eax;
    if maximum<0x8000_001e {return Err(CacheAdmissionFailure::new(1,0x80000000,maximum as u64,0x8000001e));}
    let features=__cpuid_count(0x8000_0001,0).ecx;
    if features&(1<<22)==0 {return Err(CacheAdmissionFailure::new(2,0x80000001,features as u64,1<<22));}
    let leaf = __cpuid_count(0x8000_001e, 0);
    Ok([leaf.eax, leaf.ebx, leaf.ecx, __cpuid_count(0x8000_0008, 0).ecx])
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
        if self.count != 0 || !(1..=MAX_CACHE_CPUS).contains(&count) { return false; }
        self.count = count as u32;
        true
    }
    pub fn seed(&mut self, slot: usize, observation: CacheObservation) -> bool {
        if slot >= self.count as usize || self.valid & (1 << slot) != 0 { return false; }
        self.observations[slot] = observation;
        self.valid |= 1 << slot;
        true
    }
    pub fn complete(&self, count: usize) -> bool {
        (1..=MAX_CACHE_CPUS).contains(&count) && self.count as usize == count
            && self.valid == u32::MAX >> (MAX_CACHE_CPUS - count)
    }
    pub fn observation(&self, slot: usize, count: usize) -> Option<&CacheObservation> {
        if !self.complete(count) || slot >= count { return None; }
        Some(&self.observations[slot])
    }
    pub const fn enabled(&self) -> bool { self.count != 0 }
    pub fn capture_state(&self) -> (u32,u32) { (self.count,self.valid) }

    /// The audited Windows initialization saves the BSP bank and replays it on
    /// every active processor. Admit that replay before any guest starts,
    /// allowing only irrelevant contents in disabled variable slots to differ.
    pub fn agrees_with_bsp(&self, bsp: usize, count: usize) -> bool {
        self.agrees_with_bsp_detailed(bsp,count).is_ok()
    }
    pub fn agrees_with_bsp_detailed(&self,bsp:usize,count:usize)->Result<(),(usize,CacheAdmissionFailure)> {
        let baseline=self.observation(bsp,count).ok_or((bsp,CacheAdmissionFailure::new(11,0,self.valid as u64,self.count as u64)))?;
        for slot in 0..count {
            let peer=self.observation(slot,count).ok_or((slot,CacheAdmissionFailure::new(11,0,self.valid as u64,self.count as u64)))?;
            if let Some(f)=baseline.bank_difference(peer,true) {return Err((slot,f));}
        }
        Ok(())
    }

    /// PPR57896 Fn8000001E: CoreId is per socket and threads/core is EBX15:8+1.
    /// Fn80000008 ECX15:12 supplies the nonzero initial-APIC package width.
    /// Dense firmware slots are never interpreted as hardware core numbers.
    pub fn domain_mask(&self, slot: usize, ids: &[u32]) -> Option<u32> {
        self.domain_mask_detailed(slot,ids).ok()
    }
    pub fn domain_mask_detailed(&self,slot:usize,ids:&[u32])->Result<u32,(usize,CacheAdmissionFailure)> {
        let missing=|s| (s,CacheAdmissionFailure::new(11,0,self.valid as u64,self.count as u64));
        let current = self.observation(slot, ids.len()).ok_or_else(||missing(slot))?;
        let (package, core, threads) = current.domain().ok_or((slot,CacheAdmissionFailure::new(13,0x80000008,
            (current.topology[3] as u64)<<32|current.topology[1] as u64,2)))?;
        let mut members = 0u32;
        for (index, &id) in ids.iter().enumerate() {
            let peer = self.observation(index, ids.len()).ok_or_else(||missing(index))?;
            if peer.topology[0] != id {return Err((index,CacheAdmissionFailure::new(14,0x8000001e,peer.topology[0] as u64,id as u64)));}
            if ids[..index].contains(&id) {return Err((index,CacheAdmissionFailure::new(15,0x8000001e,id as u64,index as u64)));}
            if peer.topology[3] != current.topology[3] {return Err((index,CacheAdmissionFailure::new(16,0x80000008,peer.topology[3] as u64,current.topology[3] as u64)));}
            let (peer_package, peer_core, peer_threads) = peer.domain().ok_or((index,CacheAdmissionFailure::new(13,0x80000008,
                (peer.topology[3] as u64)<<32|peer.topology[1] as u64,2)))?;
            if (package, core) == (peer_package, peer_core) {
                if peer_threads!=threads {return Err((index,CacheAdmissionFailure::new(17,0x8000001e,peer_threads as u64,threads as u64)));}
                if peer.topology[2]!=current.topology[2] {return Err((index,CacheAdmissionFailure::new(18,0x8000001e,peer.topology[2] as u64,current.topology[2] as u64)));}
                if let Some(f)=current.bank_difference(peer,false) {return Err((index,f));}
                members |= 1 << index;
            }
        }
        if members.count_ones()!=threads {return Err((slot,CacheAdmissionFailure::new(19,0x8000001e,members as u64,threads as u64)));}
        Ok(members)
    }
}

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

impl CacheObservation {
    pub const EMPTY: Self = Self {
        capability: 0, default: 0, sys_cfg: 0, top_mem: 0, top_mem2: 0,
        iorr: [0; 4], hwcr: 0, mmconfig: 0, pat: 0,
        variable: [(0, 0); 8], fixed: [0; 11], topology: [0; 4],
    };

    /// Bounded target-specific sampling, used by the actual MP observer and
    /// local pre-entry revalidation. Closures own permitted physical MSR
    /// access on the current CPU. SYS_CFG19 is restored before every return
    /// after its temporary change; no address-routing field is modified.
    pub fn capture(signature: u32, width: u8, topology: Option<[u32; 4]>,
        read: impl FnMut(u32) -> u64, write: impl FnMut(u32, u64)) -> Option<Self>
    {
        Self::capture_detailed(signature,width,topology.ok_or(CacheAdmissionFailure::new(1,0x8000001e,0,1)),read,write).ok()
    }
    pub fn capture_detailed(signature:u32,width:u8,topology:Result<[u32;4],CacheAdmissionFailure>,
        mut read:impl FnMut(u32)->u64,mut write:impl FnMut(u32,u64))->Result<Self,CacheAdmissionFailure>
    {
        if signature!=TARGET_SIGNATURE {return Err(CacheAdmissionFailure::new(3,1,signature as u64,TARGET_SIGNATURE as u64));}
        if width!=TARGET_PHYSICAL_BITS {return Err(CacheAdmissionFailure::new(4,0x80000008,width as u64,TARGET_PHYSICAL_BITS as u64));}
        let topology=topology?;
        let capability = read(MTRR_CAP);
        if capability != 0x508 { return Err(CacheAdmissionFailure::new(5,MTRR_CAP,capability,0x508)); }
        let sys_cfg = read(SYS_CFG);
        if sys_cfg & !SYS_CFG_DEFINED != 0 || sys_cfg & SYS_CFG_ENCRYPTION != 0 {
            return Err(CacheAdmissionFailure::new(6,SYS_CFG,sys_cfg,SYS_CFG_DEFINED & !SYS_CFG_ENCRYPTION)); }
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
        if result.hwcr & 0x18 != 0x10 { return Err(CacheAdmissionFailure::new(7,HWCR,result.hwcr,0x10)); }
        result.mmconfig = read(MMIO_CFG_BASE_ADDR);
        result.pat = read(PAT);
        for (index, pair) in result.variable.iter_mut().enumerate() {
            let base = MTRR_VAR_BASE0 + index as u32 * 2;
            *pair = (read(base), read(base + 1));
        }
        let visible = sys_cfg | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        if visible != sys_cfg { write(SYS_CFG, visible); }
        let observed_visible=read(SYS_CFG);
        let changed = observed_visible == visible;
        if changed {
            for (value, index) in result.fixed.iter_mut().zip(MTRR_FIXED) { *value = read(index); }
        }
        if visible != sys_cfg { write(SYS_CFG, sys_cfg); }
        let observed_restored=read(SYS_CFG);
        if !changed {return Err(CacheAdmissionFailure::new(8,SYS_CFG,observed_visible,visible));}
        if observed_restored!=sys_cfg {return Err(CacheAdmissionFailure::new(9,SYS_CFG,observed_restored,sys_cfg));}
        let final_default=read(MTRR_DEF_TYPE);
        if final_default!=result.default {return Err(CacheAdmissionFailure::new(10,MTRR_DEF_TYPE,final_default,result.default));}
        result.validate_active_fixed_types()?;
        Ok(result)
    }

    /// APM2 rev3.44 7.9.1/Table7-13, PDF295-296: valid type bits alone do
    /// not establish a supported active extended tuple. Dormant fixed type
    /// fields are not interpreted as though E/FE and extended attrs were on.
    pub fn validate_active_fixed_types(&self) -> Result<(), CacheAdmissionFailure> {
        if self.default & (DEF_TYPE_E | DEF_TYPE_FE) != DEF_TYPE_E | DEF_TYPE_FE { return Ok(()); }
        // The bounded native bootstrap profile requires enabled DRAM attrs;
        // SYS_CFG18=0 would instead route low ranges as attr00. Do not silently
        // interpret the hidden raw fields as active in that configuration.
        if self.sys_cfg & SYS_CFG_MTRR_FIX_DRAM_EN == 0 {
            return Err(CacheAdmissionFailure::new(21,SYS_CFG,self.sys_cfg,SYS_CFG_MTRR_FIX_DRAM_EN));
        }
        for (&index, &value) in MTRR_FIXED.iter().zip(&self.fixed) {
            if value.to_le_bytes().iter().any(|b|
                !matches!(b, 0x00|0x01|0x04|0x05|0x08|0x09|0x10|0x15|0x18|0x19|0x1c|0x1e)) {
                return Err(CacheAdmissionFailure::new(20,index,value,0));
            }
        }
        Ok(())
    }

    /// Compare raw physical state from two observations on the SAME thread.
    /// Revalidation must not hide an intervening physical control change.
    pub fn same_physical_state(&self, other: &Self) -> bool { self == other }

    fn domain(&self) -> Option<(u32, u32, u32)> {
        let shift = (self.topology[3] >> 12) & 15;
        let threads = ((self.topology[1] >> 8) & 255) + 1;
        if shift == 0 || self.topology[1] & 0xffff_0000 != 0 || threads > 2
            || (self.topology[3] & 0xfff) + 1 > 1 << shift { return None; }
        Some((self.topology[0] >> shift, self.topology[1] & 255, threads))
    }

    fn bank_difference(&self,peer:&Self,ignore_disabled:bool)->Option<CacheAdmissionFailure> {
        let fail=|index,observed,expected|CacheAdmissionFailure::new(12,index,observed,expected);
        let visible = SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        for (index,expected,observed) in [(MTRR_DEF_TYPE,self.default,peer.default),
            (SYS_CFG,self.sys_cfg&!visible,peer.sys_cfg&!visible),
            (TOP_MEM,self.top_mem,peer.top_mem),(TOM2,self.top_mem2,peer.top_mem2),
            (MMIO_CFG_BASE_ADDR,self.mmconfig,peer.mmconfig)] {
            if observed!=expected {return Some(fail(index,observed,expected));}
        }
        if !ignore_disabled && self.capability!=peer.capability {return Some(fail(MTRR_CAP,peer.capability,self.capability));}
        for (i,(&expected,&observed)) in self.iorr.iter().zip(&peer.iorr).enumerate() {
            if observed!=expected {return Some(fail(IORR_BASE0+i as u32,observed,expected));}
        }
        for (i,(&(base,mask),&(other_base,other_mask))) in self.variable.iter().zip(&peer.variable).enumerate() {
            if ignore_disabled && mask&VARIABLE_VALID==0 && other_mask&VARIABLE_VALID==0 {continue;}
            if base!=other_base {return Some(fail(MTRR_VAR_BASE0+2*i as u32,other_base,base));}
            if mask!=other_mask {return Some(fail(MTRR_VAR_BASE0+1+2*i as u32,other_mask,mask));}
        }
        for ((&expected,&observed),index) in self.fixed.iter().zip(&peer.fixed).zip(MTRR_FIXED) {
            if expected!=observed {return Some(fail(index,observed,expected));}
        }
        None
    }

    /// Exact effective-bank restoration for the bounded replay owner. Windows
    /// stores only valid variable pairs and may replay zero into disabled slots.
    /// A disabled pair contributes no match, irrespective of its base/mask bits.
    /// All enabled pairs, fixed attributes, default and routing stay identical.
    /// This intentionally does not accept arbitrary equivalent range rewrites.
    pub fn restored_mtrrs(&self, default: u64, variable: &[(u64, u64); 8],
        fixed: &[u64; 11], sys_cfg: u64) -> bool
    {
        self.default == default && self.fixed == *fixed
            && (self.sys_cfg ^ sys_cfg) & !SYS_CFG_MTRR_FIX_DRAM_MOD_EN == 0
            && self.variable.iter().zip(variable).all(|(&(base, mask), &(new_base, new_mask))| {
                if mask & VARIABLE_VALID == 0 && new_mask & VARIABLE_VALID == 0 { true }
                else { base == new_base && mask == new_mask }
            })
    }
}

const _: () = {
    assert!(core::mem::size_of::<CacheObservation>() == 328);
    assert!(core::mem::align_of::<CacheObservation>() == 8);
    assert!(core::mem::size_of::<CacheCapture>() == 3 * 4096);
};

/// Complete register ownership inventory. Guest PAT keeps its VMCB owner.
pub fn owned_msr(index: u32) -> bool {
    owned_msrs().any(|owned| owned == index)
}
pub fn owned_msrs() -> impl Iterator<Item = u32> {
    (MTRR_VAR_BASE0..=VAR_LAST).chain(MTRR_FIXED).chain([MTRR_CAP, MTRR_DEF_TYPE, SYS_CFG, HWCR])
        .chain(IORR_BASE0..=IORR_LAST).chain([TOP_MEM, TOM2, MMIO_CFG_BASE_ADDR])
}

/// Shared physical-core bank. Access is serialized only while software copies
/// or changes state; no caller may retain this guard while waiting for a peer.
pub type CacheCore = TryLock<CacheCoreState>;

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
    const EMPTY: Self = Self { bank: CacheObservation::EMPTY, members: 0,
        entering: 0, leaving: 0, departed: 0, phase: 0, generation: 0 };

    #[cfg(feature = "resident-runtime-test")]
    pub fn fixture(bank: CacheObservation) -> Self { Self { bank, members: 3, ..Self::EMPTY } }

    pub fn enter(&mut self, bit: u32, requested: u64) -> Result<u64, CacheWriteError> {
        if !matches!(self.phase, 0 | 1) || bit.count_ones() != 1
            || self.entering & bit != 0 || self.members & bit == 0 {
            return Err(CacheWriteError::Unsupported);
        }
        self.phase = 1; self.entering |= bit;
        if self.entering == self.members { self.bank.default = requested; self.phase = 2; }
        Ok(self.generation)
    }
    pub fn leave(&mut self, bit: u32, requested: u64, baseline: &CacheObservation)
        -> Result<u64, CacheWriteError>
    {
        if !matches!(self.phase, 2 | 3) || bit.count_ones() != 1 || self.members & bit == 0
            || self.leaving & bit != 0 || requested != baseline.default {
            return Err(CacheWriteError::Unsupported);
        }
        let complete = self.leaving | bit == self.members;
        if complete && !baseline.restored_mtrrs(requested, &self.bank.variable,
            &self.bank.fixed, self.bank.sys_cfg) { return Err(CacheWriteError::Unsupported); }
        self.phase = 3; self.leaving |= bit;
        if complete { self.bank.default = requested; self.phase = 4; }
        Ok(self.generation)
    }
    /// Called only after this CPU committed E1 and restored its root/guard.
    pub fn depart(&mut self, bit: u32, generation: u64) -> Result<(), CacheWriteError> {
        if self.phase != 4 || self.generation != generation || bit.count_ones() != 1
            || self.members & bit == 0 || self.departed & bit != 0 {
            return Err(CacheWriteError::Unsupported);
        }
        self.departed |= bit;
        if self.departed == self.members {
            self.entering = 0; self.leaving = 0; self.departed = 0;
            self.phase = 0; self.generation = self.generation.wrapping_add(1);
        }
        Ok(())
    }

    pub fn read(&self, index: u32, visibility: bool) -> Option<u64> {
        let visible = SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        Some(match index {
            MTRR_CAP => self.bank.capability, MTRR_DEF_TYPE => self.bank.default,
            SYS_CFG => (self.bank.sys_cfg & !visible) | if visibility { visible } else { 0 },
            MTRR_VAR_BASE0..=VAR_LAST => { let pair = self.bank.variable[((index-MTRR_VAR_BASE0)/2) as usize];
                if index & 1 == 0 { pair.0 } else { pair.1 } },
            IORR_BASE0..=IORR_LAST => self.bank.iorr[(index-IORR_BASE0) as usize],
            TOP_MEM => self.bank.top_mem, TOM2 => self.bank.top_mem2,
            MMIO_CFG_BASE_ADDR => self.bank.mmconfig,
            _ => { let slot = MTRR_FIXED.iter().position(|&v| v == index)?;
                self.bank.fixed[slot] & if visibility { u64::MAX } else { 0x0707_0707_0707_0707 } },
        })
    }

    /// Ordinary logical writes during CD-constrained replay. Boundary E0/E1
    /// transitions are owned separately by the paired continuation barrier.
    pub fn write(&mut self, index: u32, value: u64, visibility: &mut bool) -> Result<(), CacheWriteError>
    {
        use CacheWriteError::{Fault, Unsupported};
        let current = self.read(index, *visibility).ok_or(Unsupported)?;
        if index == MTRR_CAP { return Err(Fault); }
        if index == SYS_CFG {
            let visible = SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
            if value & !SYS_CFG_DEFINED != 0 { return Err(Fault); }
            let allowed = visible | if matches!(self.phase, 2 | 3) { SYS_CFG_MTRR_FIX_DRAM_EN } else { 0 };
            if (value ^ current) & !allowed != 0 { return Err(Unsupported); }
            self.bank.sys_cfg = value & !visible;
            *visibility = value & visible != 0;
            return Ok(());
        }
        if let Some(slot) = MTRR_FIXED.iter().position(|&v| v == index) {
            for byte in value.to_le_bytes() {
                if byte & !0x1f != 0 || !valid_type(byte & 7)
                    || !*visibility && byte & 0x18 != 0 { return Err(Fault); }
            }
            let merged = if *visibility { value } else {
                value | self.bank.fixed[slot] & 0x1818_1818_1818_1818
            };
            if !matches!(self.phase, 2 | 3) && merged != self.bank.fixed[slot] { return Err(Unsupported); }
            self.bank.fixed[slot] = merged;
            return Ok(());
        }
        if (MTRR_VAR_BASE0..=VAR_LAST).contains(&index) {
            let mask = if index & 1 == 0 { 0x0000_ffff_ffff_f007 } else { 0x0000_ffff_ffff_f800 };
            if value & !mask != 0 || index & 1 == 0 && !valid_type(value as u8) { return Err(Fault); }
            if !matches!(self.phase, 2 | 3) && value != current { return Err(Unsupported); }
            let pair = &mut self.bank.variable[((index-MTRR_VAR_BASE0)/2) as usize];
            if index & 1 == 0 { pair.0 = value; } else { pair.1 = value; }
            return Ok(());
        }
        // Routing and MMIO controls retain physical values. HWCR belongs to
        // the CPU-local physical access owner, never this shared replay bank.
        if value == current { Ok(()) } else { Err(Unsupported) }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheWriteError { Fault, Unsupported }

#[repr(C, align(4096))]
pub struct CacheOwner { pub cores: [CacheCore; MAX_CACHE_CPUS] }
impl CacheOwner {
    pub const fn empty() -> Self {
        Self { cores: [const { TryLock::new(CacheCoreState::EMPTY) }; MAX_CACHE_CPUS] }
    }
    /// Sole post-EBS BSP writer; all captures complete and no guest has entered.
    pub fn initialize(&mut self, capture: &CacheCapture, ids: &[u32]) -> bool {
        self.initialize_detailed(capture,ids).is_ok()
    }
    pub fn initialize_detailed(&mut self,capture:&CacheCapture,ids:&[u32])->Result<(),(usize,CacheAdmissionFailure)> {
        for slot in 0..ids.len() {
            let mask=capture.domain_mask_detailed(slot,ids)?;
            if mask.trailing_zeros() as usize != slot { continue; }
            let bank=capture.observation(slot,ids.len()).ok_or((slot,CacheAdmissionFailure::new(11,0,capture.valid as u64,capture.count as u64)))?;
            *self.cores[slot].get_mut() = CacheCoreState { bank: *bank,
                members: mask, ..CacheCoreState::EMPTY };
        }
        Ok(())
    }
}
const _: () = assert!(core::mem::size_of::<CacheOwner>() == 3 * 4096);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwcr_uses_live_thread_state_and_preserves_every_non_counter_bit() {
        use core::cell::Cell;
        let baseline = 0x0900_6011;
        let hardware = Cell::new(baseline);
        let writes = Cell::new(0);
        for enabled in [true, true, false, false, true] {
            let requested = baseline | if enabled { HWCR_IRPERF_EN } else { 0 };
            let before = hardware.get();
            let count = writes.get();
            assert_eq!(access_hwcr(baseline, Some(requested), true, false, false,
                || hardware.get(), |v| { writes.set(writes.get()+1); hardware.set(v); }), Ok(requested));
            assert_eq!(writes.get(), count + u32::from(before != requested));
            assert_eq!(hardware.get() & !HWCR_IRPERF_EN, baseline);
            assert_eq!(access_hwcr(baseline, None, true, false, false,
                || hardware.get(), |_| panic!("RDMSR wrote HWCR")), Ok(requested));
        }
        // Live bit30 changes by firmware/SMM are accepted as physical state;
        // no stale per-core/per-thread shadow overrides the hardware value.
        hardware.set(baseline);
        assert_eq!(access_hwcr(baseline, None, true, false, false,
            || hardware.get(), |_| panic!()), Ok(baseline));
    }

    #[test]
    fn hwcr_rejects_non_counter_changes_drift_and_missing_owners_before_write() {
        let baseline = 0x0900_6011;
        for bit in (0..64).filter(|&bit| bit != 30) {
            let changed = baseline ^ (1 << bit);
            assert_eq!(access_hwcr(baseline, Some(changed), true, false, false,
                || baseline, |_| panic!("unsupported write reached hardware")),
                Err(HwcrError::UnsupportedChange { current: baseline, requested: changed }));
            for request in [None, Some(changed), Some(changed | HWCR_IRPERF_EN)] {
                assert_eq!(access_hwcr(baseline, request, true, false, false,
                    || changed, |_| panic!("drift reached hardware write")),
                    Err(HwcrError::PhysicalDrift { observed: changed, baseline }));
            }
        }
        assert_eq!(access_hwcr(baseline, Some(baseline | HWCR_IRPERF_EN), false, false, false,
            || baseline, |_| panic!()), Err(HwcrError::UnsupportedChange {
                current: baseline, requested: baseline | HWCR_IRPERF_EN }));
        assert_eq!(access_hwcr(baseline, Some(baseline), false, false, false,
            || baseline, |_| panic!()), Ok(baseline));
        assert_eq!(access_hwcr(baseline, None, true, true, false,
            || panic!("unsupported owner reached RDMSR"), |_| panic!()), Err(HwcrError::PmcVirtualization));
    }

    #[test]
    fn hwcr_failed_readback_reports_side_effect_without_false_success_or_rollback() {
        use core::cell::Cell;
        let baseline = 0x10;
        let writes = Cell::new(0);
        let requested = baseline | HWCR_IRPERF_EN;
        let result = access_hwcr(baseline, Some(requested), true, false, false,
            || baseline, |value| { assert_eq!(value, requested); writes.set(writes.get()+1); });
        assert_eq!(result, Err(HwcrError::Readback { observed: baseline, expected: requested }));
        assert_eq!(writes.get(), 1);
        let mut shared = replay_core();
        assert_eq!(shared.read(HWCR, false), None);
        assert_eq!(shared.write(HWCR, baseline, &mut false), Err(CacheWriteError::Unsupported));
    }

    #[test]
    fn hwcr_cpuid_fault_control_requires_owner_and_preserves_other_controls() {
        use core::cell::Cell;
        let baseline = 0x0900_6011;
        let physical = Cell::new(baseline);
        for enabled in [true, true, false] {
            let requested = baseline | if enabled { HWCR_CPUID_FLT_EN } else { 0 };
            assert_eq!(access_hwcr(baseline, Some(requested), false, false, true,
                || physical.get(), |v| physical.set(v)), Ok(requested));
            assert_eq!(access_hwcr(baseline, None, false, false, true,
                || physical.get(), |_| panic!()), Ok(requested));
            assert_eq!(physical.get() & !HWCR_CPUID_FLT_EN, baseline);
        }
        for bit in (0..64).filter(|b| ![30,35].contains(b)) {
            assert!(matches!(access_hwcr(baseline, Some(baseline ^ (1 << bit)), true, false, true,
                || physical.get(), |_| panic!()), Err(HwcrError::UnsupportedChange { .. })));
        }
    }

    #[test]
    fn owned_inventory_is_exactly_the_reviewed_register_set() {
        let expected = [
            0x200, 0x201, 0x202, 0x203, 0x204, 0x205, 0x206, 0x207,
            0x208, 0x209, 0x20a, 0x20b, 0x20c, 0x20d, 0x20e, 0x20f,
            0x250, 0x258, 0x259, 0x268, 0x269, 0x26a, 0x26b, 0x26c, 0x26d, 0x26e, 0x26f,
            0xfe, 0x2ff, 0xc001_0010, 0xc001_0015,
            0xc001_0016, 0xc001_0017, 0xc001_0018, 0xc001_0019,
            0xc001_001a, 0xc001_001d, 0xc001_0058,
        ];
        assert!(owned_msrs().eq(expected));
        assert!(expected.into_iter().all(owned_msr));
        // PAT keeps its hardware G_PAT owner; neighbours stay unowned.
        for index in [0x1ff, 0x210, 0x251, 0x25a, 0x267, 0x270, 0x277, 0x2fe, 0x300,
            0xc001_000f, 0xc001_0011, 0xc001_0014, 0xc001_001b, 0xc001_001c,
            0xc001_001e, 0xc001_0057, 0xc001_0059] {
            assert!(!owned_msr(index), "{index:#x}");
        }
    }

    #[test]
    fn post_ebs_survey_requires_every_fresh_sample_and_keeps_failure_stopped() {
        let survey = CacheSurvey::new();
        assert!(!survey.admit(0));
        assert!(!survey.admit(33));
        assert!(survey.complete_sample(2));
        assert!(!survey.complete_sample(2));
        assert!(!survey.complete_sample(32));
        assert!(!survey.admit(3));
        assert!(!survey.admitted());
        assert!(survey.complete_sample(0));
        assert!(!survey.admit(3));
        assert!(survey.complete_sample(1));
        assert!(survey.admit(3));
        assert!(survey.admitted());
        survey.abort();
        assert!(!survey.admitted());
        assert!(!survey.admit(3));
    }

    #[test]
    fn active_extended_fixed_tuple_validation_uses_complete_table() {
        let mut bank = CacheObservation { default: 0xc00, sys_cfg: 1 << 18, ..CacheObservation::EMPTY };
        for byte in 0..=255u8 {
            bank.fixed[7] = u64::from_le_bytes([byte; 8]);
            assert_eq!(bank.validate_active_fixed_types().is_ok(),
                matches!(byte, 0x00|0x01|0x04|0x05|0x08|0x09|0x10|0x15|0x18|0x19|0x1c|0x1e));
        }
        bank.fixed[7] = 0x1d1d_1d1d_1d1d_1d1d;
        assert_eq!(bank.validate_active_fixed_types().unwrap_err().index, 0x26c);
        for default in [0, 0x400, 0x800] {
            bank.default = default;
            assert!(bank.validate_active_fixed_types().is_ok());
        }
        bank.default = 0xc00;
        bank.sys_cfg = 0;
        assert_eq!(bank.validate_active_fixed_types().unwrap_err().predicate,21);
    }

    #[test]
    fn late_firmware_sync_is_admitted_only_from_complete_fresh_bank() {
        let bsp = CacheObservation { default: 0xc00, sys_cfg: 1 << 18,
            fixed: [0x1515_1515_1515_1515; 11], topology: [0,0,0,0x501f],
            ..CacheObservation::EMPTY };
        let mut earlier_ap = bsp;
        earlier_ap.topology = [1,1,0,0x501f];
        earlier_ap.fixed[7] = 0x1d1d_1d1d_1d1d_1d1d;
        assert!(!earlier_ap.restored_mtrrs(bsp.default,&bsp.variable,&bsp.fixed,bsp.sys_cfg));
        let fresh_ap = CacheObservation { fixed: bsp.fixed, ..earlier_ap };
        let mut capture = CacheCapture::empty();
        let survey = CacheSurvey::new();
        assert!(capture.initialize(2));
        assert!(capture.seed(1,fresh_ap));
        assert!(survey.complete_sample(1));
        assert!(capture.agrees_with_bsp_detailed(0,2).is_err());
        assert!(!survey.admit(2));
        assert!(capture.seed(0,bsp));
        assert!(survey.complete_sample(0));
        assert!(capture.agrees_with_bsp(0,2));
        assert_eq!(capture.domain_mask(1,&[0,1]),Some(2));
        let mut owner = CacheOwner::empty();
        assert!(owner.initialize(&capture,&[0,1]));
        assert!(survey.admit(2));
        assert!(capture.observation(1,2).unwrap().same_physical_state(&fresh_ap));
        assert!(!capture.observation(1,2).unwrap().same_physical_state(&earlier_ap));
    }

    fn replay_core() -> CacheCoreState {
        let mut bank = CacheObservation::EMPTY;
        bank.default = 0xc06; bank.sys_cfg = 1 << 18;
        bank.fixed.fill(0x1e1e_1e1e_1e1e_1e1e);
        CacheCoreState { bank, members: 0b1010, ..CacheCoreState::EMPTY }
    }

    #[test]
    fn delayed_e0_and_e1_consumers_cannot_lose_release_or_clear_next_generation() {
        let mut core = replay_core(); let baseline = core.bank;
        let generation = core.enter(2, 0x406).unwrap();
        assert_eq!(core.bank.default, 0xc06); // first E0 is not published alone
        assert_eq!(core.enter(2, 0x406), Err(CacheWriteError::Unsupported));
        core.enter(8, 0x406).unwrap();
        assert_eq!(core.phase, 2);
        core.leave(8, baseline.default, &baseline).unwrap();
        // Last E0 CPU reached E1 before the first host waiter was scheduled.
        assert_eq!(core.generation, generation);
        assert!(matches!(core.phase, 2 | 3));
        core.leave(2, baseline.default, &baseline).unwrap();
        core.depart(8, generation).unwrap();
        assert_eq!(core.phase, 4);
        assert_eq!(core.enter(8, 0x406), Err(CacheWriteError::Unsupported));
        core.depart(2, generation).unwrap();
        assert_eq!(core.phase, 0);
        core.enter(8, 0x406).unwrap();
        // Late old-generation waiter only observes completion; it cannot
        // perform another depart or clear the newly armed local guard/root.
        assert_ne!(core.generation, generation);
        assert_eq!(core.depart(2, generation), Err(CacheWriteError::Unsupported));
        assert_eq!(core.phase, 1);
    }

    #[test]
    fn shared_shadow_preserves_thread_visibility_hidden_attributes_and_final_routing() {
        let mut core = replay_core(); let baseline = core.bank;
        let mut a = false; let b = false;
        assert_eq!(core.read(0x250, a), Some(0x0606_0606_0606_0606));
        assert_eq!(core.write(0x250, baseline.fixed[0], &mut a), Err(CacheWriteError::Fault));
        core.enter(2, 0x406).unwrap(); core.enter(8, 0x406).unwrap();
        core.write(SYS_CFG, 1 << 19, &mut a).unwrap();
        assert!(a); assert!(!b);
        assert_eq!(core.read(0x250, a), Some(baseline.fixed[0]));
        assert_eq!(core.read(SYS_CFG, b), Some(0));
        core.leave(2, baseline.default, &baseline).unwrap();
        assert_eq!(core.leave(8, baseline.default, &baseline), Err(CacheWriteError::Unsupported));
        assert_eq!(core.leaving, 2);
        core.write(SYS_CFG, 1 << 18, &mut a).unwrap();
        core.leave(8, baseline.default, &baseline).unwrap();
        assert_eq!(core.phase, 4);
        assert_eq!(core.write(0xc001_001a, 1, &mut a), Err(CacheWriteError::Unsupported));
        assert_eq!(core.write(0xfe, 0, &mut a), Err(CacheWriteError::Fault));
    }

    #[test]
    fn startup_core_lease_excludes_e0_publication_until_local_commit_finishes() {
        let core = CacheCore::new(replay_core());
        let lease = core.try_lock().unwrap();
        assert_eq!(lease.phase, 0);
        assert!(core.with(|s| s.enter(2, 0x406)).is_none());
        drop(lease);
        assert_eq!(core.with(|s| s.enter(2, 0x406)), Some(Ok(0)));
        assert_ne!(core.try_lock().unwrap().phase, 0);
    }

    #[test]
    fn initial_capture_is_unavailable_until_every_unique_slot_finishes() {
        let mut capture = CacheCapture::empty();
        assert!(!capture.initialize(33));
        assert!(capture.initialize(2));
        assert!(!capture.initialize(2));
        assert!(capture.seed(1, CacheObservation::EMPTY));
        assert!(capture.observation(1, 2).is_none());
        assert!(!capture.seed(1, CacheObservation::EMPTY));
        assert!(capture.seed(0, CacheObservation::EMPTY));
        assert!(capture.observation(1, 2).is_some());
        assert!(capture.observation(2, 2).is_none());
        assert!(!capture.complete(1));
    }

    #[test]
    fn sharing_uses_captured_package_core_not_dense_slot_or_apic_parity() {
        let mut capture = CacheCapture::empty();
        assert!(capture.initialize(4));
        for (slot, (apic, core)) in [(18, 7), (2, 7), (19, 7), (3, 7)].into_iter().enumerate() {
            let observation = CacheObservation { topology: [apic, 0x100 | core, 0, 0x400f],
                ..CacheObservation::EMPTY };
            assert!(capture.seed(slot, observation));
        }
        assert_eq!(capture.domain_mask(0, &[18, 2, 19, 3]), Some(0b0101));
        assert_eq!(capture.domain_mask(1, &[18, 2, 19, 3]), Some(0b1010));
        assert_eq!(capture.domain_mask(0, &[18, 2, 18, 3]), None);
        capture.observations[2].default = 0xc06;
        assert_eq!(capture.domain_mask(0, &[18, 2, 19, 3]), None);
    }

    #[test]
    fn bsp_replay_admission_ignores_only_disabled_variable_contents() {
        let mut capture = CacheCapture::empty();
        assert!(capture.initialize(2));
        let mut bsp = CacheObservation::EMPTY;
        bsp.default = 0xc06;
        bsp.variable[3] = (0x1234_5006, 0xffff_ff00_0000);
        let peer = CacheObservation { variable: [(0, 0); 8], ..bsp };
        assert!(capture.seed(0, peer));
        assert!(capture.seed(1, bsp));
        assert!(capture.agrees_with_bsp(1, 2));
        capture.observations[0].variable[0] = (6, 0xffff_8000_0800);
        assert!(!capture.agrees_with_bsp(1, 2));
    }

    #[test]
    fn fixed_capture_restores_visibility_after_failed_readback() {
        use core::cell::Cell;
        let syscfg = Cell::new(1 << 18);
        let writes = Cell::new(0);
        let result = CacheObservation::capture(0x00b4_0f40, 48, Some([0, 0, 0, 0x400f]),
            |index| match index {
                0xfe => 0x508, 0xc001_0015 => 0x10,
                SYS_CFG => syscfg.get(), _ => 0,
            },
            |index, value| {
                assert_eq!(index, SYS_CFG);
                writes.set(writes.get() + 1);
                // Simulate a failed attempt to expose bit19; restoration is
                // nevertheless required and must target the original value.
                if value & SYS_CFG_MTRR_FIX_DRAM_MOD_EN == 0 { syscfg.set(value); }
            });
        assert!(result.is_none());
        assert_eq!(writes.get(), 2);
        assert_eq!(syscfg.get(), 1 << 18);
    }

    #[test]
    fn replay_can_zero_disabled_slots_but_cannot_change_active_ranges() {
        let mut physical = CacheObservation::EMPTY;
        physical.default = 0xc06;
        physical.sys_cfg = 1 << 18;
        physical.variable[0] = (0x8000_0006, 0xffff_8000_0000);
        physical.variable[1] = (6, 0xffff_8000_0800);
        physical.fixed.fill(0x1e1e_1e1e_1e1e_1e1e);
        let mut replay = physical.variable;
        replay[0] = (0, 0);
        assert!(physical.restored_mtrrs(physical.default, &replay, &physical.fixed,
            physical.sys_cfg | SYS_CFG_MTRR_FIX_DRAM_MOD_EN));
        replay[1].0 += 0x8000_0000;
        assert!(!physical.restored_mtrrs(physical.default, &replay, &physical.fixed, physical.sys_cfg));
        assert!(!physical.same_physical_state(&CacheObservation { variable: replay, ..physical }));
    }

    #[test]
    fn replay_cannot_change_default_fixed_or_shared_routing() {
        let mut physical = CacheObservation::EMPTY;
        physical.default = 0xc06;
        physical.sys_cfg = 1 << 18;
        assert!(!physical.restored_mtrrs(0x406, &physical.variable, &physical.fixed, physical.sys_cfg));
        let mut fixed = physical.fixed;
        fixed[0] = 0x10;
        assert!(!physical.restored_mtrrs(physical.default, &physical.variable, &fixed, physical.sys_cfg));
        assert!(!physical.restored_mtrrs(physical.default, &physical.variable, &physical.fixed, 0));
    }
}
