use std::sync::Barrier;
use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        ipi::{IpiError, IpiMailbox, IpiTarget, MailboxTarget, StartupState},
        local_apic::LocalApic,
        vmcb::Vmcb,
        x2apic::{FixtureApic, MsrError, handle_fixture_msr_with_mailbox},
    },
};

fn put(v: &mut Vmcb, offset: usize, value: u64) {
    // Inert stopped images only; no privileged execution in these tests.
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn cpu(id: u8) -> (FixtureApic, Vmcb) {
    let apic = FixtureApic::admit_fixed_cpu(
        LocalApic::admit_enabled(),
        0xfee00c00 | if id == 0 { 0x100 } else { 0 },
        id,
    )
    .unwrap();
    let mut v = Vmcb::new();
    put(&mut v, 0x60, 1 << 24);
    (apic, v)
}
fn send(
    apic: &mut FixtureApic,
    v: &mut Vmcb,
    value: u64,
    mailbox: &IpiMailbox,
    rip: u64,
) -> (Result<(), MsrError>, bool) {
    put(v, 0x70, 0x7c);
    put(v, 0x78, 1);
    put(v, 0x578, rip);
    put(v, 0x5f8, value as u32 as u64);
    let mut frame = GuestRegisters {
        rcx: 0x830,
        rdx: value >> 32,
        ..GuestRegisters::default()
    };
    let mut target = MailboxTarget::new(mailbox);
    let result = handle_fixture_msr_with_mailbox(apic, v, &mut frame, &[0x0f, 0x30], &mut target);
    assert_eq!(frame.rcx, 0x830);
    assert_eq!(frame.rdx, value >> 32);
    (result, target.published())
}

#[test]
fn transport_coalesces_but_every_accepted_command_requires_a_kick() {
    let mailbox = IpiMailbox::new(1);
    let (mut source, mut sv) = cpu(0);
    let (mut target, tv) = cpu(1);
    for _ in 0..2 {
        assert_eq!(
            send(&mut source, &mut sv, (1u64 << 32) | 0x50, &mailbox, 0x1000),
            (Ok(()), true)
        );
        assert_eq!(sv.guest_rip(), 0x1002);
    }
    assert!(mailbox.pending());
    assert!(!target.controller().pending(0x50));
    assert_eq!(mailbox.drain_into(&mut target, &tv), Ok(1));
    assert!(target.controller().pending(0x50));
    assert!(!mailbox.pending());
    assert_eq!(mailbox.drain_into(&mut target, &tv), Ok(0));
}

#[test]
fn destination_and_continuation_refusals_cannot_publish_or_commit_icr() {
    let mailbox = IpiMailbox::new(1);
    for value in [
        (2u64 << 32) | 0x50,
        (1u64 << 32) | 0x500,
        (1u64 << 32) | 0x10,
        (1u64 << 32) | 0x1050,
        (1u64 << 32) | 0x850,
    ] {
        let (mut source, mut sv) = cpu(0);
        assert!(
            send(&mut source, &mut sv, value, &mailbox, 0x1000)
                .0
                .is_err()
        );
        assert!(!mailbox.pending());
        assert_eq!(source.icr(), 0);
        assert_eq!(sv.guest_rip(), 0x1000);
    }
    let (mut source, mut sv) = cpu(0);
    let (result, published) = send(
        &mut source,
        &mut sv,
        (1u64 << 32) | 0x50,
        &mailbox,
        0x7fff_ffff_ffff,
    );
    assert!(matches!(result, Err(MsrError::Continuation(_))));
    assert!(!published && !mailbox.pending());
    assert_eq!(source.icr(), 0);
    assert_eq!(sv.guest_rip(), 0x7fff_ffff_ffff);
}

#[test]
fn receiver_refusal_keeps_request_until_local_state_is_settled() {
    let mailbox = IpiMailbox::new(1);
    let (mut source, mut sv) = cpu(0);
    let (mut target, mut tv) = cpu(1);
    assert!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x50, &mailbox, 0x1000)
            .0
            .is_ok()
    );
    put(&mut tv, 0xa8, (1u64 << 31) | 0x800 | 6);
    let before = tv.bytes().to_vec();
    let apic_before = format!("{target:?}");
    assert!(matches!(
        mailbox.drain_into(&mut target, &tv),
        Err(IpiError::PendingState(_))
    ));
    assert!(mailbox.pending());
    assert_eq!(&tv.bytes()[..], before);
    assert_eq!(format!("{target:?}"), apic_before);
    put(&mut tv, 0xa8, 0);
    target.queue(0x60).unwrap();
    target.arm(&mut tv).unwrap();
    assert!(mailbox.drain_into(&mut target, &tv).is_err());
    assert!(mailbox.pending());
    // Simulate a completed stopped exit which did not consume V_IRQ.
    put(&mut tv, 0x70, 0x60);
    assert_eq!(target.observe(&tv), Ok(None));
    target.defer_after_exit(&mut tv).unwrap();
    assert_eq!(mailbox.drain_into(&mut target, &tv), Ok(1));
    assert!(target.controller().pending(0x50));
    assert!(target.controller().pending(0x60));
}

#[test]
fn publication_before_and_after_a_drain_remains_observable() {
    let mailbox = IpiMailbox::new(1);
    let (mut source, mut sv) = cpu(0);
    let (mut target, tv) = cpu(1);
    assert_eq!(mailbox.drain_into(&mut target, &tv), Ok(0));
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x50, &mailbox, 0x1000),
        (Ok(()), true)
    );
    assert!(mailbox.pending());
    assert_eq!(mailbox.drain_into(&mut target, &tv), Ok(1));
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x50, &mailbox, 0x1000),
        (Ok(()), true)
    );
    assert_eq!(mailbox.drain_into(&mut target, &tv), Ok(1));
    assert!(target.controller().pending(0x50));
}

#[test]
fn concurrent_publication_and_drain_preserve_every_supported_vector() {
    let mailbox = IpiMailbox::new(1);
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let producer = scope.spawn(|| {
            let (mut source, mut sv) = cpu(0);
            barrier.wait();
            for _ in 0..16 {
                for vector in 32u64..256 {
                    assert_eq!(
                        send(
                            &mut source,
                            &mut sv,
                            (1u64 << 32) | vector,
                            &mailbox,
                            0x1000
                        ),
                        (Ok(()), true)
                    );
                }
            }
        });
        let (mut target, tv) = cpu(1);
        barrier.wait();
        for _ in 0..8192 {
            mailbox.drain_into(&mut target, &tv).unwrap();
            core::hint::spin_loop();
        }
        // A join precedes final drain, so every producer write is accounted for.
        producer.join().unwrap();
        mailbox.drain_into(&mut target, &tv).unwrap();
        assert!(!mailbox.pending());
        for vector in 32u8..=255 {
            assert!(
                target.controller().pending(vector),
                "missing vector {vector}"
            );
        }
    });
}

fn drain_startup(
    mailbox: &IpiMailbox,
    apic: &mut FixtureApic,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    startup: &mut StartupState,
) -> Result<usize, IpiError> {
    mailbox.drain_startup(&mut IpiTarget {
        apic,
        vmcb,
        frame,
        startup,
        signature: 0x00a00f10,
    })
}

#[test]
fn asynchronous_init_and_first_sipi_apply_in_order_without_remote_state_access() {
    let mailbox = IpiMailbox::new_cold_ap();
    let (mut source, mut sv) = cpu(0);
    let (mut target, mut tv) = cpu(1);
    let mut frame = GuestRegisters {
        rbx: 0x1234,
        ..GuestRegisters::default()
    };
    let mut startup = StartupState::Cold;
    let original = *tv.bytes();
    for low in [0x500, 0x623, 0x645] {
        assert_eq!(
            send(&mut source, &mut sv, (1u64 << 32) | low, &mailbox, 0x1000),
            (Ok(()), true)
        );
    }
    assert_eq!(tv.bytes(), &original);
    assert_eq!(frame.rbx, 0x1234);
    assert_eq!(startup, StartupState::Cold);
    assert_eq!(
        drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup),
        Ok(2)
    );
    assert_eq!(startup, StartupState::Running);
    assert_eq!(
        u64::from_le_bytes(tv.bytes()[0x418..0x420].try_into().unwrap()),
        0x23000
    );
    assert_eq!(tv.guest_rip(), 0);
    assert_eq!(frame.rdx, 0x00a00f10);
    assert_eq!(frame.rbx, 0);
    assert!(!mailbox.pending());
    let started = *tv.bytes();
    frame.rbx = 0xdeadbeef;
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x699, &mailbox, 0x2000),
        (Ok(()), true)
    );
    assert_eq!(
        drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup),
        Ok(0)
    );
    assert_eq!(tv.bytes(), &started);
    assert_eq!(frame.rbx, 0xdeadbeef);
}

#[test]
fn startup_refusals_preserve_source_and_pending_owner_state() {
    let mailbox = IpiMailbox::new_cold_ap();
    let (mut source, mut sv) = cpu(0);
    for low in [0x601, 0x501, 0x50] {
        let (result, published) = send(&mut source, &mut sv, (1u64 << 32) | low, &mailbox, 0x1000);
        assert!(result.is_err());
        assert!(!published && !mailbox.pending());
        assert_eq!(source.icr(), 0);
        assert_eq!(sv.guest_rip(), 0x1000);
    }
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x500, &mailbox, 0x1000),
        (Ok(()), true)
    );
    let accepted_icr = source.icr();
    for low in [0x500, 0x50] {
        let (result, published) = send(&mut source, &mut sv, (1u64 << 32) | low, &mailbox, 0x2000);
        assert!(result.is_err());
        assert!(!published && mailbox.pending());
        assert_eq!(source.icr(), accepted_icr);
        assert_eq!(sv.guest_rip(), 0x2000);
    }
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x612, &mailbox, 0x2000),
        (Ok(()), true)
    );
    let (mut target, mut tv) = cpu(1);
    let mut frame = GuestRegisters {
        rbx: 0x1234,
        ..GuestRegisters::default()
    };
    let mut startup = StartupState::AwaitSipi; // Mismatched owner must not consume.
    let before = *tv.bytes();
    assert_eq!(
        drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup),
        Err(IpiError::StartupStateMismatch)
    );
    assert!(mailbox.pending());
    assert_eq!(tv.bytes(), &before);
    startup = StartupState::Cold;
    for (offset, poison) in [
        (0xa8, 1u64 << 31),
        (0x88, 1 << 31),
        (0x60, (1 << 24) | (1 << 8)),
    ] {
        put(&mut tv, offset, poison);
        let bytes = *tv.bytes();
        let apic = format!("{target:?}");
        assert!(drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup).is_err());
        assert!(mailbox.pending());
        assert_eq!(tv.bytes(), &bytes);
        assert_eq!(format!("{target:?}"), apic);
        assert_eq!(frame.rbx, 0x1234);
        assert_eq!(startup, StartupState::Cold);
        put(&mut tv, offset, if offset == 0x60 { 1 << 24 } else { 0 });
    }
    assert_eq!(
        drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup),
        Ok(2)
    );
    assert_eq!(
        u64::from_le_bytes(tv.bytes()[0x418..0x420].try_into().unwrap()),
        0x12000
    );
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x500, &mailbox, 0x3000),
        (Err(MsrError::Ipi(IpiError::UnsupportedInit)), false)
    );
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x50, &mailbox, 0x3000),
        (Ok(()), true)
    );
}

#[test]
fn sipi_after_init_drain_is_retained_and_fixed_waits_for_owner_application() {
    let mailbox = IpiMailbox::new_cold_ap();
    let (mut source, mut sv) = cpu(0);
    let (mut target, mut tv) = cpu(1);
    let mut frame = GuestRegisters::default();
    let mut startup = StartupState::Cold;
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x500, &mailbox, 0x1000),
        (Ok(()), true)
    );
    assert_eq!(
        drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup),
        Ok(1)
    );
    assert_eq!(startup, StartupState::AwaitSipi);
    assert!(!mailbox.pending());
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x6ab, &mailbox, 0x1000),
        (Ok(()), true)
    );
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x50, &mailbox, 0x1000),
        (Err(MsrError::Ipi(IpiError::TargetNotRunning)), false)
    );
    assert_eq!(
        mailbox.drain_into(&mut target, &tv),
        Err(IpiError::TargetNotRunning)
    );
    assert!(mailbox.pending());
    assert_eq!(
        drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup),
        Ok(1)
    );
    assert_eq!(startup, StartupState::Running);
    assert_eq!(
        u64::from_le_bytes(tv.bytes()[0x418..0x420].try_into().unwrap()),
        0xab000
    );
    assert!(!mailbox.pending());
}

#[test]
fn startup_continuation_refusal_cannot_accept_an_init() {
    let mailbox = IpiMailbox::new_cold_ap();
    let (mut source, mut sv) = cpu(0);
    let (result, published) = send(
        &mut source,
        &mut sv,
        (1u64 << 32) | 0x500,
        &mailbox,
        0x7fff_ffff_ffff,
    );
    assert!(matches!(result, Err(MsrError::Continuation(_))));
    assert!(!published && !mailbox.pending());
    assert_eq!(source.icr(), 0);
    assert_eq!(sv.guest_rip(), 0x7fff_ffff_ffff);
    assert_eq!(
        send(&mut source, &mut sv, (1u64 << 32) | 0x500, &mailbox, 0x1000),
        (Ok(()), true)
    );
}

#[test]
fn concurrent_init_application_and_sipi_publication_cannot_lose_the_first_vector() {
    for _ in 0..64 {
        let mailbox = IpiMailbox::new_cold_ap();
        let barrier = Barrier::new(2);
        let (mut source, mut sv) = cpu(0);
        assert_eq!(
            send(&mut source, &mut sv, (1u64 << 32) | 0x500, &mailbox, 0x1000),
            (Ok(()), true)
        );
        std::thread::scope(|scope| {
            let producer = scope.spawn(|| {
                barrier.wait();
                assert_eq!(
                    send(&mut source, &mut sv, (1u64 << 32) | 0x67a, &mailbox, 0x1000),
                    (Ok(()), true)
                );
                assert_eq!(
                    send(&mut source, &mut sv, (1u64 << 32) | 0x689, &mailbox, 0x1000),
                    (Ok(()), true)
                );
            });
            let (mut target, mut tv) = cpu(1);
            let mut frame = GuestRegisters::default();
            let mut startup = StartupState::Cold;
            barrier.wait();
            let first =
                drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup).unwrap();
            producer.join().unwrap();
            let second =
                drain_startup(&mailbox, &mut target, &mut tv, &mut frame, &mut startup).unwrap();
            assert_eq!(first + second, 2);
            assert_eq!(startup, StartupState::Running);
            assert_eq!(
                u64::from_le_bytes(tv.bytes()[0x418..0x420].try_into().unwrap()),
                0x7a000
            );
            assert!(!mailbox.pending());
        });
    }
}
