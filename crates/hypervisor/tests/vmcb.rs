use core::mem::{align_of, size_of};

use svmvisor_hypervisor::{
    arch::x86_64::capabilities::{
        CapabilityError, CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures,
    },
    memory::address::{AddressPolicy, EncryptionState},
    svm::vmcb::{EventIntercept, InstructionIntercept, VMCB_BYTES, Vmcb},
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

#[test]
fn storage_is_one_aligned_zeroed_page() {
    assert_eq!(VMCB_BYTES, 4096);
    assert_eq!(size_of::<Vmcb>(), 4096);
    assert_eq!(align_of::<Vmcb>(), 4096);
    let vmcb = Vmcb::default();
    assert_eq!((vmcb.bytes().as_ptr() as usize) % 4096, 0);
    assert_eq!(vmcb.bytes(), &[0; 4096]);
    assert_eq!(vmcb.guest_rip(), 0);
    assert_eq!(vmcb.guest_rsp(), 0);
    assert_eq!(vmcb.guest_rax(), 0);
}

#[test]
fn fields_match_independent_appendix_b_byte_image_with_all_other_bytes_zero() {
    let mut vmcb = Vmcb::new();
    let capabilities = CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(1),
        asid_count: Some(0x0200_0000),
        physical_address_bits: Some(48),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption: EncryptionState::Unencrypted { encryption_bit: None },
        optional: OptionalFeatures::default(),
    }
    .validate()
    .unwrap();
    vmcb.set_guest_asid(0x0102_0304, &capabilities).unwrap();
    vmcb.set_permission_maps(0x1234_5678_9000, 0x2345_6789_a000, &policy()).unwrap();
    vmcb.set_nested_root(0x3456_789a_b000, &policy()).unwrap();
    vmcb.set_instruction_intercept(InstructionIntercept::Cpuid, true);
    vmcb.set_instruction_intercept(InstructionIntercept::Hlt, true);
    vmcb.set_instruction_intercept(InstructionIntercept::Vmrun, true);
    vmcb.set_guest_rax(0x1234_5678_9abc_def0);
    vmcb.invalidate_all();

    // Explicit little-endian fixtures, not implementation offset constants.
    let mut expected = [0; 4096];
    expected[0x00c..0x010].copy_from_slice(&[0, 0, 4, 1]);
    expected[0x010..0x014].copy_from_slice(&[1, 0, 0, 0]);
    expected[0x040..0x048].copy_from_slice(&[0, 0x90, 0x78, 0x56, 0x34, 0x12, 0, 0]);
    expected[0x048..0x050].copy_from_slice(&[0, 0xa0, 0x89, 0x67, 0x45, 0x23, 0, 0]);
    expected[0x058..0x05c].copy_from_slice(&[4, 3, 2, 1]);
    expected[0x05c] = 1; // ASID/root replacement queues a full flush.
    expected[0x0b0..0x0b8].copy_from_slice(&[0, 0xb0, 0x9a, 0x78, 0x56, 0x34, 0, 0]);
    expected[0x5f8..0x600].copy_from_slice(&[0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12]);
    assert_eq!(vmcb.bytes(), &expected);
    assert_eq!(vmcb.guest_asid(), 0x0102_0304);
    assert_eq!(vmcb.permission_maps(), (0x1234_5678_9000, 0x2345_6789_a000));
    assert_eq!(vmcb.nested_root(), 0x3456_789a_b000);
    assert_eq!(vmcb.guest_rax(), 0x1234_5678_9abc_def0);

    let before = *vmcb.bytes();
    for invalid in [0, capabilities.asid_count(), u32::MAX] {
        assert_eq!(vmcb.set_guest_asid(invalid, &capabilities), Err(CapabilityError::InvalidAsid));
        assert_eq!(vmcb.bytes(), &before);
    }
}

#[test]
fn physical_fields_reject_bad_extent_or_alignment_without_partial_mutation() {
    let mut vmcb = Vmcb::new();
    vmcb.set_permission_maps(0x1000, 0x4000, &policy()).unwrap();
    vmcb.set_nested_root(0x6000, &policy()).unwrap();
    let before = *vmcb.bytes();
    let limit = 1u64 << 48;
    for (iopm, msrpm) in [
        (0x1001, 0x8000),
        (0x8000, 0x1001),
        (limit - 8192, 0x8000),
        (0x8000, limit - 4096),
        (u64::MAX, 0x8000),
    ] {
        assert!(vmcb.set_permission_maps(iopm, msrpm, &policy()).is_err());
        assert_eq!(vmcb.bytes(), &before);
    }
    for root in [0x1001, limit, u64::MAX] {
        assert!(vmcb.set_nested_root(root, &policy()).is_err());
        assert_eq!(vmcb.bytes(), &before);
    }
    // Full map spans can end exactly at the physical-address ceiling.
    vmcb.set_permission_maps(limit - 12288, limit - 8192, &policy()).unwrap();
    vmcb.set_nested_root(limit - 4096, &policy()).unwrap();
}

#[test]
fn typed_intercepts_toggle_independently_and_return_to_reserved_zero_image() {
    let mut vmcb = Vmcb::new();
    let intercepts = [
        InstructionIntercept::Rdtsc,
        InstructionIntercept::Rdtscp,
        InstructionIntercept::Cpuid,
        InstructionIntercept::Hlt,
        InstructionIntercept::Vmrun,
        InstructionIntercept::Xsetbv,
    ];
    for intercept in intercepts {
        vmcb.set_instruction_intercept(intercept, true);
        vmcb.set_instruction_intercept(intercept, true);
    }
    for intercept in intercepts {
        assert!(vmcb.instruction_intercept(intercept));
    }
    // AMD Appendix B: control offset010h bit13 intercepts XSETBV.
    assert_eq!(vmcb.bytes()[0x11], 0x20);
    // Appendix B: RDTSC 00Ch bit14; RDTSCP 010h bit7.
    assert_eq!(vmcb.bytes()[0x0d] & 0x40, 0x40);
    assert_eq!(vmcb.bytes()[0x10] & 0x80, 0x80);
    vmcb.set_instruction_intercept(InstructionIntercept::Cpuid, false);
    assert!(!vmcb.instruction_intercept(InstructionIntercept::Cpuid));
    assert!(vmcb.instruction_intercept(InstructionIntercept::Hlt));
    assert!(vmcb.instruction_intercept(InstructionIntercept::Vmrun));
    for intercept in intercepts {
        vmcb.set_instruction_intercept(intercept, false);
    }
    assert_eq!(vmcb.bytes(), &[0; 4096]);
}

#[test]
fn physical_event_intercepts_match_appendix_b_without_changing_other_fields() {
    // Independent Appendix B byte encodings, including the distinct NMI/SMI/
    // INIT bits. A decoder/setter round trip alone could share an offset bug.
    for (event, byte) in
        [(EventIntercept::Nmi, 0x02), (EventIntercept::Smi, 0x04), (EventIntercept::Init, 0x08)]
    {
        let mut vmcb = Vmcb::new();
        vmcb.set_event_intercept(event, true);
        let mut expected = [0; 4096];
        expected[0x00c] = byte;
        assert_eq!(vmcb.bytes(), &expected);
        assert!(vmcb.event_intercept(event));
        vmcb.set_event_intercept(event, false);
        assert_eq!(vmcb.bytes(), &[0; 4096]);
        assert!(!vmcb.event_intercept(event));
    }

    let mut vmcb = Vmcb::new();
    vmcb.set_instruction_intercept(InstructionIntercept::Cpuid, true);
    vmcb.set_instruction_intercept(InstructionIntercept::Vmmcall, true);
    let instructions = *vmcb.bytes();
    for event in [EventIntercept::Nmi, EventIntercept::Smi, EventIntercept::Init] {
        vmcb.set_event_intercept(event, true);
    }
    let mut expected = instructions;
    expected[0x00c] = 0x0e;
    assert_eq!(vmcb.bytes(), &expected);
    vmcb.set_event_intercept(EventIntercept::Smi, false);
    expected[0x00c] = 0x0a;
    assert_eq!(vmcb.bytes(), &expected);
    assert!(vmcb.event_intercept(EventIntercept::Nmi));
    assert!(!vmcb.event_intercept(EventIntercept::Smi));
    assert!(vmcb.event_intercept(EventIntercept::Init));
    vmcb.set_event_intercept(EventIntercept::Nmi, false);
    vmcb.set_event_intercept(EventIntercept::Init, false);
    assert_eq!(vmcb.bytes(), &instructions);
}
