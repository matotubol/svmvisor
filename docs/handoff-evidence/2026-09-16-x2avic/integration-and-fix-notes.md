# Phase B-INTEGRATION notes: x2APIC/x2AVIC wiring in the resident runtime

Date: 2026-09-16. Scratch record (work/ is gitignored). Working tree on `main`
(HEAD eafe33a plus Phase A, B-core and B-layout). No commits, no flashing, no
firmware execution. Private cargo target dir: `target/agent-b-int`. No docs/ or
README file was edited (stale statements are listed in section 7).

Manual evidence: every rule cited below comes from the fact sheets in
`work/x2avic-manual-2026-09-16/` (rendered page images; APM2 24593 rev3.44
`3d9dcb3f...c48c`, PPR 57896 rev3.00 `643cae09...17e5`). No new manual pages
were needed. Linux KVM is cited as informative only.

## Files changed in this phase

| File | Change |
| --- | --- |
| `crates/hypervisor/src/host/resident/runtime.rs` | arm admission and captured interface; MSR, INTR, 401h and 402h handlers; guest INIT (D9); `remote_backing`; EOI intercept sync; `State` owners; drop counter; unit tests |
| `crates/hypervisor/src/host/resident/terminal.rs` | `X2AvicStop`, `IrqSite`, `StartupStage`; stop encoders (`register_refusal`, `ipi_refusal`, `fan_out_failure`, `irq_failure`, `avic_exit_refusal`, `irq_error_code`, `x2avic_error_code`, `init_error_code`); encoder test |
| `crates/hypervisor/src/svm/x2avic/registers.rs` | `Lvt::check` (the shared LVT rule); `CapturedInterface` and `CaptureRefusal` |
| `crates/hypervisor/src/svm/x2avic/irq.rs` | `IrqError::NotInService`; `capture` classifies by the vector's own physical ISR bit |
| `crates/hypervisor/src/svm/x2avic/ipi.rs` | uses the shared `apic::ICR_RESERVED` |
| `crates/hypervisor/src/svm/x2avic/mod.rs` | module doc |
| `crates/hypervisor/src/arch/x86_64/apic.rs` | deletions (section 3); `ICR_RESERVED`, `ICR_DELIVERY_STATUS` |
| `crates/hypervisor/tests/x2avic_registers.rs` | 4 new tests (ExtINT signature, captured interface x3) |
| `firmware/squirrel/read_snapshot.py` | startup service stages 10-15 named and decoded; stop record field 5 is `incomplete_ipi_drops` |
| `firmware/squirrel/test_startup_diagnostics.py` | stage tests; stale env-gated vector expectations fixed (section 5) |

`runtime.S`, `irq.S`, `build.py`, `payload.ld` and every DXE file are
unchanged.

## 1. Runtime paths and their decisions

### arm (D1, D2/D3 applied to captured state, D4, D8)

- `State.apic_base` is gone. `State.guest_apic: Option<GuestX2Apic>` is the
  guest APIC_BASE shadow (D4). `State.host_apic_base` is the captured
  *physical* APIC_BASE, used only by `terminal_finish` for its physical
  recheck. Both come from the same RDMSR, but they are separate owners.
- Admission order:
  1. `X2AvicCapabilities::admit`.
  2. `GuestX2Apic::admit` (enabled x2APIC at FEE0_0000h, width-derived reserved mask).
  3. `DoorbellTarget::new(id)` for every admitted ID (254 or less; this also
     bounds the table index), then the ID MSR check.
  4. `NativeX2AvicProfile::new`.
  5. `HostX2Apic::new()`, then `apic::highest_in_service` (inherited ISR refuses, code 8).
  6. The ICR readback: the saved BSP value, or the physical ICR.
  7. `CapturedInterface::capture` (read-only). A refusal returns the new code 11.
- `configure_native_x2avic()` is now unconditional (D1). The `id_count > 1`
  guard and the `state.icr = None` branch were dead, since arm refuses fewer
  than two CPUs.
- V_TPR is seeded from `interface.task_priority() >> 4`.
- After the VM_CR and destination commits:
  1. `interface.install(backing, &mut host)` stores TPR, PPR (= TPR),
     SVR, the LVTs, the counts, divide and ICR, and writes a physical LVT only
     where the mirror adds a mask.
  2. The host-owned physical TPR and SVR writes, now through `HostX2Apic`.
  3. IsRunning.

`CapturedInterface` (registers.rs) applies the guest-write model to the loader's
state:
- Read-only DS/RIR bits are dropped (decision U14, PPR p27 Table 8).
- Any reserved bit refuses (code 11): TPR 63:8; SVR 63:10; timer 63:18, 15:13
  and 11:8; thermal/perf/error 63:17, 15:13 and 11; LINT 63:17, 13 and 11;
  initial count 63:32; divide 63:4 and bit 2; ICR 31:20, 17:16 and 13.
- A reserved LVT message type refuses (code 11). This is a decision: Figure
  16-7 p635 lists only four legal types, and PPR p55 says "all other message
  types are Reserved".
- Captured SVR bit 8 = 0 forces every LVT mask (16.3.1 p629).
- An unmasked fixed entry with vector 0-15 is stored as captured, and only its
  physical mirror is masked (Figure 16-7 p635).
- An unmasked ExtINT LINT and an unmasked SMI entry stay as captured, and stay
  live physically (coordinator instruction; see section 4).
- ICR bit 12 is dropped, not refused (section 4).
- Counts and divide are never rewritten physically: a count write restarts the
  timer (16.4.1 p636).
- PPR = TPR: the never-entered page has an empty ISR (16.6.4 p651), and AVIC
  uses the backing PPR to gate delivery (15.29.3.1 p569). The previous arm left
  PPR at 0 even with a non-zero captured TPR.

### VMEXIT_MSR (7Ch) for 800h-8FFh and APIC_BASE (D1-D4, D6)

`handle_avic_msr`:
1. Profile, guest APIC owner and pending-event checks.
2. Instruction evidence. Hardware nRIP is used in 64-bit code with NRIPS
   (15.7.1 p509). The byte fetch stays for guest code outside 64-bit mode
   (AP startup trampolines) or without NRIPS; this is the same rule as the
   EFER/VM_CR owners, so it still has a real use.
3. Continuation, instruction mode and TF checks.
4. A CPL above 0 gives #GP.
5. Otherwise `GuestX2Apic::emulate(index, write, backing, &mut state.irq, &mut HostX2Apic)`.

The pure `msr_completion` maps each outcome:
- `Read(v)`: RAX = v[31:0], RDX = v >> 32, commit nRIP once.
- `Written`: RAX unchanged, commit nRIP once.
- `GeneralProtection`: `queue_native_x2avic_general_protection`, then
  `pending_fault`; RIP is unchanged.
- `Refused` / `EoiFailed`: stop with a typed reason (section 2).

After every completed or faulted access, `sync_eoi_intercept` calls
`Msrpm::update_x2apic_eoi_intercept` and `vmcb.invalidate_all()` when the map
changed. The old value match, the 840h-8FFh branch and the 0xf511
equal-value rule were removed.

### Physical INTR (60h), host IRQ bridge (user decision 1, D6)

`capture_physical_irq` keeps the bounded acceptance helper: `u32::MAX` retries,
and a value above 255 stops (F500h). It then calls `irq::capture` (spurious,
TMR, publish, edge EOI or level hold, bounded drain) and `sync_eoi_intercept`.
`intr` counts only captured sources.

`irq::capture` now decides spurious versus not-in-service from the vector's
*own* physical ISR bit:
- bit clear and the host SVR vector: spurious, `Ok(None)` (16.4.7 p640);
- bit clear and any other vector: `IrqError::NotInService`, stop F57Bh. This is
  the ExtINT/8259 signature (16.6.3 p647: ExtINT goes directly to the core).

The duplicate, unreachable `0x60` match arm in `dispatch_body` was removed; the
early 60h return precedes ACK handling, as before.

### AVIC_INCOMPLETE_IPI (401h) (D5, D7, D8, bit-12 override)

`avic_exit_plan` decodes the exit, and `handle_incomplete_ipi` applies
`incomplete_ipi_plan`, which is `Inventory::classify` on EXITINFO1 with bit 12
masked:
- `Startup(icr)`: `route_x2avic_startup` plus the private notification. A
  refusal stops with F521h; the detail is now the recorded route predicate.
- `Fixed`: `deliver_fixed(ipi, |slot| remote_backing(slot), |target| ring_avic_doorbell(target))`.
  Fan-out errors stop with F560h/F561h.
- `Dropped`: `State.ipi_drops` is incremented, a debug line is printed
  (resident-runtime-test only), and the guest resumes.
- A refusal stops with F55xh; `info2` is the raw EXITINFO1.

On every resume path, `clear_icr_delivery_status` clears bit 12 of the backing
ICR low word (300h). The exit is a trap (Table 15-22 p567): RIP is never
changed and nRIP is never used.

`remote_backing(slot)` sits next to its only caller: `image_start +
X2AVIC_BACKING_ALIASES_OFFSET + slot*4096`, `unsafe`, valid only under the
private root and for slots below the armed pool count. The inventory only
resolves slots below `state.count`, and arm checks `id_count == POOL/1MiB`.
The own slot also goes through its alias, which maps the same physical page
(B-layout maps and DXE walks every slot).

### AVIC_NOACCEL (402h) (D1, D6)

- A level-triggered EOI write goes to `irq::level_eoi_exit`, then
  `sync_eoi_intercept`; RIP is untouched.
- Every other 402h is a D1 profile mismatch: stop F580h, with EXITINFO1 in
  `info2` and EXITINFO2[31:0] in the detail.
- Undecodable 401h/402h exits (ID above 4, EOI vector below 16) stop with F581h.

`apply_avic_register_backend` was deleted.

### Guest INIT (D9) (replaces the startup stage-14 stop)

`service_startup` validates the command (`validate_x2avic`). For Init, it calls
`guest_init(state, vmcb, frame, &profile, &routes, InitOwners::local(sig), || svmvisor_resident_reset_guest_debug())`
with the route and cache leases held.

Preparation is read-only, and all of it precedes the first effect:
- `registers::prepare_init` (backing identity; physical ISR equals the held set). Refusal: stage 10 plus a typed value.
- `state.efer` is present (otherwise stage 15). `NativeEfer::reset_after_init()`
  runs on a *copy* (otherwise stage 6), so the fallible EFER step is prepared
  before any effect.
- `routes.prepare_destination_mode(slot, X2Apic)` (otherwise stage 4).

Commit, in D9 order:
1. `registers::commit_init` (steps 1-4: physical timer/LVT reset, retirement
   and drain, EOI intercept, backing reset). A failure is terminal: stage 11
   plus a typed value.
2. `NativeStartupTarget::apply_x2avic(Init)`. A failure is terminal, stage 12;
   it is unreachable after the identical validation.
3. `state.efer = Some(prepared)`, the `reset_after_init` result.
4. `commit_destination_mode_from(GuestInit)`, with the route guard held.
5. `svmvisor_resident_reset_guest_debug()`.
6. Back in `service_startup`: `mailbox.complete(command)` (common with SIPI;
   stage 8 on failure) and the existing `resident-guest-init cpu=... kick-acks=...`
   evidence print.

A terminal failure drops the unused destination token. That is sound: the
destination mode never changes (x2APIC stays x2APIC).

`initialize_ap_after_init` was confirmed: V_INTR_CONTROL keeps only bit 24 and
bits 31:30, so V_TPR = 0, and `request_full_tlb_flush` calls `invalidate_all`.
Both the existing `x2avic_cpu_init_commit_zeroes_v_tpr_and_invalidates_clean_bits`
test and the new runtime test check this. SIPI is unchanged.

The linked image shows the helper called (through the image's GOT slot)
immediately after the destination-history store. The `audit_debug_reset`
audit passes.

### Other

- `terminal_finish` compares against `host_apic_base`.
- Stop record (event 3) context[5] carries `ipi_drops`; it was always 0 before.
- The `write_msr` doc now lists its real callers. Guest x2APIC state reaches the
  physical LAPIC only through `HostX2Apic`.
- The `arm` doc lists every return code.

## 2. Stop reason and arm return codes

Low 16 bits of the stopped `info1`, with detail in bits 63:16. The owner of
these names is `terminal::X2AvicStop`.

| Tag | Meaning | Detail (bits 63:16) | info2 |
| --- | --- | --- | --- |
| F500h (kept) | Acceptance helper returned a value above 255 | 0 | raw value |
| F510h (kept, subcodes redefined) | MSR boundary | 0 | 0 no profile/guest APIC owner; 1 profile or pending event; 2 instruction evidence; 3 mode or TF; 4 #GP not queued |
| F520h (kept) | Profile mismatch | 0 | 0 profile or IPI owner missing; 1 AVIC exit with changed profile; 2 dispatch entry with changed profile |
| F521h (detail new) | Startup router refused INIT/SIPI | `NativeRoutePredicate` (0 = none recorded) | EXITINFO1 |
| **F541h-F546h (new)** | Register refusal (`registers::Refusal` 1-6: unowned access, unsupported message type, unmasked SMI, unmasked ExtINT, APIC disable, APIC relocation) | MSR index; bit 48 = WRMSR | refused value (0 for RDMSR) |
| **F551h-F559h (new)** | Incomplete-IPI refusal (`IpiRefusal` 1-9: target not running, invalid backing page, unknown ID, reserved bits, reserved message type, level, SMI, NMI, inconsistent vector exit) | EXITINFO2 index (27:16), ID (59:28) | EXITINFO1 (raw, bit 12 included) |
| **F560h (new)** | Fan-out: remote ID is not a doorbell target (nothing published) | slot, ID << 8 | EXITINFO1 |
| **F561h (new)** | Fan-out: publication refused (lower slots already published and doorbelled) | slot, `x2avic_error_code` << 8 | EXITINFO1 |
| **F571h-F57Bh (new)** | Host IRQ bridge error, low nibble = variant: 1 reserved vector, 2 physical ISR mismatch, 3 duplicate source, 4 ambiguous level, 5 unowned completion, 6 completion not ready, 7 unexpected physical ISR, 8 virtual publication, 9 virtual ISR mismatch, 10 drain incomplete, **11 (F57Bh) accepted vector not in physical ISR = ExtINT/8259 signature** | site: 0 capture (60h), 1 software EOI (7Ch), 2 level-EOI exit (402h) | `irq_error_code`: variant 20:17, second vector 16:8 (100h = none), vector 7:0 |
| **F580h (new)** | AVIC_NOACCEL outside D1 | EXITINFO2[31:0] | EXITINFO1 |
| **F581h (new)** | Undecodable AVIC exit | EXITINFO2[31:0] | EXITINFO1 |

Retired and never reused (a test asserts this): F501h-F504h (old capture
steps), F511h (APIC_BASE change), F522h (level completion), F530h/F531h (old
register backend). The per-CPU `stop_words` terminal export sends all F5xxh
stops as kind 0 (exit code plus guest RIP), as before; the full words are in
the event-3 record and the terminal context export.

Startup service stages (reason 7 of `startup_failure`, tag F10Ch, exported as
kind 15; names in `terminal::StartupStage` and `read_snapshot.py`):

| Stage | Meaning | Value |
| --- | --- | --- |
| 1, 2, 5, 8, 9 | unchanged (INIT ack, route table, target application, mailbox completion, wait exhausted) | AwaitSipi flag |
| 4 | destination token refused (same meaning as before eafe33a) | flag |
| 6 | EFER owner refused its INIT value, now during preparation | flag |
| **10 (new)** | guest INIT LAPIC preparation refused; nothing changed | `init_error_code` |
| **11 (new)** | guest INIT LAPIC commit failed after the physical reset (terminal) | `init_error_code` |
| **12 (new)** | CPU INIT commit refused after the LAPIC commit (terminal) | flag |
| 13 | cache replay (existing; the decoder used to reject it) | flag |
| 14 | **retired**: eafe33a guest-INIT refusal and missing profile | decoder name only |
| **15 (new)** | an owner that arm always installs is missing | flag |
| 3, 7 | retired xAPIC-era stages (current mode, ICR reset) | decoder names only |

`init_error_code`: bits 31:28 = 1 for the backing page (`x2avic_error_code` in
bits 7:0), or 2 for the IRQ bridge (`irq_error_code`). Because the value stays
at or below 32 bits, the target APIC ID is kept in the export.

Arm return codes: 0 armed, 1 state/identity, 2 capabilities, 3 EFER, 4 ACK
sites, 5 VMCB controls, 6 memory map, 7 inventory, 8 x2APIC/x2AVIC admission
(now also host ID above 254 and inherited physical ISR), 9 startup commit,
10 terminal endpoint, **11 (new): captured x2APIC register state outside the
guest model**, 12 cache replay. DXE still collapses every untyped arm failure
into its failure code 20 (`activation.rs`).

## 3. Deletions

- `apic::physical_eoi`, `physical_highest_in_service`,
  `physical_level_triggered`, `LVT_TIMER_MODE`, `LVT_TIMER_PERIODIC`,
  `EXTENDED_MSR_FIRST`. The const assert now uses `msr(EXTENDED)`.
  `read_physical_msr` and `write_physical_msr` stay (`HostX2Apic`).
- `ipi.rs` private `ICR_RESERVED`, moved to `apic::ICR_RESERVED` (shared with
  the capture check).
- runtime.rs:
  - `drain_physical_eoi`, `apply_avic_register_backend` and the old capture body;
  - the old `handle_avic_msr` value match with its 840h+ and F511h branches;
  - the `complete_level` + drain EOI branch and the stage-14 INIT stop;
  - `State.apic_base`, the dead `id_count > 1` MSRPM guard and `icr: None` branch;
  - the duplicate `0x60` match arm and `handle_avic_exit`'s unused frame parameter;
  - the direct `write_msr` physical TPR/SVR writes and the raw physical LVT copy
    loop in arm;
  - the unused `irq::Capture` import.
- registers.rs: the inline LVT rule in `write_lvt`, now `Lvt::check`, shared
  with the capture.
- Kept (still used): `PhysicalIrqLedger::{prepare_capture, commit_level_capture,
  complete_level, next_eoi, commit_eoi}`. The bridge uses them internally and
  tests use them directly.

## 4. Decisions and overrides

1. **ICR bit 12 tolerance on 401h (coordinator override of the brief).**
   - Sources: APM2 16.13 p661 makes the eliminated delivery status must-be-zero
     for x2APIC ICR writes, and 15.29.9.1 p580 calls EXITINFO1 "value written".
     Informative: Linux KVM avic.c `avic_incomplete_ipi_interception` notes that
     the hardware may leave the busy flag set.
   - Rule: classification ignores bit 12, and every other reserved bit still
     refuses (F554h).
   - On every resume path, bit 12 of the backing ICR low word is cleared, so
     guest ICR reads stay conformant (16.11.3 p659: reserved bits read as zero).
     Only this CPU's stopped guest writes that word.
   - Stop evidence keeps the raw EXITINFO1.
2. **Captured ICR bit 12 is dropped**, the same rule applied to the loader's
   readback. Other reserved bits refuse arm (11).
3. **ExtINT.**
   - A firmware LINT0/LINT1 left as unmasked ExtINT, and an unmasked SMI
     entry, are kept faithfully at arm and stay live physically. They are
     platform-owned routing.
   - A guest write of the same value is still refused (F544h/F543h).
   - A source accepted through such a LINT has no local APIC ISR bit
     (16.6.3 p647), so it stops as F57Bh with the 8259 vector.
   - Spurious handling now requires the vector's own ISR bit to be clear;
     for the host SVR vector FFh this is equivalent to the old rule.
4. **A reserved LVT message type refuses arm.** It has no defined meaning
   (Figure 16-7 p635; PPR p55). Unlike ExtINT or SMI, it is not a legal
   platform configuration.
5. **Arm writes a physical LVT only to add a mask:** software-disabled captured
   SVR (16.3.1 p629), or an illegal fixed vector (Figure 16-7). Counts and
   divide are never rewritten (16.4.1 p636).
6. **PPR = TPR at install** (16.6.4 p651; 15.29.3.1 p569).
7. **EFER INIT value is computed on a copy in preparation.** It is committed
   infallibly after the CPU commit. This is equivalent to
   `NativeEfer::reset_after_init()` at D9 step 6, with the only failure moved
   before the first effect.
8. **The unused destination token is dropped on a terminal commit failure.**
   This is allowed because the mode never changes.
9. **401h and 402h are handled as traps.** RIP is never changed and nRIP is
   never used (Table 15-22 p567; 15.7.1 p509 saves nRIP only for
   instruction/MSR/IOIO intercepts).
10. **402h EOI fallback keeps B-core's U11 decision:** ISR still set means a
    software EOI of the highest vector; already clear means left alone. See
    section 6 for the risk.
11. **MSRPM change clears every VMCB clean bit.** This is conservative: Figure
    15-4 p527 names only MSRPM_BASE (U22).
12. **Doorbell safety.** Every inventory CPU is armed before any guest can send
    an IPI: DXE `physical_boot` completes every AP's arm/enter/ACK before the
    BSP's callback arms and resumes the loader. The doorbell value is 254 or
    less (Figure 15-22 p579 and PPR p216). A doorbell to a host-mode core is
    assumed harmless (U5; informative KVM).
13. **The MSR byte-fetch fallback is kept.** It is the only continuation for
    MSR exits from guest code outside 64-bit mode, or without NRIPS.
14. **Drop evidence** is a per-CPU saturating counter in the stop record, plus
    the existing per-exit event-1/2 records, which carry EXITINFO1/2.

## 5. Validation (final tree; exact results)

Hypervisor (`--target-dir target/agent-b-int`):

| Command | Result |
| --- | --- |
| `cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc` | 66 test binaries (67 result lines with doctests), **450 passed**, 0 failed, 0 warnings (B-core 445; +1 terminal, +4 x2avic_registers) |
| same with `--features resident-runtime --lib` | **63 passed** (baseline 55; +7 runtime glue, +1 terminal) |
| same with `--features resident-runtime-test --lib` | **59 passed** (baseline 51; the terminal_return_tests stay excluded under this feature) |
| `cargo check --locked -p svmvisor-hypervisor --features resident-runtime --target x86_64-pc-windows-msvc` | pass, 0 warnings |
| same with `resident-runtime-test` | pass, 0 warnings |
| `cargo check --locked -p svmvisor-hypervisor --target x86_64-unknown-uefi` | pass, 0 warnings |
| extra: `cargo clippy ... --features resident-runtime --tests` | no findings in the new code. The remaining runtime.rs/terminal.rs findings are pre-existing: collapsible ifs at the ECAM NPF path, `drop(prepared)` in syscfg, operator precedence in `prepare`, etc. |

New unit tests:
- runtime.rs `x2avic_glue_tests` (7):
  - MSR outcome mapping;
  - 401h plan with bit-12 tolerance;
  - backing ICR bit-12 clear;
  - AVIC exit plan (402h mapping, undecodable exits);
  - guest INIT D9 order (physical commit before the debug reset, backing/VMCB/EFER/state results, destination history `GuestInit` count 1);
  - four refused preparations with no effect (foreign ISR, EFER not startup-owned, EFER missing, destination slot);
  - terminal LAPIC-commit failure with CPU/EFER/backing untouched.
- terminal.rs: 1 test (encodings, distinct tags, retired tags unused,
  wire-export kinds).
- `tests/x2avic_registers.rs`: 4 tests:
  - ExtINT signature and spurious-with-in-service;
  - captured interface keeps loader state, drops RO bits and sets PPR;
  - software-disable and illegal-vector mirrors;
  - refusals, including every ICR reserved bit.
- No existing Rust assertion had to change.

DXE and other crates:

| Command | Result |
| --- | --- |
| `cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-returning` | 29 binaries, **303 passed**, 0 warnings |
| ... `card-returning-loader` | **26 passed**, 0 warnings |
| ... `memory-attribute-probe` | **59 passed**, 0 warnings |
| ... `native-preflight` | **215 passed**, 0 warnings |
| ... `native-resident-boot` | **215 passed**, 0 warnings |
| ... `native-resident-low-runtime` | **215 passed**, 0 warnings |
| `cargo test --locked -p svmvisor-memory-attributes` | **29 passed**, 0 warnings |
| `cargo build-dxe` | pass, 0 warnings |

Payload build (`python tools/native-resident/build.py --output work/x2avic-batch-2026-09-16/b-int-build --boot --low-runtime`):
- **Passed completely**: payload link under the `0x1d4000` ASSERT, relocation
  packaging, undefined-symbol check (0), **`audit_debug_reset` passed**
  (symbol body `xorl %eax,%eax; movq %rax,%dr0..dr3; retq`; 0 other
  debug-register instructions), host-fault audit (256 vectors), no FP/SIMD/xstate
  instruction, DXE UEFI build (subsystem 12; 0 warnings in `dxe-cargo.log` and
  `payload-cargo.log`), physical/boot audits.
- **25,143 linked instructions** (B-layout 23,129).
- Layout: `text_end` 0x11a000, `data_start` 0x11c000, `image_load_end`
  0x123000, **`image_bss_end` 0x17c000**, `AVIC_BACKING` 0x12b000 (offset
  0x2b000), `svmvisor_resident_reset_guest_debug` 0x1191b0.
- **Headroom below 0x1d4000: 0x58000 bytes (88 pages)** (B-layout: 90).
- SHA256 digests:

  | Artifact | SHA256 |
  | --- | --- |
  | `payload.bin` | `dd3b1a5fe4637d273b59a84f61dd38c820103c8cc36125f6519b75b904608b20` |
  | `payload.reloc` | `eb74c167413f7e39bca27263aadd91a281b476a6e8c56030f11f57dd86395be9` |
  | `payload.elf` | `dfe88b2b80a1ea9c8dbb8d4960fb0ef6b83cbf4151319256206183d795c5bf0b` |
  | `driver.efi` | `6193197af3a805a0f1f9d86ca31a81fdd7808432c338b94fc3cee70c033ee542` |
  | `source-manifest.json` | `cc7c67028dd1e8ea149c9bf56136f8d6fabe1bbe11187396199ca1c9431d5a27` (265 entries) |
  | `disassembly.log` | `7bcc8933d327d74f6cb7b95119731fe3c5cc47c001e60a02c8464de1b09557fd` |

- The manifest was re-verified equal to the current tree after all edits; the
  only later edits were in `firmware/squirrel`, which the manifest does not cover.
- `b-int-build-superseded-1/` is an earlier, fully passing build from before
  the PPR-at-install fix and the clippy cleanups. It is not current evidence.

Stack audit (`python tools/native-stack-audit/run.py --output work/x2avic-batch-2026-09-16/b-int-stack-audit`, fresh):
- **PASS**: maximum 1560 bytes below the sampled RSP, coverage margin 63,976 bytes.
- PE SHA256 `c6044193ffdee3a5841ede2d51c247db00aa38e09982d4745d50edce18f16a40`.
- Source manifest (241 entries) re-verified equal to the current tree.
- The build log has the same 7 dead-code warnings as the Phase A baseline
  (`resources/tables.rs`, `resources/guest.rs`; untouched files).

Python:
- `python -m unittest discover -s tools/native-resident -p "test_*.py"`: **11 OK**.
- `... -s tools/native-stack-audit`: **42 OK**.
- `firmware/squirrel` discover: **81 OK** (1 skipped: the env-gated vector test).
- With `SVMVISOR_REFUSAL_VECTORS` produced by `tests/native_refusal_wire.rs`:
  `test_startup_diagnostics` **4 OK**. Two stale expectations in that env-gated
  test predate this phase: eafe33a reduced the Rust generator to 12 vectors (19
  were expected) and made the recipient mode x2APIC (`extended_xapic_4bit` was
  expected). Both were updated.
- The existing check that stage 10 is invalid became a stage-16 check, because
  stage 10 is now defined.

## 6. Gaps, unsupported cases, and what reviewers should scrutinize

**Ordering**
- **402h EOI under true fault semantics (U11).** If hardware reports the level
  EOI as a fault (RIP still at the WRMSR, ISR still set), the fallback's
  software EOI plus the resume re-executes the WRMSR. That is a silent double
  EOI.
  - The path should be unreachable: EOI writes are intercepted whenever a level
    source is held, and TMR is set only for held sources.
  - It becomes reachable only if a vector is shared between a level device
    source and an accelerated IPI.
  - Informative KVM handles it as a trap with ISR still set.
  - Hardware validation should confirm (A) trap with ISR set, or (B) trap with
    ISR clear.
- **INIT.**
  - A physical interrupt pending in IRR at INIT is captured after the next
    VMRUN into the reset page (recorded D9 deviation).
  - Remote IRR publications racing the backing reset follow its per-bank order.
    One arriving after its bank clear stays pending across SIPI, which matches
    16.5 "held pending" in the INIT state.
  - Once the drain issues physical EOIs, a still-asserted level line can
    re-deliver later.

**Concurrency with remote publishers**
- `BackingPage::set_trigger` checks pending, in-service and level state, then
  writes TMR. The check is not atomic across CPUs, so a remote edge publication
  and a local level capture of the *same vector* can race. Only vector sharing
  exposes this.
- A fan-out publication failure after lower slots were published is a stop
  with partial delivery (F561h carries the slot).

**Doorbell**
- A doorbell to a host-mode core is unspecified (U5).
- HLT wake relies on doorbell/INTR delivery to a halted guest (D10, U16).

**ExtINT on the physical machine**
- If the firmware leaves LINT0 as unmasked ExtINT with the 8259 active or able
  to raise a *spurious* IRQ7/IRQ15, the first such acceptance stops with F57Bh
  (the vector is the PIC base + 7 or 15; EDK2 uses base 68h, AMI is unverified).
  This matches the coordinator's decision (faithful state plus a distinct stop),
  but it is a plausible early-boot stop. Watch for F57Bh in the capture log.

**MSRPM caching**
- Map edits happen only on the owning CPU while its guest is stopped, followed
  by `invalidate_all`. Figure 15-4 does not settle whether VMRUN caches map
  contents.

**Still unsupported (D10, stopped with evidence)**
- NMI/SMI/level/lowest-priority/ExtINT IPIs (F555h-F558h);
- guest APIC disable or relocation (F545h/F546h);
- unmasked SMI/ExtINT LVT *writes* and reserved LVT message types (F542h-F544h,
  or arm 11 when captured);
- virtual ESR error generation (ESR always 0; illegal-vector LVT only masked
  physically);
- IOMMU GA posting;
- host-mode NMI in a GIF window (existing terminal behavior).

**Pre-existing items noticed, not changed**
- `terminal::route_failure` (kind-14 wire encoder) and
  `NativeStartupTarget::{apply, validate}` are used only by tests. The runtime
  now records the route predicate in F521h. The kind-14 format cannot express
  exit 401h: its bit 4 means a 7Ch exit.
- `dispatch_body` stops "not armed" with F10Bh/0, which `stop_words` treats as a
  malformed route record, so a terminal export would report failure 4.
- DXE collapses arm code 11 (like every other untyped code) into activation
  failure 20. There is no typed export of the captured register that failed.
- `IpiRefusal::UnknownReason` is unreachable from the runtime, because
  `AvicExit::decode` refuses ID 5 or higher first; those exits stop as F581h.
- Cross-crate calls in the payload, including the debug helper, go through
  GOTPCREL slots inside the image's RW data. This is the existing pattern, and
  the audits pass.

## 7. Stale documentation (not edited; for the docs agent)

- `README.md` line 15: "rewrite is incomplete (guest INIT still stops)". Guest
  INIT is now implemented; the tree is still unflashed.
- `crates/hypervisor/README.md`:
  - lines 10-11: "an active guest INIT still stops before changing state".
  - The `svm::x2avic` module table lacks `registers` and `ipi`.
  - The resident description should mention the stop-reason table
    (`terminal::X2AvicStop`) and `CapturedInterface`.
- `crates/dxe/README.md`:
  - lines 143-146: "While guest INIT stops at startup stage 14, the linker drops
    the unreachable `svmvisor_resident_reset_guest_debug` helper and the build
    intentionally stops at its debug-register audit". `build.py` now completes.
  - lines 146-149: the image bound is `X2AVIC_BACKING_ALIASES_OFFSET`
    (0xd4000 / 0x1d4000), not `X2AVIC_TABLE_OFFSET` (B-layout already noted).
- `docs/x2avic-rewrite-design-2026-09-16.md`:
  - lines 20-31: the "provisional count-driven timer backend", "other active
    LVT source configurations stop explicitly" and "guest INIT currently stops
    at startup stage14" are superseded by the register owner and D9.
  - line 291: "active guest INIT is not implemented safely".
  - line 82: `native_apic_reset.rs` row (the file was removed in Phase A).
- `docs/native-startup-refusal-snapshots.md` lines 54-56: the service-stage
  list needs stages 10-15, with 3/7/14 marked retired, and the INIT failure
  value format (`init_error_code`).
- `docs/handoff-2026-09-16.md` lines 9 and 93: "active INIT remain incomplete" /
  "active-LAPIC INIT".
- Docs should also describe:
  - the new arm code 11;
  - the F5xxh stop table (section 2);
  - the event-3 `incomplete_ipi_drops` field;
  - the ICR bit-12 and ExtINT decisions;
  - PPR = TPR at arm.

## 8. Review fixes (coordinator fix pass F1-F13)

Inputs: `review-code/review.md`, `independent-tests/notes.md` and
`crates/hypervisor/tests/x2avic_manual_conformance.rs` (159 tests, 3 ignored as
discrepancies). Manual facts added in this pass were read from rendered pages
of APM2 rev3.44 (SHA256 3d9dcb3f...c48c): p245, p246, p261 and p563
(`b-int-pages/`, PDF 307/308/323/625), p530 Table 15-10 and p572 Figure
15-17/Table 15-23 (`fix-pages/`, PDF 592/634). Sections 1-7 above describe the
tree before this pass; where they disagree, this section wins.

### F1. Route lease no longer held across the INIT/SIPI commit

Changed:
- `runtime.rs`: the per-command work moved out of `service_startup` into
  `startup_step` (a testable function), in this order:
  1. Lease-free checks: `validate_x2avic` (pending event, armed profile) and
     the new `startup::validate_destination_slot` (slot exists and names an
     admitted CPU; mailbox identities never change).
  2. When cache replay is owned, the core lease (`CacheCore`), which must be
     idle (phase 0). Under it: the INIT commit (`guest_init`) or the SIPI
     commit. The lease is dropped before step 3.
  3. The route lease, through `lock_routes_within(ROUTE_WAIT_ATTEMPTS)`, only
     for the destination record of a guest INIT and `mailbox.complete`
     (completion stays under the route lease: it must not interleave with a
     source's FIFO preflight and store).
- No path holds both leases now, so "route, then core" stays the only
  permitted nesting.
- `guest_init` no longer takes the route guard. D9 step 7 (destination
  record) now runs after step 8 (DR0-3 reset), just before step 9 under the
  route lease. Both steps are infallible local effects.
- A route lease not acquired after the commit is a terminal stop (stage 2,
  RouteTable; the command stays queued). A destination token that fails there
  is stage 4, a completion failure stage 8. `StartupStage` documents this.
- Source side (401h): `route_startup` waits `ROUTE_WAIT_ATTEMPTS` (2^24
  attempts with PAUSE) instead of 64, because the ICR write has completed and
  cannot be retried. `try_lock_routes` (64 attempts) remains for the callers
  that retry the unchanged instruction (SYS_CFG, low-RAM guest reads).

Tests:
- runtime glue tests:
  - `guest_init_commits_d9_then_records_and_completes_under_the_route_lease`:
    during the commit the core lease is held and the route lease is free;
    afterwards both are free.
  - `a_busy_cache_lease_defers_the_command_unchanged`: returns `Busy` with
    nothing changed.
  - `the_commit_runs_while_another_cpu_holds_the_route_lease`: a second
    thread holds the route lease until it sees the backing reset, i.e. inside
    the commit.
  - `a_lost_route_lease_stops_after_the_bounded_wait_with_the_command_queued`.
- `tests/native_destination_routing.rs` (+3):
  - `destination_slot_validation_needs_no_route_lease`;
  - `incomplete_ipi_routing_waits_out_a_busy_route_lease`: a holder thread
    outlasts `try_lock_routes`, and the route still succeeds;
  - `a_lost_route_lease_holder_bounds_the_incomplete_ipi_wait`: stops with
    `RoutingBusy` / `RouteBusy` and publishes nothing.

### F2. Trap exits before the startup service

Changed (`runtime.rs`):
- Before the guest ACK: unchanged. 60h is captured early, every other exit
  goes to the ACK path, and no startup command is serviced.
- After the ACK, `dispatch_body` hands the exit to
  `sequence_exit(exit_order(code), handle_exit, startup_service)`:
  - 60h, 401h and 402h (`TrapThenStartup`): the handler runs first; if it
    resumes, `service_startup` runs as well. 60h now reaches the service on
    every exit.
  - Every other exit (`StartupThenExit`): the service runs first. A command
    that changed the guest (or a stop) decides the exit; otherwise the
    instruction handler runs.
- `handle_exit` is the former match of `dispatch_body`, now with the 60h and
  401h/402h arms. `startup_service` is the former service wrapper, with its
  F103h fallback.

Tests: `traps_are_handled_before_the_startup_service_and_instructions_after`
covers the classification of 60h/401h/402h against 63h/72h/77h/7Bh/7Ch/81h/
400h/invalid, and every call-order and result combination.

### F3. Vectors 16-31

(a) Changed:
- `registers.rs`: `Lvt::check` refuses an unmasked fixed entry with vector
  16-31 as the new `Refusal::ExceptionVector` (code 7, stop F547h).
- `CapturedInterface::capture` refuses such a live loader value (arm code 11,
  now typed, see F6).
- Masked entries, entries on a software-disabled APIC, and NMI/ExtINT/SMI
  entries are not affected.

Tests:
- `x2avic_registers.rs:330` (now
  `unmasked_fixed_lvt_below_vector_16_is_stored_but_physically_masked`)
  sweeps vectors 0-32 and FFh on all six LVTs: 16-31 are refused with nothing
  written, and 32 and FFh are mirrored as written.
- `captured_software_disable_and_illegal_vectors_mask_the_physical_mirror`:
  error LVT 10h became 20h.
- `captured_state_outside_the_register_model_is_refused_without_effects`:
  +4 refusals, plus masked and non-fixed values that are kept.
- The comment at `:756` (capture of 1Fh) now states that only a
  guest-programmed IOAPIC/MSI source can produce such a vector.
- The terminal tag test covers F547h, and the glue test covers its MSR
  mapping.

(b) Changed. This was judged safe; the reasons follow.
- `irq.S`: window-checking gates now cover vectors 16-255, except 18. The
  offsets table has 240 entries; entries 18 and 30 are 0.
  - Each gate runs `clgi`, then compares the window flag. Outside the window
    it jumps to its own `svmvisor_resident_fault_N` with the exception frame
    untouched.
  - Inside the window it records the vector, clears the saved IF and IRETQs.
    That is the same body as the existing 32-255 gates.
- `runtime.S`: the #SX gate still consumes error code 1 (INIT redirection).
  For any other first stack word it now jumps to gate 30 rather than to
  `fault_30`, and gate 30 checks the window.
- `runtime.rs prepare`: installs the gates where `window_gate(v)` holds. It
  keeps IST1 for 16-31 (they are still host exceptions outside the window)
  and IST0 for 32-255, as before.
- The helper then returns vector 16-31, `irq::capture` refuses it as
  `ReservedVector`, and the CPU stops with F571h and the vector in `info2`.
  Before this change the result was a silent halt, or a host exception
  recorded with a wrong frame.
- Why no exception can be confused with an interrupt in the window:
  - The window executes only `sti; nop; nop`. Host CPL0 has no #AC, no FP or
    SIMD (#MF, #XM), no control-transfer #CP, no #VC outside SEV-ES, and no
    TF or breakpoints.
  - External interrupts push no error code (8.2.24 p261).
  - #SX carries only error code 1 (15.28 p563), and its gate distinguishes
    that code.
  - #MC (18) is asynchronous and keeps its terminal stub.
  - NMI (2) is unchanged.
- Audit coupling:
  - `fault.S` is unchanged, so the fault-fixture evidence still applies.
  - `build.py audit_host_fault` now also checks, by regular expression, the
    exact body of all 239 gates (window check, fall-through target
    `fault_N`, recorded vector, IF clear, IRETQ). Linker alignment padding
    after IRETQ is ignored. It also checks that no gate 18 exists and that
    the complete #SX body contains `jne <svmvisor_resident_irq_30>`.
  - The audit summary gains `irq_window_gates` (239), the terminal-only
    vectors below 32, and the #SX non-INIT path.
  - `tools/native-resident-audit/run.py` links `runtime.S` alone, and its
    instruction allowlist lacks cmpq/iretq/lock/stgi. It was already stale at
    HEAD, is not part of this matrix, and was not changed.

Tests: `tools/native-resident/test_host_fault_audit.py` (3 tests):
- a passing synthetic image;
- padding after the last gate;
- 8 failing variants: a missing gate, the old 32-255 set, a wrong fall-through
  stub, a wrong vector, a changed IF mask, an added gate 18, an #SX that skips
  the check, and a changed #SX body.

The first build attempt (`fix-build-1`) failed exactly on that padding case.

Not done (still unsupported, D10):
- A physical interrupt with vector 18 is still reported as a host #MC.
- IOAPIC/MSI vectors 16-31 programmed by the guest are a stopped refusal
  (F571h), not a delivery.

### F4. 401h ID 4

Changed: `Inventory::classify` routes INIT and STARTUP to `Startup` for
IDs 0, 2 and 4. For ID 4, a fixed IPI with a vector below 16 is dropped; every
other ID 4 ICR is `InconsistentVectorExit`.

Tests:
- `x2avic_ipi.rs:103`: INIT and SIPI 09h with ID 4 are `Startup`.
- Conformance `incomplete_ipi_id4_drops_illegal_vectors`: INIT and STARTUP
  09h are `Startup`, and INIT was removed from the inconsistent list.
- Glue test: INIT with IDs 0, 2 and 4.

### F5. Physical-ID table entry checked before any visible change

Changed:
- New `PhysicalIdTable::is_stopped_entry(id, backing)`: the entry equals
  exactly V | backing | host ID, with IsRunning and bits 61:52 clear (Figure
  15-17 and Table 15-23 p572).
- `arm` checks it before the route lease, VM_CR, the destination record, the
  LAPIC install and IsRunning. The final `set_running` can then refuse only if
  the published table changed meanwhile. The arm doc comment says so.

Tests: `x2avic.rs table_format_and_publication_preserve_identity_and_reserved_bits`
covers an empty entry, the exact entry, a wrong backing page, an unaligned
backing page, reserved bits, a wrong ID, ID 512, and an entry that is already
running.

### F6. Arm code 11 carries the refused register

Changed:
- `host/resident.rs`: new `TAKEOVER_TAG` (A1h), `TAKEOVER_CAPTURED_REGISTER`
  (5) and `captured_register_refusal`, plus the ABI note on `ArmRuntime`.
- `registers.rs`: `CaptureRefusal::captured(msr)` names the only MSRs a
  refusal can carry: 808h, 80Fh, 830h, 832h-837h, 838h and 83Eh.
- `arm` returns the typed code instead of 11.
- Encoding:

  | Bits | Content |
  | --- | --- |
  | 63:56 | A1h |
  | 55 | value has bits above 31 |
  | 54:48 | reason 5 |
  | 47:32 | x2APIC MSR index |
  | 31:0 | low half of the value |

- DXE: `activation.rs` passes it through by `abi::TAKEOVER_TAG`.
  `diagnostics/resident_boot.rs takeover_failure_words` accepts reason 5 only
  with a captured MSR, so the BSP record is stage 84h. On an AP the code is
  kept in its BOOT record (reason 83h) as before.
- `read_snapshot.py apic_takeover_diagnostic`:
  - reason 5 is named `captured_x2apic_register_unsupported`;
  - it gets the new keys `msr` and `register_name`;
  - its `register_offset` is `None` (reasons 1-4 keep the MMIO offset).

Tests:
- DXE `takeover_refusals_preserve_raw_code_and_reject_bad_shape`: every
  captured MSR, with low and wide values, and 4 bad reason-5 shapes.
- Python `test_captured_register_refusal_names_the_msr`: BSP and AP records,
  plus 6 bad MSRs.

### F7. AwaitSipi waits without a bound

Changed (`service_startup`):
- With an empty queue in AwaitSipi, the CPU polls the mailbox and the
  terminal request with PAUSE, without a bound. Every `AWAIT_SIPI_POLLS`
  (2^16) polls it opens a GIF window (`acknowledge_init`).
- A pending command whose core lease stays busy is retried
  `STARTUP_LEASE_ATTEMPTS` (2^20) times, each attempt with its INIT
  acknowledgment, and then stops with stage 9 (`WaitExhausted`, now
  documented with this meaning).
- Running returns after `STARTUP_COMMANDS_PER_EXIT` (64) commands. Remaining
  commands are serviced at the next exit, and every exit now services them
  (F2). This bounds a guest flood of SIPIs.
- The comments on `service_startup` and `acknowledge_init` state the GIF
  window's purpose. INIT notifications, NMI and external SMI are held while
  GIF=0 (Table 15-10 p530), and firmware SMM needs its SMIs. They also state
  the terminal NMI exposure: an NMI taken in any host GIF window reaches the
  vector-2 fault stub and stops the CPU.
- The unbounded wait is the requested exception to the "bounded VM-exit
  work" rule, for a CPU that has no guest to run.

Tests: the loop depends on statics, STGI and the terminal page, so it has no
host test. Its per-command logic is `startup_step`, covered under F1, F12 and
F13.

### F8. HostX2Apic restricted to 800h-8FFh

Changed:
- `apic.rs`: `debug_assert!` on the x2APIC MSR range in `read` and `write`,
  with docs.
- `read_physical_msr` and `write_physical_msr` are private.
- `ring_avic_doorbell` keeps its own WRMSR.

Tests: the existing ones. Every owner access is in range.

### F9. Production-uncalled items removed

Each caller was verified before removal:
- `IdentityNpt::trap_page` is removed, together with the trapped-page state
  and branches and the stale "native LAPIC leaf holes" comment. Three
  npt.rs unit tests were only for it and are removed. `tests/identity_npt.rs`
  no longer traps FEE00000h and now asserts that it stays identity-mapped
  and writable. One test was renamed.
- `terminal::route_failure` is removed, together with the kind-14 branch of
  `stop_words`:
  - F10Bh now exports as an unhandled exit with the guest RIP. That includes
    the pre-arm stop, which a comment now notes; section 6 described its old
    malformed export (failure 4).
  - `native_refusal_wire.rs` keeps only the target vectors and a check of the
    F521h export.
  - `test_startup_diagnostics.py` now expects 4 target vectors.
  - The decoder still reads kind 14 from older images.
- `NativeStartupTarget::{validate, apply}` are removed (earlier in this pass).
  Their CET/INIT-state tests in `native_cet_independent.rs` and
  `native_guest_startup.rs` test state that the x2AVIC commit shares, so they
  were converted to `apply_x2avic` with an armed profile rather than deleted:
  - x2AVIC is enabled before EVENTINJ in the refusal tests;
  - after INIT, `0x60` equals `NATIVE_CONTROL` (V_TPR 0).
- `GuestX2Apic::apic_base()` is removed. Its test-only uses
  (`x2avic_registers.rs`, and four in the conformance file) now read
  APIC_BASE through the RDMSR emulation path.
- `BackingPage::eoi_stopped` now returns `Option<u8>`:
  - `x2avic.rs` asserts the TMR bit directly;
  - the conformance `software_eoi_sequences_recompute_ppr` does the same with
    `is_level`.
- Kept: `FixedIpi::{vector, targets}`. The runtime glue tests and
  `x2avic_ipi.rs` use them.

### F10. Comment drift

Changed (`terminal.rs`):
- The retired list now includes F505h, F523h and F524h, and the tag test
  checks them.
- It documents that images up to eafe33a used F510h with an MSR index
  (now F541h) and F520h with raw EXITINFO2 (now F580h/F581h).
- It documents that a dispatch-entry F520h on an NPF exports as GPA 2
  (kind 3).
- `StartupStage`: every stage except 10 and 11 carries the AwaitSipi flag.
  Stages 2, 4 and 8 can follow an applied command.
- Doorbell SAFETY (runtime `handle_incomplete_ipi`):
  - The receiver need not be armed, because AP guests run before the BSP
    arms.
  - DXE publishes every table entry and prepares every backing page before
    the first arm, and nothing clears V or IsRunning, so published pages stay
    valid. IsRunning is set only at the end of a CPU's own arm.
  - A host-mode doorbell has no defined effect (15.29.8.2 p579). IR does not
    distinguish guest mode from host mode (p572).
- `ipi.rs TargetNotRunning`: the same reasoning replaces "the table state is
  inconsistent".

### F11. Logical INIT/SIPI

Changed (earlier in this pass):
- `route_startup` computes targets with `Inventory::targets`, the same
  computation as fixed IPIs: shorthand 11, or a physical or logical
  destination. The separate physical-only matcher and `is_broadcast` are
  gone, and predicates 3, 7, 8 and 10 are retired (decoders keep the names).
- Table 16-4 limits are kept. Self and all-including-self shorthands,
  DEST FFFF_FFFFh without shorthand, a self destination, no match, and an
  INIT vector other than 0 are refused.
- Multi-target publication is all or nothing: every selected FIFO is
  preflighted under the lease before any store.
- F521h detail (`terminal::startup_route_refusal`):

  | Bits | Content |
  | --- | --- |
  | 3:0 | predicate |
  | 4 | recipient recorded |
  | 9:5 | INIT count, saturated |
  | 11:10 | cause |
  | 15:12 | mode |
  | 47:16 | recipient ID |

Tests:
- Conformance `init_to_a_logical_destination_reaches_the_matching_cpu` is no
  longer ignored and passes unmodified.
- Existing router tests.
- The terminal test checks the F521h encoder.

### F12. Software-disabled guest APIC

(a) Changed: `deliver_fixed` samples each target's SVR once and skips
software-disabled targets, with no publication and no doorbell. Test
`x2avic_ipi.rs pages()` now enables SVR, as a running guest's page would be.

(b) Changed:
- `irq::capture` on a software-disabled page only acknowledges an edge
  source (physical EOI, then drain, returning `Capture::Discarded`).
- A level source is still published and held. This is the documented
  deviation: without a guest EOI the asserted line would re-fire at once.
- The runtime counts discards in `State.irq_discards`. The stop record's last
  context word is now `stop_counters`: bits 31:0 are IPI drops and 63:32 are
  discards, each saturated.
- `read_snapshot.py` names that word `stop_counters` and derives
  `incomplete_ipi_drops` and `disabled_apic_edge_discards`. Older images
  decode as before, with a high half of 0.

(c) Changed (`GuestX2Apic`):
- A disabling SVR write records the pending IRR (`held_at_disable`).
- An enabling write first withdraws every IRR bit that is pending now, was
  not pending at the disable, and is not a level source in this CPU's ledger.
  Their TMR bits go too (`BackingPage::discard_pending`). The SVR is stored
  after that.
- The ordering is faithful for these reasons:
  - The guest is stopped across the withdraw and the store.
  - A real APIC would never have accepted those interrupts.
  - The software fan-out and the bridge publish no edges into a disabled
    page, so the withdrawn bits come from x2AVIC hardware IPIs, which ignore
    the virtual SVR.
  - A vector that was pending at the disable stays pending even if it
    arrived again.
- `guest_init` calls `GuestX2Apic::reset_after_init()`, because the reset
  page is disabled and has an empty IRR.
- `GuestX2Apic::emulate` now takes `&mut self`. The runtime uses
  `state.guest_apic.as_mut()`, so the record is not lost on a copy.

Tests:
- The conformance tests `fixed_ipi_is_not_accepted_by_a_software_disabled_target`
  and `device_interrupt_is_not_accepted_by_a_software_disabled_guest_apic` are
  no longer ignored and pass unmodified.
- New `x2avic_registers.rs` tests:
  - `reenable_withdraws_only_interrupts_that_arrived_while_disabled`: kept,
    withdrawn, and a stale TMR bit; a level source captured while disabled;
    an edge discard; a repeated enable; a second cycle.
  - `guest_init_forgets_the_irr_held_at_a_software_disable`.
- Glue tests:
  - `accepted_vectors_publish_and_resynchronize_the_eoi_intercept` (discard
    and level while disabled);
  - `stop_records_pack_saturated_drop_and_discard_counts`;
  - the INIT test checks that a record held before INIT is cleared.
- Python `test_stop_record_splits_drop_and_discard_counters`.

### F13. Glue coverage

Refactors, each with a real production caller:
- `apply_msr_completion` (MSR continuation, #GP and stop mapping);
- `capture_accepted` and `sync_eoi_intercept(ledger, msrpm, vmcb)`, with
  `local_msrpm()`;
- `sequence_exit` and `exit_order`;
- `startup_step`;
- `backing_alias` and `backing_alias_pte`, used by `prepare` and
  `remote_backing`;
- `window_gate`;
- `stop_counters`.

Tests: the `x2avic_glue_tests` count went from 7 to 17. The new or reworked
tests are:
- MSR completion: RDMSR loads EDX:EAX and zero-extends it; WRMSR keeps
  RAX/RDX; the RIP advances, and the shadow, RF and clean bits are consumed;
  #GP(0) at an unchanged RIP; F510h/4 when #GP cannot be queued; a stop
  leaves everything unchanged.
- 60h capture: a level source sets the intercept and clears the clean bits;
  an edge keeps both; discards; a level source on a disabled page; spurious
  interrupts; vector 17 gives F571h; 100h gives F500h.
- INIT, then AwaitSipi, then SIPI 9Ah (CS 9A00h, base 9A000h, IP 0), then an
  ignored SIPI with nothing changed and the command completed.
- Dispatch ordering (F2) and lease contention (F1).
- Alias PTE computation for 32 slots, including the bound below the table.
- The window-gate set.
- Seven refused-command cases with no effect: foreign ISR, EFER not
  startup-owned, EFER missing, guest APIC missing, bad slot, pending
  EVENTINJ, and cache replay in progress.
- A terminal LAPIC commit failure.

### Validation (final tree; `--target-dir target/agent-b-int`)

| Command | Result |
| --- | --- |
| `cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc` | 68 result lines, **611 passed**, 0 failed, **0 ignored**, 0 warnings. This includes `x2avic_manual_conformance` **159/159** (previously 156 + 3 ignored). The other 452 compare with 450 before: -3 npt trap tests, +3 routing tests, +2 register tests. |
| same with `--features resident-runtime --lib` | **70 passed** (previously 63: +10 glue tests, -3 npt) |
| same with `--features resident-runtime-test --lib` | **66 passed** (previously 59) |
| `cargo check` with `resident-runtime` / `resident-runtime-test` / `--target x86_64-unknown-uefi` | pass, 0 warnings each |
| `cargo clippy ... --features resident-runtime --tests -- -A clippy::redundant_comparisons` | No findings in new or changed code. The remaining findings are the pre-existing set from section 5. The allow is needed because the pre-existing deny-level `redundant_comparisons` at `tests/identity_npt.rs:357` (unchanged since HEAD) otherwise stops the `--tests` build under this toolchain's clippy. |
| DXE `cargo test ... --features` native-returning / card-returning-loader / memory-attribute-probe / native-preflight / native-resident-boot / native-resident-low-runtime | **303 / 26 / 59 / 215 / 215 / 215 passed**, 0 warnings |
| `cargo test --locked -p svmvisor-memory-attributes` | **29 passed**, 0 warnings |
| `cargo build-dxe` | pass, 0 warnings |
| `native_refusal_wire` with `SVMVISOR_REFUSAL_VECTORS` | 1 passed; 4 target vectors |
| Python `tools/native-resident` | **14 OK** (+3 host-fault audit tests) |
| Python `tools/native-stack-audit` | **42 OK** |
| Python `firmware/squirrel` discover | **83 OK**, 1 skipped (+1 takeover reason 5, +1 stop counters) |
| `test_startup_diagnostics` with the generated vectors | **4 OK** |

Payload build (`python tools/native-resident/build.py --output work/x2avic-batch-2026-09-16/fix-build-3 --boot --low-runtime`):
- **Passed completely**:
  - `audit_debug_reset`: unchanged body, 0 other debug-register
    instructions;
  - `audit_host_fault`: 256 fault stubs, **239 window gates**, #SX chain
    `irq_30` then `fault_30`;
  - no FP/SIMD/xstate instructions; 0 undefined symbols;
  - bootstrap audit: 164 copied bytes, 0 relocations;
  - DXE subsystem 12; 0 warnings in both cargo logs.
- **25,726 linked instructions** (previously 25,143).
- Layout:

  | Symbol | Value |
  | --- | --- |
  | `text_end` | 0x11b000 |
  | `data_start` | 0x11d000 |
  | `image_load_end` | 0x124000 |
  | **`image_bss_end`** | **0x17d000** |
  | `AVIC_BACKING` | 0x12c000 (offset 0x2c000) |
  | `svmvisor_resident_reset_guest_debug` | 0x119ae0 |
  | gates `irq_16` / `irq_255` | 0x1002d3 / 0x102e85 |

- **Headroom below 0x1d4000: 0x57000 bytes (87 pages)** (previously 88).
- SHA256 digests:

  | Artifact | SHA256 |
  | --- | --- |
  | `payload.bin` | `197485b3e8a7c4d5a0fec1fcfe34c947783f98c62989d92ee321e0e4fe495319` |
  | `payload.reloc` | `81e6243ce435e930287749ecbebff0ea8b5251fff3ff4f3e8766358b79530d68` |
  | `payload.elf` | `0aa39ee56c6ba8e51d09c6271987cebaa94496fa7ed8cab80d37183de149c4aa` |
  | `driver.efi` | `f2150d6fae978c02053ea58bb09289aeffdc6587bb4b0590522af86e16283732` |
  | `source-manifest.json` | `07eeb853a693cdfbccdb30999d6fb3ae76dc1434ff8a91aaeccc43e6be0aca06` (267 entries) |
  | `disassembly.log` | `3584311a7f7dfcc2727e5475dd99880c18bd6c85dd91e1620b7e3966b1942860` |

- The manifest was re-verified equal to the tree after the last edit. The
  only later edits were in `firmware/squirrel` and this file, which the
  manifest does not cover.
- The manifest also covers the root/crate README and CONTRIBUTING edits made
  by the docs agent.
- Other build directories:
  - `fix-build-2/` has byte-identical payload and driver artifacts. It was
    built before the last test-only edits.
  - `fix-build-1/` is the failed attempt described under F3(b).

Stack audit (`python tools/native-stack-audit/run.py --output work/x2avic-batch-2026-09-16/fix-stack-audit-2`):
- **PASS**: maximum 1560 bytes below the sampled RSP, coverage margin 63,976
  bytes.
- PE SHA256 `c6044193ffdee3a5841ede2d51c247db00aa38e09982d4745d50edce18f16a40`
  (unchanged: the native-returning profile contains none of this pass's code).
- The manifest (242 entries; it now also lists the conformance test file) was
  re-verified equal to the tree.
- The same 7 dead-code warnings as the baseline.
- `fix-stack-audit-1/` is an identical earlier pass.

### Items from section 6 that this pass resolved

- `terminal::route_failure` and the generic `NativeStartupTarget` API are
  removed.
- The F10Bh pre-arm stop now exports cleanly.
- DXE preserves the typed arm code 11.

### Not done, with reasons

- Vector 18 from a physical source (F3b) remains a host #MC report. The IDT
  cannot tell an asynchronous #MC from an interrupt.
- The AwaitSipi loop has no host unit test (F7): it depends on statics, STGI
  and the terminal page. `startup_step` carries the testable logic.
- `IpiRefusal::UnknownReason` is still unreachable from the runtime
  (section 6). It is outside F1-F13.

### Additional stale documentation (for the docs agent)

These describe the tree before this pass:
- `docs/x2avic-completion-2026-09-16.md`:
  - lines 80-86 ("Still production-uncalled") list `trap_page`, the generic
    `NativeStartupTarget::{apply, validate}` and `route_failure`. All three
    are removed; kind 14 is retired.
  - line 112 and lines 576, 1029, 1203: event-3 field 5 is now
    `stop_counters`. Its low half is `incomplete_ipi_drops`; its high half is
    the new `disabled_apic_edge_discards`.
  - lines 415, 495, 741, 1022, 1094-1100: arm code 11 is now the typed
    A1h/reason-5 code, which DXE preserves (BSP stage 84h, AP reason 83h) and
    the decoder names. Section 6's "DXE collapses code 11" no longer holds.
  - lines 453 and 528: ID 4 INIT/STARTUP goes to the startup router.
  - line 523: the reason for ID 1 changed (see F10).
  - line 529: logical INIT/SIPI destinations are admitted.
  - lines 664 and 710-712: the lease order (F1) and the unbounded AwaitSipi
    wait with periodic GIF windows (F7).
  - lines 746-754: the table-entry check precedes VM_CR (F5).
  - line 791: the F521h detail layout (F11).
  - lines 792 and 1187: register refusals are now F541h-F547h. 7 is the
    exception-vector LVT.
  - line 847: stage 9 now means that the cache lease stayed busy for a
    pending command.
  - D9 step order: 7 now follows 8.
  - The unsupported list gains:
    - guest-programmed IOAPIC/MSI vectors 16-31 (F571h);
    - a physical vector 18, reported as a host #MC;
    - unmasked fixed LVT vectors 16-31 (F547h, or arm code 11 at capture).
  - The software-disabled rules (F12) and the host IDT window gates (F3b)
    need a description.
- `docs/native-broadcast-startup.md` lines 115-119:
  - The 401h source path now waits up to 2^24 lease attempts; it does not
    re-enter the instruction.
  - A target takes the route lease only after its commit (bounded 2^24, then
    stage 2).
  - The 20,000,000-iteration service budget is gone (F1/F7).
  - Line 46 (the DXE AP release and completion polls) is unaffected.
- `docs/native-startup-refusal-snapshots.md`: stage 9's meaning, and stages
  2, 4 and 8 after an applied command.
