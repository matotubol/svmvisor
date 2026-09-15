//! Owned, fixed guest mappings. No firmware service is called during entry.
use crate::cpu::Cpu;
use core::ptr;
use svmvisor_hypervisor::{
    address::{AddressPolicy, EncryptionState},
    capabilities::EvidenceFlag,
    descriptors::GuestDescriptorRequest,
    guest_pages::{GuestPages, PagePermissions as GuestPermission, TableStorage as GuestTables},
    guest_state::GuestStateRequest,
    npt::{Npt, NptEvidence, PagePermissions as NestedPermission, TableStorage as NestedTables},
    vmcb::{InstructionIntercept, Vmcb},
    xstate::XstateArea,
};
pub const PAGES: usize = 35;
unsafe extern "C" {
    static guest_code_start: u8;
    static guest_code_end: u8;
    pub static guest_vmmcall: u8;
    static guest_ud_start: u8;
    static guest_ud_fault: u8;
    static guest_ud_end: u8;
    static guest_pf_start: u8;
    static guest_pf_fault: u8;
    static guest_pf_end: u8;
}

pub unsafe fn prepare(base: *mut u8, cpu: &Cpu, invalid: bool) -> u64 {
    unsafe {
        ptr::write_bytes(base, 0, PAGES * 4096);
    }
    let policy = AddressPolicy::new(
        cpu.physical_bits,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap();
    policy
        .validate(base as u64, (PAGES * 4096) as u64, 4096)
        .unwrap();
    let page = |n: usize| unsafe { base.add(n * 4096) };
    let (code_start, code_end, fault) = if cfg!(feature = "guest-ud") {
        (
            ptr::addr_of!(guest_ud_start) as usize,
            ptr::addr_of!(guest_ud_end) as usize,
            ptr::addr_of!(guest_ud_fault) as usize,
        )
    } else if cfg!(feature = "guest-page-fault") {
        (
            ptr::addr_of!(guest_pf_start) as usize,
            ptr::addr_of!(guest_pf_end) as usize,
            ptr::addr_of!(guest_pf_fault) as usize,
        )
    } else {
        (
            ptr::addr_of!(guest_code_start) as usize,
            ptr::addr_of!(guest_code_end) as usize,
            ptr::addr_of!(guest_vmmcall) as usize,
        )
    };
    assert!(code_end > code_start && code_end - code_start <= 4096);
    unsafe {
        ptr::copy_nonoverlapping(code_start as *const u8, page(7), code_end - code_start);
    }
    let descriptors = GuestDescriptorRequest {
        gdt_base: 0x4000,
        tss_base: 0x5000,
        rsp0: 0x9000,
        ist1: 0xb000,
    }
    .validate()
    .unwrap();
    unsafe {
        ptr::copy_nonoverlapping(
            descriptors.gdt().as_ptr(),
            page(10),
            descriptors.gdt().len(),
        );
        ptr::copy_nonoverlapping(
            descriptors.tss().as_ptr(),
            page(11),
            descriptors.tss().len(),
        );
    }
    let guest_cr3 = {
        let mut tables = GuestPages::new(
            unsafe { &mut *page(12).cast::<GuestTables>() },
            0x10000,
            policy,
        )
        .unwrap();
        tables
            .map_page(0x1000, 0x1000, GuestPermission::ReadOnly)
            .unwrap();
        for address in [0x4000, 0x5000, 0x8000, 0xa000] {
            tables
                .map_page(address, address, GuestPermission::ReadWrite)
                .unwrap();
        }
        tables.root_address()
    };
    let nested_root = {
        let mut tables = Npt::new(
            unsafe { &mut *page(16).cast::<NestedTables>() },
            page(16) as u64,
            policy,
            cpu.physical_bits.min(48),
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
        .unwrap();
        tables
            .map_page(0x1000, page(7) as u64, NestedPermission::ReadExecute)
            .unwrap();
        for (gpa, index) in [(0x4000, 10), (0x5000, 11), (0x8000, 8), (0xa000, 9)] {
            tables
                .map_page(gpa, page(index) as u64, NestedPermission::ReadWrite)
                .unwrap();
        }
        for i in 0..4 {
            tables
                .map_page(
                    0x10000 + i * 4096,
                    page(12 + i as usize) as u64,
                    NestedPermission::ReadWrite,
                )
                .unwrap();
        }
        tables.root_address()
    };
    for index in [4, 5, 6, 24] {
        let area = unsafe { &mut *page(index).cast::<XstateArea>() };
        area.reset(cpu.plan.layout(), 0xffbf).unwrap();
    }
    if cfg!(feature = "xstate-host-ud")
        || cfg!(feature = "xstate-host-gp")
        || cfg!(feature = "xstate-host-fault-mismatch")
    {
        use svmvisor_returning_probe::context::{LOADED_XMM15, LOADED_YMM15_HIGH};
        for (i, value) in LOADED_XMM15.iter().enumerate() {
            unsafe {
                field(page(6), 160 + 15 * 16 + i * 8, value.to_le_bytes());
            }
        }
        if cpu.plan.layout().uses_xsave() {
            unsafe {
                field(page(6), 512, cpu.plan.layout().mask().to_le_bytes());
            }
        }
        if cpu.plan.layout().mask() == 7 {
            let offset = cpu.plan.layout().avx_offset().unwrap() + 15 * 16;
            for (i, value) in LOADED_YMM15_HIGH.iter().enumerate() {
                unsafe {
                    field(page(6), offset + i * 8, value.to_le_bytes());
                }
            }
        }
    }
    unsafe {
        ptr::write_bytes(page(25), 0xff, 5 * 4096);
    }
    let vmcb = unsafe { &mut *base.cast::<Vmcb>() };
    vmcb.set_nested_root(nested_root, &policy).unwrap();
    vmcb.set_permission_maps(page(25) as u64, page(28) as u64, &policy)
        .unwrap();
    for intercept in [
        InstructionIntercept::Hlt,
        InstructionIntercept::Vmrun,
        InstructionIntercept::Vmmcall,
        InstructionIntercept::Xsetbv,
        InstructionIntercept::Ioio,
        InstructionIntercept::Msr,
    ] {
        vmcb.set_instruction_intercept(intercept, true);
    }
    vmcb.set_synthetic_state(
        &GuestStateRequest {
            rip: 0x1000,
            rsp: 0x9000,
            rflags: 2,
            cr0: 0x80010033,
            cr3: guest_cr3,
            cr4: cpu.plan.layout().guest_cr4(),
            efer: 0x1500,
            rax: 0,
        }
        .validate_with_xstate(&policy, cpu.plan.layout())
        .unwrap(),
    );
    vmcb.set_guest_descriptors(&descriptors);
    // Deliberate TCG fixture ASID1, not native ValidatedCapabilities: the latter
    // correctly rejects CPUID's set hypervisor bit. Invalid-entry uses ASID0.
    unsafe {
        field(base, 0x058, if invalid { 0u32 } else { 1 }.to_le_bytes());
        field(base, 0x008, u32::MAX.to_le_bytes());
        field(base, 0x05c, [1]);
        field(base, 0x090, 1u64.to_le_bytes());
        field(base, 0x560, 0x400u64.to_le_bytes());
        field(base, 0x568, 0xffff0ff0u64.to_le_bytes());
        field(base, 0x668, 0x0007040600070406u64.to_le_bytes());
    }
    0x1000 + (fault - code_start) as u64
}
unsafe fn field<const N: usize>(base: *mut u8, offset: usize, bytes: [u8; N]) {
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(offset), N);
    }
}

/// Each gate uses the current fixture's 64-bit code selector and no IST switch.
/// Only the bounded assembly fixture installs this table, with interrupts masked.
pub unsafe fn prepare_idt(base: *mut u8, selector: u16, unexpected: u64, ud: u64, gp: u64) {
    let table = unsafe { core::slice::from_raw_parts_mut(base.add(32 * 4096), 4096) };
    for vector in 0..256 {
        let address = match vector {
            6 => ud,
            13 => gp,
            _ => unexpected,
        };
        let gate = &mut table[vector * 16..vector * 16 + 16];
        gate.copy_from_slice(&svmvisor_returning_probe::context::interrupt_gate(
            address, selector,
        ));
    }
}
