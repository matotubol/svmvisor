use svmvisor_hypervisor::{
    capabilities::{CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities},
    memory::address::EncryptionState,
    registers::GuestRegisters,
    host::resident::fetch,
    svm::{dispatch::{handle_native_cpuid_with_nrip, DispatchOutcome}, vmcb::Vmcb},
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
    for (offset,value) in [(0x70,0x72), (0xc8,rip+2), (0x410,0x29bu64<<16),
        (0x4d0,0x1500), (0x558,0x80000001), (0x548,0x20), (0x550,0x1000),
        (0x578,rip), (0x570,0x10002), (0x5f8,0), (0x668,0x0007040600070606)] {
        put(&mut vmcb,offset,value);
    }
    (vmcb,GuestRegisters { rbx:u64::MAX,rcx:u64::MAX,rdx:u64::MAX,rbp:0x1234,r15:0x5678,..Default::default() })
}

#[test]
fn hardware_cpuid_completes_without_rewalking_legal_wb_cache_selection() {
    let (mut vmcb,mut frame)=stopped(0xfffff806a77701d9);
    // PAT1 is WB; CR3.PWT selects it for the first table. Fetch now accepts
    // that selector, but NRIP completion must still need no physical backing.
    put(&mut vmcb,0x550,0x1008);
    let mut reads=0;
    assert!(matches!(fetch::instruction(&vmcb,48,word(&vmcb,0x668),|_,_| { reads+=1;None }),
               Err(fetch::FetchError::Walk(svmvisor_hypervisor::host::paging::WalkError::UnreadableTable { .. }))));
    assert_eq!(reads,1);
    let original=frame;
    assert_eq!(handle_native_cpuid_with_nrip(&mut vmcb,&mut frame,&capabilities(true),[10,11,12,13],true),
               Ok(DispatchOutcome::ResumePrepared));
    assert_eq!(vmcb.guest_rip(),0xfffff806a77701db);
    assert_eq!(vmcb.guest_rax(),10);
    assert_eq!((frame.rbx,frame.rcx,frame.rdx),(11,12,13));
    assert_eq!((frame.rbp,frame.r15),(original.rbp,original.r15));
    assert_eq!(word(&vmcb,0x550),0x1008);
    assert_eq!(word(&vmcb,0x570),2); // Existing retirement clears RF, not other flags.
}

#[test]
fn malformed_or_unowned_hardware_continuations_leave_all_state_unchanged() {
    for case in 0..15 {
        let (mut vmcb,mut frame)=stopped(0x2000);
        let mut supported=true;
        let mut startup=true;
        match case {
            0 => supported=false,
            1 => put(&mut vmcb,0xc8,0),
            2 => put(&mut vmcb,0xc8,0x2001),
            3 => put(&mut vmcb,0xc8,0x2003), // Prefixes, including LOCK, unsupported.
            4 => put(&mut vmcb,0xc8,0x1fff),
            5 => put(&mut vmcb,0xc8,0x0000800000000000),
            6 => put(&mut vmcb,0x70,0x81),
            7 => put(&mut vmcb,0x4c8,3<<24), // CpuidUserDis may apply outside CPL0.
            8 => put(&mut vmcb,0x4d0,0x1000),
            9 => put(&mut vmcb,0x410,0x9b<<16),
            10 => put(&mut vmcb,0x570,0x102),
            11 => put(&mut vmcb,0xa8,1<<31),
            12 => put(&mut vmcb,0x88,1<<31),
            13 => put(&mut vmcb,0x60,1<<25),
            _ => { put(&mut vmcb,0x60,1<<24); startup=false; }
        }
        let bytes=*vmcb.bytes();let old=frame;
        assert!(handle_native_cpuid_with_nrip(&mut vmcb,&mut frame,&capabilities(supported),[10,11,12,13],startup).is_err(),"case {case}");
        assert_eq!(vmcb.bytes(),&bytes,"case {case}");assert_eq!(frame,old,"case {case}");
    }
}

#[test]
fn hardware_continuation_reuses_native_cpuid_policy_and_interrupt_shadow_retirement() {
    let (mut vmcb,mut frame)=stopped(0x2fff); // Page-crossing opcode needs no memory reread.
    put(&mut vmcb,0x5f8,1);
    put(&mut vmcb,0x548,0x20|(1<<18));
    put(&mut vmcb,0x68,1);
    put(&mut vmcb,0x60,1<<24);
    handle_native_cpuid_with_nrip(&mut vmcb,&mut frame,&capabilities(true),[1,2,1<<26,4],true).unwrap();
    assert_eq!(frame.rcx,(1<<26)|(1<<27)); // OSXSAVE follows guest CR4 when XSAVE exists.
    assert_eq!(vmcb.guest_rip(),0x3001);
    assert_eq!(word(&vmcb,0x68),0);
    assert_eq!(word(&vmcb,0x60),1<<24);
}
