//! Resident private-root layout and host APIC ID admission (host::resident).

use svmvisor_hypervisor::host::resident::{
    CACHE_CAPTURE_OFFSET, CACHE_OWNER_OFFSET, MAX_RESIDENT_CPUS, STARTUP_PAGE_OFFSET,
    X2AVIC_BACKING_ALIASES_OFFSET, X2AVIC_TABLE_OFFSET, valid_pool_slot,
};

#[test]
fn remote_backing_aliases_fill_one_page_per_slot_below_every_shared_page() {
    assert_eq!(X2AVIC_BACKING_ALIASES_OFFSET, 0xd4000);
    let end = X2AVIC_BACKING_ALIASES_OFFSET + MAX_RESIDENT_CPUS as u64 * 4096;
    assert_eq!(end, X2AVIC_TABLE_OFFSET);
    for shared in
        [X2AVIC_TABLE_OFFSET, CACHE_OWNER_OFFSET, CACHE_CAPTURE_OFFSET, STARTUP_PAGE_OFFSET]
    {
        assert!(shared >= end && shared < 0x100000, "{shared:#x}");
    }
    // The linker script cannot name the Rust constant; its literal must agree.
    let script = include_str!("../../resident-payload/payload.ld");
    let bound = format!("ASSERT(image_bss_end <= {:#x},", 0x100000 + X2AVIC_BACKING_ALIASES_OFFSET);
    assert_eq!(script.matches("ASSERT(").count(), 1);
    assert!(script.contains(&bound), "{bound}");
}

#[test]
fn host_apic_ids_satisfy_both_doorbell_formats_and_avoid_entry_255() {
    let pool = 0x200000;
    let bytes = 24 * 0x100000;
    // This machine's 24 IDs, one per dense slot.
    for (slot, id) in (0..=11).chain(16..=27).enumerate() {
        let slot = slot as u64;
        assert!(valid_pool_slot(pool + slot * 0x100000, pool, bytes, slot, id), "id {id}");
    }
    assert!(valid_pool_slot(pool, pool, bytes, 0, 254));
    for id in [255, 256, 511, 4095, u64::MAX] {
        assert!(!valid_pool_slot(pool, pool, bytes, 0, id), "id {id}");
    }
}
