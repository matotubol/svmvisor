//! AVIC_INCOMPLETE_IPI policy, x2APIC target sets and software fan-out
//! (phase B D5/D8).
use std::cell::RefCell;
use svmvisor_hypervisor::{
    arch::x86_64::{apic::{self, DoorbellTarget}, msr},
    svm::x2avic::{
        BackingPage, Error, GUEST_APIC_VERSION,
        ipi::{FanOutError, IpiAction, IpiDrop, IpiRefusal as R},
        startup::{NativeIcr, NativeIcrError},
    },
};

/// Enabled x2APIC IDs of the captured MADT in table order: primary threads,
/// then second threads (UIDs 0-23 -> {0-11, 16-27}).
const MADT_IDS: [u32; 24] = [
    0x00, 0x02, 0x04, 0x06, 0x08, 0x0a, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1a,
    0x01, 0x03, 0x05, 0x07, 0x09, 0x0b, 0x11, 0x13, 0x15, 0x17, 0x19, 0x1b,
];

fn owner(source: u32, ids: &[u32]) -> NativeIcr {
    NativeIcr::admit(source, ids).unwrap()
}

/// Fixed edge ICR with the given shorthand, mode and destination.
fn fixed(vector: u8, shorthand: u64, logical: bool, destination: u32) -> u64 {
    (u64::from(destination) << 32) | (shorthand << 18) | (u64::from(logical) << 11) | u64::from(vector)
}

/// Slot mask of the wanted IDs in `ids`.
fn slots(ids: &[u32], wanted: &[u32]) -> u32 {
    wanted.iter().fold(0, |mask, id| mask | 1 << ids.iter().position(|x| x == id).unwrap())
}

/// Target mask of a fixed-IPI classification with vector 55h (ID 2).
fn fan(owner: &NativeIcr, icr: u64) -> u32 {
    match owner.inventory().classify(icr, 2) {
        Ok(IpiAction::Fixed(ipi)) => {
            assert_eq!(ipi.vector(), 0x55);
            assert_ne!(ipi.targets(), 0);
            ipi.targets()
        }
        other => panic!("{icr:#x}: {other:?}"),
    }
}

fn dropped(owner: &NativeIcr, icr: u64) -> IpiDrop {
    match owner.inventory().classify(icr, 2) {
        Ok(IpiAction::Dropped(reason)) => reason,
        other => panic!("{icr:#x}: {other:?}"),
    }
}

#[test]
fn every_incomplete_ipi_id_has_exactly_one_policy() {
    let owner = owner(0x13, &MADT_IDS);
    let inventory = owner.inventory();
    let edge = fixed(0x55, 0, false, 0x1b);
    let init = 0x0000_001b_0000_0500;
    let sipi = 0x0000_001b_0000_069a;
    let one = slots(&MADT_IDS, &[0x1b]);
    let is_fixed = |result: Result<IpiAction, R>, targets: u32| {
        matches!(result, Ok(IpiAction::Fixed(ipi)) if ipi.targets() == targets && ipi.vector() == 0x55)
    };
    // ID 0: routed by message type.
    assert_eq!(inventory.classify(init, 0), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(sipi, 0), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(0x000c_0500, 0), Ok(IpiAction::Startup));
    // INIT level assert (Table 16-4) and deassert (compatibility no-op) belong
    // to the startup router, as do its shorthand refusals.
    assert_eq!(inventory.classify(init | 0xc000, 0), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(init | 0x8000, 0), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(0x0004_0500, 0), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(edge | 0x8000, 0), Err(R::LevelTriggered));
    assert_eq!(inventory.classify(edge | 0xc000, 0), Err(R::LevelTriggered));
    assert_eq!(inventory.classify(edge | 0x0400, 0), Err(R::Nmi));
    assert_eq!(inventory.classify(edge | 0x0200, 0), Err(R::Smi));
    for message in [1u64, 3, 7] {
        assert_eq!(inventory.classify(edge | (message << 8), 0), Err(R::ReservedMessageType));
        assert_eq!(inventory.classify(edge | (message << 8), 4), Err(R::ReservedMessageType));
    }
    assert!(is_fixed(inventory.classify(edge, 0), one));
    // Deassert level on an edge IPI is a don't-care (Table 16-4).
    assert!(is_fixed(inventory.classify(edge | 0x4000, 0), one));
    // ID 1: hardware already delivered to every valid target.
    assert_eq!(inventory.classify(edge, 1), Err(R::TargetNotRunning));
    assert_eq!(inventory.classify(init, 1), Err(R::TargetNotRunning));
    // ID 2: nothing was written; full software handling by message type.
    assert!(is_fixed(inventory.classify(edge, 2), one));
    assert_eq!(inventory.classify(init, 2), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(sipi, 2), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(edge | 0x0400, 2), Err(R::Nmi));
    assert_eq!(inventory.classify(edge | 0x8000, 2), Err(R::LevelTriggered));
    // ID 3: invalid backing page.
    assert_eq!(inventory.classify(edge, 3), Err(R::InvalidBackingPage));
    // ID 4: INIT/STARTUP take the startup router (as for IDs 0 and 2); a
    // fixed IPI is consistent only with a vector below 16.
    for vector in 0..16u8 {
        assert_eq!(inventory.classify(fixed(vector, 0, false, 0x1b), 4),
            Ok(IpiAction::Dropped(IpiDrop::IllegalVector)));
        assert_eq!(inventory.classify(fixed(vector, 3, true, 0) | 0x8000, 4),
            Ok(IpiAction::Dropped(IpiDrop::IllegalVector)));
    }
    assert_eq!(inventory.classify(edge, 4), Err(R::InconsistentVectorExit));
    assert_eq!(inventory.classify(init, 4), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(sipi & !0xff | 0x09, 4), Ok(IpiAction::Startup));
    assert_eq!(inventory.classify(0x0400, 4), Err(R::InconsistentVectorExit));
    // ID 5 (Secure AVIC) and reserved IDs.
    for reason in [5, 6, 0x100, u32::MAX] {
        assert_eq!(inventory.classify(edge, reason), Err(R::UnknownReason));
        assert_eq!(inventory.classify(u64::MAX, reason), Err(R::UnknownReason));
    }
    // Fan-out drops illegal vectors for IDs 0 and 2 as well.
    for reason in [0, 2] {
        assert_eq!(inventory.classify(fixed(15, 0, false, 0x1b), reason),
            Ok(IpiAction::Dropped(IpiDrop::IllegalVector)));
        assert!(matches!(inventory.classify(fixed(16, 0, false, 0x1b), reason),
            Ok(IpiAction::Fixed(ipi)) if ipi.vector() == 16 && ipi.targets() == one));
    }
}

#[test]
fn reserved_icr_bits_are_refused_after_completion() {
    let owner = owner(0x13, &MADT_IDS);
    let inventory = owner.inventory();
    let base = fixed(0x55, 0, false, 0x1b);
    for bit in 0..32u64 {
        let reserved = (20..32).contains(&bit) || matches!(bit, 12 | 13 | 16 | 17);
        for reason in [0, 2, 4] {
            let result = inventory.classify(base | (1 << bit), reason);
            if reserved {
                assert_eq!(result, Err(R::ReservedBits), "bit {bit} id {reason}");
            } else {
                assert_ne!(result, Err(R::ReservedBits), "bit {bit} id {reason}");
            }
        }
    }
    // The destination half is never reserved.
    for bit in 32..64u64 {
        assert_ne!(inventory.classify(base | (1 << bit), 0), Err(R::ReservedBits));
    }
    // Reserved bits are reported before the message type.
    assert_eq!(inventory.classify(0x0010_0500, 0), Err(R::ReservedBits));
    assert_eq!(inventory.classify(0x0000_1700, 0), Err(R::ReservedBits));
}

#[test]
fn physical_and_broadcast_destinations_on_the_captured_24_cpu_inventory() {
    let owner = owner(0x13, &MADT_IDS);
    for (destination, wanted) in [
        (0x1bu32, &[0x1b][..]),
        (0x00, &[0x00]),
        (0x10, &[0x10]),
        // The source itself is a valid physical destination.
        (0x13, &[0x13]),
        (u32::MAX, &MADT_IDS[..]),
    ] {
        assert_eq!(fan(&owner, fixed(0x55, 0, false, destination)), slots(&MADT_IDS, wanted),
            "{destination:#x}");
    }
    assert_eq!(fan(&owner, fixed(0x55, 0, false, u32::MAX)), (1 << 24) - 1);
    // Unassigned IDs, including x2APIC-mode FFh (no broadcast), reach nobody.
    for destination in [0x0c, 0x0f, 0x1c, 0xff, 0x100, 0x1_0000, 0xffff_fffe] {
        assert_eq!(dropped(&owner, fixed(0x55, 0, false, destination)), IpiDrop::NoTarget,
            "{destination:#x}");
    }
}

#[test]
fn logical_cluster_destinations_on_the_captured_24_cpu_inventory() {
    let owner = owner(0x13, &MADT_IDS);
    let low: Vec<u32> = (0..12).collect();
    let high: Vec<u32> = (16..28).collect();
    for (destination, wanted) in [
        (0x0000_ffffu32, &low[..]),
        (0x0001_ffff, &high[..]),
        (0x0000_0020, &[0x05][..]),
        (0x0000_0220, &[0x05, 0x09]),
        (0x0001_0001, &[0x10]),
        // The source matches its own logical ID.
        (0x0001_0008, &[0x13]),
        (0x0001_0800, &[0x1b]),
        (0x0001_0f0f, &[0x10, 0x11, 0x12, 0x13, 0x18, 0x19, 0x1a, 0x1b]),
        (u32::MAX, &MADT_IDS[..]),
    ] {
        assert_eq!(fan(&owner, fixed(0x55, 0, true, destination)), slots(&MADT_IDS, wanted),
            "{destination:#x}");
    }
    for destination in [
        0x0000_0000u32, // no logical ID bit
        0x0000_f000,    // IDs 12-15 do not exist
        0x0001_f000,    // IDs 28-31 do not exist
        0x0002_ffff,    // cluster 2 is empty
        0x0100_0001,    // cluster mismatch
        0xffff_fffe,    // cluster FFFFh, not broadcast
    ] {
        assert_eq!(dropped(&owner, fixed(0x55, 0, true, destination)), IpiDrop::NoTarget,
            "{destination:#x}");
    }
}

#[test]
fn logical_matching_uses_the_x2apic_ldr_formula_of_every_admitted_id() {
    let ids = [0x000, 0x01f, 0x020, 0x123, 0x1ff];
    let owner = owner(0x020, &ids);
    // 16.14 p662: cluster = ID[19:4], logical = 1 << ID[3:0]; the backing
    // page presents the same LDR.
    for (id, ldr) in [(0x000u32, 0x0000_0001u32), (0x01f, 0x0001_8000), (0x020, 0x0002_0001),
        (0x123, 0x0012_0008), (0x1ff, 0x001f_8000)] {
        let mut page = BackingPage::new();
        page.reset_stopped(id, GUEST_APIC_VERSION).unwrap();
        assert_eq!(page.read_register(apic::LDR).unwrap(), ldr);
        assert_eq!(fan(&owner, fixed(0x55, 0, true, ldr)), slots(&ids, &[id]), "{id:#x}");
    }
    // Clusters 1 and 1Fh both use logical bit 15.
    assert_eq!(fan(&owner, fixed(0x55, 0, true, 0x0001_ffff)), slots(&ids, &[0x01f]));
    for destination in [0x0012_0004u32, 0x0013_0008, 0x0002_0002, 0x001f_7fff] {
        assert_eq!(dropped(&owner, fixed(0x55, 0, true, destination)), IpiDrop::NoTarget,
            "{destination:#x}");
    }
}

#[test]
fn shorthands_ignore_destination_and_mode_and_handle_self() {
    let owner = owner(0x13, &MADT_IDS);
    let others: Vec<u32> = MADT_IDS.iter().copied().filter(|id| *id != 0x13).collect();
    for logical in [false, true] {
        for destination in [0, 0x1b, 0xff, u32::MAX] {
            assert_eq!(fan(&owner, fixed(0x55, 1, logical, destination)), slots(&MADT_IDS, &[0x13]));
            assert_eq!(fan(&owner, fixed(0x55, 2, logical, destination)), slots(&MADT_IDS, &MADT_IDS));
            assert_eq!(fan(&owner, fixed(0x55, 3, logical, destination)), slots(&MADT_IDS, &others));
        }
    }
    // A single-CPU inventory has no other CPU for shorthand 11.
    let alone = self::owner(7, &[7]);
    assert_eq!(dropped(&alone, fixed(0x55, 3, false, 0)), IpiDrop::NoTarget);
    assert_eq!(fan(&alone, fixed(0x55, 2, false, 0)), 1);
    // Illegal vectors are dropped before any target selection.
    assert_eq!(dropped(&owner, fixed(3, 2, false, 0)), IpiDrop::IllegalVector);
    assert_eq!(dropped(&alone, fixed(0, 3, false, 0)), IpiDrop::IllegalVector);
}

/// Reset backing pages, software-enabled (SVR 1FFh) as a running guest's.
fn pages(ids: &[u32]) -> Vec<BackingPage> {
    ids.iter().map(|&id| {
        let mut page = BackingPage::new();
        page.reset_stopped(id.min(511), GUEST_APIC_VERSION).unwrap();
        page.write_register_stopped(apic::SVR, 0x1ff).unwrap();
        page
    }).collect()
}

fn set_bit(page: &BackingPage, base: u16, vector: u8) {
    let bank = base + u16::from(vector / 32) * 16;
    let bits = page.read_register(bank).unwrap();
    page.write_register_stopped(bank, bits | (1 << (vector % 32))).unwrap();
}

fn fixed_ipi(owner: &NativeIcr, icr: u64) -> svmvisor_hypervisor::svm::x2avic::ipi::FixedIpi {
    match owner.inventory().classify(icr, 0) {
        Ok(IpiAction::Fixed(ipi)) => ipi,
        other => panic!("{icr:#x}: {other:?}"),
    }
}

#[test]
fn fan_out_publishes_edge_irr_clears_tmr_and_doorbells_only_remote_targets() {
    let ids = [4, 9, 200];
    let owner = owner(9, &ids);
    let pages = pages(&ids);
    set_bit(&pages[0], apic::TMR, 0x55); // stale level metadata
    let ipi = fixed_ipi(&owner, fixed(0x55, 2, false, 0));
    assert_eq!(ipi.targets(), 0b111);
    let resolved = RefCell::new(Vec::new());
    let mut rung = Vec::new();
    owner.inventory().deliver_fixed(ipi, |slot| {
        resolved.borrow_mut().push(slot);
        &pages[slot]
    }, |target| rung.push(target.apic_id())).unwrap();
    assert_eq!(*resolved.borrow(), [0, 1, 2]);
    assert_eq!(rung, [4, 200]);
    for page in &pages {
        assert!(page.is_pending(0x55) && !page.is_level(0x55));
    }
    // A second delivery coalesces in IRR and rings again.
    owner.inventory().deliver_fixed(ipi, |slot| &pages[slot], |target| rung.push(target.apic_id())).unwrap();
    assert_eq!(rung, [4, 200, 4, 200]);
    // A self-only IPI publishes locally and rings nobody.
    let own = fixed_ipi(&owner, fixed(0x66, 1, false, 0));
    owner.inventory().deliver_fixed(own, |slot| &pages[slot], |_| panic!("no self doorbell")).unwrap();
    assert!(pages[1].is_pending(0x66) && !pages[0].is_pending(0x66) && !pages[2].is_pending(0x66));
    // A physical IPI to one remote CPU rings exactly that CPU.
    let one = fixed_ipi(&owner, fixed(0x77, 0, false, 200));
    let mut rung = Vec::new();
    owner.inventory().deliver_fixed(one, |slot| &pages[slot], |target| rung.push(target.apic_id())).unwrap();
    assert_eq!(rung, [200]);
    assert!(pages[2].is_pending(0x77) && !pages[0].is_pending(0x77) && !pages[1].is_pending(0x77));
}

#[test]
fn fan_out_validates_doorbell_targets_before_any_publication() {
    let ids = [1, 255, 300];
    let owner = owner(1, &ids);
    let pages = pages(&ids);
    for (destination, slot, id) in [(255u32, 1usize, 255u32), (300, 2, 300)] {
        let ipi = match owner.inventory().classify(fixed(0x55, 0, false, destination), 2) {
            Ok(IpiAction::Fixed(ipi)) => ipi,
            other => panic!("{other:?}"),
        };
        assert_eq!(owner.inventory().deliver_fixed(ipi, |_| panic!("no publication"), |_| panic!("no doorbell")),
            Err(FanOutError::DoorbellTarget { slot, id }));
    }
    // A broadcast fails as a whole, before the doorbellable slot 0 is touched.
    let all = fixed_ipi(&owner, fixed(0x55, 0, false, u32::MAX));
    assert_eq!(owner.inventory().deliver_fixed(all, |_| panic!("no publication"), |_| panic!("no doorbell")),
        Err(FanOutError::DoorbellTarget { slot: 1, id: 255 }));
    assert!(pages.iter().all(|page| !page.is_pending(0x55)));
    // The source is never doorbelled, so its own ID needs no doorbell format.
    let big = self::owner(300, &ids);
    let own = fixed_ipi(&big, fixed(0x55, 1, false, 0));
    big.inventory().deliver_fixed(own, |slot| &pages[slot], |_| panic!("no self doorbell")).unwrap();
    assert!(pages[2].is_pending(0x55));
}

#[test]
fn fan_out_stops_at_a_mixed_trigger_target() {
    let ids = [4, 5, 6];
    let owner = owner(6, &ids);
    let pages = pages(&ids);
    // Slot 1 has vector 55h in service as a level interrupt.
    set_bit(&pages[1], apic::ISR, 0x55);
    set_bit(&pages[1], apic::TMR, 0x55);
    let ipi = fixed_ipi(&owner, fixed(0x55, 2, false, 0));
    let mut rung = Vec::new();
    assert_eq!(owner.inventory().deliver_fixed(ipi, |slot| &pages[slot], |target| rung.push(target.apic_id())),
        Err(FanOutError::Publication { slot: 1, error: Error::MixedTrigger }));
    assert_eq!(rung, [4]);
    assert!(pages[0].is_pending(0x55) && !pages[1].is_pending(0x55) && !pages[2].is_pending(0x55));
}

#[test]
fn doorbell_targets_fit_both_documented_formats() {
    assert_eq!(msr::AVIC_DOORBELL, 0xc001_011b);
    for id in [0u32, 1, 27, 254] {
        assert_eq!(DoorbellTarget::new(id).map(DoorbellTarget::apic_id), Some(id));
    }
    for id in [255u32, 256, 511, 0x1_0000, u32::MAX] {
        assert_eq!(DoorbellTarget::new(id), None);
    }
    assert!(MADT_IDS.iter().all(|id| DoorbellTarget::new(*id).is_some()));
}

#[test]
fn inventory_admission_is_shared_with_startup_routing() {
    for (source, ids) in [
        (0u32, &[][..]),
        (7, &[1, 2]),
        (7, &[7, 7]),
        (7, &[7, u32::MAX]),
    ] {
        assert!(matches!(NativeIcr::admit(source, ids), Err(NativeIcrError::InvalidTopology)));
    }
    let many: Vec<u32> = (0..33).collect();
    assert!(matches!(NativeIcr::admit(0, &many), Err(NativeIcrError::InvalidTopology)));
    // A full 32-CPU inventory with the source in the last slot.
    let full: Vec<u32> = (100..132).collect();
    let owner = owner(131, &full);
    assert_eq!(fixed_ipi(&owner, fixed(0x55, 3, false, 0)).targets(), u32::MAX >> 1);
    assert_eq!(fixed_ipi(&owner, fixed(0x55, 0, false, u32::MAX)).targets(), u32::MAX);
    assert_eq!(fixed_ipi(&owner, fixed(0x55, 1, false, 0)).targets(), 1 << 31);
}
