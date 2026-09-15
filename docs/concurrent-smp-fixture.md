# Concurrent guest CPUs in the emulator — 2026-09-13

The new opt-in fixture runs two guest CPUs on two emulated host CPUs using
QEMU's multithreaded TCG backend. Each host CPU has its own VMRUN loop. The
guest workloads coordinate through shared RAM and exchange actual xAPIC and
x2APIC ICR commands. The test includes interruptions of running guest loops
and a controlled publication-after-drain entry race.

This is a bounded concurrent emulator milestone. It establishes neither
physical AMD SMP correctness nor a general guest scheduler, Windows boot,
protection compatibility or malware containment.

## Host ownership and startup

The profile is flat Multiboot only, with exactly two emulated host CPUs and
APIC identities 0/1. The BSP prepares all shared mappings before releasing the
AP. It copies a real-mode trampoline to owned emulator physical page 8000h,
changes its temporary host mapping from RW/NX to RX, and sends host INIT/SIPI.
The AP executes the real/protected/long-mode transition and enters its own
host callback. UEFI ownership is explicitly refused in this profile. A
single-CPU topology is refused before host mappings or ICR ownership change.

Each CPU owns its ordinary and double-fault stacks with guard pages, GDT/TSS,
IDT, SVM HSAVE page, extended-state scratch, auxiliary save/restore pages and
clock plan. Distinct GDT TSS descriptors avoid sharing LTR's busy-bit mutation.
CPU identity comes from stopped-host CPUID, never a mutable global selector.
The existing guest_run bridge remains the only world-switch implementation;
its already explicit pointer-based context now selects per-CPU scratch.
Every entry checks guest extended-state continuity, FSBASE, clock/AUX where
supported, host CR8 and immediate host auxiliary/extended-state restoration.

The host F1 interrupt gate is integer-only and increments only its CPU's
acknowledgement counter before hardware EOI/IRETQ. INTR interception leaves
the physical interrupt pending; the host accepts it through that actual gate
only after guest state has been saved and host state restored. The bridge
uses the existing CLGI/STI/VMRUN and VMEXIT/CLI/restoration/STGI ordering, with
V_INTR_MASKING and host IF enabled at guest entry independently of guest IF.
Both CPUs admit the LAPIC mapping as UC using their own PAT/MTRR evidence.

On completion, the AP restores its LAPIC and EFER/HSAVE settings and commits
to permanent terminal park. The BSP joins only after this commitment and
restores its LAPIC/PIC/gate and removes low-trampoline/LAPIC mappings. The AP
may still execute the final image-local park instructions; the commitment
guarantees no subsequent low-page or LAPIC access, not that HLT has already
retired. The installed private host environment is retained until emulator
termination. Per-entry restoration does not mean returning the AP to its
pre-bootstrap real-mode state.

## Interrupt routing and the entry race

Only a CPU's local loop mutates its VMCB, GPR frame and FixtureApic. The existing
checked MSR/MMIO adapters accept a MailboxTarget carrying no mutable remote
state. They validate source instruction, mapping, operands and continuation,
publish a fixed edge physical-unicast request, then complete source ICR/RIP.
The existing exclusively stopped INIT/SIPI adapter remains available unchanged.

Four atomic words transport admitted vectors 32..255. They are a bounded
software mailbox, not a second architectural IRR/ISR. The receiver preflights
its stopped event/control/APIC state before taking transport bits and queuing
them into its existing LocalApic. Both mailbox and IRR duplicates coalesce at
their respective stages. A rejected drain preserves transport and local state.
Blocked V_IRQ is settled using the existing observe/deferral implementation.

Every accepted ICR is followed by an unconditional physical host kick, including
a coalesced publication. No observation that a target is stopped or a mailbox
is empty permits skipping that kick. A publication racing the atomic drain
is either consumed or remains pending with its physical notification. Targets
remain admitted and enabled throughout the producer lifetime; dynamic offline,
rebind, reset, broadcast and logical delivery are outside this profile.

One generation deliberately holds CPU 1 after an empty mailbox drain. CPU 0
then executes an actual guest ICR write. CPU 1 observes that publication and
enters without draining again. The pending host kick must produce a VMEXIT;
the receiver then transfers the request locally and proves handler/EOI/IRETQ
completion. This tests a specific lost-wakeup window rather than relying only
on random interleaving.

## Guest execution and memory

Both guests start from the existing validated long-mode initializer; guest
INIT/SIPI was established in the preceding cooperative milestone. The new
real INIT/SIPI transition starts the second emulated host CPU. Four generations
use xAPIC and four use x2APIC, with private VMCBs, frames, extended state and
ordinary stacks. Shared code, IDT/GDT and paging backing are prepared before
AP release and never rewritten during parallel execution. Every entry uses
ASID 1 with a full local flush; neither guest migrates between host CPUs.

An exact pre-entry NPT audit checks the permitted backing pages and all leaves.
The shared data page contains aligned atomic qwords; host reads use Acquire
loads and guest publication uses the admitted x86 memory-ordering protocol.
No exclusive Rust reference covers concurrently guest-written RAM. The low
host trampoline is not guest backing: guest GPA 8000h maps the separate stack
page. Host stacks, tables, HSAVE, VMCBs and mailboxes are excluded from NPT.

Each generation executes IF-clear duplicate sends, a checked pending/coalesced
query, handler/EOI/IRETQ continuation, directed running-target sends and
simultaneous sends. Readiness and completion generations prevent one phase's
interrupt from being mistaken for another. Guest interrupt frames, GPR and
FSBASE identities, stack bounds/canaries and both CPUs' extended state are
checked. Running-target evidence requires an INTR exit with stopped RIP inside
the immutable wait-loop range and guest IF set, plus a real host F1 acknowledgement.

Per successful run, each CPU commits 32 ICR writes, completes 24 guest interrupt
handlers/EOIs/IRETQs and 32 queries. There are 115 non-INTR exits per CPU; the
number of host INTR exits varies with coalescing and interleaving. The parser
checks these relationships and requires a running-loop witness on both CPUs.
An accepted ICR, mailbox transfer, interrupt dispatch and completed handler
remain distinct evidence stages.

Unknown exits, malformed state, invalid entry, faults and shutdown terminate
the bounded loop; unknown instructions never advance RIP. NPF handling uses
the existing exact immutable instruction fetch and APIC alias provenance and
is never converted into an invented guest page fault. Successful per-exit paths
allocate no memory and emit no synchronous transport or log output. The BSP
prints bounded counters only after joining the AP.

## Emulator corrections found by concurrency

Repeated execution exposed a QEMU CPU-ownership defect. During FXRSTOR/XRSTOR,
`cpu_set_fpus` passed the executing CPU through its call chain, but
`cpu_clear_ignne` discarded that identity and modified `first_cpu`. CPU 1 could
therefore overwrite CPU 0's SVM flags with a stale read/modify/write. Captured
history showed NPT re-enabled at VMEXIT completion, followed by an NPF saving
the host return address and stack as guest state. CR8 could also return guest
V_TPR while the physical host LAPIC TPR remained correct.

The isolated correction passes the executing CPU into the IGNNE-clear helper.
With the identical image `d1dc2a3d9a9ff8a854bd2381190b2cad9af36247799e3c99493503173218a2ac`,
the predecessor failed two of eight runs; the IGNNE-only correction passed all
32 runs. The actual source function is extracted into a focused owner test:
9,216 ownership cases and 9,216 idempotence checks pass, while the old body
reproduces the wrong-CPU update. This test does not model a full virtual CPU.

Review also found VMRUN publishing VIRQ with an unlocked update to QEMU's
shared interrupt-request word. A small BQL critical section now matches the
existing peer writers' locking. It ends before event injection or guest
execution. The final backend includes these four source-file changes, with
no diagnostic logging or weakened guest-state checks. The separate legacy
chipset port-F0 IGNNE path remains outside the corrected/tested scope.

Full source provenance, replacement-object/link-input hashes, both intermediate
and final runtimes, failed traces and bounded ring diagnostics are retained in
`work/qemu-smp-ignne` and `work/concurrent-smp`. The existing compiler toolchain,
firmware assets and previously corrected backend sources remain unchanged.

## Validation and provenance

Final validation passes on image
`2e61b12de7c8d2aaa69f2f9c80b55ada08a20ff04fd2a2abf04848e373bb78e3`:

- 258 hypervisor tests, 251 DXE native-returning tests and the UEFI-target check.
- 32 concurrent executions: eight each with AVX, SSE, FXSAVE and RDTSCP disabled.
  Every run completes 48 guest handlers with EOI/IRETQ, with running-target
  witnesses on both CPUs and the controlled entry-race witness. Together these
  runs complete 1,536 guest interrupt handlers.
- One exact single-CPU refusal before SMP mapping/ICR takeover.
- All 35 prior emulator profiles, including ownership, faults, extended state
  and relocation; all 25 traces carrying valid interrupt-fixture evidence re-parse successfully.
- Independent review of the source, backend patch and final trace evidence,
  with all 672 parser controls passing (168 for each of four profiles).

The final build also checks that AP-only backing is absent from legacy images.
An intermediate link failure caught the extra allocation; feature gating fixed
it without enlarging the reserved image arena or relaxing the relocation checks.
Failed development builds and runs remain in `work/concurrent-smp/validation`.

Per-profile extrema below combine both CPUs and eight runs. Interrupt counts
are per CPU per run. Raw TSC spans include guest execution and switch/helper
work; they are neither physical IPI latency nor a latency distribution.

| Profile | Runs | Host INTR exits | INTR exits in guest wait loop | Smallest raw TSC span | Largest raw TSC span |
| --- | ---: | ---: | ---: | ---: | ---: |
| Avx | 8 | 12–26 | 7–15 | 48,224 | 5,077,952 |
| Sse | 8 | 14–26 | 7–15 | 46,288 | 3,872,000 |
| Fx | 8 | 13–26 | 7–16 | 42,856 | 3,791,436 |
| NoRdtscp | 8 | 12–28 | 7–16 | 48,312 | 3,539,712 |

Full results are in `work/concurrent-smp/final-validation/summary.json`, the
per-run manifests and `work/concurrent-smp/independent-review.md`. The retained
archive is `C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/concurrent-smp-2026-09-13`.

The final corrected QEMU runtime SHA256 is
`c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`, upstream base
`f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3`. The runner explicitly requests
`tcg,thread=multi` and `2,sockets=1,cores=2,threads=1`, with no icount, network,
disk or passthrough device. Source `docs/devel/multi-thread-tcg.rst` documents
the per-vCPU host-thread model; live cross-CPU guest progress and stopped-RIP
witnesses supply execution evidence separately from that configuration.

Primary AMD APM volume 2 publication 24593 revision 3.44 SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Relevant sections include 7.2/7.6.4,15.5,15.13.1,15.21.1/4,15.27.8,15.30.4,
16.5/16.6 and Appendix B. The AP Startup Sequence citation is 15.27.8; earlier
checkpoint comments incorrectly cited 15.27.4. Historical evidence is preserved.

## Timing and remaining limits

Each CPU checks its guest timestamps for monotonicity. A guest-authored
release/acquire handshake orders a sender sample before the receiver sample,
using the admitted MFENCE/RDTSC sequence. Serialized host samples surround
each entry and record raw extrema. These measurements include switching and
helper work. They establish neither global clock synchronization nor native
IPI latency, a latency distribution, physical frequency or OS timing fidelity.

All waits and exit loops have explicit bounds; exceeding one means incomplete
execution, never a clean analysis result. The fixture has fixed two-CPU
topology and guest code, no migration or general scheduler, no generic debug
register switching, no AP reinitialization, and no concurrent UEFI/native path.
Windows, Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility, native timing
baselines, device/DMA/storage/network containment and malware execution remain
outside the measured scope. No hardware programming or Windows protection
changes were made.
