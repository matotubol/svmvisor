/// PPR 57896 rev3.00, Family1Ah Model44h B0, pp.202/206: optional WB
/// default in [4GiB,TOM2). This is a memory-type observation, not RAM ownership
/// or permission to access a physical range. Existing native unencrypted and
/// coherent-mapping admission remains required by the capture callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tom2Default {
    end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tom2Error {
    UnsupportedProfile,
    ReservedControlBits,
    ActiveEncryptionUnsupported,
    Tom2Disabled,
    InvalidTopOfMemory,
}

impl Tom2Default {
    /// Model gate for reading SYS_CFG and TOM2 after native AMD CPU admission.
    pub const fn supported_profile(signature: u32, physical_bits: u8) -> bool {
        signature == 0x00b4_0f40 && physical_bits == 48
    }

    /// Validate raw controls without modifying them. When bit22 is clear the
    /// ordinary default applies; unused TOM2 reset contents are not interpreted.
    /// Bit22 with disabled TOM2 is conservatively unsupported. DEF_TYPE.E is
    /// checked by Mtrrs::page_type on every use, including this default.
    pub fn new(signature: u32, physical_bits: u8, sys_cfg: u64, tom2: u64)
        -> Result<Option<Self>, Tom2Error>
    {
        if !Self::supported_profile(signature, physical_bits) {
            return Err(Tom2Error::UnsupportedProfile);
        }
        if sys_cfg & !0x07fc_0000 != 0 {
            return Err(Tom2Error::ReservedControlBits);
        }
        // SYS_CFG pp.202: SMEE, SNP, VMPL and host multi-key encryption.
        if sys_cfg & 0x0780_0000 != 0 {
            return Err(Tom2Error::ActiveEncryptionUnsupported);
        }
        if sys_cfg & (1 << 22) == 0 {
            return Ok(None);
        }
        if sys_cfg & (1 << 21) == 0 {
            return Err(Tom2Error::Tom2Disabled);
        }
        // Only bits47:23 exist on this processor: an 8MiB-aligned, exclusive
        // upper bound above4GiB, within the admitted 48-bit physical width.
        if tom2 & !0x0000_ffff_ff80_0000 != 0 || tom2 <= 0x1_0000_0000 {
            return Err(Tom2Error::InvalidTopOfMemory);
        }
        Ok(Some(Self { end: tom2 }))
    }
}

/// Architectural MTRR observation for the unencrypted initial profile. Fixed
/// ranges are outside the admitted >=1MiB monitor/table aliases. AMD model-
/// specific routing is a separate physical-platform admission requirement.
#[derive(Clone, Copy, Debug)]
pub struct Mtrrs {
    pub default: u64,
    pub count: usize,
    pub variable: [(u64, u64); 16],
    pub physical_bits: u8,
    pub tom2_default: Option<Tom2Default>,
}
impl Mtrrs {
    /// PPR 57896 rev3.00 pp.127-130/202 and APM2 rev3.44 7.9.1,
    /// Table7-13: WB low RAM requires both DRAM routing attributes, not just
    /// type6. `sys_cfg` is the control observed while sampling `fixed_byte`:
    /// bit19 must expose the otherwise hidden attributes; bit18 enables them.
    /// Caller establishes the reviewed CPU profile, enabled fixed MTRRs, RAM
    /// ownership and stable routing through the actual read. A temporary bit19
    /// change must be restored before accessing guest RAM or resuming the guest.
    pub const fn native_fixed_page_is_wb(sys_cfg: u64, fixed_byte: u8) -> bool {
        sys_cfg & !0x07fc_0000 == 0
            && sys_cfg & 0x0780_0000 == 0
            && sys_cfg & 0x000c_0000 == 0x000c_0000
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
                && self.default & !0xcff == 0
                && self.default & (1 << 11) == 0
                && matches!(self.default & 255, 0 | 1 | 4 | 5 | 6)
                && matches!(pat_type, 0 | 4 | 5 | 6 | 7))
    }

    /// APM2 rev3.44 7.7.2: fixed MTRR register and byte covering an aligned
    /// page below1MiB. Caller checks MTRRcap.FIX and DEF_TYPE.E/FE before RDMSR;
    /// enabled fixed ranges take precedence over variable ranges.
    pub fn fixed_range_register(page: u64) -> Option<(u32, u8)> {
        if page >= 0x100000 || page & 4095 != 0 { return None; }
        Some(if page < 0x80000 { (0x250, ((page / 0x10000) * 8) as u8) }
            else if page < 0xa0000 { (0x258, (((page - 0x80000) / 0x4000) * 8) as u8) }
            else if page < 0xc0000 { (0x259, (((page - 0xa0000) / 0x4000) * 8) as u8) }
            else { (0x268 + ((page - 0xc0000) / 0x8000) as u32, (((page & 0x7fff) / 4096) * 8) as u8) })
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
            || self.tom2_default.is_some() && self.physical_bits != 48
            || self.count > self.variable.len()
            || self.default & !0xcff != 0
            || self.default & (1 << 11) == 0
            || !matches!(self.default & 255, 0 | 1 | 4 | 5 | 6)
        {
            return None;
        }
        let physical = ((1u64 << self.physical_bits) - 1) & !4095;
        if page & !physical != 0 {
            return None;
        }
        let mut types = 0u8;
        for &(base, mask) in self.variable.iter().take(self.count) {
            if base & !(physical | 0xff) != 0 || mask & !(physical | 0x800) != 0 {
                return None;
            }
            if mask & 0x800 == 0 {
                continue;
            }
            let address_mask = mask & physical;
            let bytes = ((!address_mask & physical) | 4095) + 1;
            if !bytes.is_power_of_two()
                || base & physical & (bytes - 1) != 0
                || !matches!(base & 255, 0 | 1 | 4 | 5 | 6)
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
            if self.tom2_default.is_some_and(|tom2|
                page >= 0x1_0000_0000 && page + 4096 <= tom2.end)
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
