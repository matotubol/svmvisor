# x2APIC/x2AVIC completion with the host IRQ bridge - 2026-09-16

This is the implementation record for the batch that completes the exclusive
guest x2APIC/x2AVIC interface, including its independent reviews and the
review-fix pass. The code is in uncommitted changes on `main` on top of HEAD
`eafe33a`. Only host tests and linked-image audits have run. Nothing was
committed, packaged or flashed, and nothing ran natively. Native entry, exit
frequency, timing, interrupt loss, IPI latency and Windows boot have not been
measured, and no physical result applies to this tree. The last verified flash
is still the diagnostic image described in [the handoff](handoff-2026-09-16.md).

This record replaces the implementation checkpoints in
[the rewrite design](x2avic-rewrite-design-2026-09-16.md). The
[hardware review](x2avic-hardware-review-2026-09-16.md) remains the
architectural evidence base. The two IOMMU reports are historical.

**Evidence.** [`handoff-evidence/2026-09-16-x2avic/`](handoff-evidence/2026-09-16-x2avic/)
holds the portable text evidence: the design brief, the integration and fix
notes, the code review, the independent test notes and the four manual fact
sheets. The rendered manual page images that those reviews read (about 1,000
PNG files) stay local under `work/x2avic-manual-2026-09-16/` and
`work/x2avic-batch-2026-09-16/`, which git ignores, together with the build
and audit outputs.

The batch ran in five phases:
- A: removals, moves and baseline repair;
- B-core: the register, IRQ and IPI owners and the doorbell;
- B-layout: remote backing aliases and the host ID limit;
- B-int: runtime wiring, guest INIT and stop codes;
- fix pass: the review fixes F1-F13 (see [Review outcome](#review-outcome)).

D1-D10 refer to the decisions in the batch brief; F1-F13 to the fixes. Where
an earlier phase and the fix pass disagree, the fix pass wins.

## Scope and user decisions

1. **Device interrupts use the host IRQ bridge** (`capture_physical_irq`). The
   resident host accepts each physical interrupt on its own CPU and publishes it
   into that CPU's AVIC backing page.
   - Windows keeps programming the physical IOMMU natively.
   - IOMMU guest-APIC (GA) direct posting is deferred to a later batch.
2. **Production-uncalled code is removed, not parked.** Every new abstraction
   has a production caller in this batch.
3. **The crate layout is unchanged.** No crate was split out in this batch.

## Removed, moved and kept

**Removed.** The following were removed under user decision 2, together with
their tests:

| Removed | Tests removed |
| --- | --- |
| `svm/iommu.rs`: IOMMU capture, DMA merge, command queue and ring | `tests/iommu.rs` (8) |
| `svm/native_sources.rs`: shared source-route owner | `tests/native_sources.rs` (5) |
| `svm/native_mmio.rs`, `exit.rs::native_mmio_continuation`, and the data-operand paths of `fetch` | `tests/native_mmio.rs` (24); 5 MMIO tests of `native_apic_independent.rs`; the data-path cases of two `fetch.rs` unit tests |
| `svm/native_apic_reset.rs` | `tests/native_apic_reset.rs` (1) |
| DXE `native/resident/iommu.rs` (IVRS discovery); `iommu_boot.rs` (three IOMMU MMIO NPT traps); the IOMMU admission gate (`admit_x2avic`) | `dxe/tests/native_iommu.rs` (5) |
| `SOURCE_ROUTES_OFFSET` (0xf0000, 4 pages), with its runtime mapping, DXE write and alias checks. Also in the runtime: `source_routes()`, the F505h route check, the F523h/F524h level branch and `write_directed_eoi` (the fd000 window) | none |
| `BackingPage::prepare_trigger_stopped` (folded into `set_trigger`) | none |
| B-int, `arch::x86_64::apic`: `physical_eoi`, `physical_highest_in_service`, `physical_level_triggered`, `LVT_TIMER_MODE`, `LVT_TIMER_PERIODIC` and `EXTENDED_MSR_FIRST` | none |
| B-int, runtime: `drain_physical_eoi`; `apply_avic_register_backend` (the provisional count-driven timer backend); the old capture body; the old `handle_avic_msr` value match with its 840h+ and F511h branches; the `complete_level` EOI branch; the startup stage-14 INIT stop; `State.apic_base`; the dead `id_count > 1` MSRPM guard and `icr: None` branch; a duplicate 60h match arm; arm's direct physical TPR/SVR writes and its raw LVT copy loop | none (B-int changed no existing Rust assertion) |

**Moved.**
- `svm/x2avic.rs` became the `svm/x2avic/` directory. `BackingPage`,
  `PhysicalIdTable` and `AvicExit` are re-exported as before.
- `svm/native_irq.rs` became `svm::x2avic::irq`.
- `svm/ipi.rs` became `svm::x2avic::startup`. Type names are unchanged.
- APIC register constants and the physical accessors moved to
  `arch::x86_64::apic`.
- `tests/native_apic_independent.rs` became `native_cet_independent.rs`, which
  keeps the file's 5 CET/INIT tests.
- Both library crates lost their root compatibility aliases.

**Other Phase A changes.**
- NPT `TABLE_COUNT` went back from 16 to 8.
  - This restores the native-returning arena layout. Its UEFI build failed at
    HEAD.
  - Resident BSS shrinks by 16 pages.
- `tools/native-stack-audit/run.py` now parses cargo's current quoted `--cfg`
  rendering.
- `tools/native-resident/check_iommu.py` stays as a read-only IVRS evidence
  tool.

**Removed in the fix pass (F9),** after checking every caller:
- `IdentityNpt::trap_page`, with the trapped-page state and the trapped
  branches of `translate`. Three `npt.rs` unit tests went with it, and
  `tests/identity_npt.rs` now asserts that FEE0_0000h stays identity-mapped and
  writable.
- `terminal::route_failure` and the kind-14 branch of `stop_words`. F10Bh now
  exports as an unhandled exit with the guest RIP; the decoder still reads
  kind 14 from older images.
- The generic `NativeStartupTarget::{validate, apply}`. Their CET and
  INIT-state tests were converted to `apply_x2avic` with an armed profile,
  because they cover state the x2AVIC commit shares.
- `GuestX2Apic::apic_base()`; its test-only uses now read APIC_BASE through
  the RDMSR emulation path.
- The `bool` returned by `BackingPage::eoi_stopped`, which both production
  callers ignored.

`FixedIpi::{vector, targets}` is kept: the runtime glue tests and
`x2avic_ipi.rs` use it.

**Still production-uncalled.** The phase notes record these; the batch did not
change them:
- the unused fd000 alias page;
- the builder-less `resident-runtime-test` feature;
- firmware-handoff `retain`/`retain_with_smp`.

## Architecture

### Owners

| Owner | Source | Responsibility |
| --- | --- | --- |
| `arch::x86_64::apic` | `crates/hypervisor/src/arch/x86_64/apic.rs` | Data: APIC_BASE fields, Table 16-2 offsets, x2APIC MSR numbering, ICR reserved bits and message types. Physical access: `PhysicalX2Apic` and `HostX2Apic` are the only path from the x2AVIC owners to physical x2APIC registers; ISR and TMR readers. Doorbell: `DoorbellTarget` (host IDs 0-254) and `ring_avic_doorbell` (WRMSR C001_011Bh with EDX=0 and EAX=ID). |
| `arch::x86_64::msr` | `arch/x86_64/msr.rs` | `AVIC_DOORBELL` and the VM_CR fields |
| `svm::x2avic` | `svm/x2avic/mod.rs` | Capability admission: CPUID Fn0000_0001 ECX[21], and Fn8000_000A EDX[0] (NPT), EDX[13] (AVIC) and EDX[18] (x2AVIC). Also `NativeX2AvicProfile`, `NATIVE_CONTROL` (VMCB 060h bits 31:30 and 24), the guest version `0x0005_0010` and the derived logical x2APIC ID. |
| `svm::x2avic::registers` | `registers.rs` | The D1 profile (`intercepted`). `GuestX2Apic`, the APIC_BASE shadow, which also emulates every intercepted access. The shared LVT rule `Lvt::check`. `CapturedInterface`, the loader state admitted at arm. The guest-INIT LAPIC half (`prepare_init`, `commit_init`). The `Refusal` codes. |
| `svm::x2avic::ipi` | `ipi.rs` | The admitted inventory; 401h policy (`Inventory::classify`); destination matching; fixed-IPI fan-out with doorbells (`deliver_fixed`) |
| `svm::x2avic::irq` | `irq.rs` | `PhysicalIrqLedger` (held and completed level sources); `capture`; a bounded physical EOI `drain`; software EOI; the 402h `level_eoi_exit`; INIT `retire` |
| `svm::x2avic::backing` | `backing.rs` | The atomic 4 KiB backing page: `enqueue` (TMR, then IRR), `eoi_stopped`, the stale-TMR clear, and the INIT identity check and reset |
| `svm::x2avic::table`, `svm::x2avic::exit` | `table.rs`, `exit.rs` | The shared physical-ID table, IsRunning and the pre-arm entry check `is_stopped_entry`; 401h/402h decoding |
| `svm::x2avic::startup` | `startup.rs` | The INIT/SIPI mailbox transport (`NativeIcr::route_x2avic_startup`; `NativeIcr` holds the `ipi::Inventory`); the target CPU-state commit (`NativeStartupTarget::{validate_x2avic, apply_x2avic}`) |
| `svm::permission_maps` | `permission_maps.rs` | `Msrpm::configure_native_x2avic`, built from `registers::intercepted`; `update_x2apic_eoi_intercept` |
| `host::resident` | `host/resident.rs` | Layout constants, including `X2AVIC_BACKING_ALIASES_OFFSET`; `DIRECTORY_VERSION` 10; `valid_pool_slot` (host ID at most 254); the takeover refusal `captured_register_refusal` (tag A1h, reason 5) |
| `host::resident::runtime` | `host/resident/runtime.rs` | Exit wiring (next section) |
| `host::resident::terminal` | `host/resident/terminal.rs` | `X2AvicStop`, `IrqSite`, `StartupStage` and their encoders |
| Host IDT gates | `crates/dxe/src/native/resident/{irq,runtime}.S` | Acceptance-window gates for vectors 16-255 except 18, and the #SX chain (F3) |
| DXE resident | `crates/dxe/src/native/resident/{launch,activation,physical_boot}.rs` | The image bound, the pool-wide backing offset, the alias plan and root walks, the host ID limit, and pass-through of the typed takeover code |
| Payload link and audits | `tools/native-resident/payload.ld`, `build.py` | `ASSERT(image_bss_end <= 0x1d4000, ...)`; the host-fault audit also checks the 239 window gates and the #SX chain |
| Snapshot decoder | `firmware/squirrel/read_snapshot.py` | Names for startup stages 10-15, INIT failure values, the event-3 `stop_counters` field and takeover reason 5 |

### Runtime wiring (`host::resident::runtime`)

| Path | Owner calls |
| --- | --- |
| `prepare` | Image bound: an image ending above `base + 0xd4000` is refused. The backing page must be page-aligned inside data/bss. Writes the remote alias PTEs (`backing_alias_pte`) and installs the host IDT gates where `window_gate(vector)` holds. |
| `arm` | Admission and install (see [Arm admission](#arm-admission-and-captured-register-state)) |
| Exit 60h: `capture_physical_irq` | The bounded acceptance helper, then `irq::capture` (`capture_accepted`) and `sync_eoi_intercept` |
| Exit 7Ch: `handle_avic_msr` (MSRs 800h-8FFh and 1Bh) | Boundary checks, then `GuestX2Apic::emulate`, `apply_msr_completion` and `sync_eoi_intercept` |
| Exits 401h/402h: `handle_avic_exit` | `avic_exit_plan`. For 401h, `handle_incomplete_ipi`: `incomplete_ipi_plan`; then `route_x2avic_startup`, or `deliver_fixed` through `remote_backing` and `ring_avic_doorbell`; then `clear_icr_delivery_status`. For 402h: `irq::level_eoi_exit`, then `sync_eoi_intercept`. |
| Startup mailbox: `service_startup`, one command at a time in `startup_step` | `validate_x2avic` and `validate_destination_slot`, then `guest_init` for INIT or `apply_x2avic` for SIPI, then the destination record and `mailbox.complete` |
| `terminal_finish` | Rechecks the physical APIC_BASE against the captured `host_apic_base` |

**Dispatch order (F2).** Before the guest ACK, 60h is captured early, every
other exit goes to the ACK path, and no startup command is serviced. After the
ACK, `dispatch_body` calls `sequence_exit(exit_order(code), ...)`:
- 60h, 401h and 402h are traps: the handler runs first, and if it resumes,
  the startup service runs too. A 60h exit therefore always reaches the
  service.
- Every other exit runs the startup service first. A command that changed the
  guest, or a stop, decides the exit; otherwise the instruction handler runs.

**Lease protocol (F1).** No path holds both leases, so "route, then core"
remains the only permitted nesting.
- Lease-free: `validate_x2avic` and `validate_destination_slot`.
- The core lease of cache replay, which must be idle, covers the INIT or SIPI
  commit and is dropped before the route lease is taken.
- The route lease covers only a guest INIT's destination record and
  `mailbox.complete`, which must not interleave with a sender's FIFO preflight
  and store.
- Both the target (`lock_routes_within`) and the 401h sender (`route_startup`)
  wait up to `ROUTE_WAIT_ATTEMPTS` (2^24 CAS/PAUSE attempts), because a
  completed ICR write cannot be retried. `try_lock_routes` (64 attempts) stays
  for callers that re-enter an unchanged instruction (SYS_CFG, low-RAM guest
  reads).
- If the target cannot take the route lease after its commit, it stops at
  startup stage 2 with the command still queued.

`sync_eoi_intercept` calls `Msrpm::update_x2apic_eoi_intercept`. If the map
changed, it also calls `vmcb.invalidate_all()`.

`remote_backing(slot)` returns the page at
`image_start + X2AVIC_BACKING_ALIASES_OFFSET + slot*4096`.
- It is valid only under this CPU's private root, and only for slots below the
  armed pool count.
- The CPU's own slot is also reached through its alias.

### Per-CPU memory layout

Each resident CPU owns one 1 MiB slot of the retained pool.
- The pool has at most 32 slots.
- `pool_bytes` must equal the CPU count times 1 MiB; arm checks this.
- The offsets below are relative to a slot base and hold in every private host
  root.

| Range | Content |
| --- | --- |
| `0 .. image_bss_end` | Identity-mapped image: text RX, rodata R/NX, data/bss RW/NX. Stack guards are absent. This range contains the slot's own `AVIC_BACKING` page. |
| `image_bss_end .. 0xd4000` | Absent |
| `0xd4000 + s*0x1000`, for slot `s` < CPU count | RW/NX alias of slot `s`'s backing page. PA = `pool_base + s*1MiB + backing offset`; PTE flags present, RW and NX; PAT index 0. |
| `0xd4000 + s*0x1000`, for count <= `s` < 32 | Absent |
| `0xf4000` | Alias of the shared physical-ID table (PA `pool_base + 0xf4000`) |
| `0xf5000 .. 0xf7fff` | Cache owner, RW/NX |
| `0xf8000 ..` | Cache capture, R/NX |
| `0xfb000`, `0xfc000` | Diagnostic aliases, armed later |
| `0xfd000` | Unused |
| `0xfe000` | Startup page |
| `0xff000` | Scratch window |

- **Alias range and image bound.**
  `X2AVIC_BACKING_ALIASES_OFFSET = X2AVIC_TABLE_OFFSET - 32*4096 = 0xd4000`,
  and a const assert checks it. The image must end at or below
  `base + 0xd4000`. Three places enforce this:
  - runtime `prepare`;
  - DXE `directory_valid`;
  - `payload.ld`, which checks `0x1d4000` because the link base is 0x100000.
    `tests/resident_layout.rs` checks that linker literal.
- **One backing offset for the whole pool.** Every slot is a relocated copy of
  the same package, so the backing page sits at the same image offset in every
  slot.
  - `prepare` sees only its own offset.
  - DXE `common_backing_offset` refuses a pool whose directories disagree on
    pool base, pool size, slot order or backing offset. It runs before the
    physical-ID table is populated and refuses with `LOAD_ERROR`.
- **DXE alias walks.** `host_closure` walks all 32 alias leaves of each root.
  - A present entry must have the exact PA, RW, NX, supervisor access, WB for
    both PAT and MTRR, and its table pages inside the image data (predicates
    621-624 and 632).
  - An absent entry must fail the walk at level 1; otherwise predicate 634 is
    raised.
  - A refused plan raises predicate 633.
  - All of these fail with error 24.
  - The walk runs at install for every root, in the AP admission observer for
    every slot on every AP, and in `callback` for the CPU's own root just
    before arm.
- **Directory version and ID limit.**
  - The directory version is 10; DXE refuses version 9.
  - Host APIC IDs above 254 are refused in three places: `valid_pool_slot`,
    the DXE inventory check and arm.
- **Linked evidence** (`b-int-build`):
  - `image_bss_end` is 0x17c000, i.e. 0x7c000 above the base.
  - `AVIC_BACKING` is at 0x12b000, offset 0x2b000.
  - Headroom below 0x1d4000 is 0x58000 bytes (88 pages).

## Manual sources and citation keys

Every rule below was read from rendered page images:
- the fact sheets in `work/x2avic-manual-2026-09-16/{apm-avic,apm-x2apic,ppr-lapic,acpi-madt}/facts.md`;
- APM2 PDF pages 55-56, rendered into `work/x2avic-batch-2026-09-16/b-core-pages/`.

The SHA256 values were rechecked for this record. A citation such as
`p658/PDF720` gives the printed page, then the one-based PDF page.

| Key | Document | Revision | SHA256 prefix | Page mapping |
| --- | --- | --- | --- | --- |
| APM2 | AMD64 APM Vol. 2, pub. 24593 | 3.44, March 2026 | `3d9dcb3f` | On the cited chapter 15/16 pages, printed = PDF - 62. p.lvi = PDF 56. |
| APM3 | AMD64 APM Vol. 3, pub. 24594 | 3.37, July 2025 | `c77a21e7` | Page-specific: p511 = PDF 546, p654 = PDF 688 |
| PPR | PPR 57896, Family 1Ah Model 44h B0 (product-specific) | 3.00, Aug 28 2024 | `643cae09` | printed = PDF |
| ACPI | ACPI Specification | 6.6 | `8c7542dd` | printed = PDF - 71 |

Unless a citation names another key, it refers to APM2.

**Informative source (never overrides the manuals).** Linux KVM
`arch/x86/kvm/svm/avic.c` at `9b87fdc9af2fbfcdb5c24a64139685ef80f6573f`,
kept in `work/x2avic-review-2026-09-16/linux-reference/`. Lines cited:
- 473-480: a spurious doorbell "is harmless";
- 656-670: hardware "falls over if _any_ targets are invalid", and "hardware
  may sometimes leave the BUSY flag set";
- 684-686: KVM drops ID 4;
- 813-819: KVM treats the EOI access as a trap.

## Guest x2APIC interception profile (D1)

**Why the MSRPM intercepts.**
- Table 15-22 (p566-568/PDF628-630) makes writes to SVR, the LVTs, the timer,
  ESR, ID and LDR into AVIC traps that happen after the backing page is
  updated. That is too late to raise the chapter-16 #GP(0) cases.
- 15.11 (p518/PDF580) checks the MSRPM before MSR-specific exceptions, and
  15.29.10 (p583/PDF645) checks x2APIC MSR intercepts before AVIC permissions.
  An MSRPM intercept therefore wins before any hardware access.
- An intercepted RDMSR/WRMSR produces VMEXIT_MSR (7Ch) with a valid nRIP
  (15.7.1 p509/PDF571).

`Msrpm::configure_native_x2avic` applies the table below to MSRs 800h-8FFh in
each vCPU's private MSRPM. It always intercepts APIC_BASE (1Bh).

| MSR | Register | Read | Write |
| --- | --- | --- | --- |
| 1Bh | APIC_BASE | intercepted: returns the shadow | intercepted: D4 |
| 800h-801h, 804h-807h, 829h-82Fh, 83Ah-83Dh | unimplemented | intercepted: #GP | intercepted: #GP |
| 802h | x2APIC ID | hardware (backing page) | intercepted: #GP (16.12) |
| 803h | Version | hardware (backing page, `0x0005_0010`) | intercepted: #GP (x2APIC U1) |
| 808h | TPR | hardware | hardware (accelerated; kept in step with V_TPR) |
| 809h | APR | intercepted: emulated | intercepted: #GP (U1) |
| 80Ah | PPR | hardware | intercepted: #GP (U1) |
| 80Bh | EOI | intercepted: #GP (U2) | hardware for edge sources; intercepted while this CPU holds a level source (D6) |
| 80Ch, 80Eh, 831h | eliminated RRR, DFR and ICR high | intercepted: #GP | intercepted: #GP |
| 80Dh | LDR | hardware (derived logical ID) | intercepted: #GP (U1) |
| 80Fh | SVR | hardware | intercepted: emulated |
| 810h-827h | ISR, TMR, IRR | hardware | intercepted: #GP (U1) |
| 828h | ESR | hardware | intercepted: emulated |
| 830h | ICR | hardware | hardware (fixed edge IPIs accelerated; other IPIs exit with 401h) |
| 832h-837h | LVTs | hardware | intercepted: emulated and mirrored |
| 838h | Initial count | hardware | intercepted: emulated and mirrored |
| 839h | Current count | intercepted: returns the physical count | intercepted: #GP (U1) |
| 83Eh | Divide configuration | hardware | intercepted: emulated and mirrored |
| 83Fh | SELF IPI | intercepted: #GP (16.15) | hardware (accelerated) |
| 840h-8FFh | AMD extended space and unimplemented MSRs | intercepted: #GP (U10) | intercepted: #GP (U10) |

**Boundary rules in `handle_avic_msr`:**
- It checks the profile, the guest APIC owner and pending-event state first.
  A failure stops with F510h.
- **Continuation.**
  - In 64-bit guest code on a CPU with NRIPS, the hardware nRIP is the
    continuation.
  - Guest code outside 64-bit mode (AP trampolines), or a CPU without NRIPS,
    uses the bounded instruction byte fetch, as the EFER and VM_CR owners do.
  - An unsupported instruction mode or a set TF stops with F510h, info2 3.
- A guest CPL above 0 receives #GP(0).
- **Outcomes.**
  - A completed access commits its continuation once; an RDMSR loads EDX:EAX.
  - A #GP(0) is queued at the unchanged RIP
    (`queue_native_x2avic_general_protection`).
  - A refusal stops without side effects.

## Register semantics (D2-D4)

### Emulated registers

**#GP(0) cases.** Each of these raises #GP(0) with no side effects and RIP
unchanged:
- any access to an unimplemented MSR (16.11.1 p657-659/PDF719-721);
- a write to 802h (16.12 p660/PDF722);
- a read of 83Fh (16.15 p662/PDF724);
- a non-zero write to 80Bh or 828h (Table 16-6 p658/PDF720; 16.11.3
  p659/PDF721);
- any reserved bit set, including bits 63:32 of every register except ICR
  (16.11.3);
- the U1, U2 and U10 cases below.

**Per-register rules:**
- **APR (809h) read.** APR is the highest priority class among TPR, the highest
  backing ISR vector and the highest backing IRR vector. The sub-priority
  equals TPR's only when APR equals the TPR class (Figure 16-22 p647/PDF709).
- **ESR (828h).** A write of zero stores 0. The virtual error state is always
  empty (U15/U16).
- **SVR (80Fh).**
  - Setting any of bits 63:10, including bit 12, faults (Figure 16-17
    p641/PDF703; PPR p57, p176). Otherwise the value is stored in the backing
    SVR.
  - With bit 8 clear, the mask bit is forced into all six backing LVTs and
    their physical mirrors. The manual says "All LVT entry mask bits are set
    and cannot be cleared" (16.3.1 p629/PDF691; PPR p57).
  - After re-enable, the masks stay set until each LVT is rewritten (U13).
  - The physical SVR stays host-owned at 1FFh.
  - **While disabled (F12).** A disabling write records the currently pending
    IRR. An enabling write first withdraws every IRR bit that is pending now,
    was not pending at the disable, and is not a level source this CPU's ledger
    holds; their TMR bits go with them (`BackingPage::discard_pending`). The
    SVR is stored after that withdrawal.
    - The guest is stopped across the withdrawal and the store, and a real APIC
      would never have accepted those interrupts (16.3.1 p629: while ASE is
      clear, "Further fixed, lowest-priority, and ExtInt interrupts are not
      accepted").
    - Neither the software fan-out nor the bridge publishes edges into a
      disabled page, so the withdrawn bits come from x2AVIC hardware IPIs,
      which ignore the virtual SVR.
    - A vector already pending at the disable stays pending.
    - Guest INIT clears the record, because the reset page is disabled with an
      empty IRR.
- **Initial count (838h).**
  - Setting bits 63:32 faults.
  - Otherwise the value is stored and written to the physical register. A
    non-zero value restarts the timer and zero stops it (16.4.1 p636/PDF698).
- **Divide (83Eh).**
  - Setting bits 63:4 or bit 2 faults (Figure 16-11 p637/PDF699). All eight
    encodings of bits 3 and 1:0 are defined (Table 16-3 p638/PDF700).
  - Otherwise the value is stored and mirrored.
- **Current count (839h) read.** Returns physical 839h bits 31:0 (Figure 16-9
  p637/PDF699). On this product the counter may step by 1 to 8 (PPR p43).
- **EOI (80Bh).** While the write is intercepted, a write of zero is a software
  EOI (see [Physical interrupts](#physical-interrupts-eoi-and-level-sources)).
- **Version.** The guest version `0x0005_0010` exposes six LVTs, with bit 31
  (extended space) and bit 24 clear.
  - PPR p175 advertises DirectedEoiSupport, but the PPR defines no enable bit
    (its x2APIC SVR bits 63:10 are reserved). No directed EOI is presented.
  - `native_boot_cpuid` already hides CPUID Fn8000_0001 ECX[3]
    (ExtApicSpace).

### LVT rules

**Reserved bits (writing any of them faults):**
- timer: 63:18, 15:13 and 11:8. Figure 16-8 (p636/PDF698) defines no message
  type and no TSC-deadline bit (U23).
- thermal, performance and error: 63:17, 15:13 and 11 (Figures 16-13 to 16-15,
  p638-639/PDF700-701).
- LINT0/LINT1: 63:17, 13 and 11 (Figure 16-12 p638/PDF700).

**Read-only bits.** Delivery status (bit 12) and LINT remote IRR (bit 14) are
ignored on write and stored as 0 (U14).

**Message types (Table 16-1 p628/PDF690; Figure 16-7 p635/PDF697):**
- **Admitted types.** Thermal, performance and error admit fixed, SMI and NMI.
  LINT admits fixed, SMI, NMI and ExtINT.
- **Other types.** Any other type stops as F542h, masked or not (U3).
- **Unmasked SMI.** Stops as F543h. With HWCR SmmLock set, SMIs are not
  intercepted in SVM (PPR p204).
- **Unmasked ExtINT LINT.** Stops as F544h. The 8259 supplies the vector and
  sets no physical ISR bit, so the bridge cannot own the source.

**Forced masks and illegal vectors:**
- **Software disable.** While the virtual SVR bit 8 is clear, the stored value
  carries the mask.
- **Illegal vector.** An unmasked fixed entry with vector 0-15 is an
  illegal-vector APIC error, not a #GP (Figure 16-7 p635).
  - The entry is stored as written.
  - Only its physical mirror is masked, because the virtual APIC never delivers
    it.
  - No ESR bit is set (U15/U16).
- **Exception vector (F3).** An unmasked fixed entry with vector 16-31 stops as
  F547h (`Refusal::ExceptionVector`), and the same value captured from the
  loader refuses arm.
  - Vectors 16-31 are host exception vectors. Mirroring such an entry would
    deliver a physical interrupt into the host IDT's exception stubs.
  - Masked entries, entries on a software-disabled APIC, and NMI, SMI or
    ExtINT entries are unaffected.

### Physical mirroring (D3)

The physical LAPIC stays host-owned. Guest-visible values live in the backing
page. To mirror a register, the runtime writes the validated value, with its
read-only bits cleared, to the same physical MSR:

- **Timer LVT, initial count and divide.** Mirrored exactly. The guest timer is
  this CPU's physical LAPIC timer, and vCPUs are pinned 1:1.
- **Thermal, performance and error LVTs.**
  - Masked, fixed and NMI entries are mirrored.
  - A fixed source raises a physical interrupt at the guest's vector; the
    bridge captures and publishes it.
  - Physical NMIs are not intercepted and reach the guest in guest mode. An NMI
    that arrives during a host GIF window remains terminal.
- **LINT0/LINT1.**
  - Masked, unmasked NMI and unmasked fixed entries are mirrored.
  - A level-triggered fixed LINT sets physical TMR, so the bridge holds it as a
    level source.
  - The captured MADT has one Local APIC NMI entry: UID 0xFF, LINT1,
    active-high, edge (ACPI Table 5.28 p139/PDF210). Windows is therefore
    expected to program LINT1 as NMI on every CPU.
- **Forced masks.** An entry whose mirror must be masked is written masked.
  That covers two cases: a software-disabled virtual SVR, and an illegal fixed
  vector (0-15). An unmasked fixed vector of 16-31 is refused instead of
  mirrored (F547h).

### APIC_BASE shadow (D4)

**Admission (`GuestX2Apic::admit`).**
- The physical APIC_BASE captured at arm must show enabled x2APIC (AE=EXTD=1)
  at FEE0_0000h. BSC may be set.
- That value is returned for every guest read.
- Guest INIT leaves it unchanged. 16.10 (p657/PDF719) preserves AE and EXTD,
  and Figure 16-2 (p630/PDF692) preserves the base address.

**Writes.**
- **Faults (Figure 16-2 p630; PPR p121 reserves bits 63:48 on this
  product).** A write faults if it sets any of these:
  - bits 63:52;
  - bit 9;
  - bits 7:0;
  - base bits at or above the admitted physical-address width.
- **BSC.** BSC is read-only, and its written value is ignored (U7).
- **AE:EXTD = 11.** With the unchanged base, the write completes as a no-op.
  With a different base, it stops as F546h. Figure 16-32 (p656/PDF718) has no
  such transition (U7).
- **AE:EXTD = 01 or 10.** These fault. Table 16-5 (p655/PDF717) makes 01
  invalid, and there is no x2APIC-to-xAPIC transition.
- **AE:EXTD = 00 (documented deviation).** This is a valid x2APIC-to-disabled
  transition in Figure 16-32, but it stops as F545h because the exclusive
  profile cannot leave x2APIC mode.

## Recorded decisions on unresolved manual points

### x2APIC fact-sheet numbering (`apm-x2apic/facts.md`)

| U | Decision | Sources |
| --- | --- | --- |
| U1 | Writes to the read-only 803h, 809h, 80Ah, 80Dh, 810h-827h and 839h raise #GP(0). | Table 16-6 p658/PDF720 says only "RO". PPR p174-182: Error-on-write. The rule for 802h is explicit in 16.12 p660/PDF722. |
| U2 | RDMSR of 80Bh raises #GP(0). | Table 16-6 p658 "WO"; PPR p175 Error-on-read |
| U3 | An LVT message type not admitted for that source (Table 16-1 for thermal/perf/error, Figure 16-7 for LINT) stops with F542h, masked or not. If captured at arm, it refuses arm with code 11. | Table 16-1 p628/PDF690; Figure 16-7 p635/PDF697; PPR p55 "all other message types are Reserved" |
| U4, U24 | x2APIC ICR message types 1, 3 and 7 stop with F555h. A fixed level-triggered IPI stops with F556h. INIT/SIPI shorthand validity stays with the startup router. | 16.13 p661/PDF723 (reserved encodings; PPR p179 still lists 1 and 7); Table 16-4 p644/PDF706 |
| U5 | Guest INIT keeps the x2APIC ID. It loads LDR with the derived logical x2APIC ID rather than Table 16-2's 0. | 16.14 p661-662/PDF723-724 (LDR is initialized whenever x2APIC mode is enabled); Table 16-2 p631/PDF693; 16.10 p657 cites a "Reset in x2APIC mode" section that does not exist; PPR p55 (ApicId is unaffected) |
| U7 | For APIC_BASE: same mode and same base is a no-op; a base change is refused; a BSC write is ignored; base bits at or above the admitted width fault. | Figure 16-2 p630/PDF692; Figure 16-32 p656/PDF718 (no self-loops); PPR p121 |
| U10 | 840h-8FFh raise #GP(0), because the presented version has bit 31 (extended space) clear. | Table 16-6 p658; Figure 16-4 p632/PDF694; p653/PDF715 (version bit 31 indicates the extended space) |
| U13 | Masks forced by a software disable stay set after re-enable until each LVT is rewritten. | 16.3.1 p629/PDF691; Figure 16-17 p641/PDF703; PPR p57, p176 |
| U14 | Delivery status and remote IRR are ignored on write and stored as 0. This applies to guest writes and to captured state. | Figure 16-7 p635 (RO); PPR p27 Table 8 "Read-only: Readable; writes are ignored" |
| U15, U16 | ESR accepts only 0 and always reads 0. No virtual APIC error is generated: an illegal-vector LVT is only masked physically, and illegal-vector or no-target IPIs are dropped. | 16.11.3 p659/PDF721; Figure 16-16 p640/PDF702; Figure 16-7 p635; PPR p178 |
| U18 | An EOI with an empty virtual ISR completes as a no-op. | p652/PDF714 (effect not stated) |
| U19 | In x2APIC mode, a physical DEST of FFh is not a broadcast; only FFFF_FFFFh is. | 16.13 p660/PDF722 |
| U20 | PPS is 0 when PP differs from TP. | 16.6.4 p651/PDF713 |
| U23 | Timer LVT bits 11:8 are reserved (no message type), and bit 18 is reserved (no TSC-deadline mode). | Figure 16-8 p636/PDF698; PPR p180 |

### AVIC fact-sheet numbering (`apm-avic/facts.md`)

| U | Decision | Sources |
| --- | --- | --- |
| U4 | The doorbell value is a host APIC ID of at most 254, with EDX = 0. This satisfies both documented formats. | Figure 15-22 p579/PDF641 (bits 7:0; bits 63:8 MBZ); PPR p216 (bits 31:0; bits 63:32 reserved) |
| U5 | A doorbell to a core in host mode is assumed harmless; that core's next VMRUN evaluates IRR. | 15.29.8.3 p579/PDF641; KVM (informative) |
| U7 | Physical-table entry 255 is avoided: host IDs must be at most 254. | Figure 15-18 p573/PDF635 reserves ID FFh for xAVIC only |
| U8 | 401h IDs 0 and 2 are routed by message type; INIT and SIPI go to the startup router. | Table 15-27 p581/PDF643 names no ID for non-fixed types |
| U9 | IDs 1 and 3 are stopped refusals (F551h, F552h). ID 1 is never republished, because hardware already set IRR. | 15.29.6.1 p576-577/PDF638-639 |
| U10 | 401h and 402h are handled as traps: RIP is never changed and nRIP is never used. | Table 15-22 p567/PDF629; 15.7.1 p509/PDF571 |
| U11 | The 402h level-EOI fallback accepts the virtual ISR bit either still set (it must be the highest; a software EOI follows) or already clear (left alone). | Table 15-22 p566/PDF628 says "trap"; 15.29.9.2 p581/PDF643 says "fault" |
| U12 | D1 intercepts every access whose chapter-16 #GP rule hardware might not apply. | 15.11 p518/PDF580; 15.29.10 p583/PDF645 |
| U17 | Software publication sets TMR before IRR. A stale TMR bit is cleared only when the vector is not pending. Races with remote edge publishers are benign. | 15.29.6.1 p577/PDF639 (states only hardware atomicity) |
| Clean bits | Any change to MSRPM contents clears every VMCB clean bit. The B-core notes file this decision under U22. | Figure 15-4 p527/PDF589 names only MSRPM_BASE under bit 1 |

### Other implementation decisions and deviations

- **Reserved EXITINFO bits.** `AvicExit::decode` ignores these bits:
  - for 401h: EXITINFO2 bits 31:12, and the index for IDs 0 and 4;
  - for 402h: EXITINFO1 bits 63:33, 31:12 and 3:0, and EXITINFO2 bits 63:8.

  The manual says "Software must not depend on the state of a reserved field
  (unless qualified as RAZ)" (p.lvi/PDF56; Tables 15-26, 15-28 and 15-29,
  p580-582/PDF642-644). IDs above 4, and EOI vectors below 16, still refuse
  with F581h.
- **ID 4.** 401h ID 4 drops only a fixed IPI with a vector below 16. Any other
  ICR stops with F559h, because VEC names a delivered vector only for the fixed
  and lowest-priority types (Table 15-27 p581; Figure 16-18 p642/PDF704).
- **ICR bit 12 tolerance** (coordinator override).
  - **Sources.** 16.13 p661/PDF723 makes the delivery-status bit must-be-zero, and
    15.29.9.1 (p580/PDF642) calls EXITINFO1 the value written. KVM
    (informative) reports that hardware may leave the busy flag set.
  - **Classification.** It masks bit 12. Every other reserved bit still stops
    with F554h.
  - **Resume.** On every 401h resume path, bit 12 of the backing ICR low word is
    cleared, so guest ICR reads stay conformant (16.11.3 p659: reserved bits
    read as zero).
  - **Evidence.** The stop record keeps the raw EXITINFO1.
  - **Arm.** The same rule drops bit 12 from a captured ICR.
- **402h ISR check.** The 402h fallback stops with F579h if the in-service
  vector is not the highest.
- **INIT preparation.** INIT preparation requires the physical ISR to equal the
  held level set exactly, so the retirement drain cannot fail.
- **Level completion.**
  - The ledger decides level completion, never TMR, because remote edge
    publishers can change TMR.
  - A second, edge instance of a vector that the ledger already holds as
    completed is acknowledged as an edge interrupt.
- **Guest INIT EFER and destination token.**
  - Guest INIT computes the EFER INIT value on a copy during preparation and
    commits it after the CPU commit. This is D9 step 6 with its only failure
    moved before the first effect.
  - A terminal commit failure drops the unused destination token. That is
    sound because the destination mode never changes.
- **Arm-time physical writes.**
  - Arm writes a physical LVT only to add a mask.
  - Arm never rewrites the counts or divide, because a count write restarts the
    timer (16.4.1 p636).
  - Arm sets PPR = TPR at install (16.6.4 p651). AVIC gates delivery with the
    backing PPR (15.29.3.1 p569/PDF631), and the previous arm left PPR at 0
    under a non-zero TPR.
- **ExtINT and SMI captured faithfully** (coordinator decision).
  - At arm, a firmware LINT left as unmasked ExtINT, or an unmasked SMI entry,
    is kept and stays live physically.
  - A guest write of the same value still refuses.
  - A source accepted through such a LINT has no local APIC ISR bit (16.6.3
    p647/PDF709), so it stops as F57Bh.
  - A reserved LVT message type refuses arm with code 11. Unlike ExtINT or SMI,
    it is not a legal platform configuration.
- **MSR byte fetch.** The MSR byte-fetch continuation is kept for guest code
  outside 64-bit mode and for CPUs without NRIPS.
- **Documented deviations from APM behavior.**
  - APIC disable stops with F545h.
  - A physical interrupt already pending in physical IRR at guest INIT is
    captured after the next VMRUN into the freshly reset page. A real INIT would
    discard it.

## IPIs (AVIC_INCOMPLETE_IPI, 401h)

**What hardware accelerates** (15.29.3.1 p569; 15.29.10 p583):
- fixed, edge-triggered IPIs in physical and logical mode;
- the self and broadcast shorthands;
- SELF IPI.

**Everything else exits with 401h** (Tables 15-25 to 15-27, p580-581/PDF642-643):
- EXITINFO1 is the ICR, with the destination in bits 63:32.
- EXITINFO2 carries the ID in bits 63:32 and the table index in bits 11:0.
- The ICR write has already completed, so a refusal can only stop, never raise
  #GP.

`incomplete_ipi_plan` masks ICR bit 12. `Inventory::classify` then evaluates
these rows in order:

| Condition | Result |
| --- | --- |
| ID 1 (target not running) | F551h. IsRunning is never cleared, so this means the table state is inconsistent. The IPI is never republished. |
| ID 3 (invalid backing page) | F552h |
| ID 5 or above | F581h. The decoder refuses these before `classify` runs, so `IpiRefusal::UnknownReason` (F553h) is unreachable from the runtime. |
| Any of ICR bits 31:20, 17:16 or 13 set | F554h |
| Message type 1, 3 or 7 | F555h |
| ID 4 | A fixed IPI with a vector below 16 is dropped; anything else stops with F559h. |
| ID 0 or 2, INIT (5) or STARTUP (6) | Routed by `route_x2avic_startup`, which admits only the destination or all-excluding-self shorthand (Table 16-4 p644). A router refusal stops with F521h and carries the route predicate. |
| ID 0 or 2, SMI | F557h |
| ID 0 or 2, NMI | F558h. An NMI-IPI stop is useful bug-check evidence. |
| ID 0 or 2, fixed level-triggered | F556h |
| ID 0 or 2, fixed edge, vector below 16 | Dropped: an illegal vector, with no ESR |
| ID 0 or 2, fixed edge, no admitted target | Dropped, with no send-accept error |
| ID 0 or 2, fixed edge | Software fan-out |

ID 2 is handled like ID 0 because hardware wrote nothing for it: 15.29.6.1
steps 3-4 run before the IRR writes of step 5. KVM (informative) also
emulates IDs 0 and 2 fully.

**Target set.** The fan-out matches targets over the admitted inventory, which
has at most 32 CPUs; guest IDs equal host x2APIC IDs.
- **Shorthands.** Shorthand 01 selects self, 10 selects all including self, and
  11 selects all excluding self. A shorthand ignores DEST and DM (Figure 16-18
  p642-644/PDF704-706).
- **Shorthand 00, broadcast.** DEST FFFF_FFFFh selects all CPUs (16.13 p660).
- **Shorthand 00, physical mode.** Compares the 32-bit ID.
- **Shorthand 00, logical mode.** Requires `dest[31:16] == ldr[31:16]` and at
  least one common bit in `dest[15:0] & ldr[15:0]`, where
  `ldr = ((id >> 4) << 16) | (1 << (id & 15))` (16.14 p662/PDF724; 15.29.5.3
  p574/PDF636).
- **Not implemented.** Flat logical mode and lowest priority; x2APIC mode has
  neither.

**`deliver_fixed`** works in two passes:
1. **Validate.** It checks that every remote target is a valid doorbell target.
   If one is not, it stops with F560h and nothing is published.
2. **Publish.** In ascending slot order:
   - it publishes the vector into the slot's backing page through that slot's
     alias. `enqueue` clears TMR and then sets IRR atomically, as a native edge
     interrupt would (16.6.3 p648/PDF710);
   - it rings the doorbell of each remote target (WRMSR C001_011Bh, 15.29.8.2
     p579).
   - The source CPU is not doorbelled; its next VMRUN evaluates IRR (15.29.8.3
     p579).
   - If a backing page refuses the publication (mixed trigger), the fan-out
     stops with F561h. Lower slots have already been published and
     doorbelled.

**Arm order.** Every inventory CPU is armed before any guest can send an IPI.
DXE `physical_boot` completes each AP's arm, entry and ACK before the BSP
callback arms and resumes the loader.

**Drop evidence.** A drop increments the per-CPU saturating `ipi_drops`
counter.
- The counter is exported as field 5 (`incomplete_ipi_drops`) of the event-3
  stop record.
- The per-exit event-1 and event-2 records carry EXITINFO1/2.
- `resident-runtime-test` builds also print a debug line.
- The guest resumes with RIP unchanged.

## Physical interrupts, EOI and level sources

**Bridge invariant.** After every bridge operation on the owning CPU, the
physical ISR equals the set of held level sources.
- Edge sources are acknowledged at capture.
- A level source is acknowledged only after its guest EOI, in physical ISR
  order (16.6.3-16.6.4 p647-652/PDF709-714).
- The ledger (`held`, `completed`) is per CPU. It changes only with host
  interrupt acceptance closed and the guest stopped.

**Capture (exit 60h).** The bounded assembly helper accepts at most one
vector.
- `u32::MAX` means nothing was accepted. The guest resumes, and more than 1024
  consecutive retries stop with F107h.
- A value above 255 stops as F500h.

`irq::capture` then runs these steps:

1. **Check the vector's ISR bit.** It reads all eight physical ISR banks.
   - If the vector's own bit is clear and the vector equals the host SVR vector
     (FFh), this is a physical spurious interrupt: nothing is published and no
     EOI is sent (16.4.7 p640/PDF702).
   - If the bit is clear for any other vector, it stops as F57Bh (the
     ExtINT/8259 signature).
2. **Refuse unsupported cases:**
   - a vector below 32 (F571h);
   - a vector that is not the highest physical ISR bit (F572h);
   - a vector the ledger already holds (F573h);
   - a level source whose vector is already pending or in service virtually
     (F574h).
3. **Publish.** `enqueue` sets TMR, then IRR; a backing-page refusal stops as
   F578h. Then it sends the physical EOI for an edge source, or holds a level
   source.
4. **Drain and synchronize.** It drains completed held sources (bounded to at
   most 225 rounds) and synchronizes the EOI intercept.

`intr` counts captured sources only.

**Dynamic EOI intercept (D6).** Edge EOIs stay accelerated.
- **Set.** While the ledger holds any level source,
  `update_x2apic_eoi_intercept` sets the 80Bh write intercept in this vCPU's
  MSRPM.
- **Clear.** When the ledger empties, acceleration returns.
- **Ownership.** Only the owning CPU edits its private map, and only while its
  guest is stopped.
- **Clean bits.** Every change clears all VMCB clean bits.
- **When.** The update runs after capture, after every completed or faulted
  intercepted MSR access, after the 402h fallback, and inside the INIT commit.

**Software EOI (intercepted 80Bh write of zero).** The steps are:
1. Clear the highest backing ISR bit and recompute PPR (`eoi_stopped`).
2. If that vector is held and not yet completed, mark it completed and drain.
3. Clear the vector's TMR bit unless it is pending again.
4. Complete the WRMSR at its continuation.

An empty ISR completes as a no-op (U18). A completion failure after the ISR
clear stops as F57xh with site 1.

**402h fallback.** An AVIC_NOACCEL exit for a level-triggered EOI write
(EXITINFO2[7:0] = vector; Table 15-29 p582/PDF644) runs `level_eoi_exit`.
- **ISR bit set.** It must be the highest in-service bit; otherwise the path
  stops with F579h. The bit is then cleared.
- **ISR bit clear.** It is left alone (U11).
- **Then** the same completion as a software EOI runs. RIP is never advanced.
- **Reachability.** This path should be unreachable while the intercept is
  active, because TMR is set only for held sources. It becomes reachable only
  if a level device source and an accelerated IPI share a vector.
- **Any other 402h exit** is a D1 mismatch: it stops as F580h, with EXITINFO1
  in info2.

**Why the ledger exists.** The host can accept a higher level source H while
the guest is still servicing a lower level source L.
- The guest's EOI(L) then arrives while H is the highest physical ISR bit.
- A physical EOI at that moment would acknowledge H instead of L.
- The ledger postpones EOI(L) until H completes.
- The resulting source re-arming latency is unmeasured.

## Guest INIT and SIPI (D9)

**Route.** `route_x2avic_startup` publishes a guest INIT IPI (401h, message
type 5) into the target CPU's mailbox.
- The target services it in `service_startup`.
- The route lease is held throughout, and so is the core cache lease when the
  cache owner is in use.
- This replaces the interim startup stage-14 stop.

`validate_x2avic` must return Init. `guest_init` then runs a read-only
preparation followed by an ordered commit.

**Preparation.** These steps are read-only and run before any effect:
1. **LAPIC state.** `registers::prepare_init` checks that the backing ID is at
   most 511, that the version is `0x0005_0010`, and that the physical ISR
   equals the held level set. On failure: stage 10, with `init_error_code`.
2. **EFER.** The EFER owner must be present (otherwise stage 15), and
   `NativeEfer::reset_after_init` must succeed on a copy (otherwise stage 6).
3. **Destination token.** A token for x2APIC mode must be obtained (otherwise
   stage 4).

**Commit, in D9 order:**
1. **LAPIC.** `registers::commit_init` does four things.
   - **Physical timer.** It writes 10000h to the timer LVT, 0 to the initial
     count and 0 to divide.
   - **Physical LVTs.** It writes 10000h to the thermal, performance, LINT0,
     LINT1 and error LVTs.
   - **Level sources.** It marks every held level source completed and
     physically acknowledges it in ISR order (bounded). It then restores EOI
     acceleration.
   - **Backing page.** It calls `BackingPage::reset_after_init_stopped`. This
     sets TPR, APR, PPR, ESR, ICR, ISR, TMR, IRR, the counts and divide to 0;
     sets SVR to FFh and all six LVTs to 10000h; keeps ID and version; and
     loads the derived LDR (U5; Table 16-2 p631).
   - **Failure.** A failure is terminal: stage 11, with `init_error_code`.
2. **CPU state.** `NativeStartupTarget::apply_x2avic(Init)` applies the CPU INIT
   state.
   - `initialize_ap_after_init` keeps only V_INTR_CONTROL bit 24 and bits 31:30,
     so V_TPR becomes 0, and it invalidates every clean bit.
   - A failure is terminal (stage 12). It is unreachable after the identical
     validation.
3. **EFER.** The prepared EFER value is committed.
4. **Destination.** The destination commit runs with cause `GuestInit`; the
   mode stays x2APIC.
5. **Debug registers.** `svmvisor_resident_reset_guest_debug()` resets DR0-3.
   The linked image calls it, and the debug-register audit in `build.py`
   requires that call.
6. **Mailbox.** `mailbox.complete(command)` runs last. A failure is stage 8.

**Unchanged and edge cases.**
- The APIC_BASE shadow does not change (16.10 p657).
- SIPI handling is unchanged. AwaitSipi still polls on the host and never
  enters the guest.
- **IRR races.** Remote IRR publications that race the backing reset are
  settled by its per-bank clears.
  - A publication that lands before its bank's clear is discarded.
  - One that lands after the clear stays pending across SIPI. This matches the
    INIT state in 16.5, where other interrupts are "held pending" (p643/PDF705).
- **Level lines.** After the retirement drain, a level line that is still
  asserted can deliver again later.

## Arm admission and captured register state

`arm` refuses at the first failing step below; the number in parentheses is
the return code. DXE reports every untyped arm failure as activation failure
20.

1. **Earlier checks:** runtime state and CPU identity (1); capabilities (2);
   the terminal endpoint (10); an inventory of 1-32 IDs that matches the pool
   slot count (7); the cache replay observation (12); `NativeIcr::admit` (7).
2. **`X2AvicCapabilities::admit`** (8).
3. **`GuestX2Apic::admit`** checks the physical APIC_BASE (8). The loader must
   already run enabled x2APIC at FEE0_0000h; an xAPIC continuation is refused,
   never promoted.
4. **Host IDs** (8). Every admitted ID must be a `DoorbellTarget` (at most 254,
   which also bounds the table index), and the physical ID MSR must equal the
   assigned ID.
5. **`NativeX2AvicProfile::new`** (8).
6. **Inherited ISR** (8). A `HostX2Apic` is created; any inherited physical ISR
   bit refuses.
7. **ICR readback** (8 on an invalid pointer). This is either the BSP's value
   saved before the physical bootstrap overwrote it, or the physical ICR.
8. **`CapturedInterface::capture`**, read-only (new code 11).
9. **Remaining steps:** terminal endpoint preparation (10); EFER (3); ACK
   sites (4); the memory map (6); the unconditional D1 MSRPM profile; cache MSR
   intercepts (12); VMCB controls (5); startup ownership with at least two CPUs
   (9); V_TPR seeded with the captured TPR >> 4 (9); x2AVIC enable and mailbox
   identities (9); the route lease, destination preparation and VM_CR R_INIT
   (9).

**Install.** After the VM_CR and destination commits:
1. `interface.install` stores TPR, PPR (= TPR), SVR, the six LVTs, the counts,
   divide and ICR into the backing page. It writes a physical LVT only where the
   mirror adds a mask.
2. The host writes physical TPR 0 and SVR 1FFh through `HostX2Apic`.
3. IsRunning is set last; a failure here returns code 9.

`CapturedInterface` applies the guest-write model to the loader's state:
- read-only DS and RIR bits are dropped (U14);
- a reserved bit refuses arm (11). The reserved bits are:
  - TPR 63:8;
  - SVR 63:10;
  - timer 63:18, 15:13 and 11:8;
  - thermal, perf and error 63:17, 15:13 and 11;
  - LINT 63:17, 13 and 11;
  - initial count 63:32;
  - divide 63:4 and bit 2;
  - ICR 31:20, 17:16 and 13;
- a reserved LVT message type refuses arm (11);
- a captured SVR with bit 8 clear forces every LVT mask (16.3.1 p629);
- an unmasked fixed entry with vector 0-15 is stored as captured; only its
  physical mirror is masked;
- unmasked ExtINT LINT and unmasked SMI entries stay as captured and live
  physically;
- ICR bit 12 is dropped, not refused;
- the counts and divide are never rewritten physically.

## Stop reasons, startup stages and arm codes

x2AVIC stops use the low 16 bits of the stopped `info1` as the tag; bits 63:16
hold the detail. `terminal::X2AvicStop` owns the tags.
- The event-3 stop record carries both words, as `reason_info1` and
  `reason_info2`. So does the terminal context export (extension part 0).
- The per-CPU `stop_words` export sends F5xxh stops as kind 0: the exit code
  plus the guest RIP.
- `read_snapshot.py` prints the raw words. Decode them with this table.

| Tag | Meaning | Detail (`info1` bits 63:16) | `info2` |
| --- | --- | --- | --- |
| F500h (kept) | The acceptance helper returned a value above 255 | 0 | the value |
| F510h (kept; subcodes redefined) | MSR boundary | 0 | 0: no profile or guest APIC owner. 1: profile or pending event. 2: instruction evidence. 3: mode or TF. 4: #GP not queued. |
| F520h (kept) | Profile mismatch | 0 | 0: profile or IPI owner missing. 1: AVIC exit with a changed profile. 2: dispatch entry with a changed profile. |
| F521h (detail new) | The startup router refused an INIT/SIPI | `NativeRoutePredicate` (0 = none recorded) | EXITINFO1 |
| F541h-F546h (new) | Register refusal. 1: unowned access. 2: unsupported LVT message type. 3: unmasked SMI LVT. 4: unmasked ExtINT LINT. 5: APIC disable. 6: APIC relocation. | MSR index; `info1` bit 48 set for WRMSR | the refused value (0 for RDMSR) |
| F551h-F559h (new) | 401h refusal. 1: target not running (ID 1). 2: invalid backing page (ID 3). 3: unknown ID. 4: reserved ICR bits. 5: reserved message type. 6: level-triggered fixed IPI. 7: SMI. 8: NMI. 9: inconsistent ID 4. | EXITINFO2 index (`info1` bits 27:16) and ID (bits 59:28) | EXITINFO1, raw (bit 12 included) |
| F560h (new) | Fan-out: a remote target ID is not a doorbell target. Nothing was published. | slot, and ID << 8 | EXITINFO1 |
| F561h (new) | Fan-out: a publication was refused. Lower slots were already published and doorbelled. | slot, and `x2avic_error_code` << 8 | EXITINFO1 |
| F571h-F57Bh (new) | Host IRQ bridge error; the low nibble is the variant (list below) | site. 0: capture (60h). 1: software EOI (7Ch). 2: level-EOI exit (402h). | `irq_error_code` |
| F580h (new) | AVIC_NOACCEL outside D1 | EXITINFO2[31:0] | EXITINFO1 |
| F581h (new) | Undecodable AVIC exit (ID of 5 or more, or an EOI vector below 16) | EXITINFO2[31:0] | EXITINFO1 |

**Operand encodings:**
- **Bridge variants:**
  - 1: reserved vector;
  - 2: physical ISR mismatch;
  - 3: duplicate physical source;
  - 4: ambiguous level source;
  - 5: unowned level completion;
  - 6: completion not ready;
  - 7: unexpected physical ISR;
  - 8: virtual publication refused;
  - 9: virtual ISR mismatch;
  - 10: drain incomplete;
  - 11 (F57Bh): accepted vector not in the physical ISR, i.e. the ExtINT/8259
    signature.
- **`irq_error_code`:**
  - bits 20:17: the variant;
  - bits 16:8: the second vector (100h = none);
  - bits 7:0: the vector (0 for variant 10).
- **`x2avic_error_code`:**
  - 1: missing capability;
  - 2: address;
  - 3: invalid ID;
  - 4: aliased pages;
  - 5: occupied;
  - 6: invalid offset;
  - 7: invalid vector;
  - 8: mixed trigger;
  - 9: unsupported version;
  - 10: invalid exit;
  - 11: unsupported APIC_BASE.
- **`init_error_code`:** bits 31:28 give the source.
  - 1: backing page; `x2avic_error_code` is in bits 7:0.
  - 2: host IRQ bridge; `irq_error_code` is in bits 20:0.

**Retired tags (never reused):**
- The terminal unit test asserts that these are unused: F501h-F504h (the old
  capture steps), F511h (APIC_BASE change), F522h (level completion), and
  F530h/F531h (the old register backend).
- Phase A also removed F505h (route check) and F523h/F524h (level-route
  branch). No code uses them, but the test does not cover them.

Startup service stages are reason 7 of `startup_failure`: tag F10Ch, exported
as terminal kind 15. `terminal::StartupStage` and `read_snapshot.py` name them.
While the value is at most 32 bits, the target APIC ID stays in the export.

| Stage | Meaning | Value |
| --- | --- | --- |
| 1, 2, 5, 8, 9 | Unchanged: INIT acknowledgment, route table, target application, mailbox completion, wait exhausted | AwaitSipi flag |
| 4 | Destination token refused | flag |
| 6 | EFER owner refused its INIT value; for guest INIT this is now checked during preparation | flag |
| 10 (new) | Guest INIT LAPIC preparation refused; nothing changed | `init_error_code` |
| 11 (new) | Guest INIT LAPIC commit failed after the physical reset (terminal) | `init_error_code` |
| 12 (new) | CPU INIT commit refused after the LAPIC commit (terminal) | flag |
| 13 | Cache replay. The stage already existed; the decoder now accepts it. | flag |
| 14 | Retired: eafe33a's guest-INIT refusal and missing-profile stop. The decoder keeps the name; it is not to be reused. | none |
| 15 (new) | An owner that arm always installs is missing | flag |
| 3, 7 | Retired xAPIC-era stages (current mode, ICR reset). The decoder keeps the names; they are not to be reused. | none |

No `StartupStage` value emits stage 3, 7 or 14. Unlike the retired stop tags
above, no test asserts this.

Arm return codes:
- 0: armed;
- 1: runtime state or CPU identity;
- 2: CPU capabilities;
- 3: EFER;
- 4: bootstrap ACK sites;
- 5: VMCB controls;
- 6: memory map;
- 7: CPU inventory;
- 8: x2APIC/x2AVIC admission. This now also covers host IDs above 254 and an
  inherited physical ISR.
- 9: startup ownership and its commit;
- 10: terminal endpoint;
- 11 (new): the captured x2APIC register state is outside the guest model;
- 12: cache replay.

## Validation actually performed

All results were measured on the Windows development host. They are host tests
and linked-image audits only: nothing was flashed, and no native execution,
timing, interrupt loss, IPI latency or Windows boot was measured.

The final tree was built in the B-int phase with the private target directory
`target/agent-b-int`.

| Command | Result |
| --- | --- |
| `cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc` | 66 test binaries (67 result lines with doctests); 450 passed, 0 failed, 0 warnings |
| same with `--features resident-runtime --lib` | 63 passed |
| same with `--features resident-runtime-test --lib` | 59 passed (the terminal-return tests are excluded under this feature) |
| `cargo check --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc`, with `--features resident-runtime` and then `resident-runtime-test` | pass, 0 warnings |
| `cargo check --locked -p svmvisor-hypervisor --target x86_64-unknown-uefi` | pass, 0 warnings |
| `cargo clippy ... --features resident-runtime --tests` (extra check) | No findings in the new code. The pre-existing `runtime.rs`/`terminal.rs` findings remain. |
| `cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-returning` | 29 binaries, 303 passed, 0 warnings |
| same with `card-returning-loader` / `memory-attribute-probe` | 26 / 59 passed |
| same with `native-preflight` / `native-resident-boot` / `native-resident-low-runtime` | 215 passed each |
| `cargo test --locked -p svmvisor-memory-attributes` | 29 passed |
| `cargo build-dxe` | pass, 0 warnings |
| `python -m unittest discover -s tools/native-resident -p "test_*.py"` | 11 OK |
| `python -m unittest discover -s tools/native-stack-audit -p "test_*.py"` | 42 OK |
| `python -m unittest discover -s firmware/squirrel -p "test_*.py"` | 81 OK, 1 skipped (the env-gated vector test). This record's pass reran the command with the same result. |
| `test_startup_diagnostics`, with `SVMVISOR_REFUSAL_VECTORS` produced by `tests/native_refusal_wire.rs` | 4 OK. Two stale expectations from before B-int were updated: since eafe33a the Rust generator emits 12 vectors (19 were expected), and the recipient mode is x2APIC (`extended_xapic_4bit` was expected). The old stage-10-invalid check became a stage-16 check. |

**Hypervisor pass counts by phase:**

| Phase | Tests passed |
| --- | --- |
| A | 402 |
| B-layout | 404 |
| B-core (measured with the concurrent B-layout edits present) | 445 |
| B-int (final) | 450 |

**New or rewritten tests:**
- `tests/x2avic_registers.rs` (new, 29 tests): the D1 profile, D2-D4 emulation,
  mirroring, the INIT LAPIC half, the ExtINT signature, and the captured
  interface.
- `tests/x2avic_ipi.rs` (new, 11 tests): the inventory, 401h policy, target
  sets and fan-out.
- `tests/x2avic.rs` (13 tests, 4 new and 1 corrected decode case): IRQ ledger
  ordering, retirement, INIT reset, and the INIT CPU commit.
- `tests/permission_maps.rs` (7 tests): the profile test was rewritten to D1,
  and an EOI-intercept toggle test was added.
- `tests/resident_layout.rs` (new): the layout constants, the `payload.ld`
  literal and the ID limit.
- Runtime `x2avic_glue_tests` (7 tests):
  - MSR outcome mapping;
  - the 401h plan with the bit-12 tolerance;
  - the backing ICR bit-12 clear;
  - the AVIC exit plan;
  - guest INIT D9 order and results;
  - four refused preparations with no effect;
  - a terminal LAPIC-commit failure that leaves the CPU, EFER and backing page
    untouched.
- The terminal unit test (1 test): encodings, distinct tags, unused retired
  tags, and export kinds.
- DXE `tests/native_resident_launch.rs`: the image bound, the ID limit, the
  alias plan, pool disagreement, and refusal of version 9.
- `firmware/squirrel/test_startup_diagnostics.py`: the names and decoding of
  stages 10-15.

**Payload build.** The command was
`python tools/native-resident/build.py --output work/x2avic-batch-2026-09-16/b-int-build --boot --low-runtime`.
It passed completely:
- the payload linked under the `0x1d4000` ASSERT;
- relocation packaging passed;
- the undefined-symbol check found 0 symbols;
- `audit_debug_reset` passed. The symbol body is
  `xorl %eax,%eax; movq %rax,%dr0..dr3; retq`, and there are no other
  debug-register instructions;
- the host-fault audit covered 256 vectors;
- no FP, SIMD or xstate instruction was found;
- the DXE UEFI build passed (PE subsystem 12), with 0 warnings in
  `dxe-cargo.log` and `payload-cargo.log`;
- the physical and boot audits passed.

**Image metrics:**
- Linked instructions: 25,143 (B-layout 23,129; Phase A 23,071; HEAD 23,574).
- Layout: `text_end` 0x11a000, `data_start` 0x11c000, `image_load_end`
  0x123000, `image_bss_end` 0x17c000, `AVIC_BACKING` 0x12b000,
  `svmvisor_resident_reset_guest_debug` 0x1191b0.
- Headroom below 0x1d4000: 88 pages.

This record rehashed the artifacts on 2026-09-16, and they match the notes:

| Artifact | SHA256 |
| --- | --- |
| `payload.bin` | `dd3b1a5fe4637d273b59a84f61dd38c820103c8cc36125f6519b75b904608b20` |
| `payload.reloc` | `eb74c167413f7e39bca27263aadd91a281b476a6e8c56030f11f57dd86395be9` |
| `payload.elf` | `dfe88b2b80a1ea9c8dbb8d4960fb0ef6b83cbf4151319256206183d795c5bf0b` |
| `driver.efi` | `6193197af3a805a0f1f9d86ca31a81fdd7808432c338b94fc3cee70c033ee542` |
| `source-manifest.json` (265 entries) | `cc7c67028dd1e8ea149c9bf56136f8d6fabe1bbe11187396199ca1c9431d5a27` |
| `disassembly.log` | `7bcc8933d327d74f6cb7b95119731fe3c5cc47c001e60a02c8464de1b09557fd` |

**Manifest and superseded build.**
- The integration phase checked the manifest against the tree after its final
  edits. The only later edits were in `firmware/squirrel`, which the manifest
  does not cover.
- Any later source change invalidates these hashes.
- `b-int-build-superseded-1/` is an earlier build that also passed, from before
  the PPR-at-install fix and the clippy cleanups. It is not current evidence.

**Stack audit.** The command was
`python tools/native-stack-audit/run.py --output work/x2avic-batch-2026-09-16/b-int-stack-audit`,
on the native-returning image. It passed:
- maximum depth: 1560 bytes below the sampled RSP;
- coverage margin: 63,976 bytes;
- PE SHA256: `c6044193ffdee3a5841ede2d51c247db00aa38e09982d4745d50edce18f16a40`;
- source manifest: 241 entries, checked against the tree;
- build log: the same 7 pre-existing dead-code warnings as the Phase A
  baseline.

**Earlier trees.** These results are from earlier phases and were not repeated
on the final tree.
- **Phase A extra host profiles:**
  - `card-load-only`: 21;
  - `card-resident-loader`: 17;
  - `memory-attribute-f7`: 236;
  - `native-transition-test` and `native-transition-multi-exit`: 220 each.
- **Phase A, other checks:**
  - firmware-handoff: 15 tests;
  - the returning-probe UEFI check;
  - UEFI `cargo check` of the three card profiles (with dummy pins).
- **B-layout:**
  - DXE UEFI `cargo check` for `native-resident-boot`, with and without
    `native-resident-low-runtime` (0 warnings), and for the legacy layered
    profiles, whose warnings are pre-existing.
  - A release DXE build with its payload (PE subsystem 12).

## Unsupported, unmeasured and known gaps

Each item below is either a stopped refusal with evidence, or behavior that is
not established.

**Unsupported (stopped with evidence):**
- **IPIs:** NMI (F558h), SMI (F557h), lowest-priority and ExtINT (reserved in
  x2APIC mode; F555h), and level-triggered (F556h).
- **Guest APIC_BASE:** disabling the guest APIC (F545h, a documented deviation)
  or relocating it (F546h).
- **LVTs:**
  - unmasked SMI LVT writes (F543h);
  - unmasked ExtINT LINT writes (F544h);
  - reserved LVT message types: F542h for a write, arm code 11 when captured.
- **Host NMIs.** A host-mode NMI during a host GIF window is terminal (existing
  behavior).
- **Virtual ESR errors.** Virtual APIC error generation is not modeled.
  - ESR always reads 0.
  - An illegal-vector LVT is only masked physically.
  - Illegal-vector and no-target fixed IPIs are dropped. Only
    `incomplete_ipi_drops` counts them.
- **Physical vectors below 32.** Physical sources with such vectors are refused
  at capture (F571h).
- **Vector sharing.** Arbitrary mixed-trigger vector sharing is unsupported
  (F574h, F578h, F561h).
- **IOMMU.** IOMMU GA posting is not implemented. Windows owns the physical
  IOMMU.
- **Device routes.** No device-route owner exists. Guest-programmed
  IOAPIC/MSI routes reach the physical fabric unchanged. Physical
  lowest-priority selection uses physical priority state, not the virtual PPR
  (see the historical
  [delivery review](x2avic-interrupt-delivery-2026-09-16.md)).

**Architectural dependencies that are recorded but unverified:**
- **HLT wake.** Guest HLT runs natively with IsRunning=1. Waking depends on
  doorbell or INTR delivery to a halted guest (AVIC U16).
- **Doorbell to a host-mode core.** The manual leaves this unspecified (AVIC
  U5). The design relies on it being harmless.
- **MSRPM caching.** The owning CPU edits its map only while its guest is
  stopped, then calls `invalidate_all`. Figure 15-4 does not say whether VMRUN
  caches map contents.
- **Hardware checks on accelerated accesses.** The APM does not say which
  chapter-16 checks x2AVIC hardware applies to the accelerated writes (TPR,
  ICR, SELF IPI, edge EOI) or to the allowed reads (AVIC U12). This is
  unmeasured.
- **Pending interrupt at INIT (deviation).** A physical interrupt pending at
  INIT is published into the reset page after the next VMRUN.
- **ExtINT early in boot.** Firmware may leave LINT0 as an unmasked ExtINT
  while the 8259 is active or can raise a spurious IRQ7/IRQ15.
  - In that case, the first acceptance stops as F57Bh. The vector is the PIC
    base + 7 or + 15. EDK2 uses base 68h; the AMI base is unverified.
  - The captured MADT sets PCAT_COMPAT. ACPI requires the 8259 to be masked
    when APIC operation is enabled (Table 5.20 p134/PDF205).
  - This is a plausible early-boot stop.
- **402h with true fault semantics (AVIC U11).** Suppose hardware reports the
  level EOI as a fault, with RIP still at the WRMSR and the ISR bit still set.
  The software EOI plus the resume would then re-execute the WRMSR: a silent
  double EOI. This needs a vector shared between a level device source and an
  accelerated IPI. KVM (informative) treats the exit as a trap.
- **Same-vector race.** The `set_trigger` check is not atomic across CPUs. A
  remote edge publication can therefore race a local level capture of the same
  vector.
- **Partial fan-out.** F561h leaves a partial fan-out: lower slots were already
  published and doorbelled.
- **ICR busy flag.** Tolerating ICR bit 12 is based on informative KVM
  behavior. It has not been observed on this machine.
- **Loader handoff after the BIOS change.** The loader must hand off in x2APIC
  mode, and this is unmeasured.

**Unmeasured:**
- native execution of any of these paths;
- 401h, 402h and EOI interception behavior;
- V_TPR reconciliation;
- exit frequency;
- IPI and interrupt latency;
- interrupt loss;
- source re-arming delay caused by the ledger;
- timer accuracy;
- Windows boot;
- Hyper-V/VBS/HVCI coexistence.

No Windows protection was changed. A successful boot would not establish
sandbox readiness, containment or undetectability.

**Known gaps (noted, not changed):**
- DXE reports arm code 11 (like every other untyped arm code) as activation
  failure 20. No typed export identifies the captured register that failed.
- `stop_words` exports F5xxh stops as kind 0 only. The full words exist only in
  the event-3 record and the terminal context export.
- The kind-14 `terminal::route_failure` format cannot express exit 401h,
  because its bit 4 means a 7Ch exit. The runtime therefore records the route
  predicate in F521h instead.
- `dispatch_body` stops "not armed" with F10Bh/0. `stop_words` treats that as a
  malformed route record, so a terminal export reports failure 4.
- `IpiRefusal::UnknownReason` is unreachable.
- Cross-crate calls in the payload, including the debug helper, go through
  GOTPCREL slots inside the image's RW data. This is the existing pattern, and
  the audits pass.

## Hardware bring-up checklist

Complete these steps before the next physical attempt.

1. **BIOS x2APIC.** Enable BIOS x2APIC and confirm the setting.
   - The user reported enabling it for the next reboot. Nothing has been
     observed since.
   - The earlier CPUID sample (`work/x2avic-2026-09-16/live-cpuid.json`,
     02:10Z) and the ACPI tables (02:23Z) both predate the change.
2. **Fresh per-CPU capture, after the change and before flashing:**
   - **x2APIC bit.** CPUID Fn0000_0001 ECX[21], pinned to each logical CPU.
     The earlier sample came from a single unpinned CPU and showed
     `ECX=7ED8320B`, bit 21 clear.
   - **SVM bits.** CPUID Fn8000_000A EDX[13] (AVIC) and EDX[18] (x2AVIC).
     Admission also needs EDX[0] (NPT). The earlier `EDX=FEBFBDFF` had all
     three set.
   - **APIC_BASE (MSR 1Bh).** Admission requires `FEE0_0C00h`, or
     `FEE0_0D00h` with BSC set on the BSP.
     - Windows user mode cannot read this MSR.
     - Without an authorized privileged reader, rely on the image's own
       admission.
   - **x2APIC IDs.** All IDs must be at most 254, and there must be between 2
     and 32 CPUs. Before the change there were 24, with IDs {0-11, 16-27}.
3. **MADT after the change.** Copy `work/x2avic-2026-09-16/read-acpi.ps1` into
   a new, dated directory and run it there. It saves IVRS, APIC, MCFG and FACP
   into `acpi/` next to itself, so running it in place would overwrite the
   pre-change capture. Then decode the MADT with
   `python work/x2avic-manual-2026-09-16/acpi-madt/decode_madt.py <dir>/acpi/APIC.bin --json <dir>/madt-decoded.json`
   and check:
   - **Processor entries: type 0 or type 9.**
     - ACPI allows type 0 for IDs below 255 even in x2APIC mode
       (5.2.12.12 p142/PDF213).
     - Whether Windows accepts type 0 entries while it uses x2APIC is OS
       behavior, and remains unresolved.
     - Before the change, all entries were type 0: 24 enabled, plus 8 unusable
       entries for UIDs 24-31.
   - **LINT NMI entry.** Is it type 4 or 0xA (Table 5.35 p143/PDF214), and
     which LINT does it name?
     - Before the change: one type 4 entry, UID 0xFF, LINT1, flags 0x0005.
     - Windows is expected to program the named LINT as an unmasked NMI LVT,
       which D3 mirrors.
   - **Other fields.** Enabled IDs and UIDs, and PCAT_COMPAT (1 before the
     change).
   - **IOMMU XTSup.** IOMMU XTSup is no longer an admission requirement in this
     batch: the boot image neither reads IVRS nor programs the IOMMU.
     `check_iommu.py` can still record the value as platform evidence, but its
     exit status gates nothing. Windows owns that programming. The
     specification's system-x2APIC requirements are in the historical
     [IOMMU review](x2avic-iommu-owner-2026-09-16.md).
4. **Build and audit.**
   - Build with
     `python tools/native-resident/build.py --output work/<fresh> --boot --low-runtime`.
     Expect every audit to pass, as in `b-int-build`.
   - Record the build's `summary.json` and artifact hashes. The `b-int-build`
     hashes describe only that tree.
   - Run the stack audit into a fresh directory, and the host suites listed in
     [the README](../README.md#build-and-test).
5. **Flash.** Use only the existing documented procedure.
   - Package the card and check the routed FPGA as documented in
     [firmware/squirrel](../firmware/squirrel/README.md).
   - Follow the session-bound programmer procedure: predecessor backups, full
     5 MiB readback and the restore helper. Its last use is recorded in
     [the fault-capture delivery evidence](handoff-evidence/2026-09-16-fault-capture/).
   - **Snapshot kit.** Refresh `D:/SVMvisor-Snapshot-Kit` with the new image IDs
     and with the current `firmware/squirrel/read_snapshot.py`.
     - A hash comparison on 2026-09-16 showed that the kit's reader predates
       this batch's decoder changes.
     - Replace any remote copy of the kit.
   - **Power cycle.** A full power-off/power-on is required for activation.
6. **What to look for in the capture:**
   - **Before activation.**
     - If x2APIC is not enabled, the BSP refuses during preparation. Its
       pre-Windows card-journal record (`resident_preparation_v2`) names stage
       28 (`bootstrap_apic_base`) or stage 26 (`bootstrap_apic_capability`),
       with the observed APIC_BASE or CPUID.1:ECX.
     - An untyped arm refusal (codes 1-12, including 8 and 11) appears only as
       activation failure 20.
   - **Stop tags (F5xxh)** in the event-3 `reason_info1`/`reason_info2` fields
     and in terminal extension part 0:
     - F541h-F546h: register refusals, with the MSR index and value;
     - F551h-F559h: IPI refusals, with the raw ICR;
     - F560h/F561h: fan-out failures;
     - F571h-F57Bh: bridge errors, with site 0, 1 or 2;
     - F580h/F581h: AVIC exit mismatches.
     - Retired tags must not appear.
   - **F57Bh.** An accepted vector with no physical ISR bit, i.e. the
     ExtINT/8259 signature. `info2` bits 7:0 give the vector: the PIC base
     plus the IRQ, for example base + 7 or base + 15 for a spurious IRQ.
   - **Kind-15 startup stops.**
     - Stage 10: guest INIT refused; nothing changed.
     - Stages 11 and 12: terminal INIT commit failures.
     - Stage 15: a missing owner.
     - For stages 10 and 11, the value's high nibble is 1 (backing page) or 2
       (bridge).
     - Stage 14 must not appear.
   - **Drop counter.** A non-zero event-3 `incomplete_ipi_drops` means
     illegal-vector or no-target fixed IPIs were dropped.

## Review status

The following reviews are in progress:
- an independent manual-conformance review;
- a code review;
- independent tests derived from the manuals.

The coordinator will record their results here. Until then, this record
reflects the implementers' phase notes and this documentation pass's checks
against the source, the build artifacts and the reference hashes. It is not
independent confirmation.
