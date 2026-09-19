//! Image preparation and arm: the private host root, descriptors and directory
//! (`prepare`), CPU capability admission and the one-time arm of this CPU.

use core::{
    arch::{asm, x86_64::__cpuid_count},
    ptr,
};

use crate::{
    arch::x86_64::{
        apic::{self, DoorbellTarget, HostX2Apic, PhysicalX2Apic},
        capabilities::{
            CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures, ValidatedCapabilities,
        },
        encryption::NativeEncryptionPlan,
        msr::{
            MMIO_CFG_BASE_ADDR, SYS_CFG_MTRR_FIX_DRAM_MOD_EN, TARGET_SIGNATURE, VM_CR,
            VM_CR_R_INIT, VM_CR_SVMDIS,
        },
    },
    boot::memory::{MAX_DESCRIPTORS, MemoryDescriptor, ValidatedMemoryMap},
    guest::continuation::NativeBootstrapAck,
    host::{
        descriptors::HostDescriptorRequest,
        resident::{
            DIRECTORY_VERSION, ResidentDirectory,
            runtime::{
                ASSIGNED_APIC_ID, AUX, AVIC_BACKING, CONTEXT, FAULT_STACK, FRAME, GDT_TSS, GDTR,
                HOST_EXTRA, HSAVE, IDT, IDTR, IOPM, IRQ_GATES, MSRPM, NMI_STACK, NPT,
                PHYSICAL_BITS, POOL, RAM, RAM_COUNT, STACK, STATE, TABLES, VMCB, cache, data_start,
                diagnostics, image_bss_end, image_start,
                msr::{read_msr, write_msr},
                svmvisor_resident_enter, svmvisor_resident_fault_offsets,
                svmvisor_resident_irq_offsets, svmvisor_resident_nmi, svmvisor_resident_sx,
                text_end,
            },
            terminal::TerminalEndpoint,
        },
    },
    svm::{
        dispatch::NativeEfer,
        permission_maps::{MsrAccess, Msrpm, Permission},
        x2avic::{
            GUEST_APIC_VERSION, NativeX2AvicProfile, PhysicalIdTable, X2AvicCapabilities,
            registers::{CapturedInterface, GuestX2Apic},
            startup::{NativeDestinationMode, NativeIcr, NativeStartupMailbox, try_lock_routes},
        },
    },
};

/// # Safety
/// Call once in a legal loaded raw runtime allocation under live native CPL0
/// identity mappings. Caller has validated the entire 1MiB backing RW/X/WB,
/// original stack and this exact linked package. No CPU control is changed.
/// `output` is disjoint writable caller storage. Firmware owns allocation.
/// Every dense pool slot holds a relocated copy of this same package; the
/// private root's remote backing aliases depend on that before arm.
pub unsafe extern "win64" fn prepare(
    base: u64,
    output: *mut ResidentDirectory,
    pool_base: u64,
    pool_bytes: u64,
    cpu_slot: u64,
    apic_id: u64,
) -> u64 {
    let start = ptr::addr_of!(image_start) as u64;
    let text_limit = ptr::addr_of!(text_end) as u64;
    let data = ptr::addr_of!(data_start) as u64;
    let end = ptr::addr_of!(image_bss_end) as u64;
    let backing = ptr::addr_of!(AVIC_BACKING) as u64;
    if output.is_null()
        || !super::is_valid_pool_slot(base, pool_base, pool_bytes, cpu_slot, apic_id)
        || base != start
        || base < 0x100000
        || base & 4095 != 0
        || base + 0x100000 > 0x40000000
        || base >> 21 != (base + 0xfffff) >> 21
        || text_limit <= base
        || text_limit > data
        || data > end
        || end > base + super::X2AVIC_BACKING_ALIASES_OFFSET
        || (text_limit | data | end | backing) & 4095 != 0
        || backing < data
        || backing + 4096 > end
    {
        return 1;
    }
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if state.prepared {
        return 2;
    }
    let root = ptr::addr_of_mut!(TABLES) as u64;
    let stack = ptr::addr_of_mut!(STACK) as u64;
    let fault_stack = ptr::addr_of_mut!(FAULT_STACK) as u64;
    let nmi_stack = ptr::addr_of_mut!(NMI_STACK) as u64;
    let gdt = ptr::addr_of_mut!(GDT_TSS) as u64;
    let idt = ptr::addr_of_mut!(IDT) as u64;
    let stack_top = stack + 17 * 4096;
    let fault_top = fault_stack + 5 * 4096;
    let nmi_top = nmi_stack + 2 * 4096;
    let fault_offsets = ptr::addr_of!(svmvisor_resident_fault_offsets);
    let mut handlers = core::array::from_fn(|vector| {
        (fault_offsets as u64).wrapping_add(unsafe { (*fault_offsets)[vector] } as i64 as u64)
    });
    handlers[30] = svmvisor_resident_sx as *const () as u64;
    // Vector 2: the returning host NMI gate (irq.S) on IST2, replacing the
    // terminal fault stub, so a platform NMI in a host GIF window is swallowed
    // and re-presented to the guest as V_NMI (15.21.10 p536) instead of
    // stopping this CPU.
    handlers[2] = svmvisor_resident_nmi as *const () as u64;
    let irq_offsets = ptr::addr_of!(svmvisor_resident_irq_offsets);
    for vector in (0..256).filter(|&vector| window_gate(vector)) {
        handlers[vector] =
            (irq_offsets as u64).wrapping_add(unsafe { (*irq_offsets)[vector - 16] } as i64 as u64);
    }
    let request = HostDescriptorRequest {
        gdt_base: gdt,
        tss_base: gdt + 128,
        idt_base: idt,
        rsp0: stack_top,
        ist1: fault_top,
        ist2: nmi_top,
        handlers,
    };
    let Ok(descriptors) = request.validate_terminal_ist() else {
        return 3;
    };
    unsafe {
        ptr::copy_nonoverlapping(
            descriptors.gdt().as_ptr(),
            gdt as *mut u8,
            descriptors.gdt().len(),
        );
        ptr::copy_nonoverlapping(
            descriptors.tss().as_ptr(),
            (gdt + 128) as *mut u8,
            descriptors.tss().len(),
        );
        ptr::copy_nonoverlapping(descriptors.idt().as_ptr(), idt as *mut u8, 4096);
        // Returning IRQ gates at 32-255 use the current private host stack,
        // not terminal IST1; 16-31 keep IST1 (`window_gate`).
        for vector in 32..256 {
            (idt as *mut u8).add(vector * 16 + 4).write(0);
        }
        let gdtr = &mut *ptr::addr_of_mut!(GDTR);
        gdtr[..2].copy_from_slice(&descriptors.gdtr().limit.to_le_bytes());
        gdtr[2..].copy_from_slice(&gdt.to_le_bytes());
        let idtr = &mut *ptr::addr_of_mut!(IDTR);
        idtr[..2].copy_from_slice(&4095u16.to_le_bytes());
        idtr[2..].copy_from_slice(&idt.to_le_bytes());
    }
    let tables = unsafe { &mut (*ptr::addr_of_mut!(TABLES)).0 };
    tables[0][0] = (root + 4096) | 3;
    tables[1][0] = (root + 8192) | 3;
    tables[2][((base >> 21) & 511) as usize] = (root + 12288) | 3;
    for page in (base..end).step_by(4096) {
        if [stack, stack_top, fault_stack, fault_top, nmi_stack, nmi_top].contains(&page) {
            continue;
        }
        let flags = if page < text_limit {
            1
        } else if page < data {
            1 | (1 << 63)
        } else {
            3 | (1 << 63)
        };
        tables[3][((page >> 12) & 511) as usize] = page | flags;
    }
    // Every private root maps one shared RW/NX page through a fixed local
    // alias. Its physical backing remains inside the excluded monitor pool.
    let avic_alias = base + super::X2AVIC_TABLE_OFFSET;
    tables[3][((avic_alias >> 12) & 511) as usize] =
        (pool_base + super::X2AVIC_TABLE_OFFSET) | 3 | (1 << 63);
    let shared_alias = base + super::STARTUP_PAGE_OFFSET;
    tables[3][((shared_alias >> 12) & 511) as usize] =
        (pool_base + super::STARTUP_PAGE_OFFSET) | 3 | (1 << 63);
    for offset in (super::CACHE_OWNER_OFFSET..super::CACHE_CAPTURE_OFFSET).step_by(4096) {
        tables[3][(((base + offset) >> 12) & 511) as usize] = (pool_base + offset) | 3 | (1 << 63);
    }
    for offset in (super::CACHE_CAPTURE_OFFSET
        ..super::CACHE_CAPTURE_OFFSET
            + core::mem::size_of::<crate::svm::cache::CacheCapture>() as u64)
        .step_by(4096)
    {
        tables[3][(((base + offset) >> 12) & 511) as usize] = (pool_base + offset) | 1 | (1 << 63);
    }
    // RW/NX alias of every dense slot's retained backing page, including this
    // slot's own; later alias pages stay absent. The target reuses this image's
    // backing offset for every slot: each slot is a relocated copy of the same
    // linked image, and DXE refuses directories whose offsets differ.
    for slot in 0..pool_bytes / 0x100000 {
        tables[3][((backing_alias(base, slot) >> 12) & 511) as usize] =
            backing_alias_pte(pool_base, slot, backing - base);
    }
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    if vmcb.configure_native_boot_intercepts().is_err() {
        return 4;
    }
    // Initialize before DXE publishes this address in the shared AVIC table.
    if unsafe { &mut *ptr::addr_of_mut!(AVIC_BACKING) }
        .reset_stopped(apic_id as u32, GUEST_APIC_VERSION)
        .is_err()
    {
        return 4;
    }

    unsafe {
        ptr::write(ptr::addr_of_mut!(MSRPM), Msrpm::native_boot());
    }
    let context = unsafe { &mut *ptr::addr_of_mut!(CONTEXT) };
    context.host_stack_top = stack_top;
    context.host_cr3 = root;
    context.guest_vmcb_pa = ptr::addr_of_mut!(VMCB) as u64;
    context.guest_vmcb_va = context.guest_vmcb_pa;
    context.host_extra_pa = ptr::addr_of_mut!(HOST_EXTRA) as u64;
    context.guest_frame_va = ptr::addr_of_mut!(FRAME) as u64;
    context.hsave_pa = ptr::addr_of_mut!(HSAVE) as u64;
    context.host_gdtr_va = ptr::addr_of_mut!(GDTR) as u64;
    context.host_idtr_va = ptr::addr_of_mut!(IDTR) as u64;
    context.owner_context = ptr::addr_of_mut!(STATE) as u64;
    let directory = ResidentDirectory {
        version: DIRECTORY_VERSION,
        arena_base: base,
        arena_bytes: 0x100000,
        context: ptr::addr_of_mut!(CONTEXT) as u64,
        vmcb: context.guest_vmcb_pa,
        auxiliary: ptr::addr_of_mut!(AUX) as u64,
        registers: context.guest_frame_va,
        npt: ptr::addr_of_mut!(NPT) as u64,
        arm: arm as *const () as u64,
        enter: svmvisor_resident_enter as *const () as u64,
        text_end: text_limit,
        data_start: data,
        memory_end: end,
        pool_base,
        pool_bytes,
        cpu_slot,
        apic_id,
        avic_backing: backing,
        reserved: [0; 2],
    };
    unsafe {
        ptr::write(output, directory);
        POOL = (pool_base, pool_bytes);
        ASSIGNED_APIC_ID = apic_id as u32;
    }
    state.prepared = true;
    0
}

/// Vectors whose IDT gate checks the acceptance window (irq.S): 16-255
/// except the #MC gate (18) and the #SX gate (30), which checks the window
/// itself when its error code is not the INIT redirection's 1. External
/// interrupts push no error code (APM2 rev3.44 8.2.24 p261); every other
/// exception vector below 32 cannot be raised inside the window, which runs
/// only NOPs (Table 8-1 p246), so a window event on those vectors is a
/// physical interrupt. Vectors 16-31 keep IST1: outside the window they
/// remain host exceptions.
pub(super) const fn window_gate(vector: usize) -> bool {
    vector >= 16 && vector < 16 + IRQ_GATES && vector != 18 && vector != 30
}

/// Private-root address of dense slot `slot`'s backing-page alias (D7).
pub(super) const fn backing_alias(base: u64, slot: u64) -> u64 {
    base + super::X2AVIC_BACKING_ALIASES_OFFSET + slot * 4096
}

/// RW/NX leaf of that alias: slot `slot`'s image starts `slot` MiB into the
/// pool and, being a relocated copy of this image, holds its backing page at
/// the same `backing_offset`.
pub(super) const fn backing_alias_pte(pool_base: u64, slot: u64, backing_offset: u64) -> u64 {
    (pool_base + slot * 0x100000 + backing_offset) | 3 | (1 << 63)
}

/// # Safety
/// Owning CPU, IF=0, native capture/memory admission complete, never-entered
/// exclusively stopped VMCB/frame/NPT already prepared in this owned image.
/// Callback sites identify the audited immutable callback until its guest RET.
/// `map` and `ids` are disjoint valid immutable caller arrays for this call;
/// IDs bind the complete retained pool to admitted native processors. x2APIC
/// must already be enabled on every CPU.
/// A nonnull `initial_icr` is aligned, readable and immutable for this call,
/// revalidated by DXE under the current mapping; its value is copied only.
/// The optional terminal endpoint has the same copied-input lifetime and its
/// complete 56-byte aligned mapping has been validated by the DXE caller.
///
/// Returns 0 when armed, otherwise the refused step: 1 runtime state or CPU
/// identity, 2 CPU capabilities, 3 EFER, 4 bootstrap ACK sites, 5 VMCB
/// controls, 6 memory map, 7 CPU inventory, 8 x2APIC/x2AVIC admission
/// (capabilities, APIC_BASE, host IDs above 254, profile, inherited physical
/// ISR, initial ICR pointer), 9 startup ownership and its commit, 10 terminal
/// endpoint, 11 the loader's x2APIC register state is outside the guest
/// register model (`CapturedInterface`; returned as the typed
/// `captured_register_refusal`, which names the MSR and value), 12 cache
/// replay. Every refusal precedes the first visible change (VM_CR, the
/// destination record, the LAPIC, IsRunning): this CPU's physical-ID table
/// entry is checked first, so the final `set_running` refuses only if the
/// published table changed meanwhile. DXE never enters a runtime whose arm
/// failed.
unsafe extern "win64" fn arm(
    efer: u64,
    resume: u64,
    ack: u64,
    after: u64,
    map: *const MemoryDescriptor,
    count: usize,
    ids: *const u32,
    id_count: usize,
    startup_owned: bool,
    initial_icr: *const u64,
    terminal_endpoint: *const TerminalEndpoint,
) -> u64 {
    let state = unsafe { &mut *ptr::addr_of_mut!(STATE) };
    if !state.prepared
        || state.armed
        || (!startup_owned && !initial_icr.is_null())
        || __cpuid_count(1, 0).ebx >> 24 != unsafe { ASSIGNED_APIC_ID }
    {
        return 1;
    }
    let Some(caps) = (unsafe { capabilities() }) else {
        return 2;
    };
    let endpoint = if terminal_endpoint.is_null() {
        None
    } else {
        // Caller validated this immutable input mapping for this arm invocation.
        // PPR57896 applies only to Family1Ah Model44h B0, signature00B40F40h.
        if !startup_owned
            || terminal_endpoint as usize & 7 != 0
            || __cpuid_count(1, 0).eax != TARGET_SIGNATURE
        {
            return 10;
        }
        let value = unsafe { terminal_endpoint.read() };
        if !value.valid() || unsafe { read_msr(MMIO_CFG_BASE_ADDR) } != value.mmio_config_msr {
            return 10;
        }
        Some(value)
    };
    if ids.is_null()
        || !(ids as usize).is_multiple_of(core::mem::align_of::<u32>())
        || id_count == 0
        || id_count > 32
        || id_count as u64 != unsafe { POOL.1 } / 0x100000
    {
        return 7;
    }
    let ids = unsafe { core::slice::from_raw_parts(ids, id_count) };
    let assigned_id = unsafe { ASSIGNED_APIC_ID };
    {
        use crate::svm::cache::{CacheCapture, CacheObservation};
        // Arm still runs under the admitted caller root, so use the physical
        // pool address. The private read-only alias is used only after entry.
        let capture = unsafe { &*((POOL.0 + super::CACHE_CAPTURE_OFFSET) as *const CacheCapture) };
        if capture.enabled() {
            let Some(slot) = ids.iter().position(|&id| id == assigned_id) else {
                return 12;
            };
            let Some(original) = capture.observation(slot, id_count) else {
                return 12;
            };
            let Some(members) = capture.domain_mask(slot, ids) else {
                return 12;
            };
            let current = CacheObservation::capture(
                __cpuid_count(1, 0).eax,
                caps.address_policy().physical_bits(),
                crate::svm::cache::native_topology(),
                |index| unsafe { read_msr(index) },
                |index, value| unsafe { write_msr(index, value) },
            );
            let Some(current) = current else {
                return 12;
            };
            if current.topology[0] != assigned_id || !original.same_physical_state(&current) {
                return 12;
            }
            state.cache_observation = Some(current);
            state.cache_core = members.trailing_zeros() as usize;
            state.cache_visibility = current.sys_cfg & SYS_CFG_MTRR_FIX_DRAM_MOD_EN != 0;
        }
    }
    let Ok(icr_owner) = NativeIcr::admit(assigned_id, ids) else {
        return 7;
    };
    // Exclusive handoff: the loader must already use x2APIC. An xAPIC
    // continuation is refused, never promoted. The captured physical
    // APIC_BASE stays the host interface and seeds the guest shadow (D4).
    let host_apic_base = unsafe { read_msr(apic::APIC_BASE) };
    let Ok(avic_caps) =
        X2AvicCapabilities::admit(__cpuid_count(1, 0).ecx, __cpuid_count(0x8000_000a, 0).edx)
    else {
        return 8;
    };
    let Ok(guest_apic) = GuestX2Apic::admit(host_apic_base, &caps.address_policy()) else {
        return 8;
    };
    // D8: every host ID must be a doorbell target (at most 254), which also
    // bounds the physical-ID table index (15.29.5.2 p571).
    if ids.iter().any(|&id| DoorbellTarget::new(id).is_none())
        || unsafe { read_msr(apic::ID_MSR) } != assigned_id as u64
    {
        return 8;
    }
    let Ok(avic) = NativeX2AvicProfile::new(
        avic_caps,
        ptr::addr_of!(AVIC_BACKING) as u64,
        unsafe { POOL.0 } + super::X2AVIC_TABLE_OFFSET,
        *ids.iter().max().unwrap() as u16,
        &caps.address_policy(),
    ) else {
        return 8;
    };
    // SAFETY: CPL0 callback on its owning CPU with IF=0 (this function's
    // contract); x2APIC is enumerated (X2AvicCapabilities) and enabled
    // (GuestX2Apic); nothing else uses this CPU's LAPIC until its guest runs.
    let mut host = unsafe { HostX2Apic::new() };
    // Before table publication/guest entry, physical sources must have no
    // inherited in-service ownership. Pending physical IRR is captured later.
    if apic::highest_in_service(&mut host).is_some() {
        return 8;
    }
    // The physical bootstrap changed the BSP's ICR. Preserve its captured
    // logical readback in the virtual page without sending a second command.
    let captured_icr = if initial_icr.is_null() {
        host.read(apic::ICR_MSR)
    } else {
        if initial_icr as usize & 7 != 0
            || caps.address_policy().validate(initial_icr as u64, 8, 8).is_err()
        {
            return 8;
        }
        unsafe { initial_icr.read() }
    };
    // The loader's register interface must be representable by the guest
    // register owner before anything changes (D2/D3). ISR/IRR belong to the
    // bridge, which starts empty.
    let interface = match CapturedInterface::capture(&mut host, captured_icr) {
        Ok(interface) => interface,
        Err(refusal) => return super::captured_register_refusal(refusal),
    };
    let backing = unsafe { &*ptr::addr_of!(AVIC_BACKING) };
    let vmcb = unsafe { &mut *ptr::addr_of_mut!(VMCB) };
    let frame = unsafe { &*ptr::addr_of!(FRAME) };
    if let Some(endpoint) = endpoint {
        let slot = ids.iter().position(|&id| id == assigned_id).unwrap();
        if !unsafe { diagnostics::prepare(endpoint, slot, id_count) } {
            return 10;
        }
        let io = unsafe { &mut *ptr::addr_of_mut!(IOPM) };
        if io.set_range(0, 65536, Permission::Allow).is_err()
            || io.set_range(0xcf8, 8, Permission::Intercept).is_err()
            || unsafe { &mut *ptr::addr_of_mut!(MSRPM) }
                .set(MMIO_CFG_BASE_ADDR, MsrAccess::Write, Permission::Intercept)
                .is_err()
        {
            return 10;
        }
        vmcb.set_instruction_intercept(crate::svm::vmcb::InstructionIntercept::Ioio, true);
    }
    // Capture only enumerated same-CPU features, before the first guest entry.
    // Guest FXSR and INVLPG execute directly; VMRUN/VMEXIT owns EFER switching.
    let extended = __cpuid_count(0x8000_0001, 0);
    let extended21 = if __cpuid_count(0x8000_0000, 0).eax >= 0x8000_0021 {
        Some(__cpuid_count(0x8000_0021, 0).eax)
    } else {
        None
    };
    let Ok(mut owner) = NativeEfer::admit_native(
        efer,
        extended.ecx,
        extended.edx,
        __cpuid_count(0x8000_0008, 0).ebx,
        extended21,
    ) else {
        return 3;
    };
    let Ok(ack_owner) = NativeBootstrapAck::new(vmcb, frame, resume, ack, after) else {
        return 4;
    };
    let policy = caps.address_policy();
    if map.is_null()
        || !(map as usize).is_multiple_of(core::mem::align_of::<MemoryDescriptor>())
        || count == 0
        || count > MAX_DESCRIPTORS
    {
        return 6;
    }
    let descriptors = unsafe { core::slice::from_raw_parts(map, count) };
    if ValidatedMemoryMap::new(descriptors, policy.physical_bits()).is_err() {
        return 6;
    }
    // D1 guest x2APIC interception profile. Arm refuses fewer than two CPUs
    // below, so every armed runtime uses it.
    unsafe {
        (&mut *ptr::addr_of_mut!(MSRPM)).configure_native_x2avic();
    }
    if state.cache_observation.is_some() {
        if !startup_owned || unsafe { !cache::prepare_root() } {
            return 12;
        }
        let maps = unsafe { &mut *ptr::addr_of_mut!(MSRPM) };
        for index in crate::svm::cache::owned_msrs() {
            for access in [MsrAccess::Read, MsrAccess::Write] {
                if maps.set(index, access, Permission::Intercept).is_err() {
                    return 12;
                }
            }
        }
    }
    if vmcb
        .set_permission_maps(ptr::addr_of!(IOPM) as u64, ptr::addr_of!(MSRPM) as u64, &policy)
        .is_err()
        || vmcb.set_guest_asid(1, &caps).is_err()
        || vmcb.set_nested_root(ptr::addr_of!(NPT) as u64, &policy).is_err()
        || vmcb.enable_native_nested_paging(&policy).is_err()
    {
        return 5;
    }
    if !startup_owned || id_count < 2 {
        return 9;
    }
    let shared = unsafe {
        core::slice::from_raw_parts(
            (POOL.0 + super::STARTUP_PAGE_OFFSET) as *const NativeStartupMailbox,
            id_count,
        )
    };
    // APM2 15.21.2/15.29.5: VMRUN loads V_TPR and AVIC CR8 reads use it.
    // Seed the priority class as well as backing TPR before enabling AVIC.
    if vmcb.set_virtual_interrupt_tpr(interface.task_priority() >> 4).is_err() {
        return 9;
    }
    if shared.iter().zip(ids).any(|(mailbox, &id)| mailbox.identity() != id)
        || vmcb.enable_native_x2avic(&avic).is_err()
    {
        return 9;
    }
    // DXE published this CPU's entry stopped (valid, this backing page, host
    // ID = guest ID) before any arm; check it before anything visible
    // changes, so the final `set_running` only adds IsRunning.
    let table = unsafe { &*((POOL.0 + super::X2AVIC_TABLE_OFFSET) as *const PhysicalIdTable) };
    if !table.is_stopped_entry(assigned_id as u16, ptr::addr_of!(AVIC_BACKING) as u64) {
        return 9;
    }
    owner.enable_guest_startup();
    // Software startup commands retain the existing target-owned mailbox.
    // Ordinary fixed IPIs use x2AVIC, never a physical guest ICR write.
    let Ok(routes) = try_lock_routes(shared) else {
        return 9;
    };
    let slot = ids.iter().position(|&id| id == assigned_id).unwrap();
    let Ok(commit) = routes.prepare_destination_mode(slot, NativeDestinationMode::X2Apic) else {
        return 9;
    };
    unsafe {
        let original = read_msr(VM_CR);
        write_msr(VM_CR, original | VM_CR_R_INIT);
        if read_msr(VM_CR) != original | VM_CR_R_INIT {
            write_msr(VM_CR, original);
            return 9;
        }
    }
    commit.commit_destination_mode();
    // The captured interface becomes this vCPU's backing state. A physical
    // LVT changes only where the virtual APIC masks its source (D3), before
    // the host enables its own physical SVR below.
    interface.install(backing, &mut host);
    // Host physical TPR must not inherit a guest priority threshold. Guest CR8
    // and TPR now use AVIC; the physical LAPIC is a source capture backend.
    host.write(apic::msr(apic::TPR), 0);
    // Host capture owns physical software-enable and spurious vector FFh.
    // The captured guest SVR remains in its separate backing register.
    host.write(apic::msr(apic::SVR), u64::from(apic::SVR_SOFTWARE_ENABLE | 0xff));
    if table.set_running(assigned_id as u16, true).is_err() {
        return 9;
    }
    state.avic = Some(avic);
    unsafe {
        ptr::copy_nonoverlapping(map, ptr::addr_of_mut!(RAM).cast(), count);
        RAM_COUNT = count;
        PHYSICAL_BITS = policy.physical_bits();
    }
    state.ack = Some(ack_owner);
    state.efer = Some(owner);
    state.icr = Some(icr_owner);
    state.guest_apic = Some(guest_apic);
    state.host_apic_base = host_apic_base;
    state.startup_owned = startup_owned;
    state.slot = ids.iter().position(|&id| id == assigned_id).unwrap();
    state.count = id_count;
    state.terminal_endpoint = endpoint;
    // PAUSE filtering is optional; unsupported CPUs retain direct PAUSE.
    vmcb.configure_native_pause_filter(__cpuid_count(0x8000_000a, 0).edx);
    state.capabilities = Some(caps);
    state.armed = true;
    0
}

const _: crate::host::resident::ArmRuntime = arm;

/// Capability gating precedes every SVM-related MSR access in the caller.
/// Here VM_CR is read only after enumerated native AMD SVM. The shared native
/// encryption plan gates control reads and refuses unsupported enabled modes.
unsafe fn capabilities() -> Option<ValidatedCapabilities> {
    let basic = __cpuid_count(0, 0);
    let ext = __cpuid_count(0x80000000, 0);
    if basic.ebx != 0x68747541
        || basic.edx != 0x69746e65
        || basic.ecx != 0x444d4163
        || basic.eax < 1
        || ext.eax < 0x8000000a
    {
        return None;
    }
    let one = __cpuid_count(1, 0);
    let extone = __cpuid_count(0x80000001, 0);
    let svm = __cpuid_count(0x8000000a, 0);
    if one.ecx >> 31 != 0
        || extone.ecx & 4 == 0
        || extone.edx & ((1 << 20) | (1 << 26)) != ((1 << 20) | (1 << 26))
        || svm.edx & 1 != 1
        || svm.eax != 1
    {
        return None;
    }
    let width = __cpuid_count(0x80000008, 0).eax as u8;
    let encryption_leaf = if ext.eax >= 0x8000001f {
        let leaf = __cpuid_count(0x8000001f, 0);
        Some([leaf.eax, leaf.ebx, leaf.ecx, leaf.edx])
    } else {
        None
    };
    let plan = NativeEncryptionPlan::new(one.eax, width, encryption_leaf).ok()?;
    let encryption = plan
        .validate(
            plan.sys_cfg_msr().map(|msr| unsafe { read_msr(msr) }),
            plan.sev_status_msr().map(|msr| unsafe { read_msr(msr) }),
        )
        .ok()?;
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr",in("ecx")VM_CR,out("eax")low,out("edx")high,options(nostack));
    }
    if u64::from(low) & VM_CR_SVMDIS != 0 || high != 0 {
        return None;
    }
    CapabilityEvidence {
        vendor: CpuVendor::Amd,
        svm: EvidenceFlag::Set,
        nested_paging: EvidenceFlag::Set,
        svm_revision: Some(svm.eax as u8),
        asid_count: Some(svm.ebx),
        physical_address_bits: Some(width),
        vm_cr_svmdis: EvidenceFlag::Clear,
        hypervisor_present: EvidenceFlag::Clear,
        encryption,
        optional: OptionalFeatures { nrip_save: svm.edx & 8 != 0, ..Default::default() },
    }
    .validate()
    .ok()
}
