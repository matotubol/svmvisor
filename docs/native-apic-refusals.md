# Native APIC refusal handling

The physical boot after the VM_CR batch stopped at CPU0 NPF exit0x400 with GPA0xfee00030 and all CPUs activated. This differs from the preceding VM_CR refusal, but the old three-word generic payload cannot establish the instruction, direction, CR4, exact predicate, Windows stage, or SMM lock checkpoint. Version register reads already use the native hardware owner. Evidence is retained in `work/native-after-vmcr-2026-09-14/capture-analysis.md`.

This batch fixes two independently verified restrictions. The native guest walker now ignores leaf protection-key bits for effective supervisor mappings even when CR4.PKE is enabled. Effective U/S is the intersection across every paging level, not just the leaf or guest CPL. Instruction fetch continues to ignore keys. User-data MMIO, including protection key zero, remains refused before device access; no PKRU emulation, host PKRU change, or general protected-user-memory support is introduced. The shared strict host page parser is unchanged.

Physical ICR-idle checking now applies only to low-ICR writes that can send an interrupt. It occurs before the existing register owner's publication or device commit. Version and unrelated APIC accesses no longer read or depend on ICR delivery status as a precondition. The bounded busy refusal remains an implementation policy for sends. Routing serialization and shared startup ownership remain intact.

The adapter still accepts only the declared supervisor long64 DWORD MOV forms, validated instruction continuation, exact operand translation and aligned APIC offset. The ordinary final-data absent-NPT error must equal `(1 << 32) | 4 | RW`, and EXITINFO2 must equal the translated operand GPA. Nested U/S=1 is correct for the baseline even at guest CPL0; page-walk, execute, protection, shadow-stack, GMET and SNP/RMP variants are not silently accepted. CR4.CET remains refused pending a complete CET state/reset review. CR4 bit24 is reserved in the applicable AMD manual, not an admitted PKS control.

The admitted APIC page remains UC by MTRR evidence. Guest PAT UC, UC-, WP, WT or WB combines to UC under that prerequisite; WC remains WC and is refused. Instruction/table RAM still follows its separate WB admission. UAIE applies only to the already decoded eligible DS/ES data operand, with instruction and default-SS addresses retaining their own rules.

## Detailed stopped-state evidence

New terminal context kind13 retains stage0x83/version1 and three DWORDs. It implies NPF exit0x400; the existing 11-bit field identifies the actual predicate and the remaining64 bits carry an explicitly labeled operand. Existing kinds0-12 retain their meanings. The existing snapshot kit continues collecting the same wire frame.

The real runtime calls the detailed form of the existing MMIO owner. It distinguishes APIC mode/cache preflight, ICR busy, guest controls, instruction bytes actually fetched, operand and table translation, PAT, exact NPF information/address mismatch, pending events, continuation and register-owner errors. Diagnostic collection does not reread guest instructions or devices, allocate, or add transport to the hot path. When the payload carries CR4, PAT or another operand, it does not claim to also contain GPA or RIP. Truncated instruction prefixes are explicitly labeled. Rejected preparation preserves VMCB/GPR/device state; it does not advance RIP or inject a guessed guest exception.

The wire code/operand table and focused diagnostic test evidence are in `work/native-after-vmcr-2026-09-14/apic-diagnostics.md`. Independent source and linked-image reviews are separate artifacts in that directory. The decoder tests include all2048 possible reason codes and reject malformed encodings; old kind8 remains a generic GPA observation.

## Architectural and execution evidence

The review used rendered original AMD APM2 rev3.44 March2026 and target PPR57896 rev3.00 August2024 pages, including full tables and cross-references. APM2 section5.6.7 explicitly excludes supervisor pages and instruction fetch from MPK; sections5.7 and18 define the retained CET boundary; sections15.25.6-8 define NPF and cache combinations. PPR page72 confirms target PKU/CET feature definitions, and page56 defines the real Version register. Complete page maps, hashes and unresolved dependencies are in the CPU-mode and Version architecture reviews beside the capture.

Focused tests cover every protection key and every ancestor U/S position for1GiB/2MiB/4KiB leaves, user-key refusals including zero, unchanged rejected state, exact diagnostic operands and no additional reads. The disposable `guest-apic-pke` fixture enables guest PKE, denies all keys in PKRU, iterates all16 keys on a supervisor APIC mapping and checks Version equality before restoring guest controls. Integration requires both that witness and the VM_CR read/fault-retry witness in the two-CPU and24-CPU runs. A fixture definition alone is not a passing execution result; retained run summaries establish completion.

No timing baseline, physical Windows boot, SMM-lock milestone, protected-Windows compatibility, migration or sandbox readiness follows from these tests. Physical programming and a subsequent boot require their own exact-image evidence. The next detailed stop should identify the rejected predicate instead of forcing another guess from the APIC address.
