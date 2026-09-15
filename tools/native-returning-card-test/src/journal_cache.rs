//! TCG-only qualification of the owned mock journal page. Firmware sets UC;
//! an independent reader records the actual leaf PAT selection and variable
//! MTRR UC match. No assertion about a physical card or physical cache is made.
use core::{
    arch::{asm, x86_64::__cpuid},
    ptr,
};
use uefi_raw::{Status, guid};
#[repr(C)]
struct CpuArch {
    prefix: [usize; 7],
    set_attributes: unsafe extern "efiapi" fn(*const CpuArch, u64, u64, u64) -> Status,
    timers: u32,
    alignment: u32,
}
unsafe fn msr(index: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdmsr",in("ecx")index,out("eax")lo,out("edx")hi,options(nostack,preserves_flags));
    }
    u64::from(lo) | (u64::from(hi) << 32)
}
pub fn qualify(page: u64) {
    assert!(super::tcg());
    assert!(page & 4095 == 0 && (0x100000..0x10000000).contains(&page));
    let mut raw = ptr::null_mut();
    super::check(
        unsafe {
            (super::services().locate_protocol)(
                &guid!("26baccb1-6f42-11d4-bce7-0080c73c8881"),
                ptr::null_mut(),
                &mut raw,
            )
        },
        "locate CPU architectural protocol",
    );
    assert!(!raw.is_null());
    let cpu = raw.cast::<CpuArch>();
    super::check(
        unsafe { ((*cpu).set_attributes)(cpu, page, 4096, 1) },
        "firmware journal UC request",
    );
    let cr3: u64;
    unsafe {
        asm!("mov {}, cr3",out(reg)cr3,options(nostack,preserves_flags));
    }
    let mut table = cr3 & 0x000f_ffff_ffff_f000;
    let mut selection = None;
    for (level, shift) in [(4, 39), (3, 30), (2, 21), (1, 12)] {
        assert!((0x100000..0x10000000).contains(&table) && table & 4095 == 0);
        let entry =
            unsafe { ((table + ((page >> shift) & 511) * 8) as *const u64).read_volatile() };
        assert_ne!(entry & 1, 0);
        if level == 1 || entry & 128 != 0 {
            assert!(level != 4);
            let bytes = 1u64 << shift;
            assert_eq!(
                (entry & 0x000f_ffff_ffff_f000 & !(bytes - 1)) + (page & (bytes - 1)),
                page
            );
            let pat_bit = if level == 1 { 7 } else { 12 };
            let index =
                ((entry >> 3) & 1) | (((entry >> 4) & 1) << 1) | (((entry >> pat_bit) & 1) << 2);
            selection = Some((index, entry));
            break;
        }
        table = entry & 0x000f_ffff_ffff_f000;
    }
    let (index, leaf) = selection.unwrap();
    let pat = unsafe { msr(0x277) };
    let pat_type = (pat >> (index * 8)) & 255;
    let cap = unsafe { msr(0xfe) };
    let def = unsafe { msr(0x2ff) };
    assert_ne!(def & (1 << 11), 0);
    assert!(cap & 255 <= 16);
    let bits = __cpuid(0x80000008).eax & 255;
    assert!((32..=52).contains(&bits));
    let address_mask = ((1u64 << bits) - 1) & !4095;
    let mut uc = def & 255 == 0;
    for i in 0..(cap as u32 & 255) {
        let base = unsafe { msr(0x200 + 2 * i) };
        let mask = unsafe { msr(0x201 + 2 * i) };
        if mask & 0x800 != 0 && (base ^ page) & mask & address_mask == 0 && base & 255 == 0 {
            uc = true;
        }
    }
    super::field("journal-leaf", leaf);
    super::field("journal-pat", pat);
    super::field("journal-pat-index", index);
    super::field("journal-pat-type", pat_type);
    super::field("journal-mtrr-default", def);
    super::field("journal-mtrr-uc", uc as u64);
    assert!(
        pat_type == 0 || uc,
        "actual guest cache selection must be UC"
    );
    super::marker("PASS firmware-journal-uc-qualified\n");
}
