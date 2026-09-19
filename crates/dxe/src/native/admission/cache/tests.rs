use super::*;

use svmvisor_hypervisor::arch::x86_64::msr::TARGET_SIGNATURE;

fn snapshot() -> CacheSnapshot {
    CacheSnapshot {
        abi_version: 1,
        captured_fields: captured::REQUIRED,
        msr_reads: 30,
        signature: TARGET_SIGNATURE,
        max_basic: 0x10,
        max_extended: 0x8000_0026,
        leaf1_edx: 0x0001_1020,
        physical_bits: 48,
        encryption_eax: 1,
        encryption_ebx: 51 | (5 << 6),
        cr0: 0x8001_0033,
        cr4: 0x620,
        efer: 0xd00,
        sys_cfg: 1 << 20,
        pat: 0x0007_0406_0007_0406,
        mtrr_cap: 0x508,
        mtrr_default: 0x806,
        top_mem: 0x0800_0000,
        apic_base: 0xfee0_0900,
        ..CacheSnapshot::default()
    }
}
fn mapping() -> PageMapping {
    PageMapping {
        physical_page: 0x201000,
        leaf_physical_base: 0x200000,
        leaf_bytes: 0x200000,
        pat_index: 0,
    }
}
fn pair(base: u64, bytes: u64, kind: u64) -> RegisterPair {
    RegisterPair { base: base | kind, mask: (PHYSICAL_MASK & !(bytes - 1)) | 0x800 }
}
fn check(s: &CacheSnapshot) -> Result<WriteBackReport, CacheError> {
    classify_write_back(s, 0x201000, 4096, &[mapping()])
}

#[test]
fn actual_default_and_variable_wb_keep_advertised_encryption_bit() {
    let mut s = snapshot();
    let report = check(&s).unwrap();
    assert_eq!(
        report,
        WriteBackReport {
            pages: 1,
            leaf_checks: 1,
            mtrr_segments: 1,
            physical_bits: 48,
            encryption_bit: 51,
            encryption_physical_reduction: 5
        }
    );
    s.mtrr_default = 0x800; // UC outside the actual variable WB range.
    s.variable[0] = pair(0x200000, 0x200000, 6);
    assert_eq!(check(&s).unwrap().mtrr_segments, 1);
    s.variable[1] = pair(0x200000, 0x100000, 6);
    assert_eq!(check(&s).unwrap().mtrr_segments, 2); // Same-type overlap is defined.
}

#[test]
fn leaf_pat_selection_uses_actual_leaf_bit_position_and_current_pat_byte() {
    for index in 0..8u8 {
        let low = u64::from(index & 3) << 3;
        assert_eq!(leaf_pat_index(1 | low | (u64::from(index >> 2) << 7), 4096), Ok(index));
        for bytes in [0x20_0000, 0x4000_0000] {
            assert_eq!(
                leaf_pat_index(0x81 | low | (u64::from(index >> 2) << 12), bytes),
                Ok(index)
            );
        }
    }
    assert_eq!(leaf_pat_index(0, 4096), Err(CacheError::InvalidLeaf));
    assert_eq!(leaf_pat_index(1, 0x200000), Err(CacheError::InvalidLeaf));
    assert_eq!(leaf_pat_index(1, 8192), Err(CacheError::InvalidLeaf));
    let mut s = snapshot();
    s.pat = (s.pat & !255) | 4;
    assert_eq!(check(&s), Err(CacheError::PatNotWriteBack));
    let mut m = mapping();
    m.pat_index = 4;
    assert!(classify_write_back(&s, 0x201000, 4096, &[m]).is_ok());
    s.pat |= 7; // PA0=7 is reserved in this exact target's PPR.
    assert_eq!(check(&s), Err(CacheError::InvalidPat));
}

#[test]
fn large_leaf_rejects_non_wb_outside_the_allocated_page_and_detects_holes() {
    let mut s = snapshot();
    s.variable[0] = pair(0x300000, 0x100000, 0);
    assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
    let mut m = mapping();
    m.leaf_physical_base = m.physical_page;
    m.leaf_bytes = 4096;
    assert!(classify_write_back(&s, m.physical_page, 4096, &[m]).is_ok());
    s.mtrr_default = 0x800;
    s.variable[0] = pair(0x200000, 0x100000, 6);
    s.variable[1] = pair(0x300000, 0x100000, 6);
    assert_eq!(check(&s).unwrap().mtrr_segments, 2);
    s.variable[1] = pair(0x300000, 0x80000, 6);
    assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
}

#[test]
fn all_seventeen_possible_mtrr_segments_are_checked() {
    let mut s = snapshot();
    for (index, out) in s.variable.iter_mut().enumerate() {
        *out = pair(0x201000 + index as u64 * 8192, 4096, 6);
    }
    assert_eq!(check(&s).unwrap().mtrr_segments, 17);
    s.variable[7].base = (s.variable[7].base & !7) | 4;
    assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
}

#[test]
fn overlap_precedence_is_order_independent_and_undefined_combinations_refuse() {
    let mut s = snapshot();
    s.variable[0] = pair(0x200000, 0x200000, 6);
    s.variable[1] = pair(0x200000, 0x200000, 4);
    assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack)); // WT+WB -> WT.
    s.variable[1] = pair(0x200000, 0x200000, 1);
    assert_eq!(check(&s), Err(CacheError::UndefinedMtrrOverlap));
    s.variable[7] = pair(0x200000, 0x200000, 0);
    assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack)); // UC dominates all.
    s.variable.reverse();
    assert_eq!(check(&s), Err(CacheError::MtrrNotWriteBack));
}

#[test]
fn malformed_active_ranges_and_reserved_bits_refuse() {
    let mut s = snapshot();
    s.variable[0] = pair(0x200000, 0x200000, 6);
    s.variable[0].mask ^= 1 << 22; // Noncontiguous mask.
    assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
    s.variable[0] = pair(0x201000, 0x200000, 6); // Base not size-aligned.
    assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
    s.variable[0] = pair(0x200000, 0x200000, 3);
    assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
    s.variable[0].mask &= !0x800; // Inactive type has no effect.
    assert!(check(&s).is_ok());
    s.variable[0].base |= 1 << 48;
    assert_eq!(check(&s), Err(CacheError::ReservedRegisterBits));
    let mut s = snapshot();
    s.iorr[0] = pair(0x201000, 0x200000, 0x18);
    assert_eq!(check(&s), Err(CacheError::InvalidIorr));
}

#[test]
fn priority_routing_ranges_refuse_even_outside_the_allocated_page() {
    let mut s = snapshot();
    s.iorr[0] = pair(0x300000, 0x100000, 0x18);
    assert_eq!(check(&s), Err(CacheError::IorrOverlap));
    s.iorr[0] = pair(0x500000, 0x100000, 0);
    assert!(check(&s).is_ok());
    s.smm_address = 0x300000;
    s.smm_mask = (PHYSICAL_MASK & !0xfffff) | 2;
    assert_eq!(check(&s), Err(CacheError::SmmOverlap));
    s.smm_mask |= 3 << 12;
    assert_eq!(check(&s), Err(CacheError::InvalidSmmRange));
    s.smm_mask = 0;
    s.apic_base = 0x300000 | 0x900;
    assert_eq!(check(&s), Err(CacheError::ApicOverlap));
    s.apic_base &= !(1 << 11);
    assert!(check(&s).is_ok());
    s.mmio_config = 0x300000 | 1; // One1MiB bus window.
    assert_eq!(check(&s), Err(CacheError::MmioConfigurationOverlap));
    s.mmio_config = 0x500000 | (1 << 2) | 1; // Misaligned2MiB window.
    assert_eq!(check(&s), Err(CacheError::InvalidMmioWindow));
}

#[test]
fn encryption_and_missing_optional_observations_never_become_disabled_by_default() {
    for bit in 23..=26 {
        let mut s = snapshot();
        s.sys_cfg |= 1 << bit;
        assert_eq!(check(&s), Err(CacheError::AddressEncryptionActive));
    }
    let mut s = snapshot();
    s.encryption_eax |= 2;
    assert_eq!(check(&s), Err(CacheError::IncompleteCapture));
    s.captured_fields |= captured::SEV_STATUS;
    s.msr_reads += 1;
    assert!(check(&s).is_ok());
    s.sev_status = 1;
    assert_eq!(check(&s), Err(CacheError::AddressEncryptionActive));
    s.sev_status = 1 << 63;
    assert_eq!(check(&s), Err(CacheError::AddressEncryptionActive));
    let mut s = snapshot();
    s.encryption_eax = 0;
    assert_eq!(check(&s), Err(CacheError::UnsupportedFeatures));
    s.encryption_eax = 1;
    s.encryption_ebx = 47;
    assert_eq!(check(&s), Err(CacheError::UnsupportedFeatures));
}

#[test]
fn capture_shape_cache_mode_and_reserved_state_fail_closed() {
    let initial = snapshot();
    for mutate in [
        |s: &mut CacheSnapshot| s.abi_version = 0,
        |s: &mut CacheSnapshot| s.refusal = 5,
        |s: &mut CacheSnapshot| s.captured_fields &= !captured::MTRRS,
        |s: &mut CacheSnapshot| s.msr_reads -= 1,
    ] {
        let mut s = initial;
        mutate(&mut s);
        assert_eq!(check(&s), Err(CacheError::IncompleteCapture));
    }
    let mut s = initial;
    s.signature += 1;
    assert_eq!(check(&s), Err(CacheError::UnsupportedCpu));
    s = initial;
    s.leaf1_ecx |= 1 << 31;
    assert_eq!(check(&s), Err(CacheError::UnsupportedFeatures));
    s = initial;
    s.cr0 |= 1 << 30;
    assert_eq!(check(&s), Err(CacheError::CacheDisabled));
    s = initial;
    s.cr0 |= 1 << 29;
    assert_eq!(check(&s), Err(CacheError::CacheDisabled));
    s = initial;
    s.mtrr_default &= !0x800;
    assert_eq!(check(&s), Err(CacheError::MtrrsDisabled));
    s = initial;
    s.sys_cfg &= !(1 << 20);
    assert_eq!(check(&s), Err(CacheError::DramRoutingDisabled));
    s = initial;
    s.mtrr_cap = 0x509;
    assert_eq!(check(&s), Err(CacheError::InvalidMtrr));
    s = initial;
    s.mtrr_cap |= 1 << 11;
    assert_eq!(check(&s), Err(CacheError::ReservedRegisterBits));
}

#[test]
fn whole_leaf_low_ram_bounds_and_per_page_coverage_are_required() {
    let s = snapshot();
    let mut m = mapping();
    m.leaf_physical_base = 0;
    m.physical_page = 0x100000;
    assert_eq!(
        classify_write_back(&s, m.physical_page, 4096, &[m]),
        Err(CacheError::LeafOutsideLowRam)
    );
    m = mapping();
    m.leaf_physical_base = 0x4000000;
    m.physical_page = 0x4001000;
    m.leaf_bytes = 0x4000000; // Unsupported64MiB leaf.
    assert_eq!(classify_write_back(&s, m.physical_page, 4096, &[m]), Err(CacheError::InvalidLeaf));
    m = mapping();
    m.pat_index = 8;
    assert_eq!(classify_write_back(&s, m.physical_page, 4096, &[m]), Err(CacheError::InvalidLeaf));
    assert_eq!(classify_write_back(&s, 0x201000, 4096, &[]), Err(CacheError::MappingCountMismatch));
    let mut second = mapping();
    second.physical_page += 8192;
    assert_eq!(
        classify_write_back(&s, 0x201000, 8192, &[mapping(), second]),
        Err(CacheError::NoncontiguousMapping)
    );
    second.physical_page -= 4096;
    assert_eq!(classify_write_back(&s, 0x201000, 8192, &[mapping(), second]).unwrap().pages, 2);
    assert_eq!(
        classify_write_back(&s, 0x201000, (MAX_PAGES as u64 + 1) * 4096, &[]),
        Err(CacheError::TooManyPages)
    );
    assert_eq!(
        classify_write_back(&s, u64::MAX - 4095, 4096, &[mapping()]),
        Err(CacheError::InvalidAllocation)
    );
    let mut s = s;
    s.top_mem += 4096;
    assert_eq!(check(&s), Err(CacheError::InvalidTopOfMemory));
}
