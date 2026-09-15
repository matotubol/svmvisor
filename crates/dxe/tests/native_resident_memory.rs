#![cfg(feature = "native-preflight")]
use svmvisor_dxe::native::resident::memory::{ResidentMemoryError as E, prepare_identity_npt};
use svmvisor_hypervisor::{
    boot::memory::{MemoryDescriptor as D, MemoryError},
    capabilities::EvidenceFlag as F,
    memory::{
        address::{AddressPolicy, EncryptionState},
        npt::{NptEvidence, TableStorage},
    },
};
const RUNTIME: u64 = 1 << 63;
fn policy() -> AddressPolicy {
    AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap()
}
fn descriptor(start: u64, pages: u64, ty: u32, attributes: u64) -> D {
    D {
        memory_type: ty,
        physical_start: start,
        page_count: pages,
        attributes,
    }
}
fn evidence() -> NptEvidence {
    NptEvidence {
        nx_supported: F::Set,
        host_nxe: F::Set,
        host_four_level: F::Set,
    }
}

#[test]
fn startup_hole_is_reapplied_on_final_callback_rebuild() {
    let p = policy();
    let monitor = p.validate(0x200000, 0x200000, 4096).unwrap();
    let map = [descriptor(0x200000, 0x200, 5, RUNTIME | 8)];
    let mut storage = TableStorage([[0xa5; 4096]; 8]);
    // Exercise the actual shared builder used both before publication and in
    // callback capture. A preceding ordinary image does not carry its policy.
    for startup in [false, true, true] {
        let npt = prepare_identity_npt(
            &mut storage,
            0x200000,
            p,
            monitor,
            &map,
            evidence(),
            F::Set,
            6,
            startup,
        )
        .unwrap();
        assert_eq!(npt.translate(0xfee00000).unwrap().is_none(), startup);
        assert_eq!(
            npt.translate(0xfee01000).unwrap().unwrap().host_address,
            0xfee01000
        );
        assert_eq!(npt.translate(0x200000).unwrap(), None);
    }
}

#[test]
fn startup_aperture_plan_refuses_before_storage_mutation() {
    let p = policy();
    let monitor = p.validate(0xfee00000, 0x100000, 4096).unwrap();
    let map = [descriptor(0xfee00000, 0x100, 5, RUNTIME | 8)];
    let mut storage = TableStorage([[0xa5; 4096]; 8]);
    assert!(
        prepare_identity_npt(
            &mut storage,
            0xfee00000,
            p,
            monitor,
            &map,
            evidence(),
            F::Set,
            6,
            true
        )
        .is_err()
    );
    assert!(storage.0.iter().flatten().all(|byte| *byte == 0xa5));
}

#[test]
fn map_consumed_by_native_preparation_retains_code_and_data_and_hides_monitor() {
    let p = policy();
    let monitor = p.validate(0x280000, 0x100000, 4096).unwrap();
    let map = [
        descriptor(0, 0x280, 7, 8),
        descriptor(0x280000, 0x80, 5, RUNTIME | 8),
        descriptor(0x300000, 0x80, 6, RUNTIME | 8),
        descriptor(0x380000, 0x100, 7, 8),
    ];
    let mut storage = TableStorage([[0xa5; 4096]; 8]);
    let npt = prepare_identity_npt(
        &mut storage,
        0x300000,
        p,
        monitor,
        &map,
        evidence(),
        F::Set,
        6,
        false,
    )
    .unwrap();
    assert_eq!(
        npt.translate(0x27ffff).unwrap().unwrap().host_address,
        0x27ffff
    );
    for address in (monitor.base()..=monitor.last_byte()).step_by(4096) {
        assert_eq!(npt.translate(address).unwrap(), None);
    }
    assert_eq!(
        npt.translate(0x380000).unwrap().unwrap().host_address,
        0x380000
    );
}

#[test]
fn loader_bootservices_reserved_or_missing_runtime_bit_cannot_stand_in_for_ownership() {
    let p = policy();
    let monitor = p.validate(0x280000, 0x100000, 4096).unwrap();
    for (ty, attributes, expected) in [
        (0, RUNTIME | 8, E::MonitorNotRuntime),
        (1, RUNTIME | 8, E::MonitorNotRuntime),
        (2, RUNTIME | 8, E::MonitorNotRuntime),
        (3, RUNTIME | 8, E::MonitorNotRuntime),
        (4, RUNTIME | 8, E::MonitorNotRuntime),
        (7, RUNTIME | 8, E::MonitorNotRuntime),
        (5, 8, E::MonitorNotRuntime),
        (6, RUNTIME, E::MonitorNotWriteBack),
    ] {
        let map = [descriptor(0x280000, 0x100, ty, attributes)];
        let mut storage = TableStorage([[0xa5; 4096]; 8]);
        let result = prepare_identity_npt(
            &mut storage,
            0x280000,
            p,
            monitor,
            &map,
            evidence(),
            F::Set,
            6,
            false,
        );
        assert!(matches!(result, Err(e) if e == expected));
        assert!(storage.0.iter().flatten().all(|b| *b == 0xa5));
    }
}

#[test]
fn gap_overlap_and_any_descriptor_above_aperture_refuse_without_writes() {
    let p = policy();
    let monitor = p.validate(0x280000, 0x100000, 4096).unwrap();
    let cases = [
        (
            vec![
                descriptor(0x280000, 0x80, 5, RUNTIME | 8),
                descriptor(0x301000, 0x7f, 6, RUNTIME | 8),
            ],
            E::MonitorUncovered,
        ),
        (
            vec![
                descriptor(0x280000, 0x100, 5, RUNTIME | 8),
                descriptor(0x300000, 0x80, 6, RUNTIME | 8),
            ],
            E::Map(MemoryError::UnsortedOrOverlapping),
        ),
        (
            vec![
                descriptor(0x280000, 0x100, 5, RUNTIME | 8),
                descriptor(1 << 40, 1, 11, 1),
            ],
            E::Map(MemoryError::OutsidePhysicalWidth),
        ),
    ];
    for (map, expected) in cases {
        let mut storage = TableStorage([[0xa5; 4096]; 8]);
        let result = prepare_identity_npt(
            &mut storage,
            0x280000,
            p,
            monitor,
            &map,
            evidence(),
            F::Set,
            6,
            false,
        );
        assert!(matches!(result, Err(e) if e == expected));
        assert!(storage.0.iter().flatten().all(|b| *b == 0xa5));
    }
}
