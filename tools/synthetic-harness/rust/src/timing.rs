//! Bounded timestamp semantics in TCG; no hardware latency or drift claims.
use crate::{hex, memory, print, xstate};
use core::ptr;
use svmvisor_hypervisor::{
    capabilities::ValidatedCapabilities,
    dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction_and_clock},
    guest_state::GuestStateRequest,
    registers::GuestRegisters,
    vmcb::{InstructionIntercept, Vmcb},
};
unsafe extern "C" {
    static guest_rdtsc: u8;
    static guest_rdtsc_read: u8;
    static guest_rdtscp: u8;
    static guest_rdtscp_read: u8;
    static guest_rdtscp_after_read: u8;
    static guest_clock_read: u8;
    static guest_clock_read_pc: u8;
    static guest_clock_write: u8;
    static guest_clock_write_pc: u8;
}

/// # Safety
/// Exclusive stopped single-CPU emulator guest, immutable copied fixture code,
/// and live owned mappings/VMCB/xstate. AMD APM vol.2 rev.3.44 Appendix B and
/// 15.30.5. Clock controls are switched by the shared assembly boundary.
pub unsafe fn run(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &crate::clock::State,
) {
    let rdtscp = clock.capabilities().rdtscp();
    for (name, entry, read, intercept, exit, available) in [
        (
            "rdtsc",
            ptr::addr_of!(guest_rdtsc),
            ptr::addr_of!(guest_rdtsc_read),
            InstructionIntercept::Rdtsc,
            0x6e,
            true,
        ),
        (
            "rdtscp",
            ptr::addr_of!(guest_rdtscp),
            ptr::addr_of!(guest_rdtscp_read),
            InstructionIntercept::Rdtscp,
            0x87,
            rdtscp,
        ),
    ] {
        if !available {
            print("SKIP timing-rdtscp unsupported\n");
            continue;
        }
        let entry = core::hint::black_box(entry as u64);
        let read = core::hint::black_box(read as u64);
        let mut minimum = u64::MAX;
        let mut maximum = 0;
        let mut intercept_gap = false;
        for round in 0..17 {
            let trapped = round == 16;
            let guest = GuestStateRequest {
                rip: 0x1000 + entry as u64 - code_start as u64,
                rsp: 0x9000,
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
                memory::reset_session();
                state.reset(round);
                crate::execution::initialize(control, prepared, caps, &guest, None);
            }
            let v = unsafe { &mut *control };
            // Zero offset and capability-gated clock ownership are separate checks.
            assert_eq!(v.tsc_offset(), 0);
            assert!(!v.instruction_intercept(InstructionIntercept::Rdtsc));
            assert!(!v.instruction_intercept(InstructionIntercept::Rdtscp));
            v.set_instruction_intercept(intercept, trapped);
            let mut frame = GuestRegisters::default();
            for step in 0..if trapped { 2 } else { 4 } {
                unsafe { state.run(control, &mut frame, 0, clock) };
                let v = unsafe { &mut *control };
                let snapshot = v.exit_snapshot();
                if trapped && step == 1 {
                    print("timing-intercept-exit=");
                    print(name);
                    print("\n");
                    hex(snapshot.code);
                    if intercept == InstructionIntercept::Rdtscp && snapshot.code == 0x72 {
                        // Known QEMU 10.1 TCG gap: stop this session without
                        // dispatch/resume. Do not report intercept coverage.
                        let after =
                            core::hint::black_box(ptr::addr_of!(guest_rdtscp_after_read) as u64);
                        assert_eq!(snapshot.rip, 0x1000 + after - code_start as u64);
                        let offset = (snapshot.rip - 0x1000) as usize;
                        assert_eq!(code.get(offset..offset + 2), Some(&[0x0f, 0xa2][..]));
                        assert!(v.instruction_intercept(InstructionIntercept::Rdtscp));
                        assert!(!v.instruction_intercept(InstructionIntercept::Rdtsc));
                        assert_eq!(frame.r9, 0);
                        intercept_gap = true;
                        print("GAP timing-rdtscp-intercept not-observed tcg-only\n");
                        break;
                    }
                    assert_eq!(snapshot.code, exit);
                    assert_eq!(snapshot.rip, 0x1000 + read as u64 - code_start as u64);
                    let before = *v.bytes();
                    let before_frame = frame;
                    assert!(matches!(
                        handle_exit_with_instruction_and_clock(
                            snapshot,
                            v,
                            &mut frame,
                            &[],
                            clock.plan()
                        )
                        .unwrap(),
                        DispatchOutcome::Stop(_)
                    ));
                    assert_eq!(v.exit_snapshot().rip, snapshot.rip);
                    assert_eq!(v.bytes(), &before);
                    assert_eq!(frame, before_frame);
                    break;
                }
                assert_eq!(snapshot.code, if step == 3 { 0x81 } else { 0x72 });
                let offset = snapshot.rip.checked_sub(0x1000).unwrap() as usize;
                let length = if step == 3 { 3 } else { 2 };
                let instruction = code.get(offset..offset + length).unwrap();
                let outcome = handle_exit_with_instruction_and_clock(
                    snapshot,
                    v,
                    &mut frame,
                    instruction,
                    clock.plan(),
                )
                .unwrap();
                assert_eq!(
                    outcome,
                    if step == 3 {
                        DispatchOutcome::Stop(StopReason::Requested)
                    } else {
                        DispatchOutcome::ResumePrepared
                    }
                );
            }
            if !trapped {
                let delta = frame.r9.checked_sub(frame.r8).expect("guest TSC regressed");
                minimum = minimum.min(delta);
                maximum = maximum.max(delta);
                if intercept == InstructionIntercept::Rdtscp {
                    assert_eq!(Some(frame.r10), clock.guest_aux());
                    assert_eq!(Some(frame.r11), clock.guest_aux());
                }
                // Raw observations are emitted only after the guest has stopped.
                print("timing-sample=");
                print(name);
                print("\n");
                hex(frame.r8);
                hex(frame.r9);
                if intercept == InstructionIntercept::Rdtscp {
                    hex(frame.r10);
                    hex(frame.r11);
                }
            }
        }
        print("timing-tcg-delta-min-max=");
        print(name);
        print("\n");
        hex(minimum);
        hex(maximum);
        print("PASS timing-");
        print(name);
        print("=16 monotonic\n");
        if !intercept_gap {
            print("PASS timing-");
            print(name);
            print("-intercept-refusal\n");
        }
    }
    // Every clock MSR read/write is a terminal unsupported operation. These
    // probes exercise real entry/exit and clock restoration, not emulation.
    for msr in [0x10u32, 0xc0000103, 0xc0000104] {
        for (entry, stopped, write) in [
            (
                ptr::addr_of!(guest_clock_read),
                ptr::addr_of!(guest_clock_read_pc),
                false,
            ),
            (
                ptr::addr_of!(guest_clock_write),
                ptr::addr_of!(guest_clock_write_pc),
                true,
            ),
        ] {
            let entry = core::hint::black_box(entry as u64);
            let stopped = core::hint::black_box(stopped as u64);
            let guest = GuestStateRequest {
                rip: 0x1000 + entry - code_start as u64,
                rsp: 0x9000,
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
                memory::reset_session();
                state.reset(0);
                crate::execution::initialize(control, prepared, caps, &guest, None);
            }
            let mut frame = GuestRegisters {
                rdi: u64::from(msr),
                ..GuestRegisters::default()
            };
            unsafe {
                state.run(control, &mut frame, 0, clock);
            }
            let v = unsafe { &mut *control };
            let snapshot = v.exit_snapshot();
            assert_eq!(snapshot.code, 0x7c);
            assert_eq!(snapshot.info1, u64::from(write));
            assert_eq!(snapshot.rip, 0x1000 + stopped - code_start as u64);
            assert_eq!(frame.rcx, u64::from(msr));
            let before = *v.bytes();
            let before_frame = frame;
            assert!(matches!(
                handle_exit_with_instruction_and_clock(snapshot, v, &mut frame, &[], clock.plan())
                    .unwrap(),
                DispatchOutcome::Stop(_)
            ));
            assert_eq!(v.bytes(), &before);
            assert_eq!(frame, before_frame);
        }
    }
    print("PASS clock-msr-refusal=6 stopped-state-preserved\n");
    print("PASS clock-boundary host-restored\n");
    print("PASS timing-contract tcg-only\n");
}
