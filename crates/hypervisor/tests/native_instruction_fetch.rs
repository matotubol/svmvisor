use std::collections::BTreeMap;
use svmvisor_hypervisor::{
    host::resident::fetch::{FetchError, instruction, startup_instruction},
    svm::vmcb::Vmcb,
};

fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            value.to_le_bytes().as_ptr(),
            (vmcb as *mut Vmcb).cast::<u8>().add(offset),
            8,
        );
    }
}
fn setup() -> (Vmcb, BTreeMap<u64, u64>, BTreeMap<u64, u8>) {
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x70, 0x72);
    put(&mut vmcb, 0x410, 0x200 << 16); // Long64, CS.D=0.
    put(&mut vmcb, 0x4d0, 0x1d00);
    put(&mut vmcb, 0x548, 0x20);
    put(&mut vmcb, 0x550, 0x1000);
    put(&mut vmcb, 0x558, 0x8000_0001);
    put(&mut vmcb, 0x578, 0x4fff);
    let tables = BTreeMap::from([
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3000, 0x4003),
        (0x4020, 0x9003),
        (0x4028, 0xb003),
    ]);
    let bytes = BTreeMap::from([(0x9fff, 0x0f), (0xb000, 0xa2)]);
    (vmcb, tables, bytes)
}
fn fetch(
    vmcb: &Vmcb,
    tables: &BTreeMap<u64, u64>,
    bytes: &BTreeMap<u64, u8>,
) -> Result<[u8; 2], FetchError> {
    instruction(vmcb, 48, 6, |address, width| match width {
        8 => tables.get(&address).copied(),
        1 => bytes.get(&address).copied().map(u64::from),
        _ => panic!("unbounded read width"),
    })
}

#[test]
fn startup_fetch_uses_real_cs_base_and_protected_default_size() {
    for (attributes, ip) in [(0x9bu64, 0x1234u64), (0xc9b, 0x12345)] {
        let mut vmcb = Vmcb::new();
        put(&mut vmcb, 0x70, 0x7c);
        put(&mut vmcb, 0x410, (u32::MAX as u64) << 32 | attributes << 16);
        put(&mut vmcb, 0x418, 0x8000);
        put(&mut vmcb, 0x558, 0x11);
        put(&mut vmcb, 0x4d0, 0x1100);
        put(&mut vmcb, 0x578, ip);
        let mut reads = Vec::new();
        assert_eq!(
            startup_instruction(&vmcb, 48, 0, |address, width| {
                reads.push((address, width));
                Some(if address == 0x8000 + ip { 0x0f } else { 0x30 })
            }),
            Ok([0x0f, 0x30])
        );
        assert_eq!(reads, [(0x8000 + ip, 1), (0x8001 + ip, 1)]);
        assert_eq!(
            instruction(&vmcb, 48, 6, |_, _| panic!("strict mode read")),
            Err(FetchError::UnsupportedMode)
        );
    }
}

#[test]
fn startup_fetch_rejects_limit_wrap_paging_and_mmio_without_state_changes() {
    for (attributes, limit, rip, cr0, expected) in [
        (
            0x9bu64,
            0xffffu64,
            0xfffeu64,
            0x10u64,
            FetchError::SegmentLimit,
        ),
        (0xc9b, 0x100, 0xff, 0x11, FetchError::SegmentLimit),
        (0xc9b, 0xffff, 0x10, 0x80000011, FetchError::UnsupportedMode),
        (0x9b, 0xffff, 0x10, 0x40000010, FetchError::UnsupportedMode),
    ] {
        let mut vmcb = Vmcb::new();
        put(&mut vmcb, 0x70, 0x72);
        put(&mut vmcb, 0x410, limit << 32 | attributes << 16);
        put(&mut vmcb, 0x558, cr0);
        put(&mut vmcb, 0x578, rip);
        let before = *vmcb.bytes();
        assert_eq!(
            startup_instruction(&vmcb, 48, 6, |_, _| panic!("preflight read")),
            Err(expected)
        );
        assert_eq!(*vmcb.bytes(), before);
    }
    let mut vmcb = Vmcb::new();
    put(&mut vmcb, 0x70, 0x72);
    put(&mut vmcb, 0x410, 0xffffu64 << 32 | 0x9b << 16);
    put(&mut vmcb, 0x418, 0xa0000);
    assert_eq!(
        startup_instruction(&vmcb, 48, 6, |_, _| None),
        Err(FetchError::UnreadableInstruction { address: 0xa0000 })
    );
}

#[test]
fn fixed_mtrr_lookup_covers_every_low_page_with_architectural_granularity() {
    use svmvisor_hypervisor::memory::mtrrs::Mtrrs;
    for page in (0..0x100000).step_by(4096) {
        let (register, shift) = Mtrrs::fixed_range_register(page).unwrap();
        let expected = match register {
            0x250 => u64::from(shift / 8) * 0x10000,
            0x258 => 0x80000 + u64::from(shift / 8) * 0x4000,
            0x259 => 0xa0000 + u64::from(shift / 8) * 0x4000,
            0x268..=0x26f => {
                0xc0000 + u64::from(register - 0x268) * 0x8000 + u64::from(shift / 8) * 0x1000
            }
            _ => panic!("wrong architectural register"),
        };
        let span = if register == 0x250 {
            0x10000
        } else if register < 0x260 {
            0x4000
        } else {
            4096
        };
        assert!(expected <= page && page < expected + span);
        assert_eq!(shift & 7, 0);
        assert!(shift <= 56);
    }
    assert_eq!(Mtrrs::fixed_range_register(0x100000), None);
    assert_eq!(Mtrrs::fixed_range_register(1), None);
}

#[test]
fn noncontiguous_page_boundary_is_walked_twice_without_nrip() {
    let (mut vmcb, tables, bytes) = setup();
    for nrip in [0, u64::MAX] {
        put(&mut vmcb, 0xc8, nrip);
        let mut reads = Vec::new();
        let actual = instruction(&vmcb, 48, 6, |address, width| {
            reads.push((address, width));
            if width == 8 {
                tables.get(&address).copied()
            } else {
                bytes.get(&address).copied().map(u64::from)
            }
        });
        assert_eq!(actual, Ok([0x0f, 0xa2]));
        assert_eq!(reads.len(), 10);
        assert_eq!(reads[4], (0x9fff, 1));
        assert_eq!(reads[9], (0xb000, 1));
    }
}

#[test]
fn fetch_uses_current_guest_cr3_and_preserves_fetched_prefix() {
    let (mut vmcb, mut tables, mut bytes) = setup();
    tables.extend([
        (0x5000, 0x6003),
        (0x6000, 0x7003),
        (0x7000, 0x8003),
        (0x8020, 0xd003),
        (0x8028, 0xe003),
    ]);
    bytes.extend([(0xdfff, 0x66), (0xe000, 0x0f)]);
    assert_eq!(fetch(&vmcb, &tables, &bytes), Ok([0x0f, 0xa2]));
    put(&mut vmcb, 0x550, 0x5000);
    assert_eq!(fetch(&vmcb, &tables, &bytes), Ok([0x66, 0x0f]));
}

#[test]
fn unsupported_modes_and_noninstruction_exits_never_read_physical_memory() {
    for (offset, value) in [
        (0x558, 1),
        (0x548, 0),
        (0x548, 0x1020),
        (0x4d0, 0x1900),
        (0x410, 0),
        (0x410, 0x600 << 16),
    ] {
        let (mut vmcb, _, _) = setup();
        put(&mut vmcb, offset, value);
        assert_eq!(
            instruction(&vmcb, 48, 6, |_, _| panic!("read before mode admission")),
            Err(FetchError::UnsupportedMode)
        );
    }
    let (mut vmcb, _, _) = setup();
    put(&mut vmcb, 0x70, 0x7f);
    assert_eq!(
        instruction(&vmcb, 48, 6, |_, _| panic!("read after shutdown")),
        Err(FetchError::UnsupportedExit)
    );
}

#[test]
fn page_permissions_and_cache_aliases_are_refused_before_instruction_read() {
    let (mut vmcb, mut tables, bytes) = setup();
    tables.insert(0x4020, 0x9003 | (1 << 63));
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::NotExecutable)
    );
    tables.insert(0x4020, 0x9003);
    put(&mut vmcb, 0x4c8, 3 << 24);
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::PrivilegeMismatch)
    );
    put(&mut vmcb, 0x4c8, 0);
    tables.insert(0x2000, 0x3013);
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::UnsupportedCacheControl)
    );
    tables.insert(0x2000, 0x3003);
    tables.insert(0x4020, 0x9083); // Leaf PAT=1 selects slot4, currently UC.
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::NonWriteBackInstruction)
    );
    put(&mut vmcb, 0x550, 0x1008);
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::UnsupportedCacheControl)
    );
}

#[test]
fn unreadable_second_byte_and_noncanonical_pagecross_preserve_vmcb() {
    let (mut vmcb, mut tables, mut bytes) = setup();
    bytes.remove(&0xb000);
    let original = *vmcb.bytes();
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::UnreadableInstruction { address: 0xb000 })
    );
    assert_eq!(*vmcb.bytes(), original);
    put(&mut vmcb, 0x578, 0x7fff_ffff_ffff);
    tables.extend([
        (0x17f8, 0x2003),
        (0x2ff8, 0x3003),
        (0x3ff8, 0x4003),
        (0x4ff8, 0x9003),
    ]);
    assert!(matches!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::Walk(
            svmvisor_hypervisor::host::paging::WalkError::NoncanonicalAddress
        ))
    ));
}

#[test]
fn software_marked_page_crossing_fetch_ignores_mpk_even_when_enabled() {
    for pke in [false, true] {
        let (mut vmcb, mut tables, bytes) = setup();
        put(&mut vmcb, 0x548, 0x20 | if pke { 1 << 22 } else { 0 });
        for entry in tables.values_mut() {
            *entry |= 0x7ff0_0000_0000_0000;
        }
        let before = *vmcb.bytes();
        assert_eq!(fetch(&vmcb, &tables, &bytes), Ok([0x0f, 0xa2]));
        assert_eq!(*vmcb.bytes(), before);
    }
}

#[test]
fn nonzero_wb_selectors_fetch_across_noncontiguous_pages() {
    let (mut vmcb, mut tables, bytes) = setup();
    // Root PA3, nonleaf PA1, leaf PA7 on first page and PA2 on second page.
    put(&mut vmcb, 0x550, 0x1018);
    for address in [0x1000, 0x2000, 0x3000] {
        *tables.get_mut(&address).unwrap() |= 8;
    }
    *tables.get_mut(&0x4020).unwrap() |= 0x98;
    *tables.get_mut(&0x4028).unwrap() |= 0x10;
    let original = *vmcb.bytes();
    let pat = 0x0600_0000_0606_0600;
    let mut accesses = Vec::new();
    assert_eq!(
        instruction(&vmcb, 48, pat, |address, width| {
            accesses.push((address, width));
            match width {
                8 => tables.get(&address).copied(),
                1 => bytes.get(&address).copied().map(u64::from),
                _ => panic!("unexpected read width"),
            }
        }),
        Ok([0x0f, 0xa2])
    );
    assert_eq!(accesses.len(), 10);
    assert_eq!(accesses[4], (0x9fff, 1));
    assert_eq!(accesses[9], (0xb000, 1));
    assert_eq!(*vmcb.bytes(), original);
    // Selected WB never authorizes a missing owned physical mapping.
    assert!(matches!(
        instruction(&vmcb, 48, pat, |_, _| None),
        Err(FetchError::Walk(_))
    ));
}

#[test]
fn root_pat_selection_preserves_pcid_semantics() {
    for pcid in [false, true] {
        for low in [0, 8, 16, 24] {
            for memory_type in [0, 1, 4, 5, 6, 7] {
                let (mut vmcb, tables, bytes) = setup();
                put(&mut vmcb, 0x548, 0x20 | if pcid { 1 << 17 } else { 0 });
                put(&mut vmcb, 0x550, 0x1000 | low);
                let selector = if pcid { 0 } else { low >> 3 };
                if memory_type == 7 && selector != 2 {
                    continue;
                }
                let pat = (0x0606_0606_0606_0606 & !(255 << (selector * 8)))
                    | (memory_type << (selector * 8));
                let mut reads = 0;
                let result = instruction(&vmcb, 48, pat, |address, width| {
                    reads += 1;
                    if width == 8 {
                        tables.get(&address).copied()
                    } else {
                        bytes.get(&address).copied().map(u64::from)
                    }
                });
                if memory_type == 6 {
                    assert_eq!(result, Ok([0x0f, 0xa2]));
                } else {
                    assert_eq!(result, Err(FetchError::UnsupportedCacheControl));
                    assert_eq!(reads, 0);
                }
            }
        }
    }
    let (mut vmcb, tables, bytes) = setup();
    put(&mut vmcb, 0x548, 0x20 | (1 << 17));
    for pcid in [1, 0x18, 0xfff] {
        put(&mut vmcb, 0x550, 0x1000 | pcid);
        assert_eq!(fetch(&vmcb, &tables, &bytes), Ok([0x0f, 0xa2]));
    }
    put(&mut vmcb, 0x550, (1 << 63) | 0x1000);
    assert_eq!(
        fetch(&vmcb, &tables, &bytes),
        Err(FetchError::Walk(
            svmvisor_hypervisor::host::paging::WalkError::InvalidCr3
        ))
    );
}

#[test]
fn malformed_pat_and_disabled_caching_refuse_before_any_read() {
    let (mut vmcb, _, _) = setup();
    let original = *vmcb.bytes();
    for index in 0..8 {
        for value in 0..=255 {
            if matches!(value, 0 | 1 | 4 | 5 | 6) || (value == 7 && (index == 2 || index == 6)) {
                continue;
            }
            let pat = (0x0606_0606_0606_0606 & !(255 << (index * 8))) | (value << (index * 8));
            assert_eq!(
                instruction(&vmcb, 48, pat, |_, _| panic!("invalid PAT read")),
                Err(FetchError::UnsupportedCacheControl)
            );
        }
    }
    assert_eq!(*vmcb.bytes(), original);
    put(&mut vmcb, 0x558, 0xc000_0001);
    assert_eq!(
        instruction(&vmcb, 48, 6, |_, _| panic!("cache disabled read")),
        Err(FetchError::UnsupportedCacheControl)
    );
}
