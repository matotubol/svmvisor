use svmvisor_hypervisor::host::paging::*;
fn config() -> PagingConfig {
    PagingConfig {
        cr3: 0x1000,
        physical_bits: 48,
        la57: false,
        nxe: true,
        pcid: false,
        page1gb: true,
    }
}
fn walk(entries: [u64; 4]) -> Result<Translation, WalkError> {
    let mut reads = Vec::new();
    let result = translate(config(), 0x123, |pa| {
        reads.push(pa);
        [0x1000, 0x2000, 0x3000, 0x4000].iter().position(|&x| x == pa).map(|i| entries[i])
    });
    assert!(reads.len() <= 4);
    result
}
#[test]
fn four_kib_translation_intersects_every_level_permission() {
    let result = walk([0x2007, 0x3005 | (1 << 63), 0x4003, 0x9007]).unwrap();
    assert_eq!(result.physical_address, 0x9123);
    assert_eq!(result.page_bytes, 4096);
    assert_eq!(result.remaining_bytes, 4096 - 0x123);
    assert!(!result.writable && !result.user && !result.executable);
    assert!(walk([0x2007, 0x3007, 0x4007, 0x9087]).unwrap().executable); // 4KiB PATbit7.
}
#[test]
fn huge_pages_allow_pat_but_reject_misaligned_address_bits() {
    let g = walk([0x2007, 0x40001087, 0, 0]).unwrap();
    assert_eq!(g.physical_address, 0x40000123);
    assert_eq!(g.page_bytes, 1 << 30);
    let m = walk([0x2007, 0x3007, 0x201087, 0]).unwrap();
    assert_eq!(m.physical_address, 0x200123);
    assert_eq!(m.page_bytes, 1 << 21);
    assert_eq!(walk([0x2007, 0x40003087, 0, 0]), Err(WalkError::ReservedEntry { level: 3 }));
    assert_eq!(walk([0x2007, 0x3007, 0x203087, 0]), Err(WalkError::ReservedEntry { level: 2 }));
    assert_eq!(walk([0x2087, 0, 0, 0]), Err(WalkError::ReservedEntry { level: 4 }));
}

#[test]
fn leaf_pat_index_uses_the_correct_size_bit_and_ignores_parent_cache_controls() {
    for index in 0u64..8 {
        let controls = (index & 3) << 3;
        let pat4k = ((index >> 2) & 1) << 7;
        let pat_large = ((index >> 2) & 1) << 12;
        for parents in [0, 0x18] {
            let small = walk([
                0x2007 | parents,
                0x3007 | parents,
                0x4007 | parents,
                0x9007 | controls | pat4k,
            ])
            .unwrap();
            let medium =
                walk([0x2007 | parents, 0x3007 | parents, 0x200087 | controls | pat_large, 0])
                    .unwrap();
            let large = walk([0x2007 | parents, 0x40000087 | controls | pat_large, 0, 0]).unwrap();
            for mapping in [small, medium, large] {
                assert_eq!(mapping.pat_index, index as u8);
                assert_eq!(mapping.physical_address & 4095, 0x123);
            }
        }
    }
}
#[test]
fn nonpresent_unreadable_and_reserved_inputs_stop_at_exact_level() {
    assert_eq!(walk([0x2007, 0, 0, 0]), Err(WalkError::NotPresent { level: 3 }));
    assert_eq!(
        walk([0x5007, 0, 0, 0]),
        Err(WalkError::UnreadableTable { level: 3, address: 0x5000 })
    );
    assert_eq!(
        walk([0x2007, 0x3007, 0x4007, (1 << 48) | 7]),
        Err(WalkError::ReservedEntry { level: 1 })
    );
    assert_eq!(
        walk([0x2007 | (1 << 52), 0, 0, 0]),
        Err(WalkError::UnsupportedEntryBits { level: 4 })
    );
    // Non-present entries may carry arbitrary software encodings.
    assert_eq!(walk([u64::MAX - 1, 0, 0, 0]), Err(WalkError::NotPresent { level: 4 }));
}
#[test]
fn mode_cr3_and_canonical_checks_precede_reads() {
    for (c, va, error) in [
        (PagingConfig { la57: true, ..config() }, 0, WalkError::FiveLevelUnsupported),
        (config(), 0x800000000000, WalkError::NoncanonicalAddress),
        (PagingConfig { physical_bits: 53, ..config() }, 0, WalkError::UnsupportedPhysicalWidth),
        (PagingConfig { cr3: 0x1001, ..config() }, 0, WalkError::InvalidCr3),
        (PagingConfig { cr3: 1 << 48, ..config() }, 0, WalkError::InvalidCr3),
    ] {
        assert_eq!(translate(c, va, |_| panic!("unexpected read")), Err(error));
    }
    for c in [
        PagingConfig { cr3: 0x1018, ..config() },
        PagingConfig { cr3: 0x1fff, pcid: true, ..config() },
    ] {
        assert_eq!(
            translate(c, 0, |pa| {
                assert_eq!(pa, 0x1000);
                Some(0)
            }),
            Err(WalkError::NotPresent { level: 4 })
        );
    }
    assert_eq!(
        translate(config(), u64::MAX, |pa| {
            assert_eq!(pa, 0x1ff8);
            Some(0)
        }),
        Err(WalkError::NotPresent { level: 4 })
    );
}
#[test]
fn nxe_and_one_gib_support_are_required_when_used() {
    assert_eq!(
        translate(PagingConfig { nxe: false, ..config() }, 0, |_| Some(0x2007 | (1 << 63))),
        Err(WalkError::ReservedEntry { level: 4 })
    );
    assert_eq!(
        translate(PagingConfig { page1gb: false, ..config() }, 0, |pa| Some(if pa == 0x1000 {
            0x2007
        } else {
            0x87
        })),
        Err(WalkError::OneGiBUnsupported)
    );
}

#[test]
fn each_parent_restricts_permissions_and_last_leaf_byte_is_bounded() {
    for index in 0..4 {
        for flag in [2, 4] {
            let mut entries = [0x2007, 0x3007, 0x4007, 0x9007];
            entries[index] &= !flag;
            let t = walk(entries).unwrap();
            assert_eq!(t.writable, flag != 2);
            assert_eq!(t.user, flag != 4);
        }
        let mut entries = [0x2007, 0x3007, 0x4007, 0x9007];
        entries[index] |= 1 << 63;
        assert!(!walk(entries).unwrap().executable);
    }
    let mut reads = Vec::new();
    let t = translate(config(), 0xffff_ffff_ffff_ffff, |pa| {
        reads.push(pa);
        match pa {
            0x1ff8 => Some(0x2007),
            0x2ff8 => Some(0x3007),
            0x3ff8 => Some(0x4007),
            0x4ff8 => Some(0x0000_ffff_ffff_f007),
            _ => None,
        }
    })
    .unwrap();
    assert_eq!(reads, [0x1ff8, 0x2ff8, 0x3ff8, 0x4ff8]);
    assert_eq!(t.physical_address, 0x0000_ffff_ffff_ffff);
    assert_eq!(t.remaining_bytes, 1);
}

#[test]
fn host_admission_still_refuses_every_upper_software_bit_at_every_level() {
    for index in 0..4 {
        for bit in 52..=62 {
            let mut entries = [0x2007, 0x3007, 0x4007, 0x9007];
            entries[index] |= 1 << bit;
            assert_eq!(
                walk(entries),
                Err(WalkError::UnsupportedEntryBits { level: (4 - index) as u8 })
            );
        }
    }
}
