//! Cache-control and platform MSR indices shared by DXE and the resident runtime.
//!
//! Architectural MTRR/PAT registers follow AMD APM vol. 2 rev. 3.44 sections
//! 7.7-7.8. AMD model-specific fields are named after PPR 57896 rev. 3.00,
//! Family 1Ah Model 44h B0 (printed page = PDF page = index + 1): MTRRcap
//! p123, MtrrVarBase p126, MtrrFix pp127-167, PAT p171, MTRRdefType p173,
//! SYS_CFG p202, HWCR p203, IORR/TOP_MEM p205, TOM2 p206 and
//! MmioCfgBaseAddr p210. Other processor profiles need their own review.

/// CPUID Fn0000_0001 EAX of the only reviewed native processor profile.
pub const TARGET_SIGNATURE: u32 = 0x00b4_0f40;
/// CPUID Fn8000_0008 EAX[7:0] of that profile.
pub const TARGET_PHYSICAL_BITS: u8 = 48;

pub const MTRR_CAP: u32 = 0xfe;
/// MtrrVarBase n is `MTRR_VAR_BASE0 + 2n`; its MtrrVarMask follows it.
pub const MTRR_VAR_BASE0: u32 = 0x200;
pub const MTRR_FIX_64K: u32 = 0x250;
pub const MTRR_FIX_16K_0: u32 = 0x258;
pub const MTRR_FIX_16K_1: u32 = 0x259;
/// MtrrFix_4K_n is `MTRR_FIX_4K_0 + n` for n in 0..8.
pub const MTRR_FIX_4K_0: u32 = 0x268;
/// Every fixed-range MTRR, in address order.
pub const MTRR_FIXED: [u32; 11] = [
    MTRR_FIX_64K, MTRR_FIX_16K_0, MTRR_FIX_16K_1, MTRR_FIX_4K_0, MTRR_FIX_4K_0 + 1,
    MTRR_FIX_4K_0 + 2, MTRR_FIX_4K_0 + 3, MTRR_FIX_4K_0 + 4, MTRR_FIX_4K_0 + 5,
    MTRR_FIX_4K_0 + 6, MTRR_FIX_4K_0 + 7,
];
pub const PAT: u32 = 0x277;
pub const MTRR_DEF_TYPE: u32 = 0x2ff;

pub const SYS_CFG: u32 = 0xc001_0010;
pub const HWCR: u32 = 0xc001_0015;
/// IORR_BASE n is `IORR_BASE0 + 2n` for n in 0..2; its IORR_MASK follows it.
pub const IORR_BASE0: u32 = 0xc001_0016;
pub const TOP_MEM: u32 = 0xc001_001a;
pub const TOM2: u32 = 0xc001_001d;
pub const MMIO_CFG_BASE_ADDR: u32 = 0xc001_0058;

/// SYS_CFG bits 26:18; bits 63:27 and 17:0 are reserved.
pub const SYS_CFG_DEFINED: u64 = 0x07fc_0000;
/// SYS_CFG HMKEE, VmplEn, SecureNestedPagingEn and SMEE (bits 26:23).
pub const SYS_CFG_ENCRYPTION: u64 = 0x0780_0000;
/// Enables the fixed-MTRR RdDram/WrDram attributes. Core-shared.
pub const SYS_CFG_MTRR_FIX_DRAM_EN: u64 = 1 << 18;
/// Makes the fixed-MTRR RdDram/WrDram bits read-write. Not shared between threads.
pub const SYS_CFG_MTRR_FIX_DRAM_MOD_EN: u64 = 1 << 19;
pub const SYS_CFG_MTRR_TOM2_EN: u64 = 1 << 21;
/// Memory in [4GiB, TOM2) defaults to WB instead of MTRRdefType's type.
pub const SYS_CFG_TOM2_FORCE_MEM_TYPE_WB: u64 = 1 << 22;

pub const HWCR_IO_CFG_GP_FAULT: u64 = 1 << 20;
pub const HWCR_IRPERF_EN: u64 = 1 << 30;
/// CPUID outside SMM at CPL > 0 raises #GP.
pub const HWCR_CPUID_FLT_EN: u64 = 1 << 35;
