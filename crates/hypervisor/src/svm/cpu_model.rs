//! Admitted, synthetic AMD CPU model for two cores in one package.
//!
//! Normative field encodings: AMD PPR 57896 rev. 3.00 (2024-08-28),
//! Family 1Ah Model 44h B0, section 2.1.12, pp. 65-128; AMD APM vol. 2
//! rev. 3.44, sections 11.4/11.5 (extended state). The PPR supplies encodings,
//! not this virtual processor's identity, cache performance or capabilities.
//! Classic cache/TLB associativity uses AMD CPUID specification 25481 rev.
//! 2.34 (September 2010), pp. 22-25, Table 4; the newer product-specific PPR's
//! restricted cache values are not a definition of this compatibility model.
//! Undefined function numbers return zero, as specified in PPR 2.1.12.
//!
//! Admission consumes host observations and execution-owner commitments; it
//! does not probe hardware, allocate, or prove those commitments. CPUID
//! filtering does not prevent unadvertised instructions from executing.
//! APIC/x2APIC, SYSENTER/SYSCALL, PAT/MTRR, PMU/IBS, nested SVM, encryption,
//! CET, PKU, AVX-512 and all supervisor xstate remain unadvertised. Their
//! required architectural owners are not established by the current fixture.

use crate::arch::x86_64::xstate::XstateLayout;

pub const MAX_BASIC_LEAF: u32 = 0x0d;
pub const MAX_EXTENDED_LEAF: u32 = 0x8000_0021;
pub const VCPU_COUNT: u32 = 2;

pub const MAX_CACHE_SUBLEAVES: usize = 8;

const FPU: u32 = 1;
const TSC: u32 = 1 << 4;
const MSR: u32 = 1 << 5;
const PAE: u32 = 1 << 6;
const CX8: u32 = 1 << 8;
const CMOV: u32 = 1 << 15;
const CLFLUSH: u32 = 1 << 19;
const MMX: u32 = 1 << 23;
const FXSR: u32 = 1 << 24;
const SSE: u32 = 1 << 25;
const SSE2: u32 = 1 << 26;
const HTT: u32 = 1 << 28;
const XSAVE: u32 = 1 << 26;
const OSXSAVE: u32 = 1 << 27;
const AVX: u32 = 1 << 28;
const CR4_OSXSAVE: u64 = 1 << 18;
const NX: u32 = 1 << 20;
const RDTSCP: u32 = 1 << 27;
const LONG_MODE: u32 = 1 << 29;
const BASE_EDX: u32 = FPU | MSR | PAE | CX8 | CMOV | MMX | FXSR | SSE | SSE2;
// All these instructions use already-owned integer or legacy SIMD state.
const OPTIONAL_ECX: u32 = (1 << 0)
    | (1 << 1)
    | (1 << 9)
    | (1 << 13)
    | (1 << 19)
    | (1 << 20)
    | (1 << 22)
    | (1 << 23)
    | (1 << 25)
    | (1 << 30); // RDRAND: native CF reports availability, not guaranteed success.
const OPTIONAL_7_EBX: u32 = (1 << 3) | (1 << 8) | (1 << 9) | (1 << 18) | (1 << 19) | (1 << 29);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmdCpuModel {
    runtime: RuntimeCpuContract,
    basic_ecx: u32,
    basic_edx: u32,
    structured_ebx: u32,
    structured_ecx: u32,
    structured_edx: u32,
    extended_ecx: u32,
    extended_edx: u32,
    extended8_ebx: u32,
    caches: HostCacheEvidence,
    extended21_eax: u32,
    extended21_ebx: u32,
    extended21_available: bool,
}

impl AmdCpuModel {
    pub fn admit(
        host: HostCpuEvidence,
        runtime: RuntimeCpuContract,
    ) -> Result<Self, CpuModelError> {
        use CpuModelError::*;
        if host.vendor != *b"AuthenticAMD" {
            return Err(UnsupportedVendor);
        }
        if host.max_basic < 1 || host.max_extended < 0x8000_0008 {
            return Err(MissingHostLeaves);
        }
        if host.leaf1_edx & BASE_EDX != BASE_EDX || host.extended1_edx & LONG_MODE == 0 {
            return Err(MissingBaselineFeatures);
        }
        let physical = host.address_sizes as u8;
        let linear = (host.address_sizes >> 8) as u8;
        if !(32..=52).contains(&physical)
            || !(48..=57).contains(&linear)
            || runtime.physical_address_bits != physical
        {
            return Err(InvalidAddressWidth);
        }
        if host.leaf1_edx & CLFLUSH != 0 && host.clflush_bytes != 64 {
            return Err(InvalidClflushSize);
        }
        if runtime.xstate.uses_xsave() && (host.max_basic < 0x0d || host.leaf1_ecx & XSAVE == 0) {
            return Err(XstateNotSupported);
        }
        if runtime.xstate.mask() & 4 != 0 && host.leaf1_ecx & AVX == 0 {
            return Err(XstateNotSupported);
        }
        if (runtime.tsc && host.leaf1_edx & TSC == 0)
            || (runtime.rdtscp && (!runtime.tsc || host.extended1_edx & RDTSCP == 0))
        {
            return Err(ClockNotSupported);
        }
        if runtime.nx && host.extended1_edx & NX == 0 {
            return Err(NxNotSupported);
        }
        let topology_extensions =
            host.max_extended >= 0x8000_001d && host.extended1_ecx & (1 << 22) != 0;
        let caches = host.caches.validate_and_normalize(topology_extensions)?;
        if host.max_extended >= 0x8000_0021 && host.extended21_ebx & 0xff00_0000 != 0 {
            return Err(InvalidExtendedMetadata);
        }
        let mut basic_ecx = host.leaf1_ecx & OPTIONAL_ECX;
        let mut structured_ebx =
            if host.max_basic >= 7 { host.leaf7_ebx & OPTIONAL_7_EBX } else { 0 };
        let mut structured_ecx = 0;
        let structured_edx = if host.max_basic >= 7 {
            host.leaf7_edx & (1 << 4) // FSRM uses the native integer string engine.
        } else {
            0
        };
        // LAHF/SAHF, ABM, SSE4A, PREFETCH/W and TBM use existing state.
        let mut extended_ecx =
            host.extended1_ecx & ((1 << 0) | (1 << 5) | (1 << 6) | (1 << 8) | (1 << 21));
        let mut extended8_ebx = 0;
        if host.leaf1_edx & CLFLUSH != 0 {
            // Admission has checked the 64-byte line contract. None of these
            // bits promises persistence across external power loss.
            if host.max_basic >= 7 {
                structured_ebx |= host.leaf7_ebx & ((1 << 23) | (1 << 24));
            }
            extended8_ebx = host.extended8_ebx & 1; // CLZERO.
        }
        if host.max_basic >= 7 && runtime.rdtscp {
            // RDPID reads the same owned TSC_AUX as RDTSCP.
            structured_ecx |= host.leaf7_ecx & (1 << 22);
        }
        if runtime.xstate.uses_xsave() {
            basic_ecx |= XSAVE;
        }
        if runtime.xstate.mask() & 4 != 0 {
            basic_ecx |= AVX | (host.leaf1_ecx & ((1 << 12) | (1 << 29)));
            extended_ecx |= host.extended1_ecx & ((1 << 11) | (1 << 16)); // XOP/FMA4.
            if host.max_basic >= 7 {
                structured_ebx |= host.leaf7_ebx & (1 << 5);
                // GFNI/VAES/VPCLMUL use XMM/YMM state; this model does not
                // advertise the AVX-512 features needed for their EVEX forms.
                structured_ecx |= host.leaf7_ecx & ((1 << 8) | (1 << 9) | (1 << 10));
            }
        }
        let mut basic_edx = BASE_EDX | HTT | (host.leaf1_edx & CLFLUSH);
        if runtime.tsc {
            basic_edx |= TSC;
        }
        // AMD legacy duplicate bits occupy these positions in extended EDX.
        let mut extended_edx =
            (basic_edx & (FPU | TSC | MSR | PAE | CX8 | CMOV | MMX | FXSR)) | LONG_MODE;
        if runtime.rdtscp {
            extended_edx |= RDTSCP;
        }
        if runtime.nx {
            extended_edx |= NX;
        }
        extended_edx |= host.extended1_edx & (1 << 22); // MMX extensions.
        if host.extended1_edx & (1 << 31) != 0 {
            extended_edx |= host.extended1_edx & ((1 << 30) | (1 << 31));
        }
        Ok(Self {
            runtime,
            basic_ecx,
            basic_edx,
            structured_ebx,
            structured_ecx,
            structured_edx,
            // CmpLegacy=1: this is two separate cores, not two SMT threads.
            extended_ecx: extended_ecx | (1 << 1) | if topology_extensions { 1 << 22 } else { 0 },
            extended_edx,
            extended8_ebx,
            caches,
            // This is a cache-format dependency, not an instruction capability.
            extended21_eax: if host.max_extended >= 0x8000_0021 {
                host.extended21_eax & (1 << 14)
            } else {
                0
            },
            extended21_ebx: if host.max_extended >= 0x8000_0021 { host.extended21_ebx } else { 0 },
            extended21_available: host.max_extended >= 0x8000_0021,
        })
    }

    pub const fn physical_address_bits(&self) -> u8 {
        self.runtime.physical_address_bits
    }
    pub const fn xcr0_mask(&self) -> u64 {
        self.runtime.xstate.mask()
    }
    pub const fn uses_xsave(&self) -> bool {
        self.runtime.xstate.uses_xsave()
    }

    /// Pure CPUID result `[EAX, EBX, ECX, EDX]`; caller owns zero extension and
    /// validated instruction completion. Scalar leaves ignore ECX, while 7,
    /// B, D and 8000001D interpret it without wrapping or truncation.
    pub fn cpuid(
        &self,
        leaf: u32,
        subleaf: u32,
        state: GuestCpuState,
    ) -> Result<[u32; 4], CpuModelError> {
        if state.vcpu_id >= VCPU_COUNT {
            return Err(CpuModelError::InvalidVcpuId);
        }
        if self.uses_xsave() {
            if state.xcr0 & 1 == 0
                || state.xcr0 & !self.xcr0_mask() != 0
                || (state.xcr0 & 4 != 0 && state.xcr0 & 2 == 0)
            {
                return Err(CpuModelError::InvalidGuestXstate);
            }
        } else if state.cr4 & CR4_OSXSAVE != 0 {
            return Err(CpuModelError::InvalidGuestXstate);
        }
        Ok(match leaf {
            0 => self.runtime.identity.vendor_leaf(MAX_BASIC_LEAF),
            1 => [
                self.runtime.identity.signature(),
                (state.vcpu_id << 24)
                    | (VCPU_COUNT << 16)
                    | if self.basic_edx & CLFLUSH != 0 { 8 << 8 } else { 0 },
                self.basic_ecx
                    | if self.uses_xsave() && state.cr4 & CR4_OSXSAVE != 0 { OSXSAVE } else { 0 },
                self.basic_edx,
            ],
            // AMD does not use Intel descriptor/serial/cache leaves 2/3/4.
            // MONITOR, power management, PMU and reserved leaves are absent.
            2..=6 | 8..=10 | 12 => [0; 4],
            7 => {
                if subleaf == 0 {
                    [0, self.structured_ebx, self.structured_ecx, self.structured_edx]
                } else {
                    [0; 4]
                }
            }
            0x0b => match subleaf {
                0 => [0, 1, 1 << 8, state.vcpu_id],
                1 => [1, VCPU_COUNT, (2 << 8) | 1, state.vcpu_id],
                // Termination retains the architectural ECX level-number field.
                _ => [0, 0, subleaf & 0xff, state.vcpu_id],
            },
            0x0d => self.xstate_leaf(subleaf, state.xcr0),
            // Native-facing AMD identity has no private hypervisor namespace.
            0x4000_0000..=0x4fff_ffff => [0; 4],
            0x8000_0000 => self.runtime.identity.vendor_leaf(if self.extended21_available {
                MAX_EXTENDED_LEAF
            } else {
                0x8000_001e
            }),
            0x8000_0001 => [
                self.runtime.identity.extended_signature(),
                0,
                self.extended_ecx,
                self.extended_edx,
            ],
            0x8000_0002..=0x8000_0004 => self.runtime.identity.brand_leaf(leaf - 0x8000_0002),
            0x8000_0005 => self.caches.legacy_l1,
            0x8000_0006 => self.caches.legacy_l2_l3,
            0x8000_0007 => [0; 4], // No invariant TSC, RAS, or power interfaces.
            0x8000_0008 => [
                u32::from(self.physical_address_bits()) | (48 << 8),
                self.extended8_ebx,
                (1 << 12) | (VCPU_COUNT - 1),
                0,
            ],
            // Reserved functions and unsupported SVM, 1 GiB TLB, performance
            // hints, IBS and lightweight profiling. No side-channel/MSR claims.
            0x8000_0009..=0x8000_001c => [0; 4],
            0x8000_001d => {
                if subleaf < u32::from(self.caches.deterministic_count) {
                    self.caches.deterministic[subleaf as usize]
                } else {
                    [0; 4]
                }
            }
            // APIC capability is deliberately absent until its complete owner
            // is admitted. Reserved ExtendedApicId therefore reads zero; core
            // and node information still describe the two virtual CPUs.
            0x8000_001e => {
                if self.extended_ecx & (1 << 22) != 0 {
                    [0, state.vcpu_id, 0, 0]
                } else {
                    [0; 4]
                }
            }
            // Preserve the native L2 TLB size multiplier needed to decode leaf
            // 80000006 (PPR pp.94-95/117); do not import other behavior claims.
            0x8000_0021 => [self.extended21_eax, self.extended21_ebx, 0, 0],
            // Modern PPR facilities absent: SME/SEV (1F), QoS (20),
            // PMU (22), multi-key encryption (23), reserved 24/25, extended
            // topology (26). They are reviewed absent, not host-forwarded.
            0x8000_001f..=0x8000_0020 | 0x8000_0022..=0x8000_0026 => [0; 4],
            // This includes every other basic, hypervisor, extended, vendor,
            // out-of-range and future namespace, without Intel leaf aliasing.
            _ => [0; 4],
        })
    }

    fn xstate_leaf(&self, subleaf: u32, xcr0: u64) -> [u32; 4] {
        if !self.uses_xsave() {
            return [0; 4];
        }
        match subleaf {
            0 => [
                self.xcr0_mask() as u32,
                if xcr0 & 4 != 0 { self.runtime.xstate.size() as u32 } else { 576 },
                self.runtime.xstate.size() as u32,
                0,
            ],
            // XSAVEOPT/C/S and XGETBV ECX=1 are intentionally not advertised;
            // there is no supervisor state or compacted-layout contract.
            1 => [0; 4],
            2 => match self.runtime.xstate.avx_offset() {
                Some(offset) => [256, offset as u32, self.runtime.xstate.avx_flags(), 0],
                None => [0; 4],
            },
            _ => [0; 4],
        }
    }
}

/// Raw observations from every host CPU that can execute this virtual model.
/// Callers must admit an identical model on each CPU, or intersect observations
/// before admission. A feature bit is execution evidence, not a passthrough leaf.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostCpuEvidence {
    pub vendor: [u8; 12],
    pub max_basic: u32,
    pub max_extended: u32,
    pub leaf1_ecx: u32,
    pub leaf1_edx: u32,
    pub leaf7_ebx: u32,
    pub leaf7_ecx: u32,
    pub leaf7_edx: u32,
    pub extended1_ecx: u32,
    pub extended1_edx: u32,
    pub extended8_ebx: u32,
    /// Bit 14 describes the native legacy L2 TLB size encoding (times 32).
    /// Only collect this leaf when the host extended maximum admits it.
    pub extended21_eax: u32,
    pub extended21_ebx: u32,
    /// CPUID 80000008 EAX, including physical and linear address widths.
    pub address_sizes: u32,
    /// CPUID 1 EBX[15:8] multiplied by eight.
    pub clflush_bytes: u16,
    pub caches: HostCacheEvidence,
}

/// Native cache/TLB snapshot. `deterministic_count` excludes the terminating
/// null entry, which must occur within the eight-entry budget. Unused entries
/// are zero. Collect 1D only if host maximum and TopologyExtensions permit it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostCacheEvidence {
    pub legacy_l1: [u32; 4],
    pub legacy_l2_l3: [u32; 4],
    pub deterministic: [[u32; 4]; MAX_CACHE_SUBLEAVES],
    pub deterministic_count: u8,
}

impl HostCacheEvidence {
    fn validate_and_normalize(mut self, topology_extensions: bool) -> Result<Self, CpuModelError> {
        let count = usize::from(self.deterministic_count);
        if count >= MAX_CACHE_SUBLEAVES
            || (!topology_extensions && count != 0)
            || self.deterministic[count..].iter().any(|&leaf| leaf != [0; 4])
        {
            return Err(CpuModelError::InvalidCacheEvidence);
        }
        let has_legacy_cache = self.legacy_l1[2] >> 24 != 0
            || self.legacy_l1[3] >> 24 != 0
            || self.legacy_l2_l3[2] >> 16 != 0
            || self.legacy_l2_l3[3] >> 18 != 0;
        if topology_extensions && has_legacy_cache && count == 0 {
            return Err(CpuModelError::InvalidCacheEvidence);
        }
        // Modern AMD's associativity discriminator 9 delegates to function
        // 1D. It cannot survive if that referenced descriptor is unavailable.
        for (legacy, level) in [(self.legacy_l2_l3[2], 2), (self.legacy_l2_l3[3], 3)] {
            if (legacy >> 12) & 15 == 9
                && !self.deterministic[..count]
                    .iter()
                    .any(|leaf| leaf[0] & 31 == 3 && (leaf[0] >> 5) & 7 == level)
            {
                return Err(CpuModelError::InvalidCacheEvidence);
            }
        }
        for leaf in &mut self.deterministic[..count] {
            let kind = leaf[0] & 31;
            let level = (leaf[0] >> 5) & 7;
            if !(1..=3).contains(&kind)
                || !(1..=3).contains(&level)
                || leaf[0] & 0xfc00_3c00 != 0
                || leaf[3] & !3 != 0
            {
                return Err(CpuModelError::InvalidCacheEvidence);
            }
            // The selected topology has two separate cores with private L1/L2.
            // Preserve native geometry/policy; project L3 package sharing onto
            // the two guest CPUs rather than forwarding a host SMT/core count.
            let native_sharers = ((leaf[0] >> 14) & 0xfff) + 1;
            let sharers = if level < 3 { 1 } else { native_sharers.min(VCPU_COUNT) };
            leaf[0] = (leaf[0] & !(0xfff << 14)) | ((sharers - 1) << 14);
        }
        Ok(self)
    }
}

/// Commitments supplied by the actual runtime, not capabilities inferred from
/// CPUID. `xstate` requires eager per-vCPU preservation and, when XSAVE is
/// used, validated XSETBV plus separate host/guest XCR0 switching. `tsc` and
/// `rdtscp` require the clock owner and TSC/TSC_AUX MSR policy. `nx` requires
/// checked EFER.NXE and guest/NPT page-walk semantics. This model has fixed
/// 48-bit linear addresses; it never advertises LA57. The physical width must
/// equal hardware MAXPHYADDR: limiting backed RAM does not change hardware
/// page-walk reserved-bit faults and cannot justify advertising a smaller width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeCpuContract {
    pub identity: CpuIdentity,
    pub xstate: XstateLayout,
    pub physical_address_bits: u8,
    pub tsc: bool,
    pub rdtscp: bool,
    pub nx: bool,
}

/// Authoritative stopped vCPU state. Identity is a virtual ID, never a host ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestCpuState {
    pub vcpu_id: u32,
    pub cr4: u64,
    pub xcr0: u64,
}

/// Validated native identity, kept separate from feature execution admission.
/// The runtime captures these leaves from its executing processor. Emulated
/// host identity remains emulator evidence, never a physical identity claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuIdentity {
    vendor: [u8; 12],
    signature: u32,
    extended_signature: u32,
    brand: [u8; 48],
}

impl CpuIdentity {
    /// Caller reads maxima first and only collects optional identity leaves
    /// when enumerated. Missing evidence remains unavailable, not a zero leaf.
    /// All arrays are EAX/EBX/ECX/EDX. No hardware reads or guest writes occur.
    pub fn from_leaves(
        basic_vendor: [u32; 4],
        basic_signature: Option<[u32; 4]>,
        extended_vendor: [u32; 4],
        extended_signature: Option<[u32; 4]>,
        brand: Option<[[u32; 4]; 3]>,
    ) -> Result<Self, CpuIdentityError> {
        use CpuIdentityError::*;
        if !(1..=0x3fff_ffff).contains(&basic_vendor[0])
            || !(0x8000_0004..=0xbfff_ffff).contains(&extended_vendor[0])
        {
            return Err(InvalidMaxima);
        }
        let vendor = decode_vendor(basic_vendor);
        if vendor != *b"AuthenticAMD" {
            return Err(UnsupportedVendor);
        }
        if decode_vendor(extended_vendor) != vendor {
            return Err(InconsistentVendor);
        }
        let signature = basic_signature.ok_or(MissingIdentityLeaves)?[0];
        let extended_signature = extended_signature.ok_or(MissingIdentityLeaves)?[0];
        // AMD reserved EAX fields are 31:28 and 15:12 (PPR p.67).
        if signature & 0xf000_f000 != 0 || signature & 0xf00 == 0 {
            return Err(InvalidSignature);
        }
        if extended_signature != signature {
            return Err(InconsistentSignature);
        }
        let leaves = brand.ok_or(MissingIdentityLeaves)?;
        let mut bytes = [0; 48];
        // Both iterators contain exactly 48 bytes. Avoid dynamic slice bounds:
        // the native size-optimized no-panic image must retain no panic edge.
        for (destination, source) in
            bytes.iter_mut().zip(leaves.iter().flatten().flat_map(|value| value.to_le_bytes()))
        {
            *destination = source;
        }
        // Preserve native bytes verbatim, including full-width names and
        // firmware-selected padding; identity capture is not string rewriting.
        Ok(Self { vendor, signature, extended_signature, brand: bytes })
    }

    pub const fn vendor(&self) -> [u8; 12] {
        self.vendor
    }
    pub const fn signature(&self) -> u32 {
        self.signature
    }
    pub const fn extended_signature(&self) -> u32 {
        self.extended_signature
    }
    pub const fn brand(&self) -> [u8; 48] {
        self.brand
    }

    pub const fn family_model_stepping(&self) -> (u16, u16, u8) {
        let base_family = ((self.signature >> 8) & 15) as u16;
        let base_model = ((self.signature >> 4) & 15) as u16;
        let family =
            base_family + if base_family == 15 { ((self.signature >> 20) & 255) as u16 } else { 0 };
        let model = base_model
            | if base_family == 6 || base_family == 15 {
                ((self.signature >> 12) & 0xf0) as u16
            } else {
                0
            };
        (family, model, (self.signature & 15) as u8)
    }

    fn vendor_leaf(&self, maximum: u32) -> [u32; 4] {
        [
            maximum,
            u32::from_le_bytes(self.vendor[..4].try_into().unwrap()),
            u32::from_le_bytes(self.vendor[8..].try_into().unwrap()),
            u32::from_le_bytes(self.vendor[4..8].try_into().unwrap()),
        ]
    }

    fn brand_leaf(&self, index: u32) -> [u32; 4] {
        let mut result = [0; 4];
        for (register, value) in result.iter_mut().enumerate() {
            let offset = index as usize * 16 + register * 4;
            *value = u32::from_le_bytes(self.brand[offset..offset + 4].try_into().unwrap());
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuModelError {
    UnsupportedVendor,
    MissingHostLeaves,
    MissingBaselineFeatures,
    InvalidAddressWidth,
    InvalidClflushSize,
    XstateNotSupported,
    ClockNotSupported,
    NxNotSupported,
    InvalidVcpuId,
    InvalidGuestXstate,
    InvalidCacheEvidence,
    InvalidExtendedMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuIdentityError {
    InvalidMaxima,
    MissingIdentityLeaves,
    UnsupportedVendor,
    InconsistentVendor,
    InvalidSignature,
    InconsistentSignature,
}

/// Trusted native first-boot CPUID filter, separate from the synthetic model.
/// The runtime samples the requested leaf/subleaf on this same physical CPU
/// with guest XCR0 live. Native topology is retained: the caller must admit
/// every exposed CPU (the first executable fixture admits only one).
/// APM2 rev3.44 15.4/15.27: nested SVM/SKINIT are unavailable; no encrypted
/// guest/platform contract exists. Active host encryption or another owning
/// hypervisor must be refused at admission, not concealed by this filter.
/// Other native features remain execution capabilities, not Windows proof;
/// currently unsupported EFER feature writes stop at the EFER owner.
pub fn native_boot_cpuid(leaf: u32, mut native: [u32; 4], guest_cr4: u64) -> [u32; 4] {
    match leaf {
        1 => {
            // OSXSAVE reports the guest's CR4, not the stopped host CR4.
            native[2] = (native[2] & !(1 << 27))
                | if native[2] & (1 << 26) != 0 && guest_cr4 & (1 << 18) != 0 {
                    1 << 27
                } else {
                    0
                };
        }
        // ExtApicSpace (bit3) is absent from the native guest LAPIC model.
        // Its physical extension controls remain exclusively host-owned.
        0x8000_0001 => native[2] &= !((1 << 2) | (1 << 3) | (1 << 12)),
        0x8000_000a | 0x8000_001f | 0x8000_0023 => return [0; 4],
        _ => {}
    }
    native
}

fn decode_vendor(leaf: [u32; 4]) -> [u8; 12] {
    let mut vendor = [0; 12];
    vendor[..4].copy_from_slice(&leaf[1].to_le_bytes());
    vendor[4..8].copy_from_slice(&leaf[3].to_le_bytes());
    vendor[8..].copy_from_slice(&leaf[2].to_le_bytes());
    vendor
}
