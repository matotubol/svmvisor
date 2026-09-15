# Native SYSCFG fixed-MTRR transactions

The captured Family1Ah Model44h native boot path now uses the complete
[cache replay owner](native-cache-replay-owner.md). Its logical MTRR/SYS_CFG
bank and stable physical controls supersede the pass-through behavior described
below. This document retains the historical SYSCFG-only fallback and evidence.

The 2026-09-15 physical capture f69229e9c9cb466a89716316a103e8ec matches the APIC-refactor image and reports an MSR refusal at C0010010 after all CPUs activated. The prior policy intercepted every SYSCFG write without a completion handler.

## Evidence

Binary Ninja 6.0.10601 decoded the user-designated Windows kernel 10.0.26100.9444. KiReadFixedMtrr enables bit19, saves eleven raw fixed registers and clears19. KiWriteFixedMtrr clears18/sets19, restores those raw values, then clears19/sets18. KeLoadMTRR surrounds replay with Windows processor rendezvous, cache disable, WBINVD, TLB handling and MTRR_DEF_TYPE disable/restore. The alternate AMD branch conditionally clears18/19. All these SYSCFG transformations are supported; no instruction-address whitelist is used.

Evidence and exact binary hashes: work/native-syscfg-2026-09-15/binary-analysis/report.md. Public symbol GUID matches but public PDB age differs; names are supporting labels, assembly facts come from actual bytes. The physical snapshot has no failing RIP or module hash, so it does not identify one exact instruction in this binary.

Normative source: PPR57896 rev3.00 pp26-27/43/126-130/202 and AMD APM2 rev3.44 sections3.2.1,7.6.4,7.7.2,7.8.5,7.9.1. Original-page image review and cross-reference chain: work/native-syscfg-2026-09-15/manual-review/review.md.

## Owned instruction and state

The target-gated SYSCFG owner permits no-op writes and changes only to bits18/19. All other controls must remain unchanged; reserved input/current state and enabled encryption remain unsupported. Reads and fixed/variable MTRR instructions execute natively. This preserves actual hardware visibility and writes instead of creating a divergent software shadow.

Preparation validates same stopped VMCB/frame, actual MSR exit, opcode/NRIP evidence, CPL, mode, pending events, debug state and continuation before reading hardware. Full WRMSR input is EDX:EAX, taking each register's low32 bits. Unsupported preparation preserves hardware, VMCB and GPRs. CPL faults prepare #GP(0) without completion. Only a successful native write/readback or an admitted no-op commits RIP and clears instruction-completion state. A failed readback remains stopped and is explicitly a post-write failure; no rollback claim is made.

On long64 CPUs supporting NRIPS, SYSCFG dispatch uses hardware instruction length before attempting any guest-memory fetch. This allows Windows' cache-disabled/MTRR-disabled update interval without weakening WB guest-memory admission. Legacy unpaged fallback retains the existing byte-fetch and segment limits.

## Shared routing and memory

Retained runtime images, stacks, tables and host descriptors are above1MiB. SYS_CFG18 affects the low fixed range; SYS_CFG19 controls access/visibility of its DRAM attributes and is thread-private. The existing shared route guard spans physical SYSCFG preparation/write/readback/commit and low guest-RAM sample through actual alias read/removal. Contention retries the original instruction without completing it.

For the reviewed AMD target, low RAM is read only with bit18 enabled and a fixed byte proving WB plus read/write DRAM (0x1e). The host temporarily exposes bit19 if necessary, samples attributes, restores the exact original SYSCFG with readback, and only then creates its RAM alias. It never reads low RAM with bit18 clear. Conventional emulator profiles retain their conventional fixed-byte interpretation.

Direct guest fixed-MTRR writes remain native and do not acquire this lock. Correctness assumes the trusted OS's coordinated update sequence: fixed writes occur while bit18 is clear, then routing is enabled after replay. This is not containment of arbitrary hostile or uncoordinated memory-type changes.

## Refusal snapshots

New versioned stage85 retains reason, CPU slot and write direction. Normal target values fit in paired DWORDs, preserving requested and observed values and exact XOR delta. Wide cases retain one complete typed operand and explicitly omit the other. Instruction-boundary failures retain one full context. Readback failures identify observed as post-write readback. RIP is not exported. Existing stages remain compatible and CRC framing/RTL are unchanged.

Terminal-only MMIO mapping now recognizes validated disabled-MTRR UC behavior; normal WB RAM readers remain strict. This preserves failure reporting during a Windows cache-update window without modifying MTRRs to force reporting.

## Validation limits

Unit tests cover the Windows control sequences, bit/operand matrices, pending/fault/debug/evidence rejection, cache-disabled NRIP lengths, legacy continuation, preparation cancellation, low-memory routing and terminal UC rules. Linked payload auditing and existing executed SMP/card fixtures are required for each delivered candidate. The current QEMU backend has no target SYSCFG implementation; these tests do not establish physical SYS_CFG execution or successful Windows boot. First real execution of this change requires the next exact-image boot.

No native latency or timing baseline was measured. Extra low-RAM attribute observations are bounded serializing accesses; cost is unmeasured. Encryption, general cache-map virtualization, Hyper-V/VBS coexistence and hostile workload containment remain unsupported. No Windows protections or motherboard BIOS settings are changed.
