#![no_std]
#![no_main]
mod cpu;
mod memory;
use core::{arch::asm, ptr};
use svmvisor_returning_probe::context::{
    Context, Fixture, LOADED_EXTRA_CANARIES, LOADED_XMM15, LOADED_YMM15_HIGH, extra_state_matches,
    loaded_extra_restored, loaded_xstate_restored, post_exit_extra_restored,
    post_exit_xstate_restored, xstate_payload_matches,
};
use uefi::{
    Status,
    boot::{self, AllocateType, MemoryType},
};
unsafe extern "efiapi" {
    fn probe_entry(context: *mut Context);
    fn fixture_entry(context: *mut Fixture);
    fn host_fault_ud_stub();
    fn host_fault_gp_stub();
    fn host_fault_unexpected_stub();
    static host_fault_ud_pc: u8;
    static host_fault_gp_pc: u8;
    static host_post_exit_ud_pc: u8;
    static host_post_exit_gp_pc: u8;
    static host_xstate_ud_pc: u8;
    static host_xstate_gp_pc: u8;
    static host_loaded_ud_pc: u8;
    static host_loaded_gp_pc: u8;
    static host_armed_ud_pc: u8;
    static host_armed_gp_pc: u8;
}

fn print(message: &str) {
    for byte in message.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn hex(value: u64) {
    for shift in (0..16).rev() {
        let byte = b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize];
        unsafe {
            asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn finish(pass: bool) -> ! {
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4u16, in("eax") if pass { 16u32 } else { 17u32 }, options(nomem, nostack));
    }
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    print("FAIL returning-probe panic\n");
    finish(false)
}

fn boot_services_check() {
    let allocation =
        boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1).unwrap();
    unsafe {
        allocation.as_ptr().write_volatile(0x73);
        assert_eq!(allocation.as_ptr().read_volatile(), 0x73);
        boot::free_pages(allocation, 1).unwrap();
    }
    print("PASS boot-services-after-return\n");
}

#[uefi::entry]
fn main() -> Status {
    // No privileged observations or probe-related allocation before this guard.
    if !cpu::tcg_guard() {
        return Status::UNSUPPORTED;
    }
    print("returning-probe-entry tcg-only\n");
    if cfg!(feature = "refuse-admission") {
        let before = unsafe { (cpu::msr(0xc0000080), cpu::msr(0xc0010117)) };
        // Native admission must reject the actually reported hypervisor.
        use svmvisor_hypervisor::{memory::address::EncryptionState, arch::x86_64::capabilities::*};
        let result = CapabilityEvidence {
            vendor: CpuVendor::Amd,
            svm: EvidenceFlag::Set,
            nested_paging: EvidenceFlag::Set,
            svm_revision: Some(1),
            asid_count: Some(2),
            physical_address_bits: Some(48),
            vm_cr_svmdis: EvidenceFlag::Clear,
            hypervisor_present: EvidenceFlag::Set,
            encryption: EncryptionState::Unknown,
            optional: OptionalFeatures::default(),
        }
        .validate();
        assert_eq!(result, Err(CapabilityError::HypervisorPresentOrUnknown));
        assert_eq!(before, unsafe {
            (cpu::msr(0xc0000080), cpu::msr(0xc0010117))
        });
        boot_services_check();
        print("PASS returning-admission-refused\n");
        finish(true);
    }
    let mask = if cfg!(feature = "fixture-avx") {
        7
    } else if cfg!(feature = "fixture-sse") {
        3
    } else {
        0
    };
    assert!(!(cfg!(feature = "fixture-sse") && cfg!(feature = "fixture-avx")));
    if mask == 0 {
        run_sessions();
    } else {
        assert!(unsafe { cpu::fixture_supported(mask) });
        let areas =
            boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 4).unwrap();
        unsafe {
            ptr::write_bytes(areas.as_ptr(), 0, 4 * 4096);
        }
        let page = |i: usize| unsafe { areas.as_ptr().add(i * 4096) as u64 };
        let mut completed = 0u64;
        let mut fixture = Fixture {
            callback: measured_callback as *const () as u64,
            opaque: ptr::addr_of_mut!(completed) as u64,
            mask,
            original_fx: page(0),
            original_xsave: page(1),
            observed_fx: page(2),
            observed_xsave: page(3),
            ..Fixture::default()
        };
        unsafe {
            fixture_entry(&mut fixture);
        }
        print("outer-original-cr4=");
        hex(fixture.original_cr4);
        print(" original-xcr0=");
        hex(fixture.original_xcr0);
        print(" observed-cr4=");
        hex(fixture.observed_cr4);
        print(" observed-xcr0=");
        hex(fixture.observed_xcr0);
        print("\n");
        assert_eq!(completed, 1);
        assert!(fixture.restored_controls());
        let image = |i| unsafe { core::slice::from_raw_parts(page(i) as *const u8, 4096) };
        assert!(xstate_payload_matches(image(0), image(2), 0, None));
        let avx_offset = Some(cpu::cpuid(0xd, 2).ebx as usize);
        assert!(xstate_payload_matches(
            image(1),
            image(3),
            fixture.original_xcr0,
            avx_offset
        ));
        unsafe {
            boot::free_pages(areas, 4).unwrap();
        }
        print("PASS outer-fixture-restored\n");
    }
    boot_services_check();
    if cfg!(feature = "invalid-entry") {
        print("PASS returning-invalid-entry-restored\n");
    } else if cfg!(feature = "missing-restore") {
        print("PASS returning-missing-restore-detected\n");
    } else if cfg!(feature = "post-exit-host-ud") {
        print("PASS returning-post-exit-host-ud-restored\n");
    } else if cfg!(feature = "post-exit-host-gp") {
        print("PASS returning-post-exit-host-gp-restored\n");
    } else if cfg!(feature = "xstate-host-ud") {
        print("PASS returning-xstate-host-ud-restored\n");
    } else if cfg!(feature = "xstate-host-gp") {
        print("PASS returning-xstate-host-gp-restored\n");
    } else if cfg!(feature = "loaded-host-ud") {
        print("PASS returning-loaded-host-ud-restored\n");
    } else if cfg!(feature = "loaded-host-gp") {
        print("PASS returning-loaded-host-gp-restored\n");
    } else if cfg!(feature = "armed-host-ud") {
        print("PASS returning-armed-host-ud-restored\n");
    } else if cfg!(feature = "armed-host-gp") {
        print("PASS returning-armed-host-gp-restored\n");
    } else if cfg!(feature = "host-ud") {
        print("PASS returning-host-ud-restored\n");
    } else if cfg!(feature = "host-gp") {
        print("PASS returning-host-gp-restored\n");
    } else if cfg!(feature = "guest-ud") {
        print("PASS returning-guest-ud-restored\n");
    } else if cfg!(feature = "guest-page-fault") {
        print("PASS returning-guest-page-fault-restored\n");
    } else {
        print("PASS returning-probe sessions=16\n");
    }
    finish(true)
}

unsafe extern "efiapi" fn measured_callback(opaque: *mut u64) {
    run_sessions();
    unsafe {
        opaque.write(1);
    }
}
fn run_sessions() {
    print("observed-efer=");
    hex(unsafe { cpu::msr(0xc0000080) });
    print(" cpuid1ecx=");
    hex(cpu::cpuid(1, 0).ecx as u64);
    print("\n");
    let Some(cpu) = (unsafe { cpu::collect() }) else {
        print("FAIL returning-admission-profile\n");
        finish(false);
    };
    print(if cpu.plan.layout().mask() == 7 {
        "returning-xstate=xsave-avx\n"
    } else if cpu.plan.layout().uses_xsave() {
        "returning-xstate=xsave-sse\n"
    } else {
        "returning-xstate=fxsave\n"
    });
    print("returning-profile-xcr0=");
    hex(cpu.plan.original_controls().xcr0.unwrap_or(0));
    print("\n");
    // Firmware GDT/IDT mappings are retained. This is the fixed OVMF fixture,
    // not generic descriptor or asynchronous-event qualification.
    let cs: u16;
    let ss: u16;
    let ds: u16;
    let es: u16;
    unsafe {
        asm!("mov {:x}, cs",out(reg) cs,options(nostack));
        asm!("mov {:x}, ss",out(reg) ss,options(nostack));
        asm!("mov {:x}, ds",out(reg) ds,options(nostack));
        asm!("mov {:x}, es",out(reg) es,options(nostack));
    }
    assert!([cs, ss, ds, es].iter().all(|selector| selector & 4 == 0));
    let allocation = boot::allocate_pages(
        AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        memory::PAGES,
    )
    .unwrap();
    let base = allocation.as_ptr();
    let page = |index: usize| unsafe { base.add(index * 4096) as u64 };
    let post_exit_fault = cfg!(feature = "post-exit-host-ud")
        || cfg!(feature = "post-exit-host-gp")
        || cfg!(feature = "post-exit-host-fault-mismatch");
    let xstate_fault = cfg!(feature = "xstate-host-ud")
        || cfg!(feature = "xstate-host-gp")
        || cfg!(feature = "xstate-host-fault-mismatch");
    if (xstate_fault || post_exit_fault) && cpu.plan.layout().mask() == 7 {
        print("returning-avx-offset=");
        hex(cpu.plan.layout().avx_offset().unwrap() as u64);
        print("\n");
    }
    let loaded_fault = cfg!(feature = "loaded-host-ud")
        || cfg!(feature = "loaded-host-gp")
        || cfg!(feature = "loaded-host-fault-mismatch");
    let armed_fault = cfg!(feature = "armed-host-ud")
        || cfg!(feature = "armed-host-gp")
        || cfg!(feature = "armed-host-fault-mismatch");
    let host_fault = post_exit_fault
        || xstate_fault
        || loaded_fault
        || armed_fault
        || cfg!(feature = "host-ud")
        || cfg!(feature = "host-gp")
        || cfg!(feature = "host-fault-mismatch");
    for session in 0..16u64 {
        let expected_rip = unsafe { memory::prepare(base, &cpu, cfg!(feature = "invalid-entry")) };
        let mut prepared_xstate = [0u8; 4096];
        if post_exit_fault {
            unsafe {
                ptr::copy_nonoverlapping(base.add(6 * 4096), prepared_xstate.as_mut_ptr(), 4096);
            }
        }
        let mut prepared_descriptors = [0u8; 32];
        unsafe {
            ptr::copy_nonoverlapping(base.add(0x470), prepared_descriptors.as_mut_ptr(), 16);
            ptr::copy_nonoverlapping(
                base.add(0x490),
                prepared_descriptors.as_mut_ptr().add(16),
                16,
            );
        }
        let original_idtr = cpu::idtr();
        if host_fault {
            unsafe {
                memory::prepare_idt(
                    base,
                    cs,
                    host_fault_unexpected_stub as *const () as u64,
                    host_fault_ud_stub as *const () as u64,
                    host_fault_gp_stub as *const () as u64,
                );
            }
        }
        let mut context = Context {
            guest_vmcb: page(0),
            host_extra: page(1),
            observed_extra: page(2),
            hsave: page(3),
            original_xstate: page(4),
            observed_xstate: page(5),
            guest_xstate: page(6),
            mask: if cpu.plan.layout().uses_xsave() {
                cpu.plan.layout().mask()
            } else {
                0
            },
            negative_flags: u64::from(cfg!(feature = "missing-restore")),
            outer_xstate: page(24),
            private_idt: if host_fault { page(32) } else { 0 },
            checkpoint_extra: if loaded_fault || xstate_fault || post_exit_fault {
                page(33)
            } else {
                0
            },
            checkpoint_xstate: if xstate_fault || post_exit_fault {
                page(34)
            } else {
                0
            },
            fault_kind: if cfg!(feature = "post-exit-host-fault-mismatch") {
                15
            } else if cfg!(feature = "post-exit-host-ud") {
                13
            } else if cfg!(feature = "post-exit-host-gp") {
                14
            } else if cfg!(feature = "xstate-host-fault-mismatch") {
                12
            } else if cfg!(feature = "xstate-host-ud") {
                10
            } else if cfg!(feature = "xstate-host-gp") {
                11
            } else if cfg!(feature = "loaded-host-fault-mismatch") {
                9
            } else if cfg!(feature = "loaded-host-ud") {
                7
            } else if cfg!(feature = "loaded-host-gp") {
                8
            } else if cfg!(feature = "armed-host-fault-mismatch") {
                6
            } else if cfg!(feature = "armed-host-ud") {
                4
            } else if cfg!(feature = "armed-host-gp") {
                5
            } else if cfg!(feature = "host-fault-mismatch") {
                3
            } else if cfg!(feature = "host-ud") {
                1
            } else if cfg!(feature = "host-gp") {
                2
            } else {
                0
            },
            ..Context::default()
        };
        print(if host_fault {
            "host-fault-session="
        } else {
            "returning-session="
        });
        hex(session);
        print(" begin\n");
        unsafe {
            probe_entry(&mut context);
        }
        assert!(
            !cfg!(feature = "post-exit-host-fault-mismatch")
                && !cfg!(feature = "host-fault-mismatch")
                && !cfg!(feature = "armed-host-fault-mismatch")
                && !cfg!(feature = "loaded-host-fault-mismatch")
                && !cfg!(feature = "xstate-host-fault-mismatch")
        ); // The negative must terminate in its handler.
        let observed_idtr = cpu::idtr();
        assert_eq!(observed_idtr, original_idtr);
        print(if host_fault {
            "host-fault-return="
        } else {
            "returning-exit="
        });
        hex(context.exit_code);
        print(" abi=");
        hex(context.abi_failures);
        print("\n");
        for i in 0..8 {
            if context.original[i] != context.observed[i] {
                print("control-mismatch-index=");
                hex(i as u64);
                print(" original=");
                hex(context.original[i]);
                print(" observed=");
                hex(context.observed[i]);
                print("\n");
            }
        }
        assert!(context.restored_controls());
        assert_eq!(context.abi_failures, 0);
        let slice =
            |index: usize| unsafe { core::slice::from_raw_parts(page(index) as *const u8, 4096) };
        if !host_fault || armed_fault || loaded_fault || xstate_fault || post_exit_fault {
            assert!(extra_state_matches(slice(1), slice(2)));
        }
        if host_fault {
            let vector = if cfg!(feature = "post-exit-host-ud")
                || cfg!(feature = "host-ud")
                || cfg!(feature = "armed-host-ud")
                || cfg!(feature = "loaded-host-ud")
                || cfg!(feature = "xstate-host-ud")
            {
                6
            } else {
                13
            };
            let pc = if cfg!(feature = "post-exit-host-ud") {
                ptr::addr_of!(host_post_exit_ud_pc) as u64
            } else if cfg!(feature = "post-exit-host-gp") {
                ptr::addr_of!(host_post_exit_gp_pc) as u64
            } else if cfg!(feature = "xstate-host-ud") {
                ptr::addr_of!(host_xstate_ud_pc) as u64
            } else if cfg!(feature = "xstate-host-gp") {
                ptr::addr_of!(host_xstate_gp_pc) as u64
            } else if cfg!(feature = "loaded-host-ud") {
                ptr::addr_of!(host_loaded_ud_pc) as u64
            } else if cfg!(feature = "loaded-host-gp") {
                ptr::addr_of!(host_loaded_gp_pc) as u64
            } else if cfg!(feature = "armed-host-ud") {
                ptr::addr_of!(host_armed_ud_pc) as u64
            } else if cfg!(feature = "armed-host-gp") {
                ptr::addr_of!(host_armed_gp_pc) as u64
            } else if cfg!(feature = "host-ud") {
                ptr::addr_of!(host_fault_ud_pc) as u64
            } else {
                ptr::addr_of!(host_fault_gp_pc) as u64
            };
            print("host-fault-vector=");
            hex(context.fault_vector);
            print(" rip=");
            hex(context.fault_rip);
            print(" error=");
            hex(context.fault_error);
            print(" stage=");
            hex(context.fault_stage);
            print(" vmrun-attempts=");
            hex(context.vmrun_attempts);
            print("\n");
            print("host-idtr-base=");
            hex(u64::from_le_bytes(original_idtr[2..10].try_into().unwrap()));
            print(" restored-base=");
            hex(u64::from_le_bytes(observed_idtr[2..10].try_into().unwrap()));
            print(" limit=");
            hex(u16::from_le_bytes(original_idtr[..2].try_into().unwrap()) as u64);
            print(" restored-limit=");
            hex(u16::from_le_bytes(observed_idtr[..2].try_into().unwrap()) as u64);
            print("\n");
            if post_exit_fault {
                assert!(context.recovered_post_exit_fault(
                    vector,
                    pc,
                    expected_rip,
                    &original_idtr,
                    &observed_idtr
                ));
                print("post-exit-guest-rip=");
                hex(context.guest_exit_rip);
                print(" expected=");
                hex(expected_rip);
                print(" capture-stage=");
                hex(context.guest_capture_stage);
                print(" sentinel=");
                hex(context.guest_rax);
                print("\n");
            } else {
                assert!(context.recovered_host_fault(vector, pc, &original_idtr, &observed_idtr));
            }
            if armed_fault || loaded_fault || xstate_fault || post_exit_fault {
                print("host-checkpoint-efer=");
                hex(context.checkpoint_efer);
                print(" original-efer=");
                hex(context.original[4]);
                print(" hsave=");
                hex(context.checkpoint_hsave);
                print(" owned-hsave=");
                hex(context.hsave);
                print(" original-hsave=");
                hex(context.original[5]);
                print(" mutation-stage=");
                hex(context.mutation_stage);
                print(" checkpoint-rflags=");
                hex(context.checkpoint_rflags);
                print("\n");
                if loaded_fault || xstate_fault || post_exit_fault {
                    if post_exit_fault {
                        assert!(context.restored_post_exit_checkpoint());
                    } else if xstate_fault {
                        assert!(context.restored_xstate_checkpoint());
                    } else {
                        assert!(context.restored_loaded_checkpoint());
                    }
                    for (offset, expected) in LOADED_EXTRA_CANARIES {
                        let value = |index| {
                            u64::from_le_bytes(slice(index)[offset..offset + 8].try_into().unwrap())
                        };
                        print(if post_exit_fault {
                            "post-exit-extra-offset="
                        } else {
                            "host-loaded-extra-offset="
                        });
                        hex(offset as u64);
                        print(" original=");
                        hex(value(1));
                        print(" checkpoint=");
                        hex(value(33));
                        print(" expected=");
                        hex(expected);
                        print(" restored=");
                        hex(value(2));
                        print("\n");
                    }
                    if post_exit_fault {
                        assert!(post_exit_extra_restored(
                            slice(1),
                            &prepared_descriptors,
                            slice(33),
                            slice(2)
                        ));
                    } else {
                        assert!(loaded_extra_restored(slice(1), slice(33), slice(2)));
                    }
                } else {
                    assert!(context.restored_armed_checkpoint());
                }
            }
        } else {
            assert_eq!(context.vmrun_attempts, 1);
        }

        if post_exit_fault {
            assert_eq!(slice(6), &prepared_xstate);
            assert!(post_exit_xstate_restored(
                slice(4),
                slice(6),
                slice(34),
                slice(5),
                context.mask,
                cpu.plan.layout().avx_offset()
            ));
            for start in [
                Some(400),
                if context.mask == 7 {
                    cpu.plan.layout().avx_offset().map(|x| x + 240)
                } else {
                    None
                },
            ]
            .into_iter()
            .flatten()
            {
                for offset in [start, start + 8] {
                    let value = |index| {
                        u64::from_le_bytes(slice(index)[offset..offset + 8].try_into().unwrap())
                    };
                    print("post-exit-xstate-offset=");
                    hex(offset as u64);
                    print(" original=");
                    hex(value(4));
                    print(" checkpoint=");
                    hex(value(34));
                    print(" expected=");
                    hex(0);
                    print(" restored=");
                    hex(value(5));
                    print("\n");
                }
            }
        }
        if xstate_fault {
            assert!(loaded_xstate_restored(
                slice(4),
                slice(6),
                slice(34),
                slice(5),
                context.mask,
                cpu.plan.layout().avx_offset()
            ));
            for (offset, values) in [
                (Some(160 + 15 * 16), LOADED_XMM15),
                (
                    if context.mask == 7 {
                        cpu.plan.layout().avx_offset().map(|x| x + 15 * 16)
                    } else {
                        None
                    },
                    LOADED_YMM15_HIGH,
                ),
            ] {
                if let Some(offset) = offset {
                    for (i, expected) in values.iter().enumerate() {
                        let offset = offset + i * 8;
                        let value = |index| {
                            u64::from_le_bytes(slice(index)[offset..offset + 8].try_into().unwrap())
                        };
                        print("host-loaded-xstate-offset=");
                        hex(offset as u64);
                        print(" original=");
                        hex(value(4));
                        print(" checkpoint=");
                        hex(value(34));
                        print(" expected=");
                        hex(*expected);
                        print(" restored=");
                        hex(value(5));
                        print("\n");
                    }
                }
            }
        }

        let state_matches = xstate_payload_matches(
            slice(4),
            slice(5),
            context.mask,
            cpu.plan.layout().avx_offset(),
        );
        if context.mask == 7 {
            let offset = cpu.plan.layout().avx_offset().unwrap();
            assert_eq!(slice(4)[512] & 4, 4);
            let high = &slice(4)[offset + 15 * 16..offset + 16 * 16];
            let mut expected = [0u8; 16];
            expected[..8].copy_from_slice(&0x55aa12344321aa55u64.to_le_bytes());
            expected[8..].copy_from_slice(&0xfedc098776543210u64.to_le_bytes());
            assert_eq!(high, expected);
            if cfg!(feature = "missing-restore") {
                assert_ne!(high, &slice(5)[offset + 15 * 16..offset + 16 * 16]);
            } else {
                assert_eq!(high, &slice(5)[offset + 15 * 16..offset + 16 * 16]);
            }
        }
        if cfg!(feature = "missing-restore") {
            assert!(!state_matches);
        } else {
            assert!(state_matches);
        }
        if host_fault && !post_exit_fault {
            assert_eq!(
                unsafe { ptr::read_volatile((page(8) + 0xff0) as *const u64) },
                0
            );
            assert_eq!(context.guest_gprs, [0; 14]);
        } else if cfg!(feature = "invalid-entry") {
            // Pinned QEMU 10.1 TCG reports INVALID with a zero upper DWORD.
            assert_eq!(context.exit_code, u32::MAX as u64);
            assert_eq!(
                unsafe { ptr::read_volatile((page(8) + 0xff0) as *const u64) },
                0
            );
        } else if cfg!(feature = "guest-ud") || cfg!(feature = "guest-page-fault") {
            let read = |offset| unsafe { ptr::read_volatile((page(0) + offset) as *const u64) };
            assert_eq!(
                context.exit_code,
                if cfg!(feature = "guest-ud") {
                    0x46
                } else {
                    0x4e
                }
            );
            assert_eq!(read(0x578), expected_rip);
            assert_eq!(
                unsafe { ptr::read_volatile((page(8) + 0xfe8) as *const u64) },
                0x51554d5552455455
            );
            if cfg!(feature = "guest-page-fault") {
                assert_eq!(read(0x78), 0); // Supervisor data read, nonpresent guest PTE.
                assert_eq!(read(0x80), 0x200000);
            }
            assert_eq!(
                unsafe { ptr::read_volatile((page(8) + 0xff0) as *const u64) },
                0
            );
        } else {
            if post_exit_fault {
                assert_eq!(
                    unsafe { ptr::read_volatile((page(8) + 0xfe8) as *const u64) },
                    0
                );
            }
            assert_eq!(context.exit_code, 0x81);
            assert_eq!(context.guest_rax, 0x51554d5552455455);
            let mut expected_gprs = [
                0x201, 0x202, 0x203, 0x204, 0x205, 0x206, 0x208, 0x209, 0x20a, 0x20b, 0x20c, 0x20d,
                0x20e, 0x20f,
            ];
            if context.mask != 0 {
                expected_gprs[0] = 0;
                expected_gprs[1] = 0;
            }
            assert_eq!(context.guest_gprs, expected_gprs);
            assert_eq!(
                unsafe { ptr::read_volatile((page(0) + 0x578) as *const u64) },
                expected_rip
            );
            assert_eq!(
                unsafe { ptr::read_volatile((page(8) + 0xff0) as *const u64) },
                context.guest_rax
            );
        }
        // Independent post-boundary MSR observations, after the outer wrapper
        // has restored actual firmware state as well as its test canaries.
        assert_eq!(
            unsafe { cpu::msr(0xc0000080) },
            cpu.plan.original_controls().efer
        );
        assert_eq!(unsafe { cpu::msr(0xc0010117) }, 0);
    }
    unsafe {
        boot::free_pages(allocation, memory::PAGES).unwrap();
    }
}
