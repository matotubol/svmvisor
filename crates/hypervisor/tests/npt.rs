use svmvisor_hypervisor::address::{AddressError, AddressPolicy, EncryptionState};
use svmvisor_hypervisor::capabilities::EvidenceFlag;
use svmvisor_hypervisor::npt::{
    Npt, NptError as E, NptEvidence, PagePermissions as P, TableStorage, TABLE_COUNT,
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: Some(47),
        },
    )
    .unwrap()
}
fn evidence() -> NptEvidence {
    NptEvidence {
        nx_supported: EvidenceFlag::Set,
        host_nxe: EvidenceFlag::Set,
        host_four_level: EvidenceFlag::Set,
    }
}
fn snapshot(npt: &Npt<'_>) -> Vec<(u64, Vec<u8>)> {
    (0..npt.used_tables())
        .map(|i| {
            let page = npt.table(i).unwrap();
            (page.physical_address, page.bytes.to_vec())
        })
        .collect()
}
fn entry(bytes: &[u8; 4096], index: usize) -> u64 {
    u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap())
}

#[test]
fn exported_walk_permissions_and_default_absence() {
    let mut storage = TableStorage([[0xa5; 4096]; TABLE_COUNT]);
    let mut npt = Npt::new(&mut storage, 0x100000, policy(), 48, evidence()).unwrap();
    assert_eq!(npt.root_address(), 0x100000);
    assert_eq!(npt.used_tables(), 1);
    assert!(npt.table(0).unwrap().bytes.iter().all(|&b| b == 0));
    assert!(npt.table(1).is_none());
    assert_eq!(npt.translate(0), Ok(None));
    for (index, permissions) in [P::ReadExecute, P::ReadWrite, P::ReadOnly]
        .into_iter()
        .enumerate()
    {
        npt.map_page(
            index as u64 * 4096,
            0x200000 + index as u64 * 4096,
            permissions,
        )
        .unwrap();
        let result = npt.translate(index as u64 * 4096 + 4095).unwrap().unwrap();
        assert_eq!(result.host_address, 0x200fff + index as u64 * 4096);
        assert_eq!(result.permissions, permissions);
    }
    assert_eq!(npt.used_tables(), 4);
    for parent in 0..3 {
        assert_eq!(
            entry(npt.table(parent).unwrap().bytes, 0),
            0x101000 + parent as u64 * 4096 | 7
        );
    }
    let leaves = npt.table(3).unwrap();
    assert_eq!(entry(leaves.bytes, 0), 0x200005);
    assert_eq!(entry(leaves.bytes, 1), 0x201007 | (1 << 63));
    assert_eq!(entry(leaves.bytes, 2), 0x202005 | (1 << 63));
    assert_eq!(entry(leaves.bytes, 3), 0);
    assert_eq!(npt.translate(0x3000), Ok(None));
}

#[test]
fn duplicate_alias_overlap_and_address_failures_leave_no_mutation() {
    let mut storage = TableStorage([[0; 4096]; TABLE_COUNT]);
    let mut npt = Npt::new(&mut storage, 0x100000, policy(), 48, evidence()).unwrap();
    npt.map_page(0, 0x200000, P::ReadWrite).unwrap();
    let before = snapshot(&npt);
    for (gpa, hpa, error) in [
        (0, 0x200000, E::AlreadyMapped),
        (0, 0x300000, E::AlreadyMapped),
        (0x1000, 0x200000, E::HostPageAlias),
        (0x1000, 0x107000, E::TableArenaOverlap),
        (0x1000, 0x100000, E::TableArenaOverlap),
        (0x1001, 0x300000, E::GuestAddressMisaligned),
        (1 << 48, 0x300000, E::GuestAddressOutsideWidth),
        (0x1000, 0x300001, E::Address(AddressError::Misaligned)),
        (
            0x1000,
            1 << 48,
            E::Address(AddressError::OutsidePhysicalWidth),
        ),
        (
            0x1000,
            1 << 47,
            E::Address(AddressError::EncryptionBitEncoded),
        ),
        (
            0x1000,
            u64::MAX & !4095,
            E::Address(AddressError::OutsidePhysicalWidth),
        ),
    ] {
        assert_eq!(npt.map_page(gpa, hpa, P::ReadExecute), Err(error));
        assert_eq!(snapshot(&npt), before);
    }
}

#[test]
fn sparse_capacity_preflight_is_atomic_and_shared_paths_still_work() {
    let mut storage = TableStorage([[0; 4096]; TABLE_COUNT]);
    let mut npt = Npt::new(&mut storage, 0x100000, policy(), 48, evidence()).unwrap();
    for index in 0..4 {
        npt.map_page(index << 39, 0x200000 + index * 4096, P::ReadOnly).unwrap();
    }
    npt.map_page(1 << 30, 0x204000, P::ReadOnly).unwrap();
    assert_eq!(npt.used_tables(), TABLE_COUNT - 1);
    let before = snapshot(&npt);
    // Two further tables needed; only one remains. No parent link may leak.
    assert_eq!(
        npt.map_page(2 << 30, 0x205000, P::ReadOnly),
        Err(E::TablesExhausted)
    );
    assert_eq!(snapshot(&npt), before);
    assert_eq!(npt.translate(2 << 30), Ok(None));
    npt.map_page(1 << 21, 0x205000, P::ReadOnly).unwrap();
    assert_eq!(npt.used_tables(), TABLE_COUNT);
    npt.map_page((1 << 21) + 4096, 0x206000, P::ReadOnly)
        .unwrap();
    assert_eq!(npt.used_tables(), TABLE_COUNT);
}

#[test]
fn exact_guest_limit_and_full_table_arena_boundaries() {
    let p = AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let mut storage = TableStorage([[0; 4096]; TABLE_COUNT]);
    let mut npt = Npt::new(&mut storage, (1 << 48) - (TABLE_COUNT * 4096) as u64, p, 48, evidence()).unwrap();
    npt.map_page((1 << 48) - 4096, 0x200000, P::ReadExecute)
        .unwrap();
    assert_eq!(
        npt.translate((1 << 48) - 1).unwrap().unwrap().host_address,
        0x200fff
    );
    assert_eq!(npt.translate(1 << 48), Err(E::GuestAddressOutsideWidth));
    drop(npt);
    assert!(matches!(
        Npt::new(&mut storage, (1 << 48) - ((TABLE_COUNT - 1) * 4096) as u64, p, 48, evidence()),
        Err(E::Address(AddressError::OutsidePhysicalWidth))
    ));
    assert!(matches!(
        Npt::new(&mut storage, u64::MAX & !4095, p, 48, evidence()),
        Err(E::Address(AddressError::Overflow))
    ));
}

#[test]
fn mode_unknowns_and_constructor_errors_preserve_storage() {
    let mut storage = TableStorage([[0xa5; 4096]; TABLE_COUNT]);
    for flag in [EvidenceFlag::Unknown, EvidenceFlag::Clear] {
        for e in [
            NptEvidence {
                nx_supported: flag,
                ..evidence()
            },
            NptEvidence {
                host_nxe: flag,
                ..evidence()
            },
            NptEvidence {
                host_four_level: flag,
                ..evidence()
            },
        ] {
            assert!(matches!(
                Npt::new(&mut storage, 0x100000, policy(), 48, e),
                Err(E::RequiredModeNotEstablished)
            ));
            assert!(storage.0.iter().flatten().all(|&b| b == 0xa5));
        }
    }
    for bits in [0, 31, 49, 64] {
        assert!(matches!(
            Npt::new(&mut storage, 0x100000, policy(), bits, evidence()),
            Err(E::InvalidGuestWidth)
        ));
    }
    assert!(matches!(
        Npt::new(&mut storage, 0x100001, policy(), 48, evidence()),
        Err(E::Address(AddressError::Misaligned))
    ));
    assert!(storage.0.iter().flatten().all(|&b| b == 0xa5));
}
