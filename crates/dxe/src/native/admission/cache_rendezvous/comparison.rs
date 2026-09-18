//! Pure field-by-field comparison of two supplied cache snapshots.

use super::super::cache::CacheSnapshot;
use super::{AP_CR4_DIFFERENCE, ARITHMETIC_FLAGS, ConfigurationField};

/// Pure comparison of supplied observations. Only initial APIC ID, APIC_BASE's
/// BSP role bit and arithmetic flags may differ. This is intentionally stricter
/// than cache equivalence: all CR0/CR4/EFER and common CPUID bits must match, even
/// when a legitimate AP profile difference would not change a memory type.
pub fn compare_configuration(
    bsp: &CacheSnapshot,
    ap: &CacheSnapshot,
) -> Result<(), ConfigurationField> {
    compare_configuration_with_cr4_mask(bsp, ap, 0)
}

/// Pure cross-CPU comparison for the final AP rendezvous only. The firmware
/// may leave CR4.DE different on APs; it controls I/O debug extensions, not
/// translation or memory type. Every other CR4 bit retains exact comparison.
/// Same-CPU capture consistency and BSP before/after checks remain exact.
pub fn compare_ap_configuration(
    bsp: &CacheSnapshot,
    ap: &CacheSnapshot,
) -> Result<(), ConfigurationField> {
    compare_configuration_with_cr4_mask(bsp, ap, AP_CR4_DIFFERENCE)
}

fn compare_configuration_with_cr4_mask(
    bsp: &CacheSnapshot,
    ap: &CacheSnapshot,
    allowed_cr4_difference: u64,
) -> Result<(), ConfigurationField> {
    macro_rules! equal {
        ($($member:ident => $field:ident),* $(,)?) => {$ (
            if bsp.$member != ap.$member { return Err(ConfigurationField::$field); }
        )*};
    }
    equal!(
        abi_version => AbiVersion, captured_fields => CapturedFields,
        refusal => Refusal, msr_reads => MsrReads, signature => Signature,
        max_basic => MaximumBasicLeaf, max_extended => MaximumExtendedLeaf,
        leaf1_ecx => Leaf1Ecx, leaf1_edx => Leaf1Edx, physical_bits => PhysicalBits,
        encryption_eax => EncryptionEax, encryption_ebx => EncryptionEbx,
        multi_key_eax => MultiKeyEax, multi_key_ebx => MultiKeyEbx, reserved => Reserved,
        cr0 => Cr0,
    );
    if (bsp.cr4 ^ ap.cr4) & !allowed_cr4_difference != 0 {
        return Err(ConfigurationField::Cr4);
    }
    equal!(
        efer => Efer, sys_cfg => SysCfg, sev_status => SevStatus,
        pat => Pat, mtrr_cap => MtrrCap, mtrr_default => MtrrDefault, top_mem => TopMem,
        smm_address => SmmAddress, smm_mask => SmmMask, mmio_config => MmioConfig,
        iorr => Iorr, variable => VariableMtrrs,
    );
    if bsp.rflags & !ARITHMETIC_FLAGS != ap.rflags & !ARITHMETIC_FLAGS {
        return Err(ConfigurationField::Rflags);
    }
    if bsp.apic_base & !(1 << 8) != ap.apic_base & !(1 << 8) {
        return Err(ConfigurationField::ApicBase);
    }
    Ok(())
}
