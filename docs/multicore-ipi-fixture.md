# Two guest CPUs: startup and IPIs — 2026-09-13

The emulator fixture now starts a second guest CPU through guest-authored
INIT/SIPI commands and executes bidirectional fixed IPIs through xAPIC and
x2APIC. The two guest CPUs run cooperatively on **one host CPU**. This proves
the bounded guest startup and interrupt path; physical concurrent SMP and
general OS topology admission remain separate work.

## Architecture and execution policy

Each guest CPU owns its VMCB, GPR frame, eager extended-state buffers, APIC
controller, startup state and capability-gated clock AUX identity. Existing NPT backing, immutable
code and page tables are shared deliberately; ordinary stacks occupy disjoint
pages. Every entry requests a full ASID1 flush. There is no second interrupt
queue, APIC register owner or world-switch implementation.

The existing xAPIC MMIO and x2APIC MSR adapters route an ICR command through a
borrowed target context. They validate the stopped source instruction and its
continuation before target mutation, then complete sender ICR/RIP. Fixed edge
physical-unicast IPIs use the target's existing IRR, including coalescing.
Unsupported destinations, broadcasts, logical delivery and other unsupported
modes refuse without changing either CPU. Reserved x2APIC bits retain their
architectural #GP-required classification, distinct from policy refusal.

Cold AP startup progresses from Cold through AwaitSipi to Running (runnable).
INIT preserves admitted CR0.CD/NW and extended state while establishing the
bounded architectural INIT register state. The signature comes from the existing
synthetic guest CPUID policy. SIPI sets the startup CS/base and IP. Duplicate
SIPIs are ignored under the explicit fixture policy and leave the target CPU
state intact. Reinitializing a running AP is refused. Requiring INIT before
SIPI is a fixture admission restriction, not a claim that reset APs cannot
accept SIPI directly.

Classic SVM requires EFER.SVME in the guest backing state. The first AP entry
therefore has that internal bit set, with real-mode execution otherwise checked
at CS100:IP0. The first instruction is an actual CPUID intercept; its exact
bytes and stopped register state are checked before the existing dispatcher
advances the CS-relative IP to2. The AP then executes LGDT, CR0/CR4/CR3 changes,
far jumps and WRMSR EFER to reach protected32 and long64. A temporary owned MSR
permission map permits EFER during this transition and revokes it at the
long-mode checkpoint. The host does not manufacture that transition.

The shared emulator bridge now optionally uses VMSAVE/VMLOAD with the same
guest VMCB and dedicated host auxiliary pages. It captures guest GPRs before
saving guest auxiliary state, restores host state before host helpers, and
compares the restored FS/GS/TR/LDTR and syscall/sysenter fields. Legacy callers
opt out. Both guest CPUs execute distinct FSBASE writes and complete readbacks
across interleaving. Existing extended-state sentinels and clock restoration
checks run on every entry. DR0–3 are admitted only after a read-only zero check;
guest debug-register access is intercepted and refused. General per-CPU debug
register virtualization is outside this fixture.

Scheduling switches at actual guest query boundaries. Unknown exits, faults,
invalid entry and shutdown stop the bounded loop; unsupported instructions do
not advance RIP. APIC NPF handling uses existing immutable fetch and mapping
provenance, with no conversion into a guest page fault.

## Actual guest workload

Four repetitions on each APIC bus give eight positive sessions. BSP sends
INIT/SIPI and duplicate SIPI, then the AP executes its startup trampoline and
software-enables its local APIC. BSP queues the same fixed vector twice while
AP IF is clear. AP verifies the pending request, executes STI/NOP, enters one
real handler, checks its interrupt frame, executes EOI and IRETQ, and proves
continuation. AP then sends the same pair of reverse IPIs; BSP repeats these
checks. Later enabled entries establish no duplicate delivery. A further SIPI
after AP execution must leave its VMCB and GPR state unchanged.

Each complete trace adds **296 entries, eight real-mode starts, eight long-mode
checkpoints, 80 guest queries, 16 IRQ dispatches and 16 EOIs**. Handler and
IRETQ witnesses establish completion separately from dispatch counts. Distinct
GPR sentinels, FSBASE, extended state, clock identities and stack canaries are
checked across interleaving.

Sixteen additional real ICR refusal outcomes are included in those entries:
absent destination and INIT targeting a running AP, once per session. Exact
error types are required. Both VMCBs, frames, startup states and exposed APIC
state remain unchanged at the refused low-half commit instruction. APIC
snapshots cover ICR, priorities, SVR, timer state and all IRR/ISR bits. The xAPIC
high-half write is a separate successfully completed instruction. Guest VMCB
comparisons use bounded digests; host unit tests use exact bytes.

## Validation and retained evidence

Final host checks pass: **253 hypervisor tests, 251 returning-DXE tests and the
hypervisor UEFI target check**. Eight added tests cover startup state, identity,
ICR routing, reserved-field classification, coalescing, refusal purity and mode
transition readback. The final core source is the source checked in the first
matrix attempt; subsequent changes were harness diagnostics and the isolated
emulator correction.

All **35 emulator profiles** pass on the corrected runtime: three strict CPU
profiles, four ownership, seven synthetic, ten extended-state, ten relocation
and one RDTSCP-disabled profile. All **25 completed interrupt traces** pass
final parser revalidation. They contain 200 startup sessions, 400 IRQ/EOI
completions, 400 routing refusals and 7400 new guest entries. Expected early
failure profiles retain their original meaning; they are not counted as
completed startup execution.

**74 evidence mutations** pass independently on each of a flat and a UEFI
trace, rejecting missing, duplicate, malformed, trailing and contradictory
new markers and metrics. The independent reviewer also ran the 74 controls
against an earlier completed trace. Final logs and per-trace metrics are in
`work/multicore-ipi/final-validation`. The strict image SHA256 is
`e24d56da82247b3eef3686f2610389aaeda3b48dafd8bc27b97ed308d0408b3a`.
Observed entry/exit spans across the final traces range from 45,144 to 645,920
raw emulator TSC ticks; the timing limitations below apply.

Fresh GPT-6 Astra High agents owned core implementation, harness implementation
and independent review; root owned integration and all serialized builds and
execution. Contracts, the pre-batch dirty-source snapshot, batch-only patch,
source hashes, review and failed development evidence remain in
`work/multicore-ipi`. The previous source and runtime were hash-verified against
historical manifests before the new backend build. No blocking review findings
remain for this bounded scope.

The report, sources, runtimes, build provenance and execution evidence are
retained in the verified archive
`C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/multicore-ipi-2026-09-13`.
Sparse UEFI disk images remain at their original evidence paths; the archive
records those paths and includes their extracted ESP artifacts and logs.

## Backend defect exposed by relocation

The first strict AVX/SSE/FX runs passed, but the UEFI ownership profile loaded
at64MiB failed the exact host auxiliary-state comparison. Bounded diagnostics
identified the saved TR base at VMCB offset498h: expected040D7000h,
observed000D7000h after VMLOAD/VMSAVE. The failed trace remains retained.

The pinned Windows QEMU implementation narrowed segment bases to host `long`
before canonicalizing them. Windows `long` is32 bits; the max CPU model exposes
57-bit linear addresses, giving a seven-bit shift and loss above bit24. This
explains the observed base exactly. AMD requires sign extension of the full
address to bit63. The isolated correction uses QEMU's existing `sextract64`
helper, avoiding both host-width truncation and signed-left-shift overflow.
No guest address or assertion was changed to bypass the failure. The earlier
source and runtime remain intact; the corrected build is isolated in
`work/qemu-svm-canonicalization`.

The exact same diagnostic payload and relocation package pass at64MiB on the
new runtime. Sixteen compiled cases use the actual extracted QEMU helper bodies
for48/57-bit addresses, including high canonical and noncanonical inputs; all
pass. A defined model of the old32-bit-long behavior differs in13 cases and
reproduces the failing TR base. This complements the actual old/new emulator
comparison rather than replacing it.

The new QEMU executable SHA256 is
`78c8a365039293dd5eae97ed0e730cc16aa4592380551ad01e365ad9f3b00013`.
Its complete correction patch SHA256 is
`3d02895fd953bc65062c3eb6863eda45404dd277a40b6a6226441243bd7bd072`.
The isolated build completed1530 steps with the existing private toolchain;
stopped PC/q35 smoke checks pass. The familiar optional Windows symlink-bundle
failure was narrowly verified before compiling the unchanged generated rules
and packaging explicit DLL/firmware dependencies. Manifests retain exact source,
compiler, dependency, firmware and executable hashes. One source file changes
functionally; ten other files differ only in Git-applied newline normalization.

## References, timing and limitations

Primary reference: AMD APM volume2 publication24593 revision3.44, chapters14–16
and AppendixB. The user's primary-library PDF SHA256 is
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Rendered Table14-1 pages were checked because the Markdown extraction corrupts
merged RESET/INIT cells. Duplicate-SIPI behavior is an explicitly bounded policy
cross-checked against Intel MP specification1.4 AppendixB.4.2; AMD15.27.4 supplies
the startup-address semantics without explicitly specifying duplicate handling.
Details are retained in `work/multicore-ipi/reference-check.md` and the review.

Each guest query samples RDTSC, checked for monotonicity in actual cooperative
execution order. Serialized host samples surround each entry and preserve raw
minimum/maximum spans. These include switching and helper work; they are not
isolated VMEXIT cost, physical IPI latency, a latency distribution or parallel
cross-core clock ordering. Observation uses bounded counters and end-of-suite
output. Missing or contradictory evidence means incomplete execution.

The previous software-emulated QEMU runtime is retained with SHA256
`581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`, source revision
`f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3`. The final matrix uses the isolated
canonicalization correction described above on the same upstream revision.
Historical backend gaps remain explicit. This batch establishes no Windows
boot, Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility, physical AP startup,
native timing baseline, general scheduler, device/DMA containment or malware
sandbox readiness. Physical firmware, Windows protections and prior evidence
were not changed.
