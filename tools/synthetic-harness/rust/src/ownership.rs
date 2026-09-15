//! Consume the firmware ownership snapshot before dropping firmware mappings.
//! The retained copy lies in private image BSS, never in an external pool.
use crate::print;
use core::{arch::x86_64::__cpuid, ptr};
use svmvisor_hypervisor::{
    boot::ownership::{HANDOFF_PAGE_BYTES, OwnershipRecord},
    memory::address::{AddressPolicy, EncryptionState},
};
static mut RETAINED: [u8; HANDOFF_PAGE_BYTES] = [0; HANDOFF_PAGE_BYTES];
unsafe extern "C" {
    static resident_handoff: u64;
    static image_start: u8;
}
fn policy() -> AddressPolicy {
    assert!(__cpuid(0x80000000).eax >= 0x80000008);
    AddressPolicy::new(
        __cpuid(0x80000008).eax as u8,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap()
}
fn fail() -> ! {
    print("FAIL resident-ownership\n");
    crate::finish(false)
}
/// # Safety
/// Called once on the BSP under the identity bootstrap mapping, before private
/// CR3 installation. UEFI loader has allocated/initialized the full arena and
/// ceased boot services; resident_handoff was set only by the checked assembly
/// entry. Multiboot leaves it zero and supplies no firmware ownership evidence.
/// No guest or other CPU may access RETAINED during or after initialization.
pub unsafe fn capture() -> Option<OwnershipRecord<'static>> {
    let source = unsafe { ptr::read_volatile(ptr::addr_of!(resident_handoff)) };
    if source == 0 {
        return None;
    }
    let base = core::hint::black_box(ptr::addr_of!(image_start) as u64);
    if source != base + 0xff000 {
        fail();
    }
    let retained = ptr::addr_of_mut!(RETAINED).cast::<u8>();
    unsafe {
        ptr::copy_nonoverlapping(source as *const u8, retained, HANDOFF_PAGE_BYTES);
    }
    let bytes = unsafe { core::slice::from_raw_parts(retained.cast_const(), HANDOFF_PAGE_BYTES) };
    if bytes[48..64].iter().any(|b| *b != 0) {
        fail();
    }
    let record = OwnershipRecord::decode(bytes, &policy()).unwrap_or_else(|_| fail());
    if record.arena().base() != base {
        fail();
    }
    print("PASS resident-ownership-retained\n");
    Some(record)
}
/// Read the retained image-owned copy after private mappings replaced CR3.
/// Also exercise the proposed guest physical-map reservation independently of
/// source descriptor splitting. This view is not yet published to Windows.
pub fn verify_private(record: &OwnershipRecord<'_>) {
    let mut source_bytes = 0u64;
    for descriptor in record.descriptors() {
        source_bytes += descriptor.page_count * 4096;
    }
    let mut projected_bytes = 0u64;
    let mut reserved_bytes = 0u64;
    let arena = record.arena();
    let end = arena.last_byte() + 1;
    for descriptor in record.guest_descriptors() {
        let bytes = descriptor.page_count * 4096;
        projected_bytes += bytes;
        let start = descriptor.physical_start;
        let limit = start + bytes;
        if start < end && limit > arena.base() {
            assert!(start >= arena.base() && limit <= end);
            assert_eq!(descriptor.memory_type, 0);
            reserved_bytes += bytes;
        }
    }
    assert_eq!(source_bytes, projected_bytes);
    assert_eq!(reserved_bytes, arena.len());
    if let Some(smp)=record.smp() {
        let low=smp.low_page();
        let reserved:u64=record.guest_descriptors().filter_map(|d| {
            let finish=d.physical_start+d.page_count*4096;
            if d.physical_start<=low.base() && finish>low.last_byte() {
                assert_eq!(d.memory_type,0);Some(low.len())
            } else {None}
        }).sum();
        assert_eq!(reserved,4096);
        assert!(record.excludes_guest_range(low));
        print("PASS resident-smp-low-reservation\n");
    }
    print("PASS resident-ownership-private\n");
}
