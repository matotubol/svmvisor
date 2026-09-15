//! Physically installed checked tables for the disposable TCG fixture.
//!
//! Host bootstrap identity mapping makes the static addresses HPAs here. Guest
//! virtual addresses and GPAs are deliberately distinct from those addresses.
//! No host executable page or host page-table page is mapped into the guest.

use core::ptr;
use svmvisor_hypervisor::{
    address::AddressPolicy,
    descriptors::{GuestDescriptorRequest, ValidatedGuestDescriptors},
    guest_pages::{GuestPages, PagePermissions as GuestPermission, TableStorage as GuestStorage},
    npt::{Npt, NptEvidence, PagePermissions as NestedPermission, TableStorage as NestedStorage},
    svm::xapic::FixtureMmioMapping,
};

const PAGE_BYTES: usize = 4096;
const GUEST_TABLE_BASE: u64 = 0x10000;
pub const APIC_ALIAS: u64 = 0xc000;

#[repr(C, align(4096))]
struct Page([u8; PAGE_BYTES]);

static mut GUEST_TABLES: GuestStorage = GuestStorage([[0; PAGE_BYTES]; 4]);
static mut NESTED_TABLES: NestedStorage = NestedStorage([[0; PAGE_BYTES]; 8]);
static mut CODE: Page = Page([0; PAGE_BYTES]);
static mut STACK: Page = Page([0; PAGE_BYTES]);
static mut IST: Page = Page([0; PAGE_BYTES]);
static mut GDT: Page = Page([0; PAGE_BYTES]);
static mut TSS: Page = Page([0; PAGE_BYTES]);
static mut IDT: Page = Page([0; PAGE_BYTES]);
// A guest-shared data page distinct from both CPUs' private ordinary stacks.
// Host observations use atomics; no ordinary Rust reference aliases live guest writes.
#[cfg(feature = "concurrent-smp")]
#[repr(C, align(4096))]
struct SharedPage([core::sync::atomic::AtomicU64; 512]);
#[cfg(feature = "concurrent-smp")]
static SHARED: SharedPage = SharedPage([const { core::sync::atomic::AtomicU64::new(0) }; 512]);

#[cfg(feature = "concurrent-smp")]
pub fn concurrent_word(index: usize) -> u64 {
    SHARED.0[index].load(core::sync::atomic::Ordering::Acquire)
}

pub struct Prepared {
    pub guest_cr3: u64,
    pub nested_root: u64,
    pub descriptors: ValidatedGuestDescriptors,
    pub mmio_mapping: Option<FixtureMmioMapping>,
}

/// Borrow bytes from immutable installed guest code. NPT denies guest writes;
/// other admitted CPUs may concurrently execute/read the same immutable page.
/// # Safety
/// The caller owns its stopped guest. No host may reinitialize CODE until all
/// instruction borrows and guest execution users have ended. RIP and extent
/// must identify installed guest code; every running guest must retain read-only
/// NPT access to this backing. AMD APM2rev3.44 15.25 nested access permissions.
pub unsafe fn installed_instruction(rip: u64, length: usize) -> &'static [u8] {
    let offset = usize::try_from(rip.checked_sub(0x1000).unwrap()).unwrap();
    assert!(offset.checked_add(length).unwrap() <= PAGE_BYTES);
    unsafe { core::slice::from_raw_parts(ptr::addr_of!(CODE).cast::<u8>().add(offset), length) }
}

/// Clear mutable fixture memory between stopped sessions.
/// Caller must own the single stopped guest and retain no buffer references.
pub unsafe fn reset_session() {
    unsafe {
        ptr::write_bytes(ptr::addr_of_mut!(STACK).cast::<u8>(), 0, PAGE_BYTES);
        ptr::write_bytes(ptr::addr_of_mut!(IST).cast::<u8>(), 0, PAGE_BYTES);
        assert_eq!(
            ptr::read_volatile(ptr::addr_of!(STACK).cast::<u8>().add(0xff0).cast::<u64>()),
            0
        );
    }
}

/// Record the expected hardware interrupt return RIP in the owned data page.
/// # Safety
/// Single stopped guest, no live STACK borrows; GPA 8000h is reserved by the
/// preemption fixture and disjoint from its downward-growing stack.
pub unsafe fn set_preemption_return_rip(rip: u64) {
    unsafe {
        ptr::write_volatile(ptr::addr_of_mut!(STACK).cast::<u64>(), rip);
    }
}

/// Verify the reserved return-RIP slot, then reuse the ordinary stack/IST audit.
/// # Safety
/// Exclusive stopped preemption guest; expected is the last host-owned slot write.
pub unsafe fn verify_preemption_stack(expected: u64) {
    unsafe {
        assert_eq!(
            ptr::read_volatile(ptr::addr_of!(STACK).cast::<u64>()),
            expected
        );
        set_preemption_return_rip(0);
        verify_exception_stack();
    }
}

/// Check the guest's write and ensure it changed no other stack/IST bytes.
/// Caller must have stopped the guest with exclusive ownership of these pages.
pub unsafe fn verify_session() {
    let marker = 0x53564d534553534eu64.to_le_bytes();
    for index in 0..PAGE_BYTES {
        let expected = if (0xff0..0xff8).contains(&index) {
            marker[index - 0xff0]
        } else {
            0
        };
        unsafe {
            assert_eq!(
                ptr::read_volatile(ptr::addr_of!(STACK).cast::<u8>().add(index)),
                expected
            );
            assert_eq!(
                ptr::read_volatile(ptr::addr_of!(IST).cast::<u8>().add(index)),
                0
            );
        }
    }
}

/// Materialize the bounded guest mappings before guest execution.
///
/// # Safety
/// The caller must own these static buffers exclusively and have stopped the
/// guest. No outstanding references or hardware table walks may access them.
/// The bootstrap must identity-map the entire static image as writable normal
/// RAM; the supplied evidence must establish the host four-level/NXE mode and
/// NX support. GMET, SSS and encryption must be disabled. `guest_code` must be
/// immutable source bytes disjoint from every destination static buffer.
/// Reinitialization requires invalidating relevant TLB/ASID state before the
/// next guest entry. No references to the buffers escape this function.
pub unsafe fn prepare(
    policy: AddressPolicy,
    guest_code: &[u8],
    ownership: Option<&svmvisor_hypervisor::boot::ownership::OwnershipRecord<'_>>,
    evidence: NptEvidence,
) -> Prepared {
    assert!(!guest_code.is_empty() && guest_code.len() <= PAGE_BYTES);
    let code = ptr::addr_of_mut!(CODE).cast::<u8>();
    let stack = ptr::addr_of_mut!(STACK).cast::<u8>();
    let ist = ptr::addr_of_mut!(IST).cast::<u8>();
    let gdt = ptr::addr_of_mut!(GDT).cast::<u8>();
    let tss = ptr::addr_of_mut!(TSS).cast::<u8>();
    let idt = ptr::addr_of_mut!(IDT).cast::<u8>();
    let guest_storage = ptr::addr_of_mut!(GUEST_TABLES);
    let nested_storage = ptr::addr_of_mut!(NESTED_TABLES);
    let guest_hpa = guest_storage as u64;
    let nested_hpa = nested_storage as u64;
    // Validate actual backing extents as well as builder-assigned GPAs.
    for base in [code, stack, ist, gdt, tss, idt] {
        policy
            .validate(base as u64, PAGE_BYTES as u64, PAGE_BYTES as u64)
            .unwrap();
    }
    policy
        .validate(guest_hpa, 4 * PAGE_BYTES as u64, PAGE_BYTES as u64)
        .unwrap();
    policy
        .validate(nested_hpa, 8 * PAGE_BYTES as u64, PAGE_BYTES as u64)
        .unwrap();
    unsafe {
        for page in [code, stack, ist, gdt, tss, idt] {
            ptr::write_bytes(page, 0, PAGE_BYTES);
        }
        ptr::copy_nonoverlapping(guest_code.as_ptr(), code, guest_code.len());
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
        ptr::copy_nonoverlapping(descriptors.gdt().as_ptr(), gdt, descriptors.gdt().len());
        ptr::copy_nonoverlapping(descriptors.tss().as_ptr(), tss, descriptors.tss().len());
    }
    let mut pages =
        GuestPages::new(unsafe { &mut *guest_storage }, GUEST_TABLE_BASE, policy).unwrap();
    let guest_cr3 = {
        pages
            .map_page(0x1000, 0x1000, GuestPermission::ReadOnly)
            .unwrap();
        for address in [0x3000, 0x4000, 0x5000, 0x6000, 0x8000, 0xa000] {
            pages
                .map_page(address, address, GuestPermission::ReadWrite)
                .unwrap();
        }
        #[cfg(feature = "concurrent-smp")]
        pages.map_page(0xd000, 0xd000, GuestPermission::ReadWrite).unwrap();
        // 0x7000 is the existing guest #PF guard; 0x6000 is the NPF probe.
        assert_eq!(pages.translate(APIC_ALIAS), Ok(None));
        pages
            .map_page(APIC_ALIAS, 0xfee0_0000, GuestPermission::ReadWrite)
            .unwrap();
        pages.root_address()
    };
    let (nested_root, mmio_mapping) = {
        let mut npt = Npt::new(
            unsafe { &mut *nested_storage },
            nested_hpa,
            policy,
            policy.physical_bits().min(48),
            evidence,
        )
        .unwrap();
        npt.map_page(0x1000, code as u64, NestedPermission::ReadExecute)
            .unwrap();
        #[cfg(feature = "concurrent-smp")]
        {
            // Keep the actual page base as the only absolute relocation. LLVM
            // otherwise unrolls this into (SHARED + 4096 + lane) with a negative
            // loop index: effective stores stay in bounds, but lane displacements
            // alone escape the image and rightly fail the strict relocation
            // packager. An opaque bounded reference preserves atomic stores and
            // their exact extent without manufacturing out-of-image relocations.
            let shared = core::hint::black_box(&SHARED.0);
            for word in shared {
                word.store(0, core::sync::atomic::Ordering::Relaxed);
            }
            npt.map_page(0xd000, ptr::addr_of!(SHARED) as u64, NestedPermission::ReadWrite).unwrap();
        }
        for (gpa, backing) in [
            (0x3000, idt),
            (0x4000, gdt),
            (0x5000, tss),
            (0x8000, stack),
            (0xa000, ist),
        ] {
            npt.map_page(gpa, backing as u64, NestedPermission::ReadWrite)
                .unwrap();
        }
        // Page walkers need write access for accessed/dirty updates. These
        // table pages have no ordinary guest virtual mappings and are NX.
        for index in 0..4 {
            npt.map_page(
                GUEST_TABLE_BASE + index * PAGE_BYTES as u64,
                guest_hpa + index * PAGE_BYTES as u64,
                NestedPermission::ReadWrite,
            )
            .unwrap();
        }
        // VA/GPA 0x6000 is deliberately present only in guest paging: access
        // must reach an NPT missing-leaf fault rather than a guest page fault.
        assert_eq!(npt.translate(0x6000), Ok(None));
        assert_eq!(npt.translate(0xfee0_0000), Ok(None));
        let mmio_mapping = FixtureMmioMapping::admit(&pages, &npt, APIC_ALIAS, 0x1000).unwrap();
        #[cfg(feature = "concurrent-smp")]
        let shared_page=ptr::addr_of!(SHARED) as u64;
        #[cfg(not(feature = "concurrent-smp"))]
        let shared_page=0;
        let allowed = [
                    code as u64,
                    stack as u64,
                    ist as u64,
                    gdt as u64,
                    tss as u64,
                    idt as u64,
                    guest_hpa,
                    guest_hpa + 4096,
                    guest_hpa + 8192,
                    guest_hpa + 12288, shared_page,
                ];
        let allowed=&allowed[..10+usize::from(cfg!(feature="concurrent-smp"))];
        if let Some(record) = ownership {
            audit_guest_backing(&npt, policy, record, &allowed);
            crate::print("PASS resident-guest-exclusion\n");
        } else {
            audit_exact_backing(&npt,&allowed);
        }
        (npt.root_address(), mmio_mapping)
    };
    Prepared {
        guest_cr3,
        nested_root,
        descriptors,
        mmio_mapping: Some(mmio_mapping),
    }
}

/// Install the three owned long-mode interrupt gates used by the continuation
/// fixture. APM vol.2 rev.3.44 section 8.9: 16-byte gates, selector 8, IST=0.
///
/// # Safety
/// Single stopped guest; no live references or hardware accesses to IDT.
/// Handler addresses must refer to the mapped immutable guest code page.
pub unsafe fn install_exception_idt(handlers: [u64; 3], reject_pf: bool) {
    unsafe {
        install_idt(&[
            (6, handlers[0], true),
            (13, handlers[1], true),
            (14, handlers[2], !reject_pf),
        ]);
    }
}

/// Install bounded fixture interrupt gates in the existing owned IDT page.
/// APM vol.2 rev.3.44 section 8.9: selector 8, interrupt gate, DPL0, IST0.
/// # Safety
/// Single stopped guest with exclusive IDT ownership; addresses identify the
/// immutable mapped guest code page. No references survive guest entry.
pub unsafe fn install_idt(gates: &[(u8, u64, bool)]) {
    let idt = ptr::addr_of_mut!(IDT).cast::<u8>();
    unsafe {
        ptr::write_bytes(idt, 0, PAGE_BYTES);
    }
    for &(vector, address, present) in gates {
        assert!((0x1000..0x2000).contains(&address));
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(address as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&8u16.to_le_bytes());
        gate[5] = if present { 0x8e } else { 0x0e };
        gate[6..8].copy_from_slice(&((address >> 16) as u16).to_le_bytes());
        gate[8..12].copy_from_slice(&((address >> 32) as u32).to_le_bytes());
        unsafe {
            ptr::copy_nonoverlapping(gate.as_ptr(), idt.add(vector as usize * 16), 16);
        }
    }
}

/// Verify exception delivery stayed within the top 64 stack bytes and never
/// touched the separate IST (all three gates use IST=0).
/// # Safety
/// Exclusive stopped guest; no outstanding stack/IST references or accesses.
pub unsafe fn verify_exception_stack() {
    unsafe { verify_stack_extent(64, false) };
}

/// Audit nested IRQ and exception frames, including 16-byte hardware alignment
/// and the bounded guest APIC helper call, confined to the top 128 bytes.
/// # Safety
/// Exclusive stopped guest and stack/IST ownership, no outstanding borrows.
pub unsafe fn verify_nested_exception_stack() {
    unsafe { verify_stack_extent(128, false) };
}

/// Place one already checked IDT gate at GPA afa0h in the otherwise unused IST
/// backing. IDTR af20h makes #DF select this gate and #PF select unmapped b000h.
/// # Safety
/// Exclusive stopped guest; IDT[0] contains the desired immutable-code gate.
/// No gate in this fixture selects IST, and all actual frames use STACK.
pub unsafe fn install_boundary_df_gate() {
    unsafe {
        ptr::copy_nonoverlapping(
            ptr::addr_of!(IDT).cast::<u8>(),
            ptr::addr_of_mut!(IST).cast::<u8>().add(0xfa0),
            16,
        )
    };
}

/// # Safety
/// Same stopped ownership as install_boundary_df_gate; IDT[0] remains unchanged.
pub unsafe fn verify_boundary_delivery_stack() {
    unsafe { verify_stack_extent(64, true) };
}

unsafe fn verify_stack_extent(extent: usize, boundary_gate: bool) {
    for index in 0..PAGE_BYTES {
        unsafe {
            if index < PAGE_BYTES - extent {
                assert_eq!(
                    ptr::read_volatile(ptr::addr_of!(STACK).cast::<u8>().add(index)),
                    0
                );
            }
            let expected = if boundary_gate && (0xfa0..0xfb0).contains(&index) {
                ptr::read_volatile(ptr::addr_of!(IDT).cast::<u8>().add(index - 0xfa0))
            } else {
                0
            };
            assert_eq!(
                ptr::read_volatile(ptr::addr_of!(IST).cast::<u8>().add(index)),
                expected
            );
        }
    }
}

/// Audit the complete single-window NPT against explicit shared guest backing.
/// All table references are temporary and must end before a guest entry.
pub fn audit_guest_backing(
    npt: &Npt<'_>,
    policy: AddressPolicy,
    record: &svmvisor_hypervisor::boot::ownership::OwnershipRecord<'_>,
    allowed: &[u64],
) {
    let arena = record.arena();
    for gpa in (arena.base()..=arena.last_byte()).step_by(PAGE_BYTES) {
        assert!(record.excludes_guest_range(policy.validate(gpa, 4096, 4096).unwrap()));
        assert_eq!(npt.translate(gpa), Ok(None));
    }
    if let Some(smp)=record.smp() {
        assert!(record.excludes_guest_range(smp.low_page()));
        assert_eq!(npt.translate(smp.low_page().base()),Ok(None),"SIPI reservation aliases fixture GPA");
    }
    for &hpa in allowed {
        assert!(hpa >= arena.base() && hpa + 4096 <= arena.last_byte() + 1);
    }
    audit_exact_backing(npt, allowed);
}

/// Prove that the complete NPT has exactly this guest-backing allowlist.
/// Parent entries restrict all mappings to one 2MiB GPA window; every leaf is
/// then enumerated. No borrowed tables or entries survive this pre-entry audit.
fn audit_exact_backing(npt: &Npt<'_>, allowed: &[u64]) {
    assert!(!allowed.is_empty() && allowed.len() < 64);
    for (index, &hpa) in allowed.iter().enumerate() {
        assert_eq!(hpa & 4095, 0);
        assert!(!allowed[..index].contains(&hpa));
    }
    assert_eq!(npt.used_tables(), 4);
    for parent in 0..3 {
        let table = npt.table(parent).unwrap();
        let child = npt.table(parent + 1).unwrap().physical_address;
        for (index, bytes) in table.bytes.chunks_exact(8).enumerate() {
            assert_eq!(
                u64::from_le_bytes(bytes.try_into().unwrap()),
                if index == 0 { child | 7 } else { 0 }
            );
        }
    }
    let mut mapped = 0;
    let mut seen = 0u64;
    for gpa in (0..0x200000).step_by(PAGE_BYTES) {
        if let Some(translation) = npt.translate(gpa).unwrap() {
            let index = allowed.iter().position(|&hpa| hpa == translation.host_address).unwrap();
            assert_eq!(seen & (1 << index), 0, "duplicate guest backing alias");
            seen |= 1 << index;
            mapped += 1;
        }
    }
    assert_eq!(mapped, allowed.len());
    assert_eq!(seen, (1 << allowed.len()) - 1);
}

/// Add a 32-bit code descriptor for the real SIPI trampoline; reuse mapped GDT.
/// # Safety
/// Both vCPUs stopped, no outstanding references. GPA A000 now exclusively
/// supplies the AP ordinary stack; no fixture gate may select IST1.
pub unsafe fn prepare_smp_descriptors() {
    unsafe {
        ptr::write_unaligned(
            ptr::addr_of_mut!(GDT).cast::<u8>().add(40).cast::<u64>(),
            0x00cf9b000000ffff,
        )
    };
}
/// Check both stacks outside the admitted top128-byte frame/helper extent.
/// # Safety
/// Both vCPUs stopped and owned; no borrows/entries may overlap this audit.
pub unsafe fn verify_smp_stacks() {
    for index in 0..PAGE_BYTES - 128 {
        unsafe {
            assert_eq!(
                ptr::read_volatile(ptr::addr_of!(STACK).cast::<u8>().add(index)),
                0
            );
            assert_eq!(
                ptr::read_volatile(ptr::addr_of!(IST).cast::<u8>().add(index)),
                0
            );
        }
    }
}
