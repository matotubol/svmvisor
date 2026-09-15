use svmvisor_hypervisor::{
    address::{AddressPolicy, EncryptionState},
    arch::x86_64::registers::GuestRegisters,
    capabilities::EvidenceFlag,
    guest::pages::{GuestPages, PagePermissions as Gp, TableStorage as Gs},
    memory::npt::{Npt, NptEvidence, PagePermissions as Np, TableStorage as Ns},
    svm::{
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::{FIXTURE_APIC_BASE, FixtureApic, handle_fixture_msr},
        xapic::{FIXTURE_MMIO_BASE, FixtureMmioMapping, MmioError, handle_fixture_mmio},
    },
};

const ALIAS: u64 = 0xc000;
#[repr(C, align(4096))]
struct Code([u8; 4096]);

// Inert images only: unit tests do not establish a real VMRUN/NPF or delivery.
fn write(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn mapping(code: &Code, variant: u8) -> Result<FixtureMmioMapping, MmioError> {
    let policy = AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let mut gs = Gs([[0; 4096]; 4]);
    let mut ns = Ns([[0; 4096]; 8]);
    let mut pages = GuestPages::new(&mut gs, 0x10000, policy).unwrap();
    pages
        .map_page(
            ALIAS,
            if variant == 1 {
                0xfee0_1000
            } else {
                FIXTURE_MMIO_BASE
            },
            if variant == 2 {
                Gp::ReadOnly
            } else {
                Gp::ReadWrite
            },
        )
        .unwrap();
    pages
        .map_page(
            0x1000,
            0x1000,
            if variant == 3 {
                Gp::ReadWrite
            } else {
                Gp::ReadOnly
            },
        )
        .unwrap();
    let mut npt = Npt::new(
        &mut ns,
        0x20000,
        policy,
        48,
        NptEvidence {
            nx_supported: EvidenceFlag::Set,
            host_nxe: EvidenceFlag::Set,
            host_four_level: EvidenceFlag::Set,
        },
    )
    .unwrap();
    if variant != 6 {
        npt.map_page(
            0x1000,
            if variant == 7 {
                FIXTURE_MMIO_BASE
            } else {
                code.0.as_ptr() as u64
            },
            if variant == 4 {
                Np::ReadWrite
            } else {
                Np::ReadExecute
            },
        )
        .unwrap();
    }
    if variant == 5 {
        npt.map_page(FIXTURE_MMIO_BASE, 0x30000, Np::ReadWrite)
            .unwrap();
    }
    FixtureMmioMapping::admit(
        &pages,
        &npt,
        if variant == 8 { ALIAS + 1 } else { ALIAS },
        if variant == 9 { 0x1001 } else { 0x1000 },
    )
}
struct Fixture {
    code: Box<Code>,
    mapping: FixtureMmioMapping,
    apic: FixtureApic,
    vmcb: Vmcb,
    frame: GuestRegisters,
}
impl Fixture {
    fn new() -> Self {
        let mut code = Box::new(Code([0; 4096]));
        code.0[..4].copy_from_slice(&[0x8b, 0x03, 0x89, 0x03]);
        let mapping = mapping(&code, 0).unwrap();
        let mut vmcb = Vmcb::new();
        write(&mut vmcb, 0x4d0, 1 << 10);
        write(&mut vmcb, 0x410, 1 << 25);
        write(&mut vmcb, 0x548, 1 << 5);
        write(&mut vmcb, 0x558, (1 << 31) | 1);
        write(&mut vmcb, 0x550, 0x10000);
        write(&mut vmcb, 0xb0, 0x20000);
        write(&mut vmcb, 0x90, 1);
        write(&mut vmcb, 0x60, 1 << 24);
        write(&mut vmcb, 0x570, 0x246);
        write(&mut vmcb, 0xc8, u64::MAX); // NPF does not establish nRIP.
        let mut result = Self {
            code,
            mapping,
            vmcb,
            apic: FixtureApic::admit_fixed_bsp(
                LocalApic::admit_enabled(),
                FIXTURE_APIC_BASE & !(1 << 10),
            )
            .unwrap(),
            frame: GuestRegisters {
                rdx: u64::MAX,
                rcx: 17,
                r15: 99,
                ..GuestRegisters::default()
            },
        };
        result.access(0x20, false, 0);
        result
    }
    fn access(&mut self, offset: u16, store: bool, value: u64) {
        write(&mut self.vmcb, 0x70, 0x400);
        write(
            &mut self.vmcb,
            0x78,
            (1 << 32) | 4 | if store { 2 } else { 0 },
        );
        write(&mut self.vmcb, 0x80, FIXTURE_MMIO_BASE + offset as u64);
        write(&mut self.vmcb, 0x578, 0x1000 + if store { 2 } else { 0 });
        write(&mut self.vmcb, 0x5f8, value);
        self.frame.rbx = ALIAS + offset as u64;
    }
    fn call(&mut self) -> Result<(), MmioError> {
        let offset = (self.vmcb.guest_rip() & 4095) as usize;
        handle_fixture_mmio(
            &mut self.apic,
            &mut self.vmcb,
            &self.frame,
            &self.code.0[offset..offset + 2],
            &self.mapping,
        )
    }
    fn refused(&mut self) -> MmioError {
        let before = *self.vmcb.bytes();
        let frame = self.frame;
        let owner = format!("{:?}", self.apic);
        let error = self.call().unwrap_err();
        assert_eq!(self.vmcb.bytes(), &before);
        assert_eq!(self.frame, frame);
        assert_eq!(format!("{:?}", self.apic), owner);
        error
    }
    fn read(&mut self, offset: u16) -> u64 {
        self.access(offset, false, u64::MAX);
        self.call().unwrap();
        self.vmcb.guest_rax()
    }
    fn store(&mut self, offset: u16, value: u64) {
        self.access(offset, true, value);
        let frame = self.frame;
        self.call().unwrap();
        assert_eq!(self.vmcb.guest_rax(), value);
        assert_eq!(self.frame, frame);
    }
    fn consume(&mut self, vector: u8) {
        self.apic.queue(vector).unwrap();
        assert_eq!(self.apic.arm(&mut self.vmcb), Ok(Some(vector)));
        let ctl = self.vmcb.virtual_interrupt_control();
        write(&mut self.vmcb, 0x60, ctl & !(1 << 8));
        assert_eq!(self.apic.observe(&self.vmcb), Ok(Some(vector)));
    }
}

#[test]
fn mapping_admission_rejects_wrong_alias_permissions_backing_and_alignment() {
    let code = Box::new(Code([0; 4096]));
    for variant in 1..=9 {
        assert_eq!(
            mapping(&code, variant),
            Err(MmioError::Mapping),
            "variant {variant}"
        );
    }
}

#[test]
fn identity_tpr_ppr_zero_extension_and_write_register_flags_preservation() {
    let mut f = Fixture::new();
    assert_eq!(f.read(0x20), 0);
    f.store(0x80, 0xaabb_ccdd_0000_005b);
    assert_eq!(f.apic.controller().task_priority(), 0x5b);
    assert_eq!(f.vmcb.virtual_interrupt_control() & 15, 5);
    assert_eq!(f.read(0x80), 0x5b);
    assert_eq!(f.read(0xa0), 0x5b);
    assert_eq!(f.vmcb.guest_rip(), 0x1002);
    assert_eq!(&f.vmcb.bytes()[0x570..0x578], &0x246u64.to_le_bytes());
    assert_eq!(f.frame.rdx, u64::MAX);
}

#[test]
fn all_bitmap_windows_highest_eoi_and_empty_eoi_share_one_controller() {
    let mut f = Fixture::new();
    f.consume(0x50);
    f.consume(0xe0);
    for vector in [32, 63, 64, 95, 96, 127, 128, 159, 160, 191, 192, 223, 255] {
        f.apic.queue(vector).unwrap();
    }
    for i in 0..8 {
        let expected = [32u8, 63, 64, 95, 96, 127, 128, 159, 160, 191, 192, 223, 255]
            .into_iter()
            .filter(|v| *v as u16 / 32 == i)
            .fold(0u64, |word, v| word | (1 << (v % 32)));
        assert_eq!(f.read(0x200 + i * 16), expected);
        let service = if i == 2 {
            1 << 16
        } else if i == 7 {
            1
        } else {
            0
        };
        assert_eq!(f.read(0x100 + i * 16), service);
    }
    assert_eq!(f.read(0xa0), 0xe0);
    f.store(0xb0, 0xffff_ffff_0000_0000);
    assert_eq!(f.apic.controller().eoi_target(), Some(0x50));
    assert_eq!(f.read(0xa0), 0x50);
    f.store(0xb0, 0);
    f.store(0xb0, 0);
    assert_eq!(f.apic.controller().eoi_target(), None);
    assert!(f.apic.controller().pending(255));
}

#[test]
fn cross_bus_identity_tpr_and_irr_survive_mode_transitions() {
    let mut f = Fixture::new();
    f.apic.queue(0x7f).unwrap();
    f.store(0x80, 0x3b);
    for base in [
        FIXTURE_APIC_BASE,
        FIXTURE_APIC_BASE & !(3 << 10),
        FIXTURE_APIC_BASE & !(1 << 10),
    ] {
        f.frame.rcx = 0x1b;
        f.frame.rdx = 0;
        write(&mut f.vmcb, 0x70, 0x7c);
        write(&mut f.vmcb, 0x78, 1);
        write(&mut f.vmcb, 0x5f8, base);
        handle_fixture_msr(&mut f.apic, &mut f.vmcb, &mut f.frame, &[0x0f, 0x30]).unwrap();
        assert_eq!(f.apic.identity(), 0);
        if base == FIXTURE_APIC_BASE {
            for (index, expected) in [(0x802, 0), (0x808, 0x3b), (0x823, 1 << 31)] {
                f.frame.rcx = index;
                write(&mut f.vmcb, 0x78, 0);
                handle_fixture_msr(&mut f.apic, &mut f.vmcb, &mut f.frame, &[0x0f, 0x32]).unwrap();
                assert_eq!(f.vmcb.guest_rax(), expected);
                assert_eq!(f.frame.rdx, 0);
            }
        }
    }
    assert_eq!(f.read(0x20), 0);
    assert_eq!(f.read(0x80), 0x3b);
    assert_eq!(f.read(0x230), 1 << 31);
}

#[test]
fn unknown_registers_ro_writes_eoi_values_and_alignment_refuse_transactionally() {
    let mut f = Fixture::new();
    for (offset, store, value) in [
        (0x20, true, 0),
        (0xa0, true, 0),
        (0x100, true, 0),
        (0x200, true, 0),
        (0x80, true, 256),
        (0xb0, true, 1),
        (0xb0, false, 0),
        (0x330, false, 0),
        (0x340, false, 0),
        (0x90, false, 0),
        (0x180, false, 0),
        (0x280, false, 0),
        (0xff0, false, 0),
        (0x81, false, 0),
        (0x84, false, 0),
        (0xfff, false, 0),
    ] {
        f.access(offset, store, value);
        f.refused();
    }
}

#[test]
fn every_unadmitted_npf_bit_and_wrong_direction_are_rejected() {
    for store in [false, true] {
        for bit in 0..64 {
            let mut f = Fixture::new();
            f.access(0x80, store, 0x20);
            let valid = (1 << 32) | 4 | if store { 2 } else { 0 };
            write(&mut f.vmcb, 0x78, valid ^ (1u64 << bit));
            assert_eq!(f.refused(), MmioError::NestedFault);
        }
    }
}

#[test]
fn wrong_exit_gpa_roots_mode_controls_events_and_tpr_are_rejected() {
    for (offset, value) in [
        (0x70, 0x7c),
        (0x80, FIXTURE_MMIO_BASE + 0x90),
        (0x550, 0x11000),
        (0xb0, 0x21000),
        (0x90, 0),
        (0x90, 3),
        (0x4d0, 0),
        (0x410, 0),
        (0x4c8, 3 << 24),
        (0x548, 1 << 12),
        (0x60, 0),
        (0x60, (1 << 24) | 1),
        (0x60, (1 << 24) | (1 << 31)),
        (0xa8, 1 << 31),
        (0x88, 1 << 31),
    ] {
        for store in [false, true] {
            let mut f = Fixture::new();
            f.access(0x80, store, 0x20);
            write(&mut f.vmcb, offset, value);
            f.refused();
        }
    }
    for base in [FIXTURE_APIC_BASE, FIXTURE_APIC_BASE & !(3 << 10)] {
        let mut f = Fixture::new();
        f.apic = FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), base).unwrap();
        assert_eq!(f.refused(), MmioError::UnsupportedApicMode);
    }
}

#[test]
fn owned_code_pointer_operand_alias_and_instruction_forms_are_checked() {
    let mut f = Fixture::new();
    let before = *f.vmcb.bytes();
    assert_eq!(
        handle_fixture_mmio(
            &mut f.apic,
            &mut f.vmcb,
            &f.frame,
            &[0x8b, 0x03],
            &f.mapping
        ),
        Err(MmioError::InstructionProvenance)
    );
    assert_eq!(f.vmcb.bytes(), &before);
    for rbx in [
        0x20,
        FIXTURE_MMIO_BASE + 0x20,
        ALIAS - 16,
        ALIAS + 4096,
        u64::MAX,
    ] {
        f.frame.rbx = rbx;
        assert_eq!(f.refused(), MmioError::OperandAddress);
    }
    f.access(0x80, true, 0x20);
    for bytes in [
        &[0x48, 0x89, 0x03][..],
        &[0x66, 0x89, 0x03],
        &[0x88, 0x03],
        &[0x8b, 0x0b],
        &[0xf0, 0x89, 0x03],
        &[],
        &[0x8b],
    ] {
        let before = *f.vmcb.bytes();
        assert!(
            handle_fixture_mmio(&mut f.apic, &mut f.vmcb, &f.frame, bytes, &f.mapping).is_err()
        );
        assert_eq!(f.vmcb.bytes(), &before);
    }
}

#[test]
fn pending_and_unobserved_delivery_writes_refuse_but_retained_reads_are_allowed() {
    let mut f = Fixture::new();
    f.apic.queue(0x50).unwrap();
    f.apic.arm(&mut f.vmcb).unwrap();
    assert_eq!(f.read(0x220), 1 << 16);
    for offset in [0x80, 0xb0] {
        f.access(offset, true, 0);
        f.refused();
    }
    let ctl = f.vmcb.virtual_interrupt_control();
    write(&mut f.vmcb, 0x60, ctl & !(1 << 8));
    for offset in [0x80, 0xb0] {
        f.access(offset, true, 0);
        f.refused();
    }
    f.apic.observe(&f.vmcb).unwrap();
    f.store(0xb0, 0);
}

#[test]
fn invalid_continuation_and_code_page_boundary_preserve_stopped_state() {
    let mut f = Fixture::new();
    f.consume(0x50);
    for (register, store, value) in [(0x20, false, 0), (0x80, true, 0x3b), (0xb0, true, 0)] {
        f.access(register, store, value);
        let instruction_offset = if store { 2 } else { 0 };
        for rip in [0x0000_7fff_ffff_fffe, 0x0000_8000_0000_0000, u64::MAX] {
            write(&mut f.vmcb, 0x578, rip);
            let before = *f.vmcb.bytes();
            let frame = f.frame;
            let controller = format!("{:?}", f.apic);
            assert!(
                handle_fixture_mmio(
                    &mut f.apic,
                    &mut f.vmcb,
                    &f.frame,
                    &f.code.0[instruction_offset..instruction_offset + 2],
                    &f.mapping
                )
                .is_err()
            );
            assert_eq!(f.vmcb.bytes(), &before);
            assert_eq!(f.frame, frame);
            assert_eq!(format!("{:?}", f.apic), controller);
        }
    }
    f.access(0x20, false, 0);
    for rip in [0xffe, 0x1fff, 0x2000] {
        write(&mut f.vmcb, 0x578, rip);
        let before = *f.vmcb.bytes();
        assert_eq!(
            handle_fixture_mmio(
                &mut f.apic,
                &mut f.vmcb,
                &f.frame,
                &f.code.0[..2],
                &f.mapping
            ),
            Err(MmioError::InstructionProvenance)
        );
        assert_eq!(f.vmcb.bytes(), &before);
    }
}

#[test]
fn timer_and_svr_mmio_writes_drive_shared_msr_state_across_mode_transition() {
    let mut f = Fixture::new();
    for (offset, value) in [
        (0x30, 0x10),
        (0xf0, 0x1ff),
        (0x320, 0x10000),
        (0x380, 0),
        (0x390, 0),
        (0x3e0, 0),
    ] {
        assert_eq!(f.read(offset), value);
    }
    for (offset, value) in [(0x3e0, 11), (0x320, 0x20050), (0x380, 3), (0xf0, 0xff)] {
        f.access(offset, true, 0xdead_beef_0000_0000 | value);
        let old = f.frame;
        let flags = field(&f.vmcb, 0x570);
        f.call().unwrap();
        assert_eq!(f.frame, old);
        assert_eq!(field(&f.vmcb, 0x570), flags);
        assert_eq!(f.vmcb.guest_rax(), 0xdead_beef_0000_0000 | value);
    }
    assert_eq!(f.read(0x320), 0x30050);
    assert_eq!(
        f.apic.advance_timer(3),
        Ok(svmvisor_hypervisor::svm::local_apic::TickOutcome::MaskedExpiration)
    );
    assert_eq!(f.read(0x390), 3);
    // Same sole owner survives xAPIC -> x2APIC; read every added register.
    write(&mut f.vmcb, 0x70, 0x7c);
    write(&mut f.vmcb, 0x78, 1);
    write(&mut f.vmcb, 0x5f8, FIXTURE_APIC_BASE);
    f.frame.rcx = 0x1b;
    f.frame.rdx = 0;
    handle_fixture_msr(&mut f.apic, &mut f.vmcb, &mut f.frame, &[0x0f, 0x30]).unwrap();
    for (index, value) in [
        (0x803, 0x10),
        (0x80f, 0xff),
        (0x832, 0x30050),
        (0x838, 3),
        (0x839, 3),
        (0x83e, 11),
    ] {
        f.frame.rcx = index;
        write(&mut f.vmcb, 0x78, 0);
        handle_fixture_msr(&mut f.apic, &mut f.vmcb, &mut f.frame, &[0x0f, 0x32]).unwrap();
        assert_eq!(f.vmcb.guest_rax(), value);
        assert_eq!(f.frame.rdx, 0);
    }
}

#[test]
fn new_mmio_invalid_register_values_are_policy_refusals_without_mutation() {
    for (offset, value) in [
        (0x30, 0),
        (0x390, 0),
        (0xf0, 1 << 10),
        (0xf0, 1 << 12),
        (0x320, 1 << 12),
        (0x320, 1 << 18),
        (0x320, 1 << 8),
        (0x320, 0),
        (0x320, 16),
        (0x3e0, 4),
        (0x330, 0),
        (0x340, 0),
        (0x350, 0),
        (0x360, 0),
        (0x370, 0),
    ] {
        let mut f = Fixture::new();
        f.access(offset, true, value);
        assert_eq!(
            f.refused(),
            MmioError::UnsupportedRegister {
                offset,
                write: true
            }
        );
    }
    for (offset, value) in [(0x320, 0x51), (0x320, 0x20050), (0x3e0, 1)] {
        let mut f = Fixture::new();
        f.access(0x320, true, 0x50);
        f.call().unwrap();
        f.access(0x380, true, 5);
        f.call().unwrap();
        f.access(offset, true, value);
        assert_eq!(
            f.refused(),
            MmioError::UnsupportedRegister {
                offset,
                write: true
            }
        );
    }
}

fn field(v: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(v.bytes()[offset..offset + 8].try_into().unwrap())
}

#[test]
fn icr_high_latches_low_routes_atomically_and_bus_identity_encodings_agree() {
    use svmvisor_hypervisor::svm::{
        ipi::{IpiTarget, StartupState},
        xapic::handle_fixture_mmio_with_target,
    };
    let mut f = Fixture::new();
    let mut remote =
        FixtureApic::admit_fixed_cpu(LocalApic::admit_enabled(), FIXTURE_APIC_BASE & !(1 << 8), 1)
            .unwrap();
    let mut remote_vmcb = Vmcb::new();
    let mut remote_frame = GuestRegisters::default();
    let mut state = StartupState::Running;
    f.store(0x310, 1 << 24);
    assert_eq!(f.read(0x310), 1 << 24);
    assert!(!remote.controller().pending(0x91));
    for low in [0x91, 0x91 | (1 << 11)] {
        f.access(0x300, true, low);
        let source_before = *f.vmcb.bytes();
        let target_before = *remote_vmcb.bytes();
        let controllers = format!("{:?}{:?}", f.apic, remote);
        let result = handle_fixture_mmio_with_target(
            &mut f.apic,
            &mut f.vmcb,
            &f.frame,
            &f.code.0[2..4],
            &f.mapping,
            &mut IpiTarget {
                apic: &mut remote,
                vmcb: &mut remote_vmcb,
                frame: &mut remote_frame,
                startup: &mut state,
                signature: 0,
            },
        );
        if low == 0x91 {
            result.unwrap();
            assert!(remote.controller().pending(0x91));
            assert_eq!(f.apic.icr(), (1 << 32) | 0x91);
            assert_eq!(f.vmcb.guest_rip(), 0x1004);
        } else {
            assert!(result.is_err());
            assert_eq!(f.vmcb.bytes(), &source_before);
            assert_eq!(remote_vmcb.bytes(), &target_before);
            assert_eq!(controllers, format!("{:?}{:?}", f.apic, remote));
        }
    }
    assert_eq!(f.read(0x300), 0x91);
    assert_eq!(f.read(0x310), 1 << 24);
    // Entering x2APIC preserves the low half but does not preserve ICR high.
    write(&mut f.vmcb, 0x70, 0x7c);
    write(&mut f.vmcb, 0x78, 1);
    write(&mut f.vmcb, 0x578, 0x1000);
    write(&mut f.vmcb, 0x5f8, FIXTURE_APIC_BASE);
    f.frame.rcx = 0x1b;
    f.frame.rdx = 0;
    handle_fixture_msr(&mut f.apic, &mut f.vmcb, &mut f.frame, &[0x0f, 0x30]).unwrap();
    assert_eq!(f.apic.icr(), 0x91);
    write(&mut f.vmcb, 0x78, 0);
    f.frame.rcx = 0x830;
    handle_fixture_msr(&mut f.apic, &mut f.vmcb, &mut f.frame, &[0x0f, 0x32]).unwrap();
    assert_eq!(f.vmcb.guest_rax(), 0x91);
    assert_eq!(f.frame.rdx, 0);
}
