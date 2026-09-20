//! Native callback entry from the assembly boundary and the per-CPU takeover it performs.
use core::{
    arch::{asm, x86_64::__cpuid_count},
    ffi::c_void,
    ptr,
    sync::atomic::Ordering,
};

use svmvisor_card_abi::endpoint::TerminalEndpoint;
use svmvisor_hypervisor::{
    arch::x86_64::{
        capabilities::EvidenceFlag,
        msr::{EFER, PAT},
        registers::GuestRegisters,
    },
    boot::{
        descriptors::{FirmwareSelectors, parse_firmware_gdt},
        memory::MemoryDescriptor,
    },
    host::{descriptors::HostTablePointer, resident as abi},
    memory::{
        address::AddressPolicy,
        npt::{NptEvidence, TableStorage},
    },
    svm::vmcb::Vmcb,
};
use svmvisor_launcher::native::{
    admission::boundary::NativeBoundary,
    resident::{self, CallbackRequest, CallbackSites, GuestStackSpan, launch::xstate_valid},
};
use uefi_raw::Event;

#[cfg(feature = "native-resident-boot")]
use super::card_boot;
#[cfg(feature = "native-resident-smp-activate")]
use super::physical_boot;
use super::{
    ACTIVATING, COOKIE, CPU, CPU_COUNT, CPU_IDS, DIRECTORIES, GDT, GUEST_ACK, IMAGE, MAP,
    MAP_COUNT, READY,
    capture::{config, cpu, mtrrs, rdmsr, wrmsr},
    closure::{directories, host_closure},
    diagnostic::{trace, trace_detail, trace_error},
    mapping::mapped,
};

#[unsafe(no_mangle)]
pub unsafe extern "efiapi" fn svmvisor_resident_callback_inner(
    _event: Event,
    context: *mut c_void,
    boundary: *const NativeBoundary,
) -> u64 {
    if context as usize != COOKIE || boundary.is_null() || !READY.load(Ordering::Acquire) {
        return 1;
    }
    let id = __cpuid_count(1, 0).ebx >> 24;
    let ids =
        unsafe { core::slice::from_raw_parts(ptr::addr_of!(CPU_IDS).cast::<u32>(), CPU_COUNT) };
    let Some(slot) = ids.iter().position(|value| *value == id) else {
        return 1;
    };
    if ACTIVATING.fetch_or(1u32 << slot, Ordering::AcqRel) & (1u32 << slot) != 0 {
        return 1;
    }
    trace(b'C');
    match unsafe { callback(&*boundary, slot) } {
        Ok(()) => 0,
        Err(code) => {
            #[cfg(feature = "native-resident-boot")]
            if unsafe { physical_boot::is_bsp(slot) } {
                unsafe {
                    card_boot::takeover_failure(slot as u32, CPU_COUNT as u32, code);
                }
            }
            trace_error(code);
            code
        }
    }
}

unsafe fn callback(b: &NativeBoundary, slot: usize) -> Result<(), u64> {
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags));
    }
    if flags & (1 << 9) != 0 {
        return Err(23);
    }
    let processor = unsafe { cpu() }?;
    let mut expected = unsafe { CPU }.ok_or(10u64)?;
    expected.apic_id = unsafe { CPU_IDS[slot] };
    if processor != expected || !xstate_valid(b) {
        return Err(10);
    }
    let d = unsafe { DIRECTORIES[slot] };
    let map = unsafe {
        core::slice::from_raw_parts(ptr::addr_of!(MAP).cast::<MemoryDescriptor>(), MAP_COUNT)
    };
    let cfg = unsafe { config(processor) }?;
    if cfg.cr3 != b.cr3 {
        return Err(12);
    }
    let cr0: u64;
    let cr4: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    if cr0 != b.cr0 || cr4 != b.cr4 {
        return Err(12);
    }
    let mt = unsafe { mtrrs(processor.physical_bits) }?;
    let pat = unsafe { rdmsr(PAT) };
    let policy =
        AddressPolicy::new(processor.physical_bits, processor.encryption).map_err(|_| 13u64)?;
    unsafe { mapped(map, cfg, &mt, pat, d.arena_base, d.arena_bytes, true, true) }?;
    unsafe { host_closure(directories(), slot, map, processor, &mt, pat) }?;
    let current_rsp: u64;
    unsafe {
        asm!("mov {}, rsp", out(reg) current_rsp, options(nomem, nostack, preserves_flags));
    }
    let stack_base = current_rsp.checked_sub(64 * 1024).ok_or(14u64)?;
    let stack_end = b.entry_rsp.checked_add(40).ok_or(14u64)?;
    unsafe {
        mapped(
            map,
            cfg,
            &mt,
            pat,
            stack_base,
            stack_end.checked_sub(stack_base).ok_or(14u64)?,
            true,
            false,
        )
    }?;
    #[cfg(feature = "native-resident-boot")]
    unsafe { physical_boot::cache_sample_before_activation(processor, slot) }?;
    let sites = CallbackSites {
        resume: ptr::addr_of!(abi::svmvisor_resident_guest_resume) as u64,
        ack: ptr::addr_of!(abi::svmvisor_resident_guest_ack) as u64,
        after_ack: ptr::addr_of!(abi::svmvisor_resident_guest_after_ack) as u64,
    };
    let (image, image_bytes) = unsafe { IMAGE };
    if sites.resume < image
        || sites.after_ack.checked_add(256).is_none_or(|end| end > image + image_bytes)
    {
        return Err(21);
    }
    unsafe {
        mapped(map, cfg, &mt, pat, sites.resume, 256, false, true)?;
        // The bounded post-ACK epilogue uses this immutable admitted identity
        // table and a shared runtime-image completion word. All remain outside
        // the private monitor pool and are rewalked under each captured root.
        mapped(map, cfg, &mt, pat, ptr::addr_of!(CPU_COUNT) as u64, 8, false, false)?;
        mapped(
            map,
            cfg,
            &mt,
            pat,
            ptr::addr_of!(CPU_IDS) as u64,
            core::mem::size_of::<[u32; abi::MAX_RESIDENT_CPUS]>() as u64,
            false,
            false,
        )?;
        mapped(map, cfg, &mt, pat, ptr::addr_of!(GUEST_ACK) as u64, 4, true, false)?;
        mapped(map, cfg, &mt, pat, b.entry_rip, 1, false, true)?;
        mapped(map, cfg, &mt, pat, b.gdtr.base(), u64::from(b.gdtr.limit()) + 1, true, false)?;
        mapped(map, cfg, &mt, pat, b.idtr.base(), u64::from(b.idtr.limit()) + 1, false, false)?;
        mapped(
            map,
            cfg,
            &mt,
            pat,
            ptr::addr_of!(MAP) as u64,
            core::mem::size_of::<[MemoryDescriptor; 4096]>() as u64,
            false,
            false,
        )?;
        mapped(map, cfg, &mt, pat, ptr::addr_of!(GDT) as u64, 65536, true, false)?;
    }
    let linked = unsafe { core::slice::from_raw_parts(sites.resume as *const u8, 8) };
    if linked != [0xb8, 0x41, 0x4d, 0x56, 0x53, 0x0f, 0x01, 0xd9] {
        return Err(22);
    }
    let gdt_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            ptr::addr_of_mut!(GDT).cast::<u8>(),
            usize::from(b.gdtr.limit()) + 1,
        )
    };
    unsafe {
        ptr::copy_nonoverlapping(
            b.gdtr.base() as *const u8,
            gdt_bytes.as_mut_ptr(),
            gdt_bytes.len(),
        );
    }
    let gdt = parse_firmware_gdt(
        HostTablePointer { base: b.gdtr.base(), limit: b.gdtr.limit() },
        FirmwareSelectors { cs: b.cs, ss: b.ss, ds: b.ds, es: b.es },
        gdt_bytes,
    )
    .map_err(|_| 15u64)?;
    let original_efer = unsafe { rdmsr(EFER) };
    if original_efer != b.efer {
        return Err(16);
    }
    // Read the executing CPU's feature evidence before admitting its captured
    // EFER. Boundary capture separately refuses FFXSR to retain all XMM state.
    let maximum_extended = __cpuid_count(0x8000_0000, 0).eax;
    if maximum_extended < 0x8000_0008 {
        return Err(16);
    }
    let extended = __cpuid_count(0x8000_0001, 0);
    let extended8 = __cpuid_count(0x8000_0008, 0);
    let extended21 = if maximum_extended >= 0x8000_0021 {
        Some(__cpuid_count(0x8000_0021, 0).eax)
    } else {
        None
    };
    let efer = svmvisor_hypervisor::svm::dispatch::NativeEfer::admit_native(
        original_efer,
        extended.ecx,
        extended.edx,
        extended8.ebx,
        extended21,
    )
    .map_err(|_| 16u64)?;
    // APM2 VMSAVE requires SVME. No fallible operation or Rust return exists
    // between this temporary enable and exact restoration; HSAVE is untouched.
    unsafe {
        wrmsr(EFER, original_efer | (1 << 12));
        asm!("vmsave rax", in("rax") d.auxiliary, options(nostack, preserves_flags));
        wrmsr(EFER, original_efer);
    }
    let dr6: u64;
    let dr7: u64;
    unsafe {
        asm!("mov {}, dr6", out(reg) dr6, options(nomem, nostack, preserves_flags));
        asm!("mov {}, dr7", out(reg) dr7, options(nomem, nostack, preserves_flags));
    }
    let aux = unsafe { &*(d.auxiliary as *const Vmcb) };
    // VMSAVE captures FS/GS bases. Nonzero TLS aliases must remain backed by
    // admitted guest/native memory; zero bases do not imply page zero access.
    for offset in [0x448usize, 0x458] {
        let base = u64::from_le_bytes(aux.bytes()[offset..offset + 8].try_into().unwrap());
        if base != 0 {
            unsafe { mapped(map, cfg, &mt, pat, base, 1, false, false) }?;
        }
    }
    {
        let vmcb = unsafe { &mut *(d.vmcb as *mut Vmcb) };
        let frame = unsafe { &mut *(d.registers as *mut GuestRegisters) };
        resident::prepare_callback(
            CallbackRequest {
                boundary: b,
                efer,
                gdt: &gdt,
                auxiliary: aux,
                dr6,
                dr7,
                pat,
                stack: GuestStackSpan { base: stack_base, bytes: stack_end - stack_base },
                sites,
            },
            &policy,
            vmcb,
            frame,
        )
        .map_err(|error| {
            trace_detail(&error);
            trace_detail(&(
                "callback-native",
                slot,
                b.rflags,
                b.cr0,
                b.cr3,
                b.cr4,
                b.efer,
                b.profile,
            ));
            trace_detail(&("callback-selectors", b.cs, b.ss, b.ds, b.es, b.fs, b.gs, b.ldtr, b.tr));
            trace_detail(&("callback-debug", dr6, dr7, b.cr8, pat));
            trace_detail(&(
                "callback-stack",
                b.entry_rsp,
                b as *const NativeBoundary as u64,
                b.entry_rip,
                stack_base,
                stack_end,
            ));
            for offset in [0x440usize, 0x450, 0x470, 0x490] {
                let bytes = aux.bytes();
                trace_detail(&(
                    "callback-aux",
                    offset,
                    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()),
                    u16::from_le_bytes(bytes[offset + 2..offset + 4].try_into().unwrap()),
                    u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()),
                    u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap()),
                ));
            }
            17u64
        })?;
        // APM2 18.1.1/18.12 and Table B-2: CET_SS enumerates these MSRs.
        // VMSAVE omits them. CR4.CET was refused by prepare_callback, but
        // disabled CET does not imply its MSRs are zero. Preserve the native
        // values before VMRUN starts owning their guest VMCB copies.
        if __cpuid_count(0, 0).eax >= 7 && __cpuid_count(7, 0).ecx & (1 << 7) != 0 {
            let s_cet = unsafe { rdmsr(0x6a2) };
            let isst_addr = unsafe { rdmsr(0x6a8) };
            vmcb.initialize_native_cet_msrs(s_cet, isst_addr).map_err(|_| 17u64)?;
        }
        let storage = unsafe { &mut *(d.npt as *mut TableStorage) };
        let monitor = policy.validate(d.pool_base, d.pool_bytes, 4096).map_err(|_| 18u64)?;
        let mut _npt = resident::memory::prepare_identity_npt(
            storage,
            d.npt,
            policy,
            monitor,
            map,
            NptEvidence {
                nx_supported: EvidenceFlag::Set,
                host_nxe: EvidenceFlag::Set,
                host_four_level: EvidenceFlag::Set,
            },
            EvidenceFlag::Set,
            pat,
        )
        .map_err(|_| 19u64)?;
        #[cfg(feature = "native-resident-boot")]
        card_boot::protect_config(&mut _npt).map_err(|_| 19u64)?;
    }
    #[cfg(feature = "native-resident-smp-activate")]
    physical_boot::validate_x2apic()?;
    let arm: abi::ArmRuntime = unsafe { core::mem::transmute(d.arm as usize) };
    #[cfg(feature = "native-resident-guest-startup")]
    let initial_icr = unsafe { physical_boot::initial_icr(slot)? };
    #[cfg(not(feature = "native-resident-guest-startup"))]
    let initial_icr: *const u64 = ptr::null();
    if !initial_icr.is_null() {
        // arm copies this numeric field before switching to its private root;
        // it must not retain a caller pointer through runtime virtual mapping.
        unsafe { mapped(map, cfg, &mt, pat, initial_icr as u64, 8, false, false)? };
    }
    #[cfg(feature = "native-resident-boot")]
    let terminal_endpoint = card_boot::terminal_endpoint();
    #[cfg(not(feature = "native-resident-boot"))]
    let terminal_endpoint: *const TerminalEndpoint = ptr::null();
    if !terminal_endpoint.is_null() {
        unsafe {
            mapped(
                map,
                cfg,
                &mt,
                pat,
                terminal_endpoint as u64,
                core::mem::size_of::<TerminalEndpoint>() as u64,
                false,
                false,
            )?;
        }
    }
    let arm_result = unsafe {
        arm(
            original_efer,
            sites.resume,
            sites.ack,
            sites.after_ack,
            map.as_ptr(),
            map.len(),
            ptr::addr_of!(CPU_IDS).cast::<u32>(),
            CPU_COUNT,
            cfg!(feature = "native-resident-guest-startup"),
            initial_icr,
            terminal_endpoint,
        )
    };
    if arm_result != 0 {
        // Preserve typed takeover evidence (arm code 11 names the refused
        // captured x2APIC register) through the AP callback home area and
        // BSP's captured refusal. Ordinary arm failures retain old code20.
        return Err(if arm_result >> 56 == abi::TAKEOVER_TAG { arm_result } else { 20 });
    }
    trace(b'E');
    let enter: abi::Enter = unsafe { core::mem::transmute(d.enter as usize) };
    // All fallible preparation and original-EFER restoration precede this
    // irreversible transition. The raw runtime never returns to this stack.
    unsafe {
        wrmsr(EFER, original_efer | (1 << 12));
        enter(d.context as *mut abi::BridgeContext)
    }
}
