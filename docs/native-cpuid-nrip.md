# Native CPUID continuation from hardware nRIP

The September14 terminal capture identified a CPUID exit (72h) on CPU0 at
`fffff806a77701d9` which the runtime stopped because its software instruction
fetch failed. The precise fetch predicate was not exported. The physical
terminal record and verified image are retained in
`work/native-terminal-capture-2026-09-14/capture-analysis.md`.

Native CPUID handling now uses the processor's next-instruction RIP for the
restricted profile: NRIPS capability established on the owning CPU, 64-bit code,
CPL0, canonical RIP/nRIP, and an exact two-byte forward difference. EXITCODE72h
provides CPUID identity; a two-byte length excludes prefixes. No guest-memory
read is necessary for this register-only operation. An invalid hardware
continuation remains stopped, with no guessed advance or byte-fetch fallback.

The exact same dispatcher still applies native CPUID filtering, validates pending
and interrupted events and virtual controls, refuses TF, zero-extends results,
and completes RF/interrupt-shadow state. Every fallible check precedes VMCB/GPR
commit. Other modes, CPL>0 and processors lacking NRIPS retain the existing
instruction-fetch path; their compatibility is not expanded by this change.

This restriction is deliberate. APM2 Table15-7's CPUID statement cannot override
the current CPUID exception definition: HWCR.CpuidUserDis can cause #GP outside
CPL0, and LOCK can cause #UD. General prefixed/CPL>0 emulation is not established.

The independent fetch review found legal PAT selections choosing WB that the
old fetch policy rejects, and omission of the applicable Ryzen TOM2 forced-WB
default for high memory. Neither was proven to be the physical predicate in
this capture. Those memory-type restrictions remain in the byte-fetch/MMIO
reader and are documented separately in
`work/native-cpuid-fetch-review-2026-09-14/review.md`; no memory admission was
relaxed as part of the CPUID change.

## Manual evidence

Critical pages were rendered and read visually, including referenced tables and
adjacent explanations. APM2 24593 rev3.44 March2026 SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`:
15.7/15.7.1 PDF570–571 printed508–509; 15.9/Table15-7 PDF575–576
printed513–514; decode-assist Table15-6 PDF574–575 printed512–513;
VMCB/exit appendices PDF799–804 printed737–742 and PDF818 printed756;
RF/TF PDF115–116 printed53–54 and PDF470 printed408; interrupt shadow
PDF596–597 printed534–535; interrupted-event chain PDF571–573 printed509–511.

APM3 24594 rev3.37 July2025 SHA256
`c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`:
CPUID definition/complete exceptions PDF207–209 printed171–173;
NRIPS feature table PDF687–689 printed653–655; LOCK prefix PDF49–50
printed11–12. Rendered pages are retained in `work/cpuid-nrip-review/`.

## Validation boundary

Focused stopped-state tests exercise the actual shared dispatcher, including
legal WB cache selection rejected by the former fetch path, zero-extension,
OSXSAVE policy, RF/shadow retirement, a page-crossing opcode, and transactional
refusal for missing capability, invalid lengths/addresses, wrong exit/mode/CPL,
TF and pending-event/control conflicts. Full linked-code audits and native
fallback regressions are retained per build in the capture work directory.

The pinned system-TCG backend does not advertise or implement NRIPS. Its native
fixtures therefore exercise the byte-fetch fallback. Unit success and linked
audits must not be described as executed hardware-nRIP proof. The physical
Windows attempt on the new image is a separate result. Native timing, Hyper-V/VBS
coexistence and Windows boot are not established by this implementation.
