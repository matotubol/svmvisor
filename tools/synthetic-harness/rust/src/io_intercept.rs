//! Disposable IOIO boundary proof, not a device emulator or native I/O policy.
//! AMD APM2 rev3.44 15.7/15.10: instruction intercepts precede commitment,
//! each operand-byte port bit is checked, and EXITINFO2 is diagnostic next RIP.
use crate::{clock, execution, hex, memory, print, xstate};
use core::{arch::asm, ptr};
use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{EvidenceFlag, ValidatedCapabilities},
        registers::GuestRegisters,
    },
    guest::state::GuestStateRequest,
    memory::npt::NptEvidence,
    svm::{
        dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
        events::{ExternalInterruptState, PendingExternalInterrupt},
        permission_maps::{Iopm, Permission},
        vmcb::{InstructionIntercept, Vmcb},
    },
};
static mut CONTROL: Vmcb = Vmcb::new();
static mut CONTROL_MAP: Iopm = Iopm::new();
const RAX: u64 = 0x123456789abcdea5;
const FLAGS: u64 = 0x8d7; // OF,SF,ZF,AF,PF,CF and fixed bit; IF/DF clear.
const MARKER: u64 = 0xfeedface01234567;
const SCRATCH: u16 = 0x3ff;

// QEMU's disposable 16550 scratch register, offset7 from COM1. Pinned source
// hw/char/serial.c serial_ioport_write/read case7; no UART transmitter access.
// Only the dedicated runner supplies this endpoint, with a null chardev.
unsafe fn scratch_write(value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") SCRATCH, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
unsafe fn scratch_read() -> u8 {
    let value;
    unsafe {
        asm!("in al, dx", in("dx") SCRATCH, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}
fn word(v: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(v.bytes()[offset..offset + 8].try_into().unwrap())
}
fn metric(name: &str, value: u64) {
    print("IO ");
    print(name);
    print("=");
    hex(value);
}

/// # Safety
/// Sole stopped BSP/HSAVE/bridge and memory::prepare ownership; no live prior
/// guest mappings. Flat disposable QEMU only, owned UART scratch endpoint.
/// APM2 15.5/15.7/15.10. No firmware or external device access is admitted.
pub unsafe fn run(caps: &ValidatedCapabilities, state: &xstate::State, clock: &clock::State) {
    // Every fresh QEMU process must first prove the endpoint reacts to writes.
    unsafe {
        scratch_write(0x36);
        let first = scratch_read();
        print("IO-PROBE first=");
        hex(u64::from(first));
        if first != 0x36 {
            print("REFUSE io-endpoint first-probe\n");
            crate::finish(false);
        }
        scratch_write(0x69);
        let second = scratch_read();
        print("IO-PROBE second=");
        hex(u64::from(second));
        if second != 0x69 {
            print("REFUSE io-endpoint second-probe\n");
            crate::finish(false);
        }
    }
    let mut count = 0;
    for round in 0..8 {
        // The same guest instructions run with IOIO enabled and only this one
        // scratch bit cleared. Both OUT and IN reach the following guest marker.
        unsafe {
            case(
                count,
                &[0xee],
                SCRATCH,
                1,
                false,
                false,
                false,
                1,
                true,
                round,
                caps,
                state,
                clock,
            );
        }
        count += 1;
        unsafe {
            case(
                count,
                &[0xec],
                SCRATCH,
                1,
                true,
                false,
                false,
                1,
                true,
                round,
                caps,
                state,
                clock,
            );
        }
        count += 1;
        // Unmodified shared deny-all IOPM: OUT cannot change the proven endpoint,
        // IN cannot change AL, and neither can execute either guest marker.
        unsafe {
            case(
                count,
                &[0xee],
                SCRATCH,
                1,
                false,
                false,
                false,
                0,
                false,
                round,
                caps,
                state,
                clock,
            );
        }
        count += 1;
        unsafe {
            case(
                count,
                &[0xec],
                SCRATCH,
                1,
                true,
                false,
                false,
                0,
                false,
                round,
                caps,
                state,
                clock,
            );
        }
        count += 1;
        // Unknown ports, DX and immediate forms, all scalar widths/directions.
        for (width, output, input) in [
            (1, &[0xee][..], &[0xec][..]),
            (2, &[0x66, 0xef][..], &[0x66, 0xed][..]),
            (4, &[0xef][..], &[0xed][..]),
        ] {
            for port in [0, 7, 0x1234, 0xfffc, 0xfffd, 0xfffe, 0xffff] {
                for (ins, is_in) in [(output, false), (input, true)] {
                    unsafe {
                        case(
                            count, ins, port, width, is_in, false, false, 0, false, round, caps,
                            state, clock,
                        );
                    }
                    count += 1;
                }
            }
            for is_in in [false, true] {
                let mut ins = [0; 3];
                let start = if width == 2 {
                    ins[0] = 0x66;
                    1
                } else {
                    0
                };
                ins[start] = match (is_in, width) {
                    (false, 1) => 0xe6,
                    (true, 1) => 0xe4,
                    (false, _) => 0xe7,
                    (true, _) => 0xe5,
                };
                ins[start + 1] = 0x80;
                unsafe {
                    case(
                        count,
                        &ins[..start + 2],
                        0x80,
                        width,
                        is_in,
                        false,
                        false,
                        0,
                        false,
                        round,
                        caps,
                        state,
                        clock,
                    );
                }
                count += 1;
            }
        }
        // Clear all leading bits of the operand span; only a later bit causes
        // interception. Includes bitmap byte/page boundaries and the three
        // architectural overrun bits beyond FFFFh (never wrapped to port0).
        for (port, width, allow) in [
            (7, 2, 1),
            (7, 4, 3),
            (0x7fff, 2, 1),
            (0x7fff, 4, 3),
            (0xffff, 2, 1),
            (0xffff, 4, 1),
            (0xfffe, 4, 2),
            (0xfffd, 4, 3),
        ] {
            for is_in in [false, true] {
                let ins: &[u8] = match (is_in, width) {
                    (false, 2) => &[0x66, 0xef],
                    (true, 2) => &[0x66, 0xed],
                    (false, _) => &[0xef],
                    (true, _) => &[0xed],
                };
                unsafe {
                    case(
                        count, ins, port, width, is_in, false, false, allow, false, round, caps,
                        state, clock,
                    );
                }
                count += 1;
            }
        }
        for (ins, is_in, rep, width) in [
            (&[0x6e][..], false, false, 1),
            (&[0x6c][..], true, false, 1),
            (&[0xf3, 0x6e][..], false, true, 1),
            (&[0xf3, 0x6c][..], true, true, 1),
            (&[0xf3, 0x66, 0x6f][..], false, true, 2),
            (&[0xf3, 0x6d][..], true, true, 4),
        ] {
            unsafe {
                case(
                    count, ins, SCRATCH, width, is_in, true, rep, 0, false, round, caps, state,
                    clock,
                );
            }
            count += 1;
        }
    }
    metric("cases", count);
    print("PASS io-boundary stopped-in-out-live-endpoint-pending-preserved\n");
}

// Each case has one entry and terminal observation; no instruction is emulated
// or retried. Telemetry is emitted only after the measured entry/dispatch path.
#[allow(clippy::too_many_arguments)]
unsafe fn case(
    id: u64,
    instruction: &[u8],
    port: u16,
    width: u8,
    input: bool,
    string: bool,
    rep: bool,
    allowed_prefix: u32,
    allowed: bool,
    round: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &clock::State,
) {
    let mut code = [0x90; 40];
    code[..instruction.len()].copy_from_slice(instruction);
    let n = instruction.len();
    // mov r15,MARKER; mov dword ptr [8000h],12345678h; VMMCALL.
    code[n..n + 2].copy_from_slice(&[0x49, 0xbf]);
    code[n + 2..n + 10].copy_from_slice(&MARKER.to_le_bytes());
    code[n + 10..n + 21]
        .copy_from_slice(&[0xc7, 0x04, 0x25, 0, 0x80, 0, 0, 0x78, 0x56, 0x34, 0x12]);
    code[n + 21..n + 24].copy_from_slice(&[0x0f, 0x01, 0xd9]);
    let prepared = unsafe {
        memory::prepare(
            caps.address_policy(),
            &code,
            None,
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
    };
    let guest = GuestStateRequest {
        rip: 0x1000,
        rsp: 0x9000,
        rflags: FLAGS,
        cr0: 0x80010033,
        cr3: prepared.guest_cr3,
        cr4: state.guest_cr4(),
        efer: 0x1500,
        rax: RAX,
    }
    .validate_continuation_with_xstate(&caps.address_policy(), state.layout())
    .unwrap();
    let v = unsafe { &mut *ptr::addr_of_mut!(CONTROL) };
    unsafe {
        execution::initialize(v, &prepared, caps, &guest, None);
        memory::reset_session();
        state.reset(round);
    }
    assert!(v.instruction_intercept(InstructionIntercept::Ioio));
    if allowed_prefix != 0 {
        let map = unsafe { &mut *ptr::addr_of_mut!(CONTROL_MAP) };
        // Revoke the previous case before permitting this stopped case's prefix.
        map.set_range(0, 65536, Permission::Intercept).unwrap();
        map.set_range(port, allowed_prefix, Permission::Allow)
            .unwrap();
        v.set_permission_maps(
            map as *const Iopm as u64,
            word(v, 0x48),
            &caps.address_policy(),
        )
        .unwrap();
    }
    // Deliberately faulty disposable image: prove the OUT side-effect witness
    // catches removing the boundary, without using it in accepted artifacts.
    if cfg!(feature = "io-intercept-bypass") && id == 2 {
        v.set_instruction_intercept(InstructionIntercept::Ioio, false);
    }
    let mut pending = PendingExternalInterrupt::new(0x61).unwrap();
    v.arm_external_interrupt(&mut pending).unwrap(); // IF=0 keeps this armed.
    let pending_before = (pending.vector(), pending.state());
    let interrupt_before = v.virtual_interrupt_control();
    let mut frame = GuestRegisters {
        rcx: 3,
        rdx: u64::from(port),
        rbx: 0xb1,
        rbp: 0xb2,
        rsi: 0x8000,
        rdi: 0x8000,
        r8: 8,
        r9: 9,
        r10: 10,
        r11: 11,
        r12: 12,
        r13: 13,
        r14: 14,
        r15: 15,
    };
    let before_frame = frame;
    let endpoint_before = 0x69u8 ^ round as u8;
    unsafe {
        scratch_write(endpoint_before);
    }
    let start = unsafe { clock.sample().ticks };
    unsafe {
        state.run(v, &mut frame, 0, clock);
    }
    let end = unsafe { clock.sample().ticks };
    let endpoint_after = unsafe { scratch_read() };
    let snap = v.exit_snapshot();
    metric("id", id);
    for (name, value) in [
        ("code", snap.code),
        ("info1", snap.info1),
        ("info2", snap.info2),
        ("rip", snap.rip),
        ("rax", v.guest_rax()),
        ("flags", word(v, 0x570)),
        ("marker", frame.r15),
        ("rcx", frame.rcx),
        ("rdx", frame.rdx),
        ("rsi", frame.rsi),
        ("rdi", frame.rdi),
        ("rsp", word(v, 0x5d8)),
        ("endpoint-before", u64::from(endpoint_before)),
        ("endpoint-after", u64::from(endpoint_after)),
        ("pending-before", interrupt_before),
        ("pending-after", v.virtual_interrupt_control()),
        ("entry-ticks", end - start),
    ] {
        metric(name, value);
    }
    assert_eq!(word(v, 0x570), FLAGS);
    assert_eq!(word(v, 0x5d8), 0x9000);
    assert_eq!(v.virtual_interrupt_control(), interrupt_before);
    assert_eq!((pending.vector(), pending.state()), pending_before);
    assert_eq!(
        v.observe_external_interrupt_after_exit(&mut pending)
            .unwrap(),
        ExternalInterruptState::Armed
    );
    if allowed {
        assert_eq!(snap.code, 0x81);
        assert_eq!(snap.rip, 0x1000 + n as u64 + 21);
        let mut expected = before_frame;
        expected.r15 = MARKER;
        assert_eq!(frame, expected);
        assert_eq!(
            v.guest_rax(),
            if input {
                (RAX & !255) | u64::from(endpoint_before)
            } else {
                RAX
            }
        );
        assert_eq!(
            endpoint_after,
            if input { endpoint_before } else { RAX as u8 }
        );
        unsafe {
            memory::verify_preemption_stack(0x12345678);
        }
    } else {
        // Check side effect before exit metadata: bypass control must fail here.
        assert_eq!(
            endpoint_after, endpoint_before,
            "trapped OUT changed endpoint"
        );
        assert_eq!(snap.code, 0x7b);
        assert_eq!(snap.rip, 0x1000);
        assert_eq!(snap.info2, 0x1000 + n as u64);
        assert_eq!(v.guest_rax(), RAX);
        assert_eq!(frame, before_frame);
        unsafe {
            memory::verify_preemption_stack(0);
        }
        let metadata = snap.ioio().unwrap();
        assert_eq!(metadata.port(), port);
        assert_eq!(metadata.width_bytes(), width);
        assert_eq!(metadata.input(), input);
        assert_eq!(metadata.string(), string);
        assert_eq!(metadata.rep(), rep);
        let before = *v.bytes();
        let service_start = unsafe { clock.sample().ticks };
        let result = handle_exit_with_instruction(snap, v, &mut frame, instruction).unwrap();
        let service_end = unsafe { clock.sample().ticks };
        assert_eq!(
            result,
            DispatchOutcome::Stop(StopReason::Exit(snap.action()))
        );
        assert_eq!(v.bytes(), &before);
        assert_eq!(frame, before_frame);
        assert_eq!((pending.vector(), pending.state()), pending_before);
        metric("service-ticks", service_end - service_start);
    }
    print("IO case-pass\n");
}
