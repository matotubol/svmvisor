//! Pure, hardware-access-free model for the record-only per-processor
//! system-register slice (schema v6).
//!
//! This module never executes `RDMSR`. Callers supply values already read by
//! the firmware adapter from the reviewed allowlist in
//! `docs/m0b-msr-read-policy.md`; every decode is checked against AMD APM
//! Volume 2 rev. 3.44 and AMD PPR 57896 rev. 3.00 (Family 1Ah Model 44h B0).

use crate::cpuid::{CpuInventory, should_read_vm_cr};

pub const MTRR_CAP_MSR: u32 = 0x0000_00fe;
pub const MTRR_DEF_TYPE_MSR: u32 = 0x0000_02ff;
pub const PAT_MSR: u32 = 0x0000_0277;
pub const MTRR_PHYS_BASE_0_MSR: u32 = 0x0000_0200;
pub const MTRR_FIX_64K_00000_MSR: u32 = 0x0000_0250;
pub const MTRR_FIX_16K_80000_MSR: u32 = 0x0000_0258;
pub const MTRR_FIX_16K_A0000_MSR: u32 = 0x0000_0259;
pub const MTRR_FIX_4K_00000_MSR: u32 = 0x0000_0268;
pub const SYS_CFG_MSR: u32 = 0xc001_0010;
pub const HWCR_MSR: u32 = 0xc001_0015;
pub const IORR_BASE_0_MSR: u32 = 0xc001_0016;
pub const IORR_MASK_0_MSR: u32 = 0xc001_0017;
pub const IORR_BASE_1_MSR: u32 = 0xc001_0018;
pub const IORR_MASK_1_MSR: u32 = 0xc001_0019;
pub const TOP_MEM_MSR: u32 = 0xc001_001a;
pub const TOM2_MSR: u32 = 0xc001_001d;
pub const SMM_BASE_MSR: u32 = 0xc001_0111;
pub const SMM_ADDR_MSR: u32 = 0xc001_0112;
pub const SMM_MASK_MSR: u32 = 0xc001_0113;

pub const MAX_VARIABLE_MTRR_PAIRS: usize = 8;
pub const FIXED_MTRR_COUNT: usize = 11;
pub const IORR_RANGE_COUNT: usize = 2;

/// Variable MTRR pair count read per processor when `VCNT` is valid.
pub const PPR_DOCUMENTED_FAMILY: u16 = 26;
pub const PPR_DOCUMENTED_MODEL: u16 = 68;

/// Number of always-read sites in one observed measurement: `MTRRcap`,
/// `MTRRdefType`, PAT, SYS_CFG, HWCR, TOP_MEM, TOM2, SMM_BASE, SMMAddr,
/// SMMMask. VM_CR is counted separately by the caller.
pub const BASE_SITE_COUNT: u64 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VariableMtrrPair {
    pub base: u64,
    pub mask: u64,
}

/// One processor's allowlisted register values. Pairs and fixed registers
/// beyond the same-run `MTRRcap` gates are zero and flagged by their
/// `*_observed`/reason fields, so equality is exact and total.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemRegisterInventory {
    pub mtrr_cap: u64,
    pub mtrr_def_type: u64,
    pub pat: u64,
    pub variable_mtrr_pairs: [VariableMtrrPair; MAX_VARIABLE_MTRR_PAIRS],
    pub variable_mtrr_pair_count: u8,
    pub fixed_mtrr: [u64; FIXED_MTRR_COUNT],
    pub fixed_mtrr_observed: bool,
    pub sys_cfg: u64,
    pub hwcr: u64,
    pub top_mem: u64,
    pub tom2: u64,
    pub smm_base: u64,
    pub smm_addr: u64,
    pub smm_mask: u64,
    pub iorr_base: [u64; IORR_RANGE_COUNT],
    pub iorr_mask: [u64; IORR_RANGE_COUNT],
    pub iorr_not_attempted_reason: Option<&'static str>,
}

/// Only the reviewed allowlisted MSRs are readable in this slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemRegistersEvidence<'a> {
    Observed(SystemRegisterInventory),
    NotAttempted { reason: &'a str },
}

impl SystemRegisterInventory {
    /// Compare the run-wide register state while permitting the PPR-defined
    /// thread-scoped `SMM_BASE` value to differ between processors.
    ///
    /// Every other field, including thread-scoped `HWCR`, remains part of the
    /// exact conservative comparison. This does not weaken the ordinary
    /// `PartialEq` implementation, which remains byte-for-byte exact.
    #[must_use]
    pub fn matches_cross_processor_policy(&self, reference: &Self) -> bool {
        let mut comparable = *self;
        comparable.smm_base = reference.smm_base;
        comparable == *reference
    }
}

impl SystemRegistersEvidence<'_> {
    /// Compare status and reasons exactly, or observed inventories according
    /// to the schema-v6 cross-processor policy.
    #[must_use]
    pub fn matches_cross_processor_policy(&self, reference: &Self) -> bool {
        match (self, reference) {
            (Self::Observed(inventory), Self::Observed(reference)) => {
                inventory.matches_cross_processor_policy(reference)
            }
            (Self::NotAttempted { reason }, Self::NotAttempted { reason: reference }) => {
                reason == reference
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SysCfgDecode {
    pub mtrr_fix_dram_en: bool,
    pub mtrr_fix_dram_mod_en: bool,
    pub mtrr_var_dram_en: bool,
    pub mtrr_tom2_en: bool,
    pub tom2_force_mem_type_wb: bool,
    pub smee: bool,
    pub secure_nested_paging_en: bool,
    pub vmpl_en: bool,
    pub hmkee: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HwcrDecode {
    pub smm_lock: bool,
    pub smm_pg_cfg_lock: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MtrrCapDecode {
    pub vcnt: u8,
    pub fix: bool,
    pub wc: bool,
    pub smrr: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MtrrDefTypeDecode {
    pub mem_type: u8,
    pub fixed_range_enable: bool,
    pub mtrr_def_type_en: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IorrBaseDecode {
    pub phys_base: u64,
    pub rd_mem: bool,
    pub wr_mem: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IorrMaskDecode {
    pub phys_mask: u64,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmmMaskDecode {
    pub a_valid: bool,
    pub t_valid: bool,
    pub a_close: bool,
    pub t_close: bool,
    pub am_type_io_wc: bool,
    pub tm_type_io_wc: bool,
    pub am_type_dram: u8,
    pub tm_type_dram: u8,
    pub tseg_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemRegisterError {
    /// `MTRRcap[VCNT]` exceeds the reviewed site bound; the capture fails
    /// closed rather than reading beyond the allowlisted site set.
    VariableMtrrCountExceedsBound(u8),
    /// `MTRRcap` was gated off but variable or fixed MTRR values were recorded.
    GatedValuesPresentWithoutCapability,
    /// PPR MSRC001_0111 requires `SmmBase[3:0]` to be zero.
    SmmBaseLowNibbleNonzero(u8),
}

/// The whole slice shares the reviewed VM_CR enumeration predicate: AMD,
/// SVM enumerated, and the SVM capability leaf present.
#[must_use]
pub fn should_read_system_registers(cpu: &CpuInventory) -> bool {
    should_read_vm_cr(cpu)
}

/// The IORR quartet is read only on the CPU the pinned PPR documents.
#[must_use]
pub fn iorr_documented_by_pinned_ppr(cpu: &CpuInventory) -> bool {
    matches!(
        cpu.family_model_stepping(),
        Some((PPR_DOCUMENTED_FAMILY, PPR_DOCUMENTED_MODEL, _))
    )
}

/// `MTRRcap` fields; `VCNT` beyond the reviewed bound fails the capture.
pub fn decode_mtrr_cap(raw: u64) -> Result<MtrrCapDecode, SystemRegisterError> {
    let decode = MtrrCapDecode {
        vcnt: (raw & 0xff) as u8,
        fix: raw & (1 << 8) != 0,
        wc: raw & (1 << 10) != 0,
        smrr: raw & (1 << 11) != 0,
    };
    if usize::from(decode.vcnt) > MAX_VARIABLE_MTRR_PAIRS {
        return Err(SystemRegisterError::VariableMtrrCountExceedsBound(decode.vcnt));
    }
    Ok(decode)
}

#[must_use]
pub fn decode_mtrr_def_type(raw: u64) -> MtrrDefTypeDecode {
    MtrrDefTypeDecode {
        mem_type: (raw & 0xff) as u8,
        fixed_range_enable: raw & (1 << 10) != 0,
        mtrr_def_type_en: raw & (1 << 11) != 0,
    }
}

/// PPR `MSRC001_0010` field decode. The encryption-control bits are decoded
/// from the same raw value but claim no inherited encryption state.
#[must_use]
pub fn decode_sys_cfg(raw: u64) -> SysCfgDecode {
    SysCfgDecode {
        mtrr_fix_dram_en: raw & (1 << 18) != 0,
        mtrr_fix_dram_mod_en: raw & (1 << 19) != 0,
        mtrr_var_dram_en: raw & (1 << 20) != 0,
        mtrr_tom2_en: raw & (1 << 21) != 0,
        tom2_force_mem_type_wb: raw & (1 << 22) != 0,
        smee: raw & (1 << 23) != 0,
        secure_nested_paging_en: raw & (1 << 24) != 0,
        vmpl_en: raw & (1 << 25) != 0,
        hmkee: raw & (1 << 26) != 0,
    }
}

/// PPR `MSRC001_0015` SMM fields; APM §15.32.1 places `SmmLock` at bit 0.
#[must_use]
pub fn decode_hwcr(raw: u64) -> HwcrDecode {
    HwcrDecode {
        smm_lock: raw & 1 != 0,
        smm_pg_cfg_lock: raw & (1 << 33) != 0,
    }
}

/// PPR `MSRC001_001A`: `TOM[47:23]` divides DRAM below from MMIO above.
#[must_use]
pub fn top_mem_address(raw: u64) -> u64 {
    raw & 0x0000_ffff_ff80_0000
}

/// PPR `MSRC001_001D`: `TOM2[47:23]`, enabled by `SYS_CFG[MtrrTom2En]`.
#[must_use]
pub fn tom2_address(raw: u64) -> u64 {
    raw & 0x0000_ffff_ff80_0000
}

/// PPR `MSRC001_0111`: base of the SMM memory region.
#[must_use]
pub fn smm_base_address(raw: u64) -> u64 {
    raw & 0xffff_ffff
}

/// PPR `MSRC001_0112`: `TSegBase[47:17]`.
#[must_use]
pub fn smm_tseg_base(raw: u64) -> u64 {
    raw & 0x0000_ffff_fffe_0000
}

/// PPR `MSRC001_0113` field decode.
#[must_use]
pub fn decode_smm_mask(raw: u64) -> SmmMaskDecode {
    SmmMaskDecode {
        a_valid: raw & 1 != 0,
        t_valid: raw & (1 << 1) != 0,
        a_close: raw & (1 << 2) != 0,
        t_close: raw & (1 << 3) != 0,
        am_type_io_wc: raw & (1 << 4) != 0,
        tm_type_io_wc: raw & (1 << 5) != 0,
        am_type_dram: ((raw >> 8) & 0x7) as u8,
        tm_type_dram: ((raw >> 12) & 0x7) as u8,
        tseg_mask: raw & 0x0000_ffff_fffe_0000,
    }
}

/// PPR `MSRC001_001[6...8]`: `PhyBase[47:12]`, `RdMem` (4), `WrMem` (3).
#[must_use]
pub fn decode_iorr_base(raw: u64) -> IorrBaseDecode {
    IorrBaseDecode {
        phys_base: raw & 0x0000_ffff_ffff_f000,
        rd_mem: raw & (1 << 4) != 0,
        wr_mem: raw & (1 << 3) != 0,
    }
}

/// PPR `MSRC001_001[7...9]`: `PhyMask[47:12]`, `Valid` (11).
#[must_use]
pub fn decode_iorr_mask(raw: u64) -> IorrMaskDecode {
    IorrMaskDecode {
        phys_mask: raw & 0x0000_ffff_ffff_f000,
        valid: raw & (1 << 11) != 0,
    }
}

/// APM §7: variable MTRR base `Type[7:0]`, `PhysBase[47:12]`.
#[must_use]
pub fn decode_variable_mtrr_base(raw: u64) -> (u8, u64) {
    ((raw & 0xff) as u8, raw & 0x0000_ffff_ffff_f000)
}

/// APM §7: variable MTRR mask `V` (11), `PhysMask[47:12]`.
#[must_use]
pub fn decode_variable_mtrr_mask(raw: u64) -> (bool, u64) {
    (raw & (1 << 11) != 0, raw & 0x0000_ffff_ffff_f000)
}

/// Expected `RDMSR` site count for one observed measurement, including the
/// pre-existing VM_CR site. The access counters must equal this total.
#[must_use]
pub fn expected_read_operations(inventory: &SystemRegisterInventory) -> u64 {
    let variable = u64::from(inventory.variable_mtrr_pair_count) * 2;
    let fixed = if inventory.fixed_mtrr_observed {
        FIXED_MTRR_COUNT as u64
    } else {
        0
    };
    let iorr = if inventory.iorr_not_attempted_reason.is_none() {
        2 * IORR_RANGE_COUNT as u64
    } else {
        0
    };
    // One existing VM_CR site plus the allowlisted new sites.
    1 + BASE_SITE_COUNT + variable + fixed + iorr
}

/// Validate the internal shape of one observed inventory against its gates.
pub fn validate_inventory(inventory: &SystemRegisterInventory) -> Result<(), SystemRegisterError> {
    if inventory.smm_base & 0xf != 0 {
        return Err(SystemRegisterError::SmmBaseLowNibbleNonzero(
            (inventory.smm_base & 0xf) as u8,
        ));
    }
    let cap = decode_mtrr_cap(inventory.mtrr_cap)?;
    if inventory.variable_mtrr_pair_count != cap.vcnt {
        return Err(SystemRegisterError::GatedValuesPresentWithoutCapability);
    }
    for (index, pair) in inventory.variable_mtrr_pairs.iter().enumerate() {
        let beyond = index >= usize::from(cap.vcnt);
        if beyond && *pair != (VariableMtrrPair { base: 0, mask: 0 }) {
            return Err(SystemRegisterError::GatedValuesPresentWithoutCapability);
        }
    }
    if !inventory.fixed_mtrr_observed && inventory.fixed_mtrr != [0; FIXED_MTRR_COUNT] {
        return Err(SystemRegisterError::GatedValuesPresentWithoutCapability);
    }
    if cap.fix != inventory.fixed_mtrr_observed {
        return Err(SystemRegisterError::GatedValuesPresentWithoutCapability);
    }
    if inventory.iorr_not_attempted_reason.is_some()
        && (inventory.iorr_base != [0; IORR_RANGE_COUNT]
            || inventory.iorr_mask != [0; IORR_RANGE_COUNT])
    {
        return Err(SystemRegisterError::GatedValuesPresentWithoutCapability);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::CpuidRegisters;

    fn amd_cpu(family: u16, model: u16) -> CpuInventory {
        let base_family = if family >= 0x0f { 0x0f } else { family };
        let extended_family = family - base_family;
        let base_model = model & 0x0f;
        let extended_model = model >> 4;
        CpuInventory {
            leaf_0000_0000: CpuidRegisters {
                eax: 7,
                ebx: u32::from_le_bytes(*b"Auth"),
                edx: u32::from_le_bytes(*b"enti"),
                ecx: u32::from_le_bytes(*b"cAMD"),
            },
            leaf_0000_0001: Some(CpuidRegisters {
                eax: (u32::from(extended_family) << 20)
                    | (u32::from(extended_model) << 16)
                    | (u32::from(base_family) << 8)
                    | (u32::from(base_model) << 4),
                ..Default::default()
            }),
            leaf_0000_0007_subleaf_0: Some(CpuidRegisters::default()),
            leaf_8000_0000: CpuidRegisters {
                eax: 0x8000_001e,
                ..Default::default()
            },
            leaf_8000_0001: Some(CpuidRegisters {
                ecx: (1 << 2) | (1 << 22),
                ..Default::default()
            }),
            leaf_8000_0002_to_0004: Some([CpuidRegisters::default(); 3]),
            leaf_8000_0008: Some(CpuidRegisters {
                eax: 52,
                ..Default::default()
            }),
            leaf_8000_000a: Some(CpuidRegisters::default()),
            leaf_8000_001e: Some(CpuidRegisters::default()),
            leaf_8000_001f: None,
            brand: Some([0; 48]),
        }
    }

    fn inventory_for_comparison(smm_base: u64, hwcr: u64) -> SystemRegisterInventory {
        SystemRegisterInventory {
            mtrr_cap: 0,
            mtrr_def_type: 0,
            pat: 0,
            variable_mtrr_pairs: [VariableMtrrPair { base: 0, mask: 0 }; MAX_VARIABLE_MTRR_PAIRS],
            variable_mtrr_pair_count: 0,
            fixed_mtrr: [0; FIXED_MTRR_COUNT],
            fixed_mtrr_observed: false,
            sys_cfg: 0,
            hwcr,
            top_mem: 0,
            tom2: 0,
            smm_base,
            smm_addr: 0,
            smm_mask: 0,
            iorr_base: [0; IORR_RANGE_COUNT],
            iorr_mask: [0; IORR_RANGE_COUNT],
            iorr_not_attempted_reason: Some("cpu-family-model-not-documented-by-pinned-ppr"),
        }
    }

    #[test]
    fn gates_follow_vm_cr_predicate_and_pinned_ppr_target() {
        let target = amd_cpu(26, 68);
        assert!(should_read_system_registers(&target));
        assert!(iorr_documented_by_pinned_ppr(&target));
        let other_model = amd_cpu(26, 0x24);
        assert!(!iorr_documented_by_pinned_ppr(&other_model));
        let other_family = amd_cpu(25, 68);
        assert!(!iorr_documented_by_pinned_ppr(&other_family));
        let mut non_amd = target;
        non_amd.leaf_0000_0000.ecx = u32::from_le_bytes(*b"XXXX");
        assert!(!should_read_system_registers(&non_amd));
    }

    #[test]
    fn mtrr_cap_bounds_the_reviewed_site_set() {
        let decode = decode_mtrr_cap(0x508).unwrap();
        assert_eq!((decode.vcnt, decode.fix, decode.wc, decode.smrr), (8, true, true, false));
        assert_eq!(
            decode_mtrr_cap(0x509),
            Err(SystemRegisterError::VariableMtrrCountExceedsBound(9))
        );
    }

    #[test]
    fn sys_cfg_and_hwcr_decode_named_bits() {
        let sys_cfg = decode_sys_cfg(
            (1 << 18) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23) | (1 << 24),
        );
        assert!(sys_cfg.mtrr_fix_dram_en && !sys_cfg.mtrr_fix_dram_mod_en);
        assert!(sys_cfg.mtrr_var_dram_en && sys_cfg.mtrr_tom2_en && sys_cfg.tom2_force_mem_type_wb);
        assert!(sys_cfg.smee && sys_cfg.secure_nested_paging_en);
        assert!(!sys_cfg.vmpl_en && !sys_cfg.hmkee);
        let hwcr = decode_hwcr(1 | (1 << 33));
        assert!(hwcr.smm_lock && hwcr.smm_pg_cfg_lock);
    }

    #[test]
    fn address_fields_use_ppr_bit_masks() {
        assert_eq!(top_mem_address(0xffff_ffff_ffff_ffff), 0x0000_ffff_ff80_0000);
        assert_eq!(tom2_address(0x0000_0001_4000_0000), 0x0000_0001_4000_0000);
        assert_eq!(smm_base_address(0x1_0003_0000), 0x0003_0000);
        assert_eq!(smm_tseg_base(0xffff_ffff_ffff_ffff), 0x0000_ffff_fffe_0000);
    }

    #[test]
    fn inventory_rejects_nonzero_smm_base_low_nibble() {
        let mut inventory = inventory_for_comparison(0x0000_0000_0003_000f, 1);
        assert_eq!(
            validate_inventory(&inventory),
            Err(SystemRegisterError::SmmBaseLowNibbleNonzero(0x0f))
        );

        inventory.smm_base &= !0xf;
        assert_eq!(validate_inventory(&inventory), Ok(()));
    }

    #[test]
    fn smm_mask_decodes_ppr_layout() {
        // PPR MSRC001_0113 assigns TMTypeDram to bits 14:12. Set the
        // reserved 16:15 field as a regression guard against the old decode.
        let raw =
            (1 << 0) | (1 << 1) | (5 << 8) | (2 << 12) | (3 << 15) | 0x0000_ffff_fffc_0000;
        let decode = decode_smm_mask(raw);
        assert!(decode.a_valid && decode.t_valid);
        assert!(!decode.a_close && !decode.t_close);
        assert!(!decode.am_type_io_wc && !decode.tm_type_io_wc);
        assert_eq!((decode.am_type_dram, decode.tm_type_dram), (5, 2));
        assert_eq!(decode.tseg_mask, 0x0000_ffff_fffc_0000);
    }

    #[test]
    fn cross_processor_policy_excludes_only_smm_base() {
        let reference = inventory_for_comparison(0x0000_0000_0003_0000, 1);
        let different_smm_base = inventory_for_comparison(0x0000_0000_0005_0000, 1);
        assert!(different_smm_base.matches_cross_processor_policy(&reference));
        assert_ne!(different_smm_base, reference);

        let different_hwcr = inventory_for_comparison(0x0000_0000_0005_0000, 0);
        assert!(!different_hwcr.matches_cross_processor_policy(&reference));
    }

    #[test]
    fn cross_processor_policy_preserves_status_and_reason() {
        let reason = SystemRegistersEvidence::NotAttempted { reason: "same" };
        let same_reason = SystemRegistersEvidence::NotAttempted { reason: "same" };
        let other_reason = SystemRegistersEvidence::NotAttempted { reason: "other" };
        let observed = SystemRegistersEvidence::Observed(inventory_for_comparison(0, 0));

        assert!(reason.matches_cross_processor_policy(&same_reason));
        assert!(!reason.matches_cross_processor_policy(&other_reason));
        assert!(!reason.matches_cross_processor_policy(&observed));
    }

    #[test]
    fn iorr_decodes_ppr_layout() {
        let base = decode_iorr_base(0x0000_000f_e000_0000 | (1 << 4));
        assert_eq!(base.phys_base, 0x0000_000f_e000_0000);
        assert!(base.rd_mem && !base.wr_mem);
        let mask = decode_iorr_mask(0x0000_000f_f800_0000 | (1 << 11));
        assert_eq!(mask.phys_mask, 0x0000_000f_f800_0000);
        assert!(mask.valid);
    }

    #[test]
    fn inventory_shape_enforces_gate_zeroing() {
        let mut inventory = SystemRegisterInventory {
            mtrr_cap: 0x508,
            mtrr_def_type: 0,
            pat: 0,
            variable_mtrr_pairs: [VariableMtrrPair { base: 0, mask: 0 }; MAX_VARIABLE_MTRR_PAIRS],
            variable_mtrr_pair_count: 8,
            fixed_mtrr: [0; FIXED_MTRR_COUNT],
            fixed_mtrr_observed: true,
            sys_cfg: 0,
            hwcr: 0,
            top_mem: 0,
            tom2: 0,
            smm_base: 0,
            smm_addr: 0,
            smm_mask: 0,
            iorr_base: [0; IORR_RANGE_COUNT],
            iorr_mask: [0; IORR_RANGE_COUNT],
            iorr_not_attempted_reason: None,
        };
        assert_eq!(validate_inventory(&inventory), Ok(()));
        assert_eq!(expected_read_operations(&inventory), 42);

        inventory.variable_mtrr_pair_count = 7;
        assert_eq!(
            validate_inventory(&inventory),
            Err(SystemRegisterError::GatedValuesPresentWithoutCapability)
        );
        inventory.variable_mtrr_pair_count = 8;
        inventory.fixed_mtrr_observed = false;
        assert_eq!(
            validate_inventory(&inventory),
            Err(SystemRegisterError::GatedValuesPresentWithoutCapability)
        );
    }

    #[test]
    fn expected_reads_track_each_gate() {
        let mut inventory = SystemRegisterInventory {
            mtrr_cap: 0x404,
            mtrr_def_type: 0,
            pat: 0,
            variable_mtrr_pairs: [VariableMtrrPair { base: 0, mask: 0 }; MAX_VARIABLE_MTRR_PAIRS],
            variable_mtrr_pair_count: 4,
            fixed_mtrr: [0; FIXED_MTRR_COUNT],
            fixed_mtrr_observed: false,
            sys_cfg: 0,
            hwcr: 0,
            top_mem: 0,
            tom2: 0,
            smm_base: 0,
            smm_addr: 0,
            smm_mask: 0,
            iorr_base: [0; IORR_RANGE_COUNT],
            iorr_mask: [0; IORR_RANGE_COUNT],
            iorr_not_attempted_reason: Some("cpu-family-model-not-documented-by-pinned-ppr"),
        };
        assert_eq!(validate_inventory(&inventory), Ok(()));
        // 1 VM_CR + 10 base + 2*4 variable + 0 fixed + 0 IORR.
        assert_eq!(expected_read_operations(&inventory), 19);
        inventory.fixed_mtrr_observed = true;
        inventory.fixed_mtrr = [1; FIXED_MTRR_COUNT];
        assert_eq!(
            validate_inventory(&inventory),
            Err(SystemRegisterError::GatedValuesPresentWithoutCapability)
        );
    }
}
