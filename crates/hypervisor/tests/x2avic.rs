use svmvisor_hypervisor::{address::{AddressPolicy, EncryptionState}, svm::{vmcb::{Vmcb, EventIntercept}, x2avic::*}};

fn policy() -> AddressPolicy { AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap() }
fn capabilities() -> X2AvicCapabilities { X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18)).unwrap() }

#[test]
fn exact_capability_gate_and_page_admission() {
    assert_eq!(X2AvicCapabilities::admit(0x7ed8_320b, 0xfebf_bdff), Err(Error::MissingCapability));
    for bit in [0, 13, 18] {
        assert_eq!(X2AvicCapabilities::admit(1 << 21, (1 | (1 << 13) | (1 << 18)) & !(1 << bit)), Err(Error::MissingCapability));
    }
    assert!(NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 511, &policy()).is_ok());
    assert_eq!(NativeX2AvicProfile::new(capabilities(), 0x2000, 0x2000, 23, &policy()), Err(Error::AliasedPages));
    assert_eq!(NativeX2AvicProfile::new(capabilities(), 0x2000, 0x3000, 512, &policy()), Err(Error::InvalidId));
    assert!(NativeX2AvicProfile::new(capabilities(), 0x2001, 0x3000, 23, &policy()).is_err());
}

#[test]
fn table_format_and_publication_preserve_identity_and_reserved_bits() {
    assert_eq!(core::mem::size_of::<PhysicalIdTable>(), 4096);
    assert_eq!(core::mem::align_of::<PhysicalIdTable>(), 4096);
    let mut table = PhysicalIdTable::new();
    table.insert_stopped(37, 0x9000, &policy()).unwrap();
    assert_eq!(table.entry(37).unwrap(), (1 << 63) | 0x9025);
    assert_eq!(table.insert_stopped(38, 0x9000, &policy()), Err(Error::AliasedPages));
    assert_eq!(table.insert_stopped(37, 0xa000, &policy()), Err(Error::Occupied));
    table.set_running(37, true).unwrap();
    assert_eq!(table.entry(37).unwrap(), (3 << 62) | 0x9025);
    table.set_running(37, false).unwrap();
    assert_eq!(table.entry(37).unwrap(), (1 << 63) | 0x9025);
    assert_eq!(table.set_running(38, true), Err(Error::InvalidId));
}

#[test]
fn backing_init_irq_coalescing_and_trigger_conflict() {
    assert_eq!(core::mem::size_of::<BackingPage>(), 4096);
    assert_eq!(core::mem::align_of::<BackingPage>(), 4096);
    let mut page = BackingPage::new();
    page.reset_stopped(37, 0x50010).unwrap();
    assert_eq!(page.read_register(0x20).unwrap(), 37);
    assert_eq!(page.read_register(0xd0).unwrap(), 0x20020);
    assert_eq!(page.read_register(0xf0).unwrap(), 0xff);
    for offset in (0x320..=0x370).step_by(16) { assert_eq!(page.read_register(offset).unwrap(), 0x10000); }
    assert_eq!(page.enqueue(0xf, false), Err(Error::InvalidVector));
    assert_eq!(page.enqueue(0x61, true), Ok(true));
    assert_eq!(page.enqueue(0x61, true), Ok(false));
    assert_eq!(page.enqueue(0x61, false), Err(Error::MixedTrigger));
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
    assert_eq!(page.eoi_stopped(), Some((0xe0, true)));
    assert_eq!(page.highest_in_service(), Some(0x61));
    assert_eq!(page.read_register(0xa0).unwrap(), 0x6b);
    assert!(page.is_pending(0x70));
    assert_eq!(page.eoi_stopped(), Some((0x61, false)));
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
    assert_eq!(AvicExit::decode(0x401, 0x12345678, (1 << 32) | 37), Ok(AvicExit::IncompleteIpi { icr: 0x12345678, reason: 1, index: Some(37) }));
    assert_eq!(AvicExit::decode(0x401, 0, 5 << 32), Err(Error::InvalidExit));
    assert_eq!(AvicExit::decode(0x402, (1 << 32) | 0xb0, 0x61), Ok(AvicExit::NoAcceleration { offset: 0xb0, write: true, eoi_vector: Some(0x61) }));
    assert_eq!(AvicExit::decode(0x402, 0x390, u64::MAX), Ok(AvicExit::NoAcceleration { offset: 0x390, write: false, eoi_vector: None }));
    assert_eq!(AvicExit::decode(0x402, 1, 0), Err(Error::InvalidExit));
}

// Inert host fixture edits emulate externally prepared native entry fields.
// Actual production preparation stays with the existing admission owners.
fn set64(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), (vmcb as *mut Vmcb).cast::<u8>().add(offset), 8); }
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
