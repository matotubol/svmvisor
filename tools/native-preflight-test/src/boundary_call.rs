//! Explicit emulator test of the real loaded driver's raw PE entry.
//! StartImage tests remain separate; this invocation lets the fixture compare
//! registers at the exact boundary without firmware's intervening ABI clobbers.
use core::{arch::asm, mem::offset_of, ptr};
use uefi::{
    Handle,
    boot::{self, AllocateType, MemoryType},
    proto::loaded_image::LoadedImage,
};

#[repr(C)]
#[derive(Default)]
struct Call {
    entry: u64,
    image: u64,
    table: u64,
    original: u64,
    expected: u64,
    observed: u64,
    status: u64,
    gpr_failures: u64,
    flags_before: u64,
    flags_after: u64,
    profile: u64,
    original_cr4: u64,
    original_xcr0: u64,
    original_xsave: u64,
}
const _: () = {
    assert!(size_of::<Call>() == 112);
    assert!(offset_of!(Call, original) == 24);
    assert!(offset_of!(Call, status) == 48);
    assert!(offset_of!(Call, flags_after) == 72);
    assert!(offset_of!(Call, profile) == 80);
    assert!(offset_of!(Call, original_xsave) == 104);
};
unsafe extern "efiapi" {
    fn boundary_call(call: *mut Call);
}

pub fn run(handle: Handle, source: &[u8]) {
    let cr4: u64;
    unsafe {
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    assert_eq!(cr4 & (1 << 18), 0, "FX fixture requires OSXSAVE off");
    let profile = if cfg!(feature = "boundary-xcr0-refused") {
        1
    } else if cfg!(feature = "boundary-avx") {
        7
    } else if cfg!(feature = "boundary-sse") {
        3
    } else {
        0
    };
    if profile != 0 {
        use core::arch::x86_64::__cpuid_count;
        let one = __cpuid_count(1, 0);
        let d0 = __cpuid_count(0xd, 0);
        let d1 = __cpuid_count(0xd, 1);
        assert_ne!(one.ecx & (1 << 26), 0);
        assert_eq!(u64::from(d0.eax) & profile, profile);
        assert_eq!(d1.eax & !7, 0, "fixture requires no supervisor/XFD");
        assert_eq!(d1.ecx | d1.edx, 0);
        if profile == 7 {
            let avx = __cpuid_count(0xd, 2);
            assert_ne!(one.ecx & (1 << 28), 0);
            assert_eq!((avx.eax, avx.ebx), (256, 576));
        }
    }
    let loaded =
        boot::open_protocol_exclusive::<LoadedImage>(handle).expect("loaded image protocol");
    let (base, size) = loaded.info();
    let pe = u32::from_le_bytes(source[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&source[pe..pe + 4], b"PE\0\0");
    assert_eq!(
        u16::from_le_bytes(source[pe + 24..pe + 26].try_into().unwrap()),
        0x20b
    );
    assert_eq!(
        u16::from_le_bytes(source[pe + 92..pe + 94].try_into().unwrap()),
        11
    );
    let rva = u32::from_le_bytes(source[pe + 40..pe + 44].try_into().unwrap()) as u64;
    assert!(rva > 0 && rva < size);
    let entry = (base as u64).checked_add(rva).unwrap();
    drop(loaded);
    let areas = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 4).unwrap();
    unsafe {
        ptr::write_bytes(areas.as_ptr(), 0, 4 * 4096);
    }
    let expected = unsafe { core::slice::from_raw_parts_mut(areas.as_ptr().add(4096), 1024) };
    expected[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
    // Valid finite x87 data in all eight slots, standard masked MXCSR,
    // and unique values in every XMM (and YMM upper half when enabled).
    // x87 conditional FIP/FDP semantics are outside this canary comparison.
    expected[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
    expected[4] = 0xff;
    for register in 0..8 {
        let start = 32 + register * 16;
        expected[start..start + 8]
            .copy_from_slice(&(0x8000_0000_0000_0000u64 + register as u64).to_le_bytes());
        expected[start + 8..start + 10].copy_from_slice(&0x3fffu16.to_le_bytes());
    }
    for register in 0..16 {
        for byte in 0..16 {
            expected[160 + register * 16 + byte] = (register * 16 + byte) as u8;
        }
    }
    if profile != 0 {
        expected[512..520].copy_from_slice(&profile.to_le_bytes());
    }
    if profile == 7 {
        for byte in 0..256 {
            expected[576 + byte] = (byte as u8) ^ 0xa5;
        }
    }
    let mut call = Call {
        entry,
        image: handle.as_ptr() as u64,
        table: uefi::table::system_table_raw().unwrap().as_ptr() as u64,
        original: areas.as_ptr() as u64,
        expected: unsafe { areas.as_ptr().add(4096) } as u64,
        observed: unsafe { areas.as_ptr().add(8192) } as u64,
        profile,
        original_xsave: unsafe { areas.as_ptr().add(12288) } as u64,
        ..Call::default()
    };
    unsafe {
        boundary_call(&mut call);
    }
    assert_eq!(call.status, 0x8000_0000_0000_0003);
    if call.gpr_failures != 0 {
        super::marker("FAIL exact-driver-entry-gpr-canaries\n");
    }
    assert_eq!(call.gpr_failures, 0, "entry GPR canaries");
    assert_eq!(call.flags_before, call.flags_after, "entry RFLAGS canary");
    let observed = unsafe { core::slice::from_raw_parts(call.observed as *const u8, 1024) };
    assert_eq!(&expected[0..5], &observed[0..5], "x87 controls/status/tag");
    assert_eq!(&expected[24..28], &observed[24..28], "MXCSR");
    assert_eq!(
        &expected[160..416],
        &observed[160..416],
        "all sixteen XMM registers"
    );
    for register in 0..8 {
        let start = 32 + register * 16;
        assert_eq!(
            &expected[start..start + 10],
            &observed[start..start + 10],
            "x87 register payload"
        );
    }
    if profile == 7 {
        assert_eq!(
            &expected[576..832],
            &observed[576..832],
            "all sixteen upper YMM halves"
        );
    }
    let restored_cr4: u64;
    unsafe {
        asm!("mov {}, cr4", out(reg) restored_cr4, options(nostack, preserves_flags));
    }
    assert_eq!(restored_cr4, cr4, "fixture restores CR4");
    unsafe {
        boot::free_pages(areas, 4).unwrap();
    }
    super::marker("PASS exact-driver-entry-gpr-flags-xmm\n");
    super::marker("PASS exact-driver-entry-x87-payload\n");
    if profile == 7 {
        super::marker("PASS exact-driver-entry-upper-ymm\n");
    }
    if profile == 1 {
        super::marker("PASS exact-driver-entry-xcr0-refusal\n");
    }
}
