use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    svm::{
        cpu_model::native_boot_cpuid,
        dispatch::{
            NativeEfer, NativeEferError, NativeMsrOutcome, handle_native_cpuid, handle_native_efer,
        },
        permission_maps::Msrpm,
        vmcb::{InstructionIntercept, Vmcb},
    },
};

// Simulate hardware-saved fields; production exposes no mutable VMCB bytes.
fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn get(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}
fn stopped(write: bool, input: u64) -> (NativeEfer, Vmcb, GuestRegisters) {
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x70, 0x7c);
    put(&mut vmcb, 0x78, u64::from(write));
    put(&mut vmcb, 0x4d0, 0x1500);
    put(&mut vmcb, 0x558, 0x8000_0001);
    put(&mut vmcb, 0x578, 0x2000);
    put(&mut vmcb, 0x570, 0x202);
    put(
        &mut vmcb,
        0x5f8,
        0xface_0000_0000_0000 | input as u32 as u64,
    );
    put(&mut vmcb, 0xc8, u64::MAX); // Never consume an unadmitted/stale nRIP.
    let frame = GuestRegisters {
        rcx: 0xfeed_0000_c000_0080,
        rdx: 0xabcd_0000_0000_0000 | input >> 32,
        rbx: 0xbeef,
        ..GuestRegisters::default()
    };
    (NativeEfer::admit(0x500, true).unwrap(), vmcb, frame)
}

#[test]
fn efer_read_hides_backing_svme_and_zero_extends_exactly() {
    let (mut owner, mut vmcb, mut frame) = stopped(false, 0);
    put(&mut vmcb, 0x60, 7); // Native masking=0: retained V_TPR is inert.
    let original = frame;
    assert_eq!(
        handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x32]),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!(vmcb.guest_rax(), 0x500);
    assert_eq!(frame.rdx, 0);
    assert_eq!(frame.rcx, original.rcx);
    assert_eq!(frame.rbx, original.rbx);
    assert_eq!(vmcb.guest_rip(), 0x2002);
    assert_eq!(get(&vmcb, 0x4d0), 0x1500);
    assert_eq!(get(&vmcb, 0x570), 0x202);
}

#[test]
fn efer_sce_nxe_write_retains_backing_flushes_and_reads_back_logical() {
    let (mut owner, mut vmcb, mut frame) = stopped(true, 0xd01);
    let original = frame;
    let rax = vmcb.guest_rax();
    assert_eq!(
        handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
        Ok(NativeMsrOutcome::Completed)
    );
    assert_eq!(frame, original);
    assert_eq!(vmcb.guest_rax(), rax);
    assert_eq!(owner.logical(), 0xd01);
    assert_eq!(get(&vmcb, 0x4d0), 0x1d01);
    assert_eq!(vmcb.bytes()[0x5c], 1);
    put(&mut vmcb, 0x78, 0);
    handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x32]).unwrap();
    assert_eq!(vmcb.guest_rax(), 0xd01);
    assert_eq!(vmcb.guest_rip(), 0x2004);
}

#[test]
fn architectural_faults_queue_gp_without_completing_or_changing_efer() {
    for (value, cpl) in [(0x500 | (1 << 63), 0), (0x100, 0), (0x400, 0), (0x500, 3)] {
        let (mut owner, mut vmcb, mut frame) = stopped(true, value);
        put(&mut vmcb, 0x4c8, (cpl as u64) << 24);
        let original = frame;
        let rax = vmcb.guest_rax();
        assert_eq!(
            handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
            Ok(NativeMsrOutcome::GeneralProtectionPrepared)
        );
        assert_eq!(frame, original);
        assert_eq!(vmcb.guest_rax(), rax);
        assert_eq!(vmcb.guest_rip(), 0x2000);
        assert_eq!(get(&vmcb, 0x4d0), 0x1500);
        assert_eq!(owner.logical(), 0x500);
        assert_eq!(vmcb.event_injection(), 0x8000_0b0d);
    }
}

#[test]
fn unsupported_efer_controls_remain_refusals_not_fabricated_faults() {
    for value in [0x1500, 0x4500, 0x8500, 0x200500, 0x502] {
        let (mut owner, mut vmcb, mut frame) = stopped(true, value);
        let original = *vmcb.bytes();
        let original_frame = frame;
        assert_eq!(
            handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
            Err(NativeEferError::UnsupportedValue { value })
        );
        assert_eq!(*vmcb.bytes(), original);
        assert_eq!(frame, original_frame);
        assert_eq!(owner.logical(), 0x500);
    }
}

#[test]
fn native_efer_features_follow_each_cpu_feature_and_roundtrip_through_backing() {
    for combination in 0..32u32 {
        let feature_bits = (if combination & 1 != 0 { 1 << 14 } else { 0 })
            | (if combination & 2 != 0 { 1 << 15 } else { 0 })
            | (if combination & 4 != 0 { 1 << 21 } else { 0 })
            | (if combination & 8 != 0 { 1 << 18 } else { 0 })
            | (if combination & 16 != 0 { 1 << 20 } else { 0 });
        let ecx = if combination & 2 != 0 { 1 << 17 } else { 0 };
        let edx = (1 << 29) | (1 << 11) | (1 << 20) | if combination & 1 != 0 { 1 << 25 } else { 0 };
        let leaf8 = if combination & 8 != 0 { 1 << 13 } else { 0 };
        let leaf21 = Some((if combination & 4 != 0 { 1 << 8 } else { 0 }) | (if combination & 16 != 0 { 1 << 7 } else { 0 }));
        let (mut owner, mut vmcb, mut frame) = stopped(true, 0xd01 | feature_bits);
        owner = NativeEfer::admit_native(owner.logical(), ecx, edx, leaf8, leaf21).unwrap();
        let before_frame = frame;
        handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]).unwrap();
        assert_eq!(owner.logical(), 0xd01 | feature_bits);
        assert_eq!(get(&vmcb, 0x4d0), 0x1d01 | feature_bits);
        assert_eq!(frame, before_frame);
        assert_eq!(vmcb.bytes()[0x5c], 1);
        put(&mut vmcb, 0x78, 0);
        handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x32]).unwrap();
        assert_eq!(vmcb.guest_rax(), 0xd01 | feature_bits);
        assert_eq!(frame.rdx, 0);
        assert_eq!(vmcb.guest_rip(), 0x2004);
        assert_eq!(get(&vmcb, 0x4d0) & (1 << 12), 1 << 12);
        // Admission also accepts an already-enabled feature only with evidence.
        assert!(NativeEfer::admit_native(0xd01 | feature_bits, ecx, edx, leaf8, leaf21).is_ok());
    }
}

#[test]
fn processor_absent_efer_features_fault_without_completing_instruction() {
    for bit in [12, 13, 14, 15, 17, 18, 20, 21] {
        for leaf21 in [None, Some(0), Some(1 << 24)] {
            let value = 0x500 | (1 << bit);
            let (mut owner, mut vmcb, mut frame) = stopped(true, value);
            owner = NativeEfer::admit_native(owner.logical(), 0, (1 << 29) | (1 << 11) | (1 << 20), 1 << 20, leaf21).unwrap();
            let saved = (owner, *vmcb.bytes(), frame);
            assert_eq!(handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
                Ok(NativeMsrOutcome::GeneralProtectionPrepared));
            assert_eq!((owner, frame), (saved.0, saved.2));
            assert_eq!(vmcb.guest_rip(), 0x2000);
            assert_eq!(get(&vmcb, 0x4d0), 0x1500);
            assert_eq!(vmcb.event_injection(), 0x8000_0b0d);
            assert!(NativeEfer::admit_native(value, 0, (1 << 29) | (1 << 11) | (1 << 20), 1 << 20, leaf21).is_err());
        }
    }
    assert!(NativeEfer::admit_native(0xd01, 0, 0, 0, None).is_err());
}

#[test]
fn fast_fxsave_enable_is_preserved_until_target_owned_init() {
    let (mut owner, mut vmcb, mut frame) = stopped(true, 0x4500);
    put(&mut vmcb, 0x410, 0x29b << 16); // CS.L=1 for the startup-aware owner.
    owner = NativeEfer::admit_native(owner.logical(), 1 << 17, (1 << 29) | (1 << 11) | (1 << 20) | (1 << 25), 1 << 20, Some(1 << 8)).unwrap();
    owner.enable_guest_startup();
    handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]).unwrap();
    // Idempotent FFXSR preservation remains accepted.
    handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]).unwrap();
    put(&mut vmcb, 0x5f8, 0x500);
    let saved = (owner, *vmcb.bytes(), frame);
    assert_eq!(handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
        Err(NativeEferError::UnsupportedValue { value: 0x500 }));
    assert_eq!((owner, *vmcb.bytes(), frame), saved);
    // This method is used only after the target owner commits VMCB INIT.
    owner.reset_after_init().unwrap();
    assert_eq!(owner.logical(), 0);
}

#[test]
fn native_features_do_not_admit_unowned_efer_controls_or_change_fault_rules() {
    for value in [0x20500, 0x502] {
        let (mut owner, mut vmcb, mut frame) = stopped(true, value);
        owner = NativeEfer::admit_native(owner.logical(), u32::MAX, u32::MAX, u32::MAX, Some(u32::MAX)).unwrap();
        let saved = (owner, *vmcb.bytes(), frame);
        assert_eq!(handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
            Err(NativeEferError::UnsupportedValue { value }));
        assert_eq!((owner, *vmcb.bytes(), frame), saved);
    }
    for value in [0x100, 0x400, 0x1500, 0x2500, 0x8000_0000_0000_0500] {
        let (mut owner, mut vmcb, mut frame) = stopped(true, value);
        owner = NativeEfer::admit_native(owner.logical(), u32::MAX, u32::MAX, u32::MAX, Some(u32::MAX)).unwrap();
        assert_eq!(handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
            Ok(NativeMsrOutcome::GeneralProtectionPrepared));
        assert_eq!(owner.logical(), 0x500);
        assert_eq!(get(&vmcb, 0x4d0), 0x1500);
        assert_eq!(vmcb.guest_rip(), 0x2000);
    }
}

#[test]
fn target_efer_all_bit_positions_have_explicit_completion_fault_or_reserved_policy() {
    // PPR57896 Model44h: SCE/LM/NX/FFXSR, TCE, INTWB, UAIE/AIBRS present;
    // MCOMMIT absent and EferLmsleUnsupported set. No nested virtual SVM.
    for bit in 0..64 {
        let value = 0x500 | (1u64 << bit);
        let (mut owner, mut vmcb, mut frame) = stopped(true, value);
        owner = NativeEfer::admit_native(owner.logical(), 1 << 17,
            (1 << 11) | (1 << 20) | (1 << 25) | (1 << 29),
            (1 << 13) | (1 << 20), Some((1 << 7) | (1 << 8))).unwrap();
        let saved_owner = owner;
        let saved_frame = frame;
        let result = handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]);
        assert_eq!(frame, saved_frame, "bit {bit}");
        if matches!(bit, 0 | 8 | 10 | 11 | 14 | 15 | 18 | 20 | 21) {
            assert_eq!(result, Ok(NativeMsrOutcome::Completed), "bit {bit}");
            assert_eq!(owner.logical(), value);
            assert_eq!(get(&vmcb, 0x4d0), value | (1 << 12));
            assert_eq!(vmcb.guest_rip(), 0x2002);
        } else {
            assert_eq!(owner, saved_owner, "bit {bit}");
            assert_eq!(get(&vmcb, 0x4d0), 0x1500);
            assert_eq!(vmcb.guest_rip(), 0x2000);
            if (1..=7).contains(&bit) {
                // RAZ is not proof of WI. Target says preserve reserved fields.
                assert_eq!(result, Err(NativeEferError::UnsupportedValue { value }));
                assert_eq!(vmcb.event_injection(), 0);
            } else {
                assert_eq!(result, Ok(NativeMsrOutcome::GeneralProtectionPrepared), "bit {bit}");
                assert_eq!(vmcb.event_injection(), 0x8000_0b0d);
            }
        }
    }
}

#[test]
fn stopped_state_and_instruction_refusals_are_transactional() {
    for case in 0..6 {
        let (mut owner, mut vmcb, mut frame) = stopped(true, 0xd01);
        let mut bytes: &[u8] = &[0x0f, 0x30];
        match case {
            0 => bytes = &[0x66, 0x0f, 0x30],
            1 => put(&mut vmcb, 0x4d0, 0x500),
            2 => put(&mut vmcb, 0xa8, 1 << 31),
            3 => put(&mut vmcb, 0x88, 1 << 31),
            4 => put(&mut vmcb, 0x578, 0x7fff_ffff_ffff),
            _ => frame.rcx = 0xc001_0117,
        }
        let original = *vmcb.bytes();
        let original_frame = frame;
        assert!(handle_native_efer(&mut owner, &mut vmcb, &mut frame, bytes).is_err());
        assert_eq!(*vmcb.bytes(), original);
        assert_eq!(frame, original_frame);
        assert_eq!(owner.logical(), 0x500);
    }
}

#[test]
fn msrpm_passes_apic_and_native_msrs_but_protects_monitor_controls() {
    let map = Msrpm::native_boot();
    for (index, base) in [
        (0x1b, 0),
        (0x808, 0),
        (0x830, 0),
        (0x81, 0x800),
        (0x101, 0x800),
    ] {
        let bit = index * 2;
        assert_eq!(map.bytes()[base + bit / 8] & (3 << (bit % 8)), 0);
    }
    assert_eq!(map.bytes()[0x820], 3); // EFER only, neighboring MSRs pass.
    assert_eq!(map.bytes()[0x841], 3); // TSC_RATIO only.
    assert_eq!(map.bytes()[0x1004], 2); // SYS_CFG write only; read/adjacent MSRs pass.
    // Every protected-region slot except the two ordinary OSVW MSRs stays
    // intercepted for both accesses, including neighbors sharing their byte.
    for index in 0xc001_0100u32..=0xc001_01ff {
        let bit = ((index - 0xc001_0000) * 2) as usize;
        let accesses = (map.bytes()[0x1000 + bit / 8] >> (bit % 8)) & 3;
        assert_eq!(accesses, if matches!(index, 0xc001_0140 | 0xc001_0141) { 0 } else { 3 }, "MSR {index:08x}");
    }
    // Advertised native OSVW now has hardware access, not a terminal handler.
    let response = svmvisor_hypervisor::svm::cpu_model::native_boot_cpuid(
        0x8000_0001, [0, 0, 1 << 9, 0], 0,
    );
    assert_eq!(response[2] & (1 << 9), 1 << 9);
    assert!(map.bytes()[0x1800..].iter().all(|&b| b == 0xff));
}

#[test]
fn native_sys_cfg_writes_stop_without_hardware_access_or_guest_completion() {
    use svmvisor_hypervisor::arch::x86_64::msr::SYS_CFG;
    // Include harmless values: this profile refuses the whole write rather
    // than pretending to emulate its other cache/routing controls.
    for value in [0, 0x74_0000, 1 << 23, 1 << 24, 1 << 25, 1 << 26] {
        let (mut owner, mut vmcb, mut frame) = stopped(true, value);
        frame.rcx = u64::from(SYS_CFG);
        let original = *vmcb.bytes();
        let original_frame = frame;
        assert_eq!(
            handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]),
            Err(NativeEferError::UnsupportedMsr {
                index: SYS_CFG,
                write: true
            })
        );
        assert_eq!(*vmcb.bytes(), original);
        assert_eq!(frame, original_frame);
        assert_eq!(owner.logical(), 0x500);
        assert_eq!(vmcb.guest_rip(), 0x2000);
        assert_eq!(vmcb.event_injection(), 0);
    }
}

#[test]
fn native_intercepts_keep_svm_stopped_and_pass_real_boot_instructions() {
    let mut vmcb = Vmcb::new();
    vmcb.set_instruction_intercept(InstructionIntercept::Ioio, true);
    vmcb.set_instruction_intercept(InstructionIntercept::Xsetbv, true);
    vmcb.configure_native_boot_intercepts().unwrap();
    for instruction in [
        InstructionIntercept::Vmrun,
        InstructionIntercept::Vmmcall,
        InstructionIntercept::Vmload,
        InstructionIntercept::Vmsave,
        InstructionIntercept::Stgi,
        InstructionIntercept::Clgi,
        InstructionIntercept::Skinit,
        InstructionIntercept::Invlpga,
        InstructionIntercept::Cpuid,
        InstructionIntercept::Msr,
    ] {
        assert!(vmcb.instruction_intercept(instruction));
    }
    for instruction in [
        InstructionIntercept::Ioio,
        InstructionIntercept::Hlt,
        InstructionIntercept::Xsetbv,
        InstructionIntercept::Rdtsc,
        InstructionIntercept::Rdtscp,
    ] {
        assert!(!vmcb.instruction_intercept(instruction));
    }
    assert_eq!(&vmcb.bytes()[..12], &[0; 12]);
    put(&mut vmcb, 0x60, 1 << 24);
    let original = *vmcb.bytes();
    assert!(vmcb.configure_native_boot_intercepts().is_err());
    assert_eq!(*vmcb.bytes(), original);
}

#[test]
fn native_cpuid_preserves_native_topology_and_follows_guest_osxsave() {
    let native = [0x1234, 0x0101_0800, u32::MAX, 0xfeed];
    assert_eq!(native_boot_cpuid(1, native, 0)[2] & (1 << 27), 0);
    assert_eq!(native_boot_cpuid(1, native, 1 << 18), native);
    assert_eq!(
        native_boot_cpuid(0x8000_0001, native, 0)[2] & ((1 << 2) | (1 << 12)),
        0
    );
    for leaf in [0x8000_000a, 0x8000_001f, 0x8000_0023] {
        assert_eq!(native_boot_cpuid(leaf, native, 0), [0; 4]);
    }
    assert_eq!(native_boot_cpuid(0x8000_001e, native, 0), native);
    let (_, mut vmcb, mut frame) = stopped(false, 1);
    put(&mut vmcb, 0x60, 7);
    put(&mut vmcb, 0x70, 0x72);
    handle_native_cpuid(&mut vmcb, &mut frame, &[0x0f, 0xa2], native).unwrap();
    assert_eq!(vmcb.guest_rip(), 0x2002);
    assert_eq!(vmcb.guest_rax(), native[0] as u64);
    assert_eq!(frame.rbx, native[1] as u64);
    assert_eq!(frame.rcx & (1 << 27), 0);
    assert_eq!(frame.rdx, native[3] as u64);
}

#[test]
fn native_nested_paging_requires_prepared_root_asid_and_clean_control() {
    use svmvisor_hypervisor::memory::address::{AddressPolicy, EncryptionState};
    let policy = AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    let mut vmcb = Vmcb::new();
    let original = *vmcb.bytes();
    assert!(vmcb.enable_native_nested_paging(&policy).is_err());
    assert_eq!(*vmcb.bytes(), original);
    vmcb.set_nested_root(0x1000, &policy).unwrap();
    put(&mut vmcb, 0x58, 1);
    vmcb.enable_native_nested_paging(&policy).unwrap();
    assert_eq!(get(&vmcb, 0x90), 1);
    assert_eq!(vmcb.bytes()[0x5c], 1);
    let original = *vmcb.bytes();
    assert!(vmcb.enable_native_nested_paging(&policy).is_err());
    assert_eq!(*vmcb.bytes(), original);
}

#[test]
fn completion_consumes_shadow_rf_but_debug_traps_remain_explicitly_unsupported() {
    let (mut owner, mut vmcb, mut frame) = stopped(false, 0);
    put(&mut vmcb, 0x68, 1);
    put(&mut vmcb, 0x570, 0x1_0202);
    handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x32]).unwrap();
    assert_eq!(get(&vmcb, 0x68), 0);
    assert_eq!(get(&vmcb, 0x570), 0x202);
    put(&mut vmcb, 0x570, 0x302);
    let original = *vmcb.bytes();
    let original_frame = frame;
    assert_eq!(
        handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x32]),
        Err(NativeEferError::UnsupportedDebugState)
    );
    assert_eq!(*vmcb.bytes(), original);
    assert_eq!(frame, original_frame);
}

#[test]
fn native_handlers_complete_fetched_bytes_with_nrip_zero() {
    let (mut owner, mut vmcb, mut frame) = stopped(false, 0);
    put(&mut vmcb, 0xc8, 0);
    handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x32]).unwrap();
    assert_eq!(vmcb.guest_rip(), 0x2002);
    put(&mut vmcb, 0x78, 1);
    put(&mut vmcb, 0x5f8, 0xd01);
    handle_native_efer(&mut owner, &mut vmcb, &mut frame, &[0x0f, 0x30]).unwrap();
    assert_eq!(vmcb.guest_rip(), 0x2004);
    assert_eq!(owner.logical(), 0xd01);
    put(&mut vmcb, 0x70, 0x72);
    put(&mut vmcb, 0x5f8, 1);
    handle_native_cpuid(&mut vmcb, &mut frame, &[0x0f, 0xa2], [7, 8, 9, 10]).unwrap();
    assert_eq!(vmcb.guest_rip(), 0x2006);
    assert_eq!(vmcb.guest_rax(), 7);
    assert_eq!(frame.rbx, 8);
}

#[test]
fn native_cpuid_prefixes_are_refused_without_consuming_state() {
    for prefix in [0x66, 0x67, 0xf2, 0xf3, 0xf0, 0x48, 0x2e, 0x64] {
        let (_, mut vmcb, mut frame) = stopped(false, 1);
        put(&mut vmcb, 0xc8, 0);
        put(&mut vmcb, 0x70, 0x72);
        let original = *vmcb.bytes();
        let original_frame = frame;
        assert!(handle_native_cpuid(&mut vmcb, &mut frame, &[prefix, 0x0f], [0; 4]).is_err());
        assert_eq!(*vmcb.bytes(), original);
        assert_eq!(frame, original_frame);
    }
}
