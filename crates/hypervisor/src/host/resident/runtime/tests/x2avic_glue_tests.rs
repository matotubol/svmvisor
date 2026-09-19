extern crate std;

use core::{
    cell::Cell,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    arch::x86_64::{
        apic::{self, PhysicalX2Apic},
        registers::GuestRegisters,
    },
    host::resident::{
        runtime::{
            INITIAL_STATE, IRQ_GATES, State,
            arm::{backing_alias, backing_alias_pte, window_gate},
            avic::{
                AvicPlan, IncompleteIpi, avic_exit_plan, clear_icr_delivery_status,
                incomplete_ipi_plan,
            },
            exit::{ExitOrder, exit_order, sequence_exit},
            irq::{capture_accepted, sync_eoi_intercept},
            msr::{MsrCompletion, apply_msr_completion, msr_completion},
            startup::{
                InitOwners, NMI_DRAIN_MISS_LIMIT, NMI_DRAIN_STALL, StartupStep, nmi_drain_stalled,
                startup_step,
            },
            stop::stop_counters,
        },
        terminal::{self, StartupStage},
    },
    memory::address::{AddressPolicy, EncryptionState},
    svm::{
        cache::{CacheCore, CacheCoreState, CacheObservation},
        dispatch::NativeEfer,
        events::ExternalInterruptError,
        exit::{ExitSnapshot, MsrInstruction},
        permission_maps::Msrpm,
        vmcb::Vmcb,
        x2avic::{
            BackingPage, GUEST_APIC_VERSION, NativeX2AvicProfile, X2AvicCapabilities,
            ipi::{IpiDrop, IpiRefusal},
            irq::{self, Capture, IrqError, PhysicalIrqLedger},
            registers::{Emulation, GuestX2Apic, InitError, Refusal},
            startup::{
                NativeDestinationCause, NativeIcr, NativeStartupCommand, NativeStartupEffect,
                NativeStartupMailbox, NativeStartupState, try_lock_routes,
            },
        },
    },
};

fn exit(code: u64, info1: u64, info2: u64) -> ExitSnapshot {
    ExitSnapshot { code, info1, info2, rip: 0x1000, nrip: 0 }
}

fn put(vmcb: &mut Vmcb, offset: usize, value: u64) {
    assert!(offset + 8 <= 4096);
    // SAFETY: in-bounds write into the 4 KiB VMCB byte image.
    unsafe {
        ptr::write_unaligned((vmcb as *mut Vmcb).cast::<u8>().add(offset).cast::<u64>(), value)
    };
}

fn get(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}

fn policy() -> AddressPolicy {
    AddressPolicy::new(48, EncryptionState::Unencrypted { encryption_bit: None }).unwrap()
}

fn eoi_intercepted(msrpm: &Msrpm) -> bool {
    let bit = 2 * 0x80b + 1;
    msrpm.bytes()[bit / 8] & (1 << (bit % 8)) != 0
}

#[test]
fn undrained_nmi_exits_stop_only_when_consecutive() {
    let mut misses = 0;
    // One miss, or two, never stops; a drained 61h exit resets the count.
    assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
    assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
    assert_eq!(misses, 2);
    assert!(!nmi_drain_stalled(&mut misses, 0x61, true));
    assert_eq!(misses, 0);
    // Any other exit is guest progress and resets it, drained or not.
    for (code, drained) in [(0x60, false), (0x72, true), (0x400, false)] {
        assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
        assert!(!nmi_drain_stalled(&mut misses, 0x61, false));
        assert!(!nmi_drain_stalled(&mut misses, code, drained));
        assert_eq!(misses, 0);
    }
    // The third consecutive miss stops, and the count then saturates.
    for expected in [false, false, true, true] {
        assert_eq!(nmi_drain_stalled(&mut misses, 0x61, false), expected);
    }
    assert_eq!(misses, 4);
    misses = u8::MAX;
    assert!(nmi_drain_stalled(&mut misses, 0x61, false));
    assert_eq!(misses, u8::MAX);
    assert_eq!(NMI_DRAIN_MISS_LIMIT, 3);
    // A unique tag that exports as an unhandled exit 61h with its RIP.
    assert_eq!(NMI_DRAIN_STALL, 0xf113);
    let words = terminal::stop_words(5, 0x61, 0xffff_f800_0000_2000, NMI_DRAIN_STALL, 3).unwrap();
    assert_eq!(
        ((words[0] >> 24) & 15, (words[0] >> 13) & 0x7ff, (words[0] >> 8) & 31, words[1], words[2]),
        (0, 0x61, 5, 0x2000, 0xffff_f800)
    );
}

#[test]
fn register_outcomes_complete_fault_or_stop_with_typed_evidence() {
    assert_eq!(
        msr_completion(Emulation::Read(0x1234_5678_9abc_def0), 0x809, false),
        MsrCompletion::Complete { read: Some(0x1234_5678_9abc_def0) }
    );
    assert_eq!(
        msr_completion(Emulation::Written, 0x80b, true),
        MsrCompletion::Complete { read: None }
    );
    assert_eq!(msr_completion(Emulation::GeneralProtection, 0x802, true), MsrCompletion::Fault);
    // APIC_BASE disable: reason 5, MSR 1Bh, WRMSR, the requested value.
    let refused = Emulation::Refused { reason: Refusal::ApicDisable, value: 0xfee0_0000 };
    assert_eq!(
        msr_completion(refused, 0x1b, true),
        MsrCompletion::Stop(0xf545 | (0x1b << 16) | (1 << 48), 0xfee0_0000)
    );
    let refused = Emulation::Refused { reason: Refusal::UnownedAccess, value: 0 };
    assert_eq!(
        msr_completion(refused, 0x830, false),
        MsrCompletion::Stop(0xf541 | (0x830 << 16), 0)
    );
    let refused = Emulation::Refused { reason: Refusal::ExceptionVector, value: 0x11 };
    assert_eq!(
        msr_completion(refused, 0x832, true),
        MsrCompletion::Stop(0xf547 | (0x832 << 16) | (1 << 48), 0x11)
    );
    let failed =
        Emulation::EoiFailed(IrqError::PhysicalIsrMismatch { vector: 0x40, highest: None });
    assert_eq!(
        msr_completion(failed, 0x80b, true),
        MsrCompletion::Stop(0xf572 | (1 << 16), (2 << 17) | (0x100 << 8) | 0x40)
    );
}

#[test]
fn msr_completions_continue_fault_or_stop_the_stopped_guest() {
    let mut f = Fixture::new();
    let armed = |vmcb: &mut Vmcb| {
        put(vmcb, 0x578, 0x1000); // RIP
        put(vmcb, 0x5f8, 0xdead_beef_0000_0001); // RAX
        put(vmcb, 0x570, (1 << 16) | 2); // RFLAGS with RF
        put(vmcb, 0x068, 1); // interrupt shadow
        put(vmcb, 0xc0, u64::from(u32::MAX)); // clean bits
    };
    armed(&mut f.vmcb);
    let rdmsr = exit(0x7c, 0, 0);
    let next = MsrInstruction::Bytes(&[0x0f, 0x32]).continuation(rdmsr).unwrap();
    // RDMSR: EDX:EAX loaded with zero-extended halves, then nRIP.
    let read = MsrCompletion::Complete { read: Some(0x1234_5678_9abc_def0) };
    assert_eq!(apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, read, next), Ok(false));
    assert_eq!((f.vmcb.guest_rax(), f.frame.rdx, f.frame.rcx), (0x9abc_def0, 0x1234_5678, 7));
    assert_eq!((f.vmcb.guest_rip(), get(&f.vmcb, 0x570), get(&f.vmcb, 0x068) & 1), (0x1002, 2, 0));
    assert_eq!(f.vmcb.bytes()[0xc0..0xc4], [0; 4]);
    // WRMSR: RAX and RDX keep the written value.
    armed(&mut f.vmcb);
    let wrmsr = exit(0x7c, 1, 0);
    let next = MsrInstruction::Bytes(&[0x0f, 0x30]).continuation(wrmsr).unwrap();
    let written = MsrCompletion::Complete { read: None };
    assert_eq!(
        apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, written, next),
        Ok(false)
    );
    assert_eq!((f.vmcb.guest_rax(), f.frame.rdx), (0xdead_beef_0000_0001, 0x1234_5678));
    assert_eq!(f.vmcb.guest_rip(), 0x1002);
    // #GP(0) at the unchanged RIP.
    armed(&mut f.vmcb);
    assert_eq!(
        apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, MsrCompletion::Fault, next),
        Ok(true)
    );
    assert_eq!(
        (get(&f.vmcb, 0xa8), f.vmcb.guest_rip(), get(&f.vmcb, 0x570)),
        (0x8000_0b0d, 0x1000, 0x1_0002)
    );
    // A fault that cannot be queued, and a stop, change nothing.
    let (vmcb, frame) = (*f.vmcb.bytes(), f.frame);
    assert_eq!(
        apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, MsrCompletion::Fault, next),
        Err((0xf510, 4))
    );
    let stop = MsrCompletion::Stop(0xf541 | (0x830 << 16), 0);
    assert_eq!(
        apply_msr_completion(&mut f.vmcb, &mut f.frame, &f.profile, stop, next),
        Err((0xf541 | (0x830 << 16), 0))
    );
    assert_eq!((*f.vmcb.bytes(), f.frame), (vmcb, frame));
}

#[test]
fn incomplete_ipi_tolerates_only_the_delivery_status_bit() {
    let owner = NativeIcr::admit(0x10, &[0x10, 0x11, 0x12]).unwrap();
    let inventory = owner.inventory();
    let busy = apic::ICR_DELIVERY_STATUS;
    let edge = 0x0000_0012_0000_00ef;
    for reason in [0u64, 2] {
        for icr in [edge, edge | busy] {
            let plan =
                incomplete_ipi_plan(inventory, exit(0x401, icr, reason << 32), apic::ICR_MSR);
            assert!(
                matches!(plan, IncompleteIpi::Fixed(ipi)
                if ipi.vector() == 0xef && ipi.targets() == 1 << 2),
                "{icr:#x} {reason}"
            );
        }
    }
    // INIT/SIPI are routed with bit 12 clear, for IDs 0, 2 and 4.
    let init = 0x0000_0011_0000_0500;
    for reason in [0u64, 2, 4] {
        assert_eq!(
            incomplete_ipi_plan(inventory, exit(0x401, init | busy, reason << 32), apic::ICR_MSR),
            IncompleteIpi::Startup(init)
        );
    }
    // Every other reserved bit still refuses; the raw EXITINFO1 survives.
    for bit in [13, 16, 17, 20, 31] {
        let icr = edge | busy | (1 << bit);
        assert_eq!(
            incomplete_ipi_plan(inventory, exit(0x401, icr, 0), apic::ICR_MSR),
            IncompleteIpi::Stop(0xf550 | IpiRefusal::ReservedBits as u64, icr)
        );
    }
    // ID 1: hardware published the fixed edge IPI; resume, nothing to do.
    for icr in [edge, edge | busy] {
        assert_eq!(
            incomplete_ipi_plan(inventory, exit(0x401, icr, (1 << 32) | 0xfff_f012), apic::ICR_MSR),
            IncompleteIpi::Published
        );
    }
    // An ID 1 that hardware cannot have published keeps its ID and table
    // index as detail.
    assert_eq!(
        incomplete_ipi_plan(inventory, exit(0x401, init, (1 << 32) | 0xfff_f012), apic::ICR_MSR),
        IncompleteIpi::Stop(0xf551 | (0x1012 << 16), init)
    );
    assert_eq!(
        incomplete_ipi_plan(inventory, exit(0x401, 0x0000_0012_0000_0005, 4 << 32), apic::ICR_MSR),
        IncompleteIpi::Dropped(IpiDrop::IllegalVector)
    );
    assert_eq!(
        incomplete_ipi_plan(
            inventory,
            exit(0x401, 0x0000_0020_0000_00ef | busy, 2 << 32),
            apic::ICR_MSR
        ),
        IncompleteIpi::Dropped(IpiDrop::NoTarget)
    );
}

#[test]
fn self_ipi_exits_never_name_another_cpu() {
    // Source 12h is slot 2 and slot 0 is ID 0. EXITINFO1 of a SELF IPI
    // write may be the bare vector or the to-self ICR (16.15 p663).
    let owner = NativeIcr::admit(0x12, &[0, 0x11, 0x12]).unwrap();
    let inventory = owner.inventory();
    for info1 in [0xefu64, 0x0004_00ef, 0x0000_0011_0004_00ef] {
        for reason in [0u64, 2] {
            let plan = incomplete_ipi_plan(
                inventory,
                exit(0x401, info1, reason << 32),
                apic::SELF_IPI_MSR,
            );
            assert!(
                matches!(plan, IncompleteIpi::Fixed(ipi)
                if ipi.vector() == 0xef && ipi.targets() == 1 << 2),
                "{info1:#x} {reason}"
            );
        }
    }
    assert_eq!(
        incomplete_ipi_plan(inventory, exit(0x401, 0x05, 4 << 32), apic::SELF_IPI_MSR),
        IncompleteIpi::Dropped(IpiDrop::IllegalVector)
    );
    // The same bare value written to the ICR is a physical IPI to ID 0.
    let plan = incomplete_ipi_plan(inventory, exit(0x401, 0xef, 0), apic::ICR_MSR);
    assert!(matches!(plan, IncompleteIpi::Fixed(ipi) if ipi.targets() == 1));
}

#[test]
fn handled_incomplete_ipi_clears_only_the_backing_delivery_status() {
    let page = BackingPage::new();
    page.write_register_stopped(apic::ICR, 0x000c_14ef).unwrap();
    page.write_register_stopped(apic::ICR_HIGH, 0x12).unwrap();
    clear_icr_delivery_status(&page);
    assert_eq!(page.read_register(apic::ICR), Ok(0x000c_04ef));
    assert_eq!(page.read_register(apic::ICR_HIGH), Ok(0x12));
    clear_icr_delivery_status(&page);
    assert_eq!(page.read_register(apic::ICR), Ok(0x000c_04ef));
}

#[test]
fn avic_exits_outside_the_level_eoi_fallback_stop_with_raw_exit_information() {
    let eoi = (1u64 << 32) | 0xb0;
    assert_eq!(avic_exit_plan(exit(0x402, eoi, 0x61)), AvicPlan::LevelEoi(0x61));
    assert_eq!(avic_exit_plan(exit(0x401, 0x4ef, 2 << 32)), AvicPlan::IncompleteIpi);
    // Timer LVT write, APR read, EOI read, divide write: D1 intercepts all.
    for (info1, info2) in [
        ((1u64 << 32) | 0x320, 0xdead_beef_0000_0001),
        (0x90, 0),
        (0xb0, 0x61),
        ((1 << 32) | 0x3e0, 7),
    ] {
        assert_eq!(
            avic_exit_plan(exit(0x402, info1, info2)),
            AvicPlan::Stop(0xf580 | ((info2 & 0xffff_ffff) << 16), info1)
        );
    }
    // An EOI vector below 16 or an ID above 4 does not decode.
    assert_eq!(avic_exit_plan(exit(0x402, eoi, 0x0f)), AvicPlan::Stop(0xf581 | (0x0f << 16), eoi));
    assert_eq!(avic_exit_plan(exit(0x401, 0x4ef, 5 << 32)), AvicPlan::Stop(0xf581, 0x4ef));
}

#[test]
fn traps_are_handled_before_the_startup_service_and_instructions_after() {
    for code in [0x60, 0x401, 0x402] {
        assert_eq!(exit_order(code), ExitOrder::TrapThenStartup, "{code:#x}");
    }
    for code in [0x63, 0x72, 0x77, 0x7b, 0x7c, 0x81, 0x400, u64::MAX] {
        assert_eq!(exit_order(code), ExitOrder::StartupThenExit, "{code:#x}");
    }
    type Log = ([u8; 2], usize);
    let run = |order: ExitOrder, handled: bool, serviced: Option<bool>| {
        let mut log: Log = ([0; 2], 0);
        let resume = sequence_exit(
            &mut log,
            order,
            |log| {
                log.0[log.1] = b'h';
                log.1 += 1;
                handled
            },
            |log| {
                log.0[log.1] = b's';
                log.1 += 1;
                serviced
            },
        );
        (log, resume)
    };
    use ExitOrder::{StartupThenExit as Instruction, TrapThenStartup as Trap};
    // A trap's effect is complete before any INIT/SIPI; a stopped trap
    // services nothing.
    assert_eq!(run(Trap, true, None), ((*b"hs", 2), true));
    assert_eq!(run(Trap, true, Some(true)), ((*b"hs", 2), true));
    assert_eq!(run(Trap, true, Some(false)), ((*b"hs", 2), false));
    assert_eq!(run(Trap, false, Some(true)), ((*b"h\0", 1), false));
    // An instruction runs only if no startup command changed the guest.
    assert_eq!(run(Instruction, true, None), ((*b"sh", 2), true));
    assert_eq!(run(Instruction, false, None), ((*b"sh", 2), false));
    assert_eq!(run(Instruction, false, Some(true)), ((*b"s\0", 1), true));
    assert_eq!(run(Instruction, true, Some(false)), ((*b"s\0", 1), false));
}

#[test]
fn window_gates_cover_16_to_255_except_mc_and_sx() {
    assert_eq!(IRQ_GATES, 256 - 16);
    for vector in 0..=256 {
        assert_eq!(
            window_gate(vector),
            (16..256).contains(&vector) && vector != 18 && vector != 30,
            "{vector}"
        );
    }
}

#[test]
fn stop_records_pack_saturated_drop_and_discard_counts() {
    let mut state = INITIAL_STATE;
    assert_eq!(stop_counters(&state), 0);
    (state.ipi_drops, state.irq_discards) = (7, 3);
    assert_eq!(stop_counters(&state), (3 << 32) | 7);
    (state.ipi_drops, state.irq_discards) = (u64::MAX, 1 << 40);
    assert_eq!(stop_counters(&state), u64::MAX);
}

#[test]
fn backing_aliases_map_every_slot_page_below_the_shared_table() {
    use crate::host::resident::{X2AVIC_BACKING_ALIASES_OFFSET, X2AVIC_TABLE_OFFSET};
    // Slot 1 of a 2 MiB-aligned pool; the backing page sits 34000h into
    // every image.
    let (pool, base, offset) = (0x2000_0000u64, 0x2010_0000u64, 0x3_4000u64);
    for slot in 0..32u64 {
        let alias = backing_alias(base, slot);
        assert_eq!(alias, base + X2AVIC_BACKING_ALIASES_OFFSET + slot * 4096);
        // Inside the image's own last-level table, below every shared page.
        assert!(alias + 4096 <= base + X2AVIC_TABLE_OFFSET && alias >> 21 == base >> 21, "{slot}");
        let pte = backing_alias_pte(pool, slot, offset);
        // Slot s's image is s MiB into the pool: its directory's
        // `avic_backing`, which DXE publishes in the table.
        assert_eq!(pte & 0x000f_ffff_ffff_f000, pool + slot * 0x10_0000 + offset, "{slot}");
        assert_eq!(pte & !0x000f_ffff_ffff_f000, (1 << 63) | 3, "RW/NX present");
    }
    assert_eq!(backing_alias(base, 31) + 4096, base + X2AVIC_TABLE_OFFSET);
}

/// Physical x2APIC model over MSRs 800h-8FFh. An EOI clears the highest
/// ISR bit (APM2 16.6.4 p652) unless `broken_eoi` is set.
struct Apic<'a> {
    registers: &'a mut [u64; 256],
    writes: &'a Cell<usize>,
    broken_eoi: bool,
}

impl PhysicalX2Apic for Apic<'_> {
    fn read(&mut self, msr: u32) -> u64 {
        self.registers[(msr - 0x800) as usize]
    }

    fn write(&mut self, msr: u32, value: u64) {
        self.writes.set(self.writes.get() + 1);
        if msr != 0x80b {
            self.registers[(msr - 0x800) as usize] = value;
        } else if let Some(bank) = (0x10..0x18).rev().find(|&bank| self.registers[bank] != 0)
            && !self.broken_eoi
        {
            let bits = &mut self.registers[bank];
            *bits &= !(1 << (63 - bits.leading_zeros()));
        }
    }
}

#[test]
fn level_eoi_exit_intercepts_eoi_writes_until_the_next_one() {
    // A stale TMR bit with nothing held: the AVIC_NOACCEL fallback. The
    // ISR bit is still set, so the WRMSR at 1000h may run again (Table
    // 15-22 p566 trap, 15.29.9.2 p581 fault).
    let mut page = BackingPage::new();
    page.reset_stopped(1, GUEST_APIC_VERSION).unwrap();
    for base in [apic::ISR, apic::TMR] {
        page.write_register_stopped(base + 0x20, 1).unwrap(); // 40h
    }
    page.write_register_stopped(apic::ISR + 0x10, 1).unwrap(); // 20h
    let (mut ledger, mut msrpm, mut vmcb) =
        (PhysicalIrqLedger::new(), Msrpm::native_boot(), Vmcb::new());
    msrpm.configure_native_x2avic();
    let (mut registers, writes) = ([0u64; 256], Cell::new(0));
    let mut host = Apic { registers: &mut registers, writes: &writes, broken_eoi: false };
    irq::level_eoi_exit(0x40, 0x1000, &page, &mut ledger, &mut host).unwrap();
    sync_eoi_intercept(&ledger, &mut msrpm, &mut vmcb);
    assert!(!page.is_in_service(0x40) && page.is_in_service(0x20) && eoi_intercepted(&msrpm));
    // The re-executed write is recognized once and leaves 20h in service.
    assert!(ledger.take_eoi_replay(0x1000) && !ledger.take_eoi_replay(0x1000));
    sync_eoi_intercept(&ledger, &mut msrpm, &mut vmcb);
    assert!(page.is_in_service(0x20) && !eoi_intercepted(&msrpm) && writes.get() == 0);
}

#[test]
fn accepted_vectors_publish_and_resynchronize_the_eoi_intercept() {
    let mut page = BackingPage::new();
    page.reset_stopped(1, GUEST_APIC_VERSION).unwrap();
    page.write_register_stopped(apic::SVR, 0x1ff).unwrap();
    let (mut ledger, mut msrpm, mut vmcb) =
        (PhysicalIrqLedger::new(), Msrpm::native_boot(), Vmcb::new());
    msrpm.configure_native_x2avic();
    let mut registers = [0u64; 256];
    registers[0x0f] = 0x1ff; // host-owned physical SVR
    let writes = Cell::new(0);
    let capture = |vector: u32,
                   registers: &mut [u64; 256],
                   page: &BackingPage,
                   ledger: &mut PhysicalIrqLedger,
                   msrpm: &mut Msrpm,
                   vmcb: &mut Vmcb| {
        put(vmcb, 0xc0, u64::from(u32::MAX));
        let mut apic = Apic { registers, writes: &writes, broken_eoi: false };
        capture_accepted(vector, page, ledger, &mut apic, msrpm, vmcb)
    };
    let clean = |vmcb: &Vmcb| get(vmcb, 0xc0) as u32;
    // Level 40h: published with TMR, held, EOI now intercepted, clean
    // bits cleared, no physical EOI.
    (registers[0x12], registers[0x1a]) = (1, 1);
    assert_eq!(
        capture(0x40, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
        Ok(Some(Capture::Level))
    );
    assert!(page.is_pending(0x40) && page.is_level(0x40) && ledger.holds(0x40));
    assert!(eoi_intercepted(&msrpm) && clean(&vmcb) == 0 && writes.get() == 0);
    // Edge 50h above it: published and acknowledged; the intercept and
    // the clean bits stay.
    registers[0x12] |= 1 << 16;
    assert_eq!(
        capture(0x50, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
        Ok(Some(Capture::Edge))
    );
    assert!(page.is_pending(0x50) && !page.is_level(0x50));
    assert_eq!((registers[0x12], writes.get(), clean(&vmcb)), (1, 1, u32::MAX));
    assert!(eoi_intercepted(&msrpm));
    // Software-disabled guest APIC: an edge source is only acknowledged,
    // a level source is still published and held.
    page.write_register_stopped(apic::SVR, 0xff).unwrap();
    registers[0x12] |= 1 << 1;
    assert_eq!(
        capture(0x41, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
        Ok(Some(Capture::Discarded))
    );
    assert!(!page.is_pending(0x41));
    assert_eq!((registers[0x12], writes.get()), (1, 2));
    (registers[0x13], registers[0x1b]) = (1, 1);
    assert_eq!(
        capture(0x60, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
        Ok(Some(Capture::Level))
    );
    assert!(page.is_pending(0x60) && ledger.holds(0x60) && writes.get() == 2);
    // The host spurious vector needs nothing.
    assert_eq!(capture(0xff, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb), Ok(None));
    // Vectors 16-31 reach the bridge through the window gates and stop
    // with the vector; a helper result above 255 stops as well.
    registers[0x10] = 1 << 17;
    assert_eq!(
        capture(17, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
        Err((0xf571, (1 << 17) | (0x100 << 8) | 17))
    );
    assert_eq!(
        capture(0x100, &mut registers, &page, &mut ledger, &mut msrpm, &mut vmcb),
        Err((0xf500, 0x100))
    );
    assert_eq!(writes.get(), 2);
}

/// Guest 1 of two, INIT pending: level source 40h held and physically in
/// service, a busy guest register state, EOI writes intercepted.
struct Fixture {
    profile: NativeX2AvicProfile,
    vmcb: Vmcb,
    frame: GuestRegisters,
    state: State,
    page: BackingPage,
    msrpm: Msrpm,
    registers: [u64; 256],
}

fn mailboxes() -> [NativeStartupMailbox; 2] {
    let mailboxes = [NativeStartupMailbox::new(0), NativeStartupMailbox::new(1)];
    for mailbox in &mailboxes {
        mailbox.mark_running();
    }
    mailboxes
}

fn cache_core(phase: u32) -> CacheCore {
    CacheCore::new(CacheCoreState {
        bank: CacheObservation::EMPTY,
        members: 0b11,
        entering: 0,
        leaving: 0,
        departed: 0,
        phase,
        generation: 0,
    })
}

/// The destination history of slot 1, as a refused route to its filled
/// queue reports it: (cause, INIT count). Leaves that queue full.
fn history(mailboxes: &[NativeStartupMailbox; 2]) -> (NativeDestinationCause, u32) {
    while mailboxes[1].publish(NativeStartupCommand::Sipi(9)).is_ok() {}
    let mut source = NativeIcr::admit(0, &[0, 1]).unwrap();
    assert!(source.route_x2avic_startup(0x0000_0001_0000_0500, mailboxes, |_| {}).is_err());
    let recipient = source.route_failure().unwrap().recipient.unwrap();
    (recipient.cause, recipient.init_count)
}

impl Fixture {
    fn new() -> Self {
        let capabilities =
            X2AvicCapabilities::admit(1 << 21, 1 | (1 << 13) | (1 << 18) | (1 << 25)).unwrap();
        let profile = NativeX2AvicProfile::new(capabilities, 0x2000, 0x3000, 1, &policy()).unwrap();
        let mut vmcb = Vmcb::new();
        // NP_ENABLE, as native preparation leaves it (Table B-1 090h).
        put(&mut vmcb, 0x90, 1);
        vmcb.set_virtual_interrupt_tpr(6).unwrap();
        vmcb.enable_native_x2avic(&profile).unwrap();
        let mut efer = NativeEfer::admit(0xd01, true).unwrap();
        efer.enable_guest_startup();
        let mut state = INITIAL_STATE;
        state.efer = Some(efer);
        state.guest_apic = Some(GuestX2Apic::admit(0xfee0_0c00, &policy()).unwrap());
        state.slot = 1;
        state.count = 2;
        state.irq.commit_level_capture(0x40).unwrap();
        let mut page = BackingPage::new();
        page.reset_stopped(1, GUEST_APIC_VERSION).unwrap();
        page.write_register_stopped(apic::TPR, 0x6b).unwrap();
        page.write_register_stopped(apic::LVT_TIMER, 0x2_00ef).unwrap();
        page.enqueue(0x40, true).unwrap();
        let mut msrpm = Msrpm::native_boot();
        msrpm.configure_native_x2avic();
        assert!(msrpm.update_x2apic_eoi_intercept(&state.irq));
        let mut registers = [0; 256];
        registers[0x0f] = 0x1ff; // host-owned physical SVR
        registers[0x12] = 1; // physical ISR 40h
        registers[0x32] = 0x2_00ef;
        registers[0x38] = 5000;
        let frame = GuestRegisters { rcx: 7, rdx: 9, ..GuestRegisters::default() };
        Self { profile, vmcb, frame, state, page, msrpm, registers }
    }

    /// `startup_step` for the command at the head of slot 1's queue.
    fn step(
        &mut self,
        mailboxes: &[NativeStartupMailbox; 2],
        writes: &Cell<usize>,
        broken_eoi: bool,
        core: Option<&CacheCore>,
        reset: impl FnOnce(),
    ) -> StartupStep {
        let command = mailboxes[1].peek().unwrap();
        let owners = InitOwners {
            backing: &self.page,
            physical: Apic { registers: &mut self.registers, writes, broken_eoi },
            msrpm: &mut self.msrpm,
            signature: 0x00b4_0f40,
        };
        startup_step(
            &mut self.state,
            &mut self.vmcb,
            &mut self.frame,
            mailboxes,
            &self.profile,
            command,
            core,
            owners,
            reset,
        )
    }

    fn backing(&self) -> [u32; 256] {
        core::array::from_fn(|index| self.page.read_register(index as u16 * 16).unwrap())
    }

    fn snapshot(&self) -> impl PartialEq + core::fmt::Debug + use<> {
        (
            *self.vmcb.bytes(),
            self.backing(),
            self.registers,
            self.state.irq,
            self.state.efer,
            self.state.guest_apic,
            *self.msrpm.bytes(),
            self.frame,
            self.state.startup,
        )
    }
}

#[test]
fn guest_init_commits_d9_then_records_and_completes_under_the_route_lease() {
    let mut f = Fixture::new();
    let boxes = mailboxes();
    boxes[1].publish(NativeStartupCommand::Init).unwrap();
    // The guest enabled and then disabled its APIC with 40h pending, so
    // its APIC owner records 40h as held.
    {
        let unused = Cell::new(0);
        let mut apic = Apic { registers: &mut f.registers, writes: &unused, broken_eoi: false };
        let guest = f.state.guest_apic.as_mut().unwrap();
        for svr in [0x1ff, 0xff] {
            assert_eq!(
                guest.emulate(0x80f, Some(svr), &f.page, &mut f.state.irq, &mut apic),
                Emulation::Written
            );
        }
    }
    assert_ne!(f.state.guest_apic, Some(GuestX2Apic::admit(0xfee0_0c00, &policy()).unwrap()));
    let core = cache_core(0);
    let (writes, resets) = (Cell::new(0), Cell::new(0));
    let result = f.step(&boxes, &writes, false, Some(&core), || {
        // Steps 1-4 precede the debug reset: eight register resets and
        // the physical EOI of the retired source. The whole commit holds
        // the cache lease and not the route lease.
        assert_eq!(writes.get(), 9);
        assert!(core.try_lock().is_none());
        assert!(try_lock_routes(&boxes).is_ok());
        assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
        resets.set(resets.get() + 1);
    });
    assert_eq!(result, StartupStep::Applied(NativeStartupEffect::Init));
    assert_eq!(resets.get(), 1);
    // Both leases are free again and the command left the queue.
    assert!(core.try_lock().is_some() && try_lock_routes(&boxes).is_ok());
    assert_eq!(boxes[1].peek(), None);
    // Physical LAPIC: timer stopped, every LVT masked, source retired.
    for msr in 0x832..=0x837 {
        assert_eq!(f.registers[msr - 0x800], 0x1_0000, "{msr:#x}");
    }
    assert_eq!((f.registers[0x38], f.registers[0x3e], f.registers[0x12]), (0, 0, 0));
    assert_eq!(f.registers[0x0f], 0x1ff);
    assert!(f.state.irq.is_empty() && !eoi_intercepted(&f.msrpm));
    // Backing page: Table 16-2 values, ID kept, LDR derived for ID 1.
    let backing = f.backing();
    assert_eq!(backing[(apic::TPR / 16) as usize], 0);
    assert_eq!(backing[(apic::SVR / 16) as usize], 0xff);
    assert_eq!(backing[(apic::LVT_TIMER / 16) as usize], 0x1_0000);
    assert_eq!(backing[(apic::LDR / 16) as usize], 1 << 1);
    assert!(!f.page.is_pending(0x40) && !f.page.is_level(0x40));
    // CPU INIT state with V_TPR 0 and every clean bit clear.
    assert_eq!(f.vmcb.guest_rip(), 0xfff0);
    assert_eq!(f.vmcb.virtual_interrupt_control(), crate::svm::x2avic::NATIVE_CONTROL);
    assert_eq!(f.vmcb.bytes()[0xc0..0xc4], [0; 4]);
    assert!(f.vmcb.validate_native_x2avic(&f.profile).is_ok());
    assert_eq!((f.frame.rcx, f.frame.rdx), (0, 0x00b4_0f40));
    assert_eq!(f.state.startup, NativeStartupState::AwaitSipi);
    assert_eq!(f.state.efer.map(|efer| efer.logical()), Some(0));
    // The reset page holds nothing; APIC_BASE is unchanged (16.10 p657).
    assert_eq!(f.state.guest_apic, Some(GuestX2Apic::admit(0xfee0_0c00, &policy()).unwrap()));
    // The destination record names one guest INIT.
    assert_eq!(history(&boxes), (NativeDestinationCause::GuestInit, 1));
}

#[test]
fn init_then_sipi_starts_the_guest_once() {
    let mut f = Fixture::new();
    let boxes = mailboxes();
    for command in [
        NativeStartupCommand::Init,
        NativeStartupCommand::Sipi(0x9a),
        NativeStartupCommand::Sipi(0x9b),
    ] {
        boxes[1].publish(command).unwrap();
    }
    let writes = Cell::new(0);
    assert_eq!(
        f.step(&boxes, &writes, false, None, || {}),
        StartupStep::Applied(NativeStartupEffect::Init)
    );
    assert_eq!(f.state.startup, NativeStartupState::AwaitSipi);
    // SIPI 9Ah: real-mode CS 9A00h based at 9A000h, IP 0 (APM2 15.27.8).
    assert_eq!(
        f.step(&boxes, &writes, false, None, || panic!("SIPI resets no debug state")),
        StartupStep::Applied(NativeStartupEffect::Started)
    );
    assert_eq!(f.state.startup, NativeStartupState::Running);
    assert_eq!(
        (get(&f.vmcb, 0x410) as u16, get(&f.vmcb, 0x418), f.vmcb.guest_rip()),
        (0x9a00, 0x9a000, 0)
    );
    // One start per INIT: the next SIPI is completed without an effect.
    let before = f.snapshot();
    assert_eq!(
        f.step(&boxes, &writes, false, None, || panic!("ignored SIPI")),
        StartupStep::Applied(NativeStartupEffect::Ignored)
    );
    assert_eq!(f.snapshot(), before);
    assert_eq!(boxes[1].peek(), None);
    assert_eq!(history(&boxes), (NativeDestinationCause::GuestInit, 1));
}

#[test]
fn refused_startup_commands_change_nothing() {
    type Case = (fn(&mut Fixture), Option<u32>, StartupStep);
    let foreign = InitError::Irq(IrqError::UnexpectedPhysicalIsr(0x70));
    let cases: [Case; 7] = [
        // Physical ISR 70h (bank 3, bit 16) is not a held source.
        (
            |f| f.registers[0x13] = 1 << 16,
            None,
            StartupStep::Failed(
                StartupStage::InitPreparation,
                Some(u64::from(terminal::init_error_code(foreign))),
            ),
        ),
        (
            |f| f.state.efer = NativeEfer::admit(0xd01, true).ok(),
            None,
            StartupStep::Failed(StartupStage::EferReset, None),
        ),
        (|f| f.state.efer = None, None, StartupStep::Failed(StartupStage::OwnerMissing, None)),
        (
            |f| f.state.guest_apic = None,
            None,
            StartupStep::Failed(StartupStage::OwnerMissing, None),
        ),
        (
            |f| f.state.slot = 2,
            None,
            StartupStep::Failed(StartupStage::ModeCommitPreparation, None),
        ),
        (
            |f| put(&mut f.vmcb, 0xa8, 0x8000_0b0d),
            None,
            StartupStep::PendingEvent(ExternalInterruptError::PendingInjection),
        ),
        // Cache replay in progress on this core.
        (|_| {}, Some(1), StartupStep::Failed(StartupStage::CacheReplay, None)),
    ];
    for (index, (edit, phase, expected)) in cases.into_iter().enumerate() {
        let mut f = Fixture::new();
        let boxes = mailboxes();
        boxes[1].publish(NativeStartupCommand::Init).unwrap();
        edit(&mut f);
        let core = phase.map(cache_core);
        let before = f.snapshot();
        let writes = Cell::new(0);
        assert_eq!(
            f.step(&boxes, &writes, false, core.as_ref(), || panic!("debug reset after a refusal")),
            expected,
            "case {index}"
        );
        assert_eq!(writes.get(), 0, "case {index}");
        assert_eq!(f.snapshot(), before, "case {index}");
        assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init), "case {index}");
        assert!(try_lock_routes(&boxes).is_ok());
        assert!(core.as_ref().is_none_or(|core| core.try_lock().is_some()));
        assert_eq!(history(&boxes), (NativeDestinationCause::Observed, 0), "case {index}");
    }
}

#[test]
fn a_failed_lapic_commit_is_terminal_before_any_cpu_state_change() {
    let mut f = Fixture::new();
    let boxes = mailboxes();
    boxes[1].publish(NativeStartupCommand::Init).unwrap();
    let (vmcb, efer) = (*f.vmcb.bytes(), f.state.efer);
    let writes = Cell::new(0);
    // The physical EOI does not clear the ISR: the drain cannot finish.
    let result =
        f.step(&boxes, &writes, true, None, || panic!("debug reset after a failed commit"));
    let failure = InitError::Irq(IrqError::UnexpectedPhysicalIsr(0x40));
    assert_eq!(
        result,
        StartupStep::Failed(
            StartupStage::InitLapicCommit,
            Some(u64::from(terminal::init_error_code(failure)))
        )
    );
    // The physical reset happened; the CPU, EFER and backing did not
    // change, and the command stays queued without a destination record.
    assert_eq!(writes.get(), 9);
    assert_eq!((*f.vmcb.bytes(), f.state.efer), (vmcb, efer));
    assert_eq!(f.page.read_register(apic::TPR), Ok(0x6b));
    assert_eq!(f.state.startup, NativeStartupState::Running);
    assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
    assert_eq!(history(&boxes), (NativeDestinationCause::Observed, 0));
}

#[test]
fn a_busy_cache_lease_defers_the_command_unchanged() {
    let mut f = Fixture::new();
    let boxes = mailboxes();
    boxes[1].publish(NativeStartupCommand::Init).unwrap();
    let core = cache_core(0);
    let sibling = core.try_lock().unwrap();
    let (before, writes) = (f.snapshot(), Cell::new(0));
    assert_eq!(f.step(&boxes, &writes, false, Some(&core), || panic!("busy")), StartupStep::Busy);
    assert_eq!((f.snapshot() == before, writes.get()), (true, 0));
    assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
    assert!(try_lock_routes(&boxes).is_ok());
    drop(sibling);
    assert_eq!(
        f.step(&boxes, &writes, false, Some(&core), || {}),
        StartupStep::Applied(NativeStartupEffect::Init)
    );
}

#[test]
fn the_commit_runs_while_another_cpu_holds_the_route_lease() {
    let mut f = Fixture::new();
    let boxes = mailboxes();
    boxes[1].publish(NativeStartupCommand::Init).unwrap();
    let Fixture { profile, vmcb, frame, state, page, msrpm, registers } = &mut f;
    let page: &BackingPage = page;
    let (held, timed_out, writes) = (AtomicBool::new(false), AtomicBool::new(false), Cell::new(0));
    let step = std::thread::scope(|scope| {
        scope.spawn(|| {
            let routes = try_lock_routes(&boxes).unwrap();
            held.store(true, Ordering::SeqCst);
            // Release only once the destination has reset its backing
            // page (TPR 6Bh becomes 0), i.e. inside its INIT commit.
            let start = std::time::Instant::now();
            while page.read_register(apic::TPR) != Ok(0) {
                if start.elapsed() > std::time::Duration::from_secs(20) {
                    timed_out.store(true, Ordering::SeqCst);
                    break;
                }
                std::thread::yield_now();
            }
            drop(routes);
        });
        while !held.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        let owners = InitOwners {
            backing: page,
            physical: Apic { registers, writes: &writes, broken_eoi: false },
            msrpm,
            signature: 0x00b4_0f40,
        };
        startup_step(
            state,
            vmcb,
            frame,
            &boxes,
            profile,
            NativeStartupCommand::Init,
            None,
            owners,
            || {},
        )
    });
    assert!(!timed_out.load(Ordering::SeqCst), "the INIT commit waited for the route lease");
    assert_eq!(step, StartupStep::Applied(NativeStartupEffect::Init));
    assert_eq!(boxes[1].peek(), None);
    assert_eq!(history(&boxes), (NativeDestinationCause::GuestInit, 1));
}

#[test]
fn a_lost_route_lease_stops_after_the_bounded_wait_with_the_command_queued() {
    let mut f = Fixture::new();
    let boxes = mailboxes();
    boxes[1].publish(NativeStartupCommand::Init).unwrap();
    let writes = Cell::new(0);
    let routes = try_lock_routes(&boxes).unwrap();
    assert_eq!(
        f.step(&boxes, &writes, false, None, || {}),
        StartupStep::Failed(StartupStage::RouteTable, None)
    );
    drop(routes);
    // Applied but neither recorded nor completed: the stop is terminal.
    assert_eq!(f.state.startup, NativeStartupState::AwaitSipi);
    assert_eq!(f.page.read_register(apic::TPR), Ok(0));
    assert_eq!(boxes[1].peek(), Some(NativeStartupCommand::Init));
    assert_eq!(history(&boxes), (NativeDestinationCause::Observed, 0));
}
