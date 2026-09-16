//! Independent regression audit of native CET MSR import and INIT/SIPI state.
use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    memory::address::{AddressPolicy, EncryptionState},
    svm::{vmcb::Vmcb, x2avic::{NativeX2AvicProfile, X2AvicCapabilities}},
};
fn set(v: &mut Vmcb, offset: usize, value: u64) { unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), (v as *mut Vmcb).cast::<u8>().add(offset),8); } }
fn fixture() -> (Vmcb, GuestRegisters) {
    let mut v=Vmcb::new();
    for (o,x) in [(0x70,0x400),(0x78,(1<<32)|4), (0x80,0xfee00030),(0x90,1),(0x410,0x200<<16),(0x4d0,0x1d00),(0x548,0x20|(1<<22)),(0x550,0x1000),(0x558,0x80010001),(0x570,2),(0x578,0x8000),(0x5f8,0xabcdef0181050010)] {set(&mut v,o,x)}
    (v,GuestRegisters {rbx:0xa030,..Default::default()})
}

/// The armed x2AVIC profile that CPU startup commits require.
fn x2avic(v: &mut Vmcb) -> NativeX2AvicProfile {
    let policy = AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let caps = X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18)).unwrap();
    let profile = NativeX2AvicProfile::new(caps, 0x2000, 0x3000, 37, &policy).unwrap();
    v.enable_native_x2avic(&profile).unwrap();
    profile
}

fn seed_cet(v: &mut Vmcb) {
    // APM2 rev3.44 Table B-2, PDF807/printed745: offsets are relative
    // to the state-save area at 0x400.
    for (o,x) in [(0x548,0x20|(1<<22)|(1<<23)),(0x5e0,3),(0x5e8,0xffff800001234000),(0x5f0,0xffff800005678000),(0x5d8,0xffff800009abc000)] {set(v,o,x);}
}

#[test]
fn initial_cet_import_changes_only_sampled_msrs_and_clean_bits() {
    // APM2 B-2 PDF807 and 15.5.2 PDF568: VMSAVE does not import these
    // MSRs. APM2 18.12 PDF752 and target PPR PDF173-174 define their values.
    for s_cet in 0u64..=3 {for isst in [0,0x7fff_ffff_ffff,0xffff_8000_0000_0000,u64::MAX] {
        let (mut v,_)=fixture();
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
        let (mut v,_)=fixture();if cet {seed_cet(&mut v)}
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
    use svmvisor_hypervisor::svm::x2avic::startup::{NativeStartupTarget,NativeStartupState as S,NativeStartupCommand as C,NativeStartupEffect as E};
    // APM2 Table14-1 PDF543-544: CR4 resets, other MSRs do not. SSP is
    // not asserted here as an INIT architectural value: it is not an MSR.
    let (mut v,mut f)=fixture();seed_cet(&mut v);let profile=x2avic(&mut v);
    let mut state=S::Running;
    for vector in [8,9,10] {
        set(&mut v,0x548,0x20|(1<<23));
        let mut target=NativeStartupTarget {vmcb:&mut v,frame:&mut f,state:&mut state,signature:0xb40f40};
        assert_eq!(target.apply_x2avic(C::Init,&profile),Ok(E::Init));
        assert_eq!(&target.vmcb.bytes()[0x548..0x550],&0u64.to_le_bytes());
        assert_eq!(&target.vmcb.bytes()[0x5e0..0x5e8],&3u64.to_le_bytes());
        assert_eq!(&target.vmcb.bytes()[0x5f0..0x5f8],&0xffff800005678000u64.to_le_bytes());
        assert_eq!(target.apply_x2avic(C::Sipi(vector),&profile),Ok(E::Started));
        set(target.vmcb,0x5e8,0xffff800012345000);
        let before=*target.vmcb.bytes();
        assert_eq!(target.apply_x2avic(C::Sipi(vector),&profile),Ok(E::Ignored));
        assert_eq!(*target.vmcb.bytes(),before);
    }
}

#[test]
fn rejected_init_keeps_every_cet_field_and_frame_unchanged() {
    use svmvisor_hypervisor::svm::x2avic::startup::{NativeStartupTarget,NativeStartupState as S,NativeStartupCommand as C};
    let (mut v,mut f)=fixture();seed_cet(&mut v);let profile=x2avic(&mut v);set(&mut v,0xa8,1<<31);
    let before=*v.bytes();let frame=f;let mut state=S::Running;
    let mut target=NativeStartupTarget {vmcb:&mut v,frame:&mut f,state:&mut state,signature:0xb40f40};
    assert!(target.apply_x2avic(C::Init,&profile).is_err());
    assert_eq!(*target.vmcb.bytes(),before);assert_eq!(*target.frame,frame);assert_eq!(*target.state,S::Running);
}

#[test]
fn init_preserves_each_legal_cd_nw_tuple_and_uses_initial_unaccessed_segments() {
    use svmvisor_hypervisor::svm::x2avic::startup::{NativeStartupTarget,NativeStartupState as S,NativeStartupCommand as C,NativeStartupEffect as E};
    // APM2 PDF543/printed481 preserves CD/NW on INIT; PDF545/printed483
    // specifies Type1010 code and Type0010 data, not accessed types1011/0011.
    for retained in [0u64,1<<30,(1<<30)|(1<<29)] {
        let (mut v,mut f)=fixture();seed_cet(&mut v);let profile=x2avic(&mut v);
        set(&mut v,0x558,0x8005_003f|retained);
        let mut state=S::Running;
        let mut target=NativeStartupTarget {vmcb:&mut v,frame:&mut f,state:&mut state,signature:0xb40f40};
        assert_eq!(target.apply_x2avic(C::Init,&profile),Ok(E::Init));
        assert_eq!(&target.vmcb.bytes()[0x558..0x560],&(retained|0x10).to_le_bytes());
        for (segment,attributes) in [(0x410,0x9au16),(0x400,0x92),(0x420,0x92),(0x430,0x92),(0x440,0x92),(0x450,0x92)] {
            assert_eq!(&target.vmcb.bytes()[segment+2..segment+4],&attributes.to_le_bytes());
        }
    }
}
