//! Repeated disposable guest sessions over one checked emulator memory image.
use crate::{hex, memory, print};
use core::ptr;
use svmvisor_hypervisor::{
    address::AddressPolicy,
    capabilities::ValidatedCapabilities,
    dispatch::{DispatchOutcome, StopReason, handle_exit_with_instruction},
    guest_state::GuestStateRequest,
    registers::GuestRegisters,
    vmcb::Vmcb,
};
unsafe extern "C" {
    static guest_npf: u8;
    static guest_nx: u8;
    static guest_guard: u8;
    static guest_xsetbv: u8;
    static guest_x87: u8;
}
/// Run only in the owned single-CPU emulator with installed host tables and
/// mappings. `control` must be a live, aligned, exclusive VMCB; all prepared
/// backing must remain mapped and owned. Code bytes are immutable fixture data.
/// All guest execution is stopped between calls and memory reset operations.
pub unsafe fn run_sessions(
    control: *mut Vmcb,
    policy: AddressPolicy,
    prepared: &memory::Prepared,
    code: &[u8],
    code_start: usize,
    caps: &ValidatedCapabilities,
    xstate: &crate::xstate::State,
    clock: &crate::clock::State,
) {
    for session in 0..32 {
        unsafe {
            memory::reset_session();
            xstate.reset(session);
        }
        let guest = GuestStateRequest {
            rip: 0x1000,
            rsp: 0x9000,
            rflags: 2,
            cr0: 0x80010033,
            cr3: prepared.guest_cr3,
            cr4: xstate.guest_cr4(),
            efer: 0x1500,
            rax: 0,
        }
        .validate_with_xstate(&policy, xstate.layout())
        .unwrap();
        unsafe {
            crate::execution::initialize(control, prepared, caps, &guest, None);
        }
        let seed = (session as u64 + 1) << 32;
        let mut frame = GuestRegisters {
            rbp: seed | 0x18,
            rsi: seed | 0x20,
            rdi: seed | 0x28,
            r8: seed | 0x30,
            r9: seed | 0x38,
            r10: seed | 0x40,
            r11: seed | 0x48,
            r12: seed | 0x50,
            r13: seed | 0x58,
            r14: seed | 0x60,
            r15: seed | 0x68,
            ..GuestRegisters::default()
        };
        let sentinels = frame;
        for (step, expected) in [0x72u64, 0x81, 0x81].into_iter().enumerate() {
            unsafe {
                xstate.run(control, &mut frame, 1, clock);
            }
            let v = unsafe { &mut *control };
            let snapshot = v.exit_snapshot();
            if session == 0 {
                print("rust-exit=");
                hex(snapshot.code);
            }
            assert_eq!(snapshot.code, expected);
            let opcode = v.guest_rax();
            assert_eq!(opcode, if step == 2 { 1 } else { 0 });
            assert_eq!(
                (
                    frame.rbp, frame.rsi, frame.rdi, frame.r8, frame.r9, frame.r10, frame.r11,
                    frame.r12, frame.r13, frame.r14, frame.r15
                ),
                (
                    sentinels.rbp,
                    sentinels.rsi,
                    sentinels.rdi,
                    sentinels.r8,
                    sentinels.r9,
                    sentinels.r10,
                    sentinels.r11,
                    sentinels.r12,
                    sentinels.r13,
                    sentinels.r14,
                    sentinels.r15
                )
            );
            // Translate stopped guest RIP into the immutable source fixture bytes.
            let size = if snapshot.code == 0x72 { 2 } else { 3 };
            let offset = snapshot.rip.checked_sub(0x1000).unwrap() as usize;
            let instruction = code.get(offset..offset.checked_add(size).unwrap()).unwrap();
            let outcome =
                handle_exit_with_instruction(snapshot, v, &mut frame, instruction).unwrap();
            if expected == 0x81 && opcode == 1 {
                assert_eq!(outcome, DispatchOutcome::Stop(StopReason::Requested));
                break;
            }
            assert_eq!(outcome, DispatchOutcome::ResumePrepared);
        }
        for (entry, expected, address, fetch) in [
            (ptr::addr_of!(guest_npf), 0x400, 0x6000, false),
            (ptr::addr_of!(guest_nx), 0x400, 0x8000, true),
            (ptr::addr_of!(guest_guard), 0x4e, 0x7000, false),
        ] {
            let rip = 0x1000 + entry as u64 - code_start as u64;
            unsafe {
                (&mut *control).set_synthetic_state(
                    &GuestStateRequest {
                        rip,
                        rsp: 0x9000,
                        rflags: 2,
                        cr0: 0x80010033,
                        cr3: prepared.guest_cr3,
                        cr4: xstate.guest_cr4(),
                        efer: 0x1500,
                        rax: 0,
                    }
                    .validate_with_xstate(&policy, xstate.layout())
                    .unwrap(),
                );
                xstate.run(control, &mut frame, 0, clock);
            }
            let v = unsafe { &mut *control };
            let snapshot = v.exit_snapshot();
            if session == 0 {
                print("fault-exit=");
                hex(snapshot.code);
            }
            assert_eq!(snapshot.code, expected);
            assert_eq!(snapshot.info2, address);
            if expected == 0x400 {
                assert_eq!(snapshot.info1 & 16 != 0, fetch);
            }
            assert!(matches!(
                handle_exit_with_instruction(snapshot, v, &mut frame, &[]).unwrap(),
                DispatchOutcome::Stop(_)
            ));
        }

        // Fixed XCR0 cannot be mutated by the guest. In FXSAVE mode the
        // instruction is unavailable and faults #UD; XSAVE mode intercepts it.
        for (entry, expected, mutation) in [
            (
                ptr::addr_of!(guest_xsetbv),
                if xstate.uses_xsave() { 0x8d } else { 0x46 },
                0,
            ),
            (ptr::addr_of!(guest_x87), 0x81, 2),
        ] {
            unsafe {
                (&mut *control).set_synthetic_state(
                    &GuestStateRequest {
                        rip: 0x1000 + entry as u64 - code_start as u64,
                        rsp: 0x9000,
                        rflags: 2,
                        cr0: 0x80010033,
                        cr3: prepared.guest_cr3,
                        cr4: xstate.guest_cr4(),
                        efer: 0x1500,
                        rax: 0,
                    }
                    .validate_with_xstate(&policy, xstate.layout())
                    .unwrap(),
                );
                xstate.run(control, &mut frame, mutation, clock);
                assert_eq!((&*control).exit_snapshot().code, expected);
            }
        }
        unsafe {
            memory::verify_session();
        }
    }
    print("PASS xstate-isolation=32 xsetbv-blocked x87-arithmetic\n");
    print("PASS checked-faults\n");
    print("PASS repeated-sessions=32\n");
}
