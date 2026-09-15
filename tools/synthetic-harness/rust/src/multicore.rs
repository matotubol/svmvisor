//! Two bounded guest vCPUs time-multiplexed on the existing sole host CPU.
//! No physical AP startup, concurrent execution, generic topology or OS support.
use crate::{clock, execution, field, hex, memory, print, xstate};
use core::{arch::asm, ptr};
use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{EvidenceFlag, ValidatedCapabilities},
        registers::GuestRegisters,
    },
    boot::ownership::OwnershipRecord,
    guest::state::GuestStateRequest,
    memory::npt::NptEvidence,
    svm::{
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        ipi::{IpiError, IpiTarget, StartupState},
        local_apic::LocalApic,
        permission_maps::{MsrAccess, Msrpm, Permission},
        vmcb::Vmcb,
        x2apic::{FixtureApic, MsrError, handle_fixture_msr_with_target},
        xapic::{MmioError, handle_fixture_mmio_with_target},
    },
};
static mut CONTROLS: [Vmcb; 2] = [Vmcb::new(), Vmcb::new()];
static mut EXTENDED: [xstate::GuestState; 2] =
    [xstate::GuestState::new(), xstate::GuestState::new()];
static mut STARTUP_MAP: Msrpm = Msrpm::new();
static mut RUNNING_MAP: Msrpm = Msrpm::new();
unsafe extern "C" {
    static guest_smp_start: u8;
    static guest_smp_end: u8;
    static guest_smp_bsp: u8;
    static guest_smp_handler: u8;
    static guest_smp_bad_destination: u8;
    static guest_smp_running_init: u8;
}
#[derive(Default)]
struct Metrics {
    entries: u64,
    real: u64,
    long: u64,
    checkpoints: u64,
    delivered: u64,
    eois: u64,
    refused: u64,
    min_span: u64,
    max_span: u64,
    last_guest: u64,
}
fn word(v: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(v.bytes()[offset..offset + 8].try_into().unwrap())
}
fn apic_snapshot(a: &FixtureApic) -> [u64; 27] {
    let c = a.controller();
    let mut values = [0; 27];
    values[..11].copy_from_slice(&[
        a.icr(),
        a.apic_base(),
        u64::from(a.identity()),
        u64::from(c.task_priority()),
        u64::from(c.processor_priority()),
        u64::from(c.spurious_vector_register()),
        u64::from(c.timer_lvt()),
        u64::from(c.timer_initial()),
        u64::from(c.timer_remaining()),
        u64::from(c.timer_divide()),
        u64::from(c.delivery_armed()),
    ]);
    for vector in 0..256 {
        values[11 + vector / 32] |= u64::from(c.pending(vector as u8)) << (vector % 32);
        values[19 + vector / 32] |= u64::from(c.in_service(vector as u8)) << (vector % 32);
    }
    values
}
fn digest(v: &Vmcb) -> u64 {
    v.bytes().iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}
/// # Safety
/// Sole owned stopped host CPU; previous guest mapping tokens retired. Shared
/// immutable code/tables, disjoint stacks and per-vCPU VMCB/frame/XSTATE. The
/// existing bridge switches clock/XSTATE/auxiliary state before host Rust runs.
/// APM2rev3.44 14.6,15.5.2,15.27.8,16.6.1; no firmware or allocation in loop.
pub unsafe fn run(
    ownership: Option<&OwnershipRecord<'_>>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    // Cold AP debug-address admission: this bridge does not switch DR0–3.
    // No debug address leaks into guest execution; every DR access intercepts.
    let debug0: u64;
    let debug1: u64;
    let debug2: u64;
    let debug3: u64;
    unsafe {
        asm!("mov {},dr0",out(reg) debug0,options(nomem,nostack,preserves_flags));
        asm!("mov {},dr1",out(reg) debug1,options(nomem,nostack,preserves_flags));
        asm!("mov {},dr2",out(reg) debug2,options(nomem,nostack,preserves_flags));
        asm!("mov {},dr3",out(reg) debug3,options(nomem,nostack,preserves_flags));
    }
    assert_eq!([debug0, debug1, debug2, debug3], [0; 4]);
    let source = ptr::addr_of!(guest_smp_start) as usize;
    let code = unsafe {
        core::slice::from_raw_parts(
            source as *const u8,
            ptr::addr_of!(guest_smp_end) as usize - source,
        )
    };
    let address = |p: *const u8| 0x1000 + p as u64 - source as u64;
    let prepared = unsafe {
        memory::prepare(
            caps.address_policy(),
            code,
            ownership,
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
    };
    unsafe {
        for map in [
            ptr::addr_of_mut!(STARTUP_MAP),
            ptr::addr_of_mut!(RUNNING_MAP),
        ] {
            map.write(Msrpm::new());
            for access in [MsrAccess::Read, MsrAccess::Write] {
                (&mut *map)
                    .set(0xc0000100, access, Permission::Allow)
                    .unwrap();
            }
        }
        for access in [MsrAccess::Read, MsrAccess::Write] {
            (&mut *ptr::addr_of_mut!(STARTUP_MAP))
                .set(0xc0000080, access, Permission::Allow)
                .unwrap();
        }
    }
    let clocks = [clock.for_guest(0), clock.for_guest(1)];
    // INIT identity must agree with this guest's existing fixed CPUID policy.
    let signature = svmvisor_hypervisor::svm::emulation::cpuid(1, 0)[0];
    let mut metrics = Metrics {
        min_span: u64::MAX,
        ..Metrics::default()
    };
    for bus in 0..2 {
        for round in 0..4 {
            let controls = ptr::addr_of_mut!(CONTROLS).cast::<Vmcb>();
            let extended = ptr::addr_of_mut!(EXTENDED).cast::<xstate::GuestState>();
            unsafe {
                memory::reset_session();
                memory::prepare_smp_descriptors();
                memory::install_idt(&[(0x50, address(ptr::addr_of!(guest_smp_handler)), true)]);
            }
            let mut frames = [GuestRegisters::default(), GuestRegisters::default()];
            let mut apics = [
                FixtureApic::admit_fixed_cpu(
                    LocalApic::admit_enabled(),
                    if bus == 0 { 0xfee00900 } else { 0xfee00d00 },
                    0,
                )
                .unwrap(),
                FixtureApic::admit_fixed_cpu(
                    LocalApic::admit_enabled(),
                    if bus == 0 { 0xfee00800 } else { 0xfee00c00 },
                    1,
                )
                .unwrap(),
            ];
            let mut startup = [StartupState::Running, StartupState::Cold];
            for id in 0..2 {
                let guest = GuestStateRequest {
                    rip: address(ptr::addr_of!(guest_smp_bsp)),
                    rsp: if id == 0 { 0x9000 } else { 0xb000 },
                    rflags: 2,
                    cr0: 0x80010033,
                    cr3: prepared.guest_cr3,
                    cr4: state.guest_cr4(),
                    efer: 0x1500,
                    rax: 0,
                }
                .validate_with_xstate(&caps.address_policy(), state.layout())
                .unwrap();
                unsafe {
                    execution::initialize(
                        controls.add(id),
                        &prepared,
                        caps,
                        &guest,
                        Some((0x3000, 4095)),
                    );
                    crate::x2apic::admit(controls.add(id));
                    field(controls.add(id), 0x004, u32::MAX.to_le_bytes());
                    // Every entry flushes ASID1, so alternating same-ASID VMCBs
                    // cannot reuse stale paging state. NPT is shared and owned.
                    state.reset_owned(&mut *extended.add(id), round * 2 + id);
                    execution::permissions(
                        controls.add(id),
                        caps,
                        Some(if id == 1 {
                            ptr::addr_of!(STARTUP_MAP)
                        } else {
                            ptr::addr_of!(RUNNING_MAP)
                        }),
                    );
                }
            }
            frames[0].r15 = bus;
            let order = [0usize, 1, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0];
            let mut checkpoints = [0u64; 2];
            let start_delivered = metrics.delivered;
            let start_eois = metrics.eois;
            for id in order {
                let stopped = unsafe {
                    drive(
                        id,
                        controls,
                        extended,
                        &mut frames,
                        &mut apics,
                        &mut startup,
                        signature,
                        &prepared,
                        code,
                        caps,
                        state,
                        &clocks,
                        &mut metrics,
                        false,
                    )
                };
                if checkpoints[id] < 5 {
                    assert!(!stopped);
                    assert_eq!(frames[id].r11, checkpoints[id]);
                    assert_eq!(
                        frames[id].r12,
                        u64::from(checkpoints[id] >= if id == 0 { 3 } else { 2 })
                    );
                    assert_eq!(frames[id].r10, 0x73564d4350553030 + id as u64);
                    assert_eq!(
                        unsafe { (&*controls.add(id)).guest_rsp() },
                        if id == 0 { 0x9000 } else { 0xb000 }
                    );
                    if id == 1 && checkpoints[id] == 0 {
                        let v = unsafe { &*controls.add(id) };
                        assert_eq!(
                            u16::from_le_bytes(v.bytes()[0x412..0x414].try_into().unwrap()) & 0x600,
                            0x200
                        );
                        assert_eq!(word(v, 0x558), 0x80010033);
                        assert_eq!(word(v, 0x548), 0x620);
                        assert_eq!(word(v, 0x550), prepared.guest_cr3);
                        assert_eq!(word(v, 0x4d0) & 0x1d00, 0x1d00);
                        metrics.long += 1;
                        unsafe {
                            execution::permissions(
                                controls.add(id),
                                caps,
                                Some(ptr::addr_of!(RUNNING_MAP)),
                            )
                        };
                    }
                    if (id == 1 && checkpoints[id] == 1) || (id == 0 && checkpoints[id] == 2) {
                        assert!(apics[id].controller().pending(0x50));
                        assert!(!apics[id].controller().in_service(0x50));
                        assert_eq!(unsafe { word(&*controls.add(id), 0x570) } & 0x200, 0);
                    }
                    checkpoints[id] += 1;
                } else {
                    assert!(stopped);
                }
            }
            assert_eq!(checkpoints, [5, 5]);
            assert_eq!(metrics.delivered - start_delivered, 2);
            assert_eq!(metrics.eois - start_eois, 2);
            for id in 0..2 {
                assert!(!apics[id].controller().pending(0x50));
                assert!(!apics[id].controller().in_service(0x50));
                assert_eq!(frames[id].r12, 1);
            }
            // Real guest invalid-destination and running-INIT writes stop on
            // the actual source exit, preserving both vCPUs/APIC owners.
            for entry in [
                address(ptr::addr_of!(guest_smp_bad_destination)),
                address(ptr::addr_of!(guest_smp_running_init)),
            ] {
                unsafe { field(controls, 0x578, entry.to_le_bytes()) };
                assert!(unsafe {
                    drive(
                        0,
                        controls,
                        extended,
                        &mut frames,
                        &mut apics,
                        &mut startup,
                        signature,
                        &prepared,
                        code,
                        caps,
                        state,
                        &clocks,
                        &mut metrics,
                        true,
                    )
                });
            }
            unsafe { memory::verify_smp_stacks() };
        }
    }
    print("PASS multicore-startup=8 real16-protected32-long64-two-guests-one-host\n");
    print("PASS multicore-ipi=8 xapic-x2apic-bidirectional-coalesced-eoi-iretq\n");
    print("PASS multicore-ownership=8 vmcb-gpr-xstate-auxiliary-fsbase-clock-stack\n");
    print("PASS multicore-refusals=16 actual-icr-destination-running-init-unchanged\n");
    for (name, value) in [
        ("entries", metrics.entries),
        ("real-starts", metrics.real),
        ("long-starts", metrics.long),
        ("checkpoints", metrics.checkpoints),
        ("delivered", metrics.delivered),
        ("eois", metrics.eois),
        ("refused", metrics.refused),
        ("min-entry-exit-tsc", metrics.min_span),
        ("max-entry-exit-tsc", metrics.max_span),
    ] {
        print("MULTICORE ");
        print(name);
        print("=");
        hex(value);
    }
}
/// Return at one guest query/stop or an expected terminal ICR refusal.
unsafe fn drive(
    id: usize,
    controls: *mut Vmcb,
    extended: *mut xstate::GuestState,
    frames: &mut [GuestRegisters; 2],
    apics: &mut [FixtureApic; 2],
    startup: &mut [StartupState; 2],
    signature: u32,
    prepared: &memory::Prepared,
    code: &[u8],
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clocks: &[clock::State; 2],
    metrics: &mut Metrics,
    refusal: bool,
) -> bool {
    for _ in 0..32 {
        assert_eq!(startup[id], StartupState::Running);
        let control = unsafe { controls.add(id) };
        if !apics[id].controller().delivery_armed() {
            apics[id].arm(unsafe { &mut *control }).unwrap();
        }
        let before = unsafe { clocks[id].sample() };
        let old_cr8: u64;
        let new_cr8: u64;
        unsafe {
            asm!("mov {},cr8",out(reg) old_cr8,options(nomem,nostack,preserves_flags));
            state.run_owned(
                control,
                &mut frames[id],
                0,
                &clocks[id],
                &mut *extended.add(id),
            );
            asm!("mov {},cr8",out(reg) new_cr8,options(nomem,nostack,preserves_flags));
        }
        assert_eq!(old_cr8, new_cr8);
        let sample = unsafe { clocks[id].sample() };
        let span = sample.ticks.checked_sub(before.ticks).unwrap();
        metrics.min_span = metrics.min_span.min(span);
        metrics.max_span = metrics.max_span.max(span);
        metrics.entries += 1;
        let v = unsafe { &mut *control };
        v.clear_event_injection_after_exit().unwrap();
        if apics[id].controller().delivery_armed() {
            if let Some(vector) = apics[id].observe(v).unwrap() {
                assert_eq!(vector, 0x50);
                metrics.delivered += 1;
            }
        }
        let snap = v.exit_snapshot();
        if snap.code == 0x72 {
            // Only first actual AP instruction is CPUID. RIP is CS-relative;
            // immutable fetch is CS.base+RIP, continuation keeps the offset.
            assert_eq!(id, 1);
            assert_eq!(snap.rip, 0);
            assert_eq!(&v.bytes()[0x410..0x412], &0x100u16.to_le_bytes());
            assert_eq!(word(v, 0x418), 0x1000);
            assert_eq!(word(v, 0x558), 0x10);
            assert_eq!(word(v, 0x548), 0);
            assert_eq!(word(v, 0x550), 0);
            assert_eq!(word(v, 0x4d0), 0x1000);
            assert_eq!(v.guest_rsp(), 0);
            assert_eq!(v.guest_rax(), 0);
            assert_eq!(
                frames[id],
                GuestRegisters {
                    rdx: u64::from(signature),
                    ..GuestRegisters::default()
                }
            );
            assert_eq!(word(v, 0x448), 0);
            assert_eq!(word(v, 0x458), 0);
            assert_eq!(&code[..2], &[0x0f, 0xa2]);
            assert_eq!(
                handle_exit_with_instruction(snap, v, &mut frames[id], &code[..2]).unwrap(),
                DispatchOutcome::ResumePrepared
            );
            assert_eq!(v.guest_rip(), 2);
            metrics.real += 1;
            continue;
        }
        let length = if snap.code == 0x81 { 3 } else { 2 };
        let instruction = unsafe { memory::installed_instruction(snap.rip, length) };
        if snap.code == 0x81 {
            assert!(!refusal);
            let value = v.guest_rax();
            assert!(value <= 1, "multicore guest failure");
            let result =
                handle_exit_with_instruction(snap, v, &mut frames[id], instruction).unwrap();
            if value == 1 {
                assert_eq!(result, DispatchOutcome::Stop(StopReason::Requested));
                return true;
            }
            assert_eq!(result, DispatchOutcome::ResumePrepared);
            assert!(
                frames[id].rbp >= metrics.last_guest,
                "guest clock cross-vCPU backwards"
            );
            metrics.last_guest = frames[id].rbp;
            metrics.checkpoints += 1;
            return false;
        }
        assert!(
            snap.code == 0x7c || snap.code == 0x400,
            "unexpected multicore exit"
        );
        let eoi = if snap.code == 0x7c {
            frames[id].rcx == 0x80b
        } else {
            frames[id].rbx == 0xc0b0
        };
        let low_icr = if snap.code == 0x7c {
            frames[id].rcx == 0x830
        } else {
            frames[id].rbx == 0xc300
        };
        let icr_value = v.guest_rax();
        let old_source = digest(v);
        let old_target = digest(unsafe { &*controls.add(1 - id) });
        let old_frames = *frames;
        let old_startup = *startup;
        let old_apics = [apic_snapshot(&apics[0]), apic_snapshot(&apics[1])];
        let (source_apic, target_apic) = if id == 0 {
            let (a, b) = apics.split_at_mut(1);
            (&mut a[0], &mut b[0])
        } else {
            let (a, b) = apics.split_at_mut(1);
            (&mut b[0], &mut a[0])
        };
        let (source_frame, target_frame) = if id == 0 {
            let (a, b) = frames.split_at_mut(1);
            (&mut a[0], &mut b[0])
        } else {
            let (a, b) = frames.split_at_mut(1);
            (&mut b[0], &mut a[0])
        };
        let mut target = IpiTarget {
            apic: target_apic,
            vmcb: unsafe { &mut *controls.add(1 - id) },
            frame: target_frame,
            startup: &mut startup[1 - id],
            signature,
        };
        let result = if snap.code == 0x7c {
            match handle_fixture_msr_with_target(
                source_apic,
                v,
                source_frame,
                instruction,
                &mut target,
            ) {
                Ok(()) => Ok(()),
                Err(MsrError::Ipi(error)) => Err(error),
                Err(error) => panic!("unexpected multicore MSR error: {:?}", error),
            }
        } else {
            match handle_fixture_mmio_with_target(
                source_apic,
                v,
                source_frame,
                instruction,
                prepared.mmio_mapping.as_ref().unwrap(),
                &mut target,
            ) {
                Ok(()) => Ok(()),
                Err(MmioError::Ipi(error)) => Err(error),
                Err(error) => panic!("unexpected multicore MMIO error: {:?}", error),
            }
        };
        if refusal && low_icr {
            assert_eq!(
                result,
                Err(if icr_value == 0x500 {
                    IpiError::UnsupportedInit
                } else {
                    IpiError::UnsupportedDestination
                })
            );
            assert_eq!(digest(v), old_source);
            assert_eq!(digest(target.vmcb), old_target);
            assert_eq!(*source_frame, old_frames[id]);
            assert_eq!(*target.frame, old_frames[1 - id]);
            assert_eq!(apic_snapshot(source_apic), old_apics[id]);
            assert_eq!(apic_snapshot(target.apic), old_apics[1 - id]);
            assert_eq!(*startup, old_startup);
            assert!(!apics[0].controller().pending(0x50));
            assert!(!apics[1].controller().pending(0x50));
            assert!(!apics[0].controller().in_service(0x50));
            assert!(!apics[1].controller().in_service(0x50));
            metrics.refused += 1;
            return true;
        }
        assert_eq!(result, Ok(()), "multicore APIC operation refused");
        if eoi {
            metrics.eois += 1;
            assert!(!apics[id].controller().in_service(0x50));
        }
        // Explicitly verify duplicate SIPI leaves the target CPU/frame intact.
        if low_icr && old_startup[1 - id] == StartupState::Running && (icr_value & 0x700) == 0x600 {
            assert_eq!(digest(unsafe { &*controls.add(1 - id) }), old_target);
            assert_eq!(frames[1 - id], old_frames[1 - id]);
        }
        let _ = caps;
    }
    panic!("multicore bounded exit budget")
}
