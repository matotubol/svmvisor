//! Two-CPU AMD CPUID and dynamic XCR0 conformance, software emulator only.
//! Each host CPU exclusively owns its indexed guest state; immutable code and
//! page tables are shared. No arbitrary guest input or device passthrough.
use crate::{clock, execution, field, hex, host_smp, memory, print, xstate};
use core::{
    arch::x86_64::{__cpuid, __cpuid_count},
    ptr,
};
use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::{EvidenceFlag, ValidatedCapabilities},
        registers::GuestRegisters,
    },
    guest::state::GuestStateRequest,
    memory::npt::NptEvidence,
    svm::{
        cpu_model::{
            AmdCpuModel, CpuIdentity, GuestCpuState, HostCacheEvidence, HostCpuEvidence,
            RuntimeCpuContract,
        },
        dispatch::{DispatchOutcome, handle_exit_with_cpu_model},
        vmcb::Vmcb,
    },
};
// CPUID; VMMCALL checkpoint; XSETBV; XGETBV; VMMCALL checkpoint.
// Exact bytes have immutable guest/NPT read-only backing for the whole run.
const CODE: &[u8] = &[
    0x0f, 0xa2, 0x0f, 0x01, 0xd9, 0x0f, 0x01, 0xd1, 0x0f, 0x01, 0xd0, 0x0f, 0x01, 0xd9, 0x44, 0x0f,
    0x57, 0x3d, 10, 0, 0, 0, 0x0f, 0x01, 0xd9, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];
static mut CONTROLS: [Vmcb; 2] = [Vmcb::new(), Vmcb::new()];
static mut EXTENDED: [xstate::GuestState; 2] =
    [xstate::GuestState::new(), xstate::GuestState::new()];
#[derive(Clone, Copy)]
struct Shared {
    prepared: *const memory::Prepared,
    caps: *const ValidatedCapabilities,
    model: *const AmdCpuModel,
}
static mut SHARED: Shared = Shared {
    prepared: ptr::null(),
    caps: ptr::null(),
    model: ptr::null(),
};
#[derive(Clone, Copy)]
struct Metrics {
    queries: u64,
    entries: u64,
    refused: u64,
    xsetbv: u64,
    digest: u64,
}
static mut METRICS: [Metrics; 2] = [Metrics {
    queries: 0,
    entries: 0,
    refused: 0,
    xsetbv: 0,
    digest: 0xcbf29ce484222325,
}; 2];
fn word(v: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(v.bytes()[offset..offset + 8].try_into().unwrap())
}
fn registers(value: core::arch::x86_64::CpuidResult) -> [u32; 4] {
    [value.eax, value.ebx, value.ecx, value.edx]
}
// Executed directly in the boot-time host before any guest VMRUN. On native
// hardware this instruction queries that CPU; TCG supplies an emulated CPU.
// Neither Windows APIs nor a saved Windows inventory feed guest identity.
fn capture_identity() -> CpuIdentity {
    let basic = __cpuid(0);
    let extended = __cpuid(0x80000000);
    CpuIdentity::from_leaves(
        registers(basic),
        (basic.eax >= 1).then(|| registers(__cpuid(1))),
        registers(extended),
        (extended.eax >= 0x80000001).then(|| registers(__cpuid(0x80000001))),
        (extended.eax >= 0x80000004).then(|| {
            [
                registers(__cpuid(0x80000002)),
                registers(__cpuid(0x80000003)),
                registers(__cpuid(0x80000004)),
            ]
        }),
    )
    .unwrap()
}
fn host_evidence() -> HostCpuEvidence {
    let v = __cpuid(0);
    let e = __cpuid(0x80000000);
    let b = __cpuid(1);
    let f = if v.eax >= 7 {
        __cpuid_count(7, 0)
    } else {
        core::arch::x86_64::CpuidResult {
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
        }
    };
    assert!(e.eax >= 0x80000008);
    let x = __cpuid(0x80000001);
    let a = __cpuid(0x80000008);
    let mut caches = HostCacheEvidence {
        legacy_l1: registers(__cpuid(0x80000005)),
        legacy_l2_l3: registers(__cpuid(0x80000006)),
        deterministic: [[0; 4]; 8],
        deterministic_count: 0,
    };
    if e.eax >= 0x8000001d && x.ecx & (1 << 22) != 0 {
        let mut terminated = false;
        for index in 0..8 {
            let cache = registers(__cpuid_count(0x8000001d, index));
            if cache[0] & 31 == 0 {
                terminated = true;
                break;
            }
            caches.deterministic[index as usize] = cache;
            caches.deterministic_count += 1;
        }
        assert!(
            terminated,
            "native cache enumeration exceeds admitted bound"
        );
    }
    let mut vendor = [0; 12];
    vendor[0..4].copy_from_slice(&v.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&v.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&v.ecx.to_le_bytes());
    HostCpuEvidence {
        caches,
        extended21_eax: if e.eax >= 0x80000021 {
            __cpuid(0x80000021).eax
        } else {
            0
        },
        extended21_ebx: if e.eax >= 0x80000021 {
            __cpuid(0x80000021).ebx
        } else {
            0
        },
        vendor,
        max_basic: v.eax,
        max_extended: e.eax,
        leaf1_ecx: b.ecx,
        leaf1_edx: b.edx,
        leaf7_ebx: f.ebx,
        leaf7_ecx: f.ecx,
        leaf7_edx: f.edx,
        extended8_ebx: a.ebx,
        extended1_ecx: x.ecx,
        extended1_edx: x.edx,
        address_sizes: a.eax,
        clflush_bytes: (((b.ebx >> 8) & 255) * 8) as u16,
    }
}
extern "C" fn ap_entry() {
    unsafe {
        let state = xstate::State::install();
        let clock = clock::State::capture().for_guest(1);
        drive(1, &state, &clock);
    }
}
/// # Safety
/// Flat Multiboot BSP owns disposable emulator RAM and SVM. Shared backing is
/// initialized before host_smp publication and retained until join. APM2 ch15.
pub unsafe fn run(caps: &ValidatedCapabilities, state: &xstate::State, clock: &clock::State) {
    unsafe {
        host_smp::prepare(None);
    }
    let prepared = unsafe {
        memory::prepare(
            caps.address_policy(),
            CODE,
            None,
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
    };
    let evidence = host_evidence();
    let model = AmdCpuModel::admit(
        evidence,
        RuntimeCpuContract {
            identity: capture_identity(),
            xstate: state.layout(),
            physical_address_bits: evidence.address_sizes as u8,
            tsc: true,
            rdtscp: clock.capabilities().rdtscp(),
            nx: false,
        },
    )
    .unwrap();
    unsafe {
        memory::prepare_smp_descriptors();
        SHARED = Shared {
            prepared: &prepared,
            caps,
            model: &model,
        };
        host_smp::start(ap_entry);
        drive(0, state, &clock.for_guest(0));
        host_smp::join();
    }
    for id in 0..2 {
        let m = unsafe { ptr::addr_of!(METRICS).cast::<Metrics>().add(id).read() };
        for (name, value) in [
            ("queries", m.queries),
            ("entries", m.entries),
            ("refused", m.refused),
            ("xsetbv", m.xsetbv),
            ("digest", m.digest),
        ] {
            print("AMD-CPU cpu=");
            print(if id == 0 { "0 " } else { "1 " });
            print(name);
            print("=");
            hex(value);
        }
    }
    print("PASS amd-cpu-model two-cpu-real-cpuid-native-vendor-no-hypervisor-leaves\n");
    print("PASS amd-xcr0 dynamic-owned-state-and-unchanged-refusals\n");
}
unsafe fn drive(id: usize, state: &xstate::State, clock: &clock::State) {
    let shared = unsafe { ptr::addr_of!(SHARED).read() };
    let prepared = unsafe { &*shared.prepared };
    let caps = unsafe { &*shared.caps };
    let v = unsafe { &mut *ptr::addr_of_mut!(CONTROLS).cast::<Vmcb>().add(id) };
    let extended = unsafe {
        &mut *ptr::addr_of_mut!(EXTENDED)
            .cast::<xstate::GuestState>()
            .add(id)
    };
    let metrics = unsafe { &mut *ptr::addr_of_mut!(METRICS).cast::<Metrics>().add(id) };
    let evidence = host_evidence();
    let model = AmdCpuModel::admit(
        evidence,
        RuntimeCpuContract {
            identity: capture_identity(),
            xstate: state.layout(),
            physical_address_bits: evidence.address_sizes as u8,
            tsc: true,
            rdtscp: clock.capabilities().rdtscp(),
            nx: false,
        },
    )
    .unwrap();
    assert_eq!(
        model,
        unsafe { *shared.model },
        "both CPUs must admit the same package model"
    );
    let guest = GuestStateRequest {
        rip: 0x1000,
        rsp: if id == 0 { 0x9000 } else { 0xb000 },
        rflags: 2,
        cr0: 0x80010033,
        cr3: prepared.guest_cr3,
        cr4: state.guest_cr4(),
        efer: 0x1500,
        rax: 0,
    }
    .validate_with_xstate(&caps.address_policy(), state.layout())
    .unwrap();
    unsafe {
        execution::initialize(v, prepared, caps, &guest, None);
        state.reset_owned(extended, id);
    }
    let mut frame = GuestRegisters::default();
    for mask in [state.layout().mask(), 1, 3, state.layout().mask()] {
        if state.uses_xsave() {
            unsafe { set_mask(v, &mut frame, extended, state, clock, mask, metrics) };
        }
        // Legacy SSE is still executable with XCR0.SSE clear. Write XMM15,
        // cross an exit, and toggle it back on a second entry; the bridge's
        // complete guest/host canaries check both writes and disabled AVX state.
        if mask == 1 && state.uses_xsave() {
            for _ in 0..2 {
                unsafe {
                    field(v, 0x578, 0x100eu64.to_le_bytes());
                    state.run_owned(v, &mut frame, 3, clock, extended)
                };
                assert_eq!(v.exit_snapshot().code, 0x81);
                assert_eq!(v.guest_rip(), 0x1016);
                metrics.entries += 1;
            }
        }
        for leaf in (0..=0x20)
            .chain(0x80000000..=0x80000028)
            .chain([0x40000000, 0x40000001, 0x7fffffff, 0x800000ff, 0xffffffff])
        {
            for sub in [0, 1, 2, 3, 63, u32::MAX] {
                unsafe {
                    query(
                        v, &mut frame, extended, state, clock, &model, id, leaf, sub, metrics,
                    )
                };
            }
        }
    }
    if state.uses_xsave() {
        let old_cr4 = word(v, 0x548);
        unsafe {
            field(v, 0x548, (old_cr4 & !(1 << 18)).to_le_bytes());
        }
        for leaf in [1, 0x0d] {
            unsafe {
                query(
                    v, &mut frame, extended, state, clock, &model, id, leaf, 0, metrics,
                )
            };
        }
        unsafe {
            field(v, 0x548, old_cr4.to_le_bytes());
        }
    }
    if state.uses_xsave() {
        for bad in [0, 2, 4, 8, 1u64 << 63] {
            unsafe {
                field(v, 0x578, 0x1005u64.to_le_bytes());
            }
            v.set_guest_rax(bad as u32 as u64);
            frame.rdx = bad >> 32;
            frame.rcx = 0;
            unsafe { state.run_owned(v, &mut frame, 0, clock, extended) };
            metrics.entries += 1;
            assert_eq!(v.exit_snapshot().code, 0x8d);
            let before = *v.bytes();
            let saved = frame;
            let old = extended.guest_xcr0();
            assert!(
                state
                    .set_guest_xcr0(extended, word(v, 0x548), 0, 0, bad)
                    .is_err()
            );
            assert_eq!(v.bytes(), &before);
            assert_eq!(frame, saved);
            assert_eq!(extended.guest_xcr0(), old);
            metrics.refused += 1;
        }
    }
}
unsafe fn query(
    v: &mut Vmcb,
    frame: &mut GuestRegisters,
    extended: &mut xstate::GuestState,
    state: &xstate::State,
    clock: &clock::State,
    model: &AmdCpuModel,
    id: usize,
    leaf: u32,
    sub: u32,
    m: &mut Metrics,
) {
    unsafe {
        field(v, 0x578, 0x1000u64.to_le_bytes());
    }
    v.set_guest_rax(0xa5a5a5a500000000 | u64::from(leaf));
    frame.rcx = 0x5a5a5a5a00000000 | u64::from(sub);
    frame.rbx = u64::MAX;
    frame.rdx = u64::MAX;
    unsafe { state.run_owned(v, frame, 0, clock, extended) };
    m.entries += 1;
    assert_eq!(v.exit_snapshot().code, 0x72);
    assert_eq!(v.guest_rip(), 0x1000);
    let stopped = GuestCpuState {
        vcpu_id: id as u32,
        cr4: word(v, 0x548),
        xcr0: extended.guest_xcr0(),
    };
    let expected = model.cpuid(leaf, sub, stopped).unwrap();
    let flags = word(v, 0x570);
    let original = *v.bytes();
    let registers = *frame;
    assert!(
        handle_exit_with_cpu_model(v.exit_snapshot(), v, frame, &[0x90, 0x90], model, stopped)
            .is_err()
    );
    assert_eq!(v.bytes(), &original);
    assert_eq!(*frame, registers);
    m.refused += 1;
    assert_eq!(
        handle_exit_with_cpu_model(
            v.exit_snapshot(),
            v,
            frame,
            unsafe { memory::installed_instruction(0x1000, 2) },
            model,
            stopped
        ),
        Ok(DispatchOutcome::ResumePrepared)
    );
    assert_eq!(word(v, 0x570), flags);
    unsafe { state.run_owned(v, frame, 0, clock, extended) };
    m.entries += 1;
    assert_eq!(v.exit_snapshot().code, 0x81);
    assert_eq!(v.guest_rip(), 0x1002);
    assert_eq!(
        [v.guest_rax(), frame.rbx, frame.rcx, frame.rdx],
        expected.map(u64::from)
    );
    if leaf == 0 {
        assert_eq!(
            [expected[1], expected[3], expected[2]],
            [0x68747541, 0x69746e65, 0x444d4163]
        );
    }
    if leaf == 1 {
        assert_eq!(expected[2] >> 31, 0);
    }
    if leaf == 0x40000000 || leaf == 0x40000001 {
        assert_eq!(expected, [0; 4]);
    }
    for value in [
        leaf,
        sub,
        expected[0],
        expected[1],
        expected[2],
        expected[3],
    ] {
        for byte in value.to_le_bytes() {
            m.digest = (m.digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    m.queries += 1;
}
unsafe fn set_mask(
    v: &mut Vmcb,
    frame: &mut GuestRegisters,
    extended: &mut xstate::GuestState,
    state: &xstate::State,
    clock: &clock::State,
    mask: u64,
    m: &mut Metrics,
) {
    unsafe {
        field(v, 0x578, 0x1005u64.to_le_bytes());
    }
    v.set_guest_rax(mask);
    frame.rcx = 0;
    frame.rdx = 0;
    unsafe { state.run_owned(v, frame, 0, clock, extended) };
    m.entries += 1;
    let snapshot = v.exit_snapshot();
    assert_eq!(snapshot.code, 0x8d);
    let next = snapshot
        .xsetbv_continuation(unsafe { memory::installed_instruction(snapshot.rip, 3) })
        .unwrap();
    assert_eq!(v.event_injection(), 0);
    state
        .set_guest_xcr0(extended, word(v, 0x548), 0, frame.rcx as u32, mask)
        .unwrap();
    // XSETBV leaves operands and flags unchanged; only the validated next RIP
    // and this guest's XCR0 change. Checkpoint then executes native XGETBV.
    unsafe {
        field(v, 0x578, next.address().to_le_bytes());
        state.run_owned(v, frame, 0, clock, extended)
    };
    m.entries += 1;
    assert_eq!(v.exit_snapshot().code, 0x81);
    assert_eq!(v.guest_rip(), 0x100b);
    assert_eq!(v.guest_rax() | (frame.rdx << 32), mask);
    m.xsetbv += 1;
}
