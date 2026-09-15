//! Exact parent DriverBinding.Start under an explicitly TCG-only xstate profile.
//! Genuine firmware may clobber volatile GPRs/x87/XMM0..5/YMM. Only EFI ABI
//! nonvolatile GPRs, lower XMM6..15, IF/DF, and restored controls are asserted.
use core::{arch::asm, mem::offset_of, ptr};
use uefi::boot::{self, AllocateType, MemoryType};

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
    rsp_before: u64,
    rsp_after: u64,
    observed_cr4: u64,
    observed_xcr0: u64,
}
const _: () = {
    assert!(size_of::<Call>() == 144);
    assert!(offset_of!(Call, status) == 48);
    assert!(offset_of!(Call, profile) == 80);
    assert!(offset_of!(Call, original_xsave) == 104);
    assert!(offset_of!(Call, rsp_before) == 112);
    assert!(offset_of!(Call, observed_xcr0) == 136);
};
unsafe extern "efiapi" {
    fn boundary_call(call: *mut Call);
}

pub fn run(
    binding: *const uefi_raw::protocol::driver::DriverBindingProtocol,
    controller: uefi_raw::Handle,
) -> uefi_raw::Status {
    let cr4: u64;
    unsafe {
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    assert_eq!(cr4 & (1 << 18), 0, "fixture requires initial OSXSAVE off");
    let profile: u64 = if cfg!(feature = "boundary-avx") {
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
        assert_eq!(d1.eax & !7, 0);
        assert_eq!(d1.ecx | d1.edx, 0);
        if profile == 7 {
            let avx = __cpuid_count(0xd, 2);
            assert_ne!(one.ecx & (1 << 28), 0);
            assert_eq!((avx.eax, avx.ebx), (256, 576));
        }
    }
    let areas = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 4).unwrap();
    unsafe {
        ptr::write_bytes(areas.as_ptr(), 0, 4 * 4096);
    }
    let expected = unsafe { core::slice::from_raw_parts_mut(areas.as_ptr().add(4096), 1024) };
    expected[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
    expected[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
    expected[4] = 0xff;
    for r in 0..8 {
        let start = 32 + r * 16;
        expected[start..start + 8]
            .copy_from_slice(&(0x8000_0000_0000_0000u64 + r as u64).to_le_bytes());
        expected[start + 8..start + 10].copy_from_slice(&0x3fffu16.to_le_bytes());
    }
    for r in 0..16 {
        for b in 0..16 {
            expected[160 + r * 16 + b] = (r * 16 + b) as u8;
        }
    }
    if profile != 0 {
        expected[512..520].copy_from_slice(&profile.to_le_bytes());
    }
    if profile == 7 {
        for b in 0..256 {
            expected[576 + b] = (b as u8) ^ 0xa5;
        }
    }
    let mut call = Call {
        entry: unsafe { (*binding).start as usize as u64 },
        image: binding as u64,
        table: controller as u64,
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
    super::field("parent-start-status", call.status);
    super::field("outer-gpr-failures", call.gpr_failures);
    assert_eq!(call.gpr_failures, 0, "ABI nonvolatile GPRs");
    assert_eq!(call.flags_before & 0x600, call.flags_after & 0x600, "IF/DF");
    assert_eq!(call.rsp_before, call.rsp_after, "exact parent return RSP");
    assert_eq!(
        call.observed_cr4,
        cr4 | if profile == 0 { 0 } else { 1 << 18 },
        "CR4 before fixture repair"
    );
    if profile != 0 {
        assert_eq!(call.observed_xcr0, profile, "XCR0 before fixture repair");
    }
    let observed = unsafe { core::slice::from_raw_parts(call.observed as *const u8, 1024) };
    assert_eq!(&expected[256..416], &observed[256..416], "XMM6..15");
    assert_eq!(
        u32::from_le_bytes(observed[24..28].try_into().unwrap()) & 0xffc0,
        0x1f80,
        "ABI MXCSR control bits"
    );
    let restored_cr4: u64;
    unsafe {
        asm!("mov {}, cr4",out(reg) restored_cr4,options(nostack,preserves_flags));
    }
    assert_eq!(restored_cr4, cr4, "CR4 restored by fixture");
    unsafe {
        boot::free_pages(areas, 4).unwrap();
    }
    super::marker("PASS parent-start-abi-canaries\n");
    unsafe { core::mem::transmute(call.status as usize) }
}
