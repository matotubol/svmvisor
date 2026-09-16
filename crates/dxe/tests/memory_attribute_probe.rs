#![cfg(feature = "memory-attribute-probe")]

use std::mem::{align_of, offset_of, size_of};
use svmvisor_dxe::memory_attributes::probe::*;
use uefi_raw::Status;

fn profile() -> ProbeProfile {
    ProbeProfile {
        root: 0x1000,
        physical_bits: 48,
        nxe: true,
        page1gb: true,
        bsp_apic_id: 0x1234,
    }
}

fn fixture() -> (ProbeFrame, SystemContextX64) {
    let frame = ProbeFrame {
        source: 0x4ff8,
        root: 0x1000,
        bsp_apic_id: 0x1234,
        cookie: 0xabcdef,
        expected_rsp: 0x100008,
        pre_cr2: 0xdead0000,
        pre_cr4: (1 << 5) | (1 << 9),
        pre_cr0: (1 << 31) | (1 << 16) | 1,
        pre_rflags: 2 | (1 << 6),
        physical_bits: 48,
        fault_rip: 0x10000000,
        failure_rip: 0x10000040,
        value: 0x55,
        pre_cs: 0x38,
        pre_ss: 0x30,
        slot_address: 0x200000,
        status: 0,
    };
    // All fields are integers/byte arrays, so all-zero is a valid host fixture.
    let mut context: SystemContextX64 = unsafe { std::mem::zeroed() };
    context.exception_data = 0;
    context.cr0 = frame.pre_cr0;
    context.cr2 = frame.source;
    context.cr3 = frame.root;
    context.cr4 = frame.pre_cr4 | F7_DISPATCH_CR4_OR;
    context.rflags = frame.pre_rflags | (1 << 16);
    context.rip = frame.fault_rip;
    context.cs = frame.pre_cs;
    context.ss = frame.pre_ss;
    context.rsp = frame.expected_rsp;
    context.rcx = frame.slot_address;
    context.rax = 0x9999;
    context.r10 = frame.cookie;
    context.r11 = frame.source;
    context.rbx = 0x11223344;
    context.fx_save_state.xmm[7] = [0x5a; 16];
    (frame, context)
}

fn bytes(context: &SystemContextX64) -> &[u8] {
    // The pinned structure contains only initialized integers/arrays and no padding.
    unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(context).cast(),
            size_of::<SystemContextX64>(),
        )
    }
}

#[test]
fn exact_pinned_abi_layout() {
    assert_eq!(size_of::<FxSaveStateX64>(), 512);
    assert_eq!(size_of::<SystemContextX64>(), 0x358);
    assert_eq!(align_of::<SystemContextX64>(), 8);
    for (actual, expected) in [
        (offset_of!(SystemContextX64, exception_data), 0),
        (offset_of!(SystemContextX64, fx_save_state), 8),
        (offset_of!(SystemContextX64, dr0), 0x208),
        (offset_of!(SystemContextX64, cr2), 0x248),
        (offset_of!(SystemContextX64, cr3), 0x250),
        (offset_of!(SystemContextX64, cr4), 0x258),
        (offset_of!(SystemContextX64, rip), 0x2a0),
        (offset_of!(SystemContextX64, cs), 0x2c8),
        (offset_of!(SystemContextX64, rsp), 0x2f0),
        (offset_of!(SystemContextX64, rcx), 0x308),
        (offset_of!(SystemContextX64, rax), 0x310),
        (offset_of!(SystemContextX64, r10), 0x328),
        (offset_of!(SystemContextX64, r11), 0x330),
        (offset_of!(SystemContextX64, r15), 0x350),
    ] {
        assert_eq!(actual, expected);
    }
    assert_eq!(size_of::<CpuArchProtocol>(), 0x48);
    assert_eq!(
        offset_of!(CpuArchProtocol, register_interrupt_handler),
        0x28
    );
    assert_eq!(offset_of!(CpuArchProtocol, set_memory_attributes), 0x38);
    assert_eq!(size_of::<ProbeFrame>(), 136);
}

#[test]
fn exact_fault_recovers_only_rip_rax_cr2_cr4() {
    let (frame, mut context) = fixture();
    let mut expected = context;
    expected.rip = frame.failure_rip;
    expected.rax = 0;
    expected.cr2 = frame.pre_cr2;
    expected.cr4 = frame.pre_cr4;
    assert!(recover_fault(true, 14, 0x1234, &frame, &mut context));
    assert_eq!(bytes(&context), bytes(&expected));
    assert_eq!(context.cr4 & (1 << 3), 0, "F7 forced DE must be undone");
    assert_ne!(
        context.cr2, frame.source,
        "fault CR2 must not leak into caller state"
    );
}

#[test]
fn unarmed_wrong_vector_and_full_cpu_id_are_never_redirected() {
    let (frame, context) = fixture();
    for (armed, vector, cpu) in [
        (false, 14, 0x1234),
        (true, 13, 0x1234),
        (true, -1, 0x1234),
        (true, 14, 0x34),
        (true, 14, 0x2234),
    ] {
        let mut changed = context;
        assert!(!recover_fault(armed, vector, cpu, &frame, &mut changed));
        assert_eq!(bytes(&changed), bytes(&context));
    }
}

#[test]
fn every_error_code_bit_and_reserved_translation_is_rejected() {
    let (frame, context) = fixture();
    for error in (0..64)
        .map(|bit| 1u64 << bit)
        .chain([3, 5, 9, 16, 32, 64, u64::MAX])
    {
        let mut changed = context;
        changed.exception_data = error;
        let original = changed;
        assert!(
            !recover_fault(true, 14, 0x1234, &frame, &mut changed),
            "code {error:#x}"
        );
        assert_eq!(bytes(&changed), bytes(&original));
    }
}

#[test]
fn each_saved_fault_discriminator_must_match() {
    let (frame, context) = fixture();
    let mutations: &[fn(&mut SystemContextX64)] = &[
        |c| c.rip += 1,
        |c| c.cr2 += 8,
        |c| c.cr3 += 0x1000,
        |c| c.cr0 ^= 1 << 16,
        |c| c.cr4 ^= 1 << 20,
        |c| c.cr4 &= !(1 << 3),
        |c| c.cs |= 3,
        |c| c.ss |= 3,
        |c| c.rsp += 8,
        |c| c.rcx += 64,
        |c| c.r10 += 1,
        |c| c.r11 += 8,
        |c| c.rflags ^= 1 << 6,
    ];
    for mutate in mutations {
        let mut changed = context;
        mutate(&mut changed);
        let original = changed;
        assert!(!recover_fault(true, 14, 0x1234, &frame, &mut changed));
        assert_eq!(bytes(&changed), bytes(&original));
    }
}

#[test]
fn malformed_expectations_cannot_make_a_fault_match() {
    let (frame, context) = fixture();
    let mutations: &[fn(&mut ProbeFrame)] = &[
        |f| f.source += 1,
        |f| f.source = u64::MAX,
        |f| f.source = 1 << 47,
        |f| f.physical_bits = 31,
        |f| f.physical_bits = 64,
        |f| f.root += 1,
        |f| f.cookie = 0,
        |f| f.slot_address = 0,
        |f| f.expected_rsp = 0,
        |f| f.fault_rip = 0,
        |f| f.failure_rip = f.fault_rip,
        |f| f.failure_rip = 1 << 47,
        |f| f.pre_rflags |= 1 << 9,
        |f| f.pre_cr0 &= !(1 << 16),
        |f| f.pre_cr0 |= 1 << 3,
        |f| f.pre_cr4 |= 1 << 21,
        |f| f.pre_cr4 |= 1 << 22,
        |f| f.pre_cr4 |= 1 << 23,
        |f| f.pre_cr4 |= 1 << 24,
        |f| f.pre_cr4 |= 1 << 12,
    ];
    for mutate in mutations {
        let mut changed = frame;
        mutate(&mut changed);
        assert!(!matches_fault(true, 14, 0x1234, &changed, &context));
    }
}

#[test]
fn source_metadata_requires_complete_page_and_bounded_valid_extents() {
    let extents = [
        RamExtent {
            base: 0x1000,
            length: 0x1000,
        },
        RamExtent {
            base: 0x4000,
            length: 0x1000,
        },
    ];
    assert_eq!(validate_source(profile(), &extents, 0x4000), Ok(()));
    assert_eq!(validate_source(profile(), &extents, 0x4ff8), Ok(()));
    for source in [0x4001, 0x4ffc, 1 << 47, u64::MAX] {
        assert_eq!(
            validate_source(profile(), &extents, source),
            Err(ProbeError::InvalidSource)
        );
    }
    assert_eq!(
        validate_source(profile(), &extents, 0x3000),
        Err(ProbeError::SourceOutsideRam)
    );
    assert_eq!(
        validate_source(profile(), &[], 0x4000),
        Err(ProbeError::Capacity)
    );
    assert_eq!(
        validate_source(profile(), &vec![extents[0]; MAX_RAM_EXTENTS + 1], 0x1000),
        Err(ProbeError::Capacity)
    );
    for invalid in [
        vec![RamExtent {
            base: 0x4000,
            length: 8,
        }],
        vec![RamExtent {
            base: 0x4008,
            length: 4096,
        }],
        vec![RamExtent {
            base: 0x4000,
            length: 0,
        }],
        vec![RamExtent {
            base: 0x4000,
            length: u64::MAX - 0xfff,
        }],
        vec![extents[1], extents[0]],
        vec![extents[0], extents[0]],
    ] {
        assert_eq!(
            validate_source(profile(), &invalid, 0x4000),
            Err(ProbeError::InvalidProfile)
        );
    }
}

#[derive(Default)]
struct Registration {
    install: Option<Status>,
    remove: Option<Status>,
    installs: usize,
    removals: usize,
}

impl HandlerRegistration for Registration {
    fn install(&mut self) -> Status {
        self.installs += 1;
        self.install.unwrap_or(Status::SUCCESS)
    }
    fn remove(&mut self) -> Status {
        self.removals += 1;
        self.remove.unwrap_or(Status::SUCCESS)
    }
}

#[test]
fn registration_busy_never_removes_someone_elses_callback() {
    let mut model = RegistrationMachine::new();
    let mut backend = Registration {
        install: Some(Status::ALREADY_STARTED),
        ..Registration::default()
    };
    assert_eq!(
        model.register(&mut backend),
        Err(ProbeError::Registration(Status::ALREADY_STARTED))
    );
    assert!(!model.owns_handler());
    assert_eq!(model.remove(&mut backend), Err(ProbeError::Busy));
    assert_eq!(backend.removals, 0);
}

#[test]
fn failed_removal_retains_ownership_until_explicit_confirmed_removal() {
    let mut model = RegistrationMachine::new();
    let mut backend = Registration::default();
    assert_eq!(model.register(&mut backend), Ok(()));
    assert!(model.owns_handler());
    assert_eq!(model.register(&mut backend), Err(ProbeError::Busy));
    assert_eq!(backend.installs, 1);
    backend.remove = Some(Status::DEVICE_ERROR);
    assert_eq!(
        model.remove(&mut backend),
        Err(ProbeError::Removal(Status::DEVICE_ERROR))
    );
    assert_eq!(
        model.state(),
        RegistrationState::RemovalFailed(Status::DEVICE_ERROR)
    );
    assert!(model.owns_handler());
    backend.remove = Some(Status::SUCCESS);
    assert_eq!(model.remove(&mut backend), Ok(()));
    assert_eq!(model.state(), RegistrationState::Removed);
    assert!(!model.owns_handler());
    assert_eq!(model.remove(&mut backend), Err(ProbeError::Busy));
    assert_eq!(backend.removals, 2);
}

#[test]
fn warning_install_or_remove_retains_indeterminate_lifetime_without_null_retry() {
    for installing in [true, false] {
        let warning = Status::WARN_UNKNOWN_GLYPH;
        let mut model = RegistrationMachine::new();
        let mut backend = Registration::default();
        if installing {
            backend.install = Some(warning);
            assert_eq!(
                model.register(&mut backend),
                Err(ProbeError::Registration(warning))
            );
        } else {
            assert_eq!(model.register(&mut backend), Ok(()));
            backend.remove = Some(warning);
            assert_eq!(
                model.remove(&mut backend),
                Err(ProbeError::Removal(warning))
            );
        }
        assert_eq!(model.state(), RegistrationState::Indeterminate(warning));
        assert!(model.owns_handler());
        let removals = backend.removals;
        assert_eq!(model.remove(&mut backend), Err(ProbeError::Busy));
        assert_eq!(backend.removals, removals);
    }
}

#[test]
fn assembly_has_one_explicit_source_operand_and_fixed_epilogue() {
    // A Windows checkout may convert the committed LF source to CRLF.
    let source = include_str!("../src/memory_attributes/probe.S").replace("\r\n", "\n");
    assert_eq!(source.matches("mov (%r11), %rax").count(), 1);
    assert!(source.contains("svmvisor_memory_probe_fault_rip:\n    mov (%r11), %rax"));
    assert!(source.contains("svmvisor_memory_probe_failure_rip:"));
    assert!(!source
        .lines()
        .any(|line| line.trim_start().starts_with("call")));
    assert!(source.contains("mov %cr2, %rax"));
    assert!(source.contains("mov %cr4, %rax"));
    assert!(source.contains("svmvisor_memory_probe_fail_stop:\n    cli"));
}
