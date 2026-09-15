//! Disposable loader-owned CR3 and nonidentity runtime witnesses, below4GiB.
//! No firmware-owned paging page is ever overwritten or reclaimed.
use super::*;
use core::sync::atomic::Ordering;
use svmvisor_dxe::native::resident::physical::ActivationInterface;
use uefi::mem::memory_map::MemoryMapMut;
const ALIAS: u64 = 1 << 39;
const PAGES: usize = 128;
static mut ROOT: u64 = 0;
static mut OBSOLETE: u64 = 0;
static mut NEXT: usize = 11;

#[cfg(feature = "loader-fsgsbase")]
unsafe extern "efiapi" {
    fn svmvisor_fixture_fsgsbase_witness(fs: u64, gs: u64) -> u64;
}

#[cfg(feature = "loader-fsgsbase")]
fn enable_loader_controls() {
    #[cfg(feature = "loader-controls")]
    require(
        __cpuid(1).ecx & (1 << 17) != 0,
        "loader-pcid-not-advertised",
    );
    require(
        __cpuid(0).eax >= 7 && core::arch::x86_64::__cpuid_count(7, 0).ebx & 1 != 0,
        "loader-fsgsbase-not-advertised",
    );
    unsafe {
        let cr3: u64;
        let cr4: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
        require(
            cr3 == ROOT && cr4 & (1 << 17) == 0,
            "loader-controls-initial-root",
        );
        // APM2 3.1.3/5.5.1: set PCIDE only in long mode with zero CR3 low bits.
        // This disposable fixture owns the selected root; no host setting changes.
        let added = (1 << 16)
            | if cfg!(feature = "loader-controls") {
                1 << 17
            } else {
                0
            };
        asm!("mov cr4, {}", in(reg) cr4 | added, options(nostack, preserves_flags));
        #[cfg(feature = "loader-controls")]
        asm!("mov cr3, {}", in(reg) ROOT | 0x18, options(nostack, preserves_flags));
    }
}

pub(super) fn root() -> u64 {
    unsafe { ROOT }
}
pub(super) fn prepare() {
    #[cfg(feature = "cache-survey")]
    {
        require(survey_read(0x20f) & 0x800 == 0, "survey-variable7-must-be-disabled");
        require(survey_read(0x20e) != 0x1234_5006, "survey-sentinel-already-present");
        marker("native-cache-fixture-before-driver var7=");
        hex(survey_read(0x20e));
        marker("\n");
    }
    let obsolete = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .expect("owned obsolete PML4")
        .as_ptr() as u64;
    let tables = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, PAGES)
        .expect("owned loader tables")
        .as_ptr() as u64;
    require(
        tables + (PAGES * 4096) as u64 <= 1 << 32 && obsolete < 1 << 32,
        "loader-table-aperture",
    );
    unsafe {
        let current: u64;
        asm!("mov {}, cr3", out(reg) current, options(nostack, preserves_flags));
        require(current & 4095 == 0, "loader-no-pcid");
        core::ptr::copy_nonoverlapping(current as *const u8, obsolete as *mut u8, 4096);
        core::ptr::write_bytes(tables as *mut u8, 0, PAGES * 4096);
        // Independent identity and high-alias directories, so removing an old
        // runtime identity leaf cannot silently remove its replacement alias.
        for (pml4_slot, pdpt_page, pd_page) in [(0, 1, 2), (1, 6, 7)] {
            ((tables + pml4_slot * 8) as *mut u64).write(tables + pdpt_page * 4096 | 3);
            for gig in 0..4 {
                ((tables + pdpt_page * 4096 + gig * 8) as *mut u64)
                    .write(tables + (pd_page + gig) * 4096 | 3);
                for leaf in 0..512 {
                    ((tables + (pd_page + gig) * 4096 + leaf * 8) as *mut u64)
                        .write((gig << 30) + (leaf << 21) | 0x83);
                }
            }
        }
        ROOT = tables;
        OBSOLETE = obsolete;
        marker("native-loader-roots obsolete=");
        hex(obsolete);
        marker(" new=");
        hex(tables);
        marker("\n");
        asm!("mov cr3, {}", in(reg) obsolete, options(nostack, preserves_flags));
    }
}

static CALLBACKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "cache-survey")]
fn survey_read(index: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") index, out("eax") lo, out("edx") hi,
        options(nostack, nomem, preserves_flags)); }
    (hi as u64) << 32 | lo as u64
}
unsafe extern "efiapi" fn ebs_notify(_: Event, _: Option<NonNull<c_void>>) {
    require(
        __cpuid(TEST_LEAF).eax != TEST_MAGIC,
        "activated-before-last-EBS-notification",
    );
    #[cfg(feature = "cache-survey")]
    {
        // Disposable QEMU only: change the raw base of a disabled variable
        // range, leaving effective memory types unchanged. The later same-CPU
        // survey must capture this actual late-EBS MSR value, not installation.
        require(survey_read(0x20f) & 0x800 == 0, "survey-late-variable7-enabled");
        unsafe { asm!("wrmsr", in("ecx") 0x20eu32, in("eax") 0x1234_5006u32,
            in("edx") 0u32, options(nostack, nomem, preserves_flags)); }
        require(survey_read(0x20e) == 0x1234_5006, "survey-late-write-readback");
        marker("native-cache-fixture-late-ebs var7=0000000012345006\n");
    }
    CALLBACKS.fetch_add(1, Ordering::Release);
}
pub(super) fn failed_exit(interface: &ActivationInterface) {
    for _ in 0..2 {
        let event = unsafe {
            boot::create_event(
                EventType::SIGNAL_EXIT_BOOT_SERVICES,
                Tpl::NOTIFY,
                Some(ebs_notify),
                None,
            )
        }
        .expect("EBS order witness");
        core::mem::forget(event);
    }
    #[cfg(feature = "loader-new-root")]
    unsafe {
        asm!("mov cr3, {}", in(reg) ROOT, options(nostack, preserves_flags));
    }

    let services = unsafe {
        &*uefi::table::system_table_raw()
            .unwrap()
            .as_ref()
            .boot_services
    };
    // Independently check the live table CRC using firmware's own calculator.
    let mut copied = unsafe { core::ptr::read(services) };
    let expected = copied.header.crc;
    copied.header.crc = 0;
    let mut crc = 0;
    let status = unsafe {
        (services.calculate_crc32)(
            core::ptr::addr_of!(copied).cast(),
            core::mem::size_of_val(&copied),
            &mut crc,
        )
    };
    require(
        status == Status::SUCCESS && crc == expected,
        "loader-table-crc",
    );
    marker("native-loader-table-crc-pass\n");
    let buffer = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 16)
        .expect("bad-key map storage");
    let mut size = 16 * 4096;
    let mut key = 0;
    let mut stride = 0;
    let mut version = 0;
    let status = unsafe {
        (services.get_memory_map)(
            &mut size,
            buffer.as_ptr().cast(),
            &mut key,
            &mut stride,
            &mut version,
        )
    };
    require(status == Status::SUCCESS, "raw-map-key");
    #[cfg(feature = "loader-fsgsbase")]
    enable_loader_controls();
    let before = observe();
    let status = unsafe { (services.exit_boot_services)(boot::image_handle().as_ptr(), key ^ 1) };
    require(status == Status::INVALID_PARAMETER, "loader-bad-key-status");
    require(
        observe() == before
            && interface.completed.load(Ordering::Acquire) == 0
            && interface.failed.load(Ordering::Acquire) == 0
            && __cpuid(TEST_LEAF).eax != TEST_MAGIC,
        "loader-bad-key-mutated-state",
    );
    marker("native-loader-failed-ebs-inactive\n");
    // Memory services alone are legal after an unsuccessful first EBS attempt.
    unsafe {
        boot::free_pages(buffer, 16).expect("bad-key storage release");
    }
}

pub(super) fn reclaim() {
    unsafe {
        asm!("mov cr3, {}", in(reg) ROOT, options(nostack, preserves_flags));
        core::ptr::write_bytes(OBSOLETE as *mut u8, 0xa5, 4096);
        for index in 0..512 {
            require(
                ((OBSOLETE + index * 8) as *const u64).read_volatile() == 0xa5a5_a5a5_a5a5_a5a5,
                "obsolete-root-overwrite",
            );
        }
    }
    resident_witness();
    marker("native-loader-obsolete-root-reclaimed\n");
}

pub(super) fn virtual_map(mut map: uefi::mem::memory_map::MemoryMapOwned, previous: [u32; 3]) {
    let rt = unsafe {
        uefi::table::system_table_raw()
            .unwrap()
            .as_ref()
            .runtime_services
    };
    for index in 0..map.len() {
        let entry = map.get_mut(index).unwrap();
        if entry.att.contains(MemoryAttribute::RUNTIME) {
            require(
                entry.phys_start + entry.page_count * 4096 <= 1 << 32,
                "runtime-aperture",
            );
            entry.virt_start = entry.phys_start + ALIAS;
        }
    }
    let meta = map.meta();
    let status = unsafe {
        ((*rt).set_virtual_address_map)(
            meta.map_size,
            meta.desc_size,
            meta.desc_version,
            map.buffer_mut().as_mut_ptr().cast(),
        )
    };
    require(status == Status::SUCCESS, "nonidentity-set-virtual-map");
    // Remove every runtime identity mapping from the BSP's owned root. APs
    // keep their independent retained identity root until guest INIT/SIPI.
    for entry in map
        .entries()
        .filter(|entry| entry.att.contains(MemoryAttribute::RUNTIME))
    {
        for offset in 0..entry.page_count {
            unsafe {
                remove_identity(entry.phys_start + offset * 4096);
            }
        }
    }
    unsafe {
        asm!("mov cr3, {}", in(reg) ROOT, options(nostack, preserves_flags));
    }
    let virtual_rt = (rt as u64 + ALIAS) as *const uefi_raw::table::runtime::RuntimeServices;
    let mut time = core::mem::MaybeUninit::<uefi_raw::time::Time>::uninit();
    let get_time = unsafe { (*virtual_rt).get_time };
    require(
        get_time as usize as u64 >= ALIAS,
        "GetTime-pointer-not-converted",
    );
    let status = unsafe { get_time(time.as_mut_ptr(), core::ptr::null_mut()) };
    require(
        status == Status::SUCCESS,
        "virtual-GetTime-without-identity-alias",
    );
    require(
        resident_witness()[0] > previous[0],
        "nonidentity-resident-progress",
    );
    core::mem::forget(map);
    marker("native-fixture-after-virtual-map\nnative-loader-nonidentity-runtime-no-alias\n");
    #[cfg(feature = "guest-paging-avl")]
    software_paging_witness();
    #[cfg(feature = "guest-apic-pke")]
    supervisor_apic_keys();
}

#[cfg(feature = "guest-apic-pke")]
fn supervisor_apic_keys() {
    // Disposable guest-owned mapping only. APM2 5.6.7: MPK is ignored for
    // effective supervisor pages, even when PKRU denies every key.
    require(core::arch::x86_64::__cpuid_count(7, 0).ecx & (1 << 3) != 0, "fixture-pku-capability");
    marker("native-apic-pke-before\n");
    unsafe {
        let cr3: u64;
        let cr4: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
        require(cr3 == ROOT && cr4 & (1 << 22) == 0, "fixture-pke-initial-controls");
        let pde = (ROOT + (2 + (0xfee00000u64 >> 30)) * 4096 + ((0xfee00000u64 >> 21) & 511) * 8) as *mut u64;
        let original = pde.read_volatile();
        require(original & 0x85 == 0x81, "fixture-apic-supervisor-large-page");
        let version = core::ptr::read_volatile(0xfee00030usize as *const u32);
        asm!("mov cr4, {}", in(reg) cr4 | (1 << 22), options(nostack, preserves_flags));
        let pkru: u32;
        asm!("rdpkru", in("ecx") 0u32, out("eax") pkru, out("edx") _, options(nostack, preserves_flags));
        asm!("wrpkru", in("ecx") 0u32, in("eax") u32::MAX, in("edx") 0u32, options(nostack, preserves_flags));
        for key in 0..16u64 {
            pde.write_volatile((original & !(15u64 << 59)) | (key << 59));
            asm!("mov cr3, {}", in(reg) ROOT, options(nostack, preserves_flags));
            require(core::ptr::read_volatile(0xfee00030usize as *const u32) == version, "fixture-pke-version-readback");
        }
        pde.write_volatile(original);
        asm!("mov cr3, {}", in(reg) ROOT, options(nostack, preserves_flags));
        asm!("wrpkru", in("ecx") 0u32, in("eax") pkru, in("edx") 0u32, options(nostack, preserves_flags));
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
    marker("native-apic-pke-all-keys-pass\n");
}

#[cfg(feature = "guest-paging-avl")]
fn software_paging_witness() {
    // Only after all EBS/host admission and virtual-map checks: mutate owned
    // guest tables, never firmware/monitor tables. APM2 rev3.44 5.3/5.4 permits
    // these upper AVL bits at every level with PKE=0. NEXT bounds the allocated
    // table pages, including 4KiB leaves created by remove_identity above.
    marker("native-guest-paging-avl-before\n");
    unsafe {
        let cr3: u64;
        let cr4: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
        require(
            cr3 == ROOT && cr4 & (1 << 22) == 0 && NEXT <= PAGES,
            "guest-paging-avl-state",
        );
        for index in 0..NEXT * 512 {
            let entry = (ROOT as *mut u64).add(index);
            let original = entry.read_volatile();
            if original & 1 != 0 {
                entry.write_volatile(original | 0x7ff0_0000_0000_0000);
                require(
                    entry.read_volatile() & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000,
                    "guest-paging-avl-readback",
                );
            }
        }
        asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
    }
    // Actual intercepted instructions now use the marked hardware mappings.
    // Later guest xAPIC startup also consumes these marked operand mappings.
    resident_witness();
    marker("native-guest-paging-avl-pass\n");
}

unsafe fn remove_identity(page: u64) {
    unsafe {
        let pde = (ROOT + (2 + (page >> 30)) * 4096 + ((page >> 21) & 511) * 8) as *mut u64;
        let mut entry = pde.read();
        if entry & 0x80 != 0 {
            require(NEXT < PAGES, "runtime-split-budget");
            let table = ROOT + NEXT as u64 * 4096;
            NEXT += 1;
            for index in 0..512 {
                ((table + index * 8) as *mut u64).write((page & !0x1f_ffff) + index * 4096 | 3);
            }
            entry = table | 3;
            pde.write(entry);
        }
        let pte = ((entry & !4095) + ((page >> 12) & 511) * 8) as *mut u64;
        pte.write(0);
        require(pte.read_volatile() == 0, "runtime-old-alias-present");
    }
}

static mut FINAL_MAP: Option<uefi::mem::memory_map::MemoryMapOwned> = None;
unsafe extern "efiapi" fn exit_and_store(_: *mut c_void) -> u64 {
    unsafe {
        FINAL_MAP = Some(boot::exit_boot_services(None));
    }
    0
}
pub(super) fn exit() -> uefi::mem::memory_map::MemoryMapOwned {
    let initial_icr = canonical_icr();
    let mut before = observe();
    #[cfg(feature = "loader-fsgsbase")]
    {
        let tag = if cfg!(feature = "loader-controls") {
            0x18
        } else {
            0
        };
        let controls = (1 << 16)
            | if cfg!(feature = "loader-controls") {
                1 << 17
            } else {
                0
            };
        require(
            before[1] == unsafe { ROOT } | tag && before[2] & 0x30000 == controls,
            "loader-controls-before-capture",
        );
    }
    let result = unsafe {
        svmvisor_fixture_signal_witness(exit_and_store as *const () as usize, core::ptr::null_mut())
    };
    let mut after = observe();
    let resumed_icr = canonical_icr();
    marker("native-loader-bsp-icr before=");
    hex(initial_icr);
    marker(" after=");
    hex(resumed_icr);
    marker("\n");
    // Firmware may issue its own IPIs inside EBS. The runner compares readback
    // to DXE's post-firmware, pre-bootstrap snapshot, not this entry value.
    // EBS itself may clear IF; flags are not nonvolatile x64 ABI state.
    // The interposer preserves the original service RETURN flags in assembly.
    before[3] &= !0x200;
    after[3] &= !0x200;
    marker("loader-abi result=");
    hex(result);
    for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        if a != b {
            marker(" mismatch=");
            hex(i as u64);
            hex(*a);
            hex(*b);
        }
    }
    marker("\n");
    require(
        result == 0 && before == after,
        "loader-success-cpu-abi-state",
    );
    #[cfg(feature = "loader-fsgsbase")]
    {
        // Distinct actual fixture addresses supply canonical base sentinels.
        // The assembly restores both bases before returning to Rust and checks
        // an actual intercepted CPUID while those values are live.
        let status = unsafe {
            svmvisor_fixture_fsgsbase_witness(
                core::ptr::addr_of!(ROOT) as u64,
                core::ptr::addr_of!(OBSOLETE) as u64,
            )
        };
        require(status == 0, "loader-fsgsbase-exit-preservation");
        marker(if cfg!(feature = "loader-controls") {
            "native-loader-controls-pass cr3="
        } else {
            "native-loader-fsgsbase-pass cr3="
        });
        hex(after[1]);
        marker(" cr4=");
        hex(after[2]);
        marker("\nnative-loader-fsgsbase-exit-pass\n");
    }
    require(
        CALLBACKS.load(Ordering::Acquire) == 2,
        "missing-last-EBS-notifications",
    );
    marker("native-loader-success-cpu-abi-pass\nnative-loader-after-all-ebs-events\n");
    unsafe {
        (*core::ptr::addr_of_mut!(FINAL_MAP))
            .take()
            .expect("successful loader final map")
    }
}

fn canonical_icr() -> u64 {
    unsafe {
        let low: u32;
        let high: u32;
        asm!("rdmsr", in("ecx") 0x1bu32, out("eax") low, out("edx") high,
            options(nostack, preserves_flags));
        let value = if low & 0x400 != 0 {
            let lo: u32;
            let hi: u32;
            asm!("rdmsr", in("ecx") 0x830u32, out("eax") lo, out("edx") hi,
                options(nostack, preserves_flags));
            (u64::from(hi) << 32) | u64::from(lo)
        } else {
            let base = ((u64::from(high) << 32) | u64::from(low)) & 0xffff_f000;
            (u64::from(((base + 0x310) as *const u32).read_volatile() >> 24) << 32)
                | u64::from(((base + 0x300) as *const u32).read_volatile())
        };
        value & !(1 << 12)
    }
}
