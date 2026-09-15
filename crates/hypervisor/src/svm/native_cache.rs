//! Retained native cache-control observations for the Family1Ah Model44h host.
//! PPR57896 rev3.00 pp123,126-130,173,202-206,210; APM2 rev3.44 7.7/7.9.
//! These records do not grant permission to write physical cache controls.

pub const MAX_CACHE_CPUS: usize = 32;
pub const FIXED_MSRS: [u32; 11] = [
    0x250, 0x258, 0x259, 0x268, 0x269, 0x26a, 0x26b, 0x26c, 0x26d, 0x26e, 0x26f,
];
pub const SYS_CFG: u32 = 0xc001_0010;
pub const FIXED_VISIBILITY: u64 = 1 << 19;

/// Post-EBS collection gate. Firmware may synchronize MTRRs in its final
/// callbacks, so no guest may consume the bank until every owned CPU sampled
/// it and the BSP admitted the complete capture. This gate never grants MSR
/// write permission or replaces the bank/domain checks.
pub struct CacheSurvey {
    sampled: core::sync::atomic::AtomicU32,
    admitted: core::sync::atomic::AtomicBool,
    failed: core::sync::atomic::AtomicBool,
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
        if signature!=0x00b4_0f40 {return Err(CacheAdmissionFailure::new(3,1,signature as u64,0x00b40f40));}
        if width!=48 {return Err(CacheAdmissionFailure::new(4,0x80000008,width as u64,48));}
        let topology=topology?;
        let capability = read(0xfe);
        if capability != 0x508 { return Err(CacheAdmissionFailure::new(5,0xfe,capability,0x508)); }
        let sys_cfg = read(SYS_CFG);
        if sys_cfg & !0x07fc_0000 != 0 || sys_cfg & 0x0780_0000 != 0 {
            return Err(CacheAdmissionFailure::new(6,SYS_CFG,sys_cfg,0x007c0000)); }
        let mut result = Self { capability, sys_cfg, topology, ..Self::EMPTY };
        result.default = read(0x2ff);
        result.top_mem = read(0xc001_001a);
        result.top_mem2 = read(0xc001_001d);
        for (index, value) in result.iorr.iter_mut().enumerate() {
            *value = read(0xc001_0016 + index as u32);
        }
        result.hwcr = read(0xc001_0015);
        // Preserve the current bounded profile's HWCR3=0 restriction while
        // diagnosing admission. HWCR4 retains INVD-to-WBINVD conversion.
        if result.hwcr & 0x18 != 0x10 { return Err(CacheAdmissionFailure::new(7,0xc0010015,result.hwcr,0x10)); }
        result.mmconfig = read(0xc001_0058);
        result.pat = read(0x277);
        for (index, pair) in result.variable.iter_mut().enumerate() {
            *pair = (read(0x200 + index as u32 * 2), read(0x201 + index as u32 * 2));
        }
        let visible = sys_cfg | FIXED_VISIBILITY;
        if visible != sys_cfg { write(SYS_CFG, visible); }
        let observed_visible=read(SYS_CFG);
        let changed = observed_visible == visible;
        if changed {
            for (value, index) in result.fixed.iter_mut().zip(FIXED_MSRS) { *value = read(index); }
        }
        if visible != sys_cfg { write(SYS_CFG, sys_cfg); }
        let observed_restored=read(SYS_CFG);
        if !changed {return Err(CacheAdmissionFailure::new(8,SYS_CFG,observed_visible,visible));}
        if observed_restored!=sys_cfg {return Err(CacheAdmissionFailure::new(9,SYS_CFG,observed_restored,sys_cfg));}
        let final_default=read(0x2ff);
        if final_default!=result.default {return Err(CacheAdmissionFailure::new(10,0x2ff,final_default,result.default));}
        result.validate_active_fixed_types()?;
        Ok(result)
    }

    /// APM2 rev3.44 7.9.1/Table7-13, PDF295-296: valid type bits alone do
    /// not establish a supported active extended tuple. Dormant fixed type
    /// fields are not interpreted as though E/FE and extended attrs were on.
    pub fn validate_active_fixed_types(&self) -> Result<(), CacheAdmissionFailure> {
        if self.default & 0xc00 != 0xc00 { return Ok(()); }
        // The bounded native bootstrap profile requires enabled DRAM attrs;
        // SYS_CFG18=0 would instead route low ranges as attr00. Do not silently
        // interpret the hidden raw fields as active in that configuration.
        if self.sys_cfg & (1 << 18) == 0 {
            return Err(CacheAdmissionFailure::new(21,SYS_CFG,self.sys_cfg,1 << 18));
        }
        for (&index, &value) in FIXED_MSRS.iter().zip(&self.fixed) {
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
        for (index,expected,observed) in [(0x2ff,self.default,peer.default),
            (SYS_CFG,self.sys_cfg&!FIXED_VISIBILITY,peer.sys_cfg&!FIXED_VISIBILITY),
            (0xc001001a,self.top_mem,peer.top_mem),(0xc001001d,self.top_mem2,peer.top_mem2),
            (0xc0010058,self.mmconfig,peer.mmconfig)] {
            if observed!=expected {return Some(fail(index,observed,expected));}
        }
        if !ignore_disabled && self.capability!=peer.capability {return Some(fail(0xfe,peer.capability,self.capability));}
        for (i,(&expected,&observed)) in self.iorr.iter().zip(&peer.iorr).enumerate() {
            if observed!=expected {return Some(fail(0xc0010016+i as u32,observed,expected));}
        }
        for (i,(&(base,mask),&(other_base,other_mask))) in self.variable.iter().zip(&peer.variable).enumerate() {
            if ignore_disabled && mask&0x800==0 && other_mask&0x800==0 {continue;}
            if base!=other_base {return Some(fail(0x200+2*i as u32,other_base,base));}
            if mask!=other_mask {return Some(fail(0x201+2*i as u32,other_mask,mask));}
        }
        for ((&expected,&observed),index) in self.fixed.iter().zip(&peer.fixed).zip(FIXED_MSRS) {
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
            && (self.sys_cfg ^ sys_cfg) & !FIXED_VISIBILITY == 0
            && self.variable.iter().zip(variable).all(|(&(base, mask), &(new_base, new_mask))| {
                if mask & 0x800 == 0 && new_mask & 0x800 == 0 { true }
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
    (0x200..=0x20f).chain(FIXED_MSRS).chain([0xfe, 0x2ff, SYS_CFG, 0xc001_0015,
        0xc001_0016, 0xc001_0017, 0xc001_0018, 0xc001_0019, 0xc001_001a,
        0xc001_001d, 0xc001_0058])
}

/// Shared physical-core bank. Access is serialized only while software copies
/// or changes state; no caller may retain this guard while waiting for a peer.
#[repr(C)]
pub struct CacheCore {
    lock: core::sync::atomic::AtomicBool,
    state: core::cell::UnsafeCell<CacheCoreState>,
}
unsafe impl Sync for CacheCore {}

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

    pub fn read(&self, index: u32, visibility: bool, local: &CacheObservation) -> Option<u64> {
        Some(match index {
            0xfe => self.bank.capability, 0x2ff => self.bank.default,
            SYS_CFG => (self.bank.sys_cfg & !FIXED_VISIBILITY) | if visibility { FIXED_VISIBILITY } else { 0 },
            0x200..=0x20f => { let pair = self.bank.variable[((index-0x200)/2) as usize];
                if index & 1 == 0 { pair.0 } else { pair.1 } },
            0xc001_0015 => local.hwcr,
            0xc001_0016..=0xc001_0019 => self.bank.iorr[(index-0xc001_0016) as usize],
            0xc001_001a => self.bank.top_mem, 0xc001_001d => self.bank.top_mem2,
            0xc001_0058 => self.bank.mmconfig,
            _ => { let slot = FIXED_MSRS.iter().position(|&v| v == index)?;
                self.bank.fixed[slot] & if visibility { u64::MAX } else { 0x0707_0707_0707_0707 } },
        })
    }

    /// Ordinary logical writes during CD-constrained replay. Boundary E0/E1
    /// transitions are owned separately by the paired continuation barrier.
    pub fn write(&mut self, index: u32, value: u64, visibility: &mut bool,
        local: &CacheObservation) -> Result<(), CacheWriteError>
    {
        use CacheWriteError::{Fault, Unsupported};
        let current = self.read(index, *visibility, local).ok_or(Unsupported)?;
        if index == 0xfe { return Err(Fault); }
        if index == SYS_CFG {
            if value & !0x07fc_0000 != 0 { return Err(Fault); }
            let allowed = FIXED_VISIBILITY | if matches!(self.phase, 2 | 3) { 1 << 18 } else { 0 };
            if (value ^ current) & !allowed != 0 { return Err(Unsupported); }
            self.bank.sys_cfg = value & !FIXED_VISIBILITY;
            *visibility = value & FIXED_VISIBILITY != 0;
            return Ok(());
        }
        if let Some(slot) = FIXED_MSRS.iter().position(|&v| v == index) {
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
        if (0x200..=0x20f).contains(&index) {
            let mask = if index & 1 == 0 { 0x0000_ffff_ffff_f007 } else { 0x0000_ffff_ffff_f800 };
            if value & !mask != 0 || index & 1 == 0 && !valid_type(value as u8) { return Err(Fault); }
            if !matches!(self.phase, 2 | 3) && value != current { return Err(Unsupported); }
            let pair = &mut self.bank.variable[((index-0x200)/2) as usize];
            if index & 1 == 0 { pair.0 = value; } else { pair.1 = value; }
            return Ok(());
        }
        // Routing, MMIO and host page-walk/INVD controls retain physical values.
        if value == current { Ok(()) } else { Err(Unsupported) }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheWriteError { Fault, Unsupported }
pub fn valid_default(value: u64) -> bool { value & !0xcff == 0 && valid_type(value as u8) }
fn valid_type(value: u8) -> bool { matches!(value, 0 | 1 | 4 | 5 | 6) }
impl CacheCore {
    const EMPTY: Self = Self { lock: core::sync::atomic::AtomicBool::new(false),
        state: core::cell::UnsafeCell::new(CacheCoreState::EMPTY) };
    pub fn with<R>(&self, f: impl FnOnce(&mut CacheCoreState) -> R) -> Option<R> {
        let mut guard = self.try_lock()?;
        Some(f(&mut guard))
    }
    pub fn try_lock(&self) -> Option<CacheCoreGuard<'_>> {
        use core::sync::atomic::Ordering;
        if self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return None;
        }
        Some(CacheCoreGuard { core: self })
    }
}
pub struct CacheCoreGuard<'a> { core: &'a CacheCore }
impl core::ops::Deref for CacheCoreGuard<'_> {
    type Target = CacheCoreState;
    fn deref(&self) -> &CacheCoreState { unsafe { &*self.core.state.get() } }
}
impl core::ops::DerefMut for CacheCoreGuard<'_> {
    fn deref_mut(&mut self) -> &mut CacheCoreState { unsafe { &mut *self.core.state.get() } }
}
impl Drop for CacheCoreGuard<'_> {
    fn drop(&mut self) { self.core.lock.store(false, core::sync::atomic::Ordering::Release); }
}

#[repr(C, align(4096))]
pub struct CacheOwner { pub cores: [CacheCore; MAX_CACHE_CPUS] }
impl CacheOwner {
    pub const fn empty() -> Self { Self { cores: [const { CacheCore::EMPTY }; MAX_CACHE_CPUS] } }
    /// Sole post-EBS BSP writer; all captures complete and no guest has entered.
    pub fn initialize(&mut self, capture: &CacheCapture, ids: &[u32]) -> bool {
        self.initialize_detailed(capture,ids).is_ok()
    }
    pub fn initialize_detailed(&mut self,capture:&CacheCapture,ids:&[u32])->Result<(),(usize,CacheAdmissionFailure)> {
        for slot in 0..ids.len() {
            let mask=capture.domain_mask_detailed(slot,ids)?;
            if mask.trailing_zeros() as usize != slot { continue; }
            let bank=capture.observation(slot,ids.len()).ok_or((slot,CacheAdmissionFailure::new(11,0,capture.valid as u64,capture.count as u64)))?;
            *self.cores[slot].state.get_mut() = CacheCoreState { bank: *bank,
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
        assert_eq!(core.read(0x250, a, &baseline), Some(0x0606_0606_0606_0606));
        assert_eq!(core.write(0x250, baseline.fixed[0], &mut a, &baseline), Err(CacheWriteError::Fault));
        core.enter(2, 0x406).unwrap(); core.enter(8, 0x406).unwrap();
        core.write(SYS_CFG, 1 << 19, &mut a, &baseline).unwrap();
        assert!(a); assert!(!b);
        assert_eq!(core.read(0x250, a, &baseline), Some(baseline.fixed[0]));
        assert_eq!(core.read(SYS_CFG, b, &baseline), Some(0));
        core.leave(2, baseline.default, &baseline).unwrap();
        assert_eq!(core.leave(8, baseline.default, &baseline), Err(CacheWriteError::Unsupported));
        assert_eq!(core.leaving, 2);
        core.write(SYS_CFG, 1 << 18, &mut a, &baseline).unwrap();
        core.leave(8, baseline.default, &baseline).unwrap();
        assert_eq!(core.phase, 4);
        assert_eq!(core.write(0xc001_001a, 1, &mut a, &baseline), Err(CacheWriteError::Unsupported));
        assert_eq!(core.write(0xfe, 0, &mut a, &baseline), Err(CacheWriteError::Fault));
    }

    #[test]
    fn startup_core_lease_excludes_e0_publication_until_local_commit_finishes() {
        let core = CacheCore { lock: core::sync::atomic::AtomicBool::new(false),
            state: core::cell::UnsafeCell::new(replay_core()) };
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
                if value & FIXED_VISIBILITY == 0 { syscfg.set(value); }
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
            physical.sys_cfg | FIXED_VISIBILITY));
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

/// Explicit disposable-backend model. Standard MTRRs/PAT remain actual QEMU
/// MSR reads. QEMU does not expose the target AMD hidden fixed attributes or
/// its sharing profile, so those fields are modeled. Neither this method nor
/// its injected failures exists in a normal native build. These tests prove
/// survey ordering and refusal control flow, never physical cache coherence.
#[cfg(feature = "native-cache-survey-fixture")]
impl CacheObservation {
    pub fn capture_fixture(apic_id: u32, stage: u8, mut read: impl FnMut(u32) -> u64)
        -> Result<Self, CacheAdmissionFailure>
    {
        let visibility = core::cell::Cell::new(1u64 << 18);
        let mut value = Self::capture_detailed(0x00b4_0f40, 48,
            Ok([apic_id, apic_id, 0, 0x501f]),
            |index| match index {
                SYS_CFG => visibility.get(),
                0xc001_0015 => 0x10,
                0xc001_001a => 0x1000_0000,
                0xc001_0016..=0xc001_0019 | 0xc001_001d | 0xc001_0058 => 0,
                _ => {
                    let raw = read(index);
                    if FIXED_MSRS.contains(&index) {
                        // WP uses the supported asymmetric extended tuple15;
                        // other conventional types use both DRAM attributes.
                        u64::from_le_bytes(raw.to_le_bytes().map(|ty|
                            if ty == 5 { 0x15 } else { ty | 0x18 }))
                    } else { raw }
                }
            },
            |index, requested| { if index == SYS_CFG { visibility.set(requested); } })?;
        if cfg!(feature = "native-cache-survey-mismatch") && apic_id == 1 {
            // Supported but intentionally different modeled effective bank.
            value.fixed[7] = if value.fixed[7] == 0x1515_1515_1515_1515 {
                0x1e1e_1e1e_1e1e_1e1e
            } else { 0x1515_1515_1515_1515 };
        }
        if cfg!(feature = "native-cache-survey-drift") && apic_id == 1 && stage == 1 {
            // Local raw-bank drift is deliberately injected only at arm.
            value.variable[7].0 ^= 0x1000;
        }
        Ok(value)
    }
}
