use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        ipi::{IpiError, IpiTarget, StartupState},
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::{
            FIXTURE_APIC_BASE, FixtureApic, MsrError, handle_fixture_msr,
            handle_fixture_msr_with_target,
        },
    },
};

fn put(v: &mut Vmcb, offset: usize, value: u64) {
    // Inert host-test byte images, never guest-entry evidence.
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn field(v: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(v.bytes()[offset..offset + 8].try_into().unwrap())
}

struct Pair {
    bsp: FixtureApic,
    ap: FixtureApic,
    bsp_v: Vmcb,
    ap_v: Vmcb,
    bsp_f: GuestRegisters,
    ap_f: GuestRegisters,
    state: StartupState,
}
impl Pair {
    fn new() -> Self {
        let mut result = Self {
            bsp: FixtureApic::admit_fixed_bsp(LocalApic::admit_enabled(), FIXTURE_APIC_BASE)
                .unwrap(),
            ap: FixtureApic::admit_fixed_cpu(
                LocalApic::admit_enabled(),
                FIXTURE_APIC_BASE & !(1 << 8),
                1,
            )
            .unwrap(),
            bsp_v: Vmcb::new(),
            ap_v: Vmcb::new(),
            bsp_f: GuestRegisters::default(),
            ap_f: GuestRegisters::default(),
            state: StartupState::Cold,
        };
        put(&mut result.bsp_v, 0x60, 1 << 24);
        put(&mut result.ap_v, 0x60, 1 << 24);
        result
    }
    fn prepare(&mut self, value: u64) {
        put(&mut self.bsp_v, 0x70, 0x7c);
        put(&mut self.bsp_v, 0x78, 1);
        put(&mut self.bsp_v, 0x578, 0x1000);
        put(&mut self.bsp_v, 0x5f8, value as u32 as u64);
        self.bsp_f.rcx = 0x830;
        self.bsp_f.rdx = value >> 32;
    }
    fn call(&mut self) -> Result<(), MsrError> {
        handle_fixture_msr_with_target(
            &mut self.bsp,
            &mut self.bsp_v,
            &mut self.bsp_f,
            &[0x0f, 0x30],
            &mut IpiTarget {
                apic: &mut self.ap,
                vmcb: &mut self.ap_v,
                frame: &mut self.ap_f,
                startup: &mut self.state,
                signature: 0x00a0_0f10,
            },
        )
    }
    fn send(&mut self, low: u32) -> Result<(), MsrError> {
        self.prepare((1 << 32) | low as u64);
        self.call()
    }
    fn refusal(&mut self) -> MsrError {
        let images = (*self.bsp_v.bytes(), *self.ap_v.bytes());
        let frames = (self.bsp_f, self.ap_f);
        let owners = format!("{:?}{:?}{:?}", self.bsp, self.ap, self.state);
        let result = self.call().unwrap_err();
        assert_eq!(
            (&images.0, &images.1),
            (self.bsp_v.bytes(), self.ap_v.bytes())
        );
        assert_eq!(frames, (self.bsp_f, self.ap_f));
        assert_eq!(
            owners,
            format!("{:?}{:?}{:?}", self.bsp, self.ap, self.state)
        );
        result
    }
}

#[test]
fn cold_init_and_sipi_install_real_mode_not_a_long_mode_shortcut() {
    let mut p = Pair::new();
    put(&mut p.ap_v, 0xb0, 0x30000); // owned NPT remains installed
    put(&mut p.ap_v, 0x40, 0x40000); // maps retained
    put(&mut p.ap_v, 0x50, 99); // TSC offset retained by INIT
    put(&mut p.ap_v, 0x600, 0x1234); // retained STAR
    put(&mut p.ap_v, 0x558, 0xe001_0033); // CD/NW retained; other bits reset
    p.ap_f.r15 = u64::MAX;
    p.send(0x4500).unwrap(); // Level ignored on edge INIT
    assert_eq!(p.state, StartupState::AwaitSipi);
    assert_eq!(p.bsp_v.guest_rip(), 0x1002);
    assert_eq!(p.ap_v.guest_rip(), 0xfff0);
    assert_eq!(field(&p.ap_v, 0x558), 0x6000_0010);
    assert_eq!(field(&p.ap_v, 0x4d0), 0x1000);
    assert_eq!(field(&p.ap_v, 0x570), 2);
    assert_eq!(field(&p.ap_v, 0x560), 0x400);
    assert_eq!(field(&p.ap_v, 0x568), 0xffff_0ff0);
    assert_eq!(field(&p.ap_v, 0x418), 0xffff_0000);
    assert_eq!(
        p.ap_v.bytes()[0x410..0x418],
        [0, 0xf0, 0x9a, 0, 0xff, 0xff, 0, 0]
    );
    assert_eq!(
        p.ap_v.bytes()[0x470..0x478],
        [0, 0, 0x82, 0, 0xff, 0xff, 0, 0]
    );
    assert_eq!(
        p.ap_v.bytes()[0x490..0x498],
        [0, 0, 0x83, 0, 0xff, 0xff, 0, 0]
    );
    assert_eq!(p.ap_v.bytes()[0x480..0x488], [0, 0, 0, 0, 0xff, 0xff, 0, 0]);
    assert_eq!(
        p.ap_f,
        GuestRegisters {
            rdx: 0x00a0_0f10,
            ..GuestRegisters::default()
        }
    );
    assert_eq!(p.ap.controller().spurious_vector_register(), 0xff);
    assert_eq!(p.ap.apic_base(), FIXTURE_APIC_BASE & !(1 << 8));
    p.send(0x0601).unwrap();
    assert_eq!(p.state, StartupState::Running);
    assert_eq!(p.ap_v.guest_rip(), 0);
    assert_eq!(field(&p.ap_v, 0x418), 0x1000);
    assert_eq!(
        p.ap_v.bytes()[0x410..0x418],
        [0, 1, 0x9a, 0, 0xff, 0xff, 0, 0]
    );
    assert_eq!(field(&p.ap_v, 0x548), 0);
    assert_eq!(field(&p.ap_v, 0x550), 0);
    assert_eq!(field(&p.ap_v, 0xb0), 0x30000);
    assert_eq!(field(&p.ap_v, 0x40), 0x40000);
    assert_eq!(field(&p.ap_v, 0x50), 99);
    assert_eq!(field(&p.ap_v, 0x600), 0x1234);
}

#[test]
fn repeated_sipi_preserves_the_started_ap_and_running_init_refuses_atomically() {
    let mut p = Pair::new();
    p.send(0x500).unwrap();
    p.send(0x6ff).unwrap();
    assert_eq!(field(&p.ap_v, 0x418), 0xff000);
    put(&mut p.ap_v, 0x578, 0x123);
    p.ap_f.r13 = 0xfeed;
    let before = *p.ap_v.bytes();
    let frame = p.ap_f;
    p.send(0x601).unwrap();
    assert_eq!(p.ap_v.bytes(), &before);
    assert_eq!(p.ap_f, frame);
    p.prepare((1 << 32) | 0x500);
    assert_eq!(p.refusal(), MsrError::Ipi(IpiError::UnsupportedInit));
}

#[test]
fn fixed_ipi_uses_remote_existing_irr_and_coalesces_without_touching_source() {
    let mut p = Pair::new();
    p.state = StartupState::Running; // admitted running target fixture
    let before = *p.ap_v.bytes();
    p.send(0x91).unwrap();
    p.send(0x4091).unwrap(); // Level ignored for edge fixed
    p.send(0xa2).unwrap();
    assert!(p.ap.controller().pending(0x91));
    assert!(p.ap.controller().pending(0xa2));
    assert!(!p.bsp.controller().pending(0x91));
    assert_eq!(p.ap_v.bytes(), &before);
    assert_eq!(p.ap.arm(&mut p.ap_v).unwrap(), Some(0xa2));
    let control = p.ap_v.virtual_interrupt_control();
    put(&mut p.ap_v, 0x60, control & !(1 << 8));
    assert_eq!(p.ap.observe(&p.ap_v).unwrap(), Some(0xa2));
    assert!(p.ap.controller().in_service(0xa2));
    assert!(p.ap.controller().pending(0x91));
}

#[test]
fn reserved_msr_fields_fault_and_unsupported_routes_refuse_both_owners_unchanged() {
    for bits in [
        1 << 12,
        1 << 13,
        1 << 16,
        1 << 17,
        1 << 20,
        1 << 31,
        0x100,
        0x300,
        0x700,
    ] {
        let mut p = Pair::new();
        p.prepare((1 << 32) | bits | 0x81);
        assert_eq!(p.refusal(), MsrError::GeneralProtectionRequired);
    }
    for value in [
        0x81,
        (2 << 32) | 0x81,
        (u32::MAX as u64) << 32 | 0x81,
        (1 << 32) | 0x881,
        (1 << 32) | 0x40081,
        (1 << 32) | 0xc0081,
        (1 << 32) | 0x8000 | 0x81,
        (1 << 32) | 0x400,
        (1 << 32) | 0x200,
    ] {
        let mut p = Pair::new();
        p.state = StartupState::Running;
        p.prepare(value);
        assert!(matches!(p.refusal(), MsrError::Ipi(_)));
    }
}

#[test]
fn startup_order_invalid_vectors_and_live_pending_conflicts_do_not_mutate() {
    let mut p = Pair::new();
    p.prepare((1 << 32) | 0x601);
    assert_eq!(p.refusal(), MsrError::Ipi(IpiError::StartupWithoutInit));
    p.prepare((1 << 32) | 0x81);
    assert_eq!(p.refusal(), MsrError::Ipi(IpiError::TargetNotRunning));
    p.state = StartupState::Running;
    p.prepare((1 << 32) | 0xf);
    assert!(matches!(
        p.refusal(),
        MsrError::Ipi(IpiError::TargetQueue(_))
    ));
    for offset in [0xa8, 0x88] {
        put(&mut p.ap_v, offset, (1 << 31) | 0x8000030d);
        p.prepare((1 << 32) | 0x81);
        assert!(matches!(
            p.refusal(),
            MsrError::Ipi(IpiError::PendingState(_))
        ));
        put(&mut p.ap_v, offset, 0);
    }
    p.ap.queue(0x90).unwrap();
    p.ap.arm(&mut p.ap_v).unwrap();
    p.prepare((1 << 32) | 0x81);
    assert!(matches!(
        p.refusal(),
        MsrError::Ipi(IpiError::PendingState(_))
    ));
}

#[test]
fn bad_sender_continuation_precedes_any_target_commit_and_no_router_refuses() {
    let mut p = Pair::new();
    p.prepare((1 << 32) | 0x500);
    put(&mut p.bsp_v, 0x578, 0x7fff_ffff_ffff);
    assert!(matches!(p.refusal(), MsrError::Continuation(_)));
    put(&mut p.bsp_v, 0x578, 0x1000);
    let before = *p.bsp_v.bytes();
    assert!(matches!(
        handle_fixture_msr(&mut p.bsp, &mut p.bsp_v, &mut p.bsp_f, &[0x0f, 0x30]),
        Err(MsrError::Unsupported {
            index: 0x830,
            write: true
        })
    ));
    assert_eq!(p.bsp_v.bytes(), &before);
    assert_eq!(p.state, StartupState::Cold);
}

#[test]
fn identity_and_bootstrap_flag_admission_are_consistent() {
    assert_eq!(Pair::new().ap.identity(), 1);
    for (id, base) in [
        (1, FIXTURE_APIC_BASE),
        (0, FIXTURE_APIC_BASE & !(1 << 8)),
        (2, FIXTURE_APIC_BASE),
    ] {
        assert!(FixtureApic::admit_fixed_cpu(LocalApic::admit_enabled(), base, id).is_err());
    }
}
