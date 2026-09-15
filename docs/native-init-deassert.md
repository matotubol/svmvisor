# Native INIT deassert compatibility

The September 15, 2026 physical capture stopped CPU 0 on an APIC ICR low write of `0x8500`, with `unsupported_startup_encoding`. This means INIT delivery mode, level trigger, deassert, vector zero. The checked guest instruction reached the shared native startup owner, which refused before publishing a target reset. A new loading animation and a different refusal are progress observations, not proof of Windows boot.

## Behavior

The native xAPIC and x2APIC startup owner now completes this exact INIT-deassert encoding without any target action. It preserves the existing explicit physical remote assignment, mailbox identity/readiness, shared routing lock and unique effective destination checks. Nonzero INIT vectors, unowned or ambiguous destinations, logical routing and shorthand remain refused. Successful deassert consumes no FIFO capacity, does not erase pending INIT/SIPI entries, does not send a host notification or physical INIT, and does not change target CPU/APIC state. The source adapter retains ordinary ICR readback and checked instruction completion; pending event and continuation checks precede it.

Asserted INIT still publishes exactly one existing target-owned reset command. SIPI still uses the existing ordered mailbox and duplicate-SIPI behavior. Deassert never becomes a second reset.

## Evidence and limits

AMD APM volume 2, publication 24593 revision 3.44 March 2026, original PDF pages 704–706 / printed 642–644, Figure 16-18 and Table 16-4, defines the fields but excludes level-triggered deassert from its listed valid INIT combinations. PPR 57896 revision 3.00 August 28, 2024, Family 1Ah Model 44h B0, original PDF/printed 61–62, defines writable trigger/level/ICR fields without specifying this excluded combination's outcome. These pages were reviewed as rendered images including complete tables and adjacent explanations. The PPR contract report records applicability and cross-references.

This correction is an explicit compatibility model, not a claim that those manuals guarantee no action on Ryzen 9900X silicon. Linux v6.19 commit `05f7e89ab9731565d8a62e3b5d1ec206485eeb0b`, `arch/x86/kvm/lapic.c`, handles INIT only if edge-triggered or asserted; level deassert changes no target pending event and causes no target kick. ICR write readback still completes. Pinned QEMU commit `67cd0563376a44c0dad9df627a97c8c9ac0dfa60`, `hw/intc/apic.c`, returns before CPU INIT for level deassert after a legacy arbitration-ID assignment. These implementation precedents support no CPU reset, while their general virtual platforms do not prove modern AMD silicon details.

The independent Windows binary review recovered a real `0xc500 -> 0x8500 -> SIPI` startup sequence from the inspected local Windows kernel. The exact stopped instruction/build was not exported by the card, so this corroborates software usage without attributing the physical write to that binary. F7 BIOS analysis did not identify an actual `0x8500` APIC write; a coincidental descriptor literal was excluded.

## Regression coverage

Host tests cover a full four-entry FIFO with repeated deassert, ordered assert/deassert/SIPI across multiple targets and generations, deassert before assertion and while INIT/SIPI remain pending, routing lock and alias refusal, ICR readback, and x2APIC pending-event refusal without source mutation. The implementation owner's focused run passed 26 tests; the final batch validation records authoritative complete counts.

The executed guest fixture requires generation 1 level-assert `0xc500` and generation 2 edge INIT `0x4500`, each followed by `0x8500` readback and SIPI. It also deasserts after the AP runs with seeded debug state and requires continued progress and unchanged restart counts. The harness requires `native-guest-init-deassert-pass` for every AP and generation, alongside exact INIT/SIPI counts and the existing VM_CR and supervisor APIC protection-key witnesses. These are fixture requirements; consult the final batch summaries for actual execution status.

Direct unvirtualized Zen5 deassert effects remain unmeasured. No timing baseline, Windows boot, Hyper-V/VBS coexistence, or sandbox containment is established by these tests. The physical no-backup flash workflow independently verifies the full 5 MiB readback and does not itself activate the image or prove guest execution.

Evidence is retained under `work/native-after-cet-2026-09-15`: `ppr-init-contract.md`, `init-deassert-implementation.md`, `init-deassert-independent-review.md`, `bios-f7-binary-ninja-review.md`, `windows-startup-binary-attribution.md`, and final source/linked/package/flash reviews. A disposable unvirtualized AP witness design is recorded separately in the implementation report; it has not been executed.
