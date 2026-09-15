use svmvisor_hypervisor::{
 capabilities::{CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities},
 memory::address::EncryptionState, registers::GuestRegisters, 
 svm::{dispatch::{handle_native_vmcr, handle_native_vmcr_with_nrip, NativeVmCrError, NativeMsrOutcome, NATIVE_VM_CR_VALUE}, vmcb::Vmcb},
};
fn capabilities(nrip: bool) -> ValidatedCapabilities {
    CapabilityEvidence {
        vendor: CpuVendor::Amd, svm: EvidenceFlag::Set, nested_paging: EvidenceFlag::Set,
        svm_revision: Some(1), asid_count: Some(16), physical_address_bits: Some(48),
        vm_cr_svmdis: EvidenceFlag::Clear, hypervisor_present: EvidenceFlag::Clear,
        encryption: EncryptionState::Unencrypted { encryption_bit: None },
        optional: OptionalFeatures { nrip_save: nrip, ..Default::default() },
    }.validate().unwrap()
}

// Model an exclusively stopped hardware VMCB. Production exposes no mutable
// byte accessor; these tests make no claim of executing the physical CPU.
fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
        (vmcb as *mut Vmcb).cast::<u8>().add(offset), 8); }
}
fn word(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset+8].try_into().unwrap())
}
fn stopped(rip: u64) -> (Vmcb, GuestRegisters) {
    let mut vmcb = Vmcb::new();
    for (offset,value) in [(0x70,0x7c), (0xc8,rip+2), (0x410,0x29bu64<<16),
        (0x4d0,0x1500), (0x558,0x80000001), (0x548,0x20), (0x550,0x1000),
        (0x578,rip), (0x570,0x10002), (0x5f8,0), (0x668,0x0007040600070606)] {
        put(&mut vmcb,offset,value);
    }
    (vmcb,GuestRegisters { rbx:u64::MAX,rcx:0xc0010114,rdx:u64::MAX,rbp:0x1234,r15:0x5678,..Default::default() })
}



#[test]
fn all_bits_and_mixed_writes_have_transactional_outcomes() {
 for value in (0..64).map(|bit|1u64<<bit).chain([0,8,16,24,0xffffffffffffffff,0x22]) {
  for hardware in [false,true] {
   let (mut v,mut f)=stopped(0x2000);put(&mut v,0x78,1);
   put(&mut v,0x5f8,0xdeadbeef00000000 | value as u32 as u64);f.rdx=0xabcdef0000000000 | value>>32;
   let before=(*v.bytes(),f);let oldrax=v.guest_rax();
   let result=if hardware {handle_native_vmcr_with_nrip(&mut v,&mut f,&capabilities(true),true)}
      else {handle_native_vmcr(&mut v,&mut f,&[0x0f,0x30],true)};
   if value & !0x1f != 0 {
    assert_eq!(result,Ok(NativeMsrOutcome::GeneralProtectionPrepared),"{value:x}");
    assert_eq!(v.guest_rip(),0x2000);assert_eq!(word(&v,0xa8),0x80000b0d);assert_eq!(word(&v,0x570),0x10002);
   } else if value & 7 != 0 {
    assert_eq!(result,Err(NativeVmCrError::UnsupportedValue{value}));assert_eq!((*v.bytes(),f),before);
   } else {
    assert_eq!(result,Ok(NativeMsrOutcome::Completed));assert_eq!(v.guest_rip(),0x2002);assert_eq!(word(&v,0x570),2);
   }
   assert_eq!(f,before.1);assert_eq!(v.guest_rax(),oldrax);assert_eq!(word(&v,0x4d0),0x1500);
   assert_eq!(v.bytes()[0x5c],before.0[0x5c]); // VM_CR policy never requests TLB flush.
  }
 }
}
#[test]
fn reads_and_ignored_writes_cover_all_hardware_lengths() {
 for length in 2..=15 {
  for value in [0,8,16,24] {
   let (mut v,mut f)=stopped(0x2000);put(&mut v,0x78,1);put(&mut v,0x5f8,value);f.rdx=0;put(&mut v,0xc8,0x2000+length);
   assert_eq!(handle_native_vmcr_with_nrip(&mut v,&mut f,&capabilities(true),true),Ok(NativeMsrOutcome::Completed));
   put(&mut v,0x78,0);put(&mut v,0xc8,0x2000+length*2);f.rdx=u64::MAX;
   let old=f;
   assert_eq!(handle_native_vmcr_with_nrip(&mut v,&mut f,&capabilities(true),true),Ok(NativeMsrOutcome::Completed));
   assert_eq!(v.guest_rax(),NATIVE_VM_CR_VALUE);assert_eq!(f.rdx,0);assert_eq!(v.guest_rip(),0x2000+length*2);
   assert_eq!((f.rcx,f.rbx,f.r15),(old.rcx,old.rbx,old.r15));
  }
 }
}
#[test]
fn malformed_hardware_and_pending_debug_state_preserve_every_byte() {
 for case in 0..19 {
  let (mut v,mut f)=stopped(0x2000);let mut supported=true;
  match case {
   0=>supported=false,1=>put(&mut v,0xc8,0),2=>put(&mut v,0xc8,0x2001),3=>put(&mut v,0xc8,0x2010),
   4=>put(&mut v,0xc8,0x1fff),5=>put(&mut v,0xc8,0x800000000000),6=>put(&mut v,0x70,0x72),
   7=>put(&mut v,0x78,2),8=>put(&mut v,0x4c8,3<<24),9=>put(&mut v,0x410,0x9b<<16),
   10=>put(&mut v,0x558,1),11=>put(&mut v,0x548,0),12=>put(&mut v,0x548,0x1020),
   13=>put(&mut v,0x570,0x102),14=>put(&mut v,0xa8,1<<31),15=>put(&mut v,0x88,1<<31),
   16=>put(&mut v,0x60,1<<25),17=>put(&mut v,0x578,0x800000000000),_=>f.rcx=0xc0000080,
  }
  let before=(*v.bytes(),f);assert!(handle_native_vmcr_with_nrip(&mut v,&mut f,&capabilities(supported),true).is_err(),"case{case}");
  assert_eq!((*v.bytes(),f),before,"case{case}");
 }
}
#[test]
fn byte_fallback_faults_cpl_and_refuses_prefixes() {
 let (mut v,mut f)=stopped(0x2000);put(&mut v,0xc8,0);
 assert_eq!(handle_native_vmcr(&mut v,&mut f,&[0x0f,0x32],true),Ok(NativeMsrOutcome::Completed));
 let before=(*v.bytes(),f);assert!(handle_native_vmcr(&mut v,&mut f,&[0x48,0x0f,0x32],true).is_err());assert_eq!((*v.bytes(),f),before);
 put(&mut v,0x4c8,3<<24);let before=(v.guest_rip(),v.guest_rax(),f);
 assert_eq!(handle_native_vmcr(&mut v,&mut f,&[0x0f,0x32],true),Ok(NativeMsrOutcome::GeneralProtectionPrepared));
 assert_eq!((v.guest_rip(),v.guest_rax(),f),before);
}
