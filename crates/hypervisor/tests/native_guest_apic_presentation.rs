use svmvisor_hypervisor::{arch::x86_64::registers::GuestRegisters, svm::{
    ipi::{NativeIcr, NativeIcrError, native_guest_apic_msr, handle_native_guest_apic_msr},
    cpu_model::native_boot_cpuid, permission_maps::Msrpm, vmcb::Vmcb,
    dispatch::NativeMsrOutcome,
}};

fn put(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
        (v as *mut Vmcb).cast::<u8>().add(offset), 8); }
}
fn stopped(index: u32, write: bool) -> (Vmcb, GuestRegisters) {
    let mut v = Vmcb::new();
    for (o,n) in [(0x70,0x7c),(0x78,u64::from(write)),(0x410,0x029b0008),
        (0x4d0,0x1500),(0x558,0x80000001),(0x578,0x12345000),(0x570,0x10002),
        (0x68,1),(0x5f8,0xdeadbeef),(0xc8,u64::MAX)] { put(&mut v,o,n); }
    (v, GuestRegisters { rcx:index.into(),rdx:0xcafebabe,..GuestRegisters::default() })
}

#[test]
fn cpuid_and_both_version_buses_hide_only_the_extension_feature() {
    assert_eq!(native_boot_cpuid(0x80000001,[u32::MAX;4],0)[2],
        u32::MAX & !((1<<2)|(1<<3)|(1<<12)));
    let mut owner = NativeIcr::admit(0,&[0,16]).unwrap();
    owner.enable_startup(0).unwrap();
    let xapic = owner.xapic_access(0xfee00900,&[],0x30,None,
        |offset,write| { assert_eq!((offset,write),(0x30,None));0x81050010 },
        |_|panic!("version kick")).unwrap();
    let (mut v,mut frame) = stopped(0x803,false);
    assert_eq!(handle_native_guest_apic_msr(0xfee00d00,&mut v,&mut frame,&[0x0f,0x32],
        |index| { assert_eq!(index,0x803);0x81050010 }),Ok(NativeMsrOutcome::Completed));
    assert_eq!(v.guest_rax(),u64::from(xapic));
    assert_eq!(xapic,0x01050010);
    assert_eq!(frame.rdx,0);
    assert_eq!(v.guest_rip(),0x12345002);
}

#[test]
fn hidden_extensions_never_reach_physical_mmio_and_msr_faults_retain_instruction() {
    let mut owner = NativeIcr::admit(0,&[0,16]).unwrap();
    owner.enable_startup(0).unwrap();
    for index in 0x800..=0x8ff {
        if !native_guest_apic_msr(index) { continue; }
        for write in [false,true] {
            if index == 0x803 && !write { continue; }
            let (mut v,mut frame) = stopped(index,write);
            let original_frame = frame;
            let instruction = if write { [0x0f,0x30] } else { [0x0f,0x32] };
            assert_eq!(handle_native_guest_apic_msr(0xfee00d00,&mut v,&mut frame,&instruction,
                |_|panic!("hidden MSR escaped")),Ok(NativeMsrOutcome::GeneralProtectionPrepared));
            assert_eq!(v.guest_rip(),0x12345000);
            assert_eq!(v.guest_rax(),0xdeadbeef);
            assert_eq!(frame,original_frame);
            let offset = ((index-0x800)*16) as u16;
            assert_eq!(owner.xapic_access(0xfee00900,&[],offset,write.then_some(7),
                |_,_|panic!("hidden MMIO escaped"), |_|panic!("hidden MMIO kick")),
                Err(NativeIcrError::UnsupportedRegister));
        }
    }
}

#[test]
fn presentation_intercepts_cover_both_directions_without_trapping_ordinary_apic() {
    let mut map = Msrpm::native_boot();
    map.intercept_native_startup();
    for index in 0x800..=0x8ff {
        let slot = usize::try_from(index).unwrap()*2;
        let bits = (map.bytes()[slot/8] >> (slot%8)) & 3;
        if native_guest_apic_msr(index) { assert_eq!(bits,3,"{index:x}"); }
        if matches!(index,0x802|0x808|0x80b|0x80d|0x80f|0x832|0x838|0x83f) {
            assert_eq!(bits,0,"{index:x}");
        }
    }
}

#[test]
fn version_msr_unavailable_bus_fault_and_invalid_instruction_have_no_callback() {
    for base in [0xfee00900,0xfee00100] {
        let (mut v,mut frame) = stopped(0x803,false);
        assert_eq!(handle_native_guest_apic_msr(base,&mut v,&mut frame,&[0x0f,0x32],
            |_|panic!("wrong bus")),Ok(NativeMsrOutcome::GeneralProtectionPrepared));
        assert_eq!(v.guest_rip(),0x12345000);
    }
    let (mut v,mut frame) = stopped(0x803,false);
    let before = *v.bytes();
    assert!(handle_native_guest_apic_msr(0xfee00d00,&mut v,&mut frame,&[0x90],
        |_|panic!("bad instruction")).is_err());
    assert_eq!(*v.bytes(),before);
}

#[test]
fn conventional_dfr_masks_physical_reserved_bits_and_returns_ones() {
    let mut owner = NativeIcr::admit(0,&[0,16]).unwrap();
    owner.enable_startup(0).unwrap();
    for model in [0,0xf0000000] {
        let result = owner.xapic_access(0xfee00900,&[],0xe0,Some(model|0x0fffffff),
            |offset,write| { assert_eq!((offset,write),(0xe0,Some(u64::from(model))));model },
            |_|panic!("DFR kick")).unwrap();
        assert_eq!(result,model|0x0fffffff);
    }
}
