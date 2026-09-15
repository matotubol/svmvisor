//! Actual OVMF parent -> resident child -> stale-key retry -> loader continuation.
//! This is a disposable two-CPU fixture. No physical/Windows/protected-boot claim.
//! UEFI 2.11 7.4.2, 7.4.6 and 9.1.1 supply image and EBS lifetime semantics.
use super::*;
use uefi::mem::memory_map::MemoryMap;

fn efer() -> u64 {
    let (lo, hi): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") lo, out("edx") hi,
        options(nomem, nostack, preserves_flags)); }
    u64::from(lo) | (u64::from(hi) << 32)
}

fn retained_memory(parent: Handle) {
    let images = boot::locate_handle_buffer(boot::SearchType::ByProtocol(&LoadedImageProtocol::GUID)).unwrap();
    let mut child = None;
    for handle in images.iter() {
        // GET_PROTOCOL does not create an open record. No callback into the child.
        let mut raw = ptr::null_mut();
        check(unsafe { (services().handle_protocol)(handle.as_ptr(), &LoadedImageProtocol::GUID, &mut raw) }, "inspect retained image");
        let loaded = unsafe { &*raw.cast::<LoadedImageProtocol>() };
        if loaded.parent_handle == parent {
            assert!(child.is_none(), "one retained child");
            assert_eq!(loaded.image_code_type, uefi_raw::table::boot::MemoryType::RUNTIME_SERVICES_CODE);
            assert_eq!(loaded.image_data_type, uefi_raw::table::boot::MemoryType::RUNTIME_SERVICES_DATA);
            assert_eq!(loaded.load_options_size, 128);
            assert!(!loaded.load_options.is_null());
            let options = loaded.load_options.cast::<u8>();
            unsafe {
                assert_eq!(core::slice::from_raw_parts(options, 8), b"SVMBOT01");
                assert_eq!(options.add(16).cast::<u64>().read_unaligned(), JOURNAL as u64);
                assert_eq!(options.add(28).cast::<u32>().read_unaligned(), 1);
                assert_eq!(options.add(32).cast::<u32>().read_unaligned(), 1);
                assert_eq!(options.add(40).cast::<u64>().read_unaligned(), 0);
            }
            child = Some([(loaded.image_base as u64, loaded.image_size), (options as u64, 128), (unsafe { JOURNAL } as u64, 4096)]);
        }
    }
    let ranges = child.expect("resident child remains in firmware database");
    let map = boot::memory_map(MemoryType::LOADER_DATA).unwrap();
    for (base, bytes) in ranges {
        let end = base.checked_add(bytes).unwrap();
        let mut cursor = base;
        // Firmware map order is not assumed: extend coverage by matching the cursor.
        for _ in 0..map.len() {
            if cursor == end { break; }
            let entry = map.entries().find(|entry| entry.phys_start <= cursor
                && cursor < entry.phys_start + entry.page_count * 4096).expect("retained range covered");
            assert!(entry.att.contains(boot::MemoryAttribute::RUNTIME));
            assert!(entry.ty == MemoryType::RUNTIME_SERVICES_CODE || entry.ty == MemoryType::RUNTIME_SERVICES_DATA);
            cursor = end.min(entry.phys_start + entry.page_count * 4096);
        }
        assert_eq!(cursor, end);
    }
    marker("PASS resident-image-options-journal-runtime-retained\n");
}

pub(super) fn run(parent: Handle, binding: *const DriverBindingProtocol, controller: Handle) -> ! {
    retained_memory(parent);
    assert_eq!(unsafe { read_j(0x9c) }, 0x00080010);
    assert_eq!(unsafe { read_j(0x90) }, 4 | (1 << 8) | (1 << 9) | (1 << 10));
    assert_eq!(unsafe { read_j(0x94) }, 0);
    assert_eq!(unsafe { read_j(0x98) }, 0);
    marker("PASS exact-parent-result-journal\n");
    assert_eq!(unsafe { ((*binding).stop)(binding, controller, 0, ptr::null()) }, Status::UNSUPPORTED);
    assert_eq!(unsafe { COMMAND }, 2);
    assert_eq!(unsafe { ENABLES }, 1);
    assert_eq!(unsafe { DISABLES }, 0);
    assert_eq!(unsafe { ((*binding).start)(binding, controller, ptr::null()) }, Status::ALREADY_STARTED);
    marker("PASS resident-stop-refused-decode-retained\n");
    // The parent lifecycle observer must not replace the child's last record.
    let seq = unsafe { read_j(0x2c) };
    signal(guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b"));
    signal(guid!("3a2a00ad-98b9-4cdf-a478-702777f1c10b"));
    assert_eq!(unsafe { read_j(0x2c) }, seq);
    marker("PASS resident-parent-lifecycle-journal-exclusion\n");

    let bs = services();
    let image = boot::image_handle().as_ptr();
    let before = observe();
    let before_efer = efer();
    let before_cpuid = __cpuid(0);
    let before_diagnostic_leaf = core::arch::x86_64::__cpuid_count(0x4fff0000, 0);
    assert_ne!(before_diagnostic_leaf.eax, 0x53564d52);
    let mut map = [0u64; 8192];
    let mut key = 0;
    let mut stride = 0;
    let mut version = 0;
    let mut bytes = core::mem::size_of_val(&map);
    check(unsafe { (bs.get_memory_map)(&mut bytes, map.as_mut_ptr().cast(), &mut key, &mut stride, &mut version) }, "initial EBS map");
    // A real allocation invalidates the map key. It is retained until VM reset.
    // After failed EBS, only GetMemoryMap and ExitBootServices are called.
    let _stale_key_allocation = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1).unwrap();
    assert_eq!(unsafe { (bs.exit_boot_services)(image, key) }, Status::INVALID_PARAMETER);
    assert_eq!(observe(), before);
    assert_eq!(efer(), before_efer);
    assert_eq!(unsafe { read_j(0x2c) }, seq);
    marker("PASS resident-genuine-stale-key-ebs-refused\n");
    for attempt in 1..=2 {
        bytes = core::mem::size_of_val(&map);
        check(unsafe { (bs.get_memory_map)(&mut bytes, map.as_mut_ptr().cast(), &mut key, &mut stride, &mut version) }, "retry EBS map");
        assert!(bytes <= core::mem::size_of_val(&map) && stride != 0);
        let status = unsafe { (bs.exit_boot_services)(image, key) };
        if status == Status::SUCCESS {
            // Raw owned-memory and native CPU witnesses only after success.
            field("resident-success-ebs-attempt", attempt);
            assert_eq!(unsafe { read_j(0x9c) }, 0x00080013);
            assert_eq!(unsafe { read_j(0x90) } & 0xffff, 5);
            // This QEMU CPU is deliberately not the exact Ryzen target, so
            // endpoint discovery must remain disabled and report that fact.
            assert_eq!(unsafe { read_j(0x94) }, 0x01000000);
            assert_eq!(unsafe { read_j(0x98) }, 2);
            marker("PASS resident-stage5-two-cpu-ack\n");
            let mut after = observe();
            let mut expected = before;
            for (index, (old, new)) in before.iter().zip(after.iter()).enumerate() {
                if old != new {
                    field("resident-continuation-changed-index", index as u64);
                    field("resident-continuation-before", *old);
                    field("resident-continuation-after", *new);
                }
            }
            // OVMF EBS may clear IF; the DXE boundary preserves the original
            // firmware RETURN flags, not EBS entry flags. The existing native
            // loader fixture uses this same architectural comparison scope.
            expected[3] &= !0x200;
            after[3] &= !0x200;
            assert_eq!(after, expected);
            assert_eq!(efer(), before_efer);
            let after_cpuid = __cpuid(0);
            assert_eq!([after_cpuid.eax, after_cpuid.ebx, after_cpuid.ecx, after_cpuid.edx],
                [before_cpuid.eax, before_cpuid.ebx, before_cpuid.ecx, before_cpuid.edx]);
            marker("PASS resident-loader-cpuid-efer-continuation\n");
            let after_diagnostic_leaf = core::arch::x86_64::__cpuid_count(0x4fff0000, 0);
            assert_eq!([after_diagnostic_leaf.eax, after_diagnostic_leaf.ebx, after_diagnostic_leaf.ecx, after_diagnostic_leaf.edx],
                [before_diagnostic_leaf.eax, before_diagnostic_leaf.ebx, before_diagnostic_leaf.ecx, before_diagnostic_leaf.edx]);
            marker("PASS resident-production-cpuid-remains-native\n");
            finish(0x10);
        }
        if status != Status::INVALID_PARAMETER || attempt == 2 { check(status, "retry ExitBootServices"); }
    }
    finish(0x11)
}
