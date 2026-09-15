# Timer interrupt and synchronous exception overlap — 2026-09-13

The emulator now preserves a pending timer interrupt while reflecting a guest
#UD, #GP or #PF, completes the guest fault handler, and delivers the timer once
after the guest unmasks it. All 35 regression profiles passed.

## Implementation and ownership

The authoritative implementation remains `C:/Users/mato/.codex/worktrees/7a58/svmvisor`.
One fresh GPT-6 Astra High agent owned core implementation/tests; root owned
harness, assembly, parser, integration and validation. Additional agent starts
hit the session thread limit. Root reviewed the agent's core and the agent
independently reviewed root's harness; neither review claims independence from
its author's own implementation. Contracts and review records are in
`work/event-overlap`.

`ScheduledApic::settle_after_exit` observes the actual previous entry before
handling a fault or instruction. A consumed V_IRQ moves the pending IRR bit
to ISR. A still-pending owned V_IRQ is withdrawn from the VMCB, with the same
IRR bit retained. This permits strict synchronous fault reflection and APIC
register emulation without dropping the interrupt or duplicating its owner.
Clock service follows settlement under the old timer configuration.

`arm_pending` selects through the existing controller and leaves IF/shadow
gating to hardware. The preemption path retains its previous IF/shadow gate.
EVENTINJ retirement remains an explicit operation after a real entry/exit;
no competing V_IRQ is armed beside a newly reflected exception. Invalid entry,
interrupted IDT delivery, unsupported controls and ownership conflicts remain
refusals. Fault reflection preserves guest RIP; the guest handler alone changes
its saved continuation to skip the deliberate test fault.

No parallel APIC, second IRQ queue or new transition assembly was introduced.
The harness uses the existing code/IDT/stack backing, immutable instruction
fetch, VMRUN/xstate/clock bridge, MSR/MMIO adapters and ASID invalidation.

## Executed cases and validation

Each completed trace contains 48 sessions: three faults, two blockers (IF0 or
TPR6 with IF1), two APIC buses, four repetitions. The guest programs a one-shot
timer; bounded stopped-host clock polling establishes pending overlap at an
actual guest checkpoint. This is controlled event ordering, not an asynchronous
arrival-time measurement.

The guest fault handler checks hardware-stacked RIP, CS, old RSP, fault error
and CR2 where applicable. Its checkpoint proves IF0 and zero IRQ deliveries.
A post-IRETQ checkpoint proves restored IF and the retained pending timer.
After guest CLI/TPR0/STI, an INC witnesses the STI shadow; the IRQ handler
checks that hardware stacked the exact following instruction address.
Actual guest EOI clears ISR, IRETQ restores execution, and a final extra entry
with IF1 checks count1 and empty IRR/ISR/flight. Final RSP and stack/IST canaries
are also checked.

- **235 core tests**, **251 returning-DXE tests**, and the core UEFI check pass.
- **35 emulator profiles** pass: three strict CPU, four ownership, seven
  synthetic, ten extended-state, ten relocation and one RDTSCP-disabled.
- All 25 completed interrupt traces pass final parser revalidation, each with
  **552 new guest entries, 72 deferrals, 48 faults, 48 IRQ dispatches and 48 EOIs**.
  That is 1200 overlap sessions and 13800 new entries across the completed traces.
- The other profiles retain their expected early failures; no overlap completion
  is inferred for them. Existing backend/coverage distinctions are preserved.
- **47 negative evidence checks** pass independently on both flat and UEFI
  traces. The shared parser rejects absent, duplicate, malformed and inconsistent
  markers/metrics. The harness reviewer additionally ran 32 independent mutations.
- Six new core tests cover refusal atomicity, blocked periodic coalescence and
  cancellation, fault reflection with retained IRR, and an ISR with a second
  pending edge. That last case is a state test, not real guest execution here.

The final strict image SHA256 is
`50e74cbed7e865ce370c066375dbfb756cf715b39513770913b1f5834e2d6dea`.
The corrected QEMU runtime remains
`581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`.
Final logs and metrics are in `work/event-overlap/final-validation`. The initial
compile failure (private flags accessor) and first successful pre-review run
remain in `validation`; they are not relabeled as final evidence.

## Timing, references and limits

The sampled entry/exit span envelope is 38764..339416 raw emulator TSC counts.
It includes bridge work; this is not isolated VMEXIT cost, a latency distribution,
physical calibration or a native-versus-virtual comparison. Every session has
a 16-entry bound and 200000-poll bound, with the existing outer QEMU timeout.

Normative source: AMD APM volume 2 publication 24593 rev 3.44, sections 8.9,
15.7,15.20,15.21.4–5 and 16.6. The primary user-library PDF and worktree PDF
match SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Saved V_IRQ indicates whether dispatch began; EXITINTINFO must still be checked
to distinguish interrupted delivery. No physical processor timer rate is inferred.

General nested delivery/double-fault recovery, NMI, trap-gate preemption, MOV-SS
shadow, multicore scheduling, physical timer routing/calibration, Windows boot,
Hyper-V/VBS/HVCI compatibility and malware containment remain unestablished.
No physical image was programmed or activated during this batch.

Source, compiled harness artifacts, runtime, logs and referenced results are archived under C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/event-overlap-2026-09-13 with a SHA256 manifest. Large FAT disk images remain in their original workspace evidence directories; extracted ESP contents, drivers and ROMs are included in the archive.

