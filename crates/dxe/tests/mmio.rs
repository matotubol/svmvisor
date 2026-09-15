#[path = "../src/firmware/mmio.rs"]
mod mmio;
use svmvisor_dxe::journal::JournalIo;
use uefi_raw::Status;

fn descriptor() -> [u8; 48] {
    let mut data = [0; 48];
    data[0] = 0x8a;
    data[1] = 43;
    data[6..14].copy_from_slice(&32u64.to_le_bytes());
    data[14..22].copy_from_slice(&0x80000000u64.to_le_bytes());
    data[38..46].copy_from_slice(&4096u64.to_le_bytes());
    data
}
#[test]
fn descriptor_requires_uncached_aligned_4k_host_memory() {
    let valid = descriptor();
    assert!(unsafe { mmio::JournalMapping::from_descriptor(valid.as_ptr()) }.is_ok());
    for (offset, value) in [
        (0, 0),
        (1, 42),
        (2, 1),
        (3, 1),
        (5, 2),
        (5, 4),
        (5, 6),
        (6, 64),
        (14, 1),
        (18, 1),
        (38, 1),
    ] {
        let mut data = valid;
        data[offset] = value;
        assert!(unsafe { mmio::JournalMapping::from_descriptor(data.as_ptr()) }.is_err());
    }
    let mut data = valid;
    data[14..22].fill(0);
    assert!(unsafe { mmio::JournalMapping::from_descriptor(data.as_ptr()) }.is_err());
    assert!(unsafe { mmio::JournalMapping::from_descriptor(core::ptr::null()) }.is_err());
}
#[test]
fn invalid_accesses_fail_before_touching_the_bus() {
    let data = descriptor();
    let mut mapping = unsafe { mmio::JournalMapping::from_descriptor(data.as_ptr()) }.unwrap();
    for offset in [0, 0x024, 0x03c, 0x041, 0x061, 0x064, 0x080, u64::MAX] {
        assert_eq!(mapping.write(offset, 1), Err(Status::INVALID_PARAMETER));
    }
    for offset in [1, 0x09d, 0x0a0, u64::MAX] {
        assert_eq!(mapping.read(offset), Err(Status::INVALID_PARAMETER));
    }
}
