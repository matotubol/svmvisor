# Post-firmware cache bank ownership correction

Implementation and validation completed across 2026-09-15/16. No hardware flash
or new physical boot was performed by this source task.

## Problem and resulting behavior

The exact physical admission failure was APIC2/MSR26C,
`1D1D1D1D1D1D1D1D` versus BSP `1515151515151515`, bit3 WrDram. Masking the
difference would lose routing semantics. The complete AMD table additionally
reserves the active1D extended tuple. Native analysis of actual F7 CpuDxe found
BSP-only shadow-ROM routing writes and later BSP/AP synchronization callbacks,
including ExitBootServices. The previous comparison ran before that firmware
work completed.

The authoritative bank is now captured after the original ExitBootServices
returns success, while every guest remains stopped:

1. Retain the existing firmware inventory, CPU and mapping preparation.
2. Revalidate current BSP mappings/cache controls after EBS, then sample its
   complete bank into the existing pool-owned capture.
3. Broadcast physical INIT/SIPI once. Serially release each AP into the existing
   native-boundary callback. After CPU and high-memory closure validation, it
   writes one complete fresh cache observation and parks before arm/VMRUN.
4. Once every sample is published, BSP applies the strict BSP-bank and sibling
   topology/bank checks, initializes the existing shared cache owner, and
   publishes admission. Only then does a second serial release activate APs.
5. Actual runtime arm still resamples and requires exact equality with that
   CPU's fresh observation. BSP activation remains after AP completion.

No physical MTRR/routing normalization writer was added. Physical SYS_CFG19
visibility sampling retains its existing checked restoration. Raw fixed-bank
equality and active-variable comparisons remain; the prior dormant-variable
exception remains unchanged. EBS failure does not start or consume the survey.

Complete Table7-13 byte validation now applies to active E/FE/extended fixed
configuration. SYS_CFG18=0 with E/FE active is a separate explicit restriction
of this bounded bootstrap profile, not an assertion that bit18=0 is reserved.
The ordinary guest replay still supports temporary logical18 changes under its
existing low-memory deny root and paired guest-CD guard.

Collection/comparison failure keeps all guests stopped. A later activation
failure can follow an earlier AP bootstrap guest's completion, as in the existing
serial activation design. First-arm drift is tested before any guest entry.

## Diagnostic continuity

Post-EBS cache refusal48 now publishes retained exact operands through the
existing BSP-only USER3 publisher, whose current UC mappings, PCI configuration,
image IDs and boot ID are revalidated. This publication occurs during activation
failure handling because the earlier Prepared.complete call has already ended.
The decoder binds only current-image/current-boot cache records to the matching
post-EBS activation failure and distinguishes active tuple and profile predicates.
No new host-memory reader or transport command was introduced.

## Executed checks

| Check | Result |
|---|---|
| Hypervisor complete host suite | 552 passed |
| DXE native-resident-boot library suite | 28 passed |
| DXE documented native-returning suite | 302 passed |
| Snapshot decoder tests | 15 passed |
| Production and fixture UEFI compile checks | Passed |
| Four-CPU survey positive execution | Passed |
| Four-CPU mismatched bank refusal | Passed, no arm/entry/ACK |
| Four-CPU post-sample arm drift refusal | Passed, arm12 and no entry/ACK |

The attempted full DXE native-resident-boot host command exposed an existing
test feature-gating mismatch: native_transition_multi is enabled by
native-preflight but imports native_transition, which this profile does not
export. The failed log is preserved; production compilation, its library tests
and the documented native-returning profile pass. No unrelated test source was
changed to conceal that limitation.

The disposable fixture changes an actual disabled BSP MTRR pair in a late EBS
callback and verifies the fresh sample contains its sentinel. Its sample order
is BSP0, AP3, AP2, delayed AP1. Assertions require every sample and admission to
precede the first arm/entry. Ordinary loader root replacement/reclamation,
nonidentity runtime remapping and AP restart checks also execute.

QEMU lacks the target hidden routing/sharing fields, so the explicit fixture
models those fields and injects supported bank mismatch or local arm drift.
Standard MTRR/PAT reads and the EBS sentinel are actual emulator MSR operations.
The fixture helper, test modes, reversed order and trace markers are absent from
the normal production build. These results prove execution ordering and refusal
paths, not actual-platform cache coherence or firmware callback success.

## Production artifact

`execution/production-final-build/driver.efi`

- SHA256: `87953f4584a32d67cf7fe54e8a2a9ded6ccf14fe0a2f71a672841a1d3167d04c`
- Profile: `--boot --low-runtime`, no test output or survey fixture.
- Payload SHA256: `acb32f7936d4f5affe75f1007b6612f455d36d9d3467a7a8178a4b735f186430`
- Source manifest SHA256: `b564b987f56e9b0d991e00f1edab89ceb48c61965955179bc4c943fb3c1587dd`
- Builder audits: 23,384 linked payload instructions, no FP/SIMD/xstate
  instructions, no unresolved symbols; copied bootstrap has no relocations.

Independent source/manual review is in `bios-review/report.md`; independent
execution and final linked review are retained under `execution`.

The final independent DXE review resolved symbols through a byte-identical
relink and decoded the actual PE bytes. The new sample/park call closure has
no firmware calls, allocation, unresolved indirect calls, recursion, dynamic
stack changes or FP/SIMD. Its conservative bound including the live callback
and boundary frames is 6,176 bytes versus 126,976 usable AP bootstrap bytes.
Linked branches preserve owner initialization before admission and refusal
before activation. All four final builds share 288 verified frozen source
files. Details and exact execution bindings are in
`execution/execution-review.md` and `execution/execution-review.json`.

## Limits and next result

The correction preserves strict final admission instead of assuming that the
F7 firmware synchronization succeeded. The next exact-image physical run must
establish the actual final banks and boot progress. No native timing baseline,
Windows guest boot, Hyper-V/VBS coexistence, changed Windows protection settings,
malware containment or undetectability is claimed by this batch.
