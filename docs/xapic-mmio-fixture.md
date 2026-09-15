# Bounded xAPIC MMIO register fixture — 2026-09-13

Implemented and validated in the retained authoritative checkout
`C:\Users\mato\.codex\worktrees\7a58\svmvisor`. All pre-existing dirty work
and historical evidence were preserved. Three fresh GPT-6 Astra High agents
performed core implementation, harness integration and independent review;
root integrated evidence checks and serialized shared builds and execution.

## Implemented behavior

`FixtureApic` still owns the sole `LocalApic` and mode. Fixed BSP ID zero is
readable as x2APIC MSR 802h and legacy xAPIC offset 020h, with separate bus
encoding. Shared bounded register helpers serve both adapters for TPR, PPR,
all eight IRR/ISR windows and EOI. CR8 keeps its existing full-byte TPR and
V_TPR synchronization. No MSR exit is fabricated to service MMIO.

The harness maps guest VA `0xc000` to GPA `FEE00000h` while leaving that GPA
absent from NPT. The originally suggested `0x7000` was already a guest guard
page, so it and the `0x6000` NPF probe remain unchanged. No host APIC mapping,
backing allocation, read or write is performed. The linked guest occupies
2,806 bytes of the existing 4,096-byte code page.

Mapping admission checks the installed builders, read-only/RX code backing,
alias permissions and absent APIC NPT mapping. The resulting record binds
guest CR3, nested root, alias and code HPA. The caller owns and preserves the
tables and installed code throughout the stopped/entry/exit session; root
comparison does not detect remapping at the same address. Only permitted A/D
updates occur in these fixtures. Alternate loader-continuation mappings carry
no MMIO admission record.

`handle_fixture_mmio` accepts only a real exit 400h and the exact unprefixed
DWORD instructions `8B 03` (EAX,[RBX]) and `89 03` ([RBX],EAX). It checks the
actual installed instruction pointer at saved RIP, matching roots and classic
NPT controls, 64-bit CPL0 code, alias-derived RBX/GPA, register alignment,
direction, final-access stage and the complete raw NPF word. Admitted NPF
information is exactly `0x100000004` for reads or `0x100000006` for writes:
missing page, user nested access and final-GPA stage. All other bits, including
fetch, page walk, shadow-stack and feature-dependent bits, are refused.

Only xAPIC mode admits MMIO. Successful reads zero-extend EAX; writes preserve
GPRs; both preserve flags. Checked continuation, pending-event/control and TPR
ownership checks precede controller mutation and RIP commit. The real caller
observes armed delivery before dispatch. TPR/EOI writes refuse armed mutation;
reads can inspect retained armed state after the required observation.

Unsupported MMIO operations stop unchanged and do not manufacture #GP, #PF
or ESR behavior. This includes ID/RO writes, version/SVR, nonzero EOI, TPR
operands outside the admitted byte, other widths/encodings, alignment, modes,
addresses and provenance. x2APIC ID writes separately require the existing
checked #GP(0) delivery path.

## Validation

| Check | Result |
| --- | --- |
| Core host tests | **197 passed** (previous 186; ten MMIO tests and one ID test added) |
| Returning-DXE tests | **251 passed** |
| Hypervisor UEFI target check | Passed |
| Full emulator regression matrix | **35 profiles passed** |
| Evidence parser negative controls | **20 passed** |
| Independent review | No unresolved blocking findings in this bounded slice |

The 35 profiles comprise three strict AVX/SSE/FXSAVE runs, four ownership,
seven synthetic fault, ten extended-state, ten relocation profiles and one
RDTSCP-disabled run. Profiles deliberately stopping earlier remain negative
tests; they do not all execute APIC fixtures. Every completed interrupt fixture
now requires five separate exact-once MMIO/mode/IRETQ/refusal/ID-fault markers.
Twenty parser controls remove, duplicate or corrupt new markers, remove
historical markers, and add contradictory backend-gap markers.

Each completed new positive suite runs 16 sessions. Each session completes
13 MMIO reads, three MMIO writes, six MSRs, one CR8 write and two query
hypercalls, with exactly one interrupt consumption and handler IRETQ. It
checks fixed ID and withheld CPUID APIC/x2APIC bits, full-byte TPR/CR8/PPR,
pending IRR, service ISR, zero EOI, actual mode changes and retained shared
state on both buses. That is 256 successful MMIO NPF completions per suite.
The first strict trace records actual read NPF `info1=0000000100000004`,
`GPA=00000000fee00020`, and write NPF `info1=0000000100000006`,
`GPA=00000000fee00080`.

Thirteen additional sessions start at actual stopped NPFs and verify unchanged
VMCB, frame and APIC state on refusal: unsupported version, ID write, nonzero
EOI, reserved TPR operand, x2APIC/disabled modes, WORD/QWORD, alignment,
wrong GPA, unowned byte pointer, armed write and APIC instruction fetch.
The fetch case refuses on provenance before the raw-NPF check; unit tests
separately flip every one of 64 raw bits for both read and write exits.
Other unit cases cover mapping admission, roots, mode, controls, pending
events, windows, cross-bus retention, flags/GPRs and invalid continuation
for reads, TPR writes and EOI with nonempty ISR.

Sixteen new actual x2APIC ID-write #GP(0) sessions repair the index to TPR,
IRETQ to the original WRMSR and complete TPR `0x2b`. These are separate from
the retained 160 MSR repair sessions: **176 MSR #GP retries total** per completed
suite. Existing 32 intercepted plus 32 direct CR8 #GP/IRETQ repairs and 16
direct valid CR8 writes remain passing on the strict backend.

Runtime: `work/qemu-cr8-fault/build-validation/runtime/bin/qemu-system-x86_64.exe`,
SHA256 `581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`.
It was not modified. Strict runs use
`-GuestCr8Unblock -CorrectedBackend -RequireCr8Faults`. The prior corrected
runtime still completes the new MMIO fixture but reports the two known CR8
operand gaps; strict mode rejects it. Historical stock controls are retained
in the previous archive.

Fresh logs and exact session paths are in `work/apic-coverage/validation`.
`regressions.json` is the matrix index. `independent-review.md` records source
review and primary reference checks. The external archive is
`C:\Users\mato\Documents\Codex\2026-09-10\svmvisor-bios-f7-analysis\outputs\apic-mmio-2026-09-13`;
its manifest covers source, artifacts, runtime and evidence. The source delta
compares against the previous archived dirty source, not an unrelated Git HEAD.

## Primary references and remaining gaps

AMD APM volume 2 revision 3.44, sections 15.25.6, 16.3.2–3, 16.6.3–4,
16.9–16.11 and Table 16-6; volume 3 revision 3.37, MOV/MSR/CPUID definitions.
Local PDF SHA256 values are respectively
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c` and
`c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`.
The reviewer verified the official current 3.44 listing; direct document
transport returned 404, so the pinned local text remains normative evidence.

Exact missing semantics remain model-dependent ID writes, broad invalid MMIO
outcomes and cacheability; none is inferred from x2APIC MSR faults. The current
PAT-index-zero alias establishes trapped synthetic access, not hardware APIC
cacheability fidelity. Version and SVR wait for truthful LVT inventory and
complete software-enable/input/delivery/masking semantics. Generic CPUID
APIC/x2APIC stays clear. RESET/INIT, general instruction decoding, guest timer
programming and scheduling, physical interrupt routing, IOAPIC/MSI/NMI and
SMP remain outside this slice.

This batch measures semantic completion, refusal and entry/exit/event counts.
Retained TCG timestamp checks are not physical latency or clock-drift
measurements. Windows boot and Hyper-V/VBS/HVCI/protected-state compatibility,
native timing, telemetry-loss characterization and malware containment are
not established. No hardware activation, driver installation or Windows
protection change occurred. A policy stop or unknown exit means incomplete
execution and analysis, not a benign verdict. The earlier physical returning
probe applies only to its exact tested image.
