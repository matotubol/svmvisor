use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        dispatch::NativeMsrOutcome,
        ipi::{NativeIcr, NativeIcrError, handle_native_x2apic_write},
        permission_maps::Msrpm,
        vmcb::Vmcb,
    },
};

const APIC_BASE: u64 = 0xfee0_0d00;

// Hardware-saved stopped fields only; the public VMCB API is immutable.
fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

fn stopped(index: u32, value: u64) -> (NativeIcr, Vmcb, GuestRegisters) {
    let owner = NativeIcr::admit(7, &[7, 19, 255, 0x1234]).unwrap();
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x70, 0x7c);
    put(&mut vmcb, 0x78, 1);
    put(&mut vmcb, 0x410, 0x029b_0008); // CS.L, present executable code.
    put(&mut vmcb, 0x4d0, 0x1500);
    put(&mut vmcb, 0x558, 0x8000_0001);
    put(&mut vmcb, 0x578, 0x1234_5000);
    put(&mut vmcb, 0x570, 0x1_0002); // RF consumed only on completion.
    put(&mut vmcb, 0x68, 1); // Completed instruction consumes STI shadow.
    put(
        &mut vmcb,
        0x5f8,
        0xaabb_ccdd_0000_0000 | value as u32 as u64,
    );
    put(&mut vmcb, 0xc8, u64::MAX); // No nRIP dependency.
    let frame = GuestRegisters {
        rcx: 0xfeed_0000_0000_0000 | index as u64,
        rdx: 0xcafe_0000_0000_0000 | (value >> 32),
        rbx: 0xdead_beef,
        ..GuestRegisters::default()
    };
    (owner, vmcb, frame)
}

#[test]
fn ordinary_native_icr_keeps_physical_logical_and_shorthand_encodings() {
    for value in [
        (19u64 << 32) | 0x4040,     // fixed unicast, edge level bit ignored
        (0x1234u64 << 32) | 0x0841, // logical fixed
        0x0004_0042,                // self fixed
        0x000c_0043,                // all excluding self fixed
        (19u64 << 32) | 0x0400,     // native NMI
    ] {
        let (owner, mut vmcb, frame) = stopped(0x830, value);
        let original_frame = frame;
        let rax = vmcb.guest_rax();
        let mut writes = Vec::new();
        assert_eq!(
            handle_native_x2apic_write(
                &owner,
                APIC_BASE,
                &mut vmcb,
                &frame,
                &[0x0f, 0x30],
                |written| writes.push(written)
            ),
            Ok(NativeMsrOutcome::Completed)
        );
        assert_eq!(writes, [value]);
        assert_eq!(frame, original_frame);
        assert_eq!(vmcb.guest_rax(), rax);
        assert_eq!(vmcb.guest_rip(), 0x1234_5002);
        assert_eq!(
            u64::from_le_bytes(vmcb.bytes()[0x570..0x578].try_into().unwrap()),
            2
        );
        assert_eq!(vmcb.bytes()[0x68], 0);
    }
}

#[test]
fn assigned_init_and_sipi_never_relabel_cold_or_touch_physical_target() {
    for command in [0x500u64, 0x640, 0x4500, 0x8500, 0xc500] {
        let value = (19u64 << 32) | command;
        let (owner, mut vmcb, frame) = stopped(0x830, value);
        let original = *vmcb.bytes();
        assert_eq!(
            handle_native_x2apic_write(
                &owner,
                APIC_BASE,
                &mut vmcb,
                &frame,
                &[0x0f, 0x30],
                |_| panic!("physical startup escaped")
            ),
            Err(NativeIcrError::AssignedStartup {
                destination: 19,
                command: ((command >> 8) & 7) as u8,
            })
        );
        assert_eq!(*vmcb.bytes(), original);
    }
}

#[test]
fn broadcast_logical_unowned_startup_and_self_reset_stay_stopped() {
    for value in [
        (89u64 << 32) | 0x500,
        0xc0500,
        0x80500,
        (19u64 << 32) | 0xd00,
        0xffff_ffff_0000_0500,
    ] {
        let (owner, mut vmcb, frame) = stopped(0x830, value);
        let original = *vmcb.bytes();
        assert_eq!(
            handle_native_x2apic_write(
                &owner,
                APIC_BASE,
                &mut vmcb,
                &frame,
                &[0x0f, 0x30],
                |_| panic!("startup forwarded")
            ),
            Err(NativeIcrError::UnownedStartup { value })
        );
        assert_eq!(*vmcb.bytes(), original);
    }
    let (owner, mut vmcb, frame) = stopped(0x830, 0x40500);
    assert_eq!(
        handle_native_x2apic_write(
            &owner,
            APIC_BASE,
            &mut vmcb,
            &frame,
            &[0x0f, 0x30],
            |_| panic!("self reset")
        ),
        Err(NativeIcrError::RunningStartup {
            destination: 7,
            command: 5
        })
    );
}

#[test]
fn base_mode_changes_cannot_bypass_startup_owner_but_identical_write_completes() {
    for value in [APIC_BASE & !0x400, APIC_BASE & !0xc00, APIC_BASE + 0x1000] {
        let (owner, mut vmcb, frame) = stopped(0x1b, value);
        let original = *vmcb.bytes();
        assert_eq!(
            handle_native_x2apic_write(
                &owner,
                APIC_BASE,
                &mut vmcb,
                &frame,
                &[0x0f, 0x30],
                |_| panic!("base write issued")
            ),
            Err(NativeIcrError::ApicBaseChange)
        );
        assert_eq!(*vmcb.bytes(), original);
    }
    let (owner, mut vmcb, frame) = stopped(0x1b, APIC_BASE);
    assert_eq!(
        handle_native_x2apic_write(
            &owner,
            APIC_BASE,
            &mut vmcb,
            &frame,
            &[0x0f, 0x30],
            |_| panic!("unneeded hardware write")
        ),
        Ok(NativeMsrOutcome::Completed)
    );
}

#[test]
fn reserved_x2apic_bits_fault_at_original_rip_without_hardware_write() {
    for value in [0x1040, 0x1_0040, 0x2040, 0x140, 0x340, 0x740, 1 << 31] {
        let (owner, mut vmcb, frame) = stopped(0x830, value);
        let rax = vmcb.guest_rax();
        assert_eq!(
            handle_native_x2apic_write(
                &owner,
                APIC_BASE,
                &mut vmcb,
                &frame,
                &[0x0f, 0x30],
                |_| panic!("invalid MSR write")
            ),
            Ok(NativeMsrOutcome::GeneralProtectionPrepared)
        );
        assert_eq!(vmcb.guest_rax(), rax);
        assert_eq!(vmcb.guest_rip(), 0x1234_5000);
        assert_eq!(vmcb.event_injection(), 0x8000_0b0d);
    }
}

#[test]
fn late_instruction_and_pending_state_refusal_precedes_physical_write() {
    for case in 0..7 {
        let (owner, mut vmcb, frame) = stopped(0x830, 0x40040);
        let mut instruction: &[u8] = &[0x0f, 0x30];
        match case {
            0 => put(&mut vmcb, 0x578, 0x7fff_ffff_ffff),
            1 => put(&mut vmcb, 0x570, 0x102),
            2 => put(&mut vmcb, 0xa8, 1 << 31),
            3 => put(&mut vmcb, 0x88, 1 << 31),
            4 => put(&mut vmcb, 0x60, 1 << 24),
            5 => put(&mut vmcb, 0x78, 0),
            _ => instruction = &[0x66, 0x0f, 0x30],
        }
        let original = *vmcb.bytes();
        assert!(
            handle_native_x2apic_write(
                &owner,
                APIC_BASE,
                &mut vmcb,
                &frame,
                instruction,
                |_| panic!("write before preflight")
            )
            .is_err()
        );
        assert_eq!(*vmcb.bytes(), original);
    }
}

#[test]
fn inventory_rejects_aliases_and_native_map_covers_startup_and_presentation() {
    for ids in [
        &[][..],
        &[1, 1][..],
        &[2, 3][..],
        &[1, u32::MAX][..],
        &[1; 33][..],
    ] {
        assert!(matches!(
            NativeIcr::admit(1, ids),
            Err(NativeIcrError::InvalidTopology)
        ));
    }
    let mut map = Msrpm::native_boot();
    let mut expected = *map.bytes();
    // Independent Table15-8 bit locations (two bits per covered MSR).
    expected[0x20c] |= 2; // 830h write, read remains direct
    expected[6] |= 0x80; // 1bh write, read remains direct
    for index in [0x803, 0x840, 0x841, 0x842, 0x848, 0x849, 0x84a, 0x84b,
        0x84c, 0x84d, 0x84e, 0x84f, 0x850, 0x851, 0x852, 0x853] {
        expected[index / 4] |= 3 << ((index % 4) * 2);
    }
    map.intercept_native_startup();
    assert_eq!(map.bytes(), &expected);
}
