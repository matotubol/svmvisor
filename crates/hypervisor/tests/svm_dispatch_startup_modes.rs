use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        dispatch::{NativeEfer, NativeMsrOutcome, handle_native_efer, handle_native_startup_cpuid},
        vmcb::Vmcb,
    },
};

fn stopped(bits32: bool) -> (NativeEfer, Vmcb, GuestRegisters) {
    let mut e = NativeEfer::admit(0x500, true).unwrap();
    e.enable_guest_startup();
    e.reset_after_init().unwrap();
    let mut v = Vmcb::new();
    put(&mut v, 0x60, 1 << 24);
    put(&mut v, 0x70, 0x7c);
    put(&mut v, 0x78, 0);
    put(&mut v, 0x410, ((if bits32 { 0x49b } else { 0x9b }) << 16) | (0xffff << 32));
    put(&mut v, 0x4d0, 0x1000);
    put(&mut v, 0x558, if bits32 { 0x11 } else { 0x10 });
    put(&mut v, 0x570, 2);
    put(&mut v, 0x578, 0x100);
    (e, v, GuestRegisters { rcx: 0xc0000080, ..GuestRegisters::default() })
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
fn real16_and_protected32_efer_transition_tracks_hardware_lma() {
    for mode32 in [false, true] {
        let (mut e, mut v, mut f) = stopped(mode32);
        handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x32]).unwrap();
        assert_eq!(v.guest_rax(), 0);
        put(&mut v, 0x78, 1);
        put(&mut v, 0x5f8, 0x900);
        handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x30]).unwrap();
        assert_eq!(e.logical(), 0x900);
        // Real hardware MOV CR0.PG plus far jump derives LMA and CS.L.
        put(&mut v, 0x4d0, 0x1d00);
        put(&mut v, 0x558, 0x80000011);
        put(&mut v, 0x410, (0x29b << 16) | (0xffff << 32));
        put(&mut v, 0x78, 0);
        handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x32]).unwrap();
        assert_eq!(v.guest_rax(), 0xd00);
        assert_eq!(e.logical(), 0xd00);
        e.reset_after_init().unwrap();
        assert_eq!(e.logical(), 0);
    }
}

#[test]
fn legacy_segment_end_pending_irq_and_backing_mismatch_refuse_transactionally() {
    for case in 0..4 {
        let (mut e, mut v, mut f) = stopped(false);
        match case {
            0 => put(&mut v, 0x578, 0xfffe),
            1 => put(&mut v, 0x60, (1 << 24) | (1 << 8)),
            2 => put(&mut v, 0x4d0, 0x1900),
            _ => put(&mut v, 0x558, 0x80000011),
        }
        let before = (*v.bytes(), f, e);
        assert!(handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x32]).is_err());
        assert_eq!((*v.bytes(), f, e), before);
    }
}

#[test]
fn legacy_cpuid_with_owned_intr_masking_completes_and_refuses_wrap() {
    let (_, mut v, mut f) = stopped(false);
    put(&mut v, 0x70, 0x72);
    handle_native_startup_cpuid(&mut v, &mut f, &[0x0f, 0xa2], [7, 8, 9, 10]).unwrap();
    assert_eq!(v.guest_rip(), 0x102);
    put(&mut v, 0x578, 0xffff);
    let before = (*v.bytes(), f);
    assert!(handle_native_startup_cpuid(&mut v, &mut f, &[0x0f, 0xa2], [7, 8, 9, 10]).is_err());
    assert_eq!((*v.bytes(), f), before);
}

#[test]
fn faulting_or_refused_read_does_not_commit_lma_observation() {
    let (mut e, mut v, mut f) = stopped(true);
    put(&mut v, 0x78, 1);
    put(&mut v, 0x5f8, 0x100);
    handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x30]).unwrap();
    put(&mut v, 0x4d0, 0x1500);
    put(&mut v, 0x558, 0x80000011);
    put(&mut v, 0x410, (0x29b << 16) | (0xffff << 32));
    put(&mut v, 0x78, 0);
    put(&mut v, 0x4c8, 3 << 24);
    assert_eq!(
        handle_native_efer(&mut e, &mut v, &mut f, &[0x0f, 0x32]),
        Ok(NativeMsrOutcome::GeneralProtectionPrepared)
    );
    assert_eq!(e.logical(), 0x100);
}
