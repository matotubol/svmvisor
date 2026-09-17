//! Fixed returning synthetic guest construction shared by native and TCG callers.
//!
//! This module builds bounded bytes in already-owned memory. Its views are not
//! allocation, cache, encryption, CPU-ownership, or native-admission evidence.
//! See docs/native-guest-resources-contract.md for the caller's actual gates.
use core::{arch::x86_64::__cpuid_count, ptr};
use svmvisor_dxe::native::{
    admission::boundary::NativeBoundary,
    transition::{canary::TransitionCanary, state::*},
};
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::EvidenceFlag, descriptors::GuestDescriptorRequest},
    guest::{
        pages::{GuestPages, PagePermissions as GPerm, TableStorage as GTables},
        state::GuestStateRequest,
    },
    memory::{
        address::{AddressPolicy, EncryptionState},
        npt::{Npt, NptEvidence, PagePermissions as NPerm, TableStorage as NTables},
    },
    svm::vmcb::Vmcb,
};

pub const ARENA_PAGES: usize = 33;
pub const ARENA_BYTES: usize = ARENA_PAGES * 4096;
pub const COOKIE: u64 = 0x53564d4e41544956;
pub const GUEST_GPRS: [u64; 14] = [
    0x301, 0x302, 0x303, 0x304, 0x305, 0x306, 0x308, 0x309, 0x30a, 0x30b, 0x30c, 0x30d, 0x30e,
    0x30f,
];
// The event-test feature is rejected in combination with native-returning by
// the package feature guard. Production uses only the finite VMMCALL sequence.
pub const VMMCALL_RIP: u64 = if cfg!(feature = "native-transition-event-test") {
    0x10be
} else {
    0x1096
};

// Exact, bounded integer-only program. tests/fixtures/native_transition_multi/guest.S
// independently assembles these bytes and the five fixed lookup arrays. No
// relocation, guest-supplied address or dynamic execution target is accepted.
pub const MULTI_GUEST_BYTES: usize = 0x700;
pub const MULTI_CPUID_RIP: u64 = 0x1086;
pub const MULTI_QUERY_RIP: u64 = 0x10bf;
pub const MULTI_STOP_RIP: u64 = 0x10ff;
pub const MULTI_FAIL_RIP: u64 = 0x1102;
pub const MULTI_CPUID_LEAVES: [u32; 8] = [
    0, 1, 0x40000000, 0x40000001, 0x80000000, 0x80000001, 0xdeadbeef, 0x40000002,
];
pub const MULTI_CPUID_OUTPUTS: [[u64; 4]; 8] = [
    [1, 0x566d7653, 0x74736554, 0x726f7369],
    [0, 0, 0x80000000, 0x60],
    [0x40000001, 0x566d7653, 0x726f7369, 0x74736554],
    [1, 0, 0, 0],
    [0x80000001, 0, 0, 0],
    [0, 0, 0, 0x20000000],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
];
pub const MULTI_FINAL_GPRS: [u64; 14] = [
    32,
    32,
    0,
    0x6d75000000000304,
    0x6d75000000000305,
    0x6d75000000000306,
    0x6d75000000000308,
    0x6d75000000000309,
    0x6d7500000000030a,
    0x6d7500000000030b,
    0x6d7500000000030c,
    0x6d7500000000030d,
    0x6d7500000000030e,
    32,
];
const MULTI_CODE: [u8; 260] = [
    0x48, 0xbd, 0x04, 0x03, 0x00, 0x00, 0x00, 0x00, 0x75, 0x6d, 0x48, 0xbe, 0x05, 0x03, 0x00, 0x00,
    0x00, 0x00, 0x75, 0x6d, 0x48, 0xbf, 0x06, 0x03, 0x00, 0x00, 0x00, 0x00, 0x75, 0x6d, 0x49, 0xb8,
    0x08, 0x03, 0x00, 0x00, 0x00, 0x00, 0x75, 0x6d, 0x49, 0xb9, 0x09, 0x03, 0x00, 0x00, 0x00, 0x00,
    0x75, 0x6d, 0x49, 0xba, 0x0a, 0x03, 0x00, 0x00, 0x00, 0x00, 0x75, 0x6d, 0x49, 0xbb, 0x0b, 0x03,
    0x00, 0x00, 0x00, 0x00, 0x75, 0x6d, 0x49, 0xbc, 0x0c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x75, 0x6d,
    0x49, 0xbd, 0x0d, 0x03, 0x00, 0x00, 0x00, 0x00, 0x75, 0x6d, 0x49, 0xbe, 0x0e, 0x03, 0x00, 0x00,
    0x00, 0x00, 0x75, 0x6d, 0x45, 0x31, 0xff, 0x4a, 0x8b, 0x04, 0xfd, 0x00, 0x12, 0x00, 0x00, 0x48,
    0xb9, 0x00, 0x00, 0x00, 0x00, 0x44, 0x33, 0x22, 0x11, 0x4c, 0x89, 0xfa, 0x48, 0xbb, 0x03, 0x03,
    0x00, 0x00, 0xaa, 0x99, 0x88, 0x77, 0x0f, 0xa2, 0x4a, 0x3b, 0x04, 0xfd, 0x00, 0x13, 0x00, 0x00,
    0x75, 0x70, 0x4a, 0x3b, 0x1c, 0xfd, 0x00, 0x14, 0x00, 0x00, 0x75, 0x66, 0x4a, 0x3b, 0x0c, 0xfd,
    0x00, 0x15, 0x00, 0x00, 0x75, 0x5c, 0x4a, 0x3b, 0x14, 0xfd, 0x00, 0x16, 0x00, 0x00, 0x75, 0x52,
    0x48, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x31, 0xc9, 0x4c, 0x89, 0xfa, 0x0f,
    0x01, 0xd9, 0x48, 0x83, 0xf8, 0x01, 0x75, 0x3a, 0x49, 0xff, 0xc7, 0x4c, 0x89, 0x3c, 0x25, 0x00,
    0x80, 0x00, 0x00, 0x49, 0x83, 0xff, 0x20, 0x75, 0x8e, 0x48, 0xb8, 0x56, 0x49, 0x54, 0x41, 0x4e,
    0x4d, 0x56, 0x53, 0x48, 0x89, 0x04, 0x25, 0x08, 0x80, 0x00, 0x00, 0x48, 0xb8, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0xb9, 0x20, 0x00, 0x00, 0x00, 0xba, 0x20, 0x00, 0x00, 0x00, 0x0f,
    0x01, 0xd9, 0x0f, 0x0b,
];

/// Supplied CPUID observations, not authenticated capabilities. No MSR reads.
#[derive(Clone, Copy, Debug)]
pub struct ConstructionInputs {
    pub physical_bits: u8,
    pub max_extended: u32,
    pub extended1_ecx: u32,
    pub extended1_edx: u32,
    pub svm_edx: u32,
    pub asid_count: u32,
}

/// Observe actual CPUID on the calling CPU. Native entry must separately bind
/// this CPU, the immutable boundary, current CR3, and actual cache snapshot.
pub fn read_cpuid_inputs(physical_bits: u8) -> ConstructionInputs {
    let max_extended = __cpuid_count(0x80000000, 0).eax;
    let mut inputs = ConstructionInputs {
        physical_bits,
        max_extended,
        extended1_ecx: 0,
        extended1_edx: 0,
        svm_edx: 0,
        asid_count: 0,
    };
    if max_extended >= 0x80000001 {
        let leaf = __cpuid_count(0x80000001, 0);
        inputs.extended1_ecx = leaf.ecx;
        inputs.extended1_edx = leaf.edx;
    }
    if max_extended >= 0x8000000a && inputs.extended1_ecx & (1 << 2) != 0 {
        let leaf = __cpuid_count(0x8000000a, 0);
        inputs.svm_edx = leaf.edx;
        inputs.asid_count = leaf.ebx;
    }
    inputs
}

struct ArenaView {
    va: *mut u8,
    pa: u64,
}
impl ArenaView {
    // All call sites use compile-time bounded indices within the 33-page arena.
    fn va(&self, index: usize) -> *mut u8 {
        unsafe { self.va.add(index * 4096) }
    }
    fn pa(&self, index: usize) -> u64 {
        self.pa + index as u64 * 4096
    }
}

/// Non-owning construction view, with no authority to call the transition.
pub struct InitializedGuest {
    arena: ArenaView,
    boundary: *const NativeBoundary,
    multi_exit: bool,
}
/// Non-owning pointers into the same live arena. Never free through this view.
pub struct BoundGuest {
    arena: ArenaView,
    multi_exit: bool,
}

fn validate_inputs(
    va: *mut u8,
    pa: u64,
    boundary: &NativeBoundary,
    inputs: ConstructionInputs,
) -> Result<AddressPolicy, u64> {
    if va.is_null()
        || va as usize & 4095 != 0
        || (va as usize).checked_add(ARENA_BYTES).is_none()
        || pa < 0x100000
        || pa & 4095 != 0
        || pa > (1u64 << 32) - ARENA_BYTES as u64
    {
        return Err(6);
    }
    if !boundary.has_valid_shape()
        || boundary.cr0 & 0x80000001 != 0x80000001
        || boundary.cr4 & (1 << 5) == 0
        || boundary.efer & ((1 << 10) | (1 << 11)) != ((1 << 10) | (1 << 11))
        || boundary.cr4 & (1 << 12) != 0
        || inputs.max_extended < 0x8000000a
        || inputs.extended1_ecx & (1 << 2) == 0
        || inputs.extended1_edx & (1 << 20) == 0
        || inputs.svm_edx & 1 == 0
        || inputs.asid_count <= 1
        || (boundary.profile == 7 && boundary.avx_offset != 576)
    {
        return Err(2);
    }
    // Numeric proposal only. The native caller MUST establish actual disabled
    // address encryption through the retained live snapshot before any SVM use.
    let policy = AddressPolicy::new(
        inputs.physical_bits,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .map_err(|_| 6u64)?;
    policy
        .validate(pa, ARENA_BYTES as u64, 4096)
        .map_err(|_| 6u64)?;
    if inputs.physical_bits < 32 {
        return Err(6);
    }
    Ok(policy)
}

/// Initialize every owned page before retained mappings settle their A/D bits.
///
/// # Safety
/// `arena_va` must name exclusive, writable, aligned ordinary RAM of exactly
/// ARENA_BYTES or more, live through every later bound use. `arena_pa` is its
/// proposed contiguous system address; the caller must qualify that proposal
/// against actual allocation/mapping/cache/encryption evidence before SVM use.
/// The immutable boundary must remain at this address through binding/use.
/// No running CPU may use these guest tables or VMCBs while they are changed.
// Keep construction temporaries outside the sampled native caller frame.
#[cfg_attr(feature = "native-returning", inline(never))]
pub unsafe fn initialize(
    arena_va: *mut u8,
    arena_pa: u64,
    boundary: &NativeBoundary,
    inputs: ConstructionInputs,
) -> Result<InitializedGuest, u64> {
    unsafe { initialize_inner(arena_va, arena_pa, boundary, inputs, false) }
}

/// Construct the fixed 32-round CPUID/query/STOP guest in the same owned arena.
/// The legacy one-entry and event fixtures remain separate constructor choices.
///
/// # Safety
/// Every safety requirement of `initialize` applies unchanged.
#[cfg_attr(feature = "native-returning", inline(never))]
pub unsafe fn initialize_multi_exit(
    arena_va: *mut u8,
    arena_pa: u64,
    boundary: &NativeBoundary,
    inputs: ConstructionInputs,
) -> Result<InitializedGuest, u64> {
    unsafe { initialize_inner(arena_va, arena_pa, boundary, inputs, true) }
}

#[cfg_attr(feature = "native-returning", inline(never))]
unsafe fn initialize_inner(
    arena_va: *mut u8,
    arena_pa: u64,
    boundary: &NativeBoundary,
    inputs: ConstructionInputs,
    multi_exit: bool,
) -> Result<InitializedGuest, u64> {
    if multi_exit && cfg!(feature = "native-transition-event-test") {
        return Err(12);
    }
    let policy = validate_inputs(arena_va, arena_pa, boundary, inputs)?;
    let arena = ArenaView {
        va: arena_va,
        pa: arena_pa,
    };
    unsafe { ptr::write_bytes(arena_va, 0, ARENA_BYTES) };
    let page = |index| arena.va(index);
    let pa = |index| arena.pa(index);
    let physical_bits = inputs.physical_bits;
    // Change every captured GPR so observation proves guest-produced values.
    // movabs COOKIE,RAX; movabs RCX..R15; VMMCALL; no stack/xstate use.
    let mut code = [0u8; 193];
    code[0] = 0x48;
    code[1] = 0xb8;
    code[2..10].copy_from_slice(&COOKIE.to_le_bytes());
    for (index, ((rex, opcode), value)) in [
        (0x48, 0xb9),
        (0x48, 0xba),
        (0x48, 0xbb),
        (0x48, 0xbd),
        (0x48, 0xbe),
        (0x48, 0xbf),
        (0x49, 0xb8),
        (0x49, 0xb9),
        (0x49, 0xba),
        (0x49, 0xbb),
        (0x49, 0xbc),
        (0x49, 0xbd),
        (0x49, 0xbe),
        (0x49, 0xbf),
    ]
    .into_iter()
    .zip(GUEST_GPRS)
    .enumerate()
    {
        for (dst, byte) in code
            .iter_mut()
            .skip(10 + index * 10)
            .zip([rex, opcode].into_iter().chain(value.to_le_bytes()))
        {
            *dst = byte;
        }
    }
    #[cfg(feature = "native-transition-event-test")]
    {
        // Real guest READY store, then spin on a separately owned release
        // byte. The controller must observe READY and the actual spin RIP.
        for (dst, byte) in code.iter_mut().skip(150).zip(
            [0x48, 0xb8]
                .into_iter()
                .chain(0x53564d4556454e54u64.to_le_bytes())
                .chain([0x48, 0xa3])
                .chain(0x8000u64.to_le_bytes())
                .chain([0x48, 0xb8])
                .chain(COOKIE.to_le_bytes())
                .chain([0x80, 0x3c, 0x25, 0x08, 0x80, 0, 0, 1, 0x75, 0xf6]),
        ) {
            *dst = byte;
        }
    }
    for (dst, byte) in code
        .iter_mut()
        .skip((VMMCALL_RIP - 0x1000) as usize)
        .zip([0x0f, 0x01, 0xd9])
    {
        *dst = byte;
    }
    unsafe {
        if multi_exit {
            ptr::copy_nonoverlapping(MULTI_CODE.as_ptr(), page(7), MULTI_CODE.len());
            for round in 0..32 {
                let index = round & 7;
                field(
                    page(7),
                    0x200 + round * 8,
                    (0xaabbccdd00000000 | u64::from(MULTI_CPUID_LEAVES[index])).to_le_bytes(),
                );
                for register in 0..4 {
                    field(
                        page(7),
                        0x300 + register * 0x100 + round * 8,
                        MULTI_CPUID_OUTPUTS[index][register].to_le_bytes(),
                    );
                }
            }
        } else {
            ptr::copy_nonoverlapping(code.as_ptr(), page(7), code.len());
        }
    }
    let descriptors = GuestDescriptorRequest {
        gdt_base: 0x4000,
        tss_base: 0x5000,
        rsp0: 0x9000,
        ist1: 0xb000,
    }
    .validate()
    .map_err(|_| 7u64)?;
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
        let mut guest =
            GuestPages::new(unsafe { &mut *page(12).cast::<GTables>() }, 0x10000, policy)
                .map_err(|_| 8u64)?;
        guest
            .map_page(0x1000, 0x1000, GPerm::ReadOnly)
            .map_err(|_| 8u64)?;
        for address in [0x4000, 0x5000, 0x8000, 0xa000] {
            guest
                .map_page(address, address, GPerm::ReadWrite)
                .map_err(|_| 8u64)?;
        }
        guest.root_address()
    };
    let nested_root = {
        let mut nested = Npt::new(
            unsafe { &mut *page(16).cast::<NTables>() },
            pa(16),
            policy,
            physical_bits.min(48),
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
        )
        .map_err(|_| 9u64)?;
        nested
            .map_page(0x1000, pa(7), NPerm::ReadExecute)
            .map_err(|_| 9u64)?;
        for (gpa, index) in [(0x4000, 10), (0x5000, 11), (0x8000, 8), (0xa000, 9)] {
            nested
                .map_page(gpa, pa(index), NPerm::ReadWrite)
                .map_err(|_| 9u64)?;
        }
        for i in 0..4 {
            nested
                .map_page(0x10000 + i * 4096, pa(12 + i as usize), NPerm::ReadWrite)
                .map_err(|_| 9u64)?;
        }
        nested.root_address()
    };
    unsafe {
        ptr::write_bytes(page(25), 0xff, 5 * 4096);
    }
    let vmcb = unsafe { &mut *page(0).cast::<Vmcb>() };
    vmcb.set_nested_root(nested_root, &policy)
        .map_err(|_| 10u64)?;
    vmcb.set_permission_maps(pa(25), pa(28), &policy)
        .map_err(|_| 10u64)?;
    vmcb.set_synthetic_state(
        &GuestStateRequest {
            rip: 0x1000,
            rsp: 0x9000,
            rflags: 2,
            cr0: 0x80010033,
            cr3: guest_cr3,
            cr4: 0x20,
            efer: 0x1500,
            rax: 0,
        }
        .validate(&policy)
        .map_err(|_| 11u64)?,
    );
    vmcb.set_guest_descriptors(&descriptors);
    unsafe {
        // Literal reviewed controls, after the supplied CPUID observations
        // establish that ASID1 and NPT exist. This is construction only.
        field(page(0), 0x000, u32::MAX.to_le_bytes());
        field(page(0), 0x004, u32::MAX.to_le_bytes());
        field(page(0), 0x008, u32::MAX.to_le_bytes());
        let intercepts = 0x1800000bu32 | if multi_exit { (1 << 18) | (1 << 24) } else { 0 };
        field(page(0), 0x00c, intercepts.to_le_bytes());
        field(page(0), 0x010, 0x207fu32.to_le_bytes());
        field(page(0), 0x058, 1u32.to_le_bytes());
        field(page(0), 0x05c, [1u8]);
        field(page(0), 0x060, 0x1000000u64.to_le_bytes());
        field(page(0), 0x090, 1u64.to_le_bytes());
        field(page(0), 0x560, 0x400u64.to_le_bytes());
        field(page(0), 0x568, 0xffff0ff0u64.to_le_bytes());
        field(page(0), 0x668, 0x0007040600070406u64.to_le_bytes());
    }

    Ok(InitializedGuest {
        arena,
        boundary,
        multi_exit,
    })
}

impl InitializedGuest {
    /// Bind into preinitialized pages 30/31 after retained tables captured the
    /// immutable GDT copy. No allocation, first page use, or firmware call.
    ///
    /// # Safety
    /// All initialize safety requirements still hold. `gdt` must be the exact
    /// current retained immutable GDT and remain live through the transition
    /// and verification. Call before the callback-free execution interval.
    pub unsafe fn bind(
        self,
        boundary: &NativeBoundary,
        gdt: &[u8],
        mode: u64,
    ) -> Result<BoundGuest, u64> {
        if !ptr::eq(self.boundary, boundary)
            || !boundary.has_valid_shape()
            || gdt.is_empty()
            || gdt.len() != usize::from(boundary.gdtr.limit()) + 1
            || if self.multi_exit {
                mode != mode::MULTI_EXIT
            } else {
                !matches!(mode, mode::ONE_ENTRY | mode::BIND_ONLY)
            }
        {
            return Err(12);
        }
        let page = |index| self.arena.va(index);
        let pa = |index| self.arena.pa(index);
        let expected = unsafe { &mut *page(31).cast::<ScalarState>() };
        expected.captured_fields = capture::CORE;
        expected.cr0 = boundary.cr0;
        expected.cr3 = boundary.cr3;
        expected.cr4 = boundary.cr4;
        expected.efer = boundary.efer;
        expected.gdtr.bytes = boundary.gdtr.bytes;
        expected.idtr.bytes = boundary.idtr.bytes;
        expected.selectors = [
            boundary.cs,
            boundary.ss,
            boundary.ds,
            boundary.es,
            boundary.fs,
            boundary.gs,
            boundary.ldtr,
            boundary.tr,
        ];
        let context = unsafe { &mut *page(30).cast::<NativeTransition>() };
        context.inputs = TransitionInputs {
            abi_version: ABI_VERSION,
            context_bytes: CONTEXT_BYTES as u64,
            image_boundary_va: boundary as *const NativeBoundary as u64,
            guest_vmcb_pa: pa(0),
            guest_vmcb_va: page(0) as u64,
            host_extra_pa: pa(1),
            host_extra_va: page(1) as u64,
            restored_extra_pa: pa(2),
            restored_extra_va: page(2) as u64,
            guest_extra_pa: pa(24),
            guest_extra_va: page(24) as u64,
            hsave_pa: pa(3),
            original_xstate_va: page(4) as u64,
            restored_xstate_va: page(5) as u64,
            guest_xstate_va: page(6) as u64,
            xstate_profile: boundary.profile,
            xstate_bytes: boundary.xstate_size,
            expected_vmmcall_rip: if self.multi_exit {
                MULTI_STOP_RIP
            } else {
                VMMCALL_RIP
            },
            expected_vmmcall_rax: if self.multi_exit { 1 } else { COOKIE },
            expected_bsp_apic_id: u64::from(boundary.leaf1_ebx >> 24),
            expected_state_va: expected as *const ScalarState as u64,
            host_gdt_copy_va: gdt.as_ptr() as u64,
            host_gdt_bytes: gdt.len() as u64,
            mode,
        };

        Ok(BoundGuest {
            arena: self.arena,
            multi_exit: self.multi_exit,
        })
    }
}

impl BoundGuest {
    pub fn context(&self) -> *mut NativeTransition {
        self.arena.va(30).cast::<NativeTransition>()
    }
    pub fn canary(&self) -> *mut TransitionCanary {
        self.arena.va(32).cast::<TransitionCanary>()
    }
    /// Read the actual guest-produced completion payload, never host counters.
    ///
    /// # Safety
    /// The owned arena remains live and no guest or other writer is active.
    pub unsafe fn multi_completion(&self) -> [u64; 2] {
        unsafe {
            [
                ptr::read_volatile(self.arena.va(8).cast::<u64>()),
                ptr::read_volatile(self.arena.va(8).add(8).cast::<u64>()),
            ]
        }
    }

    /// # Safety
    /// The owned arena and its capture pages must still be live, immutable to
    /// other writers, and contain the actual completed transition observations.
    pub unsafe fn verify_observations(&self, context: &NativeTransition) -> Result<u64, u64> {
        unsafe { verify_observations(self, context) }
    }
}

unsafe fn field<const N: usize>(base: *mut u8, offset: usize, bytes: [u8; N]) {
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(offset), N) };
}

const _: () = assert!(core::mem::size_of::<NativeTransition>() <= 4096);
const _: () = assert!(core::mem::size_of::<ScalarState>() <= 4096);
const _: () = assert!(core::mem::size_of::<TransitionCanary>() <= 4096);
const _: () = assert!(core::mem::size_of::<GTables>() == 4 * 4096);
const _: () = assert!(core::mem::size_of::<NTables>() == 8 * 4096);

// Prove the deterministic xstate seeds differ from the actual saved caller
// image. This prevents an enclosing restoration from accidentally matching them.
pub fn changed_canary_components(canary: &TransitionCanary) -> u64 {
    let mut changed = 0;
    if canary.original_xstate.get(160..416) != canary.seeded_xstate.get(160..416) {
        changed |= 1;
    }
    if canary.original_xstate.get(..5) != canary.seeded_xstate.get(..5)
        || (0..8).any(|slot| {
            let offset = 32 + slot * 16;
            canary.original_xstate.get(offset..offset + 10)
                != canary.seeded_xstate.get(offset..offset + 10)
        })
    {
        changed |= 2;
    }
    if canary.profile == 7
        && canary.original_xstate.get(576..832) != canary.seeded_xstate.get(576..832)
    {
        changed |= 4;
    }
    changed
}

/// Independent adapter comparisons of retained actual captures, before free.
/// Raw no-#MF x87 pointers and empty register payloads are not asserted here.
unsafe fn verify_observations(
    fixture: &BoundGuest,
    context: &NativeTransition,
) -> Result<u64, u64> {
    if context.original != context.restored {
        return Err(30);
    }
    let original = unsafe { core::slice::from_raw_parts(fixture.arena.va(4), 1024) };
    let restored = unsafe { core::slice::from_raw_parts(fixture.arena.va(5), 1024) };
    if original.get(..5) != restored.get(..5)
        || original.get(24..28) != restored.get(24..28)
        || original.get(160..416) != restored.get(160..416)
    {
        return Err(31);
    }
    let fsw = original
        .get(2..4)
        .and_then(|bytes| bytes.first_chunk::<2>())
        .copied()
        .ok_or(31u64)?;
    let top = (u16::from_le_bytes(fsw) >> 11) & 7;
    if u16::from_le_bytes(fsw) & (1 << 7) != 0 && original.get(6..24) != restored.get(6..24) {
        return Err(32);
    }
    let tag = *original.get(4).ok_or(31u64)?;
    for slot in 0..8 {
        if tag & (1 << ((top as usize + slot) & 7)) != 0 {
            let offset = 32 + slot * 16;
            if original.get(offset..offset + 10) != restored.get(offset..offset + 10) {
                return Err(32);
            }
        }
    }
    if context.inputs.xstate_profile == 7 && original.get(576..832) != restored.get(576..832) {
        return Err(33);
    }
    let extra_original = unsafe { core::slice::from_raw_parts(fixture.arena.va(1), 4096) };
    let extra_restored = unsafe { core::slice::from_raw_parts(fixture.arena.va(2), 4096) };
    for (offset, size) in [
        (0x440, 16),
        (0x450, 16),
        (0x470, 16),
        (0x490, 16),
        (0x600, 64),
    ] {
        if extra_original.get(offset..offset + size) != extra_restored.get(offset..offset + size) {
            return Err(34);
        }
    }
    let mut checks = 7; // Scalar, xstate and VMSAVE-image comparisons completed.
    if fixture.multi_exit != (context.inputs.mode == mode::MULTI_EXIT) {
        return Err(37);
    }
    if context.inputs.mode == mode::MULTI_EXIT && context.journal.outcome == outcome::MULTI_EXIT {
        if context.guest.exit_code != 0x81
            || context.guest.exit_int_info & (1 << 31) != 0
            || context.guest.rip != MULTI_STOP_RIP
            || context.guest.rsp != 0x9000
            || context.guest.rax != 1
            || context.guest.gprs != MULTI_FINAL_GPRS
            || context.guest.captured_fields != guest_capture::ALL
            || context.journal.vmrun_attempts != 65
            || context.journal.completed_exits != 65
            || context.guest.reserved[0..4] != [32, 32, 64, 0]
            || context.guest.reserved[5] != 3
            || context.guest.reserved[6] > 1
            || (context.guest.reserved[6] == 1 && context.guest.reserved[4] != MULTI_STOP_RIP + 3)
            || context.guest.reserved[7..] != [0, 0]
            || unsafe { fixture.multi_completion() } != [32, COOKIE]
        {
            return Err(37);
        }
        checks |= 8;
    }
    if context.inputs.mode == 0 && context.journal.outcome == outcome::VMMCALL {
        if context.guest.exit_code != 0x81
            || context.guest.rip != VMMCALL_RIP
            || context.guest.rax != COOKIE
            || context.guest.gprs != GUEST_GPRS
            || context.guest.captured_fields != guest_capture::ALL
            || context.journal.vmrun_attempts != 1
            || context.journal.completed_exits != 1
        {
            return Err(35);
        }
        checks |= 8;
    }
    #[cfg(feature = "native-transition-event-test")]
    if context.inputs.mode == 0 && matches!(context.journal.outcome, outcome::NMI | outcome::INIT) {
        let expected_exit = if context.journal.outcome == outcome::NMI {
            0x61
        } else {
            0x63
        };
        if context.guest.exit_code != expected_exit
            || !matches!(context.guest.rip, 0x10b4 | 0x10bc)
            || context.guest.rax != COOKIE
            || context.guest.gprs != GUEST_GPRS
            || context.guest.captured_fields != guest_capture::ALL
            || context.journal.vmrun_attempts != 1
            || context.journal.completed_exits != 1
        {
            return Err(36);
        }
        checks |= 8;
    }
    Ok(checks)
}
