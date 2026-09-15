//! Independent regression checks of the native guest/physical APIC boundary.
use svmvisor_hypervisor::svm::{cpu_model::native_boot_cpuid, ipi::{
    NativeIcr, NativeIcrError, NativeStartupMailbox, NativeStartupCommand,
    NativeDestinationMode, try_lock_routes,
}};

fn pair(target: u32) -> ([NativeStartupMailbox; 2], NativeIcr) {
    let boxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(target)];
    for b in &boxes { b.mark_running(); }
    let routes = try_lock_routes(&boxes).unwrap();
    for slot in 0..2 {
        routes.prepare_destination_mode(slot, NativeDestinationMode::ExtendedXApic8)
            .unwrap().commit_destination_mode();
    }
    drop(routes);
    let mut owner = NativeIcr::admit(0, &[0, target]).unwrap();
    owner.enable_startup(0).unwrap();
    (boxes, owner)
}

#[test]
fn every_nonbroadcast_eight_bit_id_supports_repeated_startup_mailbox_cycles() {
    for id in 1..255 {
        let (boxes, mut owner) = pair(id);
        owner.xapic_access(0xfee00900, &boxes, 0x310, Some(id << 24),
            |_, _| panic!("ICR high escaped shadow"), |_| panic!("unexpected kick")).unwrap();
        for _ in 0..3 {
            owner.xapic_access(0xfee00900, &boxes, 0x300, Some(0xc500),
                |_, _| panic!("guest INIT reached hardware"), |to| assert_eq!(to, id)).unwrap();
            assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
            boxes[1].complete(NativeStartupCommand::Init).unwrap();
            owner.xapic_access(0xfee00900, &boxes, 0x300, Some(0x4608),
                |_, _| panic!("guest SIPI reached hardware"), |to| assert_eq!(to, id)).unwrap();
            boxes[1].complete(NativeStartupCommand::Sipi(8)).unwrap();
        }
        // The same destination, including low-nibble F IDs, reaches the
        // ordinary physical path unchanged under the eight-bit owner.
        owner.xapic_access(0xfee00900, &boxes, 0x300, Some(0xf1),
            |offset, value| { assert_eq!(offset, 0x300); assert_eq!(value, Some((u64::from(id)<<32)|0xf1)); 0 },
            |_| panic!("ordinary IPI entered startup mailbox")).unwrap();
    }
}

#[test]
fn presentation_hides_extensions_and_normalizes_both_dfr_models() {
    let (boxes, mut owner) = pair(16);
    let version = owner.xapic_access(0xfee00900, &boxes, 0x30, None,
        |_, _| 0x81050010, |_| unreachable!()).unwrap();
    assert_eq!(version, 0x01050010);
    for model in [0, 0xf0000000] {
        assert_eq!(owner.xapic_access(0xfee00900, &boxes, 0xe0, None,
            |_, _| model, |_| unreachable!()).unwrap(), model | 0x0fffffff);
        owner.xapic_access(0xfee00900, &boxes, 0xe0, Some(model | 0x0fffffff),
            |offset, value| { assert_eq!(offset, 0xe0); assert_eq!(value, Some(u64::from(model))); model },
            |_| unreachable!()).unwrap();
    }
    for offset in [0x400, 0x410, 0x420, 0x480, 0x4f0, 0x500, 0x530] {
        for write in [None, Some(0), Some(u32::MAX)] {
            assert_eq!(owner.xapic_access(0xfee00900, &boxes, offset, write,
                |_, _| panic!("hidden extension reached hardware"), |_| unreachable!()),
                Err(NativeIcrError::UnsupportedRegister));
        }
    }
    let native = [0x1234, 0x5678, u32::MAX, 0x9abc];
    let guest = native_boot_cpuid(0x80000001, native, 0);
    assert_eq!(guest[2] & (1<<3), 0);
    assert_eq!([guest[0], guest[1], guest[3]], [native[0], native[1], native[3]]);
}

#[test]
fn ordinary_logical_and_shorthand_delivery_remains_hardware_owned() {
    let (boxes, mut owner) = pair(16);
    owner.xapic_access(0xfee00900, &boxes, 0x310, Some(0xff000000),
        |_, _| unreachable!(), |_| unreachable!()).unwrap();
    for low in [0x8f1, 0x9f1, 0x0008_00f1, 0x000c_00f1] {
        owner.xapic_access(0xfee00900, &boxes, 0x300, Some(low),
            |offset, value| { assert_eq!(offset, 0x300); assert_eq!(value, Some(0xff00000000 | u64::from(low))); 0 },
            |_| panic!("ordinary IPI entered startup mailbox")).unwrap();
        assert!(boxes.iter().all(|b| b.peek().is_none()));
    }
}

#[test]
fn newly_intercepted_version_and_hidden_msrs_preserve_legacy_startup_execution() {
    use svmvisor_hypervisor::{registers::GuestRegisters, svm::{vmcb::Vmcb,
        dispatch::NativeMsrOutcome, ipi::handle_native_guest_apic_msr}};
    fn put(v: &mut Vmcb, offset: usize, value: u64) {
        unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset), 8); }
    }
    for (attributes, efer, cr0) in [(0x9b, 0x1000, 0x10), (0x49b, 0x1000, 0x11), (0x29b, 0x1500, 0x80000011)] {
        for (index, write) in [(0x803, false), (0x803, true), (0x841, false), (0x841, true)] {
            let mut v = Vmcb::new();
            put(&mut v, 0x70, 0x7c);
            put(&mut v, 0x78, u64::from(write));
            put(&mut v, 0x410, (attributes << 16) | (0xffff << 32));
            put(&mut v, 0x4d0, efer);
            put(&mut v, 0x558, cr0);
            put(&mut v, 0x570, 2);
            put(&mut v, 0x578, 0x100);
            put(&mut v, 0x5f8, 0x12345678);
            let mut f = GuestRegisters { rcx: index, rdx: 0xfeedface, ..GuestRegisters::default() };
            let before = f;
            let legal = index == 0x803 && !write;
            let outcome = handle_native_guest_apic_msr(0xfee00d00, &mut v, &mut f,
                &[0x0f, if write { 0x30 } else { 0x32 }], |msr| {
                    assert!(legal); assert_eq!(msr, 0x803); 0x81050010
                }).unwrap();
            if legal {
                assert_eq!(outcome, NativeMsrOutcome::Completed);
                assert_eq!(v.guest_rip(), 0x102);
                assert_eq!(v.guest_rax(), 0x01050010);
                assert_eq!(f.rdx, 0);
            } else {
                assert_eq!(outcome, NativeMsrOutcome::GeneralProtectionPrepared);
                assert_eq!(v.guest_rip(), 0x100);
                assert_eq!(v.guest_rax(), 0x12345678);
                assert_eq!(f, before);
            }
        }
    }
}
