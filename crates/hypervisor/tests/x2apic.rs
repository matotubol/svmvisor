use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::{FIXTURE_APIC_BASE, FixtureApic, MsrError, handle_fixture_msr},
    },
};
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn setup(index: u32, value: u64, store: bool) -> (Vmcb, GuestRegisters) {
    let mut v = Vmcb::new();
    write(&mut v, 0x70, 0x7c);
    write(&mut v, 0x78, store as u64);
    write(&mut v, 0x578, 0x1000);
    write(&mut v, 0x5f8, value as u32 as u64);
    (
        v,
        GuestRegisters {
            rcx: 0xdead_beef_0000_0000 | index as u64,
            rdx: value >> 32,
            rbx: 77,
            ..GuestRegisters::default()
        },
    )
}
fn call(
    a: &mut FixtureApic,
    v: &mut Vmcb,
    f: &mut GuestRegisters,
    store: bool,
) -> Result<(), MsrError> {
    handle_fixture_msr(a, v, f, if store { &[0x0f, 0x30] } else { &[0x0f, 0x32] })
}
#[test]
fn read_zero_extends_and_write_preserves_registers_with_full_tpr() {
    let mut a = fixture();
    let (mut v, mut f) = setup(0x808, 0x5b, true);
    write(&mut v, 0x5f8, 0xaabb_ccdd_0000_005b);
    f.rdx = 0xaabb_ccdd_0000_0000;
    let old = f;
    call(&mut a, &mut v, &mut f, true).unwrap();
    assert_eq!(f, old);
    assert_eq!(v.guest_rax(), 0xaabb_ccdd_0000_005b);
    assert_eq!(a.controller().task_priority(), 0x5b);
    assert_eq!(v.virtual_interrupt_control() & 15, 5);
    assert_eq!(v.guest_rip(), 0x1002);
    write(&mut v, 0x78, 0);
    f.rdx = u64::MAX;
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), 0x5b);
    assert_eq!(f.rdx, 0);
    assert_eq!(f.rcx, old.rcx);
    f.rcx = 0x80a;
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), 0x5b);
    f.rcx = 0x1b;
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), FIXTURE_APIC_BASE);
    assert_eq!(f.rdx, 0);
}
#[test]
fn read_bitmaps_and_guest_eoi_retire_highest_without_touching_pending() {
    let mut a = fixture();
    let (mut v, mut f) = setup(0x824, 0, false);
    a.queue(0x80).unwrap();
    a.queue(0xff).unwrap();
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), 1);
    a.arm(&mut v).unwrap();
    let ctl = v.virtual_interrupt_control();
    write(&mut v, 0x60, ctl & !(1 << 8));
    a.observe(&v).unwrap();
    f.rcx = 0x817;
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), 1 << 31);
    f.rcx = 0x80b;
    write(&mut v, 0x78, 1);
    write(&mut v, 0x5f8, 0);
    f.rdx = 0;
    call(&mut a, &mut v, &mut f, true).unwrap();
    assert_eq!(a.controller().eoi_target(), None);
    assert!(a.controller().pending(0x80));
}
#[test]
fn invalid_accesses_stop_without_any_state_change() {
    for (index, value, store, gp) in [
        (0x802, 0, true, true),
        (0x802, u64::MAX, true, true),
        (0x808, 0x100, true, true),
        (0x808, 1 << 32, true, true),
        (0x80b, 1, true, true),
        (0x80b, 0, false, true),
        (0x80a, 0, true, true),
        (0x810, 0, true, true),
        (0x820, 0, true, true),
        (0x1b, 0xfed00d00, true, false),
        (0x833, 0, false, false),
    ] {
        let mut a = fixture();
        a.queue(0x50).unwrap();
        let (mut v, mut f) = setup(index, value, store);
        let bytes = *v.bytes();
        let frame = f;
        let owner = format!("{a:?}");
        let result = call(&mut a, &mut v, &mut f, store);
        if gp {
            assert_eq!(result, Err(MsrError::GeneralProtectionRequired));
        } else {
            assert!(matches!(result, Err(MsrError::Unsupported { .. })));
        }
        assert_eq!(v.bytes(), &bytes);
        assert_eq!(f, frame);
        assert_eq!(format!("{a:?}"), owner);
    }
}
#[test]
fn failed_continuation_pending_state_and_armed_writes_are_transactional() {
    let mut a = fixture();
    let (mut v, mut f) = setup(0x808, 0x20, true);
    for offset in [0xa8, 0x88] {
        write(&mut v, offset, 1 << 31);
        let bytes = *v.bytes();
        assert!(call(&mut a, &mut v, &mut f, true).is_err());
        assert_eq!(v.bytes(), &bytes);
        write(&mut v, offset, 0);
    }
    let bytes = *v.bytes();
    assert!(handle_fixture_msr(&mut a, &mut v, &mut f, &[0x0f, 0x32]).is_err());
    assert_eq!(v.bytes(), &bytes);
    write(&mut v, 0x578, 0x0000_7fff_ffff_ffff);
    let bytes = *v.bytes();
    assert!(call(&mut a, &mut v, &mut f, true).is_err());
    assert_eq!(v.bytes(), &bytes);
    write(&mut v, 0x578, 0x1000);
    a.queue(0x50).unwrap();
    a.arm(&mut v).unwrap();
    let bytes = *v.bytes();
    let owner = format!("{a:?}");
    assert!(call(&mut a, &mut v, &mut f, true).is_err());
    assert_eq!(v.bytes(), &bytes);
    assert_eq!(format!("{a:?}"), owner);
    write(&mut v, 0x78, 0);
    f.rcx = 0x822;
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), 1 << 16);
}

#[test]
fn privilege_and_unsupported_controls_refuse_reads_and_eoi_without_mutation() {
    for (offset, value) in [(0x4c8, 3u64 << 24), (0x60, 1u64 << 31), (0x90, 3)] {
        for (index, store) in [(0x808, false), (0x80b, true)] {
            let mut a = fixture();
            let (mut v, mut f) = setup(index, 0, store);
            write(&mut v, offset, value);
            let bytes = *v.bytes();
            let before = f;
            assert!(call(&mut a, &mut v, &mut f, store).is_err());
            assert_eq!(v.bytes(), &bytes);
            assert_eq!(f, before);
            assert_eq!(a, fixture());
        }
    }
}

fn fixture() -> FixtureApic {
    FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap()
}

#[test]
fn fixed_identity_read_zero_extends_both_msr_destinations() {
    let mut a = fixture();
    let (mut v, mut f) = setup(0x802, u64::MAX, false);
    write(&mut v, 0x5f8, u64::MAX);
    f.rdx = u64::MAX;
    call(&mut a, &mut v, &mut f, false).unwrap();
    assert_eq!(v.guest_rax(), 0);
    assert_eq!(f.rdx, 0);
    assert_eq!(v.guest_rip(), 0x1002);
    assert_eq!(a.identity(), 0);
}

#[test]
fn svr_timer_registers_drive_owner_and_enforce_bus_specific_invalid_operands() {
    let mut a = fixture();
    for (index, expected) in [
        (0x803, 0x10),
        (0x80f, 0x1ff),
        (0x832, 0x10000),
        (0x838, 0),
        (0x839, 0),
        (0x83e, 0),
    ] {
        let (mut v, mut f) = setup(index, u64::MAX, false);
        call(&mut a, &mut v, &mut f, false).unwrap();
        assert_eq!(v.guest_rax(), expected);
        assert_eq!(f.rdx, 0);
    }
    for (index, value) in [(0x83e, 11), (0x832, 0x20050), (0x838, 5), (0x80f, 0xff)] {
        let (mut v, mut f) = setup(index, value, true);
        let old = f;
        call(&mut a, &mut v, &mut f, true).unwrap();
        assert_eq!(f, old);
        assert_eq!(v.guest_rip(), 0x1002);
    }
    assert_eq!(a.controller().timer_lvt(), 0x30050);
    assert_eq!(
        a.advance_timer(5),
        Ok(svmvisor_hypervisor::svm::local_apic::TickOutcome::MaskedExpiration)
    );
    assert_eq!(a.controller().timer_remaining(), 5);
    let (mut v, mut f) = setup(0x1b, 0xfee00100, true);
    call(&mut a, &mut v, &mut f, true).unwrap();
    let before = format!("{a:?}");
    assert_eq!(
        a.advance_timer(u64::MAX),
        Err(svmvisor_hypervisor::svm::x2apic::QueueError::Disabled)
    );
    assert_eq!(format!("{a:?}"), before);
}

#[test]
fn new_msr_invalid_operands_and_unimplemented_lvt_inventory_are_transactional() {
    for (index, value, gp) in [
        (0x803, 0, true),
        (0x839, 0, true),
        (0x80f, 1 << 10, true),
        (0x80f, 1 << 12, true),
        (0x80f, 1 << 32, true),
        (0x832, 1 << 18, true),
        (0x832, 1 << 8, true),
        (0x832, 1 << 32, true),
        (0x838, 1 << 32, true),
        (0x83e, 4, true),
        (0x83e, 1 << 32, true),
        (0x832, 1 << 12, false),
        (0x832, 16, false),
        (0x832, 0, false),
        (0x833, 0, false),
        (0x834, 0, false),
        (0x835, 0, false),
        (0x836, 0, false),
        (0x837, 0, false),
    ] {
        let mut a = fixture();
        let (mut v, mut f) = setup(index, value, true);
        let bytes = *v.bytes();
        let old = f;
        let before = format!("{a:?}");
        let result = call(&mut a, &mut v, &mut f, true);
        assert_eq!(
            result,
            Err(if gp {
                MsrError::GeneralProtectionRequired
            } else {
                MsrError::Unsupported { index, write: true }
            })
        );
        assert_eq!(v.bytes(), &bytes);
        assert_eq!(f, old);
        assert_eq!(format!("{a:?}"), before);
    }
}

#[test]
fn new_msr_mutations_require_valid_continuation_and_unarmed_delivery() {
    for (index, value) in [(0x80f, 0), (0x832, 0x10050), (0x838, 5), (0x83e, 11)] {
        for armed in [false, true] {
            let mut a = fixture();
            let (mut v, mut f) = setup(index, value, true);
            if armed {
                a.queue(0x50).unwrap();
                a.arm(&mut v).unwrap();
            } else {
                write(&mut v, 0x578, 0x7fff_ffff_ffff);
            }
            let bytes = *v.bytes();
            let old = f;
            let before = format!("{a:?}");
            assert!(call(&mut a, &mut v, &mut f, true).is_err());
            assert_eq!(v.bytes(), &bytes);
            assert_eq!(f, old);
            assert_eq!(format!("{a:?}"), before);
        }
    }
}
