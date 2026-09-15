//! Captured post-EBS integer loader ABI migration, not firmware CPU resume.
//! Native code and guest share only dedicated code/stack/input/scratch backing.
//! CR3/descriptors are deliberately replaced; SIMD/TLS/system-state continuity
//! is outside this ABI. The guest allocator sees GPAs, not firmware HPAs.
use crate::{memory, print, xstate};
use core::ptr;
use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{EvidenceFlag, ValidatedCapabilities},
        descriptors::GuestDescriptorRequest,
    },
    boot::ownership::OwnershipRecord,
    guest::{
        continuation::IntegerContinuation,
        pages::{GuestPages, PagePermissions as GuestPermission, TableStorage as GuestStorage},
        state::GuestStateRequest,
    },
    memory::npt::{
        Npt, NptEvidence, PagePermissions as NestedPermission, TableStorage as NestedStorage,
    },
    svm::vmcb::Vmcb,
};
#[repr(C, align(4096))]
struct Page([u8; 4096]);
static mut STACK: Page = Page([0; 4096]);
static mut INPUT: Page = Page([0; 4096]);
static mut SCRATCH: Page = Page([0; 4096]);
static mut GDT: Page = Page([0; 4096]);
static mut TSS: Page = Page([0; 4096]);
static mut GUEST_TABLES: GuestStorage = GuestStorage([[0; 4096]; 4]);
static mut NESTED_TABLES: NestedStorage = NestedStorage([[0; 4096]; 8]);
unsafe extern "C" {
    static loader_fixture_start: u8;
    static loader_fixture_end: u8;
    static loader_resume: u8;
    static loader_allocation_refused: u8;
    static loader_fail: u8;
    fn native_loader_capture(capture: *mut IntegerContinuation, stack_top: u64, input: u64);
}
/// # Safety
/// Single stopped emulator CPU after successful EBS, with private host mappings
/// and owned static image. All guest execution stopped; static references do not
/// survive native capture or VMRUN. APM3.44 15.5.1/15.5.2 and UEFI2.11 7.4.6.
pub unsafe fn run(
    control: *mut Vmcb,
    record: &OwnershipRecord<'_>,
    caps: &ValidatedCapabilities,
    state: &xstate::State,
    clock: &crate::clock::State,
) {
    let policy = caps.address_policy();
    let code = core::hint::black_box(ptr::addr_of!(loader_fixture_start) as u64);
    let end = core::hint::black_box(ptr::addr_of!(loader_fixture_end) as u64);
    assert_eq!(end - code, 4096);
    assert_eq!(code & 4095, 0);
    let stack = ptr::addr_of_mut!(STACK).cast::<u8>();
    let input = ptr::addr_of_mut!(INPUT).cast::<u8>();
    let scratch = core::hint::black_box(ptr::addr_of_mut!(SCRATCH).cast::<u8>());
    let gdt = ptr::addr_of_mut!(GDT).cast::<u8>();
    let tss = ptr::addr_of_mut!(TSS).cast::<u8>();
    let guest_storage = ptr::addr_of_mut!(GUEST_TABLES);
    let nested_storage = ptr::addr_of_mut!(NESTED_TABLES);
    let window = code & !0x1fffff;
    let descriptors = GuestDescriptorRequest {
        gdt_base: gdt as u64,
        tss_base: tss as u64,
        rsp0: stack as u64 + 4096,
        ist1: stack as u64 + 4096,
    }
    .validate()
    .unwrap();
    unsafe {
        ptr::copy_nonoverlapping(descriptors.gdt().as_ptr(), gdt, descriptors.gdt().len());
        ptr::copy_nonoverlapping(descriptors.tss().as_ptr(), tss, descriptors.tss().len());
    }
    let mappings = [
        (code, 0x1000, NestedPermission::ReadExecute),
        (stack as u64, 0x8000, NestedPermission::ReadWrite),
        (input as u64, 0x3000, NestedPermission::ReadOnly),
        (scratch as u64, 0x20000, NestedPermission::ReadWrite),
        (gdt as u64, 0x4000, NestedPermission::ReadWrite),
        (tss as u64, 0x5000, NestedPermission::ReadWrite),
    ];
    let guest_cr3 = {
        let mut pages =
            GuestPages::new_in_window(unsafe { &mut *guest_storage }, 0x10000, policy, window)
                .unwrap();
        for (va, gpa, permission) in mappings {
            pages
                .map_page(
                    va,
                    gpa,
                    if permission == NestedPermission::ReadExecute || gpa == 0x3000 {
                        GuestPermission::ReadOnly
                    } else {
                        GuestPermission::ReadWrite
                    },
                )
                .unwrap();
        }
        pages.root_address()
    };
    let nested_root = {
        let mut npt = Npt::new(
            unsafe { &mut *nested_storage },
            nested_storage as u64,
            policy,
            policy.physical_bits().min(48),
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
        .unwrap();
        for (hpa, gpa, permission) in mappings {
            npt.map_page(gpa, hpa, permission).unwrap();
        }
        for index in 0..4 {
            npt.map_page(
                0x10000 + index * 4096,
                guest_storage as u64 + index * 4096,
                NestedPermission::ReadWrite,
            )
            .unwrap();
        }
        memory::audit_guest_backing(
            &npt,
            policy,
            record,
            &[
                code,
                stack as u64,
                input as u64,
                scratch as u64,
                gdt as u64,
                tss as u64,
                guest_storage as u64,
                guest_storage as u64 + 4096,
                guest_storage as u64 + 8192,
                guest_storage as u64 + 12288,
            ],
        );
        npt.root_address()
    };
    let prepared = memory::Prepared {
        guest_cr3,
        nested_root,
        descriptors,
        mmio_mapping: None,
    };
    for round in 0..19 {
        let reserved = round == 16;
        let negative = round >= 16;
        unsafe {
            ptr::write_bytes(stack, 0, 4096);
            ptr::write_bytes(input, 0, 4096);
            ptr::write_bytes(scratch, 0, 4096);
        }
        // Entire exposed GPA domain is reserved except one controlled scratch
        // page. This includes the host arena GPA reservation from the final map.
        let domain_end = (record.arena().last_byte() + 1).max(0x200000);
        let mut fields = [
            0x31495041444c5653u64,
            3,
            0,
            0x20,
            0,
            0x20000,
            1,
            if reserved { 0 } else { 7 },
            0x21000,
            (domain_end - 0x21000) / 4096,
            0,
            scratch as u64,
            0x20000,
            0, // Filled with actual captured flags before guest entry.
        ];
        for (index, value) in fields.into_iter().enumerate() {
            unsafe {
                ptr::write_unaligned(input.add(index * 8).cast::<u64>(), value);
            }
        }
        let mut captured = IntegerContinuation::default();
        unsafe {
            native_loader_capture(&mut captured, stack as u64 + 4096, input as u64);
        }
        captured
            .validate_bounds(
                policy.validate(code, 4096, 4096).unwrap(),
                policy.validate(stack as u64, 4096, 4096).unwrap(),
            )
            .unwrap();
        assert_eq!(captured.rip, ptr::addr_of!(loader_resume) as u64);
        assert_eq!(captured.rsp, stack as u64 + 4096 - 16);
        assert_eq!(captured.registers.rdi, input as u64);
        assert_eq!(captured.rax, 0x1122334455667788);
        assert_ne!(captured.rflags & 1, 0);
        // Independent intentional corruption controls must fail in guest code.
        if round == 17 {
            captured.registers.rcx ^= 1;
        }
        if round == 18 {
            captured.rflags &= !1;
        }
        fields[13] = captured.rflags;
        unsafe {
            ptr::write_unaligned(input.add(104).cast::<u64>(), captured.rflags);
        }
        let guest = GuestStateRequest {
            rip: captured.rip,
            rsp: captured.rsp,
            rflags: captured.rflags,
            cr0: 0x80010033,
            cr3: guest_cr3,
            cr4: state.guest_cr4(),
            efer: 0x1500,
            rax: captured.rax,
        }
        .validate_continuation_with_xstate(&policy, state.layout())
        .unwrap();
        // Capture record is consumed into the stopped VMCB/frame, never reused
        // as an authoritative live guest register copy after entry.
        let mut frame = captured.registers;
        drop(captured);
        unsafe {
            state.reset(round);
            crate::execution::initialize(control, &prepared, caps, &guest, None);
            state.run(control, &mut frame, 0, clock);
        }
        let v = unsafe { &*control };
        assert_eq!(v.exit_snapshot().code, if negative { 0x78 } else { 0x81 });
        assert_eq!(
            u64::from_le_bytes(v.bytes()[0x5d8..0x5e0].try_into().unwrap()),
            stack as u64 + 4096 - 16
        );
        if negative {
            assert_eq!(
                v.exit_snapshot().rip,
                if reserved {
                    ptr::addr_of!(loader_allocation_refused) as u64
                } else {
                    ptr::addr_of!(loader_fail) as u64
                }
            );
        }
        for index in 0..4096 {
            let expected = if index < fields.len() * 8 {
                fields[index / 8].to_le_bytes()[index % 8]
            } else {
                0
            };
            assert_eq!(unsafe { ptr::read_volatile(input.add(index)) }, expected);
        }
        let written = unsafe { ptr::read_volatile(scratch.cast::<u64>()) };
        assert_eq!(written, if negative { 0 } else { 0x434f4e54494e5545 });
        if !negative {
            assert_eq!(v.guest_rax(), 1);
        }
        for index in 8..4096 {
            assert_eq!(unsafe { ptr::read_volatile(scratch.add(index)) }, 0);
        }
        // Capture call and pushfq may use 32 bytes below the two ABI words.
        // No caller host frame is ever placed on this guest stack.
        for index in 0..4096 - 64 {
            assert_eq!(unsafe { ptr::read_volatile(stack.add(index)) }, 0);
        }
    }
    print("PASS captured-loader-continuation=16 same-rip-rsp-flags\n");
    print("PASS guest-map-allocation reserved-refusal\n");
    print("PASS captured-state-negative-controls\n");
}
