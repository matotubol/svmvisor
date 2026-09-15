# Clock-driven APIC timer and checked HLT fixture — 2026-09-13

The emulator now schedules guest-programmed one-shot and periodic APIC timers
from actual serialized host TSC samples and wakes an intercepted HLT through
the existing interrupt owner. The guest executes its interrupt handler, EOI
and IRETQ, and verifies the exact instruction pointer following HLT. This is
bounded scheduling while the guest is stopped. It does not preempt a guest
that keeps executing without a VM exit.

Implemented in the authoritative dirty checkout
`C:\Users\mato\.codex\worktrees\7a58\svmvisor`, preserving the previous
[SVR/timer fixture](apic-svr-timer-fixture.md), its supplied-tick tests and
historical evidence. Fresh implementation and independent review agents used
the user's reference library at `C:\Users\mato\Documents\svmvisor\docs`.
Root owns serialized builds, final regression results and archival.

## Ownership and clock service

`svm::apic_scheduler::ScheduledApic` consumes the existing `FixtureApic` and
owns the clock epoch, rational-conversion remainder and halted continuation.
It is neither Copy nor Clone and exposes no mutable controller escape.
`LocalApic` remains the sole timer/IRR/ISR/PPR owner. The original MSR and
MMIO handlers perform register emulation; the scheduler does not duplicate
them or manufacture exit records.

Clock service and instruction completion are distinct commit boundaries.
Before dispatching each stopped guest instruction, the runtime services the
elapsed interval under the old timer configuration. It then invokes the
checked register handler. A rejected instruction preserves that post-service
state; it does not undo elapsed time. A successful initial-count write resets
both the rational source phase and the timer-divider phase. Masking, software
disable, cancellation, periodic reload and pending-bit coalescence retain the
previous controller semantics.

The admitted fixture rate is **one pre-divider APIC source tick per 1,024
host TSC ticks**. Guests program divider one and initial count 4,096. This is
an explicit synthetic relationship, not a physical APIC frequency inferred
from a processor manual. The existing clock owner supplies CPUID/RDTSC/CPUID
samples on its exclusively owned CPU. Guest TSC offset remains zero and its
optional scaling ratio remains identity; guest timestamps and host samples
therefore use the same admitted ordering domain. CPU tag zero names this
single owned fixture CPU and does not discover physical topology.

Conversion retains the fractional remainder and uses bounded integer
arithmetic. The next interrupt-producing deadline is derived from current
timer state, both retained phases and ceiling division. A backward timestamp,
foreign CPU, conversion overflow or unrepresentable absolute deadline refuses
without wrapping or inventing elapsed ticks. Masked or software-disabled
timers still count but expose no interrupt-producing deadline. APIC_BASE
Disabled preserves the count and conversion phase while advancing only the
sample epoch, so disabled time is not charged on re-enable. This freeze is
fixture policy, not a physical timer claim.

An armed interrupt cannot discard elapsed time. Ordinary service refuses
while delivery is armed, retaining its epoch. After a real entry/exit,
observation preflights the sample before consuming the actual flight and
charges the retained interval after consumption. This accounts at the observed
exit boundary; it does not recover the physical interrupt-dispatch instant
inside guest execution. Delayed periodic expirations may coalesce or become
pending again at that boundary.

## Exit and continuation contract

| Exit | Admitted behavior |
| --- | --- |
| HLT `78h` | Check a real stopped exit, CPL0/64-bit mode, enabled intercept, exact owned `F4` and canonical continuation; park with RIP still at HLT. |
| MSR `7Ch` | Service time, then reuse the checked APIC MSR handler for actual guest accesses. |
| NPF `400h` | Service time, then reuse the checked final-access xAPIC MMIO handler and fresh mapping token. |
| VMMCALL `81h` | Use the existing fixture dispatcher for the final requested stop. |

HLT preparation preserves GPRs and flags and consumes the preceding STI
shadow as part of retiring the intercepted instruction. It refuses unsupported
debug state, pending-event conflicts and invalid continuation evidence. The
ordinary unknown-exit policy remains a stop; it never advances RIP merely to
continue execution.

Each halted poll validates the retained halted state and services one actual
clock sample. Wake requires guest IF plus the existing APIC_BASE, SVR,
PPR/TPR and pending-event ownership gates. Only a deliverable owned interrupt
arms V_IRQ and advances RIP exactly once to the saved post-HLT address. The
next actual guest entry must consume that flight before EOI is handled.
Hardware dispatch observation alone does not prove handler completion; the
guest separately checks its stacked return RIP, executes EOI/IRETQ and counts
each post-HLT continuation.

The caller retains exclusive ownership of the complete stopped VMCB, register
frame and mappings throughout the parked interval. Checking retained RIP and
flags is not permission for arbitrary external state mutation. Each wait is
bounded to 200,000 samples and each session to 24 guest entries. Exhaustion or
an unexpected exit means incomplete execution, not successful timer delivery.
There is no allocation, firmware call, transport or logging inside the wait;
aggregate counters are printed after the suite.

## Guest execution and memory provenance

The original guest source remains a separate 3,802-byte blob. After every
earlier suite and loader-continuation user returns, the caller retires its
`Prepared` mapping token. The scheduler fixture installs its own immutable
source blob through the same `memory::prepare` path into the same owned
4,096-byte CODE page. It retains the ownership record and NPT backing
allowlist, obtains a fresh MMIO mapping token, and initializes each VMCB with
an ASID/TLB flush. No new guest executable mapping or host APIC mapping is
added. Installed instruction borrows end before guest entry or page reuse;
linker source pointers are not presented as installed-code evidence.

Each completed suite executes:

- 32 one-shot sessions, 16 starting on each APIC bus, with one HLT wake each.
- 32 periodic sessions, 16 starting on each bus, with two HLT wakes each and
  subsequent guest cancellation.
- 12 expected bounded no-wake sessions: masked timer, IF clear, priority
  blocked, SVR disabled, APIC_BASE disabled and cancellation on both buses.

Together these give **64 successful sessions, 12 bounded stops, 108 HLT exits
and 96 interrupt/EOI/IRETQ continuations**. Every successful guest checks its
handler and continuation counts, stack restoration and timestamp ordering.
The no-wake sessions retain halted RIP and the register frame, with no guest
re-entry after parking. IF/priority cases retain the expired IRR bit; masked
expirations do not create it, and APIC_BASE preserves its frozen countdown.

Negative setup assumes its SVR/BASE/cancellation write executes before the
initial expiration. An extreme external stall can invalidate that assumption
and leave already-pending IRR, which cancellation must not silently erase.
Such a fixture run fails visibly and must not be relabeled as passing.

## Frozen-source strict execution evidence

The final strict AVX, SSE and FXSAVE profiles passed with the same image
SHA256 `5b233227013e99bf82e0fbb56c7b3990f0339a8876beddab34c1041c2a450c9a`.
These results replace the initial-run metrics for current-source evidence.
The complete 35-profile regression matrix passed: three strict profiles, four
ownership profiles, seven synthetic profiles, ten extended-state profiles, ten
relocation profiles and one RDTSCP-disabled run.
Logs are retained in `work/apic-scheduling/validation/corrected-Avx.log`,
`corrected-Sse.log` and `corrected-Fx.log`. The shared driver is
`work/apic-scheduling/validation/run-regressions.ps1`; repeated validation
must use fresh output directories and serialize shared builds.

| Strict profile | Actual serialized source samples | Halted polling samples | Wake lateness, min / max raw TSC ticks |
| --- | --- | --- | --- |
| AVX | 551,357 | 550,713 | 36 / 960 |
| SSE | 473,986 | 473,342 | 36 / 40,296 |
| FXSAVE | 581,945 | 581,301 | 40 / 1,140 |

Each profile reports 568 guest entries, 104 HLT checkpoints before the
retained deadline and 106 HLT exits retaining STI shadow. Each completes
64 successful sessions, 12 bounded no-wake stops and 96 interrupt/EOI/IRETQ
continuations. The SSE maximum of 40,296 raw TSC ticks is retained as an
observed outlier, without conversion to a physical duration. Across all 25
completed interrupt traces, the measured envelope is 36�73,916 raw TSC ticks;
the largest maximum is the PCI AVX extended-state profile. This envelope is
not a statistical distribution or a physical latency guarantee.

The sample relation is `samples = poll_samples + entries + 76 admissions`.
The 106 shadows are the 108 HLT exits excluding two IF-clear sessions. Lateness
is measured from the retained programmed target to the actual poll that
authorizes wake. The target remains available if a one-shot has already
expired before HLT. These are raw emulator observations, not nanoseconds,
physical latency distributions or a native-versus-virtualized comparison.

The frozen source passes **222 core tests**, **251 returning-DXE tests** and
the hypervisor UEFI-target check. The shared evidence parser passes **88
negative controls**. Final parser revalidation passed all 25 completed interrupt
traces within the 35-profile matrix; other profiles test expected early failures.
The older backend remains explicitly incomplete for both CR8 fault paths and
is rejected by strict admission. The parser now requires both independent CR8
fault outcomes and the original CR8 write/source markers; missing, duplicated,
malformed or contradictory scheduling metrics cannot count as completion.

Evidence, exact tested images, source, runtime and a verified SHA256 manifest
are archived at
`C:\Users\mato\Documents\Codex\2026-09-10\svmvisor-bios-f7-analysis\outputs\apic-clock-hlt-2026-09-13`.
Per-profile measurements remain in `work/apic-scheduling/validation/final-metrics.json`;
full results are in `regressions.json`. The previous batch remains historical
evidence for its own tested images.

## References and remaining boundaries

Direct hashing confirmed the user's AMD APM2 revision 3.44 and PPR 57896
revision 3.00 match the worktree copies. Their SHA256 values are respectively
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`
and `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
Supplementary APM3 revision 3.37 remains at
`docs/24594_3.37_APM_Vol3.pdf`, SHA256
`c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`.

APM2 sections 6.5, 7.6.4, 13.2.4, 15.7, 15.9/Table 15-7, 15.21.4–5,
15.30.5 and 16.11 cover halt, serialization, TSC, intercept timing, virtual
interrupt readiness, STI shadow and timer behavior. APM3 HLT p388 and STI
p477 establish the post-HLT interrupt return address and shadow lifetime.
PPR 57896 applies specifically to Family 1Ah Model 44h Revision B0; its p43
describes a physical timer at 2xCLKIN, while p55 describes divided counting,
periodic reload and masked generation. Platform CLKIN, calibration and a
physical TSC/APIC relationship remain unestablished here.

The corrected QEMU source revision
`f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3` supplies an informative backend
cross-check: its HLT intercept occurs before guest EIP completion, and VMEXIT
saves the STI shadow. AMD manuals remain normative. Independent findings are
retained in `work/apic-scheduling/independent-review.md`.

No host APIC callback, asynchronous running-guest preemption, TSC-deadline
mode, SMP, NMI/IOAPIC/MSI or general OS decoder is established. Physical timer
frequency, latency, drift, native timing baselines and telemetry-loss behavior
remain unmeasured. Windows boot and Hyper-V/VBS/HVCI/PatchGuard/Secure Boot
compatibility, protected-memory visibility and device/DMA/network/storage
containment remain unsupported or untested as previously recorded. No hardware
activation, driver installation or protection change occurs in this batch.
Unknown exits, refusals and exhausted observation budgets mean incomplete
execution/analysis. This is an early hypervisor foundation, not a malware
analysis sandbox or a claim of undetectability.
