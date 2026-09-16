# Native CPUID compatibility and prefix review - 2026-09-16

Subsequent execution update: the separately built NRIPS backend now passes
the real compatibility32/long64 prefix fixture on both AP startup generations.
See [the final backend report](../../../tools/qemu-nrip/README.md) for the exact
binary and run evidence. The predecessor failures below remain historical
negative evidence; they are not the final execution status. CPL3 remains
modeled-handler coverage only.

Implementation is directly on main. No flash, physical boot, Windows boot, or
timing measurement was performed. The delivery priority remains trusted native
Windows continuation; none of this establishes malware containment or invisibility.

## Defect and fix

The resident hardware-nRIP route and dispatcher previously required long64 CPL0
and exactly two bytes. Ordinary user64 CPUID, WOW64 compatibility32 CPUID and
legal prefixes therefore fell into unsupported fetch/dispatch paths or stopped.

`handle_native_cpuid_with_nrip` now accepts exclusively stopped hardware CPUID
exits with same-CPU NRIPS evidence, canonical RIP/nRIP and a strictly forward
2..15-byte difference. It supports long64 and compatibility16/32 at CPL0..3;
the existing unpaged real/protected startup modes additionally require startup
ownership. LMA/LME, paging, PAE and CS.L/D checks reject incoherent state. The
native profile remains 48-bit; LA57, VM86, legacy paging, instruction-pointer
wrap, and a next offset beyond the legacy/compatibility CS limit remain stopped.
The pointer is a CS-relative offset. The unprefixed two-byte path needs no
CS.base addition or guest-memory access. The change does not weaken the general
continuation validator.

For prefixed3..15-byte CPUID the caller now supplies the exact coherently fetched
instruction span. The dispatcher verifies the `0F A2` suffix, segment/66/67
prefixes and at most one final REX in long64. LOCK, REP/F2/F3, non-final REX,
legacy/compatibility REX, missing bytes, stale lengths and other opcodes are
refused unchanged. Some processor-accepted redundant/ignored prefix forms are
therefore deliberately unsupported rather than guessed.

`fetch::cpuid_instruction` reads only that span into a bounded15-byte buffer.
It reuses the existing physical reader and four-level translation after checked
CS.base addition in compatibility mode, or the existing unpaged startup model.
Every page retains NX, effective U/S, SMEP, guest PAT, paging-structure cache
type and backing checks. Linear/segment/IP wrap, CD/NW, unreadable/MMIO/unowned
backing and legacy paging remain refused. Generic instruction and MMIO fetch
still require their original long64 profile. No shared parser admission was
weakened. Coherent stopped code/table backing remains an explicit caller
precondition; guest physical mappings are never dereferenced unchecked.

The caller supplies the owning CPU's live HWCR bit35. At CPL>0 with that bit set,
the dispatcher prepares #GP(0), preserves RIP/GPRs/flags/shadow, and returns the
distinct `GeneralProtectionPrepared` outcome. Root integrated pending-fault
accounting in the resident runtime and a shared checked EVENTINJ helper. The
no-NRIPS byte fallback explicitly stops a user-disabled CPUID. Existing byte
APIs otherwise retain their constrained byte-fetch coverage.

Completion reuses native CPUID filtering, zero-extended results, RF clearing and
interrupt-shadow retirement. Undefined upper GPR halves in compatibility mode
may be zeroed. It neither changes the returned feature policy nor invents host
CPUID operands. Pending injection, interrupted delivery, unowned virtual IRQ
controls and TF remain explicit stopped cases, including TF plus user-disable.
All fallible checks precede emulated register/RIP changes. The user-fault branch
validates the instruction/mode and pending-event boundary before injection.

Review correction: APM2 15.7 says generic illegal-opcode checks *generally*
precede instruction interception; this is not definitive proof that illegal
LOCK CPUID cannot reach exit72. APM2 Table15-7's CPUID row is already incomplete
with respect to the newer HWCR user-fault control. APM2 15.12.7 (PDF582/index581,
printed520) and exception priority Table8-9 (PDF327/index326, printed265) do not
resolve instruction-intercept ordering. Therefore bare nRIP length3..15 is
insufficient reviewed evidence for safe prefix completion. The final mandatory
byte-validation path resolves this ambiguity by refusing LOCK. The nRIP length
matrix alone is not proof of prefix semantics.

## Visual manual review

Every page below was rendered and inspected in `cpuid-pages/`; extraction was
used only to locate pages. PDF page numbers are one-based, followed by the
zero-based index and printed page. These are AMD-authored normative documents.

| Document | SHA256 | Revision / applicability |
| --- | --- | --- |
| `docs/24593_3.44_APM_Vol2.pdf` | `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c` | 3.44 March 2026; AMD64/SVM architectural rules |
| `docs/24594_3.37_APM_Vol3.pdf` | `c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4` | 3.37 July 2025; CPUID and instruction encoding |
| `docs/57896-3.00_PPR.pdf` | `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5` | 3.00 August 2024; Family1Ah Model44h B0 target PPR |
| `24592-3.24.pdf` in this work directory | `0168c6ff393b13d53b8d1371efeeccd85bd85050131cec65928e703fd4ba9330` | 3.24 August 2025; AMD64 application register semantics |

APM1 was absent from the main library and read-only archive. Official AMD
document entry was found at https://docs.amd.com/v/u/en-US/24592_3.24 . The
official static PDF and indexed combined-PDF URLs returned HTTP404. The original
AMD-authored PDF was obtained from the mirror
https://kib.kiev.ua/x86docs/AMD/AMD64/24592_APM_v1-r3.24.pdf and pinned above;
this is explicit mirror provenance, not an AMD-hosted download claim.

Reference chains followed:

- APM2 15.7/15.7.1 PDF570/569 printed508 -> PDF571/570 printed509:
  before-instruction state, exception/intercept ordering, same sequential nRIP
  definition and NRIPS capability. Followed 15.9/Table15-7 in full PDF575..578,
  indices574..577, printed513..516, including CPUID row, adjacent instructions,
  all continuation rows. Complete interrupted-delivery chain 15.7.2/15.7.3
  PDF571..573, indices570..572, printed509..511 remains refused.
- APM3 CPUID complete definition and exception table PDF207..209,
  indices206..208, printed171..173 -> APM2 HWCR3.2.10 PDF133/132 printed71 ->
  applicable target PPR HWCR complete table PDF203..204, indices202..203,
  printed203..204; feature indication PDF117/116 printed117. This resolves
  bit35/CPL>0 outside-SMM fault semantics. Guest SMM is not admitted here.
- APM3 encoding1.1/1.2 PDF39/38 printed1, PDF41..43/indices40..42 printed3..5,
  complete legacy-prefix table PDF45/44 printed7; LOCK cross-reference
  PDF49..50/indices48..49 printed11..12; REX format/table and ignore rule
  PDF53/52 printed15. Maximum encoded instruction length15, fixed CPUID opcode
  and generic illegal LOCK are distinct from nRIP arithmetic.
- APM2 modes1.3/Table1-1 and Figure1-6 PDF73..74/indices72..73 printed11..12;
  2.1 PDF85..86/indices84..85 printed23..24; segmentation/register differences
  PDF90..91/indices89..90 printed28..29. APM1 3.1.2.4 PDF66/65 printed30 and
  3.4.5 PDF110/109 printed74 resolve upper32 GPR behavior across mode changes.
- The prefix-fetch extension additionally follows APM2 2.3 PDF89..90,
  indices88..89, printed27..28; full descriptor-change Table4-7 and footnotes
  PDF165..166/indices164..165 printed103..104; 4.9.1 PDF167/166 printed105;
  long-mode translation5.3 and CR3 format/continuation PDF201..203,
  indices200..202 printed139..141 -> PCID5.5.1 PDF220/219 printed158. These
  establish compatibility uses long-mode page translation with legacy segment
  interpretation. Existing four-level parser/PAT/permission checks are reused,
  not reimplemented or declared newly proven by this review.
- APM2 RFLAGS3.1.6 complete figure and TF/RF descriptions PDF114..116,
  indices113..115, printed52..54 -> single-step13.1.4 PDF470/469 printed408
  and control-transfer debug context PDF471/470 printed409. TF remains refused;
  no partial #DB emulation or branch-step claim is made. Interrupt shadow
  15.21.5 PDF596..597/indices595..596 printed534..535 is consumed on successful
  completion only. Further branch-step conditions remain outside support.

## Validation and measured limits

`cargo test --locked -p svmvisor-hypervisor --test native_cpuid_nrip`: 9/9 pass.
The mode/prefix-length test executes 280 inert-VMCB combinations (five modes,
four CPLs, fourteen decoded lengths with supplied exact bytes), plus existing no-fetch/OSXSAVE/shadow
checks. Other cases cover CPUID-disable #GP versus privileged completion,
invalid CPL/control combinations, lengths/addresses, CS bounds and wrap,
pending events and unchanged stopped VMCB/frame on refusal. These are modeled
handler tests, not hardware decode, actual privilege transitions or delivered
user #GP proof. Added explicit LOCK/REP/REX/suffix/missing-byte refusals and
bounded fetch tests spanning noncontiguous physical pages with nonzeroCS.base
at compatibility CPL3; NX, U/S, unreadable bytes and startup wrap are refused.
`cargo test --locked -p svmvisor-hypervisor --lib host::resident::fetch::tests`:
all six existing parser/cache/permission regression tests pass.

Added opt-in fixture `guest-cpuid-nrip`: the existing copied startup code executes
`66 67 0F A2` after PG/LMA enable and before the long64 far jump (compat32/CPL0),
then `66 67 48 0F A2` in long64/CPL0. It records leaf1 signature and CS selector
in guest RAM; the BSP checks both on each INIT/SIPI generation. The runner
requires the exact CPU/generation marker set. CPL3 and actual fault delivery are
not claimed by this fixture.

Pinned TCG backend `677158d2f10933bfc8770e3741a3c6ebf33466d1f7f71fee87e6aec3e009b240`
does not support NRIPS. Both initial runs (`cpuid-fixture`, `cpuid-fixture-nrip`)
reached the compat32 CPUID but stopped in the expected unsupported byte-fetch
mode with exit72, reasonf001/detail2. Explicit `nrip-save=on` prints the TCG
unsupported-feature warning. No nRIP was fabricated. Final gated reproduction
is `cpuid-nrips-unsupported/`; its failed summary is an execution-coverage gap,
not a passing CPUID test. No actual prefixed completion was established.

The default two-CPU `loader-new-root` regression passed in `cpuid-baseline/`
(exit33, repeat INIT/SIPI, root reclamation and nonidentity runtime mapping).
Exact tested driver SHA256:
`50943c59bdd7ddad8ba66c72f355287cd668fd9cf299af34853b69fbe64047ac`.
Default fixture SHA256:
`75ac4f66bbd9f0393be172a134f9b8dfcf9c8c64a21440c71973bb1b625a6272`.
This driver predates the final mandatory prefix-byte validation/fetch change and
other root-agent changes; its pass proves the unchanged fallback regression,
not the final prefix implementation. Root must use the final aggregate build
and test report for delivery evidence. Timing distributions, physical nRIP behavior, WOW64 execution,
Hyper-V/VBS/HVCI compatibility, native Windows boot and sandbox readiness remain
unmeasured/unproven. No Windows protection was disabled.
