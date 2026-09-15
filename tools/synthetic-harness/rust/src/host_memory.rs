//! Host-only four-level identity paging for the disposable emulator image.
//! Supervisor PTEs enforce RX text, RO/NX constants and RW/NX mutable storage.
//! Unlike NPT, U/S is clear throughout. The initial and restored maps contain
//! only the linked image, with page zero absent. A finite preemption owner may
//! temporarily map the single LAPIC page as supervisor RW/NX/UC. The host and double-fault stacks each have
//! an absent guard page on both sides. This is not a native address-space switch.

use core::{arch::asm, ptr};

const PAGE: u64 = 4096;
const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const NX: u64 = 1 << 63;

#[repr(C, align(4096))]
struct Tables([[u64; 512]; 4]);
static mut HOST_TABLES: Tables = Tables([[0; 512]; 4]);
#[repr(C, align(4096))]
struct ApicTables([[u64; 512]; 2]);
static mut APIC_TABLES: ApicTables = ApicTables([[0; 512]; 2]);
#[cfg(feature = "concurrent-smp")]
#[repr(C, align(4096))]
struct StartupTable([u64;512]);
#[cfg(feature = "concurrent-smp")]
static mut STARTUP_TABLE: StartupTable = StartupTable([0;512]);

/// Add/remove one host-only supervisor RW/NX/UC identity leaf for the emulated
/// LAPIC. Guest NPT is unchanged. PAT3 and the MTRR type must both be UC;
/// APM2 7.8.2/7.8.5 selects PAT3 with PCD|PWT and combines UC+UC into UC.
/// # Safety
/// Exclusive BSP IF=0, installed HOST_TABLES, no live table borrows, no use of
/// the MMIO page outside this finite ownership interval. The initial mapping
/// contains no APIC alias. The post-EBS fixture supplies retained ownership;
/// general native platforms remain outside this fixture.
pub unsafe fn map_timer(enabled: bool) {
    let root = ptr::addr_of_mut!(HOST_TABLES).cast::<u64>();
    let storage = ptr::addr_of_mut!(APIC_TABLES).cast::<u64>();
    let branch = unsafe { root.add(512 + 3) };
    if enabled {
        unsafe {
            validate_apic_memory_type();
        }
        assert_eq!(unsafe { ptr::read_volatile(branch) }, 0);
        unsafe {
            ptr::write_bytes(storage, 0, 1024);
            // FEE00000: PDPT=3, PD=503, PT=0.
            ptr::write_volatile(storage.add(503), storage as u64 + PAGE | PRESENT | WRITE);
            ptr::write_volatile(storage.add(512), 0xfee00000 | PRESENT | WRITE | NX | 0x18);
            ptr::write_volatile(branch, storage as u64 | PRESENT | WRITE);
        }
    } else {
        assert_eq!(
            unsafe { ptr::read_volatile(branch) } & !0x20,
            storage as u64 | PRESENT | WRITE
        );
        unsafe {
            ptr::write_volatile(branch, 0);
        }
    }
    unsafe {
        asm!("mov cr3, {}", in(reg) root as u64, options(nostack));
    }
    assert_eq!(unsafe { ptr::read_volatile(branch) } != 0, enabled);
}

/// # Safety
/// CPL0 emulator CPU with PAT/MTRR MSRs implemented; only capability-gated
/// read access. Every CPU using the shared APIC leaf must independently admit
/// PAT3 and its MTRR type as UC (APM2 7.7/7.8.2/7.8.5).
pub unsafe fn validate_apic_memory_type() {
    assert_ne!(core::arch::x86_64::__cpuid(1).edx & (1 << 16), 0);
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x277u32, out("eax") low, out("edx") high, options(nostack));
    }
    let pat = u64::from(low) | (u64::from(high) << 32);
    assert_eq!((pat >> 24) & 0xff, 0, "host PAT3 must be UC");
    assert!(unsafe { apic_mtrr_uc() }, "host APIC MTRR type must be UC");
}

// APM2 7.7: FEE00000 is outside fixed MTRRs. UC matching any variable range
// wins; otherwise accept only an unmatched UC default or disabled MTRRs.
unsafe fn apic_mtrr_uc() -> bool {
    unsafe fn msr(index: u32) -> u64 {
        let low: u32;
        let high: u32;
        unsafe {
            asm!("rdmsr", in("ecx") index, out("eax") low, out("edx") high, options(nostack));
        }
        u64::from(low) | (u64::from(high) << 32)
    }
    assert_ne!(core::arch::x86_64::__cpuid(1).edx & (1 << 12), 0);
    let default = unsafe { msr(0x2ff) };
    if default & (1 << 11) == 0 {
        return true;
    }
    let count = unsafe { msr(0xfe) } & 0xff;
    assert!(count <= 32);
    let width = core::arch::x86_64::__cpuid(0x80000008).eax & 0xff;
    assert!((32..=52).contains(&width));
    let address_mask = ((1u64 << width) - 1) & !0xfff;
    let mut matched = false;
    for range in 0..count as u32 {
        let base = unsafe { msr(0x200 + range * 2) };
        let mask = unsafe { msr(0x201 + range * 2) };
        if mask & (1 << 11) != 0
            && 0xfee00000 & (mask & address_mask) == base & (mask & address_mask)
        {
            matched = true;
            if base & 0xff == 0 {
                return true;
            }
        }
    }
    !matched && default & 0xff == 0
}

unsafe extern "C" {
    static image_start: u8;
    static text_start: u8;
    static text_end: u8;
    static rodata_start: u8;
    static rodata_end: u8;
    static data_start: u8;
    static data_end: u8;
    static bss_start: u8;
    static bss_end: u8;
    static image_bss_end: u8;
    static host_stack_guard: u8;
    static host_stack_guard_end: u8;
    static host_stack: u8;
    static host_stack_top: u8;
    static host_stack_high_guard: u8;
    static host_stack_high_guard_end: u8;
    static host_df_guard: u8;
    static host_double_fault_stack: u8;
    static host_df_high_guard: u8;
    #[cfg(feature = "concurrent-smp")]
    static host_ap_stack_guard: u8;
    #[cfg(feature = "concurrent-smp")]
    static host_ap_stack_high_guard: u8;
    #[cfg(feature = "concurrent-smp")]
    static host_ap_df_guard: u8;
    #[cfg(feature = "concurrent-smp")]
    static host_ap_df_high_guard: u8;
}

/// Replace the permissive bootstrap CR3 with the image-only page tables.
///
/// # Safety
/// Call once on the emulator's BSP, with interrupts disabled, from the
/// identity-mapped linked image and its image-backed stack. The installed
/// GDT/TSS/IDT and handlers must also lie in that image. No other CPU may walk
/// or reference these static tables. The CPU must support NX; NXE, WP and
/// four-level long mode are checked below. The linker must use rust-linker.ld.
pub unsafe fn install() {
    // Linker boundary symbols may share addresses; prevent LLVM from treating
    // separately declared extern statics as distinct allocated objects.
    let start = core::hint::black_box(ptr::addr_of!(image_start) as u64);
    let text = core::hint::black_box(ptr::addr_of!(text_start) as u64);
    let text_limit = core::hint::black_box(ptr::addr_of!(text_end) as u64);
    let rodata = core::hint::black_box(ptr::addr_of!(rodata_start) as u64);
    let rodata_limit = core::hint::black_box(ptr::addr_of!(rodata_end) as u64);
    let data = core::hint::black_box(ptr::addr_of!(data_start) as u64);
    let data_limit = core::hint::black_box(ptr::addr_of!(data_end) as u64);
    let bss = core::hint::black_box(ptr::addr_of!(bss_start) as u64);
    let bss_limit = core::hint::black_box(ptr::addr_of!(bss_end) as u64);
    let limit = core::hint::black_box(ptr::addr_of!(image_bss_end) as u64);
    let boundaries = [
        start,
        text,
        text_limit,
        rodata,
        rodata_limit,
        data,
        data_limit,
        bss,
        bss_limit,
        limit,
    ];
    assert!(boundaries.iter().all(|address| address & (PAGE - 1) == 0));
    assert!(boundaries.windows(2).all(|pair| pair[0] <= pair[1]));
    // One PT maps the image's 2 MiB window. Firmware may choose any page-aligned
    // 1 MiB arena wholly within a window below 1 GiB; 1 MiB is the link address.
    assert!(start >= 0x100000);
    assert!(start <= 0x3ff00000);
    let window = start & !0x1fffff;
    assert!(start + 0x100000 <= window + 0x200000);
    let directory_index = (start >> 21) as usize;
    assert_eq!(text, start + PAGE);
    assert!(text_limit > text);
    assert_eq!(text_limit, rodata);
    assert_eq!(rodata_limit, data);
    assert_eq!(data_limit, bss);
    assert_eq!(bss_limit, limit);
    assert!(limit <= start.checked_add(0xff000).unwrap());
    assert!(limit <= window + 0x200000);

    let guard = core::hint::black_box(ptr::addr_of!(host_stack_guard) as u64);
    let guard_end = core::hint::black_box(ptr::addr_of!(host_stack_guard_end) as u64);
    let stack = core::hint::black_box(ptr::addr_of!(host_stack) as u64);
    let stack_top = core::hint::black_box(ptr::addr_of!(host_stack_top) as u64);
    let high_guard = core::hint::black_box(ptr::addr_of!(host_stack_high_guard) as u64);
    let high_end = core::hint::black_box(ptr::addr_of!(host_stack_high_guard_end) as u64);
    assert_eq!(guard & (PAGE - 1), 0);
    assert_eq!(guard_end, guard + PAGE);
    assert_eq!(stack, guard_end);
    assert_eq!(stack_top, stack + 65536);
    assert_eq!(high_guard, stack_top);
    assert_eq!(high_end, high_guard + PAGE);
    assert!(guard >= bss && high_end <= bss_limit);
    let df_guard = core::hint::black_box(ptr::addr_of!(host_df_guard) as u64);
    let df_stack = core::hint::black_box(ptr::addr_of!(host_double_fault_stack) as u64);
    let df_high_guard = core::hint::black_box(ptr::addr_of!(host_df_high_guard) as u64);
    assert_eq!(df_guard & (PAGE - 1), 0);
    assert_eq!(df_stack, df_guard.checked_add(PAGE).unwrap());
    assert_eq!(df_high_guard, df_stack.checked_add(16384).unwrap());
    let df_high_end = df_high_guard.checked_add(PAGE).unwrap();
    assert!(df_guard >= bss && df_high_end <= bss_limit);
    assert!(df_high_end <= guard || df_guard >= high_end);
    #[cfg(not(feature = "concurrent-smp"))]
    let guards = [guard, high_guard, df_guard, df_high_guard];
    #[cfg(feature = "concurrent-smp")]
    let guards = {
        let ap_guard = ptr::addr_of!(host_ap_stack_guard) as u64;
        let ap_high = ptr::addr_of!(host_ap_stack_high_guard) as u64;
        let ap_df = ptr::addr_of!(host_ap_df_guard) as u64;
        let ap_df_high = ptr::addr_of!(host_ap_df_high_guard) as u64;
        assert_eq!(ap_high, ap_guard + PAGE + 65536);
        assert_eq!(ap_df_high, ap_df + PAGE + 16384);
        for address in [ap_guard, ap_high, ap_df, ap_df_high] {
            assert_eq!(address & (PAGE - 1), 0);
            assert!(address >= bss && address + PAGE <= bss_limit);
        }
        [
            guard,
            high_guard,
            df_guard,
            df_high_guard,
            ap_guard,
            ap_high,
            ap_df,
            ap_df_high,
        ]
    };

    let cr0: u64;
    let cr4: u64;
    let efer_low: u32;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") efer_low, out("edx") _, options(nomem, nostack));
    }
    assert_eq!(cr0 & 0x80010001, 0x80010001); // PG, WP, PE.
    assert_eq!(cr4 & ((1 << 12) | (1 << 5)), 1 << 5); // !LA57, PAE.
    assert_eq!(efer_low & 0xc00, 0xc00); // NXE, LMA.

    let storage = ptr::addr_of_mut!(HOST_TABLES);
    let root = storage as u64;
    assert_eq!(root & (PAGE - 1), 0);
    assert!(root >= bss && root.checked_add(4 * PAGE).unwrap() <= bss_limit);
    let root_end = root + 4 * PAGE;
    for absent in guards {
        assert!(root_end <= absent || root >= absent + PAGE);
    }
    {
        let tables = unsafe { &mut (*storage).0 };
        for table in tables.iter_mut() {
            table.fill(0);
        }
        for parent in 0..2 {
            tables[parent][0] = root + (parent as u64 + 1) * PAGE | PRESENT | WRITE;
        }
        tables[2][directory_index] = root + 3 * PAGE | PRESENT | WRITE;
        for page in (start / PAGE)..(limit / PAGE) {
            let address = page * PAGE;
            if guards.contains(&address) {
                continue;
            }
            let flags = if address >= text && address < text_limit {
                PRESENT
            } else if address < data {
                PRESENT | NX
            } else {
                PRESENT | WRITE | NX
            };
            tables[3][(page & 511) as usize] = address | flags;
        }
        // Walk the sole present branch and verify every leaf independently:
        // exact identity addresses exclude aliases; all other leaves are zero.
        for parent in 0..3 {
            let branch = if parent == 2 { directory_index } else { 0 };
            for (index, entry) in tables[parent].iter().enumerate() {
                let expected = if index == branch {
                    root + (parent as u64 + 1) * PAGE | PRESENT | WRITE
                } else {
                    0
                };
                assert_eq!(*entry, expected);
            }
        }
        for (index, entry) in tables[3].iter().enumerate() {
            let address = window + index as u64 * PAGE;
            if address < start || address >= limit || guards.contains(&address) {
                assert_eq!(*entry, 0);
                continue;
            }
            assert_eq!(*entry & 0x000f_ffff_ffff_f000, address);
            assert_eq!(*entry & (PRESENT | 4), PRESENT); // Present supervisor.
            let writable = *entry & WRITE != 0;
            let executable = *entry & NX == 0;
            assert_eq!(writable, address >= data);
            assert_eq!(executable, address >= text && address < text_limit);
            assert!(!(writable && executable));
        }
    }
    // All temporary references end before hardware starts updating A/D bits.
    unsafe {
        asm!("mov cr3, {}", in(reg) root, options(nostack));
    }
    let installed: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) installed, options(nomem, nostack));
    }
    assert_eq!(installed, root);
    crate::print("host-memory-rx-ro-rw\n");
}

/// Map the admitted low SIPI page during the bounded emulator startup interval.
/// # Safety
/// Exclusive BSP, AP on the admitted disjoint firmware reserved map, held in
/// INIT, or committed to terminal park, IF=0; image
/// and no AP access to this low page (including cached walks). Caller proves
/// allocation via the retained EBS map or flat fixture ownership. None removes it, Some(true) is RW/NX for copy,
/// Some(false) is RX for startup. Never writable and executable (APM2 5.4/5.5).
#[cfg(feature = "concurrent-smp")]
pub unsafe fn map_startup(page: svmvisor_hypervisor::memory::address::PhysicalRange, writable: Option<bool>) -> u64 {
    assert_eq!(page.len(),PAGE);
    assert!(page.base()>=PAGE && page.last_byte()<0x100000 && page.base() & (PAGE-1)==0);
    let root = ptr::addr_of_mut!(HOST_TABLES).cast::<u64>();
    let image_window = (core::hint::black_box(ptr::addr_of!(image_start)) as u64) >> 21;
    let table = if image_window == 0 { unsafe {root.add(3*512)} }
        else {ptr::addr_of_mut!(STARTUP_TABLE).cast::<u64>()};
    let leaf = unsafe { table.add((page.base()/PAGE) as usize) };
    let value = writable.map_or(0, |write| {
        page.base() | PRESENT | if write { WRITE | NX } else { 0 }
    });
    unsafe {
        if image_window != 0 {
            let branch=root.add(2*512);
            let expected=table as u64 | PRESENT | WRITE;
            let prior=ptr::read_volatile(branch) & !(0x20|0x40);
            assert!(prior==0 || prior==expected);
            ptr::write_volatile(branch, if writable.is_some(){expected}else{0});
        }
        ptr::write_volatile(leaf, value);
        asm!("mov cr3, {}", in(reg) root as u64, options(nostack));
    }
    assert_eq!(unsafe { ptr::read_volatile(leaf) } & !(0x20 | 0x40), value);
    root as u64
}
