use crate::arch::x86_64::msr::{
    MTRR_CAP, MTRR_DEF_TYPE, MTRR_FIX_4K_0, MTRR_FIX_16K_0, MTRR_FIX_16K_1, MTRR_FIX_64K,
    MTRR_VAR_BASE0, SYS_CFG, SYS_CFG_DEFINED, SYS_CFG_ENCRYPTION, SYS_CFG_MTRR_FIX_DRAM_EN,
    SYS_CFG_MTRR_FIX_DRAM_MOD_EN, SYS_CFG_MTRR_TOM2_EN, SYS_CFG_TOM2_FORCE_MEM_TYPE_WB,
    TARGET_PHYSICAL_BITS, TARGET_SIGNATURE, TOM2,
};

/// MTRRcap FIX: fixed-range MTRRs are supported.
pub const CAP_FIX: u64 = 1 << 8;
/// MTRRdefType defines only its type byte, FE (bit10) and E (bit11).
pub(crate) const DEF_TYPE_DEFINED: u64 = 0xcff;
pub const DEF_TYPE_FE: u64 = 1 << 10;
pub const DEF_TYPE_E: u64 = 1 << 11;
/// MtrrVarMask valid bit.
pub(crate) const VARIABLE_VALID: u64 = 1 << 11;
/// Largest enumerated variable-MTRR count this owner captures.
pub const MAX_VARIABLE: usize = 16;

/// Architectural MTRR observation for the unencrypted initial profile. Fixed
/// ranges are outside the admitted >=1MiB monitor/table aliases. AMD model-
/// specific routing is a separate physical-platform admission requirement.
#[derive(Clone, Copy, Debug)]
pub struct Mtrrs {
    pub default: u64,
    pub count: usize,
    pub variable: [(u64, u64); MAX_VARIABLE],
    pub physical_bits: u8,
    pub tom2_default: Option<Tom2Default>,
}

impl Mtrrs {
    /// Bounded capture on the owning CPU, after native CPU/encryption
    /// admission. `read` performs RDMSR of MTRRcap and MTRRdefType, then
    /// SYS_CFG and TOM2 only on the reviewed profile (PPR57896 rev3.00
    /// pp.202/206), then each enumerated variable pair. It never writes.
    pub fn read(
        physical_bits: u8,
        signature: u32,
        mut read: impl FnMut(u32) -> u64,
    ) -> Result<Self, MtrrReadError> {
        let capability = read(MTRR_CAP);
        let count = (capability & 255) as usize;
        if count > MAX_VARIABLE {
            return Err(MtrrReadError::VariableCount { capability });
        }
        let mut result = Self {
            default: read(MTRR_DEF_TYPE),
            count,
            variable: [(0, 0); MAX_VARIABLE],
            physical_bits,
            tom2_default: None,
        };
        if Tom2Default::supported_profile(signature, physical_bits) {
            let sys_cfg = read(SYS_CFG);
            let tom2 = read(TOM2);
            result.tom2_default = Tom2Default::new(signature, physical_bits, sys_cfg, tom2)
                .map_err(|error| MtrrReadError::Tom2 { error, sys_cfg, tom2 })?;
        }
        for (index, pair) in result.variable.iter_mut().take(count).enumerate() {
            let base = MTRR_VAR_BASE0 + 2 * index as u32;
            *pair = (read(base), read(base + 1));
        }
        Ok(result)
    }

    /// PPR 57896 rev3.00 pp.127-130/202 and APM2 rev3.44 7.9.1,
    /// Table7-13: WB low RAM requires both DRAM routing attributes, not just
    /// type6. `sys_cfg` is the control observed while sampling `fixed_byte`:
    /// bit19 must expose the otherwise hidden attributes; bit18 enables them.
    /// Caller establishes the reviewed CPU profile, enabled fixed MTRRs, RAM
    /// ownership and stable routing through the actual read. A temporary bit19
    /// change must be restored before accessing guest RAM or resuming the guest.
    pub const fn native_fixed_page_is_wb(sys_cfg: u64, fixed_byte: u8) -> bool {
        let dram = SYS_CFG_MTRR_FIX_DRAM_EN | SYS_CFG_MTRR_FIX_DRAM_MOD_EN;
        sys_cfg & !SYS_CFG_DEFINED == 0
            && sys_cfg & SYS_CFG_ENCRYPTION == 0
            && sys_cfg & dram == dram
            && fixed_byte == 0x1e
    }

    /// Terminal-report-only UC observation, including Windows' temporary
    /// disabled-MTRR interval. APM2 rev3.44 7.7.1/7.8.5, Table7-11: disabled
    /// MTRRs supply UC, but PAT WC still produces WC. This does not authorize
    /// RAM reads, guest continuation, device ownership, or changed routing.
    /// Ordinary WB reader admission intentionally continues to reject E=0.
    pub fn terminal_page_is_uc(&self, page: u64, pat_type: u8) -> bool {
        self.page_is_uc(page, pat_type)
            || (page >= 0x100000
                && page & 4095 == 0
                && (32..=52).contains(&self.physical_bits)
                && page < (1u64 << self.physical_bits)
                && self.count <= self.variable.len()
                && is_valid_default(self.default)
                && self.default & DEF_TYPE_E == 0
                && matches!(pat_type, 0 | 4 | 5 | 6 | 7))
    }

    /// APM2 rev3.44 7.7.2: fixed MTRR register and byte covering an aligned
    /// page below1MiB. Caller checks MTRRcap.FIX and DEF_TYPE.E/FE before RDMSR;
    /// enabled fixed ranges take precedence over variable ranges.
    pub fn fixed_range_register(page: u64) -> Option<(u32, u8)> {
        if page >= 0x100000 || page & 4095 != 0 {
            return None;
        }
        Some(if page < 0x80000 {
            (MTRR_FIX_64K, ((page / 0x10000) * 8) as u8)
        } else if page < 0xa0000 {
            (MTRR_FIX_16K_0, (((page - 0x80000) / 0x4000) * 8) as u8)
        } else if page < 0xc0000 {
            (MTRR_FIX_16K_1, (((page - 0xa0000) / 0x4000) * 8) as u8)
        } else {
            (
                MTRR_FIX_4K_0 + ((page - 0xc0000) / 0x8000) as u32,
                (((page & 0x7fff) / 4096) * 8) as u8,
            )
        })
    }

    pub fn page_is_wb(&self, page: u64) -> bool {
        self.page_type(page) == Some(6)
    }

    /// APM2 rev3.44 7.8.5/Table7-11: effective UC for an enabled, validated
    /// MTRR observation and the actual leaf's PAT byte. PAT WC overrides even
    /// MTRR UC; PAT UC with a cacheable MTRR is CD rather than UC. Fixed ranges
    /// and disabled MTRRs remain outside this initial >=1MiB profile.
    pub fn page_is_uc(&self, page: u64, pat_type: u8) -> bool {
        matches!(pat_type, 0 | 4 | 5 | 6 | 7) && self.page_type(page) == Some(0)
    }

    fn page_type(&self, page: u64) -> Option<u8> {
        if page < 0x100000
            || page & 4095 != 0
            || !(32..=52).contains(&self.physical_bits)
            || self.tom2_default.is_some() && self.physical_bits != TARGET_PHYSICAL_BITS
            || self.count > self.variable.len()
            || !is_valid_default(self.default)
            || self.default & DEF_TYPE_E == 0
        {
            return None;
        }
        let physical = ((1u64 << self.physical_bits) - 1) & !4095;
        if page & !physical != 0 {
            return None;
        }
        let mut types = 0u8;
        for &(base, mask) in self.variable.iter().take(self.count) {
            if base & !(physical | 0xff) != 0 || mask & !(physical | VARIABLE_VALID) != 0 {
                return None;
            }
            if mask & VARIABLE_VALID == 0 {
                continue;
            }
            let address_mask = mask & physical;
            let bytes = ((!address_mask & physical) | 4095) + 1;
            if !bytes.is_power_of_two()
                || base & physical & (bytes - 1) != 0
                || !is_valid_type(base as u8)
            {
                return None;
            }
            if page & address_mask == base & address_mask {
                types |= 1 << (base & 255);
            }
        }
        // APM2 7.7.4: UC dominates defined overlaps; WB+WT resolves to WT.
        if types == 0 {
            // PPR pp.202/206 -> APM2 7.7.2/7.7.4: replace only the default,
            // never a matching variable MTRR. PAT is combined by the caller.
            if self
                .tom2_default
                .is_some_and(|tom2| page >= 0x1_0000_0000 && page + 4096 <= tom2.end)
            {
                Some(6)
            } else {
                Some((self.default & 255) as u8)
            }
        } else if types & 1 != 0 {
            Some(0)
        } else if types.count_ones() == 1 {
            Some(types.trailing_zeros() as u8)
        } else if types == (1 << 4) | (1 << 6) {
            Some(4)
        } else {
            None
        }
    }
}

/// PPR 57896 rev3.00, Family1Ah Model44h B0, pp.202/206: optional WB
/// default in [4GiB,TOM2). This is a memory-type observation, not RAM ownership
/// or permission to access a physical range. Existing native unencrypted and
/// coherent-mapping admission remains required by the capture callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tom2Default {
    end: u64,
}

impl Tom2Default {
    /// Validate raw controls without modifying them. When bit22 is clear the
    /// ordinary default applies; unused TOM2 reset contents are not interpreted.
    /// Bit22 with disabled TOM2 is conservatively unsupported. DEF_TYPE.E is
    /// checked by Mtrrs::page_type on every use, including this default.
    pub fn new(
        signature: u32,
        physical_bits: u8,
        sys_cfg: u64,
        tom2: u64,
    ) -> Result<Option<Self>, Tom2Error> {
        if !Self::supported_profile(signature, physical_bits) {
            return Err(Tom2Error::UnsupportedProfile);
        }
        if sys_cfg & !SYS_CFG_DEFINED != 0 {
            return Err(Tom2Error::ReservedControlBits);
        }
        if sys_cfg & SYS_CFG_ENCRYPTION != 0 {
            return Err(Tom2Error::ActiveEncryptionUnsupported);
        }
        if sys_cfg & SYS_CFG_TOM2_FORCE_MEM_TYPE_WB == 0 {
            return Ok(None);
        }
        if sys_cfg & SYS_CFG_MTRR_TOM2_EN == 0 {
            return Err(Tom2Error::Tom2Disabled);
        }
        // Only bits47:23 exist on this processor: an 8MiB-aligned, exclusive
        // upper bound above4GiB, within the admitted 48-bit physical width.
        if tom2 & !0x0000_ffff_ff80_0000 != 0 || tom2 <= 0x1_0000_0000 {
            return Err(Tom2Error::InvalidTopOfMemory);
        }
        Ok(Some(Self { end: tom2 }))
    }

    /// Model gate for reading SYS_CFG and TOM2 after native AMD CPU admission.
    pub const fn supported_profile(signature: u32, physical_bits: u8) -> bool {
        signature == TARGET_SIGNATURE && physical_bits == TARGET_PHYSICAL_BITS
    }
}

/// A capture refusal with the raw values its callers report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MtrrReadError {
    VariableCount { capability: u64 },
    Tom2 { error: Tom2Error, sys_cfg: u64, tom2: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tom2Error {
    UnsupportedProfile,
    ReservedControlBits,
    ActiveEncryptionUnsupported,
    Tom2Disabled,
    InvalidTopOfMemory,
}

/// MTRRdefType with only defined bits and a valid default type.
pub(crate) const fn is_valid_default(value: u64) -> bool {
    value & !DEF_TYPE_DEFINED == 0 && is_valid_type(value as u8)
}

/// APM2 rev3.44 7.7.1: UC, WC, WT, WP and WB; every other type is reserved.
pub(crate) const fn is_valid_type(value: u8) -> bool {
    matches!(value, 0 | 1 | 4 | 5 | 6)
}
