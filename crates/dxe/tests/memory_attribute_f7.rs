#![cfg(feature = "memory-attribute-f7")]

use svmvisor_dxe::memory_attributes::f7::{
    CpuCapabilities, CpuObservation, F7Failure, TARGET_SIGNATURE, validate_capabilities,
    validate_capabilities_detailed, validate_observation, validate_observation_detailed,
    validate_table_source,
};
use svmvisor_hypervisor::boot::memory::{MemoryDescriptor, ValidatedMemoryMap};
use svmvisor_memory_attributes::{Config, Error};

fn config() -> Config {
    Config {
        root: 0x1000,
        physical_bits: 48,
        nxe: true,
        page1gb: true,
    }
}

#[test]
fn capability_diagnostics_distinguish_the_pre_msr_refusal() {
    let mut candidate = cpu();
    candidate.vendor[0] ^= 1;
    assert_eq!(
        validate_capabilities_detailed(candidate),
        Err(F7Failure::Vendor)
    );
    candidate = cpu();
    candidate.signature ^= 1;
    assert_eq!(
        validate_capabilities_detailed(candidate),
        Err(F7Failure::Signature)
    );
    candidate = cpu();
    candidate.physical_bits = 52;
    assert_eq!(
        validate_capabilities_detailed(candidate),
        Err(F7Failure::PhysicalWidth)
    );
    candidate = cpu();
    candidate.encryption_ebx = 47;
    assert_eq!(
        validate_capabilities_detailed(candidate),
        Err(F7Failure::SmeCapability)
    );
    candidate = cpu();
    candidate.multi_key[3] = 1;
    assert_eq!(
        validate_capabilities_detailed(candidate),
        Err(F7Failure::MultiKey)
    );
}

#[test]
fn control_diagnostics_distinguish_architecture_from_mapping_failure() {
    for (reason, index) in [
        (F7Failure::Cr0, 0),
        (F7Failure::Cr3, 1),
        (F7Failure::Cr4, 2),
        (F7Failure::Efer, 3),
        (F7Failure::SysCfg, 4),
        (F7Failure::Sev, 5),
    ] {
        let mut observed = state();
        match index {
            0 => observed.cr0 &= !(1 << 16),
            1 => observed.cr3 = 0x2000,
            2 => observed.cr4 |= 1 << 17,
            3 => observed.efer &= !(1 << 11),
            4 => observed.sys_cfg |= 1 << 23,
            5 => observed.sev_status = 1,
            _ => unreachable!(),
        }
        assert_eq!(
            validate_observation_detailed(config(), observed),
            Err(reason)
        );
    }
    assert_ne!(F7Failure::RootSource.code(), F7Failure::TableSource.code());
    assert_ne!(
        F7Failure::TableSource.code(),
        F7Failure::UnsupportedQuery.code()
    );
    assert_eq!(F7Failure::ContextChanged.error(), Error::AccessDenied);
    assert_eq!(
        F7Failure::from_error(Error::NoMapping),
        F7Failure::NoMapping
    );
}

fn cpu() -> CpuCapabilities {
    CpuCapabilities {
        max_basic: 0x10,
        vendor: [0x6874_7541, 0x6974_6e65, 0x444d_4163],
        signature: TARGET_SIGNATURE,
        leaf1_edx: 0x0001_1060,
        max_extended: 0x8000_0023,
        extended_edx: (1 << 29) | (1 << 26) | (1 << 20),
        physical_bits: 48,
        topology_ebx: 2,
        topology_ecx: 1 << 8,
        apic_id: 0x1234,
        encryption_eax: 1,
        encryption_ebx: 51 | (5 << 6),
        ..CpuCapabilities::default()
    }
}

fn state() -> CpuObservation {
    CpuObservation {
        cpu: cpu(),
        cr0: 0x8001_0033,
        cr3: 0x1000,
        cr4: 0x620,
        efer: 0xd00,
        sys_cfg: 1 << 20,
        sev_status: 0,
    }
}

#[test]
fn admitted_observation_keeps_cr3_cache_bits_separate_from_root() {
    assert_eq!(validate_capabilities(cpu()), Ok(()));
    assert_eq!(validate_observation(config(), state()), Ok(()));
    let mut observed = state();
    observed.cr3 |= 0x18;
    assert_eq!(validate_observation(config(), observed), Ok(()));
}

#[test]
fn unqualified_cpu_or_msr_profile_refuses_before_native_observation() {
    for change in 0..13 {
        let mut candidate = cpu();
        match change {
            0 => candidate.signature ^= 1,
            1 => candidate.vendor[0] ^= 1,
            2 => candidate.max_basic = 0x0a,
            3 => candidate.max_extended = 0x8000_001f,
            4 => candidate.leaf1_ecx |= 1 << 31,
            5 => candidate.leaf1_edx &= !(1 << 5),
            6 => candidate.extended_edx &= !(1 << 29),
            7 => candidate.physical_bits = 52,
            8 => candidate.topology_ebx = 0,
            9 => candidate.topology_ecx = 0,
            10 => candidate.encryption_ebx = 47,
            11 => candidate.encryption_eax |= 1 << 31,
            12 => candidate.multi_key[2] = 1,
            _ => unreachable!(),
        }
        assert_eq!(validate_capabilities(candidate), Err(Error::Unsupported));
    }
}

#[test]
fn unsupported_paging_modes_and_cache_disabled_refuse() {
    for bit in [12, 17, 21, 22, 23, 24] {
        let mut observed = state();
        observed.cr4 |= 1 << bit;
        assert_eq!(
            validate_observation(config(), observed),
            Err(Error::Unsupported)
        );
    }
    for bit in [0, 16, 31] {
        let mut observed = state();
        observed.cr0 &= !(1 << bit);
        assert_eq!(
            validate_observation(config(), observed),
            Err(Error::Unsupported)
        );
    }
    for bit in [29, 30] {
        let mut observed = state();
        observed.cr0 |= 1 << bit;
        assert_eq!(
            validate_observation(config(), observed),
            Err(Error::Unsupported)
        );
    }
    let mut observed = state();
    observed.cr4 &= !(1 << 5);
    assert_eq!(
        validate_observation(config(), observed),
        Err(Error::Unsupported)
    );
}

#[test]
fn root_width_and_interpretation_must_match_actual_observation() {
    for root in [0, 0x1008, 0x2000, 1 << 47, 1 << 48] {
        let mut candidate = config();
        candidate.root = root;
        assert_eq!(
            validate_observation(candidate, state()),
            Err(Error::Unsupported)
        );
    }
    for raw in [0x1001, 0x2000, 0x8000_0000_0000_1000, (1 << 48) | 0x1000] {
        let mut observed = state();
        observed.cr3 = raw;
        assert_eq!(
            validate_observation(config(), observed),
            Err(Error::Unsupported)
        );
    }
    let mut candidate = config();
    candidate.nxe = false;
    assert_eq!(
        validate_observation(candidate, state()),
        Err(Error::Unsupported)
    );
    candidate = config();
    candidate.page1gb = false;
    assert_eq!(
        validate_observation(candidate, state()),
        Err(Error::Unsupported)
    );
    candidate = config();
    candidate.physical_bits = 47;
    assert_eq!(
        validate_observation(candidate, state()),
        Err(Error::Unsupported)
    );
}

#[test]
fn no_address_encryption_or_reserved_register_state_is_admitted() {
    for bit in [23, 24, 25, 26, 63] {
        let mut observed = state();
        observed.sys_cfg |= 1 << bit;
        assert_eq!(
            validate_observation(config(), observed),
            Err(Error::Unsupported)
        );
    }
    let mut observed = state();
    observed.sev_status = 1;
    assert_eq!(
        validate_observation(config(), observed),
        Err(Error::Unsupported)
    );
    for bit in [8, 10] {
        let mut observed = state();
        observed.efer &= !(1 << bit);
        assert_eq!(
            validate_observation(config(), observed),
            Err(Error::Unsupported)
        );
    }
    observed = state();
    observed.efer |= 1 << 63;
    assert_eq!(
        validate_observation(config(), observed),
        Err(Error::Unsupported)
    );
}

#[test]
fn source_requires_full_page_allocated_ram_before_any_load() {
    let records = [
        MemoryDescriptor {
            memory_type: 4,
            physical_start: 0x1000,
            page_count: 1,
            attributes: 8,
        },
        MemoryDescriptor {
            memory_type: 7,
            physical_start: 0x2000,
            page_count: 1,
            attributes: 8,
        },
        MemoryDescriptor {
            memory_type: 4,
            physical_start: 0x3000,
            page_count: 1,
            attributes: 0x2008,
        },
        MemoryDescriptor {
            memory_type: 4,
            physical_start: 0x4000,
            page_count: 1,
            attributes: 1,
        },
        MemoryDescriptor {
            memory_type: 11,
            physical_start: 0x5000,
            page_count: 1,
            attributes: 8,
        },
    ];
    let memory = ValidatedMemoryMap::new(&records, 48).unwrap();
    assert_eq!(validate_table_source(&memory, 0x1000), Ok(()));
    assert_eq!(validate_table_source(&memory, 0x1ff8), Ok(()));
    for address in [
        0,
        0x1001,
        0x2000,
        0x3000,
        0x4000,
        0x5000,
        0x6000,
        1 << 47,
        u64::MAX,
    ] {
        assert_eq!(
            validate_table_source(&memory, address),
            Err(Error::Unsupported)
        );
    }
}
