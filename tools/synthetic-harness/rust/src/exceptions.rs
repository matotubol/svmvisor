//! Actual intercepted faults, guest IDT delivery and IRETQ continuation in TCG.
//! Firmware context, external interrupts and OS boot are outside this fixture.
use crate::{memory, print, xstate};
use core::ptr;
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::ValidatedCapabilities, registers::GuestRegisters},
    guest::state::GuestStateRequest,
    svm::{events::ReflectionError, vmcb::Vmcb},
};
unsafe extern "C" {
    static guest_exception_entry: u8;
    static guest_ud_handler: u8;
    static guest_gp_handler: u8;
    static guest_pf_handler: u8;
}
/// # Safety
/// Same exclusive stopped VMCB, backing, host state and xstate ownership as
/// run_sessions. No references to hardware-mutated buffers survive guest entry.
/// APM vol.2 rev.3.44 sections 15.7.2, 15.12.15, 15.20 and 8.9.
pub unsafe fn run(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    code_start: usize,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &crate::clock::State,
) {
    let guest_address = |address: *const u8| 0x1000 + address as u64 - code_start as u64;
    for round in 0..17 {
        let nested = round == 16;
        unsafe {
            memory::reset_session();
            state.reset(round);
            memory::install_exception_idt(
                [
                    guest_address(ptr::addr_of!(guest_ud_handler)),
                    guest_address(ptr::addr_of!(guest_gp_handler)),
                    guest_address(ptr::addr_of!(guest_pf_handler)),
                ],
                nested,
            );
            let guest = GuestStateRequest {
                rip: guest_address(ptr::addr_of!(guest_exception_entry)),
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
            crate::execution::initialize(control, prepared, caps, &guest, Some((0x3000, 4095)));
        }
        let mut frame = GuestRegisters::default();
        for vector in [6u8, 13, 14] {
            unsafe {
                state.run(control, &mut frame, 0, clock);
            }
            let v = unsafe { &mut *control };
            assert_eq!(v.exit_snapshot().code, 0x40 + vector as u64);
            // The prior injection has completed: no secondary delivery event.
            assert_eq!(
                u64::from_le_bytes(v.bytes()[0x088..0x090].try_into().unwrap()) & (1 << 31),
                0
            );
            v.clear_event_injection_after_exit().unwrap();
            let rip = v.guest_rip();
            assert_eq!(v.reflect_exception().unwrap().vector(), vector);
            assert_eq!(v.guest_rip(), rip);
        }
        unsafe {
            state.run(control, &mut frame, 0, clock);
        }
        let v = unsafe { &mut *control };
        if nested {
            // Not-present #PF gate causes #NP during delivery of injected #PF.
            assert_eq!(v.exit_snapshot().code, 0x4b);
            let prior = u64::from_le_bytes(v.bytes()[0x088..0x090].try_into().unwrap());
            assert_eq!(prior & 0x8000_07ff, 0x8000_030e);
            let before = *v.bytes();
            assert!(matches!(
                v.reflect_exception(),
                Err(ReflectionError::NestedDeliveryUnsupported)
            ));
            assert_eq!(v.bytes(), &before);
            assert!(v.clear_event_injection_after_exit().is_err());
            assert_eq!(v.bytes(), &before);
        } else {
            assert_eq!(v.exit_snapshot().code, 0x81);
            assert_eq!(v.guest_rax(), 1);
            assert_eq!((frame.r12, frame.r13, frame.r14), (1, 1, 1));
            assert_eq!(v.guest_cr2(), 0x7000);
            assert_eq!(
                u64::from_le_bytes(v.bytes()[0x5d8..0x5e0].try_into().unwrap()),
                0x9000
            );
            v.clear_event_injection_after_exit().unwrap();
            assert_eq!(v.event_injection(), 0);
        }
        unsafe {
            memory::verify_exception_stack();
        }
    }
    print("PASS guest-exceptions=16 ud-gp-pf iretq-continuation\n");
    print("PASS guest-nested-delivery-refused\n");
}
