# Independent x2APIC/x2AVIC manual-conformance tests

Date: 2026-09-16 (final run 17:31Z). Base: HEAD eafe33a plus the uncommitted Phase A/B tree.

- Test file: `crates/hypervisor/tests/x2avic_manual_conformance.rs` (3079 lines, SHA256 `d8cf437b...9614`).
- Command:
  `cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --target-dir target/independent-tests --test x2avic_manual_conformance`
- **Result: 159 tests. 156 passed, 0 failed, 3 ignored.** All 3 ignored tests are real discrepancies. No compiler warnings.

## Implementation under test

SHA256 of the sources at the final run. Other agents were editing the tree at the same time, so results apply only to these versions.

| File | SHA256 |
|---|---|
| `src/svm/x2avic/backing.rs` | `cff74871...b979` |
| `src/svm/x2avic/exit.rs` | `1de0fc3f...a46` |
| `src/svm/x2avic/ipi.rs` | `b31a7f64...8ac3` |
| `src/svm/x2avic/irq.rs` | `a7e31f46...7428` |
| `src/svm/x2avic/mod.rs` | `b46d46b5...2bab` |
| `src/svm/x2avic/registers.rs` | `aeca7d7c...29cc` |
| `src/svm/x2avic/startup.rs` | `78f1646f...cfed` |
| `src/svm/x2avic/table.rs` | `d190bab4...36d2` |
| `src/svm/permission_maps.rs` | `e1c3fb6f...8d8a` |
| `src/arch/x86_64/apic.rs` | `c8f43ed5...59ac1` |
| `src/arch/x86_64/msr.rs` | `70667f1c...387e` |
| `src/svm/vmcb.rs` | `8fa35065...8dcb2` |

## How independence was kept

**Where the API surface came from**
- A signature extractor in the session scratchpad (`pubsig.py`) printed only these parts of the source:
  - `pub` items;
  - type bodies;
  - trait and impl headers, and trait method signatures;
  - doc comments.
- It printed no function bodies and truncated constant values at `=`.
- Before use, I ran it on a synthetic file full of marker bodies. No body text came through.
- It was run on:
  - `svm/x2avic/*.rs` and `svm/permission_maps.rs`;
  - `arch/x86_64/apic.rs` and `msr.rs`;
  - `memory/address.rs`;
  - selected `svm/vmcb.rs` signatures.

**Disclosure**
- One early `grep '^\s*pub'` over the x2avic files printed whole signature lines. That exposed:
  - a few one-line accessor bodies (`FixedIpi::vector/targets`, `BackingPage::is_pending` → `self.bit(apic::IRR, ..)`, `NativeX2AvicProfile::table_control` → `table | maximum`);
  - the values of `PAGE_BYTES`, `MAX_ID`, `ENABLE_BITS`, `NATIVE_CONTROL` and `GUEST_APIC_VERSION`.
- None of these is used as an expected value. The tests restate each value from the manual (Table B-1 pp740-742, 15.29.5.2 p571, Figure 16-4 p632).

**Existing tests, read only for API usage**
- The header, fake and environment of `x2avic_registers.rs`.
- The helpers in `x2avic_ipi.rs`.
- The startup-routing and `apply_x2avic` fixtures in `native_guest_startup.rs` and `x2avic.rs`.
- The mailbox and route-guard usage in `native_destination_routing.rs`.
- Grep lines of `permission_maps.rs`.

**Not read:** the `phase-b-*-notes.md` implementation notes, any function body in the restricted files, and `runtime.rs`.

**What was read:** the brief (`phase-b-brief.md`, decisions D1-D10) and the four fact sheets.

**Rendered pages**
- APM Vol.2 rev 3.44 pages were rendered into `pages/` with `render_pages.py` (SHA256 3d9dcb3f...c48c, matching the fact sheets).
- Viewed as images (PDF page = printed page + 62):
  - p518 (PDF 580)
  - p566-568 (628-630)
  - p572 (634)
  - p576-582 (638-644)
  - p628-633 (690-695)
  - p635-644 (697-706)
  - p646-647 (708-709)
  - p650-652 (712-714)
  - p654-662 (716-724)
  - p740-742 (802-804)
  - p758 (820)
- Rendered but not viewed; the fact sheets were used for these: p570-571 (632-633) and p574 (636).
- PPR and ACPI facts come from their fact sheets, which cite rendered images.

## What is tested (task items to tests)

**1. Table 16-6 row by row**
- The interception profile for all 256 MSRs in both directions matches Table 15-22 and D1. APIC_BASE is intercepted.
- The private MSRPM equals this profile whatever the x2APIC bits held before.
- EOI-write interception follows held level sources (D6).
- `table_16_6_outcome_matrix_for_every_msr` checks every MSR and direction with six values, classifying each result as hardware / #GP / read value / written / refused.
- Separate tests cover:
  - unlisted MSRs (800-801, 804-807, 80C, 80E, 829-82F, 831, 83A-83D);
  - 840-8FF (U10);
  - 802h write;
  - writes to read-only registers (U1);
  - reads of 83Fh and 80Bh (U2);
  - ESR and EOI non-zero writes;
  - emulated reads of APR and the current count;
  - hardware-owned accesses refused as `UnownedAccess`;
  - the MSR formula of 16.11.1.

**2. Reserved bits**
- 51 generated tests, one per boundary: a reserved bit gives #GP with no side effect, and the legal neighbouring bit is accepted.
  - Timer 18/15/13/11/8 (plus 31, 32, 63).
  - Thermal, perf and error 17/15/13/11 (plus 32, 63).
  - LINT0/LINT1 17/13/11 (plus 32, 63).
  - SVR 10 and 12 (plus 31, 32, 63).
  - Divide 2 and 4 (plus 31, 32).
  - Initial count 32 and 63.
- A 64-bit sweep covers every register.
- A self-check pins the masks to the figures.

**3. SVR software disable**
- All six virtual LVTs and all physical mirrors become masked, and the physical SVR is never written.
- LVT writes while disabled stay masked.
- Masks survive re-enable until the guest rewrites them (U13).
- Counts and divide stay writable while disabled.
- The SVR vector and FCC are stored.
- A captured live ExtINT LINT0 is masked both virtually and physically.

**4. APIC_BASE**
- Admission accepts only enabled x2APIC at the reset base, and reads return the shadow.
- 11→11 with the same base (BSC ignored) completes.
- 11→10 and 11→01 fault, including with another base.
- Reserved bits 63:52, 9 and 7:0 fault.
- Base bits at or above the physical width fault for widths 40, 44 and 48.
- With a 52-bit width, bits 51:48 are base bits, so setting one is a relocation.
- 11→00 is refused as `ApicDisable`.
- A base change is refused as `ApicRelocation`.

**5. Reset and INIT**
- `reset_stopped` and `reset_after_init_stopped` produce the Table 16-2 values for all 24 MADT IDs:
  - TPR, PPR, ESR, ICR low and high, initial count, divide, and the ISR, TMR and IRR banks are 0;
  - SVR is FFh;
  - the six LVTs are 10000h;
  - ID and version are preserved;
  - LDR is derived from the ID.
- The APR, EOI, RRR and current-count slots are 0.
- `commit_init` also:
  - puts the physical timer, LVTs, count and divide at their reset values;
  - acknowledges the held level source with exactly one physical EOI;
  - re-accelerates EOI;
  - leaves APIC_BASE unchanged.
- After INIT, guest reads of APR and the current count return 0.
- `prepare_init` is read-only and refuses a physical in-service vector the ledger does not own.
- On the CPU side (D9 step 5):
  - V_TPR becomes 0;
  - AVIC, x2AVIC and V_INTR_MASKING stay enabled;
  - clean bits are cleared;
  - 0E0h and 0F8h are kept;
  - the vCPU waits for SIPI; the first SIPI starts it and a second is ignored.
- LVT writes after INIT stay masked until the guest sets ASE.
- The version register fields match Figure 16-4.

**6. Timer divide**
- All eight Table 16-3 encodings (0-3, 8-B) are accepted and mirrored exactly; the other eight 4-bit values fault.

**7. ICR and incomplete-IPI policy**
- Exit ID 0 routes by message type:
  - INIT and STARTUP (valid Table 16-4 forms) go to startup routing;
  - fixed edge is delivered in software;
  - fixed level is refused as `LevelTriggered`;
  - SMI and NMI are refused.
- Message types 1, 3 and 7 are refused as reserved.
- Reserved bits 31:20, 17:16 and 13 are refused, and bit 12 separately.
- ID 1 is `TargetNotRunning`.
- ID 2 is full software handling.
- ID 3 is `InvalidBackingPage`.
- ID 4 drops a fixed vector below 16 and is `InconsistentVectorExit` for anything else.
- IDs 5 and above are `UnknownReason`.
- Fixed vectors below 16 are dropped under IDs 0 and 2 as well.
- Startup routing:
  - directed INIT and SIPI reach only their target;
  - INIT and SIPI to all-excluding-self reach every other CPU;
  - self and all-including-self shorthands publish nothing;
  - an absent destination publishes nothing.

**8. Destination matching (MADT IDs {0-11, 16-27}, sender 13h)**
- Physical mode:
  - the 32-bit exact ID matches, including the sender itself;
  - 32-bit IDs whose low byte aliases an inventory ID do not match;
  - slots follow the admission order.
- Physical and logical FFFFFFFFh are broadcasts including self.
- DEST FFh is not a broadcast (U19/D5); it matches only an ID-255 CPU.
- Logical mode:
  - cluster and bit matches work;
  - logical FFh means cluster 0, bits 0-7;
  - mismatches (no bits, absent IDs, empty clusters, cluster FFFFh or FFFEh) deliver nothing.
- Shorthands self, all and others work for both DM values and several DEST values.
- Empty target sets are dropped.
- Fan-out:
  - sets IRR and clears a stale TMR bit;
  - rings one doorbell per remote target in slot order;
  - never rings the sender;
  - directed, self and logical cases are covered.

**9. EOI**
- An intercepted EOI clears only the highest ISR bit and recomputes PPR:
  - PP and PPS follow p651;
  - PPS is 0 when PP ≠ TP (U20).
- It completes the held level source: exactly one physical EOI, and the stale TMR bit is cleared.
- A nested edge EOI leaves the level source held.
- An EOI with an empty ISR changes nothing (U18).
- EOI sequences through `eoi_stopped` are checked, including the empty-ISR case.
- The 402h level-EOI fallback works whether hardware already cleared the ISR bit or not (U11), and refuses a vector that is not the highest.
- Non-zero EOI writes fault.

**10. APR**
- 11 worked cases and a 294-case sweep of Figure 16-22: AP is the highest of TP, the ISR class and the IRR class; APS equals TPS only when AP = TP (D2 reading).

**11. Doorbells and the physical-ID table**
- `DoorbellTarget` accepts 0-254 and rejects 255, 256, 1FEh, 10000FEh and larger (Figure 15-22, D8).
- A fan-out that includes an ID the doorbell cannot address (255 or 275; both inventories are admitted) is refused before any publication.
- Table entries follow Figure 15-17 for all 24 IDs:
  - V is bit 63;
  - bits 61:52 are 0;
  - the backing pointer is bits 51:12;
  - the host ID is bits 11:0.
- IsRunning toggles only bit 62 of its own entry.
- The table holds 512 entries (511 is accepted; 512 is rejected).
- Misaligned or out-of-range backing addresses are rejected and leave no entry.
- The doorbell MSR is C001_011Bh and is intercepted in the guest MSRPM.

**12. AVIC exit decoding**
- 401h: `icr` is EXITINFO1; the ID is EXITINFO2 63:32; the index (11:0) is reported only for IDs 1-3; reserved bits 31:12 are ignored; IDs 5 and above are refused.
- 402h, for all 256 offsets: R/W is bit 32, the offset is bits 11:4, and the EOI vector is reported only for a write to B0h; EXITINFO2 bits 63:8 are ignored.
- Other exit codes are rejected.

**Adjacent rules also covered**
- IRQ bridge:
  - a spurious vector gives no publication and no EOI (16.4.7 p640);
  - an edge interrupt is published before its EOI, with TMR cleared;
  - a level interrupt is published with TMR set and held;
  - a vector with no ISR bit is refused.
- Arm-time capture and install:
  - PPR equals TPR;
  - RO bits are dropped;
  - counts are not rewritten;
  - with ASE=0, the masks are forced;
  - captured values with reserved bits are refused;
  - the captured ICR never shows bits 12, 13, 16, 17, 20 or 31.
- CPUID admission requires ECX[21], EDX[13] and EDX[18] (p654, p578).
- VMCB 060h bits 31, 30 and 24 are set and its SBZ bits are clear; the 0E0h/0F8h fields and MAX_INDEX ≤ 511 hold.
- Backing-page slots are 32-bit at 16-byte offsets within one 4K page.
- Vectors 0-15 cannot be enqueued, and the bank mapping follows p650.
- `highest_in_service` returns the highest ISR bit.

## Discrepancies (ignored tests)

1. **`init_to_a_logical_destination_reaches_the_matching_cpu`**
   - **Manual:** APM2 Table 16-4 p644 makes INIT valid with the "Destination" shorthand in either destination mode (DM, Figure 16-18 p643). 16.14 p662 gives the logical match: cluster 1, bit 11 selects x2APIC ID 1Bh.
   - **Brief:** D5 routes INIT/STARTUP to the startup mailbox, allowing shorthand 00 or 11. D10 does not list logical INIT/SIPI as unsupported.
   - **Observed:** `route_x2avic_startup(0x0001_0800_0000_0D00)` returns `Err(UnownedStartup { value: 0x1_0800_0000_0D00 })`, and nothing is published.
   - **Expected:** `Ok(())`, with `Init` published only to the mailbox of ID 1Bh.
   - The refusal matches the implementation's own `NativeIcrError::UnownedStartup` doc ("Unsupported logical ... destinations"). Because it fails safe as a stop, the impact is low (Windows sends physical INIT/SIPI). The gap is that this unsupported case is missing from D10 and the docs.

2. **`fixed_ipi_is_not_accepted_by_a_software_disabled_target`**
   - **Manual:** APM2 16.3.1 p629 (repeated under ASE, Figure 16-17 p641): while SVR bit 8 is clear, "Further fixed, lowest-priority, and ExtInt interrupts are not accepted."
   - **Observed:** a software fan-out of a fixed IPI to all CPUs sets IRR in the target whose backing SVR is FFh.
   - **Expected:** that target's IRR stays clear, while the enabled targets still receive the IPI.
   - **Brief:** D5 never considers the target's software-enable state.
   - **Caveat:** 15.29.6.1 does not say whether AVIC's hardware-accelerated IPI path checks the target's ASE. Hardware-delivered IPIs may therefore behave the same way, but the software path is the VMM's own choice.
   - **Practical effect:** a fixed IPI sent between a guest INIT (SVR=FFh) and the OS enabling its APIC stays pending and fires later. A real APIC would have discarded it.

3. **`device_interrupt_is_not_accepted_by_a_software_disabled_guest_apic`**
   - **Manual:** APM2 16.3.1 p629, as above.
   - **Observed:** `irq::capture(0x41, ..)` for a physical edge interrupt publishes vector 41h into a backing page whose SVR is FFh.
   - **Expected:** vector 41h is not pending in the guest IRR. How the host handles the physical interrupt is not asserted.
   - **Brief:** D1 and D6 publish every captured interrupt and do not consider the guest's ASE.
   - **Practical effect:** a level source captured this way would also stay held until a guest EOI that a disabled APIC may never issue.

## Own mistakes corrected during development

These were test errors. No expectation was weakened.

1. 402h EXITINFO2 case: `0x100` names vector 0, which can never be in service (16.6.3 p647). It was replaced with `0x110` (vector 10h). The implementation's refusal of vector 0 is consistent with the manual.
2. Outcome matrix, EOI row: EOI interception is dynamic (D6). Whenever the write reaches the owner, it now follows the Table 16-6 rule (zero completes, non-zero faults) instead of being expected as hardware-owned.
3. Outcome matrix, value 1FFh: on thermal/perf/error/LINT entries, 1FFh encodes message type 001b, which D2 refuses. A `Refused` class and the D2/D3 message-type policy were added.
4. `dirty()`: the ISR/TMR/IRR patterns now keep the reserved vector bits 15:0 of bank 0 clear.

## Observations (not marked as discrepancies)

**ICR bit 12**
- The `CapturedInterface::capture` doc says the AVIC_INCOMPLETE_IPI handler "drops" the delivery-status bit 12 of the backing ICR rather than refusing it (decision).
- `Inventory::classify` refuses bit 12 as `ReservedBits`, which agrees with D5 ("13:12 → stop") and 16.13 p661 ("must be zero"). The `icr_delivery_status_bit_stops` test passes.
- Whether the runtime handler clears bit 12 before calling `classify` could not be checked: `runtime.rs` is out of reach and its bodies are off-limits. If it does, that contradicts D5.

**Startup-routing refusals left untested**
- `route_x2avic_startup` also refuses an explicit-broadcast INIT (DSH=00, DEST=FFFFFFFFh) and INIT addressed to the sender itself (per its doc).
- Table 16-4 allows "Destination" but forbids "all including self", and DEST FFh is documented as equal to all-including-self. The manual is therefore ambiguous (U4), and these cases are not tested.

**EOI and `UnownedAccess`**
- An EOI write that reaches the owner with an empty ledger completes (`Written`) rather than being refused as `UnownedAccess`, even though `intercepted(80Bh, Write)` is false. This is harmless and agrees with 16.6.4.

**Doorbell admission**
- `NativeIcr::admit` accepts x2APIC IDs 255 and 275. `deliver_fixed` then refuses them before publishing (`FanOutError::DoorbellTarget`).
- D8's "every host APIC ID ≤ 254" admission must therefore be enforced elsewhere (DXE or runtime admission), which cannot be verified here.

**`route_x2avic_startup` result for an absent destination**
- It publishes nothing, but its return value was not asserted.

**PPS when PP ≠ TP**
- PPS is 0 in every case exercised. That is the implementation's own documented decision for U20; the brief does not state it.

## Manual rules the public API does not let these tests reach

**Hardware-accelerated writes**
- TPR (808h, reserved 63:8), ICR (830h, 31:20, 17:16 and 13:12), SELF IPI (83Fh, 31:8 per the APM and 63:8 per the PPR), and edge EOI.
- Whether these raise #GP for reserved bits is up to x2AVIC hardware (U12). The VMM sees the ICR only after the write has completed (401h).

**VM-exit dispatch in `runtime.rs` (feature `resident-runtime`)**
- RIP and nRIP handling: 7Ch uses nRIP; 401h/402h are traps or faults with nRIP 0 (15.7.1 p509, U10).
- #GP injection (`Vmcb::queue_native_x2avic_general_protection`).
- Doorbell WRMSR (`ring_avic_doorbell` is unsafe and touches hardware).
- Switching the MSRPM plus invalidating VMCB clean bits after `update_x2apic_eoi_intercept` (Figure 15-4 p527).
- Wiring of the APIC_BASE shadow.
- D9 commit steps 6-9.
- Host-mode doorbell (U5) and HLT wake (U16).

**Other behaviour outside the testable surface**
- The live timer (count-down, periodic reload, masking; 16.4.1 p636) runs on the physical LAPIC. Only the mirror writes can be tested.
- Virtual ESR error generation (SIV/RIV/SAE/RAE, Figure 16-16 p640) is not modelled (D10). Illegal-vector and empty-target IPIs can only be checked as "dropped".
- Holding pending interrupts, PPR-based selection and delivery gating (16.3.1 p629, 16.6.4 p651) happen in AVIC hardware.
- The extended APIC space 840h-853h is absent by decision (U10). Its Table 16-2 reset values (00040007h, IER FFFFFFFFh) are untestable.
- Leaving x2APIC mode is refused by decision (D4), so the disabled and xAPIC states of Figure 16-32 are unreachable.

**API shape**
- `Inventory::admit` is `pub(super)`, so an inventory is only reachable through `NativeIcr::admit(..).inventory()`.
- `FixedIpi` can only be obtained from `classify`. As a result, the doc rule "target bits outside this inventory are ignored" cannot be exercised.
