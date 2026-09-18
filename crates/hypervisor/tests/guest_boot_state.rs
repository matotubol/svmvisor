//! Join descriptors, both translation stages and the VMCB without CPU entry.

use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::EvidenceFlag, descriptors::GuestDescriptorRequest},
    guest::{
        pages::{GuestPages, PagePermissions as GuestAccess, TableStorage as GuestStorage},
        state::GuestStateRequest,
    },
    memory::{
        address::{AddressPolicy, EncryptionState},
        npt::{Npt, NptEvidence, PagePermissions as HostAccess, TableStorage as HostStorage},
    },
    svm::vmcb::Vmcb,
};

#[test]
fn guest_code_stacks_descriptors_and_table_walks_have_separate_backing() {
    let policy =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let descriptors =
        GuestDescriptorRequest { gdt_base: 0x4000, tss_base: 0x5000, rsp0: 0x9000, ist1: 0xb000 }
            .validate()
            .unwrap();
    let mut guest_storage = GuestStorage([[0; 4096]; 4]);
    let mut guest = GuestPages::new(&mut guest_storage, 0x10000, policy).unwrap();
    let mut host_storage = HostStorage([[0; 4096]; 8]);
    let mut npt = Npt::new(
        &mut host_storage,
        0x100000,
        policy,
        48,
        NptEvidence {
            nx_supported: EvidenceFlag::Set,
            host_nxe: EvidenceFlag::Set,
            host_four_level: EvidenceFlag::Set,
        },
    )
    .unwrap();
    for (va, hpa, guest_access, host_access) in [
        (0x1000, 0x200000, GuestAccess::ReadOnly, HostAccess::ReadExecute),
        (0x4000, 0x201000, GuestAccess::ReadWrite, HostAccess::ReadWrite),
        (0x5000, 0x202000, GuestAccess::ReadWrite, HostAccess::ReadWrite),
        (0x8000, 0x203000, GuestAccess::ReadWrite, HostAccess::ReadWrite),
        (0xa000, 0x204000, GuestAccess::ReadWrite, HostAccess::ReadWrite),
    ] {
        guest.map_page(va, va, guest_access).unwrap();
        npt.map_page(va, hpa, host_access).unwrap();
        let gpa = guest.translate(va + 4095).unwrap().unwrap().guest_address;
        assert_eq!(npt.translate(gpa).unwrap().unwrap().host_address, hpa + 4095);
    }
    // Hardware guest-table walks need NPT write permission for A/D updates.
    for i in 0..4 {
        let table = guest.table(i).unwrap();
        npt.map_page(table.guest_address, 0x210000 + i as u64 * 4096, HostAccess::ReadWrite)
            .unwrap();
        assert_eq!(
            npt.translate(table.guest_address).unwrap().unwrap().permissions,
            HostAccess::ReadWrite
        );
    }
    for guard in [0, 0x7000, 0x9000, 0xb000] {
        assert_eq!(guest.translate(guard).unwrap(), None);
        assert_eq!(npt.translate(guard).unwrap(), None);
    }
    assert_eq!(npt.translate(0x100000).unwrap(), None); // own host tables hidden
    let state = GuestStateRequest {
        rip: 0x1000,
        rsp: 0x9000,
        rflags: 2,
        cr0: 0x8001_0033,
        cr3: guest.root_address(),
        cr4: 0x20,
        efer: 0x1500,
        rax: 0,
    }
    .validate(&policy)
    .unwrap();
    let mut vmcb = Vmcb::new();
    vmcb.set_synthetic_state(&state);
    vmcb.set_guest_descriptors(&descriptors);
    vmcb.set_nested_root(npt.root_address(), &policy).unwrap();
    assert_eq!(&vmcb.bytes()[0x410..0x418], &[8, 0, 0x9b, 0x0a, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(&vmcb.bytes()[0x420..0x428], &[16, 0, 0x93, 0x0c, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(&vmcb.bytes()[0x490..0x498], &[24, 0, 0x8b, 0, 103, 0, 0, 0]);
    assert_eq!(&vmcb.bytes()[0x550..0x558], &0x10000u64.to_le_bytes());
    assert_eq!(&vmcb.bytes()[0x0b0..0x0b8], &0x100000u64.to_le_bytes());
    assert_eq!(&vmcb.bytes()[0x468..0x470], &0x4000u64.to_le_bytes());
    assert_eq!(&vmcb.bytes()[0x498..0x4a0], &0x5000u64.to_le_bytes());
    assert_eq!(&descriptors.tss()[4..12], &0x9000u64.to_le_bytes());
    assert_eq!(&descriptors.tss()[36..44], &0xb000u64.to_le_bytes());
    // IDT and NP_ENABLE remain unconfigured: this is not a runnable guest.
    assert_eq!(&vmcb.bytes()[0x480..0x490], &[0; 16]);
    assert_eq!(vmcb.bytes()[0x090], 0);
}
