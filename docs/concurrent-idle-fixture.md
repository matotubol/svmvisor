# Concurrent timer scheduling and actual host idle — 2026-09-13

The fixed two-CPU emulator now preempts guest integer loops using owned host
LAPIC timers and executes actual host HLT while guest CPUs are idle. Timer and
peer-IPI interrupt gates independently prove their wake source. The existing
INIT/SIPI startup and interrupt fixtures remain intact. Physical/native SMP,
Windows boot and full sandbox containment remain unestablished.

## Architecture and state ownership

The existing host LAPIC `HostTimer` implementation moved from `preemption.rs`
to `host_lapic.rs`, with real callers in the legacy timer fixture and concurrent
host owner. It consolidates admission, MMIO, LVT/divider/priority programming,
interrupt gates and restoration; it is not a second timer driver. Legacy
post-EBS timer rebasing retains its explicit terminal handoff semantics.
Concurrent admission remains flat Multiboot, exactly two CPUs, idle initial
host timers and fixed F0(timer)/F1(IPI) sources.

Only the destination CPU changes its guest VMCB, frame, ScheduledApic, XSTATE,
clock and private stack. Shared immutable code/maps and guest-authored progress
words retain the predecessor ownership rules. BSP owns shared LAPIC mapping
and PIC-mask lifetime; AP validates the established UC mapping without editing
shared page tables. Each CPU saves/restores its own F0/F1 IDT gates, LVTs,
divider, TPR/SVR and admitted zero-count baseline. Per-entry host auxiliary,
XSTATE, CR8, stack and NPT exclusions remain checked. No guest memory is written
to manufacture loop progress, timer completion or an interrupt-frame witness.

The guest `ScheduledApic` is unchanged and remains the sole clock/APIC/HLT
owner. Host timer expiry forces an exit or wakes the host; it cannot independently
authorize a guest interrupt. The caller services the guest-programmed deadline,
settles pending delivery and uses the existing ready/arm/observe/EOI ownership.

## Interrupt attribution and host HLT

Before each guest entry the CPU arms a one-shot host F0 timer with count100,000
and divide1. F1 may already be pending; a racy empty observation never suppresses
an accepted peer kick. Each stopped boundary cancels the one-shot and drains
actual F0/F1 through private integer-only gates. EXITCODE60 itself identifies
neither source and does not acknowledge an interrupt. F1-only exits do not
require F0 expiration; a single exit may have both source acknowledgements.

When F0 was pending before cancellation, current-count0 is checked first.
A source arriving between the initial observation and cancellation is classified
separately, including whether current count was still active. Its owned F0 ISR
gate proves source identity; it is not relabeled as previously observed timer
expiration. Both cancellation-race counters were0 in the final matrix. Their
rare branches were source-reviewed but not dynamically reached by these runs.

The resumable host idle function arms an owned watchdog then executes adjacent
`STI; HLT; CLI`, with IF0/GIF1 and restored host state at entry. No acknowledgement
occurs in the last empty-check/HLT gap. STI shadow covers HLT even if a source
is already pending. Each gate requires its exact F0/F1 ISR bit, checks the
hardware-stacked interrupted RIP against the post-HLT label, counts the source,
issues EOI and verifies cleared ISR before IRETQ. Return requires an actual
post-HLT witness. A pending interrupt may resume HLT immediately; no minimum
sleep residency or power-saving result is claimed.

After-park cases publish peer readiness then enter host idle while the mailbox
is still potentially empty. Each wake re-drains local transport and services
or polls the guest owner. Before-park cases deliberately publish before HLT.
Two cases per CPU first require a watchdog-only host wake with guest state
still parked and no pending guest vector, then permit the peer request. Every
CPU must record an actual F1 post-HLT witness. These complement the retained
publication-after-drain/before-VMRUN race and unconditional accepted-send kicks.

Finite per-entry and per-idle iteration budgets remain. The external QEMU
process timeout handles an emulator that never returns from VMRUN/HLT; this is
not a resident NMI watchdog or a physical scheduling guarantee.

## Executed workload and exit policy

Each of8 generations uses xAPIC or x2APIC and retains the original IPI workload.
Each CPU additionally programs two local one-shot guest timers, initial16,384
and divide1, at the admitted synthetic rate of one timer tick per1024 TSC ticks.
One timer interrupts a nonvoluntary integer loop; the other wakes guest HLT
through actual host idle. The loop counter must show positive progress at an
actual F0-attributed INTR in every session. No-progress exits are counted
separately. Guest handlers check hardware-stacked RIP against the immutable
loop range or exact HLT+1, then perform EOI/IRETQ and guest continuation checks.

| Boundary | Required behavior |
| --- | --- |
| INTR60h | Restore host state, attribute/acknowledge actual F0/F1, settle/service local guest state, preserve stopped RIP/RFLAGS/frame. No instruction completion. |
| HLT78h | Exact immutableF4, admitted mode/debug/event state and existing checked HLT continuation. Guest RIP stays parked until owned guest interrupt readiness. |
| MSR7Ch / xAPIC NPF400h | Existing exact instruction/fetch/mapping/continuation owners; service elapsed time before register changes. Checked publication precedes accepted ICR completion and unconditional kick. NPT faults are not fabricated guest page faults. |
| CPUID72h / VMMCALL81h | Preserve real16 and executed long64 startup witnesses, bounded query/STOP and immutable instruction provenance. |
| Unknown/invalid/interrupted delivery | Refuse without advancing an unknown instruction; no generic OS retry or interrupted-delivery replay. |

Running INIT/reset, migration, broadcasts/general topology and unsupported
interrupt sources remain refused. Direct guest transition instructions remain
limited to the immutable startup fixture. If an extreme emulator stall exhausts
a guest deadline before the admitted loop/HLT point, the strict fixture refuses;
its configuration timing is not a latency guarantee or arbitrary-guest policy.

## Final validation

Final image SHA256:
`cf30121082a2b485ac0fc3c1760bb729d12b62d69310465e2f0afb49a248c1fe`.
Linked ELF SHA256:
`6a715397735123d80ce44d6a2df5e189d00f3d78b484bebb2bea9336ae90ba24`.
Unchanged corrected QEMU SHA256:
`c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`,
base revision `f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3`.
The previous exact final image passed again before implementation. No backend
correction or physical activation was needed.

All32 runs pass (8 AVX,8 SSE,8 FXSAVE,8 RDTSCP-disabled), plus the exact one-CPU
refusal. Each positive trace contains94 unique metrics and10 exact markers.
Voluntary exit accounting is CPU0=251 and CPU1=249; total entries add actual
INTR exits. Source inclusion/exclusion and acknowledgement sums are checked.

The final matrix completes32 guest AP startups,2048 IPI handlers plus1024 timer
handlers,1024 guest HLT wakeups,512 timer-preempted loop sessions,2384 actual
host HLT returns and128 deliberately watchdog-only wakes. All512 IPI host-HLT
wake witnesses and1872 timer host-HLT witnesses come from hardware-stacked
post-HLT RIP. These are finite interleaving tests, not an exhaustive proof.

All265 core tests,251 native-returning DXE tests and the UEFI core check pass.
All35 legacy profiles pass, preserving the existing19 complete workloads and16
expected negative cases;25 traces retain valid interrupt evidence. No core
behavior changed, so new validation concentrates on real concurrent execution,
legacy extraction regressions and the linked host interrupt/idle boundary.

The independent linked audit passes13 boundary checks and rejects20 decoded
instruction mutations, including altered STI/HLT/CLI, missing LOCK, wrong source
ISR/stacked RIP and forbidden loop exits. Four independent parser suites reject
379 evidence mutations each (1516 total). Initial audit parser issues concerned
LLVM's standalone LOCK prefix and operand comments; normalization retained the
exact atomic/operand checks. Guest assertions were not weakened to make tests
pass. Details and final raw-trace checking are in the independent review.

Two fresh GPT-6 Astra High agents authored this batch. The agent service refused
a third fresh agent at its thread limit, so the retained High agent reviewed
this new batch without editing its implementation. That reviewer had authored
part of the completed predecessor; this provenance is explicit. Root serialized
all builds/tests/QEMU. The883-file pre-edit snapshot, batch-only patch, source
hashes, contracts, development/final images, logs and review live under
`work/concurrent-idle`. Text patches normalize line endings; hashes are exact.

The new archive is
`C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/concurrent-idle-2026-09-13`.
It retains current sources, this batch and the runnable backend, with hash-pinned
predecessor archives retaining the unchanged complete backend build provenance.
Sparse disk images stay at original regression evidence paths; extracted ESPs
and logs are archived. Earlier dirty work and archives remain preserved.

## Timing, references and next boundary

Final raw TSC envelopes are59,576–9,644,114 for entry/exit calls,
66,660–9,782,274 for host idle calls, and1,568–9,772,076 for guest deadline
lateness. Entry spans include timer arming and run_owned bridge/checks, ending before the following timer cancellation and acknowledgement. Host-idle spans include arming, HLT, cancellation and acknowledgement.
They are not isolated VMEXIT cost, minimum sleep duration, nanoseconds, physical
latency distributions or a native baseline. Guest samples retain per-CPU
monotonicity and explicit release/acquire cross-CPU ordering. Bounded telemetry
is emitted after completion; absent or contradictory evidence is incomplete.

AMD APM2 publication24593 rev3.44 and APM3 rev3.37 supply SVM IF/GIF, STI/HLT,
interrupt and APIC semantics. Their SHA256 values are respectively
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`
and `c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`.
See host-owner-contract.md for exact citations and reference applicability.
PPR57896 rev3.00 specifically covers Family1Ah Model44h B0; it does not calibrate
this emulator or establish timer behavior on arbitrary physical machines.

The next integration boundary is a concrete OS-boot machine profile and exit
coverage contract: CPU exposure/topology/ACPI, APIC/timer requirements, guest
memory/device ownership and first supported boot path. Windows boot and
Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility remain unimplemented or
untested. Physical SMP/timing, multi-vCPU run queues, device/DMA/storage/network
containment and malware-analysis readiness remain unestablished. No flashing,
malware execution or Windows-protection changes were performed.


