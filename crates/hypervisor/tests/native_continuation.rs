use svmvisor_hypervisor::{
    arch::x86_64::{descriptors::GuestDescriptorRequest, registers::GuestRegisters},
    boot::descriptors::{FirmwareSelectors, ParsedFirmwareGdt, parse_firmware_gdt},
    guest::{continuation::*, state::GuestStateRequest},
    host::descriptors::HostTablePointer,
    memory::address::{AddressPolicy, EncryptionState},
    svm::vmcb::{VMCB_BYTES, Vmcb},
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

fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

// Test-only stand-in for hardware VMSAVE or a completed exit. The real API has
// no unchecked mutable byte accessor for production Rust consumers.
fn from_bytes(bytes: &[u8; VMCB_BYTES]) -> Vmcb {
    let mut vmcb = Vmcb::new();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), (&mut vmcb as *mut Vmcb).cast(), VMCB_BYTES);
    }
    vmcb
}

struct Fixture {
    gdt: [u8; 40],
    auxiliary: Vmcb,
    registers: GuestRegisters,
}

impl Fixture {
    fn new() -> Self {
        let descriptors = GuestDescriptorRequest {
            gdt_base: 0xffff_8000_0000_1000,
            tss_base: 0xffff_8000_0000_3000,
            rsp0: 0xffff_8000_0000_5000,
            ist1: 0xffff_8000_0000_6000,
        }
        .validate()
        .unwrap();
        let mut extra = [0xa5; VMCB_BYTES];
        for offset in [0x440, 0x450, 0x470, 0x490] {
            extra[offset..offset + 16].fill(0);
        }
        extra[0x440..0x442].copy_from_slice(&16u16.to_le_bytes());
        extra[0x442..0x444].copy_from_slice(&0xc93u16.to_le_bytes());
        extra[0x444..0x448].copy_from_slice(&u32::MAX.to_le_bytes());
        put(&mut extra, 0x448, 0xffff_8000_0010_0000);
        // Null GS selector with a live independent TLS base is not zeroed.
        put(&mut extra, 0x458, 0xffff_8000_0020_0000);
        extra[0x490..0x492].copy_from_slice(&24u16.to_le_bytes());
        extra[0x492..0x494].copy_from_slice(&0x8bu16.to_le_bytes());
        extra[0x494..0x498].copy_from_slice(&103u32.to_le_bytes());
        put(&mut extra, 0x498, 0xffff_8000_0000_3000);
        for (offset, value) in [
            (0x600, 0x001b_0008_0000_0000),
            (0x608, 0xffff_8000_0030_0000),
            (0x610, 0xffff_8000_0040_0000),
            (0x618, 0x200),
            (0x620, 0xffff_8000_0050_0000),
            (0x628, 8),
            (0x630, 0xffff_8000_0060_0000),
            (0x638, 0xffff_8000_0070_0000),
        ] {
            put(&mut extra, offset, value);
        }
        Self {
            gdt: *descriptors.gdt(),
            auxiliary: from_bytes(&extra),
            registers: GuestRegisters {
                rcx: 0x1122,
                r15: 0xff00_ff00_ee00_ee00,
                ..Default::default()
            },
        }
    }

    fn parsed(&self) -> ParsedFirmwareGdt<'_> {
        parse_firmware_gdt(
            HostTablePointer {
                base: 0xffff_8000_0000_1000,
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

    fn request<'a>(&'a self, gdt: &'a ParsedFirmwareGdt<'a>) -> NativeContinuationRequest<'a> {
        NativeContinuationRequest {
            entry: GuestStateRequest {
                rip: 0xffff_8000_0000_8000,
                rsp: 0xffff_8000_0000_4e00,
                rflags: 0x200003,
                cr0: 0x80010033,
                cr3: 0x0010_0018,
                cr4: 0x40620,
                efer: 0xd01,
                rax: 0x8877_6655_4433_2211,
            },
            registers: &self.registers,
            gdt,
            idtr: HostTablePointer {
                base: 0xffff_8000_0000_9000,
                limit: 4095,
            },
            auxiliary: &self.auxiliary,
            cr2: 0xffff_8000_1234_5678,
            dr6: 0xffff0ff0,
            dr7: 0x400,
            cr8: 9,
            pat: 0x0007_0406_0007_0406,
            xstate_profile: 3,
        }
    }
}

#[test]
fn native_commit_preserves_actual_paging_system_tls_and_unrelated_vmcb_bytes() {
    let f = Fixture::new();
    let gdt = f.parsed();
    let request = f.request(&gdt);
    let original = request.entry;
    let mut before = [0x5a; VMCB_BYTES];
    for (offset, value) in [
        (0x060, 1 << 24),
        (0x068, 0),
        (0x070, 0),
        (0x088, 0),
        (0x090, 1),
        (0x0a8, 0),
        (0x0b8, 0),
    ] {
        put(&mut before, offset, value);
    }
    let mut target = from_bytes(&before);
    let mut frame = GuestRegisters::default();
    prepare_native(request, &policy())
        .unwrap()
        .apply(&mut target, &mut frame)
        .unwrap();
    assert_eq!(frame, f.registers);
    let bytes = target.bytes();
    assert_eq!(word(bytes, 0x550), original.cr3); // Retains PWT+PCD.
    assert_eq!(bytes[0x05c], 1); // Initial monitor paging state requires a flush.
    assert_eq!(word(bytes, 0x4d0), original.efer | 0x1000);
    assert_eq!(word(bytes, 0x570), original.rflags);
    assert_eq!(word(bytes, 0x578), original.rip);
    assert_eq!(word(bytes, 0x5d8), original.rsp);
    assert_eq!(word(bytes, 0x5f8), original.rax);
    assert_eq!(word(bytes, 0x640), 0xffff_8000_1234_5678);
    assert_eq!(word(bytes, 0x560), 0x400);
    assert_eq!(word(bytes, 0x568), 0xffff0ff0);
    assert_eq!(word(bytes, 0x668), 0x0007_0406_0007_0406);
    assert_eq!(word(bytes, 0x060), (1 << 24) | 9);
    for (offset, selector, attributes) in [
        (0x400, 16u16, 0xc93u16),
        (0x410, 8, 0xa9b),
        (0x420, 16, 0xc93),
        (0x430, 16, 0xc93),
    ] {
        let mut expected = [0u8; 16];
        expected[0..2].copy_from_slice(&selector.to_le_bytes());
        expected[2..4].copy_from_slice(&attributes.to_le_bytes());
        expected[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(&bytes[offset..offset + 16], &expected);
    }
    for (offset, base, limit) in [
        (0x460, 0xffff_8000_0000_1000, 39u32),
        (0x480, 0xffff_8000_0000_9000, 4095),
    ] {
        let mut expected = [0u8; 16];
        expected[4..8].copy_from_slice(&limit.to_le_bytes());
        put(&mut expected, 8, base);
        assert_eq!(&bytes[offset..offset + 16], &expected);
    }
    for (start, end) in [
        (0x440, 0x460),
        (0x470, 0x480),
        (0x490, 0x4a0),
        (0x600, 0x640),
    ] {
        assert_eq!(&bytes[start..end], &f.auxiliary.bytes()[start..end]);
    }
    let changed = [
        (0x05c, 0x05d),
        (0x060, 0x068),
        (0x0c0, 0x0c4),
        (0x400, 0x4a0),
        (0x4cb, 0x4cc),
        (0x4d0, 0x4d8),
        (0x548, 0x580),
        (0x5d8, 0x5e0),
        (0x5f8, 0x648),
        (0x668, 0x670),
    ];
    for index in 0..VMCB_BYTES {
        if !changed.iter().any(|&(a, b)| (a..b).contains(&index)) {
            assert_eq!(bytes[index], before[index], "unrelated byte {index:x}");
        }
    }
}

#[test]
fn initial_fsgsbase_and_pcid_preserve_controls_tags_and_hidden_tls_state() {
    let f = Fixture::new();
    let gdt = f.parsed();
    for fsgsbase in [0, 1 << 16] {
        for pcid in [0, 1, 0x18, 0xfff] {
            let mut request = f.request(&gdt);
            request.entry.cr4 |= fsgsbase | (1 << 17);
            request.entry.cr3 = 0x100000 | pcid;
            let original = request.entry;
            let mut target = Vmcb::new();
            let mut frame = GuestRegisters::default();
            prepare_native(request, &policy())
                .unwrap()
                .apply(&mut target, &mut frame)
                .unwrap();
            assert_eq!(word(target.bytes(), 0x548), original.cr4);
            assert_eq!(word(target.bytes(), 0x550), original.cr3);
            assert_eq!(
                &target.bytes()[0x440..0x460],
                &f.auxiliary.bytes()[0x440..0x460]
            );
            assert_eq!(
                word(target.bytes(), 0x620),
                word(f.auxiliary.bytes(), 0x620)
            );
            assert_eq!(frame, f.registers);
        }
    }
    // Enabling FSGSBASE alone cannot reinterpret a nonzero low CR3 bit as PCID.
    let mut request = f.request(&gdt);
    request.entry.cr4 |= 1 << 16;
    request.entry.cr3 = 0x100001;
    assert_eq!(
        prepare_native(request, &policy()).err(),
        Some(NativeContinuationError::UnsupportedCr3)
    );
    // The MOV-to-CR3 no-flush hint cannot appear in the captured register.
    let mut request = f.request(&gdt);
    request.entry.cr4 |= 1 << 17;
    request.entry.cr3 = (1 << 63) | 0x100001;
    assert!(matches!(
        prepare_native(request, &policy()).err(),
        Some(NativeContinuationError::Address(_))
    ));
}

#[test]
fn scalar_refusal_does_not_normalize_source_or_touch_destination() {
    let f = Fixture::new();
    let gdt = f.parsed();
    for (field, bad, expected) in [
        (
            0,
            0x8000_0000_0000,
            NativeContinuationError::NoncanonicalAddress,
        ),
        (
            1,
            0x8000_0000_0000,
            NativeContinuationError::NoncanonicalAddress,
        ),
        (2, 0x202, NativeContinuationError::UnsupportedFlags),
        (3, 0x8001003b, NativeContinuationError::UnsupportedCr0),
        (4, 0x100001, NativeContinuationError::UnsupportedCr3),
        (5, 0x140620, NativeContinuationError::UnsupportedCr4),
        (6, 0x1d01, NativeContinuationError::UnsupportedEfer),
        (7, 0x2000, NativeContinuationError::UnsupportedDebug),
        (8, 16, NativeContinuationError::InvalidCr8),
        (9, 2, NativeContinuationError::InvalidPat),
        (10, 1, NativeContinuationError::UnsupportedXstateProfile),
    ] {
        let mut r = f.request(&gdt);
        match field {
            0 => r.entry.rip = bad,
            1 => r.entry.rsp = bad,
            2 => r.entry.rflags = bad,
            3 => r.entry.cr0 = bad,
            4 => r.entry.cr3 = bad,
            5 => r.entry.cr4 = bad,
            6 => r.entry.efer = bad,
            7 => r.dr7 = bad,
            8 => r.cr8 = bad,
            9 => r.pat = bad,
            _ => r.xstate_profile = bad,
        }
        assert_eq!(prepare_native(r, &policy()).err(), Some(expected));
    }
    assert_eq!(f.registers.rcx, 0x1122);
    assert_eq!(word(f.auxiliary.bytes(), 0x458), 0xffff_8000_0020_0000);
}

#[test]
fn pending_events_and_existing_exit_refuse_transactionally() {
    let f = Fixture::new();
    let gdt = f.parsed();
    for (offset, value) in [
        (0x0a8, 1 << 31),
        (0x088, 1 << 31),
        (0x068, 1),
        (0x070, 0x81),
        (0x060, 1 << 8),
        (0x090, 2),
    ] {
        let mut before = [0; VMCB_BYTES];
        put(&mut before, offset, value);
        let mut target = from_bytes(&before);
        let mut frame = GuestRegisters {
            rcx: 0xdead,
            ..Default::default()
        };
        let frame_before = frame;
        let prepared = prepare_native(f.request(&gdt), &policy()).unwrap();
        assert_eq!(
            prepared.apply(&mut target, &mut frame),
            Err(NativeContinuationError::DestinationEventState)
        );
        assert_eq!(target.bytes(), &before);
        assert_eq!(frame, frame_before);
    }
}

#[test]
fn every_additional_virtualization_control_refuses_without_mutation() {
    let f = Fixture::new();
    let gdt = f.parsed();
    for bit in 0..64 {
        let mut before = [0u8; VMCB_BYTES];
        put(&mut before, 0x0b8, 1u64 << bit);
        let mut target = from_bytes(&before);
        let mut frame = GuestRegisters {
            r15: 0x1234,
            ..Default::default()
        };
        let before_frame = frame;
        assert_eq!(
            prepare_native(f.request(&gdt), &policy())
                .unwrap()
                .apply(&mut target, &mut frame),
            Err(NativeContinuationError::DestinationEventState)
        );
        assert_eq!(target.bytes(), &before);
        assert_eq!(frame, before_frame);
    }
}

#[test]
fn auxiliary_noncanonical_tls_and_system_targets_refuse() {
    for offset in [
        0x448, 0x458, 0x478, 0x498, 0x608, 0x610, 0x620, 0x630, 0x638,
    ] {
        let mut f = Fixture::new();
        let mut bytes = *f.auxiliary.bytes();
        put(&mut bytes, offset, 0x0000_8000_0000_0000);
        f.auxiliary = from_bytes(&bytes);
        let gdt = f.parsed();
        assert_eq!(
            prepare_native(f.request(&gdt), &policy()).err(),
            Some(NativeContinuationError::InvalidAuxiliary)
        );
    }
}

#[test]
fn fx_sse_avx_require_matching_original_osxsave_and_preserve_controls() {
    let f = Fixture::new();
    let gdt = f.parsed();
    for profile in [0, 3, 7] {
        let mut r = f.request(&gdt);
        r.xstate_profile = profile;
        if profile == 0 {
            r.entry.cr4 &= !(1 << 18);
        }
        let expected_cr4 = r.entry.cr4;
        let prepared = prepare_native(r, &policy()).unwrap();
        let mut target = Vmcb::new();
        prepared
            .apply(&mut target, &mut GuestRegisters::default())
            .unwrap();
        assert_eq!(word(target.bytes(), 0x548), expected_cr4);
        let mut bad = f.request(&gdt);
        bad.xstate_profile = profile;
        if profile != 0 {
            bad.entry.cr4 &= !(1 << 18);
        }
        assert_eq!(
            prepare_native(bad, &policy()).err(),
            Some(NativeContinuationError::UnsupportedXstateProfile)
        );
    }
}

#[test]
fn dormant_selector_zero_tr_preserves_hidden_type_without_claiming_tss_execution() {
    let mut f = Fixture::new();
    let mut bytes = *f.auxiliary.bytes();
    bytes[0x490..0x4a0].fill(0);
    bytes[0x492..0x494].copy_from_slice(&0x83u16.to_le_bytes());
    bytes[0x494..0x498].copy_from_slice(&0xffffu32.to_le_bytes());
    f.auxiliary = from_bytes(&bytes);
    let gdt = f.parsed();
    let prepared = prepare_native(f.request(&gdt), &policy()).unwrap();
    let mut target = Vmcb::new();
    prepared
        .apply(&mut target, &mut GuestRegisters::default())
        .unwrap();
    assert_eq!(&target.bytes()[0x490..0x4a0], &bytes[0x490..0x4a0]);
    // No ring transition, task switch, TSS access or event injection is tested.
}

#[test]
fn table_overflow_and_nonflat_descriptor_are_refused() {
    let mut f = Fixture::new();
    {
        let gdt = f.parsed();
        let mut r = f.request(&gdt);
        r.idtr = HostTablePointer {
            base: u64::MAX - 4,
            limit: 15,
        };
        assert_eq!(
            prepare_native(r, &policy()).err(),
            Some(NativeContinuationError::NoncanonicalAddress)
        );
    }
    // Parser accepts a nonzero descriptor base; the narrower native bootstrap
    // policy cannot claim its hidden cache agrees with this nonflat descriptor.
    f.gdt[18] = 1;
    let gdt = f.parsed();
    assert_eq!(
        prepare_native(f.request(&gdt), &policy()).err(),
        Some(NativeContinuationError::InvalidDescriptor)
    );
}

#[test]
fn captured_available_and_busy_64bit_tr_preserve_hidden_state_independent_of_gdt_busy() {
    for attributes in [0x89u16, 0x8b] {
        let mut f = Fixture::new();
        // LTR marks the memory GDT descriptor busy. VMSAVE is the authority
        // for hidden TR; it must not be reconstructed from that memory byte.
        f.gdt[29] = 0x8b;
        let original_gdt = f.gdt;
        let mut captured = *f.auxiliary.bytes();
        captured[0x492..0x494].copy_from_slice(&attributes.to_le_bytes());
        f.auxiliary = from_bytes(&captured);
        let gdt = f.parsed();
        let mut destination = Vmcb::new();
        let mut registers = GuestRegisters::default();
        prepare_native(f.request(&gdt), &policy())
            .unwrap()
            .apply(&mut destination, &mut registers)
            .unwrap();
        assert_eq!(&destination.bytes()[0x490..0x4a0], &captured[0x490..0x4a0]);
        assert_eq!(f.auxiliary.bytes(), &captured);
        assert_eq!(f.gdt, original_gdt);
        assert_eq!(registers, f.registers);
    }
}

#[test]
fn unsupported_tr_hidden_encodings_refuse_without_touching_sources_or_destinations() {
    // Available16-bit TSS, LDT, call gate, code/data S bit, reserved attribute.
    for attributes in [0x81u16, 0x82, 0x84, 0x99, 0x1089] {
        let mut f = Fixture::new();
        let mut captured = *f.auxiliary.bytes();
        captured[0x492..0x494].copy_from_slice(&attributes.to_le_bytes());
        f.auxiliary = from_bytes(&captured);
        let gdt = f.parsed();
        let mut destination = Vmcb::new();
        let before = *destination.bytes();
        let mut registers = GuestRegisters {
            rbx: 0x1234,
            ..Default::default()
        };
        let before_registers = registers;
        let result = prepare_native(f.request(&gdt), &policy())
            .and_then(|prepared| prepared.apply(&mut destination, &mut registers));
        assert_eq!(result, Err(NativeContinuationError::InvalidAuxiliary));
        assert_eq!(destination.bytes(), &before);
        assert_eq!(registers, before_registers);
        assert_eq!(f.auxiliary.bytes(), &captured);
    }
}

#[test]
fn native_efer_token_preserves_admitted_features_and_rejects_missing_or_wrong_evidence() {
    use svmvisor_hypervisor::svm::dispatch::NativeEfer;
    let f = Fixture::new();
    let gdt = f.parsed();
    // These controls do not weaken the boundary's full XMM capture. The token
    // represents their separate execution-owner admission, not a bare mask.
    for features in [1 << 15, 1 << 18, 1 << 20, 1 << 21,
        (1 << 15) | (1 << 18) | (1 << 20) | (1 << 21)] {
        let value = 0xd01 | features;
        let efer = NativeEfer::admit_native(value, 1 << 17, (1 << 11) | (1 << 20) | (1 << 29), 1 << 13, Some((1 << 7) | (1 << 8))).unwrap();
        let mut request = f.request(&gdt);
        request.entry.efer = value;
        assert_eq!(prepare_native(request, &policy()).err(), Some(NativeContinuationError::UnsupportedEfer));
        let mut request = f.request(&gdt);
        request.entry.efer = value;
        let mut vmcb = Vmcb::new();
        let mut frame = GuestRegisters::default();
        prepare_native_with_efer(request, &policy(), &efer).unwrap().apply(&mut vmcb, &mut frame).unwrap();
        assert_eq!(word(vmcb.bytes(), 0x4d0), value | (1 << 12));
        assert_eq!(frame, f.registers);
        assert_eq!(prepare_native_with_efer(f.request(&gdt), &policy(), &efer).err(), Some(NativeContinuationError::UnsupportedEfer));
        assert!(NativeEfer::admit_native(value, 0, (1 << 11) | (1 << 20) | (1 << 29), 0, Some(0)).is_err());
    }
    let mut reset = NativeEfer::admit(0xd01, true).unwrap();
    reset.enable_guest_startup();
    reset.reset_after_init().unwrap();
    let mut request = f.request(&gdt);
    request.entry.efer = 0;
    assert_eq!(prepare_native_with_efer(request, &policy(), &reset).err(), Some(NativeContinuationError::UnsupportedEfer));
}
