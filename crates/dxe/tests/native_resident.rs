#![cfg(feature = "native-preflight")]

use svmvisor_dxe::native::{
    admission::{
        boundary::{ABI_VERSION, NativeBoundary},
        snapshot::TableSnapshot,
    },
    resident::{CallbackError, CallbackRequest, CallbackSites, GuestStackSpan, prepare_callback},
};
use svmvisor_hypervisor::{
    arch::x86_64::{descriptors::GuestDescriptorRequest, registers::GuestRegisters},
    boot::descriptors::{FirmwareSelectors, ParsedFirmwareGdt, parse_firmware_gdt},
    guest::continuation::{NativeBootstrapAck, NativeContinuationError},
    host::descriptors::HostTablePointer,
    memory::address::{AddressPolicy, EncryptionState},
    svm::vmcb::Vmcb,
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap()
}

fn table(base: u64, limit: u16) -> TableSnapshot {
    let mut value = TableSnapshot::default();
    value.bytes[..2].copy_from_slice(&limit.to_le_bytes());
    value.bytes[2..].copy_from_slice(&base.to_le_bytes());
    value
}

// Test-only stand-in for captured VMSAVE/exit fields, never hardware evidence.
fn put(vmcb: &mut Vmcb, offset: usize, bytes: &[u8]) {
    assert!(offset + bytes.len() <= 4096);
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            bytes.len(),
        );
    }
}

fn word(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}

struct Fixture {
    boundary: Box<NativeBoundary>,
    gdt: [u8; 40],
    auxiliary: Vmcb,
}

impl Fixture {
    fn new() -> Self {
        // Box keeps the semantic record address stable while the fixture moves.
        // The surrounding numeric stack span is never dereferenced in this test.
        let mut boundary: Box<NativeBoundary> = Box::new(unsafe { core::mem::zeroed() });
        boundary.abi_version = ABI_VERSION;
        boundary.xstate_size = 512;
        boundary.cr0 = 0x8001_0033;
        boundary.cr2 = 0xffff_8000_0123_4567;
        boundary.cr3 = 0x0054_3018;
        boundary.cr4 = 0x620;
        boundary.cr8 = 7;
        boundary.efer = 0xd01;
        boundary.gdtr = table(0x8000, 39);
        boundary.idtr = table(0x9000, 4095);
        boundary.cs = 8;
        boundary.ss = 16;
        boundary.ds = 16;
        boundary.es = 16;
        boundary.fs = 16;
        boundary.tr = 24;
        boundary.rflags = 0x0020_0603; // IF, DF, ID and carry are all retained.
        boundary.entry_rsp = (&*boundary as *const NativeBoundary as u64) + 1592;
        boundary.entry_rip = 0xffff_8000_0010_0000;
        for (index, value) in boundary.gprs.iter_mut().enumerate() {
            *value = 0xa1b2_c3d4_5566_0000 | index as u64;
        }
        let descriptors = GuestDescriptorRequest {
            gdt_base: 0x8000,
            tss_base: 0xa000,
            rsp0: 0xb000,
            ist1: 0xc000,
        }
        .validate()
        .unwrap();
        let mut auxiliary = Vmcb::new();
        put(&mut auxiliary, 0x440, &16u16.to_le_bytes());
        put(&mut auxiliary, 0x442, &0xc93u16.to_le_bytes());
        put(&mut auxiliary, 0x444, &u32::MAX.to_le_bytes());
        put(&mut auxiliary, 0x448, &0x1234_5000u64.to_le_bytes());
        put(&mut auxiliary, 0x458, &0x1234_6000u64.to_le_bytes());
        put(&mut auxiliary, 0x490, &24u16.to_le_bytes());
        put(&mut auxiliary, 0x492, &0x8bu16.to_le_bytes());
        put(&mut auxiliary, 0x494, &103u32.to_le_bytes());
        put(&mut auxiliary, 0x498, &0xa000u64.to_le_bytes());
        put(
            &mut auxiliary,
            0x600,
            &0x001b_0008_0000_0000u64.to_le_bytes(),
        );
        Self {
            boundary,
            gdt: *descriptors.gdt(),
            auxiliary,
        }
    }

    fn parsed(&self) -> ParsedFirmwareGdt<'_> {
        parse_firmware_gdt(
            HostTablePointer {
                base: 0x8000,
                limit: 39,
            },
            FirmwareSelectors {
                cs: 8,
                ss: 16,
                ds: 16,
                es: 16,
            },
            &self.gdt,
        )
        .unwrap()
    }

    fn request<'a>(&'a self, gdt: &'a ParsedFirmwareGdt<'a>) -> CallbackRequest<'a> {
        let base = (&*self.boundary as *const NativeBoundary as u64) - 64;
        CallbackRequest {
            boundary: &self.boundary,
            efer: svmvisor_hypervisor::svm::dispatch::NativeEfer::admit(0xd01, true).unwrap(),
            gdt,
            auxiliary: &self.auxiliary,
            dr6: 0xffff0ff0,
            dr7: 0x400,
            pat: 0x0007_0406_0007_0406,
            stack: GuestStackSpan {
                base,
                bytes: self.boundary.entry_rsp + 40 - base,
            },
            sites: CallbackSites {
                resume: 0x120000,
                ack: 0x120005,
                after_ack: 0x120008,
            },
        }
    }
}

fn unchanged_refusal(request: CallbackRequest<'_>, expected: CallbackError) {
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x300, &[0x52; 32]);
    let before = *vmcb.bytes();
    let mut frame = GuestRegisters {
        rcx: 0xdeadbeef,
        r15: 0xabcdef,
        ..Default::default()
    };
    let original_frame = frame;
    assert_eq!(
        prepare_callback(request, &policy(), &mut vmcb, &mut frame),
        Err(expected)
    );
    assert_eq!(vmcb.bytes(), &before);
    assert_eq!(frame, original_frame);
}

#[test]
fn callback_recipe_retains_actual_cr3_registers_and_original_return_state() {
    let f = Fixture::new();
    let gdt = f.parsed();
    let original_gprs = f.boundary.gprs;
    let original_auxiliary = *f.auxiliary.bytes();
    let mut vmcb = Vmcb::new();
    let mut frame = GuestRegisters::default();
    let prepared = prepare_callback(f.request(&gdt), &policy(), &mut vmcb, &mut frame).unwrap();
    assert_eq!(word(&vmcb, 0x550), f.boundary.cr3);
    assert_eq!(word(&vmcb, 0x4d0), f.boundary.efer | 0x1000);
    assert_eq!(word(&vmcb, 0x640), f.boundary.cr2);
    assert_eq!(word(&vmcb, 0x5f8), original_gprs[0]);
    assert_eq!(word(&vmcb, 0x570), f.boundary.rflags & !0x600);
    assert_eq!(word(&vmcb, 0x5d8), prepared.boundary_va - 64);
    assert_eq!(
        frame,
        GuestRegisters {
            rcx: original_gprs[1],
            rdx: original_gprs[2],
            rbx: original_gprs[3],
            rbp: original_gprs[4],
            rsi: original_gprs[5],
            rdi: original_gprs[6],
            r8: original_gprs[7],
            r9: original_gprs[8],
            r10: original_gprs[9],
            r11: original_gprs[10],
            r12: original_gprs[11],
            r13: original_gprs[12],
            r14: prepared.boundary_va,
            r15: f.boundary.entry_rsp - 128,
        }
    );
    assert_eq!(prepared.original_return_rip, f.boundary.entry_rip);
    assert_eq!(prepared.original_entry_rsp, f.boundary.entry_rsp);
    assert_eq!(prepared.original_rflags, f.boundary.rflags);
    assert_eq!(f.boundary.gprs, original_gprs);
    assert_eq!(f.auxiliary.bytes(), &original_auxiliary);
    assert!(
        !NativeBootstrapAck::new(
            &vmcb,
            &frame,
            prepared.sites.resume,
            prepared.sites.ack,
            prepared.sites.after_ack
        )
        .unwrap()
        .acknowledged()
    );
}

#[test]
fn every_efi_alignment_phase_uses_the_actual_aligned_boundary() {
    let mut f = Fixture::new();
    for delta in [1544, 1560, 1576, 1592] {
        f.boundary.entry_rsp = (&*f.boundary as *const NativeBoundary as u64) + delta;
        let gdt = f.parsed();
        let mut vmcb = Vmcb::new();
        let mut frame = GuestRegisters::default();
        let output = prepare_callback(f.request(&gdt), &policy(), &mut vmcb, &mut frame).unwrap();
        assert_eq!(output.saved_register_frame, f.boundary.entry_rsp - 128);
    }
}

#[test]
fn stale_stack_record_and_missing_frame_or_shadow_bytes_refuse_transactionally() {
    let mut f = Fixture::new();
    f.boundary.entry_rsp += 64;
    let gdt = f.parsed();
    unchanged_refusal(f.request(&gdt), CallbackError::StackRecipe);
    drop(gdt);
    f.boundary.entry_rsp -= 64;
    let gdt = f.parsed();
    let mut request = f.request(&gdt);
    request.stack.base += 1;
    unchanged_refusal(request, CallbackError::StackSpan);
    let mut request = f.request(&gdt);
    request.stack.bytes -= 1;
    unchanged_refusal(request, CallbackError::StackSpan);
    let mut request = f.request(&gdt);
    request.stack.bytes = u64::MAX;
    unchanged_refusal(request, CallbackError::StackSpan);
}

#[test]
fn wrong_gdt_or_stale_auxiliary_selectors_are_not_substituted() {
    let mut f = Fixture::new();
    let gdt = parse_firmware_gdt(
        HostTablePointer {
            base: 0x18000,
            limit: 39,
        },
        FirmwareSelectors {
            cs: 8,
            ss: 16,
            ds: 16,
            es: 16,
        },
        &f.gdt,
    )
    .unwrap();
    unchanged_refusal(f.request(&gdt), CallbackError::GdtMismatch);
    drop(gdt);
    let gdt = parse_firmware_gdt(
        HostTablePointer {
            base: 0x8000,
            limit: 39,
        },
        FirmwareSelectors {
            cs: 8,
            ss: 16,
            ds: 0,
            es: 16,
        },
        &f.gdt,
    )
    .unwrap();
    unchanged_refusal(f.request(&gdt), CallbackError::GdtMismatch);
    drop(gdt);
    put(&mut f.auxiliary, 0x440, &0u16.to_le_bytes());
    let gdt = f.parsed();
    unchanged_refusal(f.request(&gdt), CallbackError::AuxiliarySelectorMismatch);
}

#[test]
fn invalid_original_flags_and_unsupported_controls_cannot_be_normalized() {
    let mut f = Fixture::new();
    for flags in [0, 0x102, 0x3002, 0x10002, 0x20002, 0x40002] {
        f.boundary.rflags = flags;
        let gdt = f.parsed();
        unchanged_refusal(f.request(&gdt), CallbackError::OriginalFlags);
    }
    f.boundary.rflags = 0x603;
    f.boundary.cr3 |= 1;
    let gdt = f.parsed();
    unchanged_refusal(
        f.request(&gdt),
        CallbackError::Native(NativeContinuationError::UnsupportedCr3),
    );
}

#[test]
fn malformed_boundary_return_pointer_and_linked_site_recipe_refuse() {
    let mut f = Fixture::new();
    f.boundary.abi_version = 0;
    let gdt = f.parsed();
    unchanged_refusal(f.request(&gdt), CallbackError::BoundaryShape);
    drop(gdt);
    f.boundary.abi_version = ABI_VERSION;
    f.boundary.entry_rip = 0x0000_8000_0000_0000;
    let gdt = f.parsed();
    unchanged_refusal(f.request(&gdt), CallbackError::ReturnAddress);
    drop(gdt);
    f.boundary.entry_rip = 0x100000;
    let gdt = f.parsed();
    for sites in [
        CallbackSites {
            resume: 0x120000,
            ack: 0x120006,
            after_ack: 0x120009,
        },
        CallbackSites {
            resume: 0x120000,
            ack: 0x120005,
            after_ack: 0x120009,
        },
        CallbackSites {
            resume: u64::MAX - 4,
            ack: 0,
            after_ack: 3,
        },
    ] {
        let mut request = f.request(&gdt);
        request.sites = sites;
        unchanged_refusal(request, CallbackError::LinkedSites);
    }
}

#[test]
fn destination_event_refusal_keeps_whole_vmcb_and_frame_unchanged() {
    let f = Fixture::new();
    let gdt = f.parsed();
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x0a8, &0x8000_0006u64.to_le_bytes());
    let before = *vmcb.bytes();
    let mut frame = GuestRegisters {
        r12: 0xabba,
        ..Default::default()
    };
    let original_frame = frame;
    assert_eq!(
        prepare_callback(f.request(&gdt), &policy(), &mut vmcb, &mut frame),
        Err(CallbackError::Native(
            NativeContinuationError::DestinationEventState
        ))
    );
    assert_eq!(vmcb.bytes(), &before);
    assert_eq!(frame, original_frame);
}

#[test]
fn callback_requires_matching_feature_admission_and_preserves_tce_aibrse() {
    use svmvisor_hypervisor::svm::dispatch::NativeEfer;
    let mut f = Fixture::new();
    f.boundary.efer |= (1 << 15) | (1 << 21);
    let gdt = f.parsed();
    unchanged_refusal(f.request(&gdt), CallbackError::Native(svmvisor_hypervisor::guest::continuation::NativeContinuationError::UnsupportedEfer));
    let mut request = f.request(&gdt);
    request.efer = NativeEfer::admit_native(f.boundary.efer, 1 << 17, (1 << 11) | (1 << 20) | (1 << 29), 0, Some(1 << 8)).unwrap();
    let mut vmcb = Vmcb::new();
    let mut frame = GuestRegisters::default();
    prepare_callback(request, &policy(), &mut vmcb, &mut frame).unwrap();
    assert_eq!(u64::from_le_bytes(vmcb.bytes()[0x4d0..0x4d8].try_into().unwrap()), f.boundary.efer | (1 << 12));
}
