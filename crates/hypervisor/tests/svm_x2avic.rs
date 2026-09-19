use svmvisor_hypervisor::{
    memory::address::{AddressPolicy, EncryptionState},
    svm::{
        vmcb::{EventIntercept, Vmcb},
        x2avic::*,
    },
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

fn capabilities() -> X2AvicCapabilities {
    X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18) | (1 << 25)).unwrap()
}

// Inert host fixture edits emulate externally prepared native entry fields.
// Actual production preparation stays with the existing admission owners.
fn set64(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

#[test]
fn exact_capability_gate_and_page_admission() {
    assert_eq!(X2AvicCapabilities::admit(0x7ed8_320b, 0xfebf_bdff), Err(Error::MissingCapability));
    // NP (0), AVIC (13), x2AVIC (18) and NmiVirt/VNMI (25) are all required.
    for bit in [0, 13, 18, 25] {
        assert_eq!(
            X2AvicCapabilities::admit(
                1 << 21,
                (1 | (1 << 13) | (1 << 18) | (1 << 25)) & !(1 << bit)
            ),
            Err(Error::MissingCapability)
        );
    }
    assert!(NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 511, &policy()).is_ok());
    assert_eq!(
        NativeX2AvicProfile::new(capabilities(), 0x2000, 0x2000, 23, &policy()),
        Err(Error::AliasedPages)
    );
    assert_eq!(
        NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 512, &policy()),
        Err(Error::InvalidId)
    );
    assert!(NativeX2AvicProfile::new(capabilities(), 0x2001, 0x3000, 23, &policy()).is_err());
}

#[test]
fn table_format_and_publication_preserve_identity_and_reserved_bits() {
    assert_eq!(core::mem::size_of::<PhysicalIdTable>(), 4096);
    assert_eq!(core::mem::align_of::<PhysicalIdTable>(), 4096);
    let mut table = PhysicalIdTable::new();
    assert!(!table.is_stopped_entry(37, 0x9000), "empty entry");
    table.insert_stopped(37, 0x9000, &policy()).unwrap();
    assert_eq!(table.entry(37).unwrap(), (1 << 63) | 0x9025);
    // Arm's pre-commit check (F5): exactly the published stopped entry.
    assert!(table.is_stopped_entry(37, 0x9000));
    for (id, backing) in
        [(37, 0xa000), (37, 0x9001), (37, 0x9000 | (1 << 52)), (38, 0x9000), (512, 0x9000)]
    {
        assert!(!table.is_stopped_entry(id, backing), "{id} {backing:#x}");
    }
    assert_eq!(table.insert_stopped(38, 0x9000, &policy()), Err(Error::AliasedPages));
    assert_eq!(table.insert_stopped(37, 0xa000, &policy()), Err(Error::Occupied));
    table.set_running(37, true).unwrap();
    assert_eq!(table.entry(37).unwrap(), (3 << 62) | 0x9025);
    assert!(!table.is_stopped_entry(37, 0x9000), "already running");
    table.set_running(37, false).unwrap();
    assert_eq!(table.entry(37).unwrap(), (1 << 63) | 0x9025);
    assert_eq!(table.set_running(38, true), Err(Error::InvalidId));
}

#[test]
fn backing_init_irq_coalescing_and_trigger_of_the_last_acceptance() {
    assert_eq!(core::mem::size_of::<BackingPage>(), 4096);
    assert_eq!(core::mem::align_of::<BackingPage>(), 4096);
    let mut page = BackingPage::new();
    page.reset_stopped(37, 0x50010).unwrap();
    assert_eq!(page.read_register(0x20).unwrap(), 37);
    assert_eq!(page.read_register(0xd0).unwrap(), 0x20020);
    assert_eq!(page.read_register(0xf0).unwrap(), 0xff);
    for offset in (0x320..=0x370).step_by(16) {
        assert_eq!(page.read_register(offset).unwrap(), 0x10000);
    }
    assert_eq!(page.enqueue(0xf, false), Err(Error::InvalidVector));
    assert_eq!(page.enqueue(0x61, true), Ok(true));
    assert_eq!(page.enqueue(0x61, true), Ok(false));
    // APM2 16.6.3 p648: TMR takes the trigger type of each acceptance.
    assert_eq!(page.enqueue(0x61, false), Ok(false));
    assert!(!page.is_level(0x61) && page.is_pending(0x61));
    assert_eq!(page.enqueue(0x61, true), Ok(false));
    assert!(page.is_level(0x61) && page.is_pending(0x61));
    assert_eq!(page.read_register(0x201), Err(Error::InvalidOffset));
    assert_eq!(page.reset_stopped(600, 0x50010), Err(Error::InvalidId));
    assert!(page.is_pending(0x61)); // Refused reset makes no changes.
}

#[test]
fn stopped_eoi_changes_only_top_isr_and_recomputes_ppr() {
    let mut page = BackingPage::new();
    page.reset_stopped(1, 0x50010).unwrap();
    page.write_register_stopped(0x80, 0x6b).unwrap();
    page.write_register_stopped(0x130, 1 << 1).unwrap(); // ISR61
    page.write_register_stopped(0x170, 1 << 0).unwrap(); // ISRe0
    page.write_register_stopped(0x1f0, 1 << 0).unwrap(); // TMRe0
    page.enqueue(0x70, false).unwrap();
    assert_eq!(page.eoi_stopped(), Some(0xe0));
    assert!(page.is_level(0xe0), "TMR belongs to the level-source ledger");
    assert_eq!(page.highest_in_service(), Some(0x61));
    assert_eq!(page.read_register(0xa0).unwrap(), 0x6b);
    assert!(page.is_pending(0x70));
    assert_eq!(page.eoi_stopped(), Some(0x61));
    assert_eq!(page.eoi_stopped(), None);
}

#[test]
fn concurrent_same_bank_producers_do_not_lose_irr_bits() {
    let mut page = BackingPage::new();
    page.reset_stopped(1, 0x50010).unwrap();
    let page = &page;
    std::thread::scope(|scope| {
        for vector in 0x40..0x60 {
            scope.spawn(move || {
                assert_eq!(page.enqueue(vector, false), Ok(true));
                assert_eq!(page.enqueue(vector, false), Ok(false));
            });
        }
    });
    assert_eq!(page.read_register(0x220).unwrap(), u32::MAX);
    assert_eq!(page.read_register(0x1a0).unwrap(), 0);
    assert_eq!(page.highest_in_service(), None);
}

#[test]
fn exit_decode_distinguishes_partial_ipi_and_post_write_eoi() {
    assert_eq!(
        AvicExit::decode(0x401, 0x12345678, (1 << 32) | 37),
        Ok(AvicExit::IncompleteIpi { icr: 0x12345678, reason: 1, index: Some(37) })
    );
    assert_eq!(AvicExit::decode(0x401, 0, 5 << 32), Err(Error::InvalidExit));
    assert_eq!(
        AvicExit::decode(0x402, (1 << 32) | 0xb0, 0x61),
        Ok(AvicExit::NoAcceleration { offset: 0xb0, write: true, eoi_vector: Some(0x61) })
    );
    assert_eq!(
        AvicExit::decode(0x402, 0x390, u64::MAX),
        Ok(AvicExit::NoAcceleration { offset: 0x390, write: false, eoi_vector: None })
    );
    // Reserved EXITINFO1 bits 3:0 carry no meaning (APM2 p.lvi, Table 15-28).
    assert_eq!(
        AvicExit::decode(0x402, 1, 0),
        Ok(AvicExit::NoAcceleration { offset: 0, write: false, eoi_vector: None })
    );
}

#[test]
fn exit_decode_follows_tables_15_25_to_15_29_and_ignores_reserved_fields() {
    let icr = 0x0000_0013_0000_04ef;
    // Table 15-26: ID in 63:32, reserved 31:12, index 11:0 only for IDs 1-3.
    for reason in 0..=4u64 {
        let index = (1..=3).contains(&reason).then_some(0xabc);
        assert_eq!(
            AvicExit::decode(0x401, icr, (reason << 32) | 0xffff_fabc),
            Ok(AvicExit::IncompleteIpi { icr, reason: reason as u32, index }),
            "id {reason}"
        );
    }
    for reason in [5u64, 6, 0xffff_ffff] {
        assert_eq!(AvicExit::decode(0x401, icr, reason << 32), Err(Error::InvalidExit));
    }
    // EXITINFO1 is the complete written ICR, including reserved-looking bits.
    assert_eq!(
        AvicExit::decode(0x401, u64::MAX, 0),
        Ok(AvicExit::IncompleteIpi { icr: u64::MAX, reason: 0, index: None })
    );
    // Table 15-28: bit 32 R/W, bits 11:4 offset; bits 63:33, 31:12, 3:0 reserved.
    let noise = !((1u64 << 32) | 0xff0);
    for (info1, offset, write) in [
        (0x320 | noise, 0x320, false),
        ((1 << 32) | 0x320 | noise, 0x320, true),
        ((1 << 32) | 0x3e0, 0x3e0, true),
        (0xff0, 0xff0, false),
    ] {
        assert_eq!(
            AvicExit::decode(0x402, info1, u64::MAX),
            Ok(AvicExit::NoAcceleration { offset, write, eoi_vector: None }),
            "{info1:#x}"
        );
    }
    // Table 15-29: EOI writes carry the highest in-service vector in bits 7:0.
    assert_eq!(
        AvicExit::decode(0x402, (1 << 32) | 0xb0 | noise, 0xffff_ff00_0000_1f10),
        Ok(AvicExit::NoAcceleration { offset: 0xb0, write: true, eoi_vector: Some(0x10) })
    );
    assert_eq!(
        AvicExit::decode(0x402, (1 << 32) | 0xb0, 0xff),
        Ok(AvicExit::NoAcceleration { offset: 0xb0, write: true, eoi_vector: Some(0xff) })
    );
    // ISR bits 15:0 are reserved, so no in-service vector is below 16.
    for vector in [0u64, 15, 0x100] {
        assert_eq!(AvicExit::decode(0x402, (1 << 32) | 0xb0, vector), Err(Error::InvalidExit));
    }
    // A read of the EOI offset has no vector.
    assert_eq!(
        AvicExit::decode(0x402, 0xb0, 0),
        Ok(AvicExit::NoAcceleration { offset: 0xb0, write: false, eoi_vector: None })
    );
    for code in [0x400, 0x403, 0x7c, u64::MAX] {
        assert_eq!(AvicExit::decode(code, 0, 0), Err(Error::InvalidExit));
    }
}

#[test]
fn ledger_init_retirement_completes_every_source_and_drains_in_physical_isr_order() {
    use svmvisor_hypervisor::{
        arch::x86_64::apic::highest_vector,
        svm::x2avic::irq::{IrqError as E, PhysicalIrqLedger},
    };
    let mut ledger = PhysicalIrqLedger::new();
    let mut physical = [0u32; 8];
    for vector in [0x40u8, 0x80, 0xc1, 0x61] {
        ledger.commit_level_capture(vector).unwrap();
        physical[usize::from(vector / 32)] |= 1 << (vector % 32);
    }
    // 0x61 was already completed by the guest but waits behind 0x80/0xc1.
    ledger.complete_level(0x61).unwrap();
    assert_eq!(ledger.next_eoi(highest_vector(&physical)), Ok(None));
    // Preparation is read-only and exact.
    assert_eq!(ledger.validate_retirement(&physical), Ok(()));
    let mut foreign = physical;
    foreign[1] |= 1 << 2; // vector 0x22, not held
    assert_eq!(ledger.validate_retirement(&foreign), Err(E::UnexpectedPhysicalIsr(0x22)));
    let mut missing = physical;
    missing[2] &= !(1 << 0); // held 0x40 without a physical ISR bit
    assert_eq!(
        ledger.validate_retirement(&missing),
        Err(E::PhysicalIsrMismatch { vector: 0x40, highest: Some(0xc1) })
    );
    let before = ledger;
    ledger.retire_all();
    assert_ne!(ledger, before);
    assert_eq!(ledger.complete_level(0x40), Err(E::UnownedLevelCompletion(0x40)));
    // Drain: always the highest physical in-service vector, one at a time.
    let mut order = Vec::new();
    while let Some(vector) = ledger.next_eoi(highest_vector(&physical)).unwrap() {
        physical[usize::from(vector / 32)] &= !(1 << (vector % 32));
        ledger.commit_eoi(vector).unwrap();
        order.push(vector);
    }
    assert_eq!(order, [0xc1, 0x80, 0x61, 0x40]);
    assert!(ledger.is_empty() && physical == [0; 8]);
    assert_eq!(ledger.validate_retirement(&[0; 8]), Ok(()));
    // Retiring an empty ledger is a no-op.
    ledger.retire_all();
    assert_eq!(ledger, PhysicalIrqLedger::new());
}

#[test]
fn backing_init_reset_takes_exact_table_16_2_values_and_preserves_identity() {
    use svmvisor_hypervisor::arch::x86_64::apic;
    let mut page = BackingPage::new();
    page.reset_stopped(0x1b, GUEST_APIC_VERSION).unwrap();
    // Dirty every register INIT resets, plus two words it must not touch.
    for offset in (0..0x1000u16).step_by(16).filter(|o| ![apic::ID, apic::VERSION].contains(o)) {
        page.write_register_stopped(offset, 0xa5a5_0000 | offset as u32).unwrap();
    }
    page.reset_after_init_stopped().unwrap();
    let mut expected = std::collections::BTreeMap::new();
    expected.insert(apic::ID, 0x1b);
    expected.insert(apic::VERSION, 0x0005_0010);
    // 16.14 p662: cluster 1, logical bit 11.
    expected.insert(apic::LDR, 0x0001_0800);
    expected.insert(apic::SVR, 0xff);
    for offset in apic::LVTS {
        expected.insert(offset, 0x0001_0000);
    }
    for offset in [
        apic::TPR,
        apic::APR,
        apic::PPR,
        apic::EOI,
        apic::RRR,
        apic::ESR,
        apic::ICR,
        apic::ICR_HIGH,
        apic::TIMER_INITIAL_COUNT,
        apic::TIMER_CURRENT_COUNT,
        apic::TIMER_DIVIDE,
    ] {
        expected.insert(offset, 0);
    }
    for base in [apic::ISR, apic::TMR, apic::IRR] {
        for bank in 0..8 {
            expected.insert(base + bank * 16, 0);
        }
    }
    assert_eq!(expected.len(), 2 + 1 + 1 + 6 + 11 + 24);
    for offset in (0..0x1000u16).step_by(16) {
        let value = page.read_register(offset).unwrap();
        match expected.get(&offset) {
            Some(want) => assert_eq!(value, *want, "offset {offset:#x}"),
            // Not an x2APIC INIT register (DFR E0h, extended space, ...).
            None => assert_eq!(value, 0xa5a5_0000 | offset as u32, "offset {offset:#x}"),
        }
    }
    // A refused reset changes nothing.
    page.write_register_stopped(apic::TPR, 0x20).unwrap();
    page.write_register_stopped(apic::VERSION, 0x8005_0010).unwrap();
    assert_eq!(page.reset_after_init_stopped(), Err(Error::UnsupportedVersion));
    assert_eq!(page.read_register(apic::TPR).unwrap(), 0x20);
    page.write_register_stopped(apic::VERSION, GUEST_APIC_VERSION).unwrap();
    page.write_register_stopped(apic::ID, 512).unwrap();
    assert_eq!(page.reset_after_init_stopped(), Err(Error::InvalidId));
    assert_eq!(page.read_register(apic::TPR).unwrap(), 0x20);
}

#[test]
fn v_nmi_is_set_only_on_the_armed_profile_and_survives_init() {
    use svmvisor_hypervisor::{
        arch::x86_64::registers::GuestRegisters,
        svm::x2avic::startup::{
            NativeStartupCommand, NativeStartupEffect, NativeStartupState, NativeStartupTarget,
        },
    };
    let profile = NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 37, &policy()).unwrap();
    let mut vmcb = Vmcb::new();
    // A bare VMCB is not the armed x2AVIC profile: V_NMI has no effect there
    // (V_NMI_ENABLE clear), so it is refused and nothing changes.
    let before = *vmcb.bytes();
    assert!(vmcb.set_guest_v_nmi_pending(&profile).is_err());
    assert_eq!(*vmcb.bytes(), before);
    set64(&mut vmcb, 0x90, 1);
    vmcb.set_virtual_interrupt_tpr(0).unwrap();
    vmcb.enable_native_x2avic(&profile).unwrap();
    // enable_native_x2avic sets V_NMI_ENABLE (bit 26) and the NMI intercept.
    assert_ne!(vmcb.virtual_interrupt_control() & (1 << 26), 0);
    assert!(vmcb.event_intercept(EventIntercept::Nmi));
    // Re-presenting a physical NMI (VMEXIT_NMI) or a guest NMI IPI sets V_NMI
    // (bit 11); virtual NMIs coalesce, so a repeat is idempotent.
    vmcb.set_guest_v_nmi_pending(&profile).unwrap();
    assert_ne!(vmcb.virtual_interrupt_control() & (1 << 11), 0);
    vmcb.set_guest_v_nmi_pending(&profile).unwrap();
    assert_eq!((vmcb.virtual_interrupt_control() & (1 << 11)).count_ones(), 1);
    // INIT clears the pending virtual NMI but keeps V_NMI_ENABLE and the NMI
    // intercept, so a fresh AP can take NMIs once started.
    let mut frame = GuestRegisters::default();
    let mut state = NativeStartupState::Running;
    NativeStartupTarget {
        vmcb: &mut vmcb,
        frame: &mut frame,
        state: &mut state,
        signature: 0x00b4_0f40,
    }
    .apply_x2avic(NativeStartupCommand::Init, &profile)
    .unwrap();
    assert_eq!(vmcb.virtual_interrupt_control() & (1 << 11), 0);
    assert_ne!(vmcb.virtual_interrupt_control() & (1 << 26), 0);
    assert!(vmcb.event_intercept(EventIntercept::Nmi));
    vmcb.set_guest_v_nmi_pending(&profile).unwrap();
    assert_ne!(vmcb.virtual_interrupt_control() & (1 << 11), 0);
    // A shutdown guest refuses the virtual NMI.
    set64(&mut vmcb, 0x70, 0x7f);
    assert!(vmcb.set_guest_v_nmi_pending(&profile).is_err());
}

#[test]
fn hardware_written_v_irq_keeps_the_armed_profile_valid() {
    let profile = NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 37, &policy()).unwrap();
    let mut vmcb = Vmcb::new();
    set64(&mut vmcb, 0x90, 1);
    vmcb.set_virtual_interrupt_tpr(0).unwrap();
    vmcb.enable_native_x2avic(&profile).unwrap();
    // #VMEXIT wrote V_IRQ (bit 8) back for an IRR bit the guest has not taken
    // (Table B-1 p740); VMRUN ignores it under AVIC, so nothing is refused.
    set64(&mut vmcb, 0x60, NATIVE_CONTROL | (1 << 8));
    vmcb.validate_native_x2avic(&profile).unwrap();
    vmcb.set_guest_v_nmi_pending(&profile).unwrap();
    vmcb.queue_native_x2avic_general_protection(&profile).unwrap();
    // Priority, V_IGN_TPR and vector of that interrupt are ignored with it.
    set64(&mut vmcb, 0x60, NATIVE_CONTROL | (1 << 8) | (3 << 16) | (1 << 20) | (0x30 << 32));
    vmcb.validate_native_x2avic(&profile).unwrap();
    // Host-owned and reserved bits still refuse: VGIF enable, bit 13, bit 40.
    for bit in [25, 13, 40] {
        set64(&mut vmcb, 0x60, NATIVE_CONTROL | (1 << bit));
        assert!(vmcb.validate_native_x2avic(&profile).is_err());
    }
}

#[test]
fn x2avic_cpu_init_commit_zeroes_v_tpr_and_invalidates_clean_bits() {
    use svmvisor_hypervisor::{
        arch::x86_64::registers::GuestRegisters,
        svm::x2avic::startup::{
            NativeStartupCommand, NativeStartupEffect, NativeStartupState, NativeStartupTarget,
        },
    };
    let profile = NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 37, &policy()).unwrap();
    let mut vmcb = Vmcb::new();
    set64(&mut vmcb, 0x90, 1);
    vmcb.set_virtual_interrupt_tpr(6).unwrap();
    vmcb.enable_native_x2avic(&profile).unwrap();
    // Hardware later wrote guest CR8 = 9 and the processor cached everything.
    set64(&mut vmcb, 0x60, NATIVE_CONTROL | 9);
    set64(&mut vmcb, 0xc0, 0xffff_ffff);
    let mut frame = GuestRegisters::default();
    let mut state = NativeStartupState::Running;
    let mut target = NativeStartupTarget {
        vmcb: &mut vmcb,
        frame: &mut frame,
        state: &mut state,
        signature: 0x00b4_0f40,
    };
    assert_eq!(
        target.apply_x2avic(NativeStartupCommand::Init, &profile),
        Ok(NativeStartupEffect::Init)
    );
    assert_eq!(vmcb.virtual_interrupt_control(), NATIVE_CONTROL);
    assert_eq!(u32::from_le_bytes(vmcb.bytes()[0xc0..0xc4].try_into().unwrap()), 0);
    vmcb.validate_native_x2avic(&profile).unwrap();
    assert_eq!(state, NativeStartupState::AwaitSipi);
}

#[test]
fn explicit_vmcb_profile_and_generic_rejection_preserve_stopped_bytes() {
    let profile = NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 37, &policy()).unwrap();
    let mut vmcb = Vmcb::new();
    let before = *vmcb.bytes();
    assert!(vmcb.enable_native_x2avic(&profile).is_err());
    assert_eq!(*vmcb.bytes(), before);
    set64(&mut vmcb, 0x90, 1);
    // Captured TPR=0x6b: CR8 must start at its priority class, six.
    vmcb.set_virtual_interrupt_tpr(6).unwrap();
    vmcb.enable_native_x2avic(&profile).unwrap();
    vmcb.validate_native_x2avic(&profile).unwrap();
    assert_eq!(vmcb.virtual_interrupt_control(), NATIVE_CONTROL | 6);
    assert!(vmcb.event_intercept(EventIntercept::PhysicalInterrupt));
    let before = *vmcb.bytes();
    assert!(vmcb.set_virtual_interrupt_tpr(2).is_err());
    assert_eq!(*vmcb.bytes(), before);
    let wrong = NativeX2AvicProfile::new(capabilities(), 0x4000, 0x3000, 37, &policy()).unwrap();
    assert!(vmcb.validate_native_x2avic(&wrong).is_err());
    vmcb.queue_native_x2avic_general_protection(&profile).unwrap();
    assert_eq!(vmcb.event_injection(), (1 << 31) | (1 << 11) | (3 << 8) | 13);
    assert_eq!(vmcb.guest_rip(), 0);
    assert!(vmcb.queue_native_x2avic_general_protection(&profile).is_err());
}

#[test]
fn irq_ledger_holds_level_sources_and_acknowledges_only_in_physical_isr_order() {
    use svmvisor_hypervisor::svm::x2avic::irq::{Capture, IrqError as E, PhysicalIrqLedger};
    let mut ledger = PhysicalIrqLedger::new();
    assert_eq!(ledger.prepare_capture(0x1f, false, Some(0x1f)), Err(E::ReservedVector(0x1f)));
    assert_eq!(
        ledger.prepare_capture(0x40, false, Some(0x41)),
        Err(E::PhysicalIsrMismatch { vector: 0x40, highest: Some(0x41) })
    );
    assert_eq!(ledger.prepare_capture(0x40, false, Some(0x40)), Ok(Capture::Edge));
    // A lower level source is captured first, then a higher one preempts it.
    assert_eq!(ledger.prepare_capture(0x40, true, Some(0x40)), Ok(Capture::Level));
    ledger.commit_level_capture(0x40).unwrap();
    assert_eq!(
        ledger.prepare_capture(0x40, true, Some(0x40)),
        Err(E::DuplicatePhysicalSource(0x40))
    );
    assert_eq!(ledger.prepare_capture(0x80, true, Some(0x80)), Ok(Capture::Level));
    ledger.commit_level_capture(0x80).unwrap();
    assert_eq!(ledger.commit_level_capture(0x80), Err(E::DuplicatePhysicalSource(0x80)));
    // The guest completes the lower source first; its physical EOI must wait.
    ledger.complete_level(0x40).unwrap();
    assert_eq!(ledger.complete_level(0x40), Err(E::UnownedLevelCompletion(0x40)));
    assert_eq!(ledger.complete_level(0x60), Err(E::UnownedLevelCompletion(0x60)));
    assert_eq!(ledger.next_eoi(Some(0x80)), Ok(None));
    assert_eq!(ledger.commit_eoi(0x80), Err(E::CompletionNotReady(0x80)));
    assert_eq!(ledger.next_eoi(Some(0x50)), Err(E::UnexpectedPhysicalIsr(0x50)));
    assert_eq!(
        ledger.next_eoi(Some(0x40)),
        Err(E::PhysicalIsrMismatch { vector: 0x80, highest: Some(0x40) })
    );
    ledger.complete_level(0x80).unwrap();
    assert_eq!(ledger.next_eoi(Some(0x80)), Ok(Some(0x80)));
    ledger.commit_eoi(0x80).unwrap();
    assert_eq!(ledger.next_eoi(Some(0x40)), Ok(Some(0x40)));
    ledger.commit_eoi(0x40).unwrap();
    assert!(ledger.is_empty() && !ledger.holds(0x40));
    assert_eq!(ledger.next_eoi(None), Ok(None));
    ledger.commit_level_capture(0x90).unwrap();
    assert_eq!(ledger.next_eoi(None), Err(E::PhysicalIsrMismatch { vector: 0x90, highest: None }));
}

#[test]
fn apic_register_numbering_matches_reviewed_backing_offsets() {
    use svmvisor_hypervisor::arch::x86_64::apic;
    assert_eq!(apic::highest_vector(&[0; 8]), None);
    assert_eq!(apic::highest_vector(&[1 << 5, 0, 0, 0, 0, 0, 1, 0]), Some(0xc0));
    for (offset, msr) in [
        (apic::TPR, 0x808),
        (apic::SVR, 0x80f),
        (apic::ICR, 0x830),
        (apic::LVT_ERROR, 0x837),
        (apic::TIMER_INITIAL_COUNT, 0x838),
        (apic::EXTENDED, 0x840),
    ] {
        assert_eq!(apic::msr(offset), msr);
    }
    assert_eq!(apic::LVTS, [0x320, 0x330, 0x340, 0x350, 0x360, 0x370]);
    let mut page = BackingPage::new();
    page.reset_stopped(37, GUEST_APIC_VERSION).unwrap();
    for offset in apic::LVTS {
        assert_eq!(page.read_register(offset).unwrap(), apic::LVT_MASKED);
    }
    page.write_register_stopped(apic::TPR, 0x20).unwrap();
    page.enqueue(0x61, true).unwrap();
    page.reset_after_init_stopped().unwrap();
    assert_eq!(page.read_register(apic::TPR).unwrap(), 0);
    assert!(!page.is_pending(0x61) && !page.is_level(0x61));
    assert_eq!(page.read_register(apic::ID).unwrap(), 37);
    assert_eq!(page.read_register(apic::LDR).unwrap(), 0x20020);
}
