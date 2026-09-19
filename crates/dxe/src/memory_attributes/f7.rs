//! Local Get-only bootstrap for the admitted F7/Ryzen firmware environment.
//!
//! The access premise is the same trusted firmware identity mapping required by
//! native table preparation: allocated firmware table sources have resident,
//! readable identity aliases. RAM descriptors and the checks here do NOT prove
//! that premise. No exception handler, firmware setter or published protocol is
//! involved. The later native cache, table-closure and CPU gates still apply.

use svmvisor_hypervisor::boot::memory::ValidatedMemoryMap;
use svmvisor_memory_attributes::{Config, Error};

#[cfg(all(target_os = "uefi", target_arch = "x86_64"))]
pub use native::F7TableReader;
pub use svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE;

const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const LOW_CANONICAL_END: u64 = 1 << 47;
const REQUIRED_CR0: u64 = (1 << 31) | (1 << 16) | 1;
const FORBIDDEN_CR4: u64 = (1 << 12) | (1 << 17) | (1 << 21) | (1 << 22) | (1 << 23) | (1 << 24);

/// Integer observations are intentionally constructible for policy tests; this
/// type alone does not authenticate a CPU or authorize a native load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuCapabilities {
    pub max_basic: u32,
    pub vendor: [u32; 3],
    pub signature: u32,
    pub leaf1_ecx: u32,
    pub leaf1_edx: u32,
    pub max_extended: u32,
    pub extended_edx: u32,
    pub physical_bits: u8,
    pub topology_ebx: u32,
    pub topology_ecx: u32,
    pub apic_id: u32,
    pub encryption_eax: u32,
    pub encryption_ebx: u32,
    pub multi_key: [u32; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuObservation {
    pub cpu: CpuCapabilities,
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub efer: u64,
    pub sys_cfg: u64,
    pub sev_status: u64,
}

/// Stable diagnostic reasons carried in the upper half of native refusals.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum F7Failure {
    PrivilegeOrFlags = 1,
    Vendor = 2,
    Signature = 3,
    BasicFeatures = 4,
    ExtendedFeatures = 5,
    PhysicalWidth = 6,
    Topology = 7,
    SmeCapability = 8,
    MultiKey = 9,
    Cr0 = 10,
    Cr3 = 11,
    Cr4 = 12,
    Efer = 13,
    SysCfg = 14,
    Sev = 15,
    RootSource = 16,
    TableSource = 17,
    ContextChanged = 18,
    InvalidQuery = 19,
    UnsupportedQuery = 20,
    NoMapping = 21,
    Capacity = 22,
    AccessDenied = 23,
    DeviceError = 24,
}

impl F7Failure {
    pub const fn from_error(error: Error) -> Self {
        match error {
            Error::InvalidParameter => Self::InvalidQuery,
            Error::Unsupported => Self::UnsupportedQuery,
            Error::NoMapping => Self::NoMapping,
            Error::OutOfResources => Self::Capacity,
            Error::AccessDenied => Self::AccessDenied,
            Error::DeviceError => Self::DeviceError,
        }
    }

    pub const fn code(self) -> u16 {
        self as u16
    }

    pub const fn error(self) -> Error {
        match self {
            Self::PrivilegeOrFlags | Self::ContextChanged | Self::AccessDenied => {
                Error::AccessDenied
            }
            Self::InvalidQuery => Error::InvalidParameter,
            Self::NoMapping => Error::NoMapping,
            Self::Capacity => Error::OutOfResources,
            Self::DeviceError => Error::DeviceError,
            _ => Error::Unsupported,
        }
    }
}

/// Validate actual observations against the separately captured paging config.
/// This excludes address encryption and unsupported interpretation modes. It
/// deliberately makes no claim about effective PAT/MTRR types or readability.
pub fn validate_observation(config: Config, state: CpuObservation) -> Result<(), Error> {
    validate_observation_detailed(config, state).map_err(F7Failure::error)
}

pub fn validate_observation_detailed(
    config: Config,
    state: CpuObservation,
) -> Result<(), F7Failure> {
    validate_capabilities_detailed(state.cpu)?;
    if config.physical_bits != state.cpu.physical_bits {
        return Err(F7Failure::PhysicalWidth);
    }
    if config.root == 0
        || config.root & 4095 != 0
        || config.root >= LOW_CANONICAL_END
        || state.cr3 & !(ADDRESS | 0x18) != 0
        || state.cr3 & ADDRESS != config.root
    {
        return Err(F7Failure::Cr3);
    }
    if state.cr0 & REQUIRED_CR0 != REQUIRED_CR0 || state.cr0 & 0x6000_0000 != 0 {
        return Err(F7Failure::Cr0);
    }
    if state.cr4 & (1 << 5) == 0 || state.cr4 & FORBIDDEN_CR4 != 0 {
        return Err(F7Failure::Cr4);
    }
    if state.efer & 0x500 != 0x500
        || state.efer & !0x0034_fd01 != 0
        || state.efer & (1 << 20) != 0
        || config.nxe != (state.efer & (1 << 11) != 0)
        || (config.nxe && state.cpu.extended_edx & (1 << 20) == 0)
    {
        return Err(F7Failure::Efer);
    }
    if config.page1gb != (state.cpu.extended_edx & (1 << 26) != 0) {
        return Err(F7Failure::ExtendedFeatures);
    }
    if state.sys_cfg & !0x07fc_0000 != 0 || state.sys_cfg & 0x0780_0000 != 0 {
        return Err(F7Failure::SysCfg);
    }
    if state.sev_status != 0 {
        return Err(F7Failure::Sev);
    }
    Ok(())
}

/// Check the exact CPU/capability guards before any target-specific MSR read.
pub fn validate_capabilities(cpu: CpuCapabilities) -> Result<(), Error> {
    validate_capabilities_detailed(cpu).map_err(F7Failure::error)
}

pub fn validate_capabilities_detailed(cpu: CpuCapabilities) -> Result<(), F7Failure> {
    if cpu.vendor != [0x6874_7541, 0x6974_6e65, 0x444d_4163] {
        return Err(F7Failure::Vendor);
    }
    if cpu.signature != TARGET_SIGNATURE {
        return Err(F7Failure::Signature);
    }
    if cpu.max_basic < 0x0b
        || cpu.leaf1_ecx & (1 << 31) != 0
        || cpu.leaf1_edx & 0x0001_1060 != 0x0001_1060
    {
        return Err(F7Failure::BasicFeatures);
    }
    if cpu.max_extended < 0x8000_0023 || cpu.extended_edx & (1 << 29) == 0 {
        return Err(F7Failure::ExtendedFeatures);
    }
    if cpu.physical_bits != 48 {
        return Err(F7Failure::PhysicalWidth);
    }
    if cpu.topology_ebx & 0xffff == 0 || (cpu.topology_ecx >> 8) & 0xff == 0 {
        return Err(F7Failure::Topology);
    }
    if cpu.encryption_eax & 1 == 0
        || cpu.encryption_eax & !0x41ff_ffff != 0
        || cpu.encryption_ebx & !0xffff != 0
        || cpu.encryption_ebx & 63 != 51
        || (cpu.encryption_ebx >> 6) & 63 > 6
    {
        return Err(F7Failure::SmeCapability);
    }
    if cpu.multi_key[0] & !1 != 0
        || cpu.multi_key[1] & !0xffff != 0
        || cpu.multi_key[2] != 0
        || cpu.multi_key[3] != 0
    {
        return Err(F7Failure::MultiKey);
    }
    Ok(())
}

/// Metadata-only filtering. Every byte of the containing source page must be
/// allocated firmware RAM with WB capability and no advertised RP. This never
/// converts a physical address to a pointer or establishes current access.
pub fn validate_table_source(memory: &ValidatedMemoryMap<'_>, address: u64) -> Result<(), Error> {
    if address == 0 || address & 7 != 0 || address >= LOW_CANONICAL_END {
        return Err(Error::Unsupported);
    }
    memory
        .permit_table_entry(address)
        .and_then(|_| memory.permit_gdt_copy(address & !4095, 4096))
        .map_err(|_| Error::Unsupported)?;
    Ok(())
}

#[cfg(all(target_os = "uefi", target_arch = "x86_64"))]
mod native {
    use super::*;
    use core::{arch::asm, marker::PhantomData, ptr};
    use svmvisor_hypervisor::arch::x86_64::msr::{EFER, SEV_STATUS};
    use svmvisor_memory_attributes::{Attributes, Memory, x86};

    /// Lexically local reader. It cannot be sent to an AP or shared; no stored
    /// handler or protocol pointer survives its destruction.
    pub struct F7TableReader<'memory, 'descriptors> {
        memory: &'memory ValidatedMemoryMap<'descriptors>,
        config: Config,
        original: CpuObservation,
        reads: usize,
        last_failure: Option<F7Failure>,
        not_send_sync: PhantomData<*mut ()>,
    }

    impl<'memory, 'descriptors> F7TableReader<'memory, 'descriptors> {
        /// # Safety
        /// Run synchronously on the admitted BSP at APPLICATION or NOTIFY in
        /// the existing cooperating-firmware preparation interval. The caller
        /// supplies the ordinary firmware identity-access contract: every
        /// allocated table source reached through the original root has a
        /// readable, resident identity alias with compatible RAM/cache/routing
        /// provenance. The memory map, code, stack and sources remain valid;
        /// no allocation/free, mapping/protection change, ExitBootServices or
        /// unload may intervene while using this reader. Firmware, APs, SMM and
        /// DMA must preserve those allocations, translations and permissions.
        /// Hardware A/D changes are immaterial to this read-only interpretation.
        ///
        /// These premises are NOT inferred from GetMemoryMap, a successful Get,
        /// or a checked CR3. This bootstrap supplies no fault containment for a
        /// violated premise. Target-specific MSRs use the same exact CPU guard
        /// as native cache capture. Ordinary architectural reads must be safe
        /// under the caller's native firmware/MSR contract. Effective cache and
        /// complete native-launch admission remain the later existing gates.
        pub unsafe fn new(
            memory: &'memory ValidatedMemoryMap<'descriptors>,
            config: Config,
        ) -> Result<Self, Error> {
            unsafe { Self::new_detailed(memory, config) }.map_err(F7Failure::error)
        }

        /// # Safety
        /// Identical firmware identity-access and lifetime contract to `new`.
        pub unsafe fn new_detailed(
            memory: &'memory ValidatedMemoryMap<'descriptors>,
            config: Config,
        ) -> Result<Self, F7Failure> {
            let original = unsafe { observe() }?;
            validate_observation_detailed(config, original)?;
            validate_table_source(memory, config.root).map_err(|_| F7Failure::RootSource)?;
            Ok(Self {
                memory,
                config,
                original,
                reads: 0,
                last_failure: None,
                not_send_sync: PhantomData,
            })
        }

        pub fn config(&self) -> Config {
            self.config
        }

        pub fn observation(&self) -> CpuObservation {
            self.original
        }

        pub fn reads(&self) -> usize {
            self.reads
        }

        /// Observe actual table protections. Interrupt masking covers the whole
        /// bounded walk; it is not a lock against SMM, DMA or noncooperating APs.
        pub fn get(&mut self, base: u64, length: u64) -> Result<u64, Error> {
            self.get_detailed(base, length).map_err(F7Failure::error)
        }

        pub fn get_detailed(&mut self, base: u64, length: u64) -> Result<u64, F7Failure> {
            self.last_failure = None;
            self.check_context()?;
            let _interrupts = unsafe { InterruptScope::enter() };
            self.check_context()?;
            let result = x86::get(self, self.config, base, length);
            // Do not let an apparently successful read conceal context drift.
            self.check_context()?;
            result
                .map_err(|error| self.last_failure.unwrap_or_else(|| F7Failure::from_error(error)))
        }

        /// No registration or allocation needs cleanup. This checks that the
        /// captured context still applies before the existing closure is handed
        /// to later gates. Those gates retain their own direct-access contract.
        pub fn finish_handoff(&mut self) -> Result<(), Error> {
            self.finish_handoff_detailed().map_err(F7Failure::error)
        }

        pub fn finish_handoff_detailed(&mut self) -> Result<(), F7Failure> {
            self.check_context()
        }

        fn check_context(&self) -> Result<(), F7Failure> {
            let now = unsafe { observe() }?;
            validate_observation_detailed(self.config, now)?;
            if now != self.original {
                return Err(F7Failure::ContextChanged);
            }
            Ok(())
        }
    }

    impl Attributes for F7TableReader<'_, '_> {
        fn get(&mut self, base: u64, length: u64) -> Result<u64, Error> {
            F7TableReader::get(self, base, length)
        }

        fn set(&mut self, _: u64, _: u64, _: u64) -> Result<(), Error> {
            Err(Error::Unsupported)
        }

        fn clear(&mut self, _: u64, _: u64, _: u64) -> Result<(), Error> {
            Err(Error::Unsupported)
        }
    }

    impl Memory for F7TableReader<'_, '_> {
        fn read_entry(&mut self, address: u64) -> Result<u64, Error> {
            if validate_table_source(self.memory, address).is_err() {
                self.last_failure = Some(F7Failure::TableSource);
                return Err(Error::Unsupported);
            }
            // Each dereference rechecks owner/root/mode before the source load.
            if let Err(failure) = self.check_context() {
                self.last_failure = Some(failure);
                return Err(failure.error());
            }
            self.reads = self.reads.checked_add(1).ok_or(Error::OutOfResources)?;
            // SAFETY: The constructor's independent identity/residency premise
            // and retained map apply to this complete metadata-checked page.
            // Volatile prevents reuse; it does not supply access permission.
            Ok(unsafe { ptr::read_volatile(address as *const u64) })
        }

        fn begin_update(&mut self) -> Result<(), Error> {
            Err(Error::Unsupported)
        }

        fn write_entry(&mut self, _: u64, _: u64) -> Result<(), Error> {
            Err(Error::Unsupported)
        }

        fn allocate_table(&mut self) -> Result<u64, Error> {
            Err(Error::Unsupported)
        }

        fn commit_update(&mut self) -> Result<(), Error> {
            Err(Error::Unsupported)
        }

        fn abort_update(&mut self) {}
    }

    struct InterruptScope(u64);

    impl InterruptScope {
        unsafe fn enter() -> Self {
            let flags: u64;
            unsafe { asm!("pushfq; pop {}; cli", out(reg) flags) };
            Self(flags)
        }
    }

    impl Drop for InterruptScope {
        fn drop(&mut self) {
            unsafe { asm!("push {}; popfq", in(reg) self.0) };
        }
    }

    // Only fixed, capability-qualified architectural/target MSRs are read.
    unsafe fn observe() -> Result<CpuObservation, F7Failure> {
        let cs: u16;
        let flags: u64;
        unsafe {
            asm!("mov {:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
            asm!("pushfq; pop {}", out(reg) flags);
        }
        if cs & 3 != 0 || flags & ((1 << 8) | (1 << 10) | (1 << 14) | (1 << 17) | (1 << 18)) != 0 {
            return Err(F7Failure::PrivilegeOrFlags);
        }
        let cpuid = core::arch::x86_64::__cpuid_count;
        let basic = cpuid(0, 0);
        let extended = cpuid(0x8000_0000, 0);
        if basic.eax < 0x0b {
            return Err(F7Failure::BasicFeatures);
        }
        if extended.eax < 0x8000_0023 {
            return Err(F7Failure::ExtendedFeatures);
        }
        let features = cpuid(1, 0);
        let topology = cpuid(0x0b, 0);
        let ext_features = cpuid(0x8000_0001, 0);
        let encryption = cpuid(0x8000_001f, 0);
        let multi = cpuid(0x8000_0023, 0);
        let cpu = CpuCapabilities {
            max_basic: basic.eax,
            vendor: [basic.ebx, basic.edx, basic.ecx],
            signature: features.eax,
            leaf1_ecx: features.ecx,
            leaf1_edx: features.edx,
            max_extended: extended.eax,
            extended_edx: ext_features.edx,
            physical_bits: cpuid(0x8000_0008, 0).eax as u8,
            topology_ebx: topology.ebx,
            topology_ecx: topology.ecx,
            apic_id: topology.edx,
            encryption_eax: encryption.eax,
            encryption_ebx: encryption.ebx,
            multi_key: [multi.eax, multi.ebx, multi.ecx, multi.edx],
        };
        validate_capabilities_detailed(cpu)?;
        let mut state = CpuObservation { cpu, ..CpuObservation::default() };
        let low: u32;
        let high: u32;
        unsafe {
            asm!("mov {}, cr0", out(reg) state.cr0, options(nomem, nostack, preserves_flags));
            asm!("mov {}, cr3", out(reg) state.cr3, options(nomem, nostack, preserves_flags));
            asm!("mov {}, cr4", out(reg) state.cr4, options(nomem, nostack, preserves_flags));
            asm!("rdmsr", in("ecx") EFER, out("eax") low, out("edx") high,
                options(nomem, nostack, preserves_flags));
        }
        state.efer = u64::from(low) | (u64::from(high) << 32);
        let low: u32;
        let high: u32;
        unsafe {
            asm!("rdmsr", in("ecx") 0xc001_0010u32, out("eax") low, out("edx") high,
                options(nomem, nostack, preserves_flags));
        }
        state.sys_cfg = u64::from(low) | (u64::from(high) << 32);
        if cpu.encryption_eax & 2 != 0 {
            let low: u32;
            let high: u32;
            unsafe {
                asm!("rdmsr", in("ecx") SEV_STATUS, out("eax") low, out("edx") high,
                    options(nomem, nostack, preserves_flags));
            }
            state.sev_status = u64::from(low) | (u64::from(high) << 32);
        }
        Ok(state)
    }
}
