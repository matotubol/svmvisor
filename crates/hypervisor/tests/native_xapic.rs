use std::collections::BTreeMap;
use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    host::{paging::WalkError, resident::fetch::FetchError},
    svm::{
        ipi::NativeIcrError,
        vmcb::Vmcb,
        xapic::{NativeMmioError, handle_native_mmio},
    },
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

#[test]
fn uaie_ds_operands_ignore_only_the_upper_seven_bits_and_preserve_sources() {
    for tag in [1u64, 0x35, 0x7f] {
        for write in [false, true] {
            let mut g = Guest::new(&[if write { 0x89 } else { 0x8b }, 0x03], write);
            put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
            g.frame.rbx |= tag << 57;
            let frame = g.frame;
            g.run(|_, offset, value| {
                assert_eq!((offset, value), (0x300, write.then_some(0x4600)));
                Ok(0x8000_0001)
            })
            .unwrap();
            assert_eq!(g.frame, frame);
            assert_eq!(g.vmcb.guest_rip(), 0x4002);
            assert_eq!(
                g.vmcb.guest_rax(),
                if write {
                    0xabcd_1234_0000_4600
                } else {
                    0x8000_0001
                }
            );
        }
    }
}

#[test]
fn uaie_uses_the_actual_base_segment_not_the_index_or_low_rex_bits() {
    for (code, base, stack) in [
        (&[0x89, 0x45, 0][..], 5, true),            // [RBP+0]
        (&[0x89, 0x04, 0x24][..], 4, true),         // [RSP]
        (&[0x41, 0x89, 0x45, 0][..], 13, false),    // [R13+0]
        (&[0x41, 0x89, 0x04, 0x24][..], 12, false), // [R12]
        (&[0x89, 0x04, 0x2b][..], 3, false),        // [RBX+RBP]
    ] {
        let mut g = Guest::new(code, true);
        put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
        let tagged = 0x6a00_0000_0000_6300;
        match base {
            4 => put(&mut g.vmcb, 0x5d8, tagged),
            5 => g.frame.rbp = tagged,
            12 => g.frame.r12 = tagged,
            13 => g.frame.r13 = tagged,
            _ => {
                g.frame.rbx = 0;
                g.frame.rbp = tagged;
            }
        }
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        let result = g.run(|_, offset, _| {
            assert!(!stack, "tagged SS operand reached owner");
            assert_eq!(offset, 0x300);
            Ok(0)
        });
        if stack {
            assert_eq!(
                result,
                Err(NativeMmioError::Fetch(FetchError::Walk(
                    WalkError::NoncanonicalAddress
                )))
            );
            assert_eq!(*g.vmcb.bytes(), before);
        } else {
            assert_eq!(result, Ok(()));
        }
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn uaie_normalizes_after_wrapping_address_arithmetic_and_keeps_high_half() {
    // Tagged RBX + RBP wraps modulo 2^64 to a differently tagged DS address.
    let mut g = Guest::new(&[0x89, 0x04, 0x2b], true);
    put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
    g.frame.rbx = 0xffff_ffff_ffff_fff0;
    g.frame.rbp = 0x0200_0000_0000_6310;
    g.run(|_, offset, _| {
        assert_eq!(offset, 0x300);
        Ok(0)
    })
    .unwrap();

    // Canonical high-half FFFF800000006300 with an arbitrary top-seven tag.
    let mut g = Guest::new(&[0x89, 0x03], true);
    put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
    g.tables.insert(0x1800, 0x2003); // PML4[256]
    g.frame.rbx = 0x55ff_8000_0000_6300;
    g.run(|_, offset, _| {
        assert_eq!(offset, 0x300);
        Ok(0)
    })
    .unwrap();
}

#[test]
fn uaie_keeps_remaining_canonical_bits_and_all_refusal_state() {
    for case in 0..13 {
        let mut g = Guest::new(&[0x89, 0x03], true);
        put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
        g.frame.rbx = 0x6a00_0000_0000_6300;
        match case {
            0 => put(&mut g.vmcb, 0x4d0, 0x1d00),      // UAIE disabled
            1..=9 => g.frame.rbx |= 1 << (47 + case),  // bits48..56 remain checked
            10 => g.frame.rbx |= 1 << 47,              // bit47=1 needs all56:48 set
            11 => put(&mut g.vmcb, 0x80, 0xfee0_0310), // exact NPF mismatch
            _ => g.frame.rbx = 0x6a00_0000_0000_6ffe,  // DWORD crosses leaf
        }
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        let result = g.run(|_, _, _| panic!("invalid UAIE operand reached owner"));
        assert!(result.is_err(), "case {case}");
        if case <= 10 {
            assert_eq!(
                result,
                Err(NativeMmioError::Fetch(FetchError::Walk(
                    WalkError::NoncanonicalAddress
                )))
            );
        }
        assert_eq!(*g.vmcb.bytes(), before);
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn uaie_does_not_extend_prefix_address_size_or_instruction_address_support() {
    for prefix in [0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0xf0] {
        let mut g = Guest::new(&[prefix, 0x89, 0x03], true);
        put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
        g.frame.rbx = 0x6a00_0000_0000_6300;
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        assert_eq!(
            g.run(|_, _, _| panic!("unsupported prefix reached owner")),
            Err(NativeMmioError::Instruction)
        );
        assert_eq!(*g.vmcb.bytes(), before);
        assert_eq!(g.frame, frame);
    }
    let mut g = Guest::new(&[0x89, 0x03], true);
    put(&mut g.vmcb, 0x4d0, 0x1d00 | (1 << 20));
    put(&mut g.vmcb, 0x578, 0x6a00_0000_0000_4000);
    let before = *g.vmcb.bytes();
    let frame = g.frame;
    assert_eq!(
        g.run(|_, _, _| panic!("tagged CS reached owner")),
        Err(NativeMmioError::Fetch(FetchError::Walk(
            WalkError::NoncanonicalAddress
        )))
    );
    assert_eq!(*g.vmcb.bytes(), before);
    assert_eq!(g.frame, frame);
}

struct Guest {
    vmcb: Vmcb,
    frame: GuestRegisters,
    tables: BTreeMap<u64, u64>,
    code: BTreeMap<u64, u8>,
    pat: u64,
}
impl Guest {
    fn new(code: &[u8], write: bool) -> Self {
        let mut vmcb = Vmcb::new();
        put(&mut vmcb, 0x70, 0x400);
        put(&mut vmcb, 0x78, (1 << 32) | 4 | if write { 2 } else { 0 });
        put(&mut vmcb, 0x80, 0xfee0_0300);
        put(&mut vmcb, 0x90, 1);
        put(&mut vmcb, 0xc8, u64::MAX); // NPF does not establish nRIP.
        put(&mut vmcb, 0x410, 0x200 << 16);
        put(&mut vmcb, 0x4d0, 0x1d00);
        put(&mut vmcb, 0x548, 0x20);
        put(&mut vmcb, 0x550, 0x1000);
        put(&mut vmcb, 0x558, 0x8001_0001);
        put(&mut vmcb, 0x570, 2 | (1 << 16));
        put(&mut vmcb, 0x578, 0x4000);
        put(&mut vmcb, 0x5f8, 0xabcd_1234_0000_4600);
        Self {
            vmcb,
            pat: 6,
            frame: GuestRegisters {
                rbx: 0x6300,
                ..Default::default()
            },
            tables: BTreeMap::from([
                (0x1000, 0x2003),
                (0x2000, 0x3003),
                (0x3000, 0x4003),
                (0x4020, 0x9003),
                (0x4028, 0xb003),
                (0x4030, 0xfee0_000b), // PAT index1 is UC; table RAM stays WB.
            ]),
            code: code
                .iter()
                .enumerate()
                .map(|(i, b)| (0x9000 + i as u64, *b))
                .collect(),
        }
    }

    fn run(
        &mut self,
        access: impl FnOnce(&mut Vmcb, u16, Option<u32>) -> Result<u32, NativeIcrError>,
    ) -> Result<(), NativeMmioError> {
        let Self {
            vmcb,
            frame,
            tables,
            code,
            pat,
        } = self;
        handle_native_mmio(
            vmcb,
            frame,
            48,
            *pat,
            0xfee0_0000,
            |address, width| match width {
                8 => tables.get(&address).copied(),
                1 => code.get(&address).map(|v| *v as u64),
                _ => panic!("unbounded read"),
            },
            access,
        )
    }
}

#[test]
fn write_uses_stopped_rax_and_preserves_gprs_while_consuming_rf() {
    let mut g = Guest::new(&[0x89, 0x03], true);
    let frame = g.frame;
    g.run(|_, offset, value| {
        assert_eq!((offset, value), (0x300, Some(0x4600)));
        Ok(0)
    })
    .unwrap();
    assert_eq!(g.vmcb.guest_rax(), 0xabcd_1234_0000_4600);
    assert_eq!(g.frame, frame);
    assert_eq!(g.vmcb.guest_rip(), 0x4002);
    assert_eq!(
        u64::from_le_bytes(g.vmcb.bytes()[0x570..0x578].try_into().unwrap()),
        2
    );
}

#[test]
fn reads_zero_extend_eax_and_extended_registers() {
    for (code, extended) in [(&[0x8b, 0x03][..], false), (&[0x44, 0x8b, 0x0b][..], true)] {
        let mut g = Guest::new(code, false);
        g.frame.r9 = u64::MAX;
        g.run(|_, offset, value| {
            assert_eq!((offset, value), (0x300, None));
            Ok(0x8000_4321)
        })
        .unwrap();
        assert_eq!(
            if extended {
                g.frame.r9
            } else {
                g.vmcb.guest_rax()
            },
            0x8000_4321
        );
        assert_eq!(g.vmcb.guest_rip(), 0x4000 + code.len() as u64);
    }
}

#[test]
fn sib_extended_base_index_and_signed_displacement_are_decoded() {
    // MOV [R12+R13*4-16],R10D.
    let mut g = Guest::new(&[0x47, 0x89, 0x54, 0xac, 0xf0], true);
    g.frame.r12 = 0x6200;
    g.frame.r13 = 0x44;
    g.frame.r10 = 0xffff_ffff_0000_4500;
    g.run(|_, offset, value| {
        assert_eq!((offset, value), (0x300, Some(0x4500)));
        Ok(0)
    })
    .unwrap();
}

#[test]
fn rip_relative_immediate_uses_end_of_complete_instruction() {
    // MOV DWORD PTR [RIP+22F6h],4500h: 400Ah+22F6h = 6300h.
    let mut g = Guest::new(&[0xc7, 0x05, 0xf6, 0x22, 0, 0, 0, 0x45, 0, 0], true);
    g.run(|_, offset, value| {
        assert_eq!((offset, value), (0x300, Some(0x4500)));
        Ok(0)
    })
    .unwrap();
    assert_eq!(g.vmcb.guest_rip(), 0x400a);
}

#[test]
fn sib_without_base_ignores_rex_b_and_rsp_base_uses_vmcb() {
    // A mod=00 SIB base=101 remains disp32-only even with REX.B set.
    let mut g = Guest::new(&[0x41, 0x89, 0x04, 0x25, 0, 0x63, 0, 0], true);
    g.frame.r13 = u64::MAX;
    g.run(|_, offset, _| {
        assert_eq!(offset, 0x300);
        Ok(0)
    })
    .unwrap();
    let mut g = Guest::new(&[0x89, 0x04, 0x24], true); // MOV [RSP],EAX.
    put(&mut g.vmcb, 0x5d8, 0x6300);
    g.run(|_, offset, _| {
        assert_eq!(offset, 0x300);
        Ok(0)
    })
    .unwrap();
}

#[test]
fn crossing_code_page_fetches_only_actual_instruction_bytes() {
    let mut g = Guest::new(&[], true);
    put(&mut g.vmcb, 0x578, 0x4fff);
    g.code = BTreeMap::from([(0x9fff, 0x89), (0xb000, 0x03)]);
    g.run(|_, _, _| Ok(0)).unwrap();
    assert_eq!(g.vmcb.guest_rip(), 0x5001);
}

#[test]
fn unsupported_width_prefix_layout_and_esp_destination_refuse_unchanged() {
    for code in [
        &[0x48, 0x89, 0x03][..],
        &[0x66, 0x89, 0x03],
        &[0x67, 0x89, 0x03],
        &[0xf0, 0x89, 0x03],
        &[0x65, 0x89, 0x03],
        &[0x89, 0xc3],
        &[0xc7, 0x0b],
        &[0x8b, 0x23],
        &[0x40, 0x40, 0x89, 0x03],
    ] {
        let mut g = Guest::new(code, true);
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        assert_eq!(
            g.run(|_, _, _| panic!("unsupported instruction reached owner")),
            Err(NativeMmioError::Instruction)
        );
        assert_eq!(*g.vmcb.bytes(), before);
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn malformed_npf_operand_cache_and_permissions_never_reach_owner() {
    for case in 0..7 {
        let mut g = Guest::new(&[0x89, 0x03], true);
        match case {
            0 => put(&mut g.vmcb, 0x78, (1 << 33) | 6), // page walk, not final data
            1 => put(&mut g.vmcb, 0x80, 0xfee0_0310),   // stopped GPA mismatch
            2 => g.frame.rbx += 4,                      // register hole
            3 => {
                g.pat = 0x106;
            } // WC operand even with MTRR UC
            4 => {
                g.tables.insert(0x4030, 0xfee0_0009);
            } // read-only + CR0.WP
            5 => put(&mut g.vmcb, 0x90, 3),             // unsupported NPT controls
            _ => {
                g.code.remove(&0x9001);
            } // incomplete installed bytes
        }
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        assert!(
            g.run(|_, _, _| panic!("invalid provenance reached owner"))
                .is_err()
        );
        assert_eq!(*g.vmcb.bytes(), before);
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn backend_refusal_does_not_complete_read_or_write() {
    for (code, write) in [(&[0x89, 0x03][..], true), (&[0x8b, 0x03][..], false)] {
        let mut g = Guest::new(code, write);
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        assert_eq!(
            g.run(|_, _, _| Err(NativeIcrError::MailboxBusy)),
            Err(NativeMmioError::Register(NativeIcrError::MailboxBusy))
        );
        assert_eq!(*g.vmcb.bytes(), before);
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn native_mtrr_uc_admission_composes_guest_pat_without_forcing_a_single_index() {
    for pat_type in [0, 4, 5, 6, 7] {
        let mut g = Guest::new(&[0x89, 0x03], true);
        // This target's PPR permits UC- only in PA2/PA6.
        let selector = if pat_type == 7 { 2 } else { 1 };
        g.pat = 6 | (pat_type << (selector * 8));
        g.tables.insert(0x4030, 0xfee0_0003 | (selector << 3));
        g.run(|_, _, _| Ok(0)).unwrap();
    }
}

#[test]
fn wb_nonzero_table_selectors_keep_uc_mmio_leaf_separate() {
    let mut g = Guest::new(&[0x89, 0x03], true);
    // PA0/PA1 WB for table/code accesses; PA3 UC for the MMIO operand.
    g.pat = 0x0606;
    put(&mut g.vmcb, 0x550, 0x1008);
    for address in [0x1000, 0x2000, 0x3000, 0x4020] {
        *g.tables.get_mut(&address).unwrap() |= 8;
    }
    g.tables.insert(0x4030, 0xfee0_001b);
    g.run(|_, offset, value| {
        assert_eq!((offset, value), (0x300, Some(0x4600)));
        Ok(0)
    })
    .unwrap();
    assert_eq!(g.vmcb.guest_rip(), 0x4002);
}

#[test]
fn software_marked_supervisor_instruction_and_mmio_pages_complete_with_pke() {
    for pke in [false, true] {
        let mut g = Guest::new(&[0x89, 0x03], true);
        put(&mut g.vmcb, 0x548, 0x20 | if pke { 1 << 22 } else { 0 });
        for entry in g.tables.values_mut() {
            *entry |= 0x7ff0_0000_0000_0000;
        }
        let frame = g.frame;
        let result = g.run(|_, offset, value| {
            assert_eq!((offset, value), (0x300, Some(0x4600)));
            Ok(0)
        });
        assert_eq!(result, Ok(()));
        assert_eq!(g.vmcb.guest_rip(), 0x4002);
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn pke_user_mmio_operands_including_key_zero_never_reach_device() {
    for key in 0..16 {
        for write in [false, true] {
            let mut g = Guest::new(&[if write { 0x89 } else { 0x8b }, 0x03], write);
            put(&mut g.vmcb, 0x548, 0x20 | (1 << 22));
            for entry in g.tables.values_mut() {
                *entry |= 4;
            }
            *g.tables.get_mut(&0x4030).unwrap() |= key << 59;
            let before = *g.vmcb.bytes();
            let frame = g.frame;
            let result = g.run(|_, _, _| panic!("user MPK operand reached device"));
            if key == 0 {
                assert_eq!(result, Err(NativeMmioError::Operand));
            } else {
                assert_eq!(result, Err(NativeMmioError::Fetch(FetchError::Walk(
                    WalkError::UnsupportedEntryBits { level: 1 }
                ))));
            }
            assert_eq!(*g.vmcb.bytes(), before);
            assert_eq!(g.frame, frame);
        }
    }
}

#[test]
fn detailed_refusals_capture_actual_predicate_operand_without_mutation_or_extra_reads() {
    use svmvisor_hypervisor::svm::xapic::handle_native_mmio_detailed;
    use svmvisor_hypervisor::host::resident::terminal::{apic_failure, stop_words};
    for (bytes, change, reason, operand) in [
        (&[0x48, 0x8b, 3][..], 0, 0x11u64, (1u64 << 56) | 0x48),
        (&[0x8b, 0xc3][..], 0, 0x12, (2u64 << 56) | 0xc38b),
        (&[0x8b, 3][..], 1, 10, 0x1000020),
        (&[0x8b, 3][..], 2, 0x19, u64::MAX),
        (&[0x8b, 3][..], 3, 0x1a, 0xfee00300),
    ] {
        let mut g=Guest::new(bytes,false);
        match change { 1 => put(&mut g.vmcb,0x548,0x1000020),
            2 => put(&mut g.vmcb,0x78,u64::MAX),
            3 => put(&mut g.vmcb,0x80,0xfee00030), _=>{} }
        let before=*g.vmcb.bytes(); let frame=g.frame; let mut instruction_reads=0;
        let failure=handle_native_mmio_detailed(&mut g.vmcb,&mut g.frame,48,g.pat,0xfee00000,
            |address,width| match width { 8=>g.tables.get(&address).copied(),
                1=>{instruction_reads+=1; g.code.get(&address).map(|b|*b as u64)}, _=>panic!() },
            |_,_,_|panic!("refusal reached APIC")).unwrap_err();
        let (tag,value)=apic_failure(failure);
        assert_eq!((tag>>16,value),(reason,operand));
        let wire=stop_words(0,0x400,0,tag,value).unwrap();
        assert_eq!((wire[0]>>24)&15,13);
        assert_eq!(g.vmcb.bytes(),&before); assert_eq!(g.frame,frame);
        assert_eq!(instruction_reads,if change==1 {0} else if reason==0x11 {1} else {2});
    }
}

#[test]
fn cet_ordinary_mov_obeys_effective_rw_without_changing_shadow_state() {
    // All ancestor/leaf R/W combinations, with and without leaf Dirty.
    // RW=0,D=1 with every ancestor writable is the shadow-stack encoding;
    // ordinary reads remain legal and ordinary writes remain prohibited.
    for writable in 0u8..16 {
        for dirty in [false, true] {
            for write in [false, true] {
                let mut g = Guest::new(&[if write { 0x89 } else { 0x8b }, 3], write);
                put(&mut g.vmcb, 0x548, 0x20 | (1 << 23));
                for (i, address) in [0x1000, 0x2000, 0x3000, 0x4030].into_iter().enumerate() {
                    let entry = g.tables.get_mut(&address).unwrap();
                    *entry = (*entry & !2) | (u64::from(writable & (1 << i) != 0) << 1);
                }
                if dirty { *g.tables.get_mut(&0x4030).unwrap() |= 1 << 6; }
                // State owned by hardware VMRUN/VMEXIT must survive emulation.
                for (offset, value) in [(0x5e0, 1), (0x5e8, 0x1234_5000), (0x5f0, 0x2345_6000)] {
                    put(&mut g.vmcb, offset, value);
                }
                let before = *g.vmcb.bytes();
                let frame = g.frame;
                let tables = g.tables.clone();
                let permitted = !write || writable == 15;
                let mut called = false;
                let result = g.run(|vmcb, offset, value| {
                    assert!(permitted);
                    assert_eq!(vmcb.bytes(), &before);
                    assert_eq!((offset, value), (0x300, write.then_some(0x4600)));
                    called = true;
                    Ok(0x8000_0011)
                });
                assert_eq!(called, permitted);
                assert_eq!(g.tables, tables, "emulation must not rewrite guest PTEs");
                assert_eq!(g.frame, frame);
                if permitted {
                    result.unwrap();
                    assert_eq!(g.vmcb.guest_rip(), 0x4002);
                    assert_eq!(&g.vmcb.bytes()[0x5e0..0x5f8], &before[0x5e0..0x5f8]);
                    assert_eq!(g.vmcb.guest_rax(), if write { 0xabcd_1234_0000_4600 } else { 0x8000_0011 });
                } else {
                    assert_eq!(result, Err(NativeMmioError::Operand));
                    assert_eq!(g.vmcb.bytes(), &before);
                }
            }
        }
    }
}

#[test]
fn cet_without_wp_is_refused_before_any_guest_or_device_read() {
    use svmvisor_hypervisor::svm::xapic::handle_native_mmio_detailed;
    for write in [false, true] {
        let mut g = Guest::new(&[if write { 0x89 } else { 0x8b }, 3], write);
        put(&mut g.vmcb, 0x548, 0x20 | (1 << 23));
        put(&mut g.vmcb, 0x558, 0x8000_0001);
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        let failure = handle_native_mmio_detailed(&mut g.vmcb, &mut g.frame, 48, g.pat,
            0xfee0_0000, |_, _| panic!("inconsistent CET read guest memory"),
            |_, _, _| panic!("inconsistent CET reached APIC")).unwrap_err();
        assert_eq!(failure.error, NativeMmioError::UnsupportedMode);
        assert_eq!((failure.predicate, failure.operand), (10, 0x800020));
        assert_eq!(g.vmcb.bytes(), &before);
        assert_eq!(g.frame, frame);
    }
}

#[test]
fn cet_shadow_marked_instruction_page_still_obeys_nx() {
    for nx in [false, true] {
        let mut g = Guest::new(&[0x8b, 3], false);
        put(&mut g.vmcb, 0x548, 0x20 | (1 << 23));
        g.tables.insert(0x4020, 0x9041 | if nx { 1 << 63 } else { 0 });
        let before = *g.vmcb.bytes();
        let frame = g.frame;
        let result = g.run(|_, _, _| { assert!(!nx); Ok(0) });
        if nx {
            assert_eq!(result, Err(NativeMmioError::Fetch(FetchError::NotExecutable)));
            assert_eq!(g.vmcb.bytes(), &before);
            assert_eq!(g.frame, frame);
        } else {
            result.unwrap();
            assert_eq!(g.vmcb.guest_rip(), 0x4002);
        }
    }
}

#[test]
fn detailed_fetch_distinguishes_instruction_and_operand_and_retains_physical_failure_address() {
    use svmvisor_hypervisor::svm::xapic::handle_native_mmio_detailed;
    use svmvisor_hypervisor::host::resident::terminal::apic_failure;
    for data in [false,true] {
        let mut g=Guest::new(&[0x8b,3],false);
        if data { g.tables.remove(&0x4030); } else { g.code.clear(); }
        let failure=handle_native_mmio_detailed(&mut g.vmcb,&mut g.frame,48,g.pat,0xfee00000,
            |address,width| if width==8 {g.tables.get(&address).copied()} else {g.code.get(&address).map(|b|*b as u64)},
            |_,_,_|panic!()).unwrap_err();
        let (tag,value)=apic_failure(failure);
        assert_eq!((tag>>16,value),if data {(0x654,0x4030)} else {(0x408,0x9000)});
    }
}