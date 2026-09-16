use core::mem::{align_of, offset_of, size_of};
use svmvisor_hypervisor::arch::x86_64::registers::GuestRegisters;

fn frame() -> GuestRegisters {
    GuestRegisters {
        rcx: 0xaabb_ccdd_1234_5678,
        rdx: 0xbbbb_bbbb_bbbb_bbbb,
        rbx: 0xcccc_cccc_cccc_cccc,
        rbp: 0x1111_1111_1111_1111,
        rsi: 0x2222_2222_2222_2222,
        rdi: 0x3333_3333_3333_3333,
        r8: 0x4444_4444_4444_4444,
        r9: 0x5555_5555_5555_5555,
        r10: 0x6666_6666_6666_6666,
        r11: 0x7777_7777_7777_7777,
        r12: 0x8888_8888_8888_8888,
        r13: 0x9999_9999_9999_9999,
        r14: 0xdddd_dddd_dddd_dddd,
        r15: 0xeeee_eeee_eeee_eeee,
    }
}

#[test]
fn every_field_has_the_documented_assembly_offset_without_hidden_padding() {
    assert_eq!(size_of::<GuestRegisters>(), 112);
    assert_eq!(align_of::<GuestRegisters>(), 8);
    let offsets = [
        offset_of!(GuestRegisters, rcx),
        offset_of!(GuestRegisters, rdx),
        offset_of!(GuestRegisters, rbx),
        offset_of!(GuestRegisters, rbp),
        offset_of!(GuestRegisters, rsi),
        offset_of!(GuestRegisters, rdi),
        offset_of!(GuestRegisters, r8),
        offset_of!(GuestRegisters, r9),
        offset_of!(GuestRegisters, r10),
        offset_of!(GuestRegisters, r11),
        offset_of!(GuestRegisters, r12),
        offset_of!(GuestRegisters, r13),
        offset_of!(GuestRegisters, r14),
        offset_of!(GuestRegisters, r15),
    ];
    assert_eq!(
        offsets,
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104]
    );
}

#[test]
fn cpuid_inputs_use_only_low_halves_and_leave_the_frame_unchanged() {
    let registers = frame();
    let before = registers;
    assert_eq!(
        registers.cpuid_inputs(0xdead_beef_8765_4321),
        [0x8765_4321, 0x1234_5678]
    );
    assert_eq!(registers, before);
    assert_eq!(GuestRegisters::default().cpuid_inputs(0), [0, 0]);
}

#[test]
fn cpuid_outputs_zero_extend_and_preserve_every_nonoutput_register() {
    for output in [
        [0, 1, 2, 3],
        [u32::MAX, 0xfedc_ba98, 0x8765_4321, 0x8000_0000],
    ] {
        let mut registers = frame();
        let mut expected = registers;
        expected.rbx = u64::from(output[1]);
        expected.rcx = u64::from(output[2]);
        expected.rdx = u64::from(output[3]);
        let guest_rax = registers.apply_cpuid(output);
        assert_eq!(guest_rax, u64::from(output[0]));
        assert_eq!(registers, expected);
    }
}
