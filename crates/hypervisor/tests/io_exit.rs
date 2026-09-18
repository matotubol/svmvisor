use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{
            CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
        },
        registers::GuestRegisters,
    },
    memory::address::EncryptionState,
    svm::{
        dispatch::{
            DispatchError, DispatchOutcome, StopReason, handle_exit, handle_exit_with_instruction,
        },
        exit::{
            ExitAction, ExitSnapshot, IoDecodeError, IoDirection, IoRefusal, IoWidth, ResumeError,
        },
        vmcb::Vmcb,
    },
};

fn capabilities(nrip: bool) -> ValidatedCapabilities {
    CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(1),
        asid_count: Some(16),
        physical_address_bits: Some(48),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption: EncryptionState::Unencrypted { encryption_bit: None },
        optional: OptionalFeatures { nrip_save: nrip, ..OptionalFeatures::default() },
    }
    .validate()
    .unwrap()
}

fn snapshot(info1: u64) -> ExitSnapshot {
    ExitSnapshot { code: 0x7b, info1, info2: 0x1001, rip: 0x1000, nrip: 0x1001 }
}

fn stopped(raw: u64, injection: u64, interrupted: u64) -> (Vmcb, GuestRegisters) {
    let mut vmcb = Vmcb::new();
    // Whole-page canaries catch writes beyond the commonly sampled fields.
    for offset in (0..4096).step_by(8) {
        write(&mut vmcb, offset, 0xa55a_f00d_1234_0000 | offset as u64);
    }
    for (offset, value) in [
        (0x070, 0x7b),
        (0x078, raw),
        (0x080, 0x1001),
        (0x088, interrupted),
        (0x0a8, injection),
        (0x0c8, 0x1001),
        (0x570, 0x247),
        (0x578, 0x1000),
        (0x5d8, 0x8000),
        (0x5f8, 0xfedc_ba98_7654_3210),
    ] {
        write(&mut vmcb, offset, value);
    }
    (
        vmcb,
        GuestRegisters {
            rcx: 0x1234_0000_0000_0001,
            rdx: 0x1234_0000_0000_0002,
            rbx: 0x1234_0000_0000_0003,
            rbp: 0x1234_0000_0000_0004,
            rsi: 0x1234_0000_0000_0005,
            rdi: 0x1234_0000_0000_0006,
            r8: 0x1234_0000_0000_0007,
            r9: 0x1234_0000_0000_0008,
            r10: 0x1234_0000_0000_0009,
            r11: 0x1234_0000_0000_000a,
            r12: 0x1234_0000_0000_000b,
            r13: 0x1234_0000_0000_000c,
            r14: 0x1234_0000_0000_000d,
            r15: 0x1234_0000_0000_000e,
        },
    )
}

// Emulate hardware writes to exclusively owned inert storage. This cannot
// execute a guest; the pointer originates from &mut, not a shared byte view.
fn write(vmcb: &mut Vmcb, offset: usize, value: u64) {
    assert!(offset + 8 <= 4096);
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

#[test]
fn ioio_metadata_preserves_width_direction_port_and_raw_unconsumed_fields() {
    for (size, width, bytes) in
        [(1, IoWidth::Byte, 1), (2, IoWidth::Word, 2), (4, IoWidth::Dword, 4)]
    {
        for (input, direction) in [(0, IoDirection::Out), (1, IoDirection::In)] {
            for port in [0, 7, 8, 0xff, 0x100, 0xfffc, 0xfffd, 0xfffe, 0xffff] {
                for address in [0, 1, 2, 4, 7] {
                    for segment in 0..8 {
                        let raw = 0xfedc_ba98_0000_0000
                            | (port << 16)
                            | (segment << 10)
                            | (address << 7)
                            | (size << 4)
                            | input;
                        let io = snapshot(raw).ioio().unwrap();
                        assert_eq!(io.raw_info(), raw);
                        assert_eq!(io.port(), port as u16);
                        assert_eq!(io.width(), width);
                        assert_eq!(io.width_bytes(), bytes);
                        assert_eq!(io.direction(), direction);
                        assert_eq!(io.input(), input != 0);
                        assert!(!io.string());
                        assert!(!io.rep());
                        assert_eq!(io.address_size_bits(), address as u8);
                        assert_eq!(io.segment_bits(), segment as u8);
                    }
                }
            }
        }
    }
}

#[test]
fn only_ioio_defines_io_metadata_and_malformed_widths_or_reserved_bits_are_refused() {
    for code in [0, 0x72, 0x78, 0x7c, 0x400, 0x1_0000_007b, u64::MAX] {
        assert_eq!(ExitSnapshot { code, ..snapshot(0x10) }.ioio(), Err(IoDecodeError::NotIoioExit));
    }
    for size in [0, 3, 5, 6, 7] {
        let value = snapshot(size << 4);
        assert_eq!(value.ioio(), Err(IoDecodeError::InvalidOperandSize));
        assert_eq!(
            value.action(),
            ExitAction::IoioRefused(IoRefusal::Malformed(IoDecodeError::InvalidOperandSize))
        );
    }
    for bit in [1, 13, 14, 15] {
        let value = snapshot(0x10 | (1 << bit));
        assert_eq!(value.ioio(), Err(IoDecodeError::ReservedBits));
        assert_eq!(
            value.action(),
            ExitAction::IoioRefused(IoRefusal::Malformed(IoDecodeError::ReservedBits))
        );
    }
}

#[test]
fn scalar_port_spans_are_linear_and_unknown_ports_never_gain_an_owner() {
    for size in [1, 2, 4] {
        for port in [0, 7, 8, 0xff, 0x100, 0xfffc, 0xfffd, 0xfffe, 0xffff] {
            let value = snapshot((port << 16) | (size << 4));
            let io = value.ioio().unwrap();
            let last = port + size - 1;
            let refusal = if last > 0xffff {
                assert_eq!(io.last_port(), None);
                IoRefusal::PortSpanOverrun(io)
            } else {
                assert_eq!(io.last_port(), Some(last as u16));
                IoRefusal::UnknownPort(io)
            };
            assert_eq!(value.action(), ExitAction::IoioRefused(refusal));
        }
    }
}

#[test]
fn every_string_or_rep_form_is_explicitly_refused_without_memory_field_admission() {
    for (string, rep) in [(true, false), (false, true), (true, true)] {
        for input in [0, 1] {
            // Include the backend's absent address/segment fields and raw
            // encodings outside any admitted string-memory decode contract.
            for fields in [0, 0x1f80] {
                let value = snapshot(
                    0xffff_0040 | fields | ((string as u64) << 2) | ((rep as u64) << 3) | input,
                );
                let io = value.ioio().unwrap();
                assert_eq!(io.string(), string);
                assert_eq!(io.rep(), rep);
                assert_eq!(value.action(), ExitAction::IoioRefused(IoRefusal::StringOrRep(io)));
            }
        }
    }
}

#[test]
fn refusal_dispatch_preserves_entire_vmcb_gprs_flags_and_pending_events() {
    for raw in [
        0x0080_0010,
        0x0080_0011,
        0x0080_0020,
        0x0080_0041,
        0xffff_0040,
        0x0080_0014,
        0x0080_0018,
        0x0080_001d,
        0x0080_0000,
        0x0080_0030,
        0x0080_0012,
        0x0080_2010,
    ] {
        for injection in [0, 0x8000_0351, 0x1234_5678_8000_0b0e] {
            for interrupted in [0, 0x8000_0352, 0x8765_4321_8000_0b0d] {
                let (mut vmcb, mut frame) = stopped(raw, injection, interrupted);
                let value = vmcb.exit_snapshot();
                let before = *vmcb.bytes();
                let before_frame = frame;
                let expected = Ok(DispatchOutcome::Stop(StopReason::Exit(value.action())));
                for nrip in [false, true] {
                    assert_eq!(
                        handle_exit(value, &mut vmcb, &mut frame, &capabilities(nrip)),
                        expected
                    );
                    assert_eq!(vmcb.bytes(), &before);
                    assert_eq!(frame, before_frame);
                }
                for bytes in [&[][..], &[0xee][..], &[0xed][..], &[0xf3, 0x6f][..]] {
                    assert_eq!(
                        handle_exit_with_instruction(value, &mut vmcb, &mut frame, bytes),
                        expected
                    );
                    assert_eq!(vmcb.bytes(), &before);
                    assert_eq!(frame, before_frame);
                }
            }
        }
    }
}

#[test]
fn ioio_never_offers_nrip_or_instruction_completion_even_with_plausible_following_rip() {
    for info in [0x80_0010, 0x80_0011, 0x80_001c, 0xffff_0040, 0] {
        for following in [0, 0x1001, 0x1002, u64::MAX] {
            let value = ExitSnapshot { info2: following, nrip: following, ..snapshot(info) };
            assert_eq!(
                value.resume_candidate(&capabilities(true)),
                Err(ResumeError::ExitDoesNotPermitCandidate)
            );
            for instruction in [&[0xee][..], &[0xe4, 0x80][..], &[0xf3, 0x6c][..]] {
                assert_eq!(
                    value.resume_candidate_from_instruction(instruction),
                    Err(ResumeError::ExitDoesNotPermitCandidate)
                );
            }
        }
    }
}

#[test]
fn stale_io_snapshot_refusal_preserves_all_stopped_state() {
    let (mut vmcb, mut frame) = stopped(0x80_0011, 0x8000_0351, 0x8000_0352);
    let value = ExitSnapshot { rip: 0x9999, ..vmcb.exit_snapshot() };
    let before = *vmcb.bytes();
    let before_frame = frame;
    assert_eq!(
        handle_exit_with_instruction(value, &mut vmcb, &mut frame, &[0xec]),
        Err(DispatchError::SnapshotRipMismatch)
    );
    assert_eq!(vmcb.bytes(), &before);
    assert_eq!(frame, before_frame);
}
