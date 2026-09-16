#![cfg(feature = "native-preflight")]
use svmvisor_dxe::native::resident::iommu::*;
use svmvisor_hypervisor::{
    memory::address::{AddressPolicy, EncryptionState},
    svm::iommu::Error as HardwareError,
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap()
}
fn sum(bytes: &mut [u8], offset: usize) {
    bytes[offset] = 0;
    bytes[offset] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |s, b| s.wrapping_add(*b)));
}
fn block(kind: u8, xt: bool) -> Vec<u8> {
    let header = if kind == 0x10 { 24 } else { 40 };
    let mut b = vec![0u8; header + 4];
    b[0] = kind;
    b[2..4].copy_from_slice(&((header + 4) as u16).to_le_bytes());
    b[4..6].copy_from_slice(&2u16.to_le_bytes());
    b[6..8].copy_from_slice(&0x40u16.to_le_bytes());
    b[8..16].copy_from_slice(&0xf7600000u64.to_le_bytes());
    if kind != 0x10 {
        b[24..32]
            .copy_from_slice(&(1u64 << 7 | 1 << 21 | if xt { 1 << 2 } else { 0 }).to_le_bytes());
    }
    b[header] = 1;
    b
}
fn ivrs(blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 48];
    b[..4].copy_from_slice(b"IVRS");
    b[8] = 2;
    b[36] = 3;
    for block in blocks {
        b.extend_from_slice(block);
    }
    let len = b.len() as u32;
    b[4..8].copy_from_slice(&len.to_le_bytes());
    sum(&mut b, 9);
    b
}

#[test]
fn inventory_deduplicates_formats_preserves_source_bytes_and_refuses_prior_xt_clear() {
    let bytes = ivrs(&[block(0x10, false), block(0x11, false), block(0x40, false)]);
    let inventory = discover_ivrs(&bytes, &policy()).unwrap();
    assert_eq!(inventory.units().count(), 1);
    let d = inventory.units().next().unwrap();
    assert_eq!(d.kind, 0x11);
    assert_eq!(d.unit.mmio.base(), 0xf7600000);
    assert_eq!(inventory.bytes(), &bytes);
    assert_eq!(inventory.device_bytes(d), Ok(&[1, 0, 0, 0][..]));
    assert_eq!(
        inventory.admit_x2avic(),
        Err(Error::Features(HardwareError::MissingX2Apic))
    );
    let bytes = ivrs(&[block(0x11, true)]);
    assert!(
        discover_ivrs(&bytes, &policy())
            .unwrap()
            .admit_x2avic()
            .is_ok()
    );
}

#[test]
fn duplicates_conflicts_truncated_sources_and_bad_checksum_are_not_silently_accepted() {
    let bytes = ivrs(&[block(0x11, true), block(0x40, true), block(0x40, true)]);
    assert!(matches!(
        discover_ivrs(&bytes, &policy()),
        Err(Error::Duplicate)
    ));
    let bytes = ivrs(&[block(0x11, true), block(0x40, false)]);
    assert!(matches!(
        discover_ivrs(&bytes, &policy()),
        Err(Error::Duplicate)
    ));
    let mut b = block(0x11, true);
    b[40] = 3;
    let bytes = ivrs(&[b]);
    assert!(matches!(
        discover_ivrs(&bytes, &policy()),
        Err(Error::Range)
    ));
    let mut bytes = ivrs(&[block(0x11, true)]);
    bytes[9] ^= 1;
    assert!(matches!(
        discover_ivrs(&bytes, &policy()),
        Err(Error::Checksum)
    ));
}

#[test]
fn ivmd_exclusion_and_special_requester_are_retained_without_policy_rewrite() {
    let mut b = block(0x40, true);
    b.truncate(40);
    b.extend_from_slice(&[0x48, 0, 0, 0xd7, 0x20, 0xa0, 0, 1]);
    b[2..4].copy_from_slice(&48u16.to_le_bytes());
    let mut md = vec![0u8; 32];
    md[0] = 0x22;
    md[1] = 8;
    md[2] = 32;
    md[6..8].copy_from_slice(&0xfffu16.to_le_bytes());
    md[16..24].copy_from_slice(&0x947a5000u64.to_le_bytes());
    md[24..32].copy_from_slice(&4096u64.to_le_bytes());
    let bytes = ivrs(&[b, md]);
    let inventory = discover_ivrs(&bytes, &policy()).unwrap();
    assert_eq!(inventory.ivinfo() & 2, 2);
    assert_eq!(inventory.bytes(), &bytes);
    assert_eq!(
        inventory
            .device_bytes(inventory.units().next().unwrap())
            .unwrap(),
        [0x48, 0, 0, 0xd7, 0x20, 0xa0, 0, 1]
    );
}

struct Reader {
    memory: Vec<u8>,
    reads: Vec<(u64, usize)>,
}
impl FirmwareReader for Reader {
    fn read(&mut self, a: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads.push((a, out.len()));
        let data = self
            .memory
            .get(a as usize..(a as usize).checked_add(out.len()).ok_or(Error::Address)?)
            .ok_or(Error::Address)?;
        out.copy_from_slice(data);
        Ok(())
    }
}
fn acpi_reader(duplicate: bool) -> Reader {
    let mut memory = vec![0u8; 0x4000];
    let bytes = ivrs(&[block(0x11, true)]);
    memory[0x3000..0x3000 + bytes.len()].copy_from_slice(&bytes);
    let n = if duplicate { 52 } else { 44 };
    let root = &mut memory[0x2000..0x2000 + n];
    root[..4].copy_from_slice(b"XSDT");
    root[4..8].copy_from_slice(&(n as u32).to_le_bytes());
    root[8] = 1;
    root[36..44].copy_from_slice(&0x3000u64.to_le_bytes());
    if duplicate {
        root[44..52].copy_from_slice(&0x3000u64.to_le_bytes());
    }
    sum(root, 9);
    let rsdp = &mut memory[0x1000..0x1024];
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[15] = 2;
    rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
    rsdp[24..32].copy_from_slice(&0x2000u64.to_le_bytes());
    sum(&mut rsdp[..20], 8);
    sum(rsdp, 32);
    Reader {
        memory,
        reads: vec![],
    }
}

#[test]
fn acpi_walk_validates_checksums_and_unaligned_xsdt_pointer_before_copy() {
    let mut reader = acpi_reader(false);
    let mut output = [0u8; MAX_IVRS_BYTES];
    let len = load_ivrs(0x1000, &mut reader, &mut output).unwrap();
    assert_eq!(len, 92);
    assert!(
        discover_ivrs(&output[..len], &policy())
            .unwrap()
            .admit_x2avic()
            .is_ok()
    );
    assert!(reader.reads.contains(&(0x2024, 8)));
    reader.memory[0x1008] ^= 1;
    assert_eq!(
        load_ivrs(0x1000, &mut reader, &mut output),
        Err(Error::Checksum)
    );
}

#[test]
fn acpi_walk_rejects_duplicates_unreadable_roots_and_oversized_tables() {
    let mut output = [0u8; MAX_IVRS_BYTES];
    let mut reader = acpi_reader(true);
    assert_eq!(
        load_ivrs(0x1000, &mut reader, &mut output),
        Err(Error::Duplicate)
    );
    let mut reader = acpi_reader(false);
    reader.memory[0x2004..0x2008].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        load_ivrs(0x1000, &mut reader, &mut output),
        Err(Error::Header)
    );
    assert_eq!(
        load_ivrs(0x5000, &mut reader, &mut output),
        Err(Error::Address)
    );
}
