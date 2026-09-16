//! Independent regression audit of Version DWORD accesses and NPF provenance.
use svmvisor_hypervisor::svm::ipi::NativeIcrError;
use svmvisor_hypervisor::{arch::x86_64::registers::GuestRegisters, svm::{vmcb::Vmcb, native_mmio::handle_native_mmio_detailed}};
fn set(v: &mut Vmcb, offset: usize, value: u64) { unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), (v as *mut Vmcb).cast::<u8>().add(offset),8); } }
fn fixture(write: bool) -> (Vmcb, GuestRegisters) {
    let mut v=Vmcb::new();
    for (o,x) in [(0x70,0x400),(0x78,(1<<32)|4|if write {2}else{0}), (0x80,0xfee00030),(0x90,1),(0x410,0x200<<16),(0x4d0,0x1d00),(0x548,0x20|(1<<22)),(0x550,0x1000),(0x558,0x80010001),(0x570,2),(0x578,0x8000),(0x5f8,0xabcdef0181050010)] {set(&mut v,o,x)}
    (v,GuestRegisters {rbx:0xa030,..Default::default()})
}
fn read(address:u64,width:usize,write:bool)->Option<u64> {
    match (address,width) {
        (0x1000,8)=>Some(0x2003), (0x2000,8)=>Some(0x3007), (0x3000,8)=>Some(0x4007),
        (0x4040,8)=>Some(0x9007), (0x4050,8)=>Some(0x78000000fee0000f),
        (0x9000,1)=>Some(if write {0x89}else{0x8b}), (0x9001,1)=>Some(3), _=>None
    }
}
#[test]
fn version_operand_uses_effective_supervisor_permission_with_nonzero_key() {
    for write in [false,true] {
        let (mut v,mut f)=fixture(write); let before=f;
        handle_native_mmio_detailed::<NativeIcrError>(&mut v,&mut f,48,6,0xfee00000,|a,n|read(a,n,write),|_,o,w| {
            assert_eq!(o,0x30); assert_eq!(w,write.then_some(0x81050010)); Ok(0x81050010)
        }).unwrap();
        assert_eq!(f,before); assert_eq!(v.guest_rip(),0x8002);
        assert_eq!(v.guest_rax(),if write {0xabcdef0181050010}else{0x81050010});
    }
}
#[test]
fn every_npf_bit_mutation_refuses_before_version_callback_and_preserves_state() {
    for cet in [false,true] { for write in [false,true] { for bit in 0..64 {
        let (mut v,mut f)=fixture(write); if cet { seed_cet(&mut v); }
        set(&mut v,0x78,((1u64<<32)|4|if write {2}else{0})^(1u64<<bit));
        let before=*v.bytes();let frame=f;
        let e=handle_native_mmio_detailed::<NativeIcrError>(&mut v,&mut f,48,6,0xfee00000,|a,n|read(a,n,write),|_,_,_|panic!("bad NPF reached callback")).unwrap_err();
        assert_eq!(e.predicate,0x19,"write={write}, bit={bit}"); assert_eq!(*v.bytes(),before);assert_eq!(f,frame);
    }}}
}
#[test]
fn unknown_cr4_remains_refused_with_and_without_cet() {
    for cet in [false,true] {let (mut v,mut f)=fixture(false); if cet { seed_cet(&mut v); }
        let cr4=0x20|(1<<22)|(1<<24)|if cet {1<<23}else{0};set(&mut v,0x548,cr4);let before=*v.bytes();let frame=f;
        let e=handle_native_mmio_detailed::<NativeIcrError>(&mut v,&mut f,48,6,0xfee00000,|_,_|panic!("unsupported mode fetched guest RAM"),|_,_,_|panic!("unsupported mode accessed device")).unwrap_err();
        assert_eq!(e.predicate,10);assert_eq!(e.operand,cr4);assert_eq!(*v.bytes(),before);assert_eq!(f,frame);
    }
}

fn seed_cet(v: &mut Vmcb) {
    // APM2 rev3.44 Table B-2, PDF807/printed745: offsets are relative
    // to the state-save area at 0x400. Ordinary MOV must not update these.
    for (o,x) in [(0x548,0x20|(1<<22)|(1<<23)),(0x5e0,3),(0x5e8,0xffff800001234000),(0x5f0,0xffff800005678000),(0x5d8,0xffff800009abc000)] {set(v,o,x);}
}

#[test]
fn cet_version_mov_preserves_shadow_state_and_only_completes_the_instruction() {
    // APM3 rev3.37 MOV, PDF276-278/printed240-242: this ordinary DWORD
    // register/memory transfer is not a CALL/RET or shadow-stack instruction.
    for pke in [false,true] { for write in [false,true] {
        let (mut v,mut f)=fixture(write);seed_cet(&mut v);
        set(&mut v,0x548,0x20|(1<<23)|if pke {1<<22}else{0});
        // NPF does not establish NRIP; completion must use the decoded MOV.
        set(&mut v,0xc8,0xffff800099990000);
        let before=*v.bytes();let frame=f;let mut calls=0;
        handle_native_mmio_detailed::<NativeIcrError>(&mut v,&mut f,48,6,0xfee00000,|a,n|read(a,n,write),|_,offset,value| {
            calls+=1;assert_eq!(offset,0x30);assert_eq!(value,write.then_some(0x81050010));Ok(0x81050010)
        }).unwrap();
        assert_eq!(calls,1);assert_eq!(f,frame);assert_eq!(v.guest_rip(),0x8002);
        assert_eq!(v.guest_rax(),if write {0xabcdef0181050010}else{0x81050010});
        // Full stopped record apart from the existing completion-owned bytes.
        let mut expected=before;
        expected[0x578..0x580].copy_from_slice(&0x8002u64.to_le_bytes());
        expected[0x5f8..0x600].copy_from_slice(&v.guest_rax().to_le_bytes());
        assert_eq!(*v.bytes(),expected);
    }}
}

#[test]
fn cet_pending_mode_fetch_and_device_refusals_preserve_all_stopped_state() {
    for write in [false,true] { for case in 0..6 {
        let (mut v,mut f)=fixture(write);seed_cet(&mut v);
        match case {
            0=>set(&mut v,0xa8,1<<31), // pending event
            1=>set(&mut v,0x60,1<<63), // unowned interrupt controls
            2=>set(&mut v,0x570,2|(1<<8)), // unsupported single-step mode
            _=>{}
        }
        let before=*v.bytes();let frame=f;let mut calls=0;
        let result=handle_native_mmio_detailed::<NativeIcrError>(&mut v,&mut f,48,6,0xfee00000,
            |a,n|if case==3&&n==1 {None} else if case==4&&a==0x4050 {None} else {read(a,n,write)},
            |_,_,_|{calls+=1;Err(svmvisor_hypervisor::svm::ipi::NativeIcrError::MailboxBusy)});
        assert!(result.is_err(),"case={case}");
        assert_eq!(calls,usize::from(case==5),"case={case}");
        assert_eq!(*v.bytes(),before,"case={case}");assert_eq!(f,frame);
    }}
}

#[test]
fn initial_cet_import_changes_only_sampled_msrs_and_clean_bits() {
    // APM2 B-2 PDF807 and 15.5.2 PDF568: VMSAVE does not import these
    // MSRs. APM2 18.12 PDF752 and target PPR PDF173-174 define their values.
    for s_cet in 0u64..=3 {for isst in [0,0x7fff_ffff_ffff,0xffff_8000_0000_0000,u64::MAX] {
        let (mut v,_)=fixture(false);
        set(&mut v,0x5e8,0xffff800001234000);
        set(&mut v,0xc0,u32::MAX as u64);
        let mut expected=*v.bytes();
        expected[0x5e0..0x5e8].copy_from_slice(&s_cet.to_le_bytes());
        expected[0x5f0..0x5f8].copy_from_slice(&isst.to_le_bytes());
        expected[0xc0..0xc4].fill(0);
        v.initialize_native_cet_msrs(s_cet,isst).unwrap();
        assert_eq!(*v.bytes(),expected);
    }}
}

#[test]
fn initial_cet_import_rejects_reserved_bits_active_cet_and_noncanonical_addresses_unchanged() {
    let check=|s_cet,isst,cet| {
        let (mut v,_)=fixture(false);if cet {seed_cet(&mut v)}
        let before=*v.bytes();
        assert!(v.initialize_native_cet_msrs(s_cet,isst).is_err());
        assert_eq!(*v.bytes(),before);
    };
    for bit in 2..64 {check(1u64<<bit,0,false);}
    for isst in [0x8000_0000_0000,0xffff_7fff_ffff_ffff,1u64<<63] {check(0,isst,false);}
    check(0,0,true);
}

#[test]
fn init_retains_cet_msrs_and_duplicate_sipi_does_not_erase_shadow_state() {
    use svmvisor_hypervisor::svm::ipi::{NativeStartupTarget,NativeStartupState as S,NativeStartupCommand as C,NativeStartupEffect as E};
    // APM2 Table14-1 PDF543-544: CR4 resets, other MSRs do not. SSP is
    // not asserted here as an INIT architectural value: it is not an MSR.
    let (mut v,mut f)=fixture(false);seed_cet(&mut v);
    let mut state=S::Running;
    for vector in [8,9,10] {
        set(&mut v,0x548,0x20|(1<<23));
        let mut target=NativeStartupTarget {vmcb:&mut v,frame:&mut f,state:&mut state,signature:0xb40f40};
        assert_eq!(target.apply(C::Init),Ok(E::Init));
        assert_eq!(&target.vmcb.bytes()[0x548..0x550],&0u64.to_le_bytes());
        assert_eq!(&target.vmcb.bytes()[0x5e0..0x5e8],&3u64.to_le_bytes());
        assert_eq!(&target.vmcb.bytes()[0x5f0..0x5f8],&0xffff800005678000u64.to_le_bytes());
        assert_eq!(target.apply(C::Sipi(vector)),Ok(E::Started));
        set(target.vmcb,0x5e8,0xffff800012345000);
        let before=*target.vmcb.bytes();
        assert_eq!(target.apply(C::Sipi(vector)),Ok(E::Ignored));
        assert_eq!(*target.vmcb.bytes(),before);
    }
}

#[test]
fn rejected_init_keeps_every_cet_field_and_frame_unchanged() {
    use svmvisor_hypervisor::svm::ipi::{NativeStartupTarget,NativeStartupState as S,NativeStartupCommand as C};
    let (mut v,mut f)=fixture(false);seed_cet(&mut v);set(&mut v,0xa8,1<<31);
    let before=*v.bytes();let frame=f;let mut state=S::Running;
    let mut target=NativeStartupTarget {vmcb:&mut v,frame:&mut f,state:&mut state,signature:0xb40f40};
    assert!(target.apply(C::Init).is_err());
    assert_eq!(*target.vmcb.bytes(),before);assert_eq!(*target.frame,frame);assert_eq!(*target.state,S::Running);
}

#[test]
fn init_preserves_each_legal_cd_nw_tuple_and_uses_initial_unaccessed_segments() {
    use svmvisor_hypervisor::svm::ipi::{NativeStartupTarget,NativeStartupState as S,NativeStartupCommand as C,NativeStartupEffect as E};
    // APM2 PDF543/printed481 preserves CD/NW on INIT; PDF545/printed483
    // specifies Type1010 code and Type0010 data, not accessed types1011/0011.
    for retained in [0u64,1<<30,(1<<30)|(1<<29)] {
        let (mut v,mut f)=fixture(false);seed_cet(&mut v);
        set(&mut v,0x558,0x8005_003f|retained);
        let mut state=S::Running;
        let mut target=NativeStartupTarget {vmcb:&mut v,frame:&mut f,state:&mut state,signature:0xb40f40};
        assert_eq!(target.apply(C::Init),Ok(E::Init));
        assert_eq!(&target.vmcb.bytes()[0x558..0x560],&(retained|0x10).to_le_bytes());
        for (segment,attributes) in [(0x410,0x9au16),(0x400,0x92),(0x420,0x92),(0x430,0x92),(0x440,0x92),(0x450,0x92)] {
            assert_eq!(&target.vmcb.bytes()[segment+2..segment+4],&attributes.to_le_bytes());
        }
    }
}
