use std::cell::Cell;

use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{
            CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
        },
        registers::GuestRegisters,
    },
    memory::address::EncryptionState,
    svm::{
        dispatch::{NativeEferError, NativeMsrOutcome},
        syscfg::{self, SyscfgError, SyscfgInstruction, SyscfgPreparation},
        vmcb::Vmcb,
    },
};

fn stopped(value: u64) -> (Vmcb, GuestRegisters) {
    let mut v = Vmcb::new();
    for (offset, value) in [
        (0x70, 0x7c),
        (0x78, 1),
        (0xc8, 0x2002),
        (0x410, 0x29bu64 << 16),
        (0x4d0, 0x1500),
        (0x558, 0x80000001),
        (0x548, 0x20),
        (0x550, 0x1000),
        (0x578, 0x2000),
        (0x570, 0x10002),
        (0x5f8, 0xfeedface00000000 | value as u32 as u64),
    ] {
        put(&mut v, offset, value);
    }
    (
        v,
        GuestRegisters {
            rcx: 0xc0010010,
            rdx: 0xdeadbeef00000000 | value >> 32,
            rbx: 0x1234,
            r15: 0x5678,
            ..Default::default()
        },
    )
}

fn put(v: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (v as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}

fn run(
    v: &mut Vmcb,
    f: &GuestRegisters,
    current: u64,
    hardware: bool,
    writes: &mut Vec<u64>,
) -> Result<NativeMsrOutcome, SyscfgError> {
    let c = caps(true);
    match syscfg::prepare(
        v,
        f,
        if hardware {
            SyscfgInstruction::Hardware(&c)
        } else {
            SyscfgInstruction::Bytes(&[0x0f, 0x30])
        },
        true,
        0xb40f40,
        48,
        || current,
    )? {
        SyscfgPreparation::GeneralProtectionPrepared => {
            Ok(NativeMsrOutcome::GeneralProtectionPrepared)
        }
        SyscfgPreparation::Write(token) => {
            assert_eq!(token.current(), current);
            assert_eq!(token.delta(), token.requested() ^ current);
            if let Some(value) = token.write_value() {
                writes.push(value);
            }
            Ok(token.commit())
        }
    }
}

fn caps(nrip: bool) -> ValidatedCapabilities {
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
        optional: OptionalFeatures { nrip_save: nrip, ..Default::default() },
    }
    .validate()
    .unwrap()
}

#[test]
fn every_requested_bit_is_completed_or_refused_without_partial_state() {
    for hardware in [false, true] {
        for current in [0, 0x740000, 0x780000, 0x7c0000] {
            for value in (0..64).map(|bit| current ^ (1u64 << bit)).chain([current]) {
                let (mut v, f) = stopped(value);
                let before = *v.bytes();
                let mut writes = Vec::new();
                let result = run(&mut v, &f, current, hardware, &mut writes);
                if (value ^ current) & !0xc0000 == 0 {
                    assert_eq!(result, Ok(NativeMsrOutcome::Completed));
                    assert_eq!(v.guest_rip(), 0x2002);
                    assert_eq!(u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap()), 2);
                    assert_eq!(writes, if value == current { vec![] } else { vec![value] });
                    assert_eq!(v.guest_rax(), 0xfeedface00000000 | value as u32 as u64);
                } else {
                    assert!(result.is_err());
                    assert_eq!(writes.len(), 0);
                    assert_eq!(*v.bytes(), before);
                    let expected = if value & !0x07fc0000 != 0 {
                        SyscfgError::RequestedReserved { requested: value, current }
                    } else {
                        SyscfgError::UnsupportedChange { requested: value, current }
                    };
                    assert_eq!(result, Err(expected));
                }
            }
        }
    }
}

#[test]
fn binary_ninja_windows_fixed_mtrr_sequences_complete_all_syscfg_writes() {
    // Exact KiReadFixedMtrr / KiWriteFixedMtrr masks, including alternate
    // AMD branch. The eleven intervening fixed MSR accesses remain native.
    for initial in [0x740000, 0x780000, 0x7c0000] {
        for sequence in [
            vec![initial | 0x80000, initial & !0x80000],
            vec![(initial & !0x40000) | 0x80000, (initial | 0x40000) & !0x80000],
            vec![initial & !0xc0000],
        ] {
            let mut current = initial;
            for requested in sequence {
                let (mut v, f) = stopped(requested);
                // KeLoadMTRR sets CD while MTRR_DEF_TYPE.E is clear. NRIP
                // completion requires no instruction read or cache type claim.
                put(&mut v, 0x558, 0xc0000001);
                let mut writes = Vec::new();
                assert_eq!(
                    run(&mut v, &f, current, true, &mut writes),
                    Ok(NativeMsrOutcome::Completed)
                );
                current = requested;
            }
        }
    }
}

#[test]
fn dropped_preparation_and_all_current_invariant_failures_preserve_guest() {
    let (mut v, f) = stopped(0x7c0000);
    let before = *v.bytes();
    let _ = syscfg::prepare(
        &mut v,
        &f,
        SyscfgInstruction::Bytes(&[0x0f, 0x30]),
        true,
        0xb40f40,
        48,
        || 0x740000,
    )
    .unwrap();
    assert_eq!(*v.bytes(), before);
    for current in [1, 1 << 63, 0x800000, 0x1000000, 0x2000000, 0x4000000] {
        let mut writes = Vec::new();
        let result = run(&mut v, &f, current, true, &mut writes);
        assert_eq!(
            result,
            Err(if current & !0x07fc0000 != 0 {
                SyscfgError::CurrentReserved { requested: 0x7c0000, current }
            } else {
                SyscfgError::CurrentEncryption { requested: 0x7c0000, current }
            })
        );
        assert_eq!(*v.bytes(), before);
        assert!(writes.is_empty());
    }
}

#[test]
fn boundary_errors_and_unsupported_profiles_never_read_hardware() {
    for case in 0..24 {
        let (mut v, mut f) = stopped(0x7c0000);
        let mut nrip = true;
        let mut signature = 0xb40f40;
        let mut width = 48;
        match case {
            0 => nrip = false,
            1 => put(&mut v, 0xc8, 0),
            2 => put(&mut v, 0xc8, 0x2001),
            3 => put(&mut v, 0xc8, 0x2010),
            4 => put(&mut v, 0xc8, 0x1fff),
            5 => put(&mut v, 0xc8, 0x800000000000),
            6 => put(&mut v, 0x70, 0x72),
            7 => put(&mut v, 0x78, 2),
            8 => put(&mut v, 0x4c8, 3 << 24),
            9 => put(&mut v, 0x410, 0x9b << 16),
            10 => put(&mut v, 0x558, 1),
            11 => put(&mut v, 0x548, 0),
            12 => put(&mut v, 0x548, 0x1020),
            13 => put(&mut v, 0x570, 0x102),
            14 => put(&mut v, 0xa8, 1 << 31),
            15 => put(&mut v, 0x88, 1 << 31),
            16 => put(&mut v, 0x60, 1 << 25),
            17 => put(&mut v, 0x578, 0x800000000000),
            18 => f.rcx = 0xc0000080,
            19 => put(&mut v, 0x78, 0),
            20 => signature = 0,
            21 => width = 52,
            22 => put(&mut v, 0x570, 0x20002),
            _ => put(&mut v, 0x410, 0x69b << 16),
        }
        let before = *v.bytes();
        let reads = Cell::new(0);
        let c = caps(nrip);
        assert!(
            syscfg::prepare(
                &mut v,
                &f,
                SyscfgInstruction::Hardware(&c),
                true,
                signature,
                width,
                || {
                    reads.set(reads.get() + 1);
                    0x740000
                }
            )
            .is_err(),
            "case{case}"
        );
        assert_eq!(reads.get(), 0, "case{case}");
        assert_eq!(*v.bytes(), before, "case{case}");
    }
}

#[test]
fn byte_privilege_fault_preserves_operands_and_never_reads_physical_syscfg() {
    let (mut v, f) = stopped(0x7c0000);
    put(&mut v, 0x4c8, 3 << 24);
    let old = (
        v.guest_rip(),
        v.guest_rax(),
        u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap()),
    );
    let result = syscfg::prepare(
        &mut v,
        &f,
        SyscfgInstruction::Bytes(&[0x0f, 0x30]),
        true,
        0xb40f40,
        48,
        || panic!("CPL fault must precede hardware read"),
    );
    assert!(matches!(result, Ok(SyscfgPreparation::GeneralProtectionPrepared)));
    drop(result);
    assert_eq!(
        (
            v.guest_rip(),
            v.guest_rax(),
            u64::from_le_bytes(v.bytes()[0x570..0x578].try_into().unwrap())
        ),
        old
    );
    assert_eq!(u64::from_le_bytes(v.bytes()[0xa8..0xb0].try_into().unwrap()), 0x80000b0d);
}

#[test]
fn hardware_lengths_and_legacy_byte_modes_share_completion_contract() {
    for length in 2..=15 {
        let (mut v, f) = stopped(0x7c0000);
        put(&mut v, 0xc8, 0x2000 + length);
        assert_eq!(run(&mut v, &f, 0x740000, true, &mut vec![]), Ok(NativeMsrOutcome::Completed));
        assert_eq!(v.guest_rip(), 0x2000 + length);
    }
    for protected in [false, true] {
        let (mut v, f) = stopped(0x7c0000);
        put(&mut v, 0x410, 0x9b << 16);
        put(&mut v, 0x418, 0);
        unsafe {
            core::ptr::copy_nonoverlapping(
                0xffffu32.to_le_bytes().as_ptr(),
                (&mut v as *mut Vmcb).cast::<u8>().add(0x414),
                4,
            );
        }
        put(&mut v, 0x558, u64::from(protected));
        put(&mut v, 0x4d0, 0x1000);
        assert_eq!(run(&mut v, &f, 0x740000, false, &mut vec![]), Ok(NativeMsrOutcome::Completed));
    }
    let (mut v, f) = stopped(0x7c0000);
    let before = *v.bytes();
    let error = syscfg::prepare(
        &mut v,
        &f,
        SyscfgInstruction::Bytes(&[0x48, 0x0f, 0x30]),
        true,
        0xb40f40,
        48,
        || panic!("prefix failure before hardware read"),
    );
    assert!(matches!(error, Err(SyscfgError::Boundary(NativeEferError::Instruction(_)))));
    drop(error);
    assert_eq!(*v.bytes(), before);
}
