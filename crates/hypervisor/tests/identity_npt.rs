use svmvisor_hypervisor::{
    arch::x86_64::capabilities::EvidenceFlag,
    memory::{
        address::{AddressPolicy, EncryptionState},
        npt::*,
    },
};

fn policy(bits: u8) -> AddressPolicy {
    AddressPolicy::new(bits, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}
fn evidence() -> NptEvidence {
    NptEvidence {
        nx_supported: EvidenceFlag::Set,
        host_nxe: EvidenceFlag::Set,
        host_four_level: EvidenceFlag::Set,
    }
}
fn entry(table: &TableView<'_>, index: usize) -> u64 {
    u64::from_le_bytes(table.bytes[index * 8..index * 8 + 8].try_into().unwrap())
}

#[test]
fn all_domain_widths_cap_at_one_tib_and_use_at_most_five_tables() {
    for bits in 32..=52 {
        let p = policy(bits);
        let mut storage = TableStorage([[0xa5; PAGE_BYTES]; TABLE_COUNT]);
        let excluded = p.validate(0x280000, 0x100000, 4096).unwrap();
        let npt = IdentityNpt::new(
            &mut storage,
            0x280000,
            p,
            excluded,
            evidence(),
            EvidenceFlag::Set,
            0x0007040600070406,
        )
        .unwrap();
        let guest_bits = bits.min(40);
        assert_eq!(npt.guest_bits(), guest_bits);
        assert_eq!(npt.root_address(), 0x280000);
        assert_eq!(npt.used_tables(), if guest_bits == 40 { 5 } else { 4 });
        assert_eq!(npt.excluded(), excluded);
        assert!(npt.table(npt.used_tables()).is_none());
        for gpa in [
            0,
            4095,
            0x1fffff,
            0x380000,
            0x3fffff,
            0x400000,
            (1 << 30) - 1,
            1 << 30,
            (1u64 << guest_bits) - 1,
        ] {
            let translated = npt.translate(gpa).unwrap().unwrap();
            assert_eq!(translated.host_address, gpa);
            assert!(translated.writable && translated.executable);
            assert_eq!(translated.pat_index, 0);
        }
        assert_eq!(
            npt.translate(1u64 << guest_bits),
            Err(IdentityNptError::GuestAddressOutsideWidth)
        );
        // Every byte of all eight backing pages is inaccessible as a guest GPA,
        // including unused pages, so no writable alias can reach the tables.
        for i in 0..TABLE_COUNT {
            for offset in [0, 4095] {
                assert_eq!(npt.translate(0x280000 + (i * 4096 + offset) as u64), Ok(None));
            }
        }
        let used = npt.used_tables();
        assert!(storage.0[used..].iter().flatten().all(|&v| v == 0));
    }
}

#[test]
fn exact_split_leaf_edges_preserve_adjacent_native_identity_pages() {
    let p = policy(48);
    for (base, len) in
        [(0x200000, 0x10000), (0x3f0000, 0x10000), (0x280000, 0x100000), (0x8000000000, 0x10000)]
    {
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let excluded = p.validate(base, len, 4096).unwrap();
        let npt =
            IdentityNpt::new(&mut storage, base, p, excluded, evidence(), EvidenceFlag::Set, 6)
                .unwrap();
        for page in 0..len / 4096 {
            assert_eq!(npt.translate(base + page * 4096), Ok(None));
            assert_eq!(npt.translate(base + page * 4096 + 4095), Ok(None));
        }
        for address in [base - 1, base + len] {
            let t = npt.translate(address).unwrap().unwrap();
            assert_eq!(t.host_address, address);
            let same_2m = address >> 21 == base >> 21;
            let same_1g = address >> 30 == base >> 30;
            assert_eq!(
                t.page_bytes,
                if same_2m {
                    4096
                } else if same_1g {
                    1 << 21
                } else {
                    1 << 30
                }
            );
        }
        let gib_other = (base & !((1 << 30) - 1)) ^ (1 << 30);
        assert_eq!(npt.translate(gib_other).unwrap().unwrap().page_bytes, 1 << 30);
    }
}

#[test]
fn serialized_parent_and_leaf_flags_select_pat_zero_and_identity() {
    let p = policy(48);
    let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
    let base = 0x8000280000;
    let excluded = p.validate(base, 0x10000, 4096).unwrap();
    let npt = IdentityNpt::new(&mut storage, base, p, excluded, evidence(), EvidenceFlag::Set, 6)
        .unwrap();
    assert_eq!(entry(&npt.table(0).unwrap(), 0), (base + 4096) | 7);
    assert_eq!(entry(&npt.table(0).unwrap(), 1), (base + 8192) | 7);
    assert_eq!(entry(&npt.table(0).unwrap(), 2), 0);
    // Exclusion resides under second PML4 slot, first 1GiB leaf, second 2MiB.
    assert_eq!(entry(&npt.table(1).unwrap(), 0), 0x87);
    assert_eq!(entry(&npt.table(2).unwrap(), 0), (base + 12288) | 7);
    assert_eq!(entry(&npt.table(2).unwrap(), 1), 0x8040000000 | 0x87);
    assert_eq!(entry(&npt.table(3).unwrap(), 0), 0x8000000000 | 0x87);
    assert_eq!(entry(&npt.table(3).unwrap(), 1), (base + 16384) | 7);
    assert_eq!(entry(&npt.table(4).unwrap(), 127), 0x800027f000 | 7);
    for i in 128..144 {
        assert_eq!(entry(&npt.table(4).unwrap(), i), 0);
    }
    assert_eq!(entry(&npt.table(4).unwrap(), 144), 0x8000290000 | 7);
}

#[test]
fn every_admission_refusal_leaves_all_table_storage_unchanged() {
    let p = policy(48);
    for (base, len, table_base, one_gib, pat, expected) in [
        (
            0x280000,
            0x100000,
            0x280000,
            EvidenceFlag::Clear,
            6,
            IdentityNptError::OneGiBPagesNotEstablished,
        ),
        (
            0x280000,
            0x100000,
            0x280000,
            EvidenceFlag::Unknown,
            6,
            IdentityNptError::OneGiBPagesNotEstablished,
        ),
        (0x280000, 0x100000, 0x280000, EvidenceFlag::Set, 0, IdentityNptError::PatZeroNotWriteBack),
        (
            0x280000,
            0x100000,
            0x280000,
            EvidenceFlag::Set,
            0x86,
            IdentityNptError::PatZeroNotWriteBack,
        ),
        (
            0x8000280000,
            0x101000,
            0x8000280000,
            EvidenceFlag::Set,
            6,
            IdentityNptError::InvalidExclusion,
        ),
        (
            0x80003f0000,
            0x20000,
            0x80003f0000,
            EvidenceFlag::Set,
            6,
            IdentityNptError::InvalidExclusion,
        ),
        (0x280000, 0x8001, 0x280000, EvidenceFlag::Set, 6, IdentityNptError::InvalidExclusion),
        (
            0x280000,
            0x100000,
            0x27f000,
            EvidenceFlag::Set,
            6,
            IdentityNptError::TableArenaOutsideExclusion,
        ),
        (
            0x280000,
            0x100000,
            0x379000,
            EvidenceFlag::Set,
            6,
            IdentityNptError::TableArenaOutsideExclusion,
        ),
        (
            0x280000,
            0x7000,
            0x280000,
            EvidenceFlag::Set,
            6,
            IdentityNptError::TableArenaOutsideExclusion,
        ),
        (1u64 << 40, 0x8000, 1u64 << 40, EvidenceFlag::Set, 6, IdentityNptError::InvalidExclusion),
    ] {
        let mut storage = TableStorage([[0xa5; PAGE_BYTES]; TABLE_COUNT]);
        let excluded = p.validate(base, len, 1).unwrap();
        assert_eq!(
            IdentityNpt::new(&mut storage, table_base, p, excluded, evidence(), one_gib, pat).err(),
            Some(expected)
        );
        assert!(storage.0.iter().flatten().all(|&b| b == 0xa5));
    }
}

#[test]
fn contiguous_cpu_pools_exclude_every_copy_with_four_tables() {
    let p = policy(48);
    for cpus in [2, 24, 32] {
        let base = 0x200000;
        let len = cpus * 0x100000;
        let excluded = p.validate(base, len, 4096).unwrap();
        // Each CPU's private NPT can reside anywhere inside the shared pool.
        for cpu in [0, cpus / 2, cpus - 1] {
            let table_base = base + cpu * 0x100000 + 0x20000;
            let mut storage = TableStorage([[0xa5; PAGE_BYTES]; TABLE_COUNT]);
            let npt = IdentityNpt::new(
                &mut storage,
                table_base,
                p,
                excluded,
                evidence(),
                EvidenceFlag::Set,
                6,
            )
            .unwrap();
            assert_eq!(npt.root_address(), table_base);
            assert_eq!(npt.excluded(), excluded);
            assert_eq!(npt.used_tables(), 4);
            for page in (base..base + len).step_by(4096) {
                assert_eq!(npt.translate(page), Ok(None));
                assert_eq!(npt.translate(page + 4095), Ok(None));
            }
            for address in [0, base - 1, base + len, (1 << 30) - 1, 1 << 30] {
                let t = npt.translate(address).unwrap().unwrap();
                assert_eq!(t.host_address, address);
                assert!(t.writable && t.executable);
                assert_eq!(t.pat_index, 0);
            }
            // Every excluded PD entry is absent; there are no endpoint PTs.
            for index in 1..1 + cpus as usize / 2 {
                assert_eq!(entry(&npt.table(3).unwrap(), index), 0);
            }
            assert!(npt.table(4).is_none());
            assert!(storage.0[4..].iter().flatten().all(|&byte| byte == 0));
        }
    }
}

#[test]
fn endpoint_tables_preserve_every_neighbor_page_and_stay_within_capacity() {
    // Start/end on 2MiB boundaries need no PT; partial endpoints need one or
    // two, including when both endpoints share the same 2MiB page.
    for (base, len, pts) in [
        (0x200000, 0x100000, 1),
        (0x280000, 0x180000, 1),
        (0x200000, 0x600000, 0),
        (0x200000, 0x601000, 1),
        (0x201000, 0x5ff000, 1),
        (0x201000, 0x600000, 2),
        (0x3f0000, 0x20000, 2),
        (0x280000, 0x101000, 1),
        (0x201000, 32 << 20, 2),
        (0x3e000000, 32 << 20, 0),
    ] {
        for bits in [32, 40, 52] {
            let p = policy(bits);
            let excluded = p.validate(base, len, 4096).unwrap();
            let mut storage = TableStorage([[0xa5; PAGE_BYTES]; TABLE_COUNT]);
            let npt =
                IdentityNpt::new(&mut storage, base, p, excluded, evidence(), EvidenceFlag::Set, 6)
                    .unwrap();
            let used = if bits < 40 { 3 } else { 4 } + pts;
            assert_eq!(npt.used_tables(), used);
            // `IdentityNpt::new` never needs more than 6 of the TABLE_COUNT tables.
            assert!(used <= 6);
            let start = (base & !0x1fffff) - 4096;
            let end = ((base + len + 0x1fffff) & !0x1fffff) + 4096;
            for page in (start..end).step_by(4096) {
                for address in [page, page + 4095] {
                    let t = npt.translate(address).unwrap();
                    if (base..base + len).contains(&address) {
                        assert_eq!(t, None);
                    } else {
                        let t = t.unwrap();
                        assert_eq!(t.host_address, address);
                        assert!(t.writable && t.executable);
                        assert_eq!(t.pat_index, 0);
                    }
                }
            }
            assert!(storage.0[used..].iter().flatten().all(|&byte| byte == 0));
        }
    }
}

#[test]
fn oversized_out_of_profile_and_unowned_pool_tables_refuse_without_writes() {
    let p = policy(48);
    for (base, len, table_base, expected) in [
        (0x200000, (32 << 20) + 4096, 0x200000, IdentityNptError::InvalidExclusion),
        (0x3ff00000, 0x200000, 0x3ff00000, IdentityNptError::InvalidExclusion),
        (0x40000000, 0x200000, 0x40000000, IdentityNptError::InvalidExclusion),
        (0x201000, 0x400001, 0x201000, IdentityNptError::InvalidExclusion),
        (0x3ffff000, 0x1000, 0x3ffff000, IdentityNptError::TableArenaOutsideExclusion),
        (0x200000, 32 << 20, 0x1ff000, IdentityNptError::TableArenaOutsideExclusion),
        (0x200000, 32 << 20, 0x21f9000, IdentityNptError::TableArenaOutsideExclusion),
    ] {
        let excluded = p.validate(base, len, 4096).unwrap();
        let mut storage = TableStorage([[0xa5; PAGE_BYTES]; TABLE_COUNT]);
        assert_eq!(
            IdentityNpt::new(
                &mut storage,
                table_base,
                p,
                excluded,
                evidence(),
                EvidenceFlag::Set,
                6,
            )
            .err(),
            Some(expected),
        );
        assert!(storage.0.iter().flatten().all(|&byte| byte == 0xa5));
    }
}

#[test]
fn missing_mode_and_encryption_address_bit_are_not_inferred_safe() {
    let p = policy(48);
    let excluded = p.validate(0x280000, 0x8000, 4096).unwrap();
    for index in 0..3 {
        let mut e = evidence();
        match index {
            0 => e.host_four_level = EvidenceFlag::Unknown,
            1 => e.host_nxe = EvidenceFlag::Clear,
            _ => e.nx_supported = EvidenceFlag::Clear,
        }
        let mut storage = TableStorage([[0x5a; PAGE_BYTES]; TABLE_COUNT]);
        assert_eq!(
            IdentityNpt::new(&mut storage, 0x280000, p, excluded, e, EvidenceFlag::Set, 6).err(),
            Some(IdentityNptError::RequiredModeNotEstablished)
        );
        assert!(storage.0.iter().flatten().all(|&b| b == 0x5a));
    }
    let encrypted_bit =
        AddressPolicy::new(40, EncryptionState::Unencrypted { encryption_bit: Some(39) }).unwrap();
    let mut storage = TableStorage([[0x5a; PAGE_BYTES]; TABLE_COUNT]);
    assert!(matches!(
        IdentityNpt::new(
            &mut storage,
            0x280000,
            encrypted_bit,
            excluded,
            evidence(),
            EvidenceFlag::Set,
            6
        ),
        Err(IdentityNptError::Address(_))
    ));
    assert!(storage.0.iter().flatten().all(|&b| b == 0x5a));
}
#[test]
fn ecam_aperture_write_guard_and_restore_preserve_all_neighbor_mappings() {
    for (base, bytes) in [
        (0xe0000000, 0x10000000),
        (0xf0000000, 0x1000000),
        (0x90000000, 0x10000000),
        (0xf0100000, 0x100000),
    ] {
        let p = policy(48);
        let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
        let excluded = p.validate(0x200000, 0x1800000, 4096).unwrap();
        let mut n = IdentityNpt::new(
            &mut storage,
            0x200000,
            p,
            excluded,
            evidence(),
            EvidenceFlag::Set,
            0x0007040600070406,
        )
        .unwrap();
        n.protect_write_range(base, bytes).unwrap();
        let (start, end) = identity_protection_range(base, bytes).unwrap();
        for a in (start..end).step_by(4096) {
            let t = n.translate(a).unwrap().unwrap();
            assert!(!t.writable);
            assert_eq!(t.host_address, a);
            assert!(t.executable);
            assert_eq!(t.pat_index, 0);
        }
        assert!(n.translate(0xfee00000).unwrap().unwrap().writable);
        for a in [start - 4096, end] {
            assert!(n.translate(a).unwrap().unwrap().writable);
        }
        let used = n.used_tables();
        let before = storage.0;
        restore_identity_write_range(&mut storage, 0x200000, base, bytes).unwrap();
        let mut changed = 0;
        for (before, after) in before.iter().zip(storage.0.iter()).take(used) {
            for i in 0..512 {
                let o = i * 8;
                let b = u64::from_le_bytes(before[o..o + 8].try_into().unwrap());
                let a = u64::from_le_bytes(after[o..o + 8].try_into().unwrap());
                if a != b {
                    assert_eq!(a, b | 2);
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, (end - start) >> 21);
        let restored = storage.0;
        restore_identity_write_range(&mut storage, 0x200000, base, bytes).unwrap();
        assert_eq!(storage.0, restored);
    }
}
#[test]
fn ecam_guard_refusal_and_restore_corruption_are_transactional() {
    let p = policy(48);
    let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
    let excluded = p.validate(0x200000, 0x1800000, 4096).unwrap();
    let mut n = IdentityNpt::new(
        &mut storage,
        0x200000,
        p,
        excluded,
        evidence(),
        EvidenceFlag::Set,
        0x0007040600070406,
    )
    .unwrap();
    for (base, bytes) in
        [(0x100000, 0x100000), (0xe0000001, 0x100000), (0xfff00000, 0x200000), (0xe0000000, 0)]
    {
        let before: Vec<_> =
            (0..n.used_tables()).map(|i| n.table(i).unwrap().bytes.to_vec()).collect();
        assert!(n.protect_write_range(base, bytes).is_err());
        assert_eq!(
            before,
            (0..n.used_tables()).map(|i| n.table(i).unwrap().bytes.to_vec()).collect::<Vec<_>>()
        );
    }
    n.protect_write_range(0xe0000000, 0x10000000).unwrap();
    storage.0[0][0] = 0;
    let before = storage.0;
    assert!(restore_identity_write_range(&mut storage, 0x200000, 0xe0000000, 0x10000000).is_err());
    assert_eq!(storage.0, before);
}
