use svmvisor_hypervisor::firmware_memory::*;
#[test]
fn guest_ram_reuses_retained_coverage_but_never_monitor_or_mmio() {
    use svmvisor_hypervisor::memory::address::{AddressPolicy, EncryptionState};
    let policy = AddressPolicy::new(
        40,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let monitor = policy.validate(0x400000, 0x100000, 4096).unwrap();
    let descriptors = [
        ram(0x200000, 256),
        MemoryDescriptor {
            memory_type: 7,
            ..ram(0x300000, 256)
        },
        ram(0x400000, 512),
        MemoryDescriptor {
            memory_type: 11,
            ..ram(0x600000, 1)
        },
    ];
    let map = ValidatedMemoryMap::new(&descriptors, 40).unwrap();
    assert!(map.permit_guest_ram(0x2fffff, 2, monitor).is_ok());
    assert_eq!(
        map.permit_table_entry(0x300000),
        Err(MemoryError::UntrustedMemoryType)
    );
    assert!(map.permit_guest_ram(0x3ff000, 4096, monitor).is_ok());
    assert_eq!(
        map.permit_guest_ram(0x3fffff, 2, monitor),
        Err(MemoryError::MonitorOverlap)
    );
    assert_eq!(
        map.permit_guest_ram(0x400000, 4096, monitor),
        Err(MemoryError::MonitorOverlap)
    );
    assert!(map.permit_guest_ram(0x500000, 4096, monitor).is_ok());
    assert_eq!(
        map.permit_guest_ram(0x600000, 1, monitor),
        Err(MemoryError::UntrustedMemoryType)
    );
    assert_eq!(
        map.permit_guest_ram(0x601000, 1, monitor),
        Err(MemoryError::UncoveredRange)
    );
    assert_eq!(
        map.permit_guest_ram(u64::MAX, 2, monitor),
        Err(MemoryError::Overflow)
    );
}
fn ram(start: u64, pages: u64) -> MemoryDescriptor {
    MemoryDescriptor {
        memory_type: 4,
        physical_start: start,
        page_count: pages,
        attributes: 8,
    }
}
#[test]
fn rejects_malformed_maps_before_any_access() {
    for (descriptors, error) in [
        (vec![], MemoryError::DescriptorCount),
        (vec![ram(0x1000, 0)], MemoryError::EmptyDescriptor),
        (vec![ram(1, 1)], MemoryError::MisalignedDescriptor),
        (vec![ram(0, u64::MAX)], MemoryError::Overflow),
        (vec![ram(1 << 48, 1)], MemoryError::OutsidePhysicalWidth),
        (
            vec![ram(0x1000, 2), ram(0x2000, 1)],
            MemoryError::UnsortedOrOverlapping,
        ),
        (
            vec![ram(0x2000, 1), ram(0x1000, 1)],
            MemoryError::UnsortedOrOverlapping,
        ),
    ] {
        assert_eq!(
            ValidatedMemoryMap::new(&descriptors, 48).unwrap_err(),
            error
        );
    }
}
#[test]
fn denies_untrusted_types_and_non_writeback_or_protected_metadata() {
    for ty in [0, 7, 8, 9, 10, 11, 12, 13, 14, 15, u32::MAX] {
        let descriptors = [MemoryDescriptor {
            memory_type: ty,
            ..ram(0x1000, 1)
        }];
        let map = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        assert_eq!(
            map.read_table_entry(0x1000, |_| panic!("denied read")),
            Err(MemoryError::UntrustedMemoryType)
        );
    }
    for attr in [0, 1, 2, 4, 16, 7, 23] {
        let descriptors = [MemoryDescriptor {
            attributes: attr,
            ..ram(0x1000, 1)
        }];
        let map = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
        assert_eq!(
            map.permit_table_entry(0x1000),
            Err(MemoryError::MissingWriteBackCapability)
        );
    }
    let descriptors = [MemoryDescriptor {
        attributes: 8 | 0x2000,
        ..ram(0x1000, 1)
    }];
    assert_eq!(
        ValidatedMemoryMap::new(&descriptors, 48)
            .unwrap()
            .permit_table_entry(0x1000),
        Err(MemoryError::ReadProtected)
    );
}
#[test]
fn ranges_cover_adjacent_descriptors_but_never_gaps_or_untrusted_tail() {
    let descriptors = [ram(0x1000, 1), ram(0x2000, 1)];
    let map = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    assert_eq!(map.permit_gdt_copy(0x1ff8, 16).unwrap().bytes(), 16);
    assert_eq!(
        map.permit_table_entry(0x2ff8).unwrap().physical_start(),
        0x2ff8
    );
    assert_eq!(
        map.permit_table_entry(0x3000),
        Err(MemoryError::UncoveredRange)
    );
    assert_eq!(
        map.permit_table_entry(0x1fff),
        Err(MemoryError::MisalignedEntry)
    );
    let descriptors = [ram(0x1000, 1), ram(0x3000, 1)];
    let map = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    assert_eq!(
        map.permit_gdt_copy(0x1ff8, 16),
        Err(MemoryError::UncoveredRange)
    );
    let descriptors = [
        ram(0x1000, 1),
        MemoryDescriptor {
            memory_type: 11,
            ..ram(0x2000, 1)
        },
    ];
    let map = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    assert_eq!(
        map.copy_gdt(0x1ff8, &mut [0; 16], |_, _| panic!("untrusted tail")),
        Err(MemoryError::UntrustedMemoryType)
    );
}
#[test]
fn bounded_callbacks_are_invoked_only_after_full_metadata_validation() {
    let descriptors = [ram(0x1000, 16)];
    let map = ValidatedMemoryMap::new(&descriptors, 48).unwrap();
    assert_eq!(
        map.read_table_entry(0x1000, |r| {
            assert_eq!(r.bytes(), 8);
            Some(0x1234)
        }),
        Ok(0x1234)
    );
    assert_eq!(
        map.read_table_entry(0x1000, |_| None),
        Err(MemoryError::ReadFailed)
    );
    let mut bytes = [0; 16];
    map.copy_gdt(0x1000, &mut bytes, |r, out| {
        assert_eq!(r.bytes(), out.len());
        out.fill(7);
        true
    })
    .unwrap();
    assert_eq!(bytes, [7; 16]);
    assert_eq!(
        map.permit_gdt_copy(0x1000, 65537),
        Err(MemoryError::CopyTooLarge)
    );
    assert_eq!(map.permit_gdt_copy(0x1000, 0), Err(MemoryError::EmptyRange));
    assert_eq!(map.permit_gdt_copy(u64::MAX, 2), Err(MemoryError::Overflow));
    assert!(map.permit_gdt_copy(0x1000, 65536).is_ok());
}

#[test]
fn writeback_capability_can_coexist_with_other_supported_cache_modes() {
    for attrs in [8, 9, 10, 12, 15, 24, 31] {
        let descriptors = [MemoryDescriptor {
            attributes: attrs,
            ..ram(0x1000, 1)
        }];
        assert!(
            ValidatedMemoryMap::new(&descriptors, 48)
                .unwrap()
                .permit_table_entry(0x1000)
                .is_ok()
        );
    }
}
