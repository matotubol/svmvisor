//! Physical AP bootstrap. Caller has returned successfully from EBS.
//! PI1.10 II-13.4.1 makes same-group notification ordering insufficient.
//! The diagnostic consumer and native boot interposer share this startup owner.
use super::*;
use core::sync::atomic::AtomicU32;
use resident::bootstrap_paging::BootstrapPaging;
use resident::physical::{ACTIVATION_GUID, ActivationInterface};
use svmvisor_hypervisor::host::descriptors::HostDescriptorRequest;
use svmvisor_hypervisor::svm::ipi::native_apic_mode_supported;
use uefi_raw::table::boot::AllocateType;
use svmvisor_dxe::diagnostics::resident_boot::AdmissionFailure;
// One blocking MP observer at a time. No card access from this context.
static mut ADMISSION_CONTEXT:Option<AdmissionFailure>=None;
static ADMISSION_ACTIVE:AtomicBool=AtomicBool::new(false);
fn admission_clear(){ADMISSION_ACTIVE.store(false,Ordering::Release);unsafe{ADMISSION_CONTEXT=None;}}
fn admission_begin(operation:u32,processor:u32,apic_id:u32){
    let mut value=AdmissionFailure::new(operation,0,0,0,0,0);
    value.processor=processor;value.apic_id=apic_id;
    unsafe{ADMISSION_CONTEXT=Some(value);}
    ADMISSION_ACTIVE.store(true,Ordering::Release);
}
pub(super) fn admission_cpu_id(apic_id:u32){
    if !ADMISSION_ACTIVE.load(Ordering::Acquire){return;}
    unsafe{if let Some(value)=&mut *ptr::addr_of_mut!(ADMISSION_CONTEXT){value.apic_id=apic_id;}}
}
pub(super) fn admission_hint(predicate:u32,item:u64,observed:u64,expected:u64){
    if !ADMISSION_ACTIVE.load(Ordering::Acquire){return;}
    unsafe{if let Some(value)=&mut *ptr::addr_of_mut!(ADMISSION_CONTEXT){
        if value.predicate==0{value.predicate=predicate;value.item=item;value.observed=observed;value.expected=expected;}
    }}
}
pub(super) fn admission_walk(error:paging::WalkError,cfg:PagingConfig,address:u64,last:Option<(u64,u64)>){
    use paging::WalkError::*;
    let(reason,level)=match error{UnsupportedPhysicalWidth=>(1,0),FiveLevelUnsupported=>(2,0),NoncanonicalAddress=>(3,0),
        InvalidCr3=>(4,0),UnreadableTable{level,..}=>(5,level),NotPresent{level}=>(6,level),ReservedEntry{level}=>(7,level),
        UnsupportedEntryBits{level}=>(8,level),OneGiBUnsupported=>(9,0),IncompleteWalk=>(10,0)};
    let(item,observed)=match error {
        UnreadableTable{address,..}=>(address,0),
        InvalidCr3=>(3,cfg.cr3),UnsupportedPhysicalWidth=>(0x80000008,cfg.physical_bits as u64),
        FiveLevelUnsupported=>(4,1<<12),NoncanonicalAddress=>(address,address),
        _=>last.unwrap_or((cfg.cr3,0)),
    };
    admission_hint(400+reason+u32::from(level)*16,item,observed,address);
}
fn admission_end(code:u64)->AdmissionFailure{
    ADMISSION_ACTIVE.store(false,Ordering::Release);
    let mut value=unsafe{ptr::addr_of_mut!(ADMISSION_CONTEXT).replace(None)}
        .unwrap_or(AdmissionFailure::new(3,0,0,0,0,0));
    if value.predicate==0 {value.predicate=code as u32;}
    value.status=code;value
}
fn admission_refused(value:AdmissionFailure)->Status{
    #[cfg(feature="native-resident-boot")]
    card_boot::admission_failure(value,unsafe{CPU_COUNT} as u32);
    Status::UNSUPPORTED
}
const BOOT_BYTES: usize = 128 * 1024;
const AP_FAILURE_OFFSET: u64 = 120;
use svmvisor_dxe::diagnostics::resident_boot::ApFailureObservation;
const _: () = assert!(AP_FAILURE_OFFSET as usize + core::mem::size_of::<ApFailureObservation>() <= 256);
#[repr(C, align(4096))]
struct Bootstrap([u8; BOOT_BYTES]);
static mut BOOT: [Bootstrap; abi::MAX_RESIDENT_CPUS] =
    [const { Bootstrap([0; BOOT_BYTES]) }; abi::MAX_RESIDENT_CPUS];
static STARTED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "native-resident-boot")]
static CACHE_SURVEY: svmvisor_hypervisor::svm::native_cache::CacheSurvey =
    svmvisor_hypervisor::svm::native_cache::CacheSurvey::new();
#[cfg(feature = "native-resident-boot")]
static CACHE_FAILURE_SLOT: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg(feature = "native-resident-boot")]
static mut CACHE_SAMPLE_FAILURES: [svmvisor_hypervisor::svm::native_cache::CacheAdmissionFailure; 32] =
    [svmvisor_hypervisor::svm::native_cache::CacheAdmissionFailure::new(0,0,0,0); 32];
static mut LOW: u64 = 0;
static mut BOOT_CFG: Option<PagingConfig> = None;
static mut AP_TABLES: BootstrapPaging = BootstrapPaging::empty();
static mut BOOT_APIC_BASE: u64 = 0;
static mut BSP: usize = 0;
/// Inventory is immutable before any activation callback executes.
pub(super) unsafe fn is_bsp(slot: usize) -> bool { slot == unsafe { BSP } }
pub(super) unsafe fn bsp_slot()->usize{unsafe{BSP}}
#[cfg(feature = "native-resident-guest-startup")]
static mut BSP_INITIAL_ICR: u64 = 0;
#[cfg(feature = "native-resident-guest-startup")]
static BSP_INITIAL_ICR_READY: AtomicBool = AtomicBool::new(false);
#[unsafe(no_mangle)]
static mut svmvisor_ap_boots: [u64; abi::MAX_RESIDENT_CPUS] = [0; abi::MAX_RESIDENT_CPUS];
#[unsafe(no_mangle)]
static mut svmvisor_ap_count: u32 = 0;
#[unsafe(no_mangle)]
static svmvisor_ap_discovery_failed: AtomicU32 = AtomicU32::new(0);
static mut INTERFACE: ActivationInterface = ActivationInterface {
    version: 1,
    count: 0,
    pool_base: 0,
    pool_bytes: 0,
    start,
    completed: AtomicU32::new(0),
    failed: AtomicU32::new(0),
};
unsafe extern "C" {
    static svmvisor_ap_trampoline: u8;
    static svmvisor_ap_trampoline_end: u8;
    static svmvisor_ap_protected_target: u8;
    static svmvisor_ap_protected: u8;
    static svmvisor_ap_root: u8;
    static svmvisor_ap_long_target: u8;
    static svmvisor_ap_gdt: u8;
    static svmvisor_ap_gdt_base: u8;
    static svmvisor_ap_wait: u8;
    static svmvisor_ap_wait_end: u8;
    static svmvisor_ap_wait_fault: u8;
    fn svmvisor_ap_entry64();
}
fn interface() -> &'static ActivationInterface {
    unsafe { &*ptr::addr_of!(INTERFACE) }
}
fn boot_address(slot: usize) -> u64 {
    unsafe { ptr::addr_of_mut!(BOOT).cast::<Bootstrap>().add(slot) as u64 }
}
/// BSP-only saved pre-bootstrap ICR, copied by arm while the admitted DXE
/// mapping is still live. This numeric field is never a runtime virtual pointer.
/// # Safety
/// Called from the serialized native callback after start's successful-EBS
/// preflight. The caller revalidates this field's current readable mapping.
#[cfg(feature = "native-resident-guest-startup")]
pub(super) unsafe fn initial_icr(slot: usize) -> Result<*const u64, u64> {
    if slot != unsafe { BSP } {
        return Ok(ptr::null());
    }
    if !BSP_INITIAL_ICR_READY.load(Ordering::Acquire) {
        return Err(46);
    }
    Ok(ptr::addr_of!(BSP_INITIAL_ICR))
}
pub(super) struct LowAllocation<'a> {
    bs: &'a BootServices,
    base: u64,
    retained: bool,
}
impl LowAllocation<'_> {
    pub(super) fn retain(&mut self) {
        self.retained = true;
    }
}
impl Drop for LowAllocation<'_> {
    fn drop(&mut self) {
        if !self.retained {
            let status = unsafe { (self.bs.free_pages)(self.base, 1) };
            if status != Status::SUCCESS {
                trace_detail(&("low-page-cleanup", self.base, status));
            }
        }
    }
}
/// All allocation and complete static-bootstrap construction happens before
/// publication. Runtime image owns BOOT, AP_TABLES and interface until reset.
/// The low LoaderCode page is consumed before successful activation returns;
/// no permanent OS reservation is inferred from that type.
pub(super) unsafe fn prepare(
    bs: &BootServices,
    bsp: usize,
    count: usize,
    cfg: PagingConfig,
) -> Result<LowAllocation<'_>, Status> {
    preparation_step(25, count as u64);
    if !(2..=abi::MAX_RESIDENT_CPUS).contains(&count) {
        return Err(Status::UNSUPPORTED);
    }
    let vm_cr = unsafe { rdmsr(0xc0010114) };
    preparation_step(27, vm_cr);
    if vm_cr & 2 != 0 {
        return Err(Status::UNSUPPORTED);
    }
    let apic_base = unsafe { rdmsr(0x1b) };
    preparation_step(28, apic_base);
    if !lapic_base_valid(apic_base) {
        return Err(Status::UNSUPPORTED);
    }
    // APM2 16.9/Table16-5: x2APIC capability is required only for the
    // selected x2APIC mode. Native guest startup preserves firmware xAPIC;
    // the ordinary activation fixture deliberately promotes to x2APIC.
    let cpuid1_ecx = __cpuid_count(1, 0).ecx;
    preparation_step(26, u64::from(cpuid1_ecx));
    if !native_apic_mode_supported(
        cpuid1_ecx,
        apic_base & 0x400 != 0 || !cfg!(feature = "native-resident-guest-startup"),
    ) {
        return Err(Status::UNSUPPORTED);
    }
    unsafe {
        BOOT_APIC_BASE = apic_base;
    }
    preparation_step(21, 0xfffff);
    let mut low = 0xfffff;
    let status = unsafe {
        (bs.allocate_pages)(
            AllocateType::MAX_ADDRESS,
            MemoryType::LOADER_CODE,
            1,
            &mut low,
        )
    };
    if status != Status::SUCCESS {
        preparation_failure(1, status.0 as u64, low);
        return Err(status);
    }
    preparation_step(21, low);
    let allocation = LowAllocation {
        bs,
        base: low,
        retained: false,
    };
    if low == 0 || low >= 0x100000 || low & 4095 != 0 {
        return Err(Status::UNSUPPORTED);
    }
    unsafe {
        LOW = low;
    }
    preparation_step(22, 0);
    let mut map = unsafe { memory::collect(bs) }.map_err(preparation_map_failure)?;
    let mt = unsafe { mtrrs(cfg.physical_bits) }.map_err(unsupported)?;
    preparation_step(23, low);
    unsafe { validate(map.descriptors(), cfg, &mt, rdmsr(0x277), count) }.map_err(unsupported)?;
    map.release()?;
    preparation_step(24, low);
    let length = ptr::addr_of!(svmvisor_ap_trampoline_end) as usize
        - ptr::addr_of!(svmvisor_ap_trampoline) as usize;
    if length == 0 || length >= 4096 {
        return Err(Status::LOAD_ERROR);
    }
    unsafe {
        ptr::write_bytes(low as *mut u8, 0, 4096);
        ptr::copy_nonoverlapping(
            ptr::addr_of!(svmvisor_ap_trampoline),
            low as *mut u8,
            length,
        );
    }
    let offset = |p: *const u8| p as usize - ptr::addr_of!(svmvisor_ap_trampoline) as usize;
    for (field, value) in [
        (
            ptr::addr_of!(svmvisor_ap_protected_target),
            low + offset(ptr::addr_of!(svmvisor_ap_protected)) as u64,
        ),
        (
            ptr::addr_of!(svmvisor_ap_gdt_base),
            low + offset(ptr::addr_of!(svmvisor_ap_gdt)) as u64,
        ),
        (
            ptr::addr_of!(svmvisor_ap_root),
            ptr::addr_of!(AP_TABLES) as u64,
        ),
        (
            ptr::addr_of!(svmvisor_ap_long_target),
            svmvisor_ap_entry64 as *const () as u64,
        ),
    ] {
        if value > u32::MAX as u64 || offset(field) + 4 > length {
            return Err(Status::UNSUPPORTED);
        }
        unsafe {
            ((low + offset(field) as u64) as *mut u32).write_unaligned(value as u32);
        }
    }
    for slot in 0..count {
        let base = boot_address(slot);
        let wait = ptr::addr_of!(svmvisor_ap_wait) as usize;
        let wait_bytes = ptr::addr_of!(svmvisor_ap_wait_end) as usize - wait;
        let fault_offset = ptr::addr_of!(svmvisor_ap_wait_fault) as usize - wait;
        if wait_bytes == 0 || wait_bytes > 4096 - 256 || fault_offset >= wait_bytes {
            return Err(Status::LOAD_ERROR);
        }
        let descriptors = HostDescriptorRequest {
            gdt_base: base + 4096,
            tss_base: base + 4160,
            idt_base: base + 8192,
            rsp0: base + BOOT_BYTES as u64,
            ist1: base + 32768,
            handlers: [base + 256 + fault_offset as u64; 256],
        }
        .validate()
        .map_err(|_| Status::UNSUPPORTED)?;
        unsafe {
            (base as *mut u64).write(base + BOOT_BYTES as u64);
            ((base + 8) as *mut u16).write_unaligned(39);
            ((base + 10) as *mut u64).write_unaligned(base + 4096);
            ((base + 24) as *mut u16).write_unaligned(4095);
            ((base + 26) as *mut u64).write_unaligned(base + 8192);
            ((base + 48) as *mut u32).write(1u32 << slot);
            ((base + 56) as *mut u64).write(ptr::addr_of!(INTERFACE.completed) as u64);
            ((base + 64) as *mut u64).write(ptr::addr_of!(INTERFACE.failed) as u64);
            ((base + 80) as *mut u64).write(base + 256);
            ((base + 88) as *mut u32)
                .write(u32::from(!cfg!(feature = "native-resident-guest-startup")));
            ((base + 96) as *mut u64).write(0);
            // Assembly claims its identity before touching a stack. BSP then
            // releases exactly one CPU into the shared capture owner at a time.
            ((base + 104) as *mut u32).write(0);
            ((base + 108) as *mut u32).write(0);
            // Numeric guest-accessible witness pointer, copied before release.
            // The callback revalidates its mapping; copied code has no fixup.
            ((base + 112) as *mut u64).write(ptr::addr_of!(GUEST_ACK) as u64);
            ((base + AP_FAILURE_OFFSET) as *mut ApFailureObservation).write(ApFailureObservation {
                reason: 0, reserved: 0, observation: 0,
            });
            ptr::copy_nonoverlapping(wait as *const u8, (base + 256) as *mut u8, wait_bytes);
            ptr::copy_nonoverlapping(descriptors.gdt().as_ptr(), (base + 4096) as *mut u8, 40);
            ptr::copy_nonoverlapping(descriptors.tss().as_ptr(), (base + 4160) as *mut u8, 104);
            ptr::copy_nonoverlapping(descriptors.idt().as_ptr(), (base + 8192) as *mut u8, 4096);
        }
    }
    unsafe {
        BOOT_CFG = Some(cfg);
        BSP = bsp;
    }
    Ok(allocation)
}
/// Validate complete shared bootstrap closure before any physical INIT. The
/// low page uses architectural fixed-MTRR bytes; variable-only Mtrrs rejects it.
pub(super) unsafe fn validate(
    map: &[MemoryDescriptor],
    cfg: PagingConfig,
    mt: &Mtrrs,
    pat: u64,
    count: usize,
) -> Result<(), u64> {
    unsafe {
        validate_lapic(map, cfg, mt, pat)?;
    }
    for slot in 0..count {
        unsafe {
            mapped(
                map,
                cfg,
                mt,
                pat,
                boot_address(slot),
                BOOT_BYTES as u64,
                true,
                false,
            )?;
        }
    }
    let low = unsafe { LOW };
    if !ram_span(map,low,4096){admission_hint(130,low,4096,1);return Err(30);}
    if mt.default&0xc00!=0xc00{admission_hint(131,0x2ff,mt.default,0xc00);return Err(30);}
    let capability=unsafe{rdmsr(0xfe)};
    if capability&0x100==0{admission_hint(132,0xfe,capability,0x100);return Err(30);}
    let (msr,shift)=Mtrrs::fixed_range_register(low).ok_or_else(||{admission_hint(133,low,low,0x100000);30u64})?;
    let fixed=unsafe{rdmsr(msr)};
    if (fixed>>shift)&255!=6{admission_hint(134,msr as u64,fixed,shift as u64|(6u64<<32));return Err(30);}
    if pat&255!=6{admission_hint(135,0x277,pat,6);return Err(30);}
    let mut last=None;
    let translation = paging::translate(cfg, low, |address| {
        if !ram_span(map, address, 8) || !mt.page_is_wb(address & !4095) {
            admission_hint(108,address,u64::from(ram_span(map,address,8))|u64::from(mt.page_is_wb(address&!4095))<<1,3);
            return None;
        }
        let entry = unsafe { ptr::read_volatile(address as *const u64) };
        last=Some((address,entry));
        if entry & 0x18 != 0 {admission_hint(109,address,entry,0x18);None} else { Some(entry) }
    })
    .map_err(|error|{admission_walk(error,cfg,low,last);30u64})?;
    if translation.physical_address != low
        || !translation.writable
        || !translation.executable
        || (pat >> (translation.pat_index * 8)) & 255 != 6
    {
        let(predicate,observed,expected)=if translation.physical_address!=low{(110,translation.physical_address,low)}
            else if !translation.writable{(111,0,1)}else if !translation.executable{(112,0,1)}else{(114,pat,translation.pat_index as u64)};
        admission_hint(predicate,low,observed,expected);
        return Err(30);
    }
    Ok(())
}

// APM2 16.3.1/Figure16-2: ABA extends through bit51, not only bit31.
fn lapic_base_valid(base: u64) -> bool {
    base & !0x000f_ffff_ffff_fd00 == 0
        && base & 0x000f_ffff_ffff_f000 == 0xfee0_0000
        && base & 0x800 != 0
}

/// Observe the current CPU's fixed LAPIC mapping without reading MMIO first.
/// APM2 16.3.2 requires an uncacheable register aperture; 7.8.5/Table7-11
/// supplies the exact PAT/MTRR combination. Page-table backing remains WB RAM
/// under the adapter's existing coherent identity-access contract. No allocation
/// or cache-control mutation is performed. x2APIC performs no MMIO access.
pub(super) unsafe fn validate_lapic(
    map: &[MemoryDescriptor],
    cfg: PagingConfig,
    mt: &Mtrrs,
    pat: u64,
) -> Result<(), u64> {
    let base = unsafe { rdmsr(0x1b) };
    if !lapic_base_valid(base) {
        admission_hint(139,0x1b,base,0xfee00800);
        return Err(39);
    }
    if base & 0x400 != 0 {
        return Ok(());
    }
    unsafe { validate_uc_mmio(map, cfg, mt, pat, 0xfee0_0000) }.map_err(|_| 39u64)
}

pub(super) unsafe fn publish(
    bs: &BootServices,
    count: usize,
    base: u64,
    bytes: u64,
) -> Result<(), Status> {
    unsafe {
        (*ptr::addr_of_mut!(INTERFACE)).count = count as u64;
        (*ptr::addr_of_mut!(INTERFACE)).pool_base = base;
        (*ptr::addr_of_mut!(INTERFACE)).pool_bytes = bytes;
    }
    let status = unsafe {
        (bs.install_configuration_table)(&ACTIVATION_GUID, ptr::addr_of_mut!(INTERFACE).cast())
    };
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}
/// Returning MP reader: complete AP-local controls, capability, cache and all
/// retained resource mappings are admitted before the first physical INIT.
/// Firmware can still change AP state afterward; capture revalidates locally.
fn admission_observer() -> Result<resident::processors::Identity, resident::processors::Error> {
    admission_begin(3,u32::MAX,u32::MAX);
    let result = (|| -> Result<(), u64> {
        let processor = unsafe { cpu() }?;
        unsafe{if let Some(value)=&mut *ptr::addr_of_mut!(ADMISSION_CONTEXT){
            value.apic_id=processor.apic_id;
            value.processor=CPU_IDS[..CPU_COUNT].iter().position(|&id|id==processor.apic_id).map_or(u32::MAX,|v|v as u32);
        }}
        let current = unsafe { config(processor) }?;
        let vm_cr=unsafe { rdmsr(0xc0010114) };
        if vm_cr & 2 != 0 {
            admission_hint(38,0xc0010114,vm_cr,0);
            return Err(38);
        }
        let apic_base = unsafe { rdmsr(0x1b) };
        let expected = unsafe { BOOT_APIC_BASE };
        if !lapic_base_valid(apic_base) {
            admission_hint(138,0x1b,apic_base,expected);return Err(38);
        }
        let apic_features=__cpuid_count(1,0).ecx;
        if !native_apic_mode_supported(apic_features,
            expected&0x400!=0 || !cfg!(feature="native-resident-guest-startup")) {
            admission_hint(157,1,apic_features as u64,1<<21);return Err(38);
        }
        // INIT preserves EXTD: AP x2APIC cannot demote to BSP xAPIC.
        if apic_base&0x400!=0 && expected&0x400==0 {
            admission_hint(158,0x1b,apic_base,expected);return Err(38);
        }
        let mt = unsafe { mtrrs(processor.physical_bits) }?;
        let pat = unsafe { rdmsr(0x277) };
        let cr0: u64;
        unsafe {
            asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        }
        trace_detail(&(
            "ap-local-cache",
            processor.apic_id,
            cr0,
            current.cr3,
            unsafe { rdmsr(EFER) },
            pat,
            mt.default,
            mt.count,
            mt.physical_bits,
        ));
        for (index, &(base, mask)) in mt.variable.iter().take(mt.count).enumerate() {
            trace_detail(&("ap-variable-mtrr", processor.apic_id, index, base, mask));
        }
        let map = unsafe {
            core::slice::from_raw_parts(ptr::addr_of!(MAP).cast::<MemoryDescriptor>(), MAP_COUNT)
        };
        let cfg = unsafe { BOOT_CFG }.ok_or_else(||{admission_hint(140,0,0,1);38u64})?;
        if current.nxe != cfg.nxe || current.physical_bits != cfg.physical_bits {
            admission_hint(238,0,(current.physical_bits as u64)<<1|u64::from(current.nxe),
                (cfg.physical_bits as u64)<<1|u64::from(cfg.nxe));
            return Err(38);
        }
        let count = unsafe { CPU_COUNT };
        unsafe {
            validate_current_closure(map, current, &mt, pat, count)?;
            validate_owned_root(map, &mt, pat)?;
        }
        if apic_base & 0x400 == 0 {
            // The preceding cache walk precedes this first MMIO access.
            // Broadcast INIT/SIPI does not depend on ExtApicIdEn after reset.
            // physical.S may enable that bit only on the exact reviewed PPR
            // layout, after receiving SIPI and before resident capture.
            let version = unsafe { (0xfee0_0030 as *const u32).read_volatile() };
            if version & (1 << 31) != 0 {
                let signature = __cpuid_count(1, 0).eax;
                let feature = unsafe { (0xfee0_0400 as *const u32).read_volatile() };
                if signature != 0x00b4_0f40 || version != 0x8105_0010 || feature != 0x0004_0007 {
                    let(item,observed,expected):(u64,u32,u32)=if signature!=0x00b40f40{(1,signature,0x00b40f40)}
                        else if version!=0x81050010{(0xfee00030,version,0x81050010)}else{(0xfee00400,feature,0x40007)};
                    admission_hint(42,item,observed as u64,expected as u64);
                    trace_detail(&("ap-startup-routing", processor.apic_id, signature, version));
                    return Err(42);
                }
            }
        }
        for slot in 0..count {
            let d = unsafe { DIRECTORIES[slot] };
            unsafe {
                mapped(
                    map,
                    current,
                    &mt,
                    pat,
                    d.arena_base,
                    d.arena_bytes,
                    true,
                    true,
                )?;
                host_closure(&d, map, processor, &mt, pat)?;
            }
        }
        // This is allocation/bootstrap admission, not the final cache bank.
        // F7 CpuDxe synchronizes BSP/AP MTRRs in its EBS callbacks. Capture the
        // authoritative bank only after original EBS succeeds and all APs are
        // under our bootstrap owner, before any guest entry.
        Ok(())
    })();
    result.map_err(|code|resident::processors::Error::Admission(admission_end(code)))?;
    let identity=resident::processors::capture_identity().map_err(|error|match error{
        resident::processors::Error::Admission(mut value)=>{
            if let Some(context)=unsafe{ADMISSION_CONTEXT}{value.apic_id=context.apic_id;value.processor=context.processor;}
            resident::processors::Error::Admission(value)
        }});
    admission_clear();
    identity
}
pub(super) unsafe fn admit_processors(bs: &BootServices) -> Result<(), Status> {
    admission_begin(1,unsafe{BSP} as u32,unsafe{CPU_IDS[BSP]});
    unsafe { build_owned_root() }.map_err(|code|admission_refused(admission_end(code)))?;
    admission_clear();
    let inventory =
        unsafe { resident::processors::inspect_with(bs, admission_observer) }.map_err(|error| {
            admission_clear();
            trace_detail(&error);
            let resident::processors::Error::Admission(value)=error;admission_refused(value)
        })?;
    if inventory.processors().len() != unsafe { CPU_COUNT }
        || inventory.bsp_number() != unsafe { BSP }
        || inventory
            .processors()
            .iter()
            .enumerate()
            .any(|(slot, p)| p.identity.apic_id != unsafe { CPU_IDS[slot] })
    {
        let(observed,expected,item)=if inventory.processors().len()!=unsafe{CPU_COUNT}{(inventory.processors().len()as u64,unsafe{CPU_COUNT}as u64,0)}
            else if inventory.bsp_number()!=unsafe{BSP}{(inventory.bsp_number()as u64,unsafe{BSP}as u64,1)}
            else{let(slot,p)=inventory.processors().iter().enumerate().find(|(s,p)|p.identity.apic_id!=unsafe{CPU_IDS[*s]}).unwrap();
                (p.identity.apic_id as u64,unsafe{CPU_IDS[slot]}as u64,2+slot as u64)};
        let mut failure=AdmissionFailure::new(5,1,item,observed,expected,0);
        if item>=2 { failure.processor=(item-2) as u32;failure.apic_id=observed as u32; }
        else {failure.processor=unsafe{BSP} as u32;failure.apic_id=unsafe{CPU_IDS[BSP]};}
        return Err(admission_refused(failure));
    }
    Ok(())
}

#[cfg(feature = "native-resident-boot")]
unsafe fn cache_capture() -> *mut svmvisor_hypervisor::svm::native_cache::CacheCapture {
    unsafe { (DIRECTORIES[0].pool_base + abi::CACHE_CAPTURE_OFFSET) as *mut _ }
}

#[cfg(feature = "native-resident-boot")]
unsafe fn cache_failure(operation: u32, slot: usize,
    f: svmvisor_hypervisor::svm::native_cache::CacheAdmissionFailure) -> u64
{
    CACHE_SURVEY.abort();
    #[cfg(feature = "native-cache-survey-fixture")]
    trace_detail(&("cache-survey-failure",operation,slot,f.predicate,f.index,f.observed,f.expected));
    let mut value = AdmissionFailure::new(operation,f.predicate,f.index as u64,f.observed,f.expected,0);
    value.processor = slot as u32;
    value.apic_id = if slot < unsafe { CPU_COUNT } { unsafe { CPU_IDS[slot] } } else { u32::MAX };
    let _ = admission_refused(value);
    48
}

/// Serial post-EBS writer; caller already admitted its CPU and high-memory
/// callback closure. The publication mask orders the complete bank write.
#[cfg(feature = "native-resident-boot")]
unsafe fn sample_cache(processor: Cpu, slot: usize) -> Result<(), u64> {
    use svmvisor_hypervisor::svm::native_cache::CacheAdmissionFailure;
    let sample = unsafe { cache_observation_detailed(processor) };
    let sample = sample.and_then(|value| {
        #[cfg(feature = "native-cache-survey-fixture")]
        trace_detail(&("cache-survey-sample",slot,value.default,value.variable[7].0,value.fixed[7]));
        let capture = unsafe { &mut *cache_capture() };
        if !capture.seed(slot,value) {
            let (count,valid) = capture.capture_state();
            Err(CacheAdmissionFailure::new(11,slot as u32,valid as u64,count as u64))
        } else { Ok(()) }
    });
    if let Err(f) = sample {
        unsafe { ptr::addr_of_mut!(CACHE_SAMPLE_FAILURES).cast::<CacheAdmissionFailure>().add(slot).write(f); }
        let _ = CACHE_FAILURE_SLOT.compare_exchange(u32::MAX,slot as u32,Ordering::Release,Ordering::Relaxed);
        CACHE_SURVEY.abort();
        return Err(48);
    }
    if !CACHE_SURVEY.complete_sample(slot) { CACHE_SURVEY.abort(); return Err(48); }
    Ok(())
}

#[cfg(feature = "native-resident-boot")]
unsafe fn report_cache_survey_failure() -> u64 {
    let slot = CACHE_FAILURE_SLOT.load(Ordering::Acquire) as usize;
    if slot < unsafe { CPU_COUNT } {
        let f = unsafe { ptr::addr_of!(CACHE_SAMPLE_FAILURES)
            .cast::<svmvisor_hypervisor::svm::native_cache::CacheAdmissionFailure>().add(slot).read() };
        unsafe { cache_failure(4,slot,f) }
    } else { 48 }
}

/// Before arm/VMRUN: owned APs retain the captured callback and park. BSP
/// consumes every sample and publishes the owner before any activation release.
/// No firmware service, allocation or routing write occurs here.
#[cfg(feature = "native-resident-boot")]
pub(super) unsafe fn cache_sample_before_activation(processor: Cpu, slot: usize) -> Result<(), u64> {
    if !unsafe { (&*cache_capture()).enabled() } { return Ok(()); }
    if unsafe { is_bsp(slot) } {
        return if CACHE_SURVEY.admitted() { Ok(()) } else { Err(48) };
    }
    #[cfg(feature = "native-cache-survey-fixture")]
    if slot == 1 { for _ in 0..100_000 { core::hint::spin_loop(); } }
    unsafe { sample_cache(processor,slot) }?;
    let release = unsafe { &*((boot_address(slot)+108) as *const AtomicU32) };
    for _ in 0..0x7fff_ffffu32 {
        if CACHE_SURVEY.failed() || interface().failed.load(Ordering::Acquire) != 0 { return Err(48); }
        if release.load(Ordering::Acquire) == 2 {
            return if CACHE_SURVEY.admitted() { Ok(()) } else { Err(48) };
        }
        core::hint::spin_loop();
    }
    CACHE_SURVEY.abort();
    Err(48)
}

#[cfg(feature = "native-resident-boot")]
unsafe fn finish_cache_survey() -> Result<(), u64> {
    let capture = unsafe { &*cache_capture() };
    let ids = unsafe { core::slice::from_raw_parts(ptr::addr_of!(CPU_IDS).cast::<u32>(), CPU_COUNT) };
    capture.agrees_with_bsp_detailed(unsafe { BSP },ids.len())
        .map_err(|(slot,f)| unsafe { cache_failure(6,slot,f) })?;
    for slot in 0..ids.len() {
        capture.domain_mask_detailed(slot,ids).map_err(|(slot,f)| unsafe { cache_failure(7,slot,f) })?;
    }
    let owner = unsafe { &mut *((DIRECTORIES[0].pool_base+abi::CACHE_OWNER_OFFSET)
        as *mut svmvisor_hypervisor::svm::native_cache::CacheOwner) };
    owner.initialize_detailed(capture,ids).map_err(|(slot,f)| unsafe { cache_failure(8,slot,f) })?;
    if !CACHE_SURVEY.admit(ids.len()) { CACHE_SURVEY.abort(); return Err(48); }
    #[cfg(feature = "native-cache-survey-fixture")]
    trace_detail(&("cache-survey-admitted",ids.len()));
    Ok(())
}

/// Before MP admission/publication only: the complete callback lives in the
/// retained DXE image, and all raw per-CPU entries/tables live in the pool.
/// No firmware paging page is copied or linked into the owned root. APM2
/// 5.3.3/5.4; UEFI2.11 7.2/Table7.10 and 8.4.1 runtime image fixups.
unsafe fn build_owned_root() -> Result<(), u64> {
    let cfg = unsafe { BOOT_CFG }.ok_or_else(||{admission_hint(140,0,0,1);43u64})?;
    let mt = unsafe { mtrrs(cfg.physical_bits) }?;
    let pat = unsafe { rdmsr(0x277) };
    let map = unsafe {
        core::slice::from_raw_parts(ptr::addr_of!(MAP).cast::<MemoryDescriptor>(), MAP_COUNT)
    };
    let (image, bytes) = unsafe { IMAGE };
    let tables = unsafe { &mut *ptr::addr_of_mut!(AP_TABLES) };
    tables
        .initialize(ptr::addr_of!(AP_TABLES) as u64)
        .map_err(|error|{admission_hint(141,ptr::addr_of!(AP_TABLES)as u64,error as u64,0);43u64})?;
    let count = unsafe { CPU_COUNT };
    for (base, length, raw) in
        core::iter::once((image, bytes, false)).chain((0..count).map(|slot| {
            let d = unsafe { DIRECTORIES[slot] };
            (d.arena_base, d.arena_bytes, true)
        }))
    {
        let end = base.checked_add(length).ok_or_else(||{admission_hint(142,base,length,u64::MAX-base);43u64})?;
        if base & 4095 != 0 || length == 0 || length > 16 * 1024 * 1024 || end > 1 << 40 {
            admission_hint(142,base,length,16*1024*1024);
            return Err(43);
        }
        // Validate current firmware accesses before reading/copying leaf policy.
        let mut cursor = base;
        while cursor < end {
            let chunk = (end - cursor).min(2 * 1024 * 1024);
            unsafe {
                mapped(map, cfg, &mt, pat, cursor, chunk, raw, raw)?;
            }
            cursor += chunk;
        }
        let mut page = base;
        while page < end {
            let mut last=None;
            let t = paging::translate(cfg, page, |address| {
                // The preceding mapped() proves this exact table-fetch closure.
                let entry=unsafe { ptr::read_volatile(address as *const u64) };last=Some((address,entry));Some(entry)
            })
            .map_err(|error|{admission_walk(error,cfg,page,last);43u64})?;
            let wait_page = (0..count).any(|slot| page == boot_address(slot));
            tables
                .map_page(page, t.writable, t.executable || wait_page, 0)
                .map_err(|error|{admission_hint(143,page,error as u64,0);43u64})?;
            page += 4096;
        }
    }
    tables
        .map_page(unsafe { LOW }, true, true, 0)
        .map_err(|error|{admission_hint(144,unsafe{LOW},error as u64,0);43u64})?;
    // A later native callback may read the fixed LAPIC in xAPIC mode. Admit
    // effective UC using the existing MTRR owner, without borrowing its mapping.
    if unsafe { BOOT_APIC_BASE } & 0x400 == 0 {
        let index = (0..8u8)
            .find(|index| mt.page_is_uc(0xfee0_0000, ((pat >> (*index * 8)) & 255) as u8))
            .ok_or_else(||{admission_hint(145,0xfee00000,pat,0);43u64})?;
        tables
            .map_page(0xfee0_0000, true, false, index)
            .map_err(|error|{admission_hint(146,0xfee00000,error as u64,index as u64);43u64})?;
    }
    for slot in 0..count {
        unsafe {
            ((boot_address(slot) + 72) as *mut u32).write(CPU_IDS[slot]);
            // A BSP or unknown identity never receives an AP stack/record.
            svmvisor_ap_boots[slot] = if slot == BSP { 0 } else { boot_address(slot) };
        }
    }
    unsafe { svmvisor_ap_count = count as u32 };
    Ok(())
}

/// Both MP admission and pre-INIT rechecks prove the independent root's actual
/// callback closure. Original firmware configuration remains BOOT_CFG; APs
/// capture this root with NXE explicitly enabled by physical.S.
unsafe fn validate_owned_root(map: &[MemoryDescriptor], mt: &Mtrrs, pat: u64) -> Result<(), u64> {
    let mut cfg = unsafe { BOOT_CFG }.ok_or_else(||{admission_hint(140,0,0,1);43u64})?;
    cfg.cr3 = ptr::addr_of!(AP_TABLES) as u64;
    cfg.nxe = true;
    let count = unsafe { CPU_COUNT };
    unsafe {
        validate(map, cfg, mt, pat, count)?;
    }
    let (image, bytes) = unsafe { IMAGE };
    let mut cursor = image;
    while cursor < image + bytes {
        let chunk = (image + bytes - cursor).min(2 * 1024 * 1024);
        unsafe {
            mapped(map, cfg, mt, pat, cursor, chunk, false, false)?;
        }
        cursor += chunk;
    }
    for slot in 0..count {
        let d = unsafe { DIRECTORIES[slot] };
        unsafe {
            mapped(map, cfg, mt, pat, d.arena_base, d.arena_bytes, true, true)?;
        }
        let mut last=None;
        let t = paging::translate(cfg, boot_address(slot), |address| unsafe {
            let entry=(&*ptr::addr_of!(AP_TABLES)).read(address);if let Some(value)=entry{last=Some((address,value));}entry
        })
        .map_err(|error|{admission_walk(error,cfg,boot_address(slot),last);43u64})?;
        if !t.executable || !t.writable || t.user {
            admission_hint(150,boot_address(slot),u64::from(t.writable)|u64::from(t.executable)<<1|u64::from(t.user)<<2,3);
            return Err(43);
        }
    }
    Ok(())
}

/// Firmware MP callbacks and a normal loader may select different roots. Prove
/// the current identity closure instead of treating the installation CR3 value
/// as a permanent lease. All checks precede physical INIT.
unsafe fn validate_current_closure(
    map: &[MemoryDescriptor],
    cfg: PagingConfig,
    mt: &Mtrrs,
    pat: u64,
    count: usize,
) -> Result<(), u64> {
    unsafe {
        validate(map, cfg, mt, pat, count)?;
    }
    let (image, bytes) = unsafe { IMAGE };
    let mut cursor = image;
    while cursor < image + bytes {
        let chunk = (image + bytes - cursor).min(2 * 1024 * 1024);
        unsafe {
            mapped(map, cfg, mt, pat, cursor, chunk, false, false)?;
        }
        cursor += chunk;
    }
    // The image walk above makes AP_TABLES safe to read through this current
    // root. Its saved leaf permissions describe every original callback page;
    // only the copied AP wait page needs execution solely under the owned root.
    let mut owned = cfg;
    owned.cr3 = ptr::addr_of!(AP_TABLES) as u64;
    owned.nxe = true;
    let mut page = image;
    while page < image + bytes {
        let mut last=None;
        let expected = paging::translate(owned, page, |address| unsafe {
            let entry=(&*ptr::addr_of!(AP_TABLES)).read(address);if let Some(value)=entry{last=Some((address,value));}entry
        })
        .map_err(|error|{admission_walk(error,owned,page,last);43u64})?;
        let wait_page = (0..count).any(|slot| page == boot_address(slot));
        unsafe {
            mapped(
                map,
                cfg,
                mt,
                pat,
                page,
                4096,
                expected.writable,
                expected.executable && !wait_page,
            )?;
        }
        page += 4096;
    }
    for slot in 0..count {
        let d = unsafe { DIRECTORIES[slot] };
        unsafe {
            mapped(map, cfg, mt, pat, d.arena_base, d.arena_bytes, true, true)?;
        }
    }
    Ok(())
}
unsafe fn enable_x2apic() -> Result<(), u64> {
    let base = unsafe { rdmsr(0x1b) };
    if base & 0x800 == 0 || __cpuid_count(1, 0).ecx & (1 << 21) == 0 {
        return Err(31);
    }
    unsafe {
        wrmsr(0x1b, base | 0x400);
    }
    Ok(())
}
/// Caller exclusively owns BSP after successful EBS return, still using the
/// admitted root/image, IF=0. The BSP's original root remains its guest-owned
/// continuation; AP guests use the independent retained bootstrap root. No
/// original firmware paging structures are needed by APs after startup. The
/// low LoaderCode page is consumed before success. No firmware calls or allocation remain. Failures
/// retain both runtime and bootstrap resources. This diagnostic has bounded
/// poll/settle iterations, not a calibrated physical startup timing claim.
pub(super) unsafe extern "efiapi" fn start() -> u64 {
    let flags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags));
    }
    if flags & 0x200 != 0 || STARTED.load(Ordering::Acquire) || !READY.load(Ordering::Acquire) {
        return 32;
    }
    // Firmware/consumer page tables may have changed since MP observation.
    // Recheck before changing APIC mode, publishing a target or issuing MMIO.
    let preflight = (|| -> Result<(), u64> {
        let processor = unsafe { cpu() }?;
        let cfg = unsafe { config(processor) }?;
        let mt = unsafe { mtrrs(processor.physical_bits) }?;
        let map = unsafe {
            core::slice::from_raw_parts(ptr::addr_of!(MAP).cast::<MemoryDescriptor>(), MAP_COUNT)
        };
        let base = unsafe { rdmsr(0x1b) };
        if !lapic_base_valid(base)
            || !native_apic_mode_supported(
                __cpuid_count(1, 0).ecx,
                base & 0x400 != 0 || !cfg!(feature = "native-resident-guest-startup"),
            )
            || (base & 0x400 == 0 && unsafe { BOOT_APIC_BASE } & 0x400 != 0)
        {
            return Err(39);
        }
        let pat = unsafe { rdmsr(0x277) };
        unsafe {
            validate_current_closure(map, cfg, &mt, pat, CPU_COUNT)?;
            validate_owned_root(map, &mt, pat)?;
        }
        if base & 0x400 == 0 {
            let version = unsafe { (0xfee0_0030 as *const u32).read_volatile() };
            if version & (1 << 31) != 0 {
                let signature = __cpuid_count(1, 0).eax;
                let feature = unsafe { (0xfee0_0400 as *const u32).read_volatile() };
                let control = unsafe { (0xfee0_0410 as *const u32).read_volatile() };
                let ids = unsafe {
                    core::slice::from_raw_parts(ptr::addr_of!(CPU_IDS).cast::<u32>(), CPU_COUNT)
                };
                #[cfg(feature = "native-resident-boot")]
                unsafe {
                    let mut cpu_ids = [0; 32];
                    cpu_ids[..ids.len()].copy_from_slice(ids);
                    card_boot::routing_observation(
                        svmvisor_dxe::diagnostics::resident_boot::BspRoutingObservation {
                            apic_base: base, signature, version, feature, control,
                            bsp_apic_id: processor.apic_id,
                            processor_count: ids.len() as u32, cpu_ids,
                        },
                    );
                }
                // APM2 16.5/Table16-4 and PPR57896 APIC300/410: the
                // all-excluding-self bootstrap does not use a unicast ID.
                // Preserve the BSP's real matching width; runtime startup
                // checks each target under the shared routing guard.
                if let Err(error) = svmvisor_hypervisor::svm::ipi::admit_native_xapic_extended_profile(
                    signature, version, feature, control,
                ) {
                    #[cfg(feature = "native-resident-boot")]
                    unsafe {
                        use svmvisor_dxe::diagnostics::resident_boot::BspRoutingPredicate as P;
                        use svmvisor_hypervisor::svm::ipi::NativeXApicProfileError as E;
                        let (predicate, observed, expected) = match error {
                            E::Signature { actual } => (P::Signature, actual, 0x00b4_0f40),
                            E::Version { actual } => (P::Version, actual, 0x8105_0010),
                            E::Feature { actual } => (P::Feature, actual, 0x0004_0007),
                            E::ReservedControl { actual } => (P::ExtendedControl, actual & !7, 0),
                        };
                        card_boot::routing_failure(predicate, observed, expected);
                    }
                    let _ = error;
                    return Err(42);
                }
                if svmvisor_hypervisor::svm::ipi::NativeIcr::admit(processor.apic_id, ids)
                    .and_then(|owner| owner.admit_apic_base(base)).is_err() {
                    #[cfg(feature = "native-resident-boot")]
                    unsafe {
                        card_boot::routing_failure(
                            svmvisor_dxe::diagnostics::resident_boot::BspRoutingPredicate::Topology,
                            processor.apic_id, ids.len() as u32,
                        );
                    }
                    return Err(42);
                }
            }
        }
        Ok(())
    })();
    if let Err(code) = preflight {
        return code;
    }
    if STARTED.swap(true, Ordering::AcqRel) {
        return 32;
    }
    #[cfg(feature = "native-resident-boot")]
    let cache_survey = unsafe { (&*cache_capture()).enabled() };
    #[cfg(not(feature = "native-resident-boot"))]
    let cache_survey = false;
    #[cfg(feature = "native-resident-boot")]
    if cache_survey {
        let processor = match unsafe { cpu() } { Ok(value) => value, Err(code) => return code };
        if unsafe { sample_cache(processor,BSP) }.is_err() {
            return unsafe { report_cache_survey_failure() };
        }
    }
    #[cfg(feature = "native-resident-guest-startup")]
    {
        // Capture before any physical command or APIC mode change. Restoring
        // hardware ICR low would send another IPI; arm seeds its existing
        // guest readback overlay from this retained value instead (APM2 16.5).
        let base = unsafe { rdmsr(0x1b) };
        let initial = if base & 0x400 != 0 {
            unsafe { rdmsr(0x830) }
        } else {
            if unsafe { wait_icr_idle() }.is_err() {
                return 40;
            }
            unsafe {
                (u64::from((0xfee0_0310 as *const u32).read_volatile() >> 24) << 32)
                    | u64::from((0xfee0_0300 as *const u32).read_volatile())
            }
        };
        unsafe { BSP_INITIAL_ICR = initial };
        BSP_INITIAL_ICR_READY.store(true, Ordering::Release);
        trace_detail(&("native-bsp-initial-icr", initial & !0x31000));
    }
    #[cfg(not(feature = "native-resident-guest-startup"))]
    if unsafe { enable_x2apic() }.is_err() {
        return 31;
    }
    let apic_base = unsafe { rdmsr(0x1b) };
    if !lapic_base_valid(apic_base) {
        return 31;
    }
    let count = interface().count as usize;
    // PI inspect_with requires total=enabled, every CPU healthy, and a complete
    // returned observation. No subset or disabled processor can be broadcast to.
    // Publish every immutable private record before the first physical INIT.
    for slot in 0..count {
        unsafe {
            ((boot_address(slot) + 40) as *mut u32).write((apic_base & 0x400) as u32);
        }
    }
    unsafe { asm!("mfence", options(nostack, preserves_flags)) };
    // APM2 16.5/Table16-4 pp643-644 permits all-excluding-self for both
    // edge INIT and SIPI. No APIC physical-ID width participates in shorthand
    // matching, including the PPR57896 APIC410 reset-to-four-bit interval.
    if let Err(code) = unsafe { send_startup(apic_base, 0x000c_4500) } {
        interface().failed.store(u32::MAX, Ordering::Release);
        return code;
    }
    for _ in 0..10000 {
        core::hint::spin_loop();
    }
    if let Err(code) = unsafe { send_startup(apic_base, 0x000c_0600 | (LOW >> 12) as u32) } {
        interface().failed.store(u32::MAX, Ordering::Release);
        return code;
    }
    // First collect all fresh banks with APs parked in the captured callback.
    // Only the second pass can reach arm/VMRUN, after owner publication. Other
    // profiles keep their existing single activation pass.
    for pass in 0..=usize::from(cache_survey) {
        #[cfg(feature = "native-resident-boot")]
        if pass == 1 {
            if let Err(code) = unsafe { finish_cache_survey() } { return code; }
        }
        for iteration in 0..count {
            let slot = if cfg!(feature = "native-cache-survey-fixture") && pass == 0 {
                count-1-iteration
            } else { iteration };
            if slot == unsafe { BSP } {
                continue;
            }
            let release = unsafe { &*((boot_address(slot) + 108) as *const AtomicU32) };
            #[cfg(feature = "native-resident-boot")]
            unsafe {
                card_boot::stage(3, slot as u32, 0)
            };
            release.store((pass+1) as u32, Ordering::Release);
            let mut done = false;
            for _ in 0..20_000_000 {
                #[cfg(feature = "native-resident-boot")]
                if cache_survey && CACHE_SURVEY.failed() {
                    return unsafe { report_cache_survey_failure() };
                }
                if svmvisor_ap_discovery_failed.load(Ordering::Acquire) != 0 {
                    // No slot is authoritative for an unknown/duplicate identity.
                    interface().failed.store(u32::MAX, Ordering::Release);
                    return 45;
                }
                let failed = interface().failed.load(Ordering::Acquire);
                if failed != 0 {
                    // AP owns its BOOT record until locked failure publication.
                    // The acquire above orders these final volatile field reads.
                    // Only the serially released AP is authoritative for this batch.
                    #[cfg(feature = "native-resident-boot")]
                    if failed == 1u32 << slot {
                        let sample = unsafe {
                            ((boot_address(slot) + AP_FAILURE_OFFSET) as *const ApFailureObservation)
                                .read_volatile()
                        };
                        unsafe { card_boot::ap_failure(slot as u32, count as u32, sample) };
                    }
                    return 33;
                }
                #[cfg(feature = "native-resident-boot")]
                if cache_survey && pass == 0 && CACHE_SURVEY.sampled(slot) {
                    done = true;
                    break;
                }
                if interface().completed.load(Ordering::Acquire) & (1u32 << slot) != 0 {
                    // The copied guest continuation stores CR3 before its locked
                    // completion publication; this acquire precedes the read.
                    let observed = unsafe { ((boot_address(slot) + 96) as *const u64).read_volatile() };
                    trace_detail(&("native-ap-owned-root", slot, observed));
                    if observed != ptr::addr_of!(AP_TABLES) as u64 {
                        interface().failed.fetch_or(1u32 << slot, Ordering::AcqRel);
                        return 44;
                    }
                    done = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !done {
                #[cfg(feature = "native-resident-boot")]
                if cache_survey { CACHE_SURVEY.abort(); }
                interface().failed.fetch_or(1u32 << slot, Ordering::AcqRel);
                return 34;
            }
        }
    }
    unsafe {
        #[cfg(feature = "native-resident-boot")]
        card_boot::stage(4, BSP as u32, 0);
        abi::svmvisor_resident_callback(ptr::null_mut(), COOKIE as *mut c_void);
    }
    // A normal refusal also returns from the callback. Only the immutable
    // post-VMMCALL guest epilogue publishes this bit after NativeBootstrapAck.
    if GUEST_ACK.load(Ordering::Acquire) & (1u32 << unsafe { BSP }) == 0 {
        return 35;
    }
    interface()
        .completed
        .fetch_or(1u32 << unsafe { BSP }, Ordering::Release);
    0
}
/// BSP-only before resident capture, IF=0 and admitted identity-mapped LAPIC.
/// APM2 16.5/16.11: all-excluding-self ignores destination high, so preserve the
/// BSP's xAPIC ICR high half. Bounded polling detects stalled xAPIC delivery.
unsafe fn send_startup(base: u64, command: u32) -> Result<(), u64> {
    unsafe {
        if base & 0x400 != 0 {
            let high = rdmsr(0x830) & !0xffff_ffff;
            wrmsr(0x830, high | u64::from(command));
        } else {
            // Do not replace an IPI still owned by firmware/the prior sender.
            wait_icr_idle().map_err(|_| 40u64)?;
            (0xfee00300 as *mut u32).write_volatile(command);
            wait_icr_idle().map_err(|_| 41u64)?;
        }
    }
    Ok(())
}
/// Same admitted BSP xAPIC mapping and exclusive ICR ownership as send_startup.
unsafe fn wait_icr_idle() -> Result<(), ()> {
    for _ in 0..1_000_000 {
        if unsafe { (0xfee00300 as *const u32).read_volatile() } & 0x1000 == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(())
}
