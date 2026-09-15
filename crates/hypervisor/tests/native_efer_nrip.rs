use svmvisor_hypervisor::{
 capabilities::{CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities},
 memory::address::EncryptionState, registers::GuestRegisters, host::resident::fetch,
 svm::{dispatch::{handle_native_efer, handle_native_efer_with_nrip, NativeEfer, NativeMsrOutcome}, vmcb::Vmcb},
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
    (vmcb,GuestRegisters { rbx:u64::MAX,rcx:0xc0000080,rdx:u64::MAX,rbp:0x1234,r15:0x5678,..Default::default() })
}


fn owner() -> NativeEfer { let mut e=NativeEfer::admit(0x500,true).unwrap(); e.enable_guest_startup(); e }

#[test]
fn decoded_prefix_lengths_complete_without_guest_memory() {
 for length in [2,3,4,7,15] {
  for write in [false,true] {
   let (mut v,mut f)=stopped(0xfffff80012345fff);
   let mut e=owner(); let caps=capabilities(true);
   let rip=v.guest_rip();put(&mut v,0xc8,rip+length);
   put(&mut v,0x78,write as u64);
   if write { put(&mut v,0x5f8,0xd01); f.rdx=0; }
   let old=f;
   // The same guest has no readable tables through this supplied reader.
   assert!(fetch::instruction(&v,48,word(&v,0x668),|_,_|None).is_err());
   assert_eq!(handle_native_efer_with_nrip(&mut e,&mut v,&mut f,&caps),Ok(NativeMsrOutcome::Completed));
   assert_eq!(v.guest_rip(),rip+length);
   assert_eq!(e.logical(),if write {0xd01}else{0x500});
   assert_eq!(word(&v,0x4d0),e.logical()|0x1000);
   if write { assert_eq!(f,old); } else { assert_eq!(v.guest_rax(),0x500);assert_eq!(f.rdx,0); }
   assert_eq!((f.rbx,f.rcx,f.r15),(old.rbx,old.rcx,old.r15));
   assert_eq!(word(&v,0x570),2);
  }
 }
}

#[test]
fn malformed_hardware_evidence_and_pending_state_refuse_transactionally() {
 for case in 0..20 {
  let (mut v,mut f)=stopped(0x2000);let mut e=owner();let mut supported=true;
  match case {
   0=>supported=false, 1=>put(&mut v,0xc8,0), 2=>put(&mut v,0xc8,0x2001),
   3=>put(&mut v,0xc8,0x2010),4=>put(&mut v,0xc8,0x1fff),
   5=>put(&mut v,0xc8,0x0000800000000000),6=>put(&mut v,0x70,0x72),
   7=>put(&mut v,0x78,2),8=>put(&mut v,0x4c8,3<<24),
   9=>put(&mut v,0x410,0x9b<<16),10=>put(&mut v,0x558,1),
   11=>put(&mut v,0x548,0),12=>put(&mut v,0x548,0x1020),
   13=>put(&mut v,0x570,0x102),14=>put(&mut v,0xa8,1<<31),
   15=>put(&mut v,0x88,1<<31),16=>put(&mut v,0x60,1<<25),
   17=>put(&mut v,0x578,0x0000800000000000),18=>f.rcx=0x1b,
   _=>put(&mut v,0x4d0,0x1d00),
  }
  let before=(e,*v.bytes(),f);
  assert!(handle_native_efer_with_nrip(&mut e,&mut v,&mut f,&capabilities(supported)).is_err(),"case {case}");
  assert_eq!((e,*v.bytes(),f),before,"case {case}");
 }
}

#[test]
fn nrip_boundaries_are_checked_without_wrapping() {
 for (rip,next,ok) in [(0x7ffffffffffd,0x7fffffffffff,true),(0x7ffffffffffe,0x800000000000,false),
  (0xfffffffffffffff0,0xffffffffffffffff,true),(0xfffffffffffffffe,0,false)] {
  let (mut v,mut f)=stopped(0x2000);let mut e=owner();
  put(&mut v,0x578,rip);put(&mut v,0xc8,next);let before=(e,*v.bytes(),f);
  assert_eq!(handle_native_efer_with_nrip(&mut e,&mut v,&mut f,&capabilities(true)).is_ok(),ok);
  if ok {assert_eq!(v.guest_rip(),next);} else {assert_eq!((e,*v.bytes(),f),before);}
 }
}

#[test]
fn prefixed_fault_uses_original_rip_and_real_efer_fault_owner() {
 for length in [3,15] {
  let (mut v,mut f)=stopped(0x2000);let mut e=owner();put(&mut v,0x78,1);
  put(&mut v,0xc8,0x2000+length);put(&mut v,0x5f8,0x100);f.rdx=0;
  let before=(e,f,v.guest_rax(),v.guest_rip(),word(&v,0x4d0));
  assert_eq!(handle_native_efer_with_nrip(&mut e,&mut v,&mut f,&capabilities(true)),Ok(NativeMsrOutcome::GeneralProtectionPrepared));
  assert_eq!((e,f,v.guest_rax(),v.guest_rip(),word(&v,0x4d0)),before);
  assert_eq!(word(&v,0xa8),0x80000b0d); // #GP(0), exception, valid/error-valid.
  assert_eq!(word(&v,0x570),0x10002); // Fault does not retire RF.
 }
}

#[test]
fn byte_fallback_remains_exact_and_does_not_require_nrips() {
 let (mut v,mut f)=stopped(0x2000);let mut e=owner();put(&mut v,0xc8,0);
 assert_eq!(handle_native_efer(&mut e,&mut v,&mut f,&[0x0f,0x32]),Ok(NativeMsrOutcome::Completed));
 let before=(e,*v.bytes(),f);
 assert!(handle_native_efer(&mut e,&mut v,&mut f,&[0x48,0x0f,0x32]).is_err());
 assert_eq!((e,*v.bytes(),f),before);
}

#[test]
fn hardware_diagnostics_report_real_nrip_not_synthetic_opcode_length() {
 use svmvisor_hypervisor::{host::resident::terminal,svm::{dispatch::NativeEferError,exit::ResumeError}};
 let (mut v,_)=stopped(0x2000);put(&mut v,0xc8,0x800000000000);
 let (_,payload)=terminal::efer_nrip_failure(NativeEferError::Instruction(ResumeError::NonCanonicalNrip),&v,0x500);
 assert_eq!(payload,0x800000000000);
}
