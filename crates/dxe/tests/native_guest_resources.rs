#![cfg(any(feature = "native-transition-test", feature = "native-returning"))]
#[allow(dead_code)]
#[path = "../src/native/resources/guest.rs"]
mod native_guest_resources;

use native_guest_resources::*;
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use svmvisor_dxe::{
    native::admission::boundary::{self as native_boundary, NativeBoundary},
    native::transition::state::{
        self as native_transition, ScalarState, guest_capture, mode, outcome,
    },
};

struct Storage(*mut u8);
impl Storage {
    fn new() -> Self {
        let layout = Layout::from_size_align(ARENA_BYTES, 4096).unwrap();
        let pointer = unsafe { alloc(layout) };
        if pointer.is_null() {
            handle_alloc_error(layout);
        }
        unsafe { pointer.write_bytes(0xa5, ARENA_BYTES) };
        Self(pointer)
    }
    fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.0, ARENA_BYTES) }
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        unsafe { dealloc(self.0, Layout::from_size_align(ARENA_BYTES, 4096).unwrap()) };
    }
}

fn inputs() -> ConstructionInputs {
    ConstructionInputs {
        physical_bits: 48,
        max_extended: 0x8000000a,
        extended1_ecx: 1 << 2,
        extended1_edx: 1 << 20,
        svm_edx: 1,
        asid_count: 16,
    }
}
fn boundary(profile: u64) -> NativeBoundary {
    let mut b: NativeBoundary = unsafe { core::mem::zeroed() };
    b.abi_version = native_boundary::ABI_VERSION;
    b.cr0 = 0x80010033;
    b.cr3 = 0x12345000;
    b.cr4 = 0x620;
    b.efer = 0xd00;
    b.profile = profile;
    b.xstate_size = 512;
    b.gdtr.bytes[..2].copy_from_slice(&63u16.to_le_bytes());
    b.gdtr.bytes[2..].copy_from_slice(&0x34560000u64.to_le_bytes());
    b.idtr.bytes[..2].copy_from_slice(&4095u16.to_le_bytes());
    b.idtr.bytes[2..].copy_from_slice(&0x45670000u64.to_le_bytes());
    b.cs = 0x38;
    b.ss = 0x30;
    b.ds = 0x30;
    b.es = 0x30;
    b.tr = 0x40;
    b.leaf1_ebx = 5 << 24;
    if profile != 0 {
        b.captured_fields = native_boundary::CAPTURE_XCR0;
        b.cr4 |= 1 << 18;
        b.leaf1_ecx = 3 << 26;
        b.xcr0 = profile;
        b.supported_xcr0 = 7;
        b.xstate_size = 576;
    }
    if profile == 7 {
        b.leaf1_ecx |= 1 << 28;
        b.avx_offset = 576;
        b.avx_size = 256;
        b.xstate_size = 832;
    }
    b
}

#[cfg(not(feature = "native-transition-event-test"))]
const GOLDEN: &[u8; 30 * 4096] = include_bytes!("data/native_guest_resources_normal.bin");
#[cfg(feature = "native-transition-event-test")]
const GOLDEN: &[u8; 30 * 4096] = include_bytes!("data/native_guest_resources_event.bin");

#[test]
fn exact_frozen_fixture_bytes_for_fx_sse_and_avx_with_separate_virtual_backing() {
    // Golden is the unmodified old initialize method executed at VA=PA=2MiB;
    // see the contract for frozen source hash and host VirtualAlloc generator.
    for profile in [0, 3, 7] {
        let storage = Storage::new();
        let b = boundary(profile);
        let guest = unsafe { initialize(storage.0, 0x200000, &b, inputs()).unwrap() };
        assert_eq!(&storage.bytes()[..30 * 4096], GOLDEN);
        assert!(storage.bytes()[30 * 4096..].iter().all(|byte| *byte == 0));
        let before = storage.bytes().to_vec();
        let gdt = [0u8; 64];
        let bound = unsafe { guest.bind(&b, &gdt, mode::ONE_ENTRY).unwrap() };
        let c = unsafe { &*bound.context() };
        let va = |page: u64| storage.0 as u64 + page * 4096;
        let pa = |page: u64| 0x200000u64 + page * 4096;
        assert_eq!(bound.context() as u64, va(30));
        assert_eq!(bound.canary() as u64, va(32));
        let i = c.inputs;
        assert_eq!((i.abi_version, i.context_bytes), (1, 1088));
        assert_eq!(i.image_boundary_va, &b as *const _ as u64);
        assert_eq!((i.guest_vmcb_pa, i.guest_vmcb_va), (pa(0), va(0)));
        assert_eq!((i.host_extra_pa, i.host_extra_va), (pa(1), va(1)));
        assert_eq!((i.restored_extra_pa, i.restored_extra_va), (pa(2), va(2)));
        assert_eq!((i.guest_extra_pa, i.guest_extra_va), (pa(24), va(24)));
        assert_eq!(i.hsave_pa, pa(3));
        assert_eq!(
            (i.original_xstate_va, i.restored_xstate_va, i.guest_xstate_va),
            (va(4), va(5), va(6))
        );
        assert_eq!((i.xstate_profile, i.xstate_bytes), (profile, b.xstate_size));
        assert_eq!((i.expected_vmmcall_rip, i.expected_vmmcall_rax), (VMMCALL_RIP, COOKIE));
        assert_eq!(i.expected_bsp_apic_id, 5);
        assert_eq!(i.expected_state_va, va(31));
        assert_eq!((i.host_gdt_copy_va, i.host_gdt_bytes), (gdt.as_ptr() as u64, 64));
        assert_eq!(i.mode, mode::ONE_ENTRY);
        let expected = unsafe { &*(i.expected_state_va as *const ScalarState) };
        assert_eq!(expected.captured_fields, native_transition::capture::CORE);
        assert_eq!(
            (expected.cr0, expected.cr3, expected.cr4, expected.efer),
            (b.cr0, b.cr3, b.cr4, b.efer)
        );
        assert_eq!(expected.gdtr.bytes, b.gdtr.bytes);
        assert_eq!(expected.idtr.bytes, b.idtr.bytes);
        assert_eq!(expected.selectors, [b.cs, b.ss, b.ds, b.es, b.fs, b.gs, b.ldtr, b.tr]);
        // Binding is confined to context.inputs and expected scalar data. The
        // canary, journal, and all previously built pages stay initialized.
        for (index, (&old, &new)) in before.iter().zip(storage.bytes()).enumerate() {
            if !(30 * 4096..30 * 4096 + 192).contains(&index)
                && !(31 * 4096..31 * 4096 + 256).contains(&index)
            {
                assert_eq!(old, new, "unexpected bind write at {index:#x}");
            }
        }
    }
}

#[test]
fn refused_numeric_and_cpu_profiles_leave_every_owned_byte_untouched() {
    let storage = Storage::new();
    let b = boundary(0);
    for pa in [0, 0xff000, 0x200001, (1u64 << 32) - ARENA_BYTES as u64 + 4096, u64::MAX] {
        assert!(unsafe { initialize(storage.0, pa, &b, inputs()) }.is_err());
    }
    for change in 0..7 {
        let mut supplied = inputs();
        match change {
            0 => supplied.physical_bits = 31,
            1 => supplied.physical_bits = 53,
            2 => supplied.max_extended = 0x80000009,
            3 => supplied.extended1_ecx = 0,
            4 => supplied.extended1_edx = 0,
            5 => supplied.svm_edx = 0,
            _ => supplied.asid_count = 1,
        }
        assert!(unsafe { initialize(storage.0, 0x200000, &b, supplied) }.is_err());
    }
    for change in 0..8 {
        let mut b = boundary(7);
        match change {
            0 => b.cr0 &= !(1 << 31),
            1 => b.cr4 &= !(1 << 5),
            2 => b.cr4 |= 1 << 12,
            3 => b.efer &= !(1 << 11),
            4 => b.efer &= !(1 << 10),
            5 => b.avx_offset = 640,
            6 => b.profile = 15,
            _ => b.abi_version = 2,
        }
        assert!(unsafe { initialize(storage.0, 0x200000, &b, inputs()) }.is_err());
    }
    assert!(unsafe { initialize(storage.0.add(1), 0x200000, &b, inputs()) }.is_err());
    assert!(storage.bytes().iter().all(|byte| *byte == 0xa5));
}

#[test]
fn binding_rejects_wrong_boundary_gdt_extent_and_mode_without_partial_context() {
    let storage = Storage::new();
    let b = boundary(0);
    for case in 0..4 {
        let guest = unsafe { initialize(storage.0, 0x200000, &b, inputs()).unwrap() };
        let other = boundary(0);
        let gdt = [0u8; 65];
        let result = unsafe {
            match case {
                0 => guest.bind(&other, &gdt[..64], mode::ONE_ENTRY),
                1 => guest.bind(&b, &[], mode::ONE_ENTRY),
                2 => guest.bind(&b, &gdt, mode::ONE_ENTRY),
                _ => guest.bind(&b, &gdt[..64], 2),
            }
        };
        assert!(matches!(result, Err(12)));
        assert!(storage.bytes()[30 * 4096..].iter().all(|byte| *byte == 0));
    }
}

#[test]
fn actual_observation_verifier_rejects_corruption_and_counts_guest_evidence() {
    let storage = Storage::new();
    let b = boundary(7);
    let gdt = [0u8; 64];
    let guest = unsafe {
        initialize(storage.0, 0x200000, &b, inputs())
            .unwrap()
            .bind(&b, &gdt, mode::ONE_ENTRY)
            .unwrap()
    };
    let c = unsafe { &mut *guest.context() };
    c.journal.outcome = outcome::VMMCALL;
    c.journal.vmrun_attempts = 1;
    c.journal.completed_exits = 1;
    c.guest.exit_code = 0x81;
    c.guest.rip = VMMCALL_RIP;
    c.guest.rax = COOKIE;
    c.guest.gprs = GUEST_GPRS;
    c.guest.captured_fields = guest_capture::ALL;
    assert_eq!(unsafe { guest.verify_observations(c) }, Ok(15));
    // Matching guest values do not establish that every capture completed.
    // Reject both an absent mask and each individually missing component.
    c.guest.captured_fields = 0;
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(35));
    for missing_bit in 0..4 {
        c.guest.captured_fields = guest_capture::ALL & !(1 << missing_bit);
        assert_eq!(unsafe { guest.verify_observations(c) }, Err(35));
    }
    c.guest.captured_fields = guest_capture::ALL;
    assert_eq!(unsafe { guest.verify_observations(c) }, Ok(15));
    c.guest.gprs[13] ^= 1;
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(35));
    c.guest.gprs[13] ^= 1;
    c.restored.cr3 = 1;
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(30));
    c.restored.cr3 = 0;
    unsafe { storage.0.add(5 * 4096 + 576).write(1) };
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(33));
    unsafe {
        storage.0.add(5 * 4096 + 576).write(0);
        storage.0.add(2 * 4096 + 0x440).write(1);
    }
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(34));
}

#[cfg(not(feature = "native-transition-event-test"))]
#[test]
fn multi_guest_matches_independently_assembled_program_and_core_protocol() {
    let assembled = include_bytes!("fixtures/native_transition_multi/guest.bin");
    assert_eq!(assembled.len(), MULTI_GUEST_BYTES);
    for profile in [0, 3, 7] {
        let storage = Storage::new();
        let b = boundary(profile);
        let guest = unsafe { initialize_multi_exit(storage.0, 0x200000, &b, inputs()).unwrap() };
        assert_eq!(&storage.bytes()[7 * 4096..7 * 4096 + assembled.len()], assembled);
        // All pages beyond code and the CPUID/HLT intercept word retain the
        // independently frozen one-entry arena's mappings and initial state.
        for (offset, (&old, &new)) in GOLDEN.iter().zip(storage.bytes()).enumerate() {
            if !(7 * 4096..8 * 4096).contains(&offset) && !(0x0c..0x10).contains(&offset) {
                assert_eq!(old, new, "unexpected constructor write at {offset:#x}");
            }
        }
        assert!(storage.bytes()[8 * 4096..9 * 4096].iter().all(|byte| *byte == 0));
        let intercepts = u32::from_le_bytes(storage.bytes()[0x0c..0x10].try_into().unwrap());
        assert_eq!(intercepts, 0x1904000b);
        let gdt = [0u8; 64];
        let bound = unsafe { guest.bind(&b, &gdt, mode::MULTI_EXIT).unwrap() };
        let c = unsafe { &*bound.context() };
        assert_eq!(
            (c.inputs.mode, c.inputs.expected_vmmcall_rip, c.inputs.expected_vmmcall_rax),
            (mode::MULTI_EXIT, MULTI_STOP_RIP, 1)
        );
        assert_eq!(unsafe { bound.multi_completion() }, [0, 0]);
    }
    for (index, &leaf) in MULTI_CPUID_LEAVES.iter().enumerate() {
        let actual = svmvisor_hypervisor::svm::emulation::cpuid(leaf, 0).map(u64::from);
        assert_eq!(MULTI_CPUID_OUTPUTS[index], actual);
        assert_eq!(
            svmvisor_hypervisor::svm::emulation::cpuid(leaf, 0xfeedbeef).map(u64::from),
            actual
        );
        for cycle in 0..4 {
            let round = index + cycle * 8;
            assert_eq!(
                u64::from_le_bytes(
                    assembled[0x200 + round * 8..0x208 + round * 8].try_into().unwrap()
                ),
                0xaabbccdd00000000 | u64::from(leaf)
            );
            for register in 0..4 {
                let offset = 0x300 + register * 0x100 + round * 8;
                assert_eq!(
                    u64::from_le_bytes(assembled[offset..offset + 8].try_into().unwrap()),
                    actual[register]
                );
            }
        }
    }
    use native_transition::multi;
    assert_eq!(
        (MULTI_CPUID_RIP, MULTI_QUERY_RIP, MULTI_STOP_RIP),
        (multi::CPUID_RIP, multi::QUERY_RIP, multi::STOP_RIP)
    );
    for (rip, bytes) in [
        (MULTI_CPUID_RIP, &[0x0f, 0xa2][..]),
        (MULTI_QUERY_RIP, &[0x0f, 0x01, 0xd9][..]),
        (MULTI_STOP_RIP, &[0x0f, 0x01, 0xd9][..]),
        (MULTI_FAIL_RIP, &[0x0f, 0x0b][..]),
    ] {
        let offset = (rip - 0x1000) as usize;
        assert_eq!(&assembled[offset..offset + bytes.len()], bytes);
    }
}

#[cfg(not(feature = "native-transition-event-test"))]
#[test]
fn multi_program_cannot_bind_legacy_modes_or_repurpose_another_boundary() {
    let storage = Storage::new();
    let b = boundary(0);
    let other = boundary(0);
    let gdt = [0u8; 64];
    for requested in [mode::ONE_ENTRY, mode::BIND_ONLY, 3, u64::MAX] {
        let guest = unsafe { initialize_multi_exit(storage.0, 0x200000, &b, inputs()).unwrap() };
        assert!(matches!(unsafe { guest.bind(&b, &gdt, requested) }, Err(12)));
        assert!(storage.bytes()[30 * 4096..].iter().all(|byte| *byte == 0));
    }
    let guest = unsafe { initialize_multi_exit(storage.0, 0x200000, &b, inputs()).unwrap() };
    assert!(matches!(unsafe { guest.bind(&other, &gdt, mode::MULTI_EXIT) }, Err(12)));
}

#[cfg(not(feature = "native-transition-event-test"))]
#[test]
fn multi_success_requires_guest_payload_and_all_actual_terminal_evidence() {
    let storage = Storage::new();
    let b = boundary(7);
    let gdt = [0u8; 64];
    let guest = unsafe {
        initialize_multi_exit(storage.0, 0x200000, &b, inputs())
            .unwrap()
            .bind(&b, &gdt, mode::MULTI_EXIT)
            .unwrap()
    };
    let c = unsafe { &mut *guest.context() };
    c.journal.outcome = outcome::MULTI_EXIT;
    c.journal.vmrun_attempts = 65;
    c.journal.completed_exits = 65;
    c.guest.exit_code = 0x81;
    c.guest.rip = MULTI_STOP_RIP;
    c.guest.rsp = 0x9000;
    c.guest.rax = 1;
    c.guest.gprs = MULTI_FINAL_GPRS;
    c.guest.captured_fields = guest_capture::ALL;
    c.guest.reserved = [32, 32, 64, 0, MULTI_STOP_RIP + 3, 3, 1, 0, 0];
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
    unsafe {
        storage.0.add(8 * 4096).cast::<u64>().write(32);
        storage.0.add(8 * 4096 + 8).cast::<u64>().write(COOKIE);
    }
    assert_eq!(unsafe { guest.verify_observations(c) }, Ok(15));
    for register in 0..14 {
        for bit in [0, 32, 63] {
            c.guest.gprs[register] ^= 1 << bit;
            assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
            c.guest.gprs[register] ^= 1 << bit;
        }
    }
    for field in [0, 1, 2, 3, 5, 7, 8] {
        c.guest.reserved[field] ^= 1;
        assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
        c.guest.reserved[field] ^= 1;
    }
    c.guest.reserved[4] += 1;
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
    c.guest.reserved[4] -= 1;
    c.guest.reserved[6] = 2;
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
    c.guest.reserved[6] = 1;
    for field in 0..8 {
        let saved = c.guest;
        match field {
            0 => c.guest.exit_code = 0x72,
            1 => c.guest.exit_int_info = 1 << 31,
            2 => c.guest.rip += 1,
            3 => c.guest.rsp -= 8,
            4 => c.guest.rax |= 1 << 32,
            5 => c.guest.captured_fields = 0,
            6 => c.journal.vmrun_attempts -= 1,
            _ => c.journal.completed_exits -= 1,
        }
        assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
        c.guest = saved;
        c.journal.vmrun_attempts = 65;
        c.journal.completed_exits = 65;
    }
    for (offset, valid) in [(0, 32), (8, COOKIE)] {
        unsafe { storage.0.add(8 * 4096 + offset).cast::<u64>().write(valid ^ 1) };
        assert_eq!(unsafe { guest.verify_observations(c) }, Err(37));
        unsafe { storage.0.add(8 * 4096 + offset).cast::<u64>().write(valid) };
    }
    // An abnormal partial run may prove restoration, but cannot acquire the
    // guest-success adapter bit from a completed-looking payload alone.
    c.journal.outcome = outcome::UNEXPECTED_EXIT;
    c.journal.vmrun_attempts = 2;
    c.journal.completed_exits = 2;
    assert_eq!(unsafe { guest.verify_observations(c) }, Ok(7));
    c.restored.cr3 = 1;
    assert_eq!(unsafe { guest.verify_observations(c) }, Err(30));
}
