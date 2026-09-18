//! Stopped-guest memory reads through the temporary scratch alias, for
//! instruction fetch, with their MTRR admission.

use core::{
    arch::{asm, x86_64::__cpuid_count},
    ptr,
};

use crate::{
    arch::x86_64::msr::{
        MTRR_CAP, PAT, SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN,
        SYS_CFG_MTRR_FIX_DRAM_MOD_EN,
    },
    boot::memory::ValidatedMemoryMap,
    host::resident::{
        runtime::{
            CACHE_NPT, PHYSICAL_BITS, POOL, RAM, RAM_COUNT, TABLES, image_start,
            msr::{read_msr, write_msr},
            startup::mailboxes,
        },
        terminal,
    },
    memory::address::EncryptionState,
    svm::{vmcb::Vmcb, x2avic::startup::try_lock_routes},
};

/// Existing temporary RAM alias shared by CPUID, MSR and cache-owner fetches.
/// Owns one stopped CPU; each read removes its alias before returning.
pub(super) struct GuestReader {
    deny_low: bool,
    map: ValidatedMemoryMap<'static>,
    monitor: crate::memory::address::PhysicalRange,
    mt: crate::memory::mtrrs::Mtrrs,
    pub(super) width: u8,
    pub(super) guest_pat: u64,
    startup_owned: bool,
    pub(super) failure: Option<terminal::FetchReadFailure>,
    count: usize,
}

impl GuestReader {
    pub(super) unsafe fn new(
        vmcb: &Vmcb,
        startup_owned: bool,
        count: usize,
    ) -> Result<Self, terminal::FetchReadFailure> {
        use crate::memory::address::AddressPolicy;
        use terminal::FetchReadFailure as R;
        let width = unsafe { PHYSICAL_BITS };
        let map = unsafe { core::slice::from_raw_parts(ptr::addr_of!(RAM).cast(), RAM_COUNT) };
        let map = ValidatedMemoryMap::new(map, width).map_err(|_| R::MemoryMap)?;
        let policy =
            AddressPolicy::new(width, EncryptionState::Unencrypted { encryption_bit: None })
                .map_err(|_| R::AddressPolicy)?;
        let (pool_base, pool_bytes) = unsafe { POOL };
        let monitor = policy.validate(pool_base, pool_bytes, 4096).map_err(|_| R::MonitorRange)?;
        let host_pat = unsafe { read_msr(PAT) };
        if host_pat & 255 != 6 {
            return Err(R::HostPat);
        }
        let mt = unsafe { native_mtrrs(width) }.ok_or(R::MtrrCapture)?;
        let guest_pat = u64::from_le_bytes(vmcb.bytes()[0x668..0x670].try_into().unwrap());
        Ok(Self {
            deny_low: vmcb.nested_root() == ptr::addr_of!(CACHE_NPT) as u64,
            map,
            monitor,
            mt,
            width,
            guest_pat,
            startup_owned,
            failure: None,
            count,
        })
    }

    /// Same stopped/private-root WB RAM admission as fetch_instruction.
    pub(super) unsafe fn read(&mut self, address: u64, bytes: usize) -> Option<u64> {
        use terminal::FetchReadFailure as R;
        if !matches!(bytes, 1 | 8)
            || (address & 4095) + bytes as u64 > 4096
            || (bytes == 8 && address & 7 != 0)
        {
            self.failure = Some(R::ReadShape);
            return None;
        }
        let physical = address & !4095;
        if self.deny_low && physical < 0x100000 {
            self.failure = Some(R::RamAdmission);
            return None;
        }
        if self.map.permit_guest_ram(physical, 4096, self.monitor).is_err() {
            self.failure = Some(R::RamAdmission);
            return None;
        }
        // Serialize core-shared SYS_CFG18 with every low-RAM sample and access.
        // Fetch completes before the APIC operation takes this same route gate.
        let _memory_controls = if self.startup_owned && physical < 0x100000 {
            match try_lock_routes(unsafe { mailboxes(self.count) }) {
                Ok(guard) => Some(guard),
                Err(_) => {
                    self.failure = Some(R::MemoryControlBusy);
                    return None;
                }
            }
        } else {
            None
        };
        let wb = if self.startup_owned && physical < 0x100000 {
            use crate::memory::mtrrs::{CAP_FIX, DEF_TYPE_DEFINED, DEF_TYPE_E, DEF_TYPE_FE};
            if self.mt.default & !DEF_TYPE_DEFINED != 0
                || self.mt.default & (DEF_TYPE_E | DEF_TYPE_FE) != DEF_TYPE_E | DEF_TYPE_FE
                || unsafe { read_msr(MTRR_CAP) } & CAP_FIX == 0
            {
                self.failure = Some(R::FixedMtrrControl);
                return None;
            }
            let Some((index, shift)) = crate::memory::mtrrs::Mtrrs::fixed_range_register(physical)
            else {
                self.failure = Some(R::FixedMtrrRange);
                return None;
            };
            if crate::memory::mtrrs::Tom2Default::supported_profile(
                __cpuid_count(1, 0).eax,
                self.width,
            ) {
                unsafe { native_fixed_page_is_wb(index, shift) }
            } else {
                (unsafe { read_msr(index) } >> shift) & 255 == 6
            }
        } else {
            self.mt.page_is_wb(physical)
        };
        if !wb {
            self.failure = Some(R::PhysicalMemoryNotWb);
            return None;
        }
        let window = ptr::addr_of!(image_start) as u64 + 0xff000;
        let slot = unsafe {
            ptr::addr_of_mut!((*ptr::addr_of_mut!(TABLES)).0[3][((window >> 12) & 511) as usize])
        };
        // The final page has no backing object and starts absent. Never replace
        // an unexpected retained mapping or leave an alias across guest entry.
        if unsafe { ptr::read_volatile(slot) } != 0 {
            self.failure = Some(R::ScratchAliasOccupied);
            return None;
        }
        unsafe {
            ptr::write_volatile(slot, physical | 1 | (1 << 63));
            asm!("invlpg [{}]",in(reg)window,options(nostack,preserves_flags));
        }
        let pointer = (window + (address & 4095)) as *const u8;
        let value = unsafe {
            if bytes == 1 {
                ptr::read_volatile(pointer) as u64
            } else {
                ptr::read_volatile(pointer.cast::<u64>())
            }
        };
        unsafe {
            ptr::write_volatile(slot, 0);
            asm!("invlpg [{}]",in(reg)window,options(nostack,preserves_flags));
        }
        Some(value)
    }
}

/// Read only one qualified physical page through the final, otherwise absent
/// arena PTE. The alias is supervisor RO/NX and is removed before returning.
/// Physical source pages are admitted RAM, effective WB and outside the monitor.
/// Single CPU, stopped guest and GIF/IF clear make the temporary PTE exclusive.
pub(super) unsafe fn fetch_instruction(
    vmcb: &Vmcb,
    startup_owned: bool,
    count: usize,
) -> Result<[u8; 2], u16> {
    let mut reader =
        unsafe { GuestReader::new(vmcb, startup_owned, count) }.map_err(|e| e as u16)?;
    let result = if startup_owned {
        super::fetch::startup_instruction(
            vmcb,
            reader.width,
            reader.guest_pat,
            |address, bytes| unsafe { reader.read(address, bytes) },
        )
    } else {
        super::fetch::instruction(vmcb, reader.width, reader.guest_pat, |address, bytes| unsafe {
            reader.read(address, bytes)
        })
    };
    result.map_err(|error| {
        reader.failure.map_or_else(|| terminal::fetch_failure_code(error), |e| e as u16)
    })
}

/// Enumerated architectural MTRRs, captured boundedly on the owning CPU.
/// Shared by diagnostic UC admission and stopped instruction reading.
pub(super) unsafe fn native_mtrrs(width: u8) -> Option<crate::memory::mtrrs::Mtrrs> {
    crate::memory::mtrrs::Mtrrs::read(width, __cpuid_count(1, 0).eax, |index| unsafe {
        read_msr(index)
    })
    .ok()
}

/// PPR57896 p202/43, APM2 7.9.1: bit19 is thread-private visibility;
/// bit18 is core-shared routing. Caller holds the shared route gate through
/// this sample and the eventual low RAM read. Fixed writes retain the native
/// trusted OS rendezvous contract. All executing monitor storage is >=1MiB.
unsafe fn native_fixed_page_is_wb(index: u32, shift: u8) -> bool {
    use crate::memory::mtrrs::Mtrrs;
    let original = unsafe { read_msr(SYS_CFG) };
    if original & !SYS_CFG_DEFINED != 0
        || original & SYS_CFG_ENCRYPTION != 0
        || original & SYS_CFG_MTRR_FIX_DRAM_EN == 0
    {
        return false;
    }
    let visible = original | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
    if visible != original {
        unsafe {
            write_msr(SYS_CFG, visible);
        }
    }
    let matched = unsafe { read_msr(SYS_CFG) } == visible;
    let byte = if matched { (unsafe { read_msr(index) } >> shift) as u8 } else { 0 };
    if visible != original {
        unsafe {
            write_msr(SYS_CFG, original);
        }
    }
    let restored = unsafe { read_msr(SYS_CFG) } == original;
    matched && restored && Mtrrs::native_fixed_page_is_wb(visible, byte)
}
