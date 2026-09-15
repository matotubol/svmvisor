use svmvisor_hypervisor::{registers::GuestRegisters, svm::dispatch::{NativeEfer, NativeEferError, NativeMsrOutcome, handle_native_efer}, vmcb::Vmcb};

fn put(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), (v as *mut Vmcb).cast::<u8>().add(offset), 8); }
}
fn stopped(value: u64) -> (Vmcb, GuestRegisters) {
    let mut v=Vmcb::new();
    for (o,x) in [(0x70,0x7c),(0x78,1),(0x4d0,0x1500),(0x558,0x80000011),(0x570,2),(0x578,0x4000),(0x5f8,value as u32 as u64)] { put(&mut v,o,x); }
    (v,GuestRegisters {rcx:0xc0000080,rdx:value>>32,rbx:0xabcdef,..Default::default()})
}
fn owner() -> NativeEfer {
    NativeEfer::admit_native(0x500,1<<17,(1<<11)|(1<<20)|(1<<25)|(1<<29),(1<<13)|(1<<20),Some((1<<7)|(1<<8))).unwrap()
}

// Expectations independently enumerated from PPR57896 p186 and APM2 Fig3-9,
// not copied from the implementation mask. Test inputs are inert stopped state.
#[test]
fn every_bit_has_an_explicit_target_write_outcome() {
    for bit in 0..64 {
        let value=0x500|(1u64<<bit);
        let (mut v,mut g)=stopped(value); let before=*v.bytes(); let gb=g; let mut o=owner();
        let result=handle_native_efer(&mut o,&mut v,&mut g,&[15,48]);
        match bit {
            0|8|10|11|14|15|18|20|21 => {
                assert_eq!(result,Ok(NativeMsrOutcome::Completed),"bit {bit}");
                assert_eq!(o.logical(),value); assert_eq!(v.guest_rip(),0x4002);
                assert_eq!(u64::from_le_bytes(v.bytes()[0x4d0..0x4d8].try_into().unwrap()),value|0x1000);
            }
            1..=7 => {
                assert_eq!(result,Err(NativeEferError::UnsupportedValue { value }),"RAZ bit {bit}");
                assert_eq!(v.bytes(),&before); assert_eq!(o.logical(),0x500);
            }
            _ => {
                assert_eq!(result,Ok(NativeMsrOutcome::GeneralProtectionPrepared),"bit {bit}");
                assert_eq!(o.logical(),0x500); assert_eq!(v.guest_rip(),0x4000);
                assert_eq!(v.guest_rax(),value as u32 as u64);
                assert_eq!(u64::from_le_bytes(v.bytes()[0xa8..0xb0].try_into().unwrap()),0x80000b0d);
            }
        }
        assert_eq!(g,gb,"bit {bit} changed GPRs");
    }
}

#[test]
fn same_cpu_capability_absence_faults_each_added_control() {
    for bit in [11,14,15,18,20,21] {
        let (mut v,mut g)=stopped(0x500|(1<<bit));
        let mut o=NativeEfer::admit_native(0x500,0,(1<<11)|(1<<29),1<<20,None).unwrap();
        assert_eq!(handle_native_efer(&mut o,&mut v,&mut g,&[15,48]),Ok(NativeMsrOutcome::GeneralProtectionPrepared),"bit {bit}");
        assert_eq!(o.logical(),0x500); assert_eq!(v.guest_rip(),0x4000);
    }
}
