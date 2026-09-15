# Concurrent guest startup and HLT wakeup — 2026-09-13

The bounded flat emulator now starts its guest AP through guest-authored
INIT/SIPI while both host CPUs execute concurrently. The AP executes the
real16/protected32/long64 transition before joining the running-target and HLT
interrupt workload. This extends the completed [concurrent SMP fixture](concurrent-smp-fixture.md)
using its corrected runtime; it establishes no physical SMP or Windows support.

## Ownership and startup contract

Exactly two emulated host CPUs own two guest CPUs with fixed identities0/1.
Only each destination owner changes its VMCB, GPR frame, APIC, XSTATE, clock
state and private stack. Shared guest code and page tables remain immutable;
shared aligned progress words are explicit guest-owned release/acquire handoffs.
The original NPT allowlist, excluded host storage, stack guards, debug admission,
eager XSTATE, auxiliary state and host restoration checks remain active.

`IpiMailbox::new_cold_ap` adds a separate monotonic atomic control word alongside
the existing fixed-vector transport. INIT accepted/applied and first SIPI
accepted/applied are distinct states. The first SIPI vector cannot be overwritten.
The producer performs no remote mutable guest-state access. Local drain applies
INIT before SIPI through the existing `IpiTarget` startup implementation, with
fallible preflight before mutation. Later publications survive a concurrent drain.
Every accepted ICR causes an unconditional host kick, including duplicate SIPI
and coalesced fixed interrupts. This is a transport protocol, not a second IRR.

Only one cold INIT is admitted. INIT after initial acceptance, running reset,
migration, rebinding, broadcasts, logical destinations and arbitrary topology
are refused. Fixed delivery before owner-applied SIPI is also refused: this is
a narrower fixture policy than architectural retention of other interrupts
during INIT. Duplicate SIPI is explicitly accepted and ignored without changing
CS:IP or execution state, retaining the earlier bounded cooperative policy.

The BSP guest executes xAPIC INIT, SIPI and duplicate SIPI commands. The target
owner establishes the admitted INIT state and marks runnable after SIPI. Neither
acceptance nor runnable state is execution proof. First real16 CPUID validates
CS100:IP0, startup base1000h, control/GPR state and immutable0FA2 bytes before
advancing IP to2. The extracted `guest-startup.inc` is shared with the cooperative
fixture; its guest instructions execute LGDT, control-register changes, EFER
WRMSR and far jumps into long64. The executed long-mode checkpoint proves the
transition and switches the AP VMCB from an immutable startup MSRPM allowing
EFER to the ordinary immutable map. Classic SVM's internal EFER.SVME backing
requirement remains explicit. INIT does not reset the owned extended state.

## HLT and exit semantics

`ScheduledApic` remains the sole APIC/clock/HLT owner. Its new mailbox adapters
reuse the existing checked xAPIC/x2APIC instruction handlers. No mutable APIC
escape or second scheduler is added. Every real exit either settles armed
interrupt delivery or services the owned clock before dispatch.

| Exit | Authoritative state and completion |
| --- | --- |
| CPUID72h | First AP real16 witness checks exact startup state and installed bytes; existing dispatcher prepares checked next IP. Unexpected CPUID state refuses. |
| MSR7Ch | Existing stopped source frame/VMCB and exact WRMSR/RDMSR bytes drive APIC adapters. Checked source admission precedes transport publication and ICR/RIP commit. Reserved x2APIC encodings retain #GP-required classification; unsupported cases stop this fixture. |
| NPF400h | Only the admitted xAPIC mapping and exact immutable load/store provenance authorize MMIO completion. NPT violations are not synthesized guest page faults. |
| HLT78h | Exact installedF4, admitted long64 privilege/debug/event state and checked continuation. RIP remains at HLT while parked; preceding STI shadow is retired. Only owned deliverable interrupt readiness commits after-HLT RIP and armsV_IRQ. |
| INTR60h | Existing hostF1 source must be actually acknowledged after host restoration. No guest instruction is completed. Interrupted delivery remains refused. |
| VMMCALL81h | Existing checked dispatcher handles startup checkpoint, bounded query and STOP. Guest sentinels, stack, clock and actual handler/IRETQ progress are verified. |

Unknown exits, faults outside the admitted fixture, invalid entry, shutdown,
unsupported instructions and interrupted event delivery stop execution rather
than advancing RIP. Direct guest paging/control/EFER transition instructions
are limited to the immutable startup trampoline; this is no general OS policy.

Eight generations use xAPIC then x2APIC, preserving the earlier48 real
handler/EOI/IRETQ completions and adding16 HLT wakeups. Each CPU executes8 HLTs:
three publications after the last empty drain but before park, three after
committed park, and two after an actual empty parked poll. Each wait checks
stopped VMCB digest and exact saved frame preservation. The existing controlled
publication-after-drain/before-VMRUN race is retained. A ready HLT already owns
armedV_IRQ, so the next entry preserves it without another drain.

The target host CPU polls with IF clear under finite budgets while its guest
is HLT-parked. This is guest HLT wakeup, not physical host HLT sleep. Accepted
physical kicks remain pending for acknowledgement or a subsequent real entry.
Both CPUs independently prove running-target INTR and guest handler frames,
EOI and IRETQ continuation; transport/drain counts alone are insufficient.

## Qualification and provenance

Final image SHA256:
`0b168185f17ccc6419ce1a775e330d0bdde0bd0dbaa77d0bae9af7f38e6b4549`.
Corrected QEMU runtime SHA256:
`c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`.
Base revision: `f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3`.
The predecessor exact image and runtime hashes were reconfirmed and an unchanged
baseline run passed before the new image. No QEMU source changes were needed.
The earlier IGNNE/SMP-corrected backend's build provenance remains authoritative.

Final host validation passes265 hypervisor tests,251 native-returning DXE tests
and the core UEFI target check. Seven added tests cover ordered and combined
startup, duplicate first-vector retention, failed instruction/local preparation
purity,64 concurrent startup publication/drain iterations, and HLT mailbox
wakeup/altered-state refusal. Builds and emulator runs are serialized by root.

The final32 concurrent runs comprise8 each AVX, SSE, FXSAVE and RDTSCP-disabled
profiles, plus one exact single-CPU refusal. Each successful run completes one
AP startup,64 guest handlers with EOI/IRETQ,16 HLT wakeups and96 checked queries.
Totals are32 executed startups,2048 handlers and512 HLT wakeups. Strict evidence
requires42 metrics and7 markers, with exact entry accounting:
CPU0 entries=163+INTR; CPU1 entries=161+INTR. Startup command acceptance and
owner application remain separately counted from guest real/long witnesses.

All35 legacy profiles pass:19 full workloads and16 expected negative cases;25 traces retain valid interrupt-fixture evidence. Independent review checks all33 concurrent raw traces and35 legacy records. All1116 parser controls pass (279 each on AVX/SSE/FXSAVE/RDTSCP-disabled final traces). Results are recorded in
`work/concurrent-startup/final-validation` and the independent review. The batch
contains its contract,881-file before snapshot, batch-only patch/source hashes,
initial development image/logs, final image, exact runtime references and tests.
The new archive is `C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/concurrent-startup-2026-09-13`. It retains this batch and its runtime; the hash-pinned immutable predecessor archive retains the complete unchanged backend source/build inputs. Sparse regression disk images stay at their original evidence paths, with extracted ESP artifacts retained.

Fresh GPT-6 Astra High agents separately owned core implementation, harness
integration and independent architecture/source/evidence review.

## Measurements, references and limits

Across final32 runs, raw sampled entry/exit spans range43,384–6,679,266 emulator
TSC ticks. They include bridge checks and helper work, not isolated exit-service
cost, wakeup latency, calibrated nanoseconds or a native baseline. Every guest
query checks per-CPU monotonicity;8 guest-authored release/acquire handoffs per
run check cross-CPU sample ordering. This does not prove globally synchronized
physical clocks. Successful-loop output is deferred to bounded end-of-suite
counters; missing or contradictory evidence means incomplete execution.

AMD APM2 publication24593 rev3.44 (March2026), Table14-1,15.27.8 and chapter16
supply initialization/startup/APIC semantics. User-library PDF SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
The shared HLT owner cites APM3 rev3.37 HLT/STI and the APM2 event rules.
Duplicate SIPI remains a bounded policy cross-checked in the predecessor work
against Intel MPspec1.4 AppendixB.4.2; AMD's cited startup paragraph does not
explicitly settle duplicate handling. PPR57896 rev3.00 applies specifically to
Family1Ah Model44h B0; its physical timer rate does not calibrate this emulator.

Windows boot and Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility are
unimplemented or untested. Physical AP startup, physical host idle scheduling,
general topology/reset, native timing, device/DMA/storage/network containment
and malware sandbox readiness remain unestablished. No physical activation,
flashing, malware execution or Windows-protection changes were performed.

