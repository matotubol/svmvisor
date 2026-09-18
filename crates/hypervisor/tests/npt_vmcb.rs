use svmvisor_hypervisor::{
    arch::x86_64::capabilities::EvidenceFlag,
    memory::address::{AddressPolicy, EncryptionState},
    memory::npt::{Npt, NptEvidence, PAGE_BYTES, PagePermissions, TABLE_COUNT, TableStorage},
    svm::exit::ExitAction,
    svm::vmcb::{InstructionIntercept, Vmcb},
};

#[test]
fn planned_root_transfers_to_vmcb_without_enabling_npt_or_fabricating_exit() {
    let policy =
        AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap();
    let mut storage = TableStorage([[0; PAGE_BYTES]; TABLE_COUNT]);
    let mut npt = Npt::new(
        &mut storage,
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
    npt.map_page(0x2000, 0x200000, PagePermissions::ReadExecute).unwrap();
    npt.map_page(0x3000, 0x201000, PagePermissions::ReadWrite).unwrap();
    assert_eq!(npt.translate(0x2fff).unwrap().unwrap().host_address, 0x200fff);
    assert_eq!(npt.translate(0x3000).unwrap().unwrap().permissions, PagePermissions::ReadWrite);
    assert_eq!(npt.translate(0x4000).unwrap(), None); // explicit unmapped guard
    let mut vmcb = Vmcb::new();
    vmcb.set_nested_root(npt.root_address(), &policy).unwrap();
    vmcb.set_instruction_intercept(InstructionIntercept::Vmmcall, true);
    let mut expected = [0u8; 4096];
    expected[0x010] = 2;
    expected[0x05c] = 1; // Root publication queues a flush without executing it.
    expected[0x0b0..0x0b8].copy_from_slice(&0x100000u64.to_le_bytes());
    assert_eq!(vmcb.bytes(), &expected);
    assert_eq!(vmcb.exit_snapshot().action(), ExitAction::Unsupported { code: 0 });
    vmcb.set_instruction_intercept(InstructionIntercept::Vmmcall, false);
    assert!(!vmcb.instruction_intercept(InstructionIntercept::Vmmcall));
}
