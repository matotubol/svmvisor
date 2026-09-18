//! Independent contract tests: target manual VM_CR policy, including the
//! legacy interval after INIT. These are inert VMCB tests, not hardware proof.

use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        cpu_model::native_boot_cpuid,
        dispatch::{
            NativeEfer, NativeEferError, NativeMsrOutcome, handle_native_efer, handle_native_vmcr,
        },
        vmcb::Vmcb,
    },
};

fn legacy(mode32: bool) -> (Vmcb, GuestRegisters) {
    let mut v = Vmcb::new();
    for (offset, value) in [
        (0x60, 1 << 24),
        (0x70, 0x7c),
        (0x410, ((if mode32 { 0x49b } else { 0x9b }) << 16) | (0xffff << 32)),
        (0x4d0, 0x1000),
        (0x558, if mode32 { 0x11 } else { 0x10 }),
        (0x570, 2),
        (0x578, 0x100),
    ] {
        put(&mut v, offset, value);
    }
    (
        v,
        GuestRegisters {
            rcx: 0xffff_ffff_c001_0114,
            rdx: u64::MAX,
            rbx: 0xabc,
            ..Default::default()
        },
    )
}

fn put(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

#[test]
fn unavailable_svm_contract_survives_the_init_execution_interval() {
    let cpuid = native_boot_cpuid(0x8000_0001, [u32::MAX; 4], 0);
    assert_eq!(cpuid[2] & ((1 << 2) | (1 << 12)), 0);
    assert_eq!(native_boot_cpuid(0x8000_000a, [u32::MAX; 4], 0), [0; 4]);
    for mode32 in [false, true] {
        let (mut v, mut f) = legacy(mode32);
        assert_eq!(
            handle_native_vmcr(&mut v, &mut f, &[0x0f, 0x32], true),
            Ok(NativeMsrOutcome::Completed)
        );
        assert_eq!((v.guest_rax(), f.rdx, f.rcx, f.rbx), (16, 0, 0xffff_ffff_c001_0114, 0xabc));
        let mut e =
            NativeEfer::admit_native(0x500, 0, (1 << 11) | (1 << 20) | (1 << 29), 0, None).unwrap();
        e.enable_guest_startup();
        e.reset_after_init().unwrap();
        f.rcx = 0xc0000080;
        f.rdx = 0;
        put(&mut v, 0x78, 1);
        put(&mut v, 0x5f8, 1 << 12);
        let before = (v.guest_rip(), v.guest_rax(), f, e);
        assert_eq!(
            handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x30]),
            Ok(NativeMsrOutcome::GeneralProtectionPrepared)
        );
        assert_eq!((v.guest_rip(), v.guest_rax(), f, e), before);
        assert_eq!(v.event_injection(), 0x8000_0b0d);
    }
}

#[test]
fn every_low_byte_combination_preserves_fault_and_refusal_priority() {
    for value in 0u64..=255 {
        let (mut v, mut f) = legacy(false);
        put(&mut v, 0x78, 1);
        put(&mut v, 0x5f8, value);
        f.rdx = 0;
        let before = (*v.bytes(), f);
        let result = handle_native_vmcr(&mut v, &mut f, &[0x0f, 0x30], true);
        match value {
            0 | 8 | 16 | 24 => {
                assert_eq!(result, Ok(NativeMsrOutcome::Completed));
                assert_eq!(v.guest_rip(), 0x102);
            }
            0..=31 => {
                assert_eq!(result, Err(NativeEferError::UnsupportedValue { value }));
                assert_eq!((*v.bytes(), f), before);
            }
            _ => {
                assert_eq!(result, Ok(NativeMsrOutcome::GeneralProtectionPrepared));
                assert_eq!(v.event_injection(), 0x80000b0d);
                assert_eq!(v.guest_rip(), 0x100);
            }
        }
        assert_eq!(f, before.1);
        assert_eq!(v.guest_rax(), value);
    }
}

#[test]
fn legacy_ownership_segment_and_virtual_interrupt_limits_remain_transactional() {
    for case in 0..5 {
        let (mut v, mut f) = legacy(false);
        match case {
            0 => {}
            1 => put(&mut v, 0x578, 0xfffe),
            2 => put(&mut v, 0x558, 0x80000011),
            3 => put(&mut v, 0x60, (1 << 24) | (1 << 8)),
            _ => put(&mut v, 0xa8, 1 << 31),
        }
        let before = (*v.bytes(), f);
        assert!(
            handle_native_vmcr(&mut v, &mut f, &[0x0f, 0x32], case != 0).is_err(),
            "case {case}"
        );
        assert_eq!((*v.bytes(), f), before, "case {case}");
    }
}
