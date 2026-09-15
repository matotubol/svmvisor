//! Disposable consumer of the driver's explicit post-successful-EBS seam.
//! This establishes actual per-CPU resident entry, not Windows loader support.
use super::*;
use core::sync::atomic::Ordering;
use svmvisor_dxe::native::resident::physical::{ACTIVATION_GUID, ActivationInterface};

unsafe fn read_msr(index: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") index, out("eax") low, out("edx") high,
            options(nomem, nostack, preserves_flags));
    }
    u64::from(low) | (u64::from(high) << 32)
}

unsafe fn write_msr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32,
            in("edx") (value >> 32) as u32, options(nostack, preserves_flags));
    }
}

pub(super) fn activate(before_start: [u64; 10]) -> ! {
    require(
        observe() == before_start,
        "smp-install-changed-native-state",
    );
    require(
        __cpuid(TEST_LEAF).eax != TEST_MAGIC,
        "smp-install-activated-early",
    );
    let interface = uefi::system::with_config_table(|tables| {
        tables
            .iter()
            .find(|entry| entry.guid == ACTIVATION_GUID)
            .map(|entry| entry.address.cast::<ActivationInterface>())
    })
    .expect("post-EBS activation interface");
    require(
        !interface.is_null() && interface as usize % 8 == 0,
        "activation-interface-address",
    );
    // Driver image is retained RuntimeServicesCode/Data. The shared ABI is
    // immutable except release/acquire completion/failure publications.
    let interface = unsafe { &*interface };
    require(
        interface.version == 1 && (2..=32).contains(&interface.count),
        "activation-interface-shape",
    );
    require(
        interface.completed.load(Ordering::Acquire) == 0,
        "activation-published-early",
    );
    marker("native-smp-activation-installed count=");
    hex(interface.count);
    marker(" pool=");
    hex(interface.pool_base);
    marker(" bytes=");
    hex(interface.pool_bytes);
    marker("\n");
    allocate_after_ready();
    #[cfg(feature = "guest-startup")]
    let startup = super::startup::Startup::prepare_all(interface.count as usize);
    #[cfg(feature = "loader-boot")]
    {
        super::startup::quiesce_bsp();
        super::loader::failed_exit(interface);
    }
    #[cfg(not(feature = "loader-boot"))]
    let final_map = unsafe { boot::exit_boot_services(None) };
    #[cfg(feature = "loader-boot")]
    let final_map = super::loader::exit();
    marker("native-smp-activation-after-ebs\n");
    let mut cursor = interface.pool_base;
    let limit = cursor
        .checked_add(interface.pool_bytes)
        .expect("pool limit");
    // Firmware descriptor order is not assumed. Bound each progress pass by
    // the finite map and require complete RuntimeServicesCode coverage.
    for _ in 0..final_map.len() {
        let old = cursor;
        for entry in final_map.entries() {
            if entry.ty == MemoryType::RUNTIME_SERVICES_CODE
                && entry.att.contains(MemoryAttribute::RUNTIME)
            {
                let end = entry
                    .phys_start
                    .checked_add(entry.page_count.checked_mul(4096).expect("map size"))
                    .expect("map end");
                if entry.phys_start <= cursor && cursor < end {
                    cursor = end.min(limit);
                }
            }
        }
        if cursor == limit || cursor == old {
            break;
        }
    }
    require(cursor == limit, "smp-runtime-pool-not-retained");
    marker("native-smp-activation-pool-retained\n");
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
    #[cfg(feature = "guest-startup")]
    super::startup::quiesce_bsp();
    let before = observe();
    #[cfg(not(feature = "loader-boot"))]
    let result = unsafe { (interface.start)() };
    #[cfg(feature = "loader-boot")]
    let result = {
        require(
            __cpuid(TEST_LEAF).eax == TEST_MAGIC,
            "loader-return-not-virtualized",
        );
        0u64
    };
    marker("native-smp-activation-return status=");
    hex(result);
    marker("\n");
    require(result == 0, "post-ebs-activation-failed");
    require(observe() == before, "smp-bsp-continuation-state-changed");
    let expected = u32::MAX >> (32 - interface.count as u32);
    require(
        interface.completed.load(Ordering::Acquire) == expected,
        "missing-cpu-guest-continuation",
    );
    require(
        interface.failed.load(Ordering::Acquire) == 0,
        "cpu-activation-failure",
    );
    marker("native-smp-activation-all-guests mask=");
    hex(u64::from(expected));
    marker("\n");
    require(
        unsafe { (interface.start)() } == 32,
        "repeated-start-not-refused",
    );
    require(
        interface.completed.load(Ordering::Acquire) == expected
            && interface.failed.load(Ordering::Acquire) == 0,
        "repeated-start-changed-ownership",
    );
    marker("native-smp-repeat-refused\n");
    let cpu_count = interface.count as usize;
    #[cfg(feature = "loader-boot")]
    super::loader::reclaim();
    let counts = resident_witness();
    uefi::runtime::get_time().expect("GetTime after actual AP activation");
    marker("native-smp-activation-runtime-time\n");
    // Exception delivery still needs the guest GDT. The virtual-map witness
    // below deliberately removes firmware runtime aliases, including that GDT.
    #[cfg(feature = "guest-vmcr")]
    super::vmcr::run();
    #[cfg(feature = "guest-apic-contract")]
    super::apic_contract::run();
    #[cfg(all(feature = "virtual-map", not(feature = "loader-boot")))]
    install_identity_map(final_map, counts);
    #[cfg(feature = "loader-boot")]
    super::loader::virtual_map(final_map, counts);
    #[cfg(not(feature = "virtual-map"))]
    {
        let _ = counts;
        core::mem::forget(final_map);
    }

    #[cfg(feature = "guest-startup")]
    super::startup::Startup::run_all(&startup, cpu_count);

    // Positive physical x2APIC ICR forwarding. Keep IF clear through debug-exit:
    // the self-directed fixed vector is physically pending, with no guest ISR.
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
    let apic_base = unsafe { read_msr(0x1b) };
    require(apic_base & 0xc00 == 0xc00, "bsp-x2apic-not-enabled");
    let previous_icr = unsafe { read_msr(0x830) };
    let vector = if previous_icr & 0xff == 0x40 {
        0x41
    } else {
        0x40
    };
    let command = 0x40000 | vector;
    unsafe {
        write_msr(0x1b, apic_base);
        write_msr(0x830, command);
    }
    require(
        unsafe { read_msr(0x830) } == command,
        "native-icr-forward-readback",
    );
    marker("native-smp-native-icr-forward-readback\n");
    #[cfg(any(
        feature = "smp-reject-init",
        feature = "smp-reject-sipi",
        feature = "smp-reject-remote-init",
        feature = "smp-reject-apic-mode"
    ))]
    {
        #[cfg(feature = "smp-reject-init")]
        {
            marker("native-smp-before-rejected-init\n");
            unsafe {
                write_msr(0x830, 0x44500);
            }
        }
        #[cfg(feature = "smp-reject-sipi")]
        {
            marker("native-smp-before-rejected-sipi\n");
            unsafe {
                write_msr(0x830, 0x40608);
            }
        }
        #[cfg(feature = "smp-reject-remote-init")]
        {
            // This one test restricts the pinned q35 fixture to BSP0/AP1.
            // Product topology admission continues to use discovered IDs.
            require(__cpuid(1).ebx >> 24 == 0, "remote-init-fixture-bsp-id");
            marker("native-smp-before-rejected-remote-init\n");
            unsafe {
                write_msr(0x830, (1u64 << 32) | 0x4500);
            }
        }
        #[cfg(feature = "smp-reject-apic-mode")]
        {
            marker("native-smp-before-rejected-apic-mode\n");
            unsafe {
                write_msr(0x1b, apic_base & !0xc00);
            }
        }
        marker("FAIL native-smp-unowned-write-returned\n");
        finish(false);
    }
    marker("PASS native-smp-activation-ebs\n");
    finish(true)
}
