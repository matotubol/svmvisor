# Interrupted exception delivery and nested handler faults — 2026-09-13

The emulator now executes #UD/#GP/#PF inside a running IRQ handler while a
second timer request remains pending. It also resolves a bounded set of faults
during IDT exception delivery, applies AMD's double-fault combination rules for
that set, and handles derived and actual intercepted shutdown as terminal.

## Architecture and scope

`Vmcb::resolve_exception_delivery_after_exit` is an opt-in operation for a real
stopped exit. The existing ordinary `reflect_exception` remains strict. The
resolver accepts only interrupted type-3 #UD/#GP/#PF/#DF and secondary
#NP/#SS/#GP/#PF. Its complete 16-case matrix is:

| Interrupted exception | Secondary #NP/#SS/#GP | Secondary #PF |
| --- | --- | --- |
| #UD | Inject secondary exception | Inject #PF |
| #GP | Inject #DF(0) | Inject #PF |
| #PF | Inject #DF(0) | Inject #DF(0) |
| #DF | Terminal guest shutdown | Terminal guest shutdown |

Success replaces EVENTINJ without advancing RIP or changing RSP, flags, GPRs,
or APIC state. An intercepted #PF has not written CR2; reflection writes its
EXITINFO2 address even when it contributes to #DF. The prior request must match
EXITINTINFO if EVENTINJ remains valid. Undefined error payload with EV clear is
ignored; malformed, unsupported and competing events refuse without mutation.
The replacement request is retired only after its next actual entry/exit.

Both shutdown outcomes preserve the stopped VMCB. An actual shutdown exit is
recognized before interpreting undefined guest state or fetching instructions.
The existing APIC conflict guard rejects shutdown before consuming a flight or
charging elapsed timer time. There is no new APIC, event queue, transition
bridge, or parallel persistent shutdown state. The owning loop stops execution.

An IRQ dispatch, executing guest handler, guest EOI and IRETQ are separate
milestones. Ordinary faults inside an already entered IRQ handler have invalid
EXITINTINFO and use the existing settlement/reflection path. Interrupted
external/type-0 IDT delivery remains a state-preserving refusal; this batch
does not claim its recovery.

## Actual guest execution

Each complete trace adds 24 IRQ-handler sessions: two APIC buses, three faults,
four repetitions. The guest programs a timer, enters its first IRQ handler,
then programs a second one-shot timer. Bounded stopped-host polling establishes
the second IRR bit while the first ISR bit remains active. Guest frame checks
cover exact RIP/CS/RSP/IF, fault error and #PF CR2. Nested fault IRETQ, first
EOI, outer IRETQ, second IRQ/EOI/IRETQ and an extra enabled entry establish no
loss or duplicate delivery. Final RSP and stack/IST canaries are checked.

These sessions use **312 entries, 24 faults, 48 IRQ dispatches and 48 EOIs**.
Dispatch counts alone are not handler-completion evidence; the guest instruction
witnesses and frames supply that evidence separately.

Seven further cases repeat four times:

| Actual delivery case | Checked outcome |
| --- | --- |
| #UD gate not present → #NP | #NP frame/error `0x32`, guest-authored continuation, IRETQ |
| #GP gate not present → #NP | #DF error zero and terminal handler checkpoint |
| #PF gate not present → #NP | #DF error zero and terminal handler checkpoint |
| #GP → #NP → #DF whose gate is not present → #NP | Derived terminal shutdown; unchanged stopped evidence |
| Guest-raised #GP with #GP/#NP/#DF intercepts disabled, absent gates | Actual `VMEXIT_SHUTDOWN` (`0x7f`), terminal classification |
| #GP delivery reads unmapped IDT gate `0x2ff0` → #PF | #PF error/CR2, guest-authored continuation, IRETQ |
| #PF delivery reads unmapped IDT gate `0xb000` → #PF | #DF with CR2 `0xb000`, terminal handler checkpoint |

These use **84 entries, 28 secondary faults, four #NP IRETQs, four #PF IRETQs,
12 terminal #DF handlers, four derived shutdowns and four actual shutdown
intercepts**. A #DF is an abort with undefined saved RIP; no test restarts or
executes IRETQ from it. Actual shutdown is distinct from software combination.

Boundary IDT layouts reuse owned backing and existing absent guest pages, with
no NPT-to-guest-#PF conversion. The last case places one checked 16-byte gate in
otherwise unused IST backing, while all frames use the ordinary stack. The
exact gate and every other IST byte are audited. IRQ nested frames permit only
the top 128 stack bytes; delivery cases permit 64. Existing bridge, mapping
ownership, ASID flush and immutable instruction-fetch paths are reused.

## Validation and review

The authoritative implementation is `C:/Users/mato/.codex/worktrees/7a58/svmvisor`.
Fresh GPT-6 Astra High agents owned core, harness and independent review; root
owned integration, serialized validation, report and retained artifacts. The
pre-batch dirty source snapshot, batch-only patch, contracts and review are in
`work/interrupted-delivery`. Historical evidence was preserved.

Final host checks pass: **245 hypervisor tests**, **251 returning-DXE tests**,
and the hypervisor UEFI target check. The ten new host tests include all 16
combinations, CR2 effects, injection lifecycle, terminal purity, malformed and
unsupported cases, and shutdown with pending/armed IRQ ownership.

All **35 emulator profiles** pass: three strict CPU profiles, four ownership,
seven synthetic, ten extended-state, ten relocation and one RDTSCP-disabled.
All **25 completed interrupt traces** pass final parser revalidation, each
with the complete new 396-entry workload. Across those traces this is 600
IRQ-handler sessions, 600 interrupted-IDT sessions, 100 actual shutdowns and
9900 new entries. Expected early failures retain their original meaning;
completion is not inferred for the other profiles.

**102 independent evidence mutations** pass on both a flat and a UEFI trace.
The shared parser rejects missing, duplicate, malformed and contradictory
new markers/metrics. The reviewer ran eight additional independent mutations.
Final logs, results and per-trace metrics are in
`work/interrupted-delivery/final-validation`; they supersede development runs.
The strict image SHA256 is
`20ac6313751eed1c6e10b28c0406805dfe28a69b9499551192c488bec2c084a0`.
The pinned QEMU executable remains
`581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`, source
revision `f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3`. No backend edit was needed.

The initial failed guest #NP check and diagnostic run remain in `validation`.
They exposed a fixture error (`0x62` expected instead of architectural `0x32`),
corrected without changing captured exit evidence. Independent review also
strengthened a shutdown test so preexisting conflict guards could not mask a
missing shutdown guard. No blocking source findings remain.

## References, timing and limits

Primary reference: AMD APM volume 2 publication 24593 rev 3.44, sections 8.2.9,
8.4.1, 8.9, 15.7.2–3, 15.12.15, 15.14.3, 15.20, 15.21.4 and appendices B/C.
The user-library and worktree PDFs both hash to
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
See `work/interrupted-delivery/reference-check.md` for the independent source
and backend checks. This is classic SVM architecture, not processor PPR proof.

Each IRQ session has a 24-entry and 200000-poll bound; each delivery session
has a six-entry bound, with existing process timeouts outside both. Timer
overlap is deliberately established while stopped. IRQFAULT TSC extrema
span **38500..334708 raw emulator TSC counts** across final traces and include
the entry/exit bridge; they are not physical interrupt latency, an
isolated VMEXIT cost, a distribution or a native-versus-virtual comparison.
IDT cases add entry counts without a separate latency measurement.

Prior #DE/#TS/#NP/#SS, other secondary faults, arbitrary delivery chains,
external/NMI/software-interrupt replay, repair/dismiss of delivery faults,
NPF repair, IST switching, trap-gate preemption and MOV-SS shadow remain outside
the resolver. Missing or contradictory evidence means incomplete execution.
Multicore startup and IPIs are the next major milestone and were not added.

Physical evidence remains the earlier returning 65-entry probe. Windows is not
running under a resident hypervisor. Windows/Hyper-V/VBS/HVCI/PatchGuard/Secure
Boot compatibility, native timing and storage/device/DMA/network containment
remain unestablished. No physical programming, activation, protection changes
or malware execution occurred.

Sources, compiled harness artifacts, pinned runtime, final and development
records, contracts, review and batch-only diff are archived under
`C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/interrupted-delivery-2026-09-13`
with a verified SHA256 manifest. Large FAT disk images remain in their original
workspace evidence paths; extracted ESP contents, drivers and ROMs are archived.
