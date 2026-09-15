# Native physical cache stability during Windows MTRR replay

The target native boot path now owns guest cache-control MSRs before the first
guest entry. It keeps the physical MTRR and routing bank stable, preserving the
resident monitor's admitted WB mappings. This supersedes physical MTRR/SYS_CFG
pass-through for the captured Family 1Ah Model 44h boot profile. The older
SYSCFG-only handler remains the fallback for profiles without this capture.

## Evidence and admission

The exact previous image `fe1217e0477e46d28bc99e3b5854f766` recorded physical
MTRR_DEF_TYPE.E=0 while both guest and host CR0.CD were clear. The first software
post-VMRUN RIP/NRIP reads already equaled the entry RIP. That proves the physical
WB admission was no longer stable; it does not prove a particular hardware
cache-line or VMCB corruption mechanism. No continuation rule is relaxed.

Original rendered manual review, hashes and reference chains are retained in
`work/native-raw-result-2026-09-15/cache-register-contract.md`. The independent
Windows byte review is in `work/native-raw-result-2026-09-15/windows-review/report.md`.
It establishes participation of all active logical CPUs, no sibling dependency
between the common barrier and E0, and no such dependency in the replay before
E1. The active bank comes from BSP capture. Inactive variable-pair payloads may
be zeroed, so byte equality of disabled pairs is deliberately not required.

Before any guest, the returning firmware MP observer captures every complete
bank, including hidden fixed RdMem/WrMem fields by temporarily exposing and
restoring thread-private SYS_CFG19. Actual CPUID package/core/thread topology
defines cohorts; slot or APIC parity does not. Every active bank must agree
with the BSP, and same-core physical banks must agree. Each CPU revalidates its
complete physical observation at arm. Directory version 8 reserves shared RW/NX
owner pages at offsets f5000..f8000 and immutable RO/NX captures at f8000..fb000.
DXE validates their actual translations and physical WB typing.

## Runtime ownership

One inventory in `svm/native_cache.rs` installs both-direction MSRPM interception
and selects dispatch for MTRRcap, DEF, variable/fixed registers, SYS_CFG, HWCR,
IORRs, TOM/TOM2 and MMCONFIG. PAT retains the existing hardware G_PAT owner.
Existing APIC and protected SMM/control policies continue to own their fields.
Reads return the shared logical core bank, with SYS_CFG19 and fixed-field
visibility belonging to the current thread. Reserved fixed/type inputs fault;
with logical SYS_CFG19 clear, attribute-one inputs fault and zero inputs preserve
hidden attributes. Unsupported routing/MMCONFIG changes stop before commit.
HWCR has a CPU-local physical owner described below; only its counter-enable
bit may change.

Each E0 writer must already have guest CD=1/NW=0. It rebuilds a private NPT copy
from its current root, denies all guest access below 1MiB, selects that root and
arms the pre-execution CR0-write intercept. Only after every cohort member is
stopped at its own checked E0 does the shared logical E0 become visible. Physical
types stay unchanged; guest CD maintains the required coherent cache behavior.
The NPT change removes permissions only. Existing monitor/APIC holes and current
ECAM permissions survive, including the case where the first PDE already points
to a partial-exclusion PT. Software guest readers also reject low physical
instruction and page-table reads under this root.

Logical SYS_CFG18 may change while low accesses are denied: the architectural
low-memory routing difference is never silently treated as ordinary RAM. Every
logical MSR write requests a full guest TLB flush. Long64 uses the existing
same-CPU NRIPS boundary owner, including during CD=1. A separate owner-scoped
fallback permits long64 CD=1/NW=0 with independently qualified physical WB
backing, current NPT permission enforcement, and every existing guest PAT,
page-table, executable and privilege check. Generic fetch continues to refuse
CD=1. Physical DEF/SYS_CFG are rechecked before this fallback. Each CPU keeps
and commits its own validated continuation once.

At paired E1, enabled variable pairs, default, fixed types/attributes and routing
must match the admitted baseline. Both-invalid variable pairs are equivalent.
Only then do CPUs restore their original root and CR0 guard and commit E1.
The generation becomes reusable only after both local commits. A delayed old
waiter cannot clear a new transaction's guard or root. Host waits are bounded,
hold no shared lease and service terminal/private INIT notification state.

Guest startup commits take the route lease then the core lease and require an
idle core, retaining that lease through the bounded target commit. This excludes
INIT racing a sibling's pending E0. Native WBINVD remains native; admission and
write protection preserve HWCR.INVDWBINVD and the host page-walk assumptions.

## Validation and limits

Focused production-method tests cover delayed E0/E1 consumers, generation reuse,
shared visibility/fixed attributes, final routing refusal, startup exclusion,
and low-root permission preservation/refresh. Independent state-space review
exercises three generations and delayed waiters. Hypervisor and DXE UEFI cargo
checks pass. The disposable executable cache suite now passes three paired
replay generations, physical default/fixed/variable readbacks, low-GPA NPF,
CR0-write interception, and INIT exclusion while a sibling is pending E0.
Exact artifacts and backend limitations are in
`work/native-raw-result-2026-09-15/cache-execution-result.md`. Linked
integer-only/stack/relocation audits remain required for the final production
image; emulator execution does not establish successful Windows boot.

This is the bounded, inspected Windows initialization replay, not arbitrary
cache-map reprogramming or hostile containment. Actual low guest accesses in
the replay window, unsupported final map changes, CR0 writes in the window,
or overlapping startup are explicit terminal outcomes. An E1 departure-wait
failure is post-commit and may report advanced RIP; it is not described as an
unchanged-instruction refusal. External SMM changes are not controlled by guest
MSRPM interception. Physical DEF/SYS_CFG drift checks reject observed divergence.
No timing cost or native boot success has yet been measured for this change.

## Native HWCR counter enable — 2026-09-16

The CPU-local owner now accepts HWCR bit30 (IRPerfEn) changes on the captured
Family1Ah Model44h stepping0 profile when the executing CPU advertises
Fn80000008_EBX[1]. Native CPUID retains that capability, and the existing native
MSRPM leaves IRPerfCount (C00000E9) accesses physical. A shadow-only enable
would therefore be inconsistent. The [visual manual review](native-hwcr-manual-verification.md)
records the exact manuals, hashes, page numbers and cross-reference chains.

HWCR reads use live physical state. All non30 bits must match the immutable
admission capture. Writes may change only bit30, preserve all other live bits,
and require exact physical readback before committing the validated guest RIP.
There is no mutable shadow and no shared-core HWCR bank. The owner retains
existing MSR boundary/CPL/debug/event validation. RDMSR returns zero-extended
EDX:EAX; WRMSR preserves GPRs and flags and completes once. Unlike MTRR writes,
the counter enable needs no guest TLB flush. Unsupported changes remain
terminal; invalid CPL follows the existing #GP path without completing WRMSR.

The native VMCB leaves PMC virtualization disabled; the HWCR owner also checks
VMCB0xB8[3] before physical access. Enabling this optional virtualization mode
would require a different counter-state owner. Guest INIT retains HWCR and the
counter as specified for other MSRs/performance resources; it does not restore
the pre-guest capture or apply their RESET defaults.

F400 detail16 retains unsupported-change refusal, detail17 identifies non30
physical drift, detail18 identifies failed physical readback, and detail19
refuses enabled PMC virtualization. Preparation refusals do not write HWCR or
complete the instruction. A detail18 stop can follow a hardware side effect;
guest completion is withheld, but no hardware rollback is claimed. Diagnostics
currently preserve raw guest operands and the failure category, not the
physical observed/expected HWCR values. SMM is outside this owner: live bit30
changes are accepted, while observed changes to other bits stop execution.

Physical counter accounting is shared with monitor execution on this CPU.
No host-instruction subtraction, counter virtualization, guest-only count
fidelity, or timing transparency is claimed. Broader HWCR controls remain
unsupported. This fixes a justified compatibility gap, but historical HWCR30
fault attribution remains static and Windows boot still requires an exact-image
physical test. No protections were disabled and no hardware was flashed.

## Stopped MSR operand evidence — 2026-09-16

USER3 event13 now exports immutable assembly-captured RIP, full RCX, architectural
EDX:EAX (the low DWORD of each register), hardware nRIP, and the original full-width
software stop reason/detail for owned cache MSRs other than SYS_CFG. It reuses the
existing raw capture and validates matching entry/exit sequence, VMCB addresses,
and physical CPU identity before publishing operands. No extra privileged read,
assembly capture field, or resumable-path transport is introduced. Existing
SYS_CFG events10/11 retain their encoding; identity mismatch uses event11.

The physical-cache-sample flag qualifies only the SYS_CFG-specific sample and
does not gate other captured MSR operands. Event13 does not export read/write
direction or claim instruction completion; EDX:EAX is the stopped operand, not
a read result or proof of a successful write. The bounded sticky first-fault
record retains reason/detail even if later barrier progress replaces event3.
Transport availability and first-fault-bank ownership still limit observation.
This supplies future exact-image evidence; it cannot identify the missing HWCR
operands in an older physical snapshot. Timing overhead and physical execution
of this diagnostic change have not been measured.

## HWCR batch validation

Portable results and artifact/source hashes are retained in
[`handoff-evidence/2026-09-16-hwcr`](handoff-evidence/2026-09-16-hwcr/validation.json).
The production driver is `work/hwcr-2026-09-16/production/driver.efi`, built with
`--boot --low-runtime`; no ROM packaging or flash was performed for this batch.

- 555 hypervisor tests, 28 DXE boot-library tests and 302 native-returning tests
  passed. Pure HWCR tests cover bit30 enable/disable/idempotence, every other-bit
  refusal, non30 drift, absent capability, enabled PMC virtualization and failed
  readback without a false-success or rollback claim.
- Executed two-CPU guest RDMSR/WRMSR tests pass per-thread isolation and three
  MTRR replay generations. A protected HWCR4 change stops at F400/detail16 with
  a complete terminal barrier and no post-write continuation. This uses a
  **modeled HWCR backend**, not physical HWCR or counter measurement; real guest
  instructions and the production handler execute against that seam.
- Production audit: 23,548 linked instructions, no FP/SIMD/xstate instructions
  or undefined symbols; bootstrap, fault-frame and debug-reset audits pass.
  The diagnostic build also passes its corresponding linked audits.
- Fresh independent implementation review found no blocking correctness issue.
  Enabled-HWCR guest INIT and integrated readback-failure guest-state snapshots
  remain unexecuted; INIT retention is supported by the visual manual review.

Validation: `python -m unittest discover -s firmware/squirrel -p
test_percpu_diagnostics.py` passes 16 tests; the optimized resident-runtime
`raw_capture_tests` profile passes both provenance/operand tests. The ordinary
debug resident-runtime host-test profile cannot link the unrelated firmware
`image_start` symbol; it was not counted as a passing run. No flash was performed.
