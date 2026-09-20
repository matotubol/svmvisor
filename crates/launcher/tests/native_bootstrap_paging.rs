#![cfg(feature = "native-preflight")]

use svmvisor_hypervisor::host::paging::{self, PagingConfig, WalkError};
use svmvisor_launcher::native::resident::bootstrap_paging::{BootstrapPaging, Error};

fn config() -> PagingConfig {
    PagingConfig {
        cr3: 0x200000,
        physical_bits: 40,
        la57: false,
        nxe: true,
        pcid: false,
        page1gb: false,
    }
}

#[test]
fn owned_sparse_root_covers_low_code_and_high_pool_without_firmware_reads() {
    let mut tables = BootstrapPaging::empty();
    tables.initialize(config().cr3).unwrap();
    for (page, write, execute) in [
        (0x8000, true, true),
        (0x1ffff000, false, true),
        (0x20000000, true, false),
        (0x8000000000, true, false),
    ] {
        tables.map_page(page, write, execute).unwrap();
        let t = paging::translate(config(), page + 127, |address| tables.read(address)).unwrap();
        assert_eq!(t.physical_address, page + 127);
        assert_eq!(t.page_bytes, 4096);
        assert_eq!((t.writable, t.executable, t.pat_index), (write, execute, 0));
        assert!(!t.user);
    }
    assert!(matches!(
        paging::translate(config(), 0x9000, |a| tables.read(a)),
        Err(WalkError::NotPresent { .. })
    ));
    assert_eq!(tables.read(0x100000), None);
}

#[test]
fn address_and_conflicting_mapping_refusals_preserve_existing_leaf() {
    let mut tables = BootstrapPaging::empty();
    assert_eq!(tables.initialize(1 << 32), Err(Error::Address));
    assert_eq!(tables.initialize(0x200001), Err(Error::Address));
    tables.initialize(config().cr3).unwrap();
    tables.map_page(0x5000, false, true).unwrap();
    assert_eq!(tables.map_page(0x5000, true, false), Err(Error::Conflict));
    assert_eq!(tables.map_page(1 << 40, true, true), Err(Error::Address));
    assert_eq!(tables.map_page(0x5001, true, true), Err(Error::Address));
    let t = paging::translate(config(), 0x5000, |a| tables.read(a)).unwrap();
    assert!(!t.writable && t.executable);
}

#[test]
fn exhausted_table_budget_refuses_without_damaging_published_leaves() {
    let mut tables = BootstrapPaging::empty();
    tables.initialize(config().cr3).unwrap();
    let mut mapped = 0u64;
    // Each leaf is in a different GiB and needs another PD and PT. The
    // construction budget is independent of the supplied physical aperture.
    for index in 0..64u64 {
        match tables.map_page(index << 30, true, false) {
            Ok(()) => mapped += 1,
            Err(Error::Capacity) => break,
            result => panic!("unexpected construction result: {result:?}"),
        }
    }
    assert!(mapped > 0 && mapped < 64);
    for index in 0..mapped {
        assert_eq!(
            paging::translate(config(), index << 30, |a| tables.read(a)).unwrap().physical_address,
            index << 30
        );
    }
}
