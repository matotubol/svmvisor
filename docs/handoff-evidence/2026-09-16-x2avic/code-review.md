# Code review: x2AVIC batch (uncommitted tree on main, HEAD eafe33a)

Date: 2026-09-16. Independent, read-only review. The only file written is this one.

Scope:
- Phase A (removals and reorganization), B-core, B-layout and B-integration, as described in `phase-a-notes.md` and the three `phase-b-*-notes.md` files.
- Manual conformance is out of scope; a separate agent reviews it.

Validation run for this review (`--target-dir target/review-code`), all passing:
- `cargo test -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --no-fail-fast`: 450 tests in the batch's own binaries.
- The same with `--features resident-runtime --lib`: 63 tests.
- `cargo test -p svmvisor-dxe --features native-resident-boot`.

The hypervisor run also picked up `tests/x2avic_manual_conformance.rs`, an untracked file that the manual-conformance reviewer was writing during this review. It is not part of the batch. One of its tests failed on an earlier run and passed on the last one, which reported 131 passed.

No finding below is caught by the existing tests.

No critical or high findings. Findings are ordered by severity; the three medium ones are all liveness or robustness issues in the runtime.

---

## Findings

### M1. A busy route lease stops the INIT/SIPI sender, and guest INIT now holds that lease through the whole commit

- **Severity:** medium
- **Where:**
  - `crates/hypervisor/src/svm/x2avic/startup.rs:151-154` (lease refusal) and `:329` (64-spin bound)
  - `crates/hypervisor/src/host/resident/runtime.rs:1553-1562` (the refusal becomes a stop)
  - lease holders: `runtime.rs:1789-1872` (`service_startup`, now including the whole `guest_init`, 1917-1947) and `runtime.rs:2083-2088` (every low-memory `GuestReader::read`)

**Defect.** Routing an INIT/SIPI from an AVIC_INCOMPLETE_IPI exit tries the shared route lease 64 times. On failure the sending CPU stops terminally (F521h, predicate 13 = RouteBusy). Meanwhile:
- A target servicing guest INIT holds the same lease across `prepare_init`, `commit_init` (about 16 RDMSR, 8 WRMSR, the drain and its EOIs), the CPU commit and `mailbox.complete`.
- Any CPU fetching an instruction from memory below 1 MiB holds the lease across the fixed-MTRR SYS_CFG sampling (several WRMSR/RDMSR).

Each of these takes microseconds; 64 CAS+PAUSE iterations take much less.

**Failure scenario (Windows AP start):**
1. The BSP guest writes an INIT ICR for AP k. A 401h exit routes it and kicks.
2. AP k takes the lease and starts the INIT commit.
3. The BSP guest immediately writes the SIPI ICR, with no or a short INIT-to-SIPI stall (Linux uses 0 on modern CPUs; the Windows value is unmeasured).
4. The BSP's 401h routing finds the lease busy after 64 spins. The **BSP stops with F521h** and boot is lost.

The same happens when the second SIPI lands while AP k's real-mode trampoline is inside a low-memory fetch (for example its EFER WRMSR takes the byte-fetch path because the guest is not in 64-bit code).

Before this batch, INIT stopped at stage 14, so the lease was never held for a whole INIT commit.

**Minimal fix (either of):**
- On the 401h path, wait for the lease with a bound longer than the worst-case target critical section. The trap cannot be retried, so a short bound turns ordinary contention into a stop.
- Have `guest_init` do the LAPIC and CPU commit without the route lease. The destination mode never changes for x2APIC INIT. Take the lease only around `commit_destination_mode_from` and `mailbox.complete`.

In either case, add a contention test: the source routes while a target holds the lease.

### M2. A startup wake-up is lost when the private #SX is consumed during a physical-INTR exit

- **Severity:** medium
- **Where:**
  - `crates/hypervisor/src/host/resident/runtime.rs:1025` (`acknowledge_init` on every exit)
  - `runtime.rs:1080` (the 60h exit returns before `service_startup` at 1102-1110)
  - `svmvisor_resident_accept_irq` in `crates/dxe/src/native/resident/irq.S:12-25` (opens GIF again)

**Defect.** A 60h dispatch consumes a pending redirected INIT, the kick for a newly published mailbox command, in either of two places:
- the entry `acknowledge_init()`;
- the `stgi` window of the acceptance helper.

It then returns without looking at the mailbox. No 63h exit follows, because the INIT was already turned into #SX.

**Failure scenario:**
1. AP k is in a physical-INTR dispatch (for example a timer tick or a firmware-left LVT source).
2. The BSP routes INIT to AP k. The notification INIT arrives between VMEXIT(INTR) and the end of the acceptance window.
3. The #SX is consumed. The IRQ is captured and the guest resumes. The INIT command stays queued.
4. The guest HLTs natively (HLT is not intercepted, D10), so there is no further exit.

The command is serviced only if a later kick or exit arrives. If the last kick (SIPI) is also lost, the AP never starts. Commands then accumulate toward the 4-entry FIFO limit, and a full queue is a sender stop (QueueBusy, F521h).

**Minimal fix.** On the 60h path, after a successful capture, also run `service_startup` when `state.startup_owned` (it returns quickly on an empty mailbox). Alternatively, return to the common path instead of returning early. Add a dispatch-order test.

### M3. Physical interrupt vectors 16-31 that the guest can program reach the terminal host exception gates

- **Severity:** medium
- **Where:**
  - `crates/hypervisor/src/svm/x2avic/registers.rs:144` (mirror masking only for vectors below 16)
  - `crates/hypervisor/src/host/resident/runtime.rs:476-483, 508` (IDT: 0-31 are fault stubs on IST1; 30 is `svmvisor_resident_sx`)
  - `tools/native-resident/fault.S:14,44`
  - `crates/dxe/src/native/resident/runtime.S:225-226`

**Defect.** The source paths:
- `Lvt::check` mirrors an unmasked fixed LVT with vector 16-31 unmasked to the physical LAPIC, and `CapturedInterface` keeps such a loader value live.
- The guest also programs IOAPIC and MSI vectors natively.

Such an interrupt is taken inside `svmvisor_resident_accept_irq` through IDT[16..31], which holds only terminal exception stubs. It never reaches `irq::capture`, whose `ReservedVector` refusal is therefore unreachable for these vectors.

**Failure scenario:** the guest writes MSR 832h = 0x11 (timer, vector 11h) and 838h = 1. The physical timer fires and the host accepts vector 17:
1. `fault.S` treats 17 as an error-code vector and pushes no dummy error code.
2. `rep movsq` copies 7 qwords from IST1 and reads the qword at `fault_top`. That page is the unmapped guard page (runtime.rs 474/521).
3. The nested #PF re-enters IST1, finds the latch already set and halts in `svmvisor_resident_fault_stop`.

The result is a silent CPU halt with **no** diagnostic record. The same applies to vectors 21 and 29, and to 30 via the #SX gate: its first stack word is the interrupted RIP, not 1, so it falls through to `fault_30` and records a misparsed frame. Vectors 16, 18-20, 22-28 and 31 are recorded as host exceptions (vector 18 looks like a host **#MC**), which misattributes a guest action to the host.

**Minimal fix:**
- In `Lvt::check` (and therefore `CapturedInterface`), treat an unmasked fixed entry with vector 16-31 as a stopped refusal. It cannot be delivered through this host IDT.
- In the host gates:
  - make the stubs for vectors 16-31 other than 18 and 30 check `svmvisor_resident_irq_window`, and return the vector, so capture stops with F571h (ReservedVector);
  - make the #SX gate check the window before falling through to `fault_30`.
- Record IOAPIC/MSI vectors 16-31 as unsupported.
- Fix the test `x2avic_registers.rs:330` (it asserts that vector 16 is mirrored unmasked). The test at `:756` gives false confidence: production never reaches it.

### L1. The AwaitSipi wait bound is uncalibrated and now reachable in production; GIF stays closed for the whole wait

- **Severity:** low
- **Where:** `crates/hypervisor/src/host/resident/runtime.rs:1768-1782, 1874`

**Defect.** After guest INIT, the target waits for SIPI with 20,000,000 iterations of `terminal_requested` + `peek` + PAUSE. It then stops terminally (stage 9, `WaitExhausted`). GIF is open only when a command is present, so SMIs and NMIs are held for the whole wait.

**Failure scenario.** On Zen 5 each iteration is only a few tens of cycles, so the bound is plausibly tens of milliseconds (unmeasured). A guest whose INIT-to-SIPI gap is longer loses the AP with a stage-9 stop: the MP-spec 10 ms stall plus scheduling, or sequential starts after a broadcast INIT. Platform SMIs are delayed for the same period.

**Minimal fix:**
- Bound the wait by TSC time, calibrated from the admitted CPUID 15h/16h or the captured TSC frequency.
- Periodically open GIF (for example with `acknowledge_init()`) while idle.
- Measure the guest's INIT-to-SIPI gap on hardware.

### L2. An AVIC_INCOMPLETE_IPI trap is discarded when an INIT is serviced in the same dispatch

- **Severity:** low
- **Where:** `crates/hypervisor/src/host/resident/runtime.rs:1102-1110` (`service_startup` returns `Some(true)` before the 401h handler at 1325)

**Defect.** The 401h ICR write has already completed. If the same dispatch applies a queued INIT (and SIPI), `dispatch_body` resumes without `handle_incomplete_ipi`, so the fixed or startup IPI is never delivered.

**Failure scenario:**
1. A running CPU A writes a fixed IPI that exits with ID 0 or 2.
2. An INIT for A was queued just before.
3. A resets and its targets never receive the IPI. On real hardware the IPI would have been sent before A's INIT was recognized.

For 402h this is harmless, because `commit_init` retires every held source.

**Minimal fix.** Handle 401h (the trap effect) before `service_startup`, or make `service_startup` leave trap exits to their handler.

### L3. The last arm refusal comes after irreversible physical commits

- **Severity:** low
- **Where:** `crates/hypervisor/src/host/resident/runtime.rs:881-898`

**Defect.** `table.set_running` (code 9) is checked only after several changes have already been made:
- VM_CR.R_INIT is set (883);
- the destination record is committed (886);
- the backing page is installed and the physical LVT masks are written (890);
- the physical TPR is set to 0 and SVR to 1FFh (893-896).

**Failure scenario.** If the table entry for this ID were not valid (for example a layout or table-population bug), arm returns 9. DXE maps that to 20, and the loader continues natively with R_INIT set: any later INIT becomes #SX in the OS. The loader's TPR and SVR and the masked LVTs are also lost.

**Minimal fix.** Before the first commit, read `table.entry(assigned_id)` and require the valid bit and the expected backing PA. Leave `set_running` as the only infallible step.

### L4. Arm code 11 discards the refused register and value

- **Severity:** low
- **Where:** `crates/hypervisor/src/host/resident/runtime.rs:797`; `crates/hypervisor/src/svm/x2avic/registers.rs:446-449`

**Defect.** `CapturedInterface::capture` returns `CaptureRefusal { msr, value }`, but arm maps it to a bare 11, and DXE maps that to 20. Nothing outside tests reads the fields.

**Failure scenario.** Suppose the firmware leaves LINT0 with bit 13 set (Intel-style polarity) or a reserved message type. The physical run reports only "arm failed (20)", with no MSR or value.

**Minimal fix.** Keep the refusal in a retained runtime record (or an extended arm result) and export it to the DXE admission hint, so the rule "honest unsupported reporting" holds for this plausible first-boot refusal.

### L5. `HostX2Apic` offers safe RDMSR/WRMSR of any MSR index

- **Severity:** low (unsafe contract leak)
- **Where:** `crates/hypervisor/src/arch/x86_64/apic.rs:175-185`; `pub unsafe fn read_physical_msr` and `write_physical_msr` at `:194` and `:208`

**Defect.** The safe trait methods forward any `u32` MSR index and value to raw RDMSR/WRMSR. Soundness depends on the safe `svm::x2avic` code passing only implemented x2APIC registers with legal values, which the `new()` contract states but cannot enforce.

**Failure scenario.** A future owner passes an unvalidated guest value or index through `PhysicalX2Apic::write`. Safe code then causes a host #GP (terminal) or an unintended MSR write, without an `unsafe` block.

**Minimal fix:**
- Check `msr` in `X2APIC_MSR_FIRST..=X2APIC_MSR_LAST` inside the impl, and make out-of-range access a no-op or panic (at minimum a `debug_assert`).
- Make both raw primitives private to `apic.rs`; `HostX2Apic` is their only caller.

### L6. An INIT or SIPI reported with 401h ID 4 is a stop, not routed

- **Severity:** low
- **Where:** `crates/hypervisor/src/svm/x2avic/ipi.rs:160-168`

**Defect.** For ID 4 only a fixed IPI with vector below 16 is accepted. INIT, whose vector is 0 by definition, and SIPI with a vector below 10h become `InconsistentVectorExit` (F559h).

The x2avic-manual `apm-avic` fact sheet leaves the ID for non-fixed message types unresolved (U8). Its own implication is to route INIT/SIPI by the ICR fields, not by the ID. Nothing is delivered for ID 0, 2 or 4, so routing such an exit to the startup router is safe.

**Failure scenario.** If the silicon checks the vector before the message type, every INIT IPI (vector 0), and every SIPI whose trampoline page is below 10000h, stops the BSP.

Informative: Linux KVM drops ID 4 but emulates INIT/SIPI under ID 0, which suggests hardware reports ID 0. This is unmeasured here.

**Minimal fix.** Classify INIT/SIPI to `Startup` for IDs 0, 2 and 4, before the ID 4 vector check. The startup router already validates the vector.

### L7. Code with no production caller remains

- **Severity:** low (cleanup rule: "production-uncalled code is removed")

The items:
- **`IdentityNpt::trap_page`** (`crates/hypervisor/src/memory/npt.rs:637-692`): its only callers are tests (`npt.rs` unit tests and `tests/identity_npt.rs:488,500`). The IOMMU and LAPIC traps are gone. The `trapped_pages`/`trapped_count` state and the trapped-page branches of `translate` exist only for it. The comment at `npt.rs:696` ("native LAPIC leaf holes stay unchanged") is stale.
- **`terminal::route_failure`** (`crates/hypervisor/src/host/resident/terminal.rs:386`): kind-14 encoder, tests only. F521h replaced it; this predates the batch, but the batch owns the startup route stop.
- **`NativeStartupTarget::{validate, apply}`** (`crates/hypervisor/src/svm/x2avic/startup.rs:581-619`): tests only.
- **`GuestX2Apic::apic_base()`** (`crates/hypervisor/src/svm/x2avic/registers.rs:283`): tests only. It duplicates `State.host_apic_base`, which holds the same captured RDMSR.
- **Other test-only items:**
  - `FixedIpi::{vector, targets}` (`ipi.rs:76-78`);
  - the `bool` returned by `BackingPage::eoi_stopped` (`backing.rs:174`), ignored by both production callers.

**Fix.** Delete these, or make them `#[cfg(test)]` where a test is their only purpose, and move the affected tests.

### N1. Comments and docs inside the code have drifted

- **Severity:** nit
- **Where and what:**
  - `terminal.rs:406-408`: the "Retired, never reused" list omits F505h, F523h and F524h, which Phase A retired with the route code. The test at `terminal.rs:812` omits them too.
  - `terminal.rs:551-553`: says "Stages 1-9 and 13 carry the AwaitSipi flag". Stages 12 and 15 also carry it.
  - F510h and F520h keep their tags but redefine `info2`. HEAD emitted F520h with raw EXITINFO2. A decoder for old images reads the two differently, and nothing records this.
  - F520h/2 from `dispatch_body` on an NPF exit is exported by `stop_words` as kind 3, with 2 presented as a GPA (this predates the batch).
  - `runtime.rs:1570-1572` (doorbell SAFETY comment) says every inventory CPU is armed before any guest can send an IPI. In fact AP guests run before the BSP is armed, so a software fan-out from an AP guest would doorbell an unarmed BSP. That is outside `ring_avic_doorbell`'s own contract ("a running resident CPU").

### T1. Test gaps and tests that assert questionable behavior

- **Severity:** low

**Untested:**
- The `handle_avic_msr` glue: the nRIP path versus the byte-fetch path, the CPL>0 and TF refusals, the RAX/RDX writes, RIP commit and interrupt-shadow/RF clear. Only `msr_completion` has a test.
- The 60h path: capture, then `sync_eoi_intercept` (map bit and clean bits).
- `service_startup` INIT to AwaitSipi to SIPI.
- `dispatch_body` ordering (M2, L2).
- Route-lease contention (M1).
- The `prepare()` alias PTE value and index. Only disassembly evidence exists; a small pure helper would make it testable.

**Questionable assertions:**
- `x2avic_registers.rs:330` enshrines M3: vector 16 is mirrored unmasked.
- `x2avic_registers.rs:756` tests a refusal that production can never reach for 16-31.
- `x2avic_ipi.rs:103` enshrines L6: an INIT with ID 4 stops.

---

## Checked and sound

- **`handle_avic_msr`:**
  - Every fallible check runs before emulation.
  - nRIP is used only for 64-bit code with NRIPS; otherwise the byte evidence path applies, bounded by `native_startup_instruction_mode`.
  - RIP is committed once, as an absolute nRIP.
  - EDX:EAX uses low DWORDs. A read zero-extends RAX and RDX; a write leaves RAX unchanged.
  - #GP is queued only for outcomes with no side effects (every `GeneralProtection` return precedes stores and physical writes). CPL>0 gives #GP without emulation.
  - The EOI intercept is resynchronized after every completed or faulted access, with `invalidate_all` on a change. The EOI bit index matches `Msrpm::set`.
- **Register owner:**
  - The masks match D2.
  - `intercepted`, `implemented`, `read` and `write` partition 800h-8FFh consistently (checked against the test's literal lists).
  - APIC_BASE reserved bits: `AddressPolicy` bounds the width to 32-52, so the shift cannot overflow.
  - `CapturedInterface::capture` is read-only; `install` writes a physical LVT only to add a mask.
- **IRQ bridge:**
  - The ledger invariant holds: physical ISR equals the held set, and a newly accepted vector is always the highest, because host TPR is 0.
  - Spurious and ExtINT are classified by the vector's own ISR bit.
  - Publication precedes the physical EOI.
  - The drain is bounded (225 rounds covers 224 held sources).
  - Level completion is decided by the ledger, not by TMR.
  - Stale-TMR clearing races are benign without vector sharing.
  - `level_eoi_exit` handles both ISR states and refuses a vector that is not the highest.
- **401h and 402h:**
  - Bit 12 is masked only for classification; the stop evidence keeps the raw EXITINFO1.
  - RIP and nRIP are never used.
  - Doorbell targets are validated before any publication.
  - `enqueue` sets TMR before IRR, and the doorbell follows the publication.
  - The inventory resolves only slots below `count`, and arm binds `count` to `POOL.1/1MiB`.
  - Undecodable exits and 402h outside D1 stop with the raw EXITINFO.
- **Guest INIT (`guest_init`):**
  - Every fallible step precedes the first effect: `prepare_init`, EFER on a copy, the destination token.
  - The commit follows the D9 order.
  - The CPU commit's re-validation cannot fail after the identical `validate_x2avic`, because `commit_init` touches no VMCB field.
  - Dropping the unused destination token on a terminal failure is sound.
  - The debug reset runs last, and the mailbox is completed afterwards.
  - AwaitSipi never enters the guest, and terminal requests are polled.
  - `pending_fault`/EVENTINJ were already cleared by `check_exit_event` before servicing.
  - `initialize_ap_after_init` sets V_TPR to 0 and clears the clean bits.
- **SIPI:** unchanged and correct.
- **Concurrency:**
  - Remote publishers use only atomic TMR and IRR operations.
  - The INIT reset's per-bank order (ISR, TMR, IRR) gives the documented discard-or-keep outcome.
  - A remote edge publication cannot leave a stale TMR.
  - MSRPM edits are per CPU (image-local static) and made only with the guest stopped.
  - `runtime.S` runs VMRUN after `sti` with GIF=0, so V_INTR_MASKING=1 still produces INTR exits (fact sheet U15).
- **Stop codes:**
  - No tag collisions: codes are at most 15, the IRQ variant has 4 bits and the other vector has 9.
  - The detail fields fit in bits 63:16.
  - `init_error_code` stays within 32 bits, so the startup record keeps the APIC ID.
  - `stop_words` exports every F5xxh stop as kind 0 with the guest RIP.
  - The `read_snapshot.py` decoders match the encoders.
- **Remote aliases (`prepare`, DXE):**
  - The PA formula is correct and the flags are P|RW|NX (PAT 0).
  - The slot bound comes from `valid_pool_slot` (32 MiB or less).
  - There is no overlap with the image, table, cache, startup or scratch pages.
  - The PTEs are written before the root is ever loaded.
  - `common_backing_offset` and `backing_aliases` cover offset agreement, dense slots, `pool_bytes == count*1MiB` (it equals `keep_pages`), and absent leaves (`NotPresent{level:1}`).
  - Every `host_closure` caller passes the whole pool (install, AP observer, callback).
  - `DIRECTORY_VERSION` 10 has exactly one consumer.
  - The image bound agrees in `payload.ld`, `prepare` and `directory_valid`, and a test ties the linker-script literal to the constant.
- **Phase A:**
  - `TABLE_COUNT` = 8 is enough: `IdentityNpt::new` uses at most 6 tables and the ECAM PD 1. `LowMemoryNptStorage` uses a u16 visited mask and relocates correctly. The returning arena keeps 8 NPT pages, and `host_closure` now uses `size_of::<TableStorage>()`.
  - The `linked_symbols` stand-ins are `cfg(test)` only, so the payload links the real externs.
  - The stack-audit regex accepts single-quoted `-C`/`--cfg`, does not match `--check-cfg`, and still refuses overrides.
  - The CRLF normalization in `memory_attribute_probe.rs` is correct.
  - The `fetch.rs` `instruction` parameter removal preserves behavior: every caller was an instruction fetch.
  - The removed `lib.rs` aliases have no remaining users.
