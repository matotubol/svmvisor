# Running-guest timer preemption fixture - 2026-09-13

The emulator now interrupts a continuously running guest using an owned host LAPIC timer, services the guest timer through the existing APIC owner and resumes the exact interrupted instruction. The final 35-profile matrix passed; scope and measurements follow.

## Ownership and scope

This batch extends the stopped-guest clock scheduler with a real emulator host LAPIC timer that can force SVM INTR exits while the guest runs an integer loop. Guest timer state remains in the sole `ScheduledApic`/`FixtureApic` owner; the emulator host timer has separate, explicit physical-source ownership. It does not implement a physical platform timer driver or establish Windows compatibility.

Authoritative implementation: `C:/Users/mato/.codex/worktrees/7a58/svmvisor`. Fresh High-reasoning agents own core contracts, harness implementation and independent review respectively. Root owns shared builds, evidence admission and archival. The user's primary reference library is `C:/Users/mato/Documents/svmvisor/docs`.

## Architectural boundary

AMD APM2 rev3.44 sections 15.5-6, 15.13.1, 15.17 and 15.21.1-4 define the boundary. With V_INTR_MASKING enabled, host IF captured at VMRUN controls physical interrupts independently of guest IF. CLGI protects the transition before host IF is enabled; VMRUN sets GIF after loading guest state, and VMEXIT clears GIF before restoring host state. Host IF must be cleared before exposing host interrupt delivery again.

An INTR exit has no instruction completion: the stopped RIP remains the continuation. EXITCODE 60h alone is not timer provenance. The emulator must verify its owned source and perform a real interrupt acknowledge and EOI; it cannot manufacture a timer event from unused EXITINFO fields. Guest dispatch, EOI and IRETQ are separate observations.

## Host timer and handoff ownership

The emulator timer owner uses a single temporary supervisor RW/NX mapping of FEE00000, with PAT3 and the effective MTRR type explicitly checked as UC. It leaves guest NPT unchanged. Admission requires the fixed enabled xAPIC base, the emulated six-LVT inventory, no delivery in flight and empty host IRR/ISR. PIC masks, LVT programming, TPR, SVR, divider and the temporary F0 IDT gate are owned and restored. Unexpected sources are terminal refusals.

The first UEFI regression correctly refused inherited firmware timer state: initial count 10,000,000, a nonzero current count, masked periodic LVT 00030020, and empty IRR/ISR. The terminal post-ExitBootServices harness now performs an explicit takeover: a retained ownership record and a masked classic timer permit cancellation and establishment of a zero-count baseline. The original count and mode are retained as telemetry. The discarded firmware countdown phase is not restored; this path never resumes firmware. Direct bootstrap execution retains idle-only admission. The finite test owner restores its admitted baseline and programming and removes the temporary mapping.

Each guest entry arms an emulator one-shot host timer at vector F0 with count 100,000/divider1. For an INTR exit, the owner first requires current count zero, before cancellation can erase that expiration evidence. It then cancels remaining counting and checks actual pending F0 provenance and permits one real host interrupt acknowledge/EOI under its installed gate. A timer becoming pending at a voluntary guest exit is counted separately and never labeled preemption. The assembly handler preserves the interrupted host register/flag state, and the existing shared bridge still owns guest GPR, clock and extended-state switching.

## Guest behavior and evidence meaning

The guest programs its APIC through both existing buses and executes an integer loop containing no HLT, hypercall, MSR or other voluntary VM exit. Physical exits in the loop are counted separately from those with no new loop iteration. The counter must never decrease, and every session requires positive progress measured at an actual in-loop physical exit. Timer service and injection are unchanged on a zero-progress exit. The scheduler preserves the interrupted RIP, RFLAGS and saved frame; it supplies the expected interrupted RIP in an owned guest data cell. The guest handler checks its hardware-stacked return RIP, issues EOI and returns through IRETQ. The handler intentionally uses scratch registers; this is a scoped live-loop continuation test, not a claim that arbitrary guest handlers preserve all registers.

There are 16 one-shot and 16 periodic successful sessions, with 48 guest interrupt/EOI/IRETQ continuations. Twelve negative sessions cover IF=0, TPR, timer masking, SVR disable, cancellation and APIC_BASE disable on both buses. They receive 64 physical preemptions each without guest timer delivery. There are 44 sessions total and at most 256 returned entries per session. A failed host timer can prevent VMRUN from returning; the runner's external QEMU timeout is the outer termination bound, not an implemented resident watchdog.

Negative setup assumes cancellation/disable executes before virtual timer expiration. Extreme stalls can violate that assumption and leave previously pending IRR, causing a visible failure. Individual physical exits without new loop progress are counted separately; a session with no progress measured at an actual physical loop exit fails. Periodic expirations coalesce in the existing IRR bit, and time is serviced at observed exit boundaries, without reconstructing an unobserved dispatch timestamp.

The entry/exit timing counter spans the sample before timer arming through context switching, guest execution, timer cancellation and host acknowledgement to the final sample. It is not isolated guest residency or VMEXIT cost. Wake lateness is the sampled service time minus the retained synthetic guest deadline. Neither metric establishes a physical frequency, latency guarantee or native-versus-virtual comparison.

## Validation

The final matrix passed all 35 profiles: three strict CPU profiles, four ownership profiles, seven synthetic profiles, ten extended-state profiles, ten relocation profiles and one RDTSCP-disabled run. All 25 completed interrupt traces pass final parser revalidation: 15 use the idle baseline and 10 use explicit post-EBS rebasing. Early-failure profiles retain their expected outcomes. Core **229 tests**, returning-DXE **251 tests** and the hypervisor UEFI target check passed. Those core logs predate the harness corrections; core source remained frozen, and their provenance is recorded.

The final strict AVX/SSE/FXSAVE image SHA256 is `053aad14bf9e906c0e29ce93632724392cb9874a7110f283d274440ff97f0744`. The corrected runtime remains unchanged at `581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`. The older runtime retains its known CR8 gaps and is still rejected by strict admission.

| Final strict profile | Guest entries | Physical INTR exits | Host acknowledgements | Acknowledgements at voluntary exits | Deadline lateness min / max, raw TSC |
| --- | ---: | ---: | ---: | ---: | ---: |
| AVX | 1,093 | 841 | 841 | 0 | 3,173,048 / 8,630,900 |
| SSE | 1,075 | 823 | 824 | 1 | 1,496,876 / 8,641,064 |
| FXSAVE | 1,084 | 832 | 832 | 0 | 1,292,188 / 8,607,668 |

Across the 25 completed traces there are 20,952 physical INTR exits and two additional acknowledgements at voluntary exits. Every trace completes 32 positive sessions, 48 guest interrupt continuations and 12 negative sessions. The observed deadline-lateness envelope is 11,832..9,504,651 raw TSC counts; the complete arm/entry/exit/cancel/ack span is 521,004..11,206,184. These envelopes are not statistical distributions or physical latency guarantees.

The final matrix records zero no-progress spin exits. The earlier failing SSE run demonstrated that condition and motivated separate accounting; it is preserved as superseded evidence, not relabeled as final execution of the corrected branch. Source review verifies the nondecreasing counter, the separate equality count and per-session measured progress requirement.

There are **180 evidence-negative checks** on the idle branch and **186 on the post-EBS branch**, including six checks that missing/duplicate/malformed EBS and retained-ownership markers cannot authorize phase discard. The linked boundary audit checks CLGI, the actual INTR-bit branch selecting STI, immediate CLI after VMRUN, restored host context before STGI/return, and a guest spin containing only integer instructions.

The shared evidence parser checks fixed suite markers, unique 16-digit dynamic metrics and relational accounting. Exactly 252 voluntary guest exits are expected, so entries=INTR exits+252. Host acknowledgements=INTR exits+voluntary acknowledgements. Loop progress, per-session bounds and ordered timing extrema are checked. Missing, duplicate, malformed or contradictory records do not count as completed execution.

## Evidence and references

Final evidence is under `work/apic-preemption/final-validation-v2`, including `regressions.json`, `final-metrics.json`, both evidence-control logs and `boundary-audit.json`. The previous `validation` and `final-validation` attempts preserve the inherited-timer refusal and zero-progress failure. Core, harness and independent-review contracts are under `work/apic-preemption`.

The source, exact retained artifacts, runtime, final and superseded evidence, and SHA256 manifest are archived at `C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/apic-running-preemption-2026-09-13`.

User-library APM2 rev3.44 SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c` and PPR57896 rev3.00 SHA256 `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5` match the worktree copies. APM2 chapters 7, 15 and 16 supply the memory-type, interrupt and APIC rules. The PPR applies to Family 1Ah Model 44h B0; no physical clock relationship is inferred from it. Corrected QEMU source revision `f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3` is an informative cross-check that INTR interception precedes interrupt acknowledgement. Independent review found no unresolved blocking finding in the bounded slice.

## Remaining boundaries

Emulator timing is not a physical latency measurement. Physical timer calibration, cross-CPU scheduling, Windows boot and protected-state compatibility, device/DMA/network/storage containment and malware-analysis readiness remain unestablished. Unknown exits, source conflicts and exhausted bounds mean incomplete execution.
