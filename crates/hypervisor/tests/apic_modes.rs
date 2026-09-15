use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        local_apic::{Error, LocalApic},
        vmcb::Vmcb,
        x2apic::{
            AdmissionError, ApicMode, FIXTURE_APIC_BASE, FixtureApic, MsrError, QueueError,
            handle_fixture_msr,
        },
    },
};

const BASE: u64 = FIXTURE_APIC_BASE & !(3 << 10);

// Inert host-only VMCB fixtures: no hardware entry or privileged operation.
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

fn setup(v: &mut Vmcb, index: u32, value: u64, store: bool) -> GuestRegisters {
    write(v, 0x70, 0x7c);
    write(v, 0x78, u64::from(store));
    write(v, 0x578, 0x1000);
    write(v, 0x5f8, 0xcafe_beef_0000_0000 | value as u32 as u64);
    GuestRegisters {
        rcx: 0xdead_beef_0000_0000 | index as u64,
        rdx: 0xfeed_face_0000_0000 | (value >> 32),
        rbx: 77,
        ..GuestRegisters::default()
    }
}

fn access(
    a: &mut FixtureApic,
    v: &mut Vmcb,
    f: &mut GuestRegisters,
    store: bool,
) -> Result<(), MsrError> {
    handle_fixture_msr(a, v, f, if store { &[0x0f, 0x30] } else { &[0x0f, 0x32] })
}

#[test]
fn admission_distinguishes_invalid_from_unsupported_and_rejects_armed_state() {
    for (base, mode) in [
        (BASE, ApicMode::Disabled),
        (BASE | 0x800, ApicMode::XApic),
        (BASE | 0xc00, ApicMode::X2Apic),
    ] {
        let a = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), base).unwrap();
        assert_eq!(a.mode(), mode);
        assert_eq!(a.apic_base(), base);
    }
    for (base, error) in [
        (BASE | 0x400, AdmissionError::InvalidBase),
        (BASE | 1, AdmissionError::InvalidBase),
        (BASE | (1 << 52), AdmissionError::InvalidBase),
        (BASE ^ 0x1000, AdmissionError::UnsupportedBaseAddress),
        (BASE & !0x100, AdmissionError::UnsupportedBootstrapCpu),
    ] {
        let mut controller = LocalApic::admit_enabled();
        controller.queue(0x50).unwrap();
        controller.write_timer_initial(0).unwrap();
        controller.write_timer_divide(0xb).unwrap();
        controller.write_timer_lvt(0x60).unwrap();
        controller.write_timer_initial(7).unwrap();
        let retained = format!("{controller:?}");
        let (reason, returned) = FixtureApic::admit_fixed_bsp(controller, base).unwrap_err();
        assert_eq!(reason, error);
        assert_eq!(format!("{returned:?}"), retained);
    }
    let mut controller = LocalApic::admit_enabled();
    controller.queue(0x50).unwrap();
    controller.arm(&mut Vmcb::new()).unwrap();
    let retained = format!("{controller:?}");
    let (reason, returned) =
        FixtureApic::admit_fixed_bsp(controller, FIXTURE_APIC_BASE).unwrap_err();
    assert_eq!(reason, AdmissionError::ArmedController);
    assert_eq!(format!("{returned:?}"), retained);
}

#[test]
fn every_mode_write_transition_is_completed_or_faults_transactionally() {
    for source in [0, 2, 3] {
        for target in 0..4 {
            let mut controller = LocalApic::admit_enabled();
            controller.set_task_priority(0x2b).unwrap();
            controller.queue(0x70).unwrap();
            let mut a = FixtureApic::admit_fixed_bsp(controller, BASE | (source << 10)).unwrap();
            let mut v = Vmcb::new();
            v.set_virtual_interrupt_tpr(2).unwrap();
            let mut f = setup(&mut v, 0x1b, BASE | (target << 10), true);
            let before = *v.bytes();
            let frame = f;
            let owner = format!("{a:?}");
            let result = access(&mut a, &mut v, &mut f, true);
            if target == 1 || (source == 0 && target == 3) || (source == 3 && target == 2) {
                assert_eq!(result, Err(MsrError::GeneralProtectionRequired));
                assert_eq!(v.bytes(), &before);
                assert_eq!(format!("{a:?}"), owner);
            } else {
                result.unwrap();
                assert_eq!(a.apic_base(), BASE | (target << 10));
                assert_eq!(v.guest_rip(), 0x1002);
                assert_eq!(
                    v.guest_rax(),
                    u64::from_le_bytes(before[0x5f8..0x600].try_into().unwrap())
                );
                assert_eq!(v.virtual_interrupt_control() & 15, 2);
                assert_eq!(a.controller().task_priority(), 0x2b);
                assert!(a.controller().pending(0x70));
            }
            assert_eq!(f, frame);
        }
    }
}

#[test]
fn base_mbz_faults_but_legal_relocation_and_bsc_policy_stop_without_fault() {
    for value in [
        BASE | 1,
        BASE | 0x200,
        BASE | (1 << 52),
        BASE | (1 << 63),
        BASE ^ 0x1000,
        BASE | (1 << 40),
        BASE & !0x100,
    ] {
        let mut a =
            FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
        let mut v = Vmcb::new();
        let mut f = setup(&mut v, 0x1b, value, true);
        let before = *v.bytes();
        let frame = f;
        let owner = format!("{a:?}");
        let result = access(&mut a, &mut v, &mut f, true);
        if value & (0xfff0_0000_0000_02ff) != 0 {
            assert_eq!(result, Err(MsrError::GeneralProtectionRequired));
        } else {
            assert_eq!(
                result,
                Err(MsrError::Unsupported {
                    index: 0x1b,
                    write: true
                })
            );
        }
        assert_eq!(v.bytes(), &before);
        assert_eq!(f, frame);
        assert_eq!(format!("{a:?}"), owner);
    }
}

#[test]
fn entire_x2apic_msr_range_is_gated_in_disabled_and_xapic_modes() {
    for mode in [0, 0x800] {
        let mut a = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), BASE | mode).unwrap();
        for index in 0x800..=0x8ff {
            for store in [false, true] {
                let mut v = Vmcb::new();
                let mut f = setup(&mut v, index, 0, store);
                let before = *v.bytes();
                let frame = f;
                assert_eq!(
                    access(&mut a, &mut v, &mut f, store),
                    Err(MsrError::GeneralProtectionRequired)
                );
                assert_eq!(v.bytes(), &before);
                assert_eq!(f, frame);
                assert_eq!(a.apic_base(), BASE | mode);
            }
        }
        let mut v = Vmcb::new();
        let mut f = setup(&mut v, 0x1b, 0, false);
        access(&mut a, &mut v, &mut f, false).unwrap();
        assert_eq!(v.guest_rax(), BASE | mode);
        assert_eq!(f.rdx, 0);
    }
}

#[test]
fn reserved_x2apic_slots_fault_but_unmodeled_architectural_registers_are_policy_stops() {
    let mut a =
        FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
    for index in [
        0x800, 0x801, 0x804, 0x80c, 0x80e, 0x829, 0x831, 0x83a, 0x843, 0x847, 0x854, 0x8ff, 0x833,
        0x834, 0x853,
    ] {
        for store in [false, true] {
            let mut v = Vmcb::new();
            let mut f = setup(&mut v, index, 0, store);
            let before = *v.bytes();
            let expected = if [0x833, 0x834, 0x853].contains(&index) {
                MsrError::Unsupported {
                    index,
                    write: store,
                }
            } else {
                MsrError::GeneralProtectionRequired
            };
            assert_eq!(access(&mut a, &mut v, &mut f, store), Err(expected));
            assert_eq!(v.bytes(), &before);
        }
    }
}

#[test]
fn disable_two_step_reenable_retains_controller_and_gates_delivery() {
    let mut controller = LocalApic::admit_enabled();
    let mut v = Vmcb::new();
    controller.queue(0x50).unwrap();
    controller.arm(&mut v).unwrap();
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8)); // Synthesized consumed V_IRQ.
    write(&mut v, 0x70, 0x81);
    controller.observe(&v).unwrap();
    controller.queue(0x70).unwrap();
    controller.set_task_priority(0x2b).unwrap();
    v.set_virtual_interrupt_tpr(2).unwrap();
    controller.write_timer_initial(0).unwrap();
    controller.write_timer_divide(0xb).unwrap();
    controller.write_timer_lvt(0x60).unwrap();
    controller.write_timer_initial(7).unwrap();
    let mut a = FixtureApic::admit_fixed_bsp(controller, FIXTURE_APIC_BASE).unwrap();
    let retained = format!("{:?}", a.controller());
    for base in [BASE, BASE | 0x800, FIXTURE_APIC_BASE] {
        let mut f = setup(&mut v, 0x1b, base, true);
        access(&mut a, &mut v, &mut f, true).unwrap();
        assert_eq!(format!("{:?}", a.controller()), retained);
        assert_eq!(a.identity(), 0);
        if base == BASE {
            let before = *v.bytes();
            assert_eq!(a.arm(&mut v), Ok(None));
            assert_eq!(a.queue(0x80), Err(QueueError::Disabled));
            assert_eq!(v.bytes(), &before);
            assert_eq!(format!("{:?}", a.controller()), retained);
        }
    }
    assert_eq!(a.arm(&mut v), Ok(Some(0x70)));
    let mut f = setup(&mut v, 0x1b, BASE, true);
    let before = *v.bytes();
    assert!(matches!(
        access(&mut a, &mut v, &mut f, true),
        Err(MsrError::PendingState(_))
    ));
    assert_eq!(v.bytes(), &before);
    // Even after hardware clears V_IRQ, accounting must precede a mode write.
    let control = v.virtual_interrupt_control();
    write(&mut v, 0x60, control & !(1 << 8));
    let before = *v.bytes();
    assert_eq!(
        access(&mut a, &mut v, &mut f, true),
        Err(MsrError::Controller(Error::Armed))
    );
    assert_eq!(v.bytes(), &before);
    assert_eq!(a.apic_base(), FIXTURE_APIC_BASE);
    assert_eq!(a.observe(&v), Ok(Some(0x70)));
    access(&mut a, &mut v, &mut f, true).unwrap();
    assert_eq!(a.mode(), ApicMode::Disabled);
    assert!(a.controller().in_service(0x50));
    assert!(a.controller().in_service(0x70));
    assert_eq!(a.controller().timer_remaining(), 7);
}

#[test]
fn faults_do_not_require_a_valid_sequential_rip_but_success_does() {
    for (index, value, store, fault) in [
        (0x1b, BASE | 0x800, true, true), // Illegal x2APIC -> xAPIC.
        (0x808, 0x100, true, true),
        (0x80b, 0, false, true),
        (0x1b, BASE, true, false),
        (0x808, 0x20, true, false),
        (0x808, 0, false, false),
    ] {
        let mut a =
            FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE).unwrap();
        let mut v = Vmcb::new();
        let mut f = setup(&mut v, index, value, store);
        // The two instruction bytes are canonical, the following RIP is not.
        write(&mut v, 0x578, 0x0000_7fff_ffff_fffe);
        let before = *v.bytes();
        let frame = f;
        let owner = format!("{a:?}");
        let result = access(&mut a, &mut v, &mut f, store);
        if fault {
            assert_eq!(result, Err(MsrError::GeneralProtectionRequired));
        } else {
            assert!(matches!(result, Err(MsrError::Continuation(_))));
        }
        assert_eq!(v.bytes(), &before);
        assert_eq!(f, frame);
        assert_eq!(format!("{a:?}"), owner);
    }
}
