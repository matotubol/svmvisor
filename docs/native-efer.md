# Native EFER contract on the Ryzen 9900X

This describes the final 2026-09-14 implementation, including corrections discovered after the initial three-feature review. It is a bounded resident execution contract, not full architectural or Windows compatibility certification. The historical review reports record findings at their respective source revisions; this document states the resulting behavior.

## Ownership and admission

`NativeEfer` owns logical EFER per CPU. The guest-visible value excludes SVME; hardware VMCB backing always includes SVME. VMRUN/VMEXIT switches EFER, while VMLOAD/VMSAVE owns auxiliary state. Guest writes do not modify live host EFER. The owning CPU supplies CPUID80000001 ECX/EDX, 80000008 EBX and, when available, 80000021 EAX. Runtime and DXE callback share the five-argument admission policy. The callback requires exact agreement between the admitted owner and captured EFER before either destination is committed.

Initial continuation still requires long mode and the existing control/segment/capture checks. The assembly capture boundary still refuses initially enabled FFXSR because its FXSAVE capture can omit XMM state. Later runtime enablement is supported; initial capture completeness is not inferred from the expanded EFER mask.

| Field | Final behavior |
|---|---|
| SCE0, LME8, NXE11 | Feature-backed guest controls. Changing LME while CR0.PG is set faults. NX remains coherent with the software fetch walk. |
| LMA10 | Hardware-derived mode state; mismatching WRMSR value faults. INIT/startup synchronization follows guest LME and CR0.PG. |
| FFXSR14 | Gate 80000001.EDX25. Enable and preserve; clearing after enable remains explicit terminal refusal under the target PPR policy, except target-owned INIT. Direct hardware FX behavior is retained. |
| TCE15 | Gate 80000001.ECX17. Real guest EFER controls direct hardware invalidation behavior. Host scratch INVLPG uses host state. |
| INTWB18 | Gate 80000008.EBX13. WBINVD/WBNOINVD remain direct guest instructions; interruptible completion/restart is hardware-owned. Their separate instruction capability gates still apply. |
| UAIE20 | Gate 80000021.EAX7. Direct hardware semantics plus the bounded xAPIC data adapter below. |
| AIBRSE21 | Gate 80000021.EAX8. Real guest control is preserved across world switches. Guest protection is not proof of complete host speculation mitigation. |
| SVME12 | Private hardware backing is always one; native virtual CPUID hides nested SVM and requested logical enable faults. |
| LMSLE13, MCOMMIT17 | Model44h LMSLE unsupported evidence and absent MCOMMIT evidence produce architectural #GP on enable. Generic supported-but-unimplemented fields remain an explicit refusal, not invented success. |
| MBZ fields | Write-one produces #GP(0). Processor-absent feature enables also fault. |
| RAZ7:1 | Read zero; nonzero writes remain explicit unsupported-value refusal. The review did not establish write-discard semantics. |

Architectural faults preserve stopped RIP and operand registers and queue #GP(0) through the existing event owner. Read completion zero-extends EAX/EDX; writes preserve input registers. Backing mismatch, unsupported modes, pending events and debug completion limits remain separately diagnosed. An unsupported implementation case is not silently converted into a hardware exception.

## Instruction boundary

Actual native EFER MSR exits use hardware NRIP before any guest instruction-memory fetch when same-CPU captured NRIPS support and the long64/CPL0 constraints hold. EXITINFO1 must be exactly zero or one; canonical RIP/NRIP must advance without wrapping by 2 through 15 bytes. Typed evidence binds the entire stopped exit snapshot. The shared EFER semantic owner then commits either completion or fault retry transactionally.

AMD's common-exception-before-MSR-intercept ordering establishes that an authentic intercepted instruction passed common opcode/prefix exceptions, including illegal LOCK. This accepts hardware-decoded prefixed forms without inventing a software prefix parser or fictitious opcode bytes. It does not claim that arbitrary prefix combinations are legal. Rejected hardware evidence stops; it does not fall back to guessed bytes. Other profiles retain the exact unprefixed byte path. TCG does not advertise NRIPS, so its execution validates that fallback, not the hardware NRIP path.

Hardware diagnostics encode actual next RIP in distinct reason ranges 0x50..0x54 and 0x60..0x64; the decoder labels it `hardware_next_rip`. Byte diagnostics retain their own fetched-byte/length provenance. TF/post-instruction debug completion remains a refusal: implementing it requires the general debug owner, including BTF semantics.

## UAIE and dependent execution

In 64-bit mode UAIE ignores bits63:57 for DS/ES data references only; four-level addressing still validates bits56:48 against bit47. CS, SS, FS, GS and implicit descriptor references keep ordinary canonical checks. The supported xAPIC DWORD MOV adapter retains default segment: actual RSP/RBP bases use SS, R12/R13 and other bases use DS, and a SIB index does not select the segment. For admitted DS operands it sign-extends from bit56 before the canonical-48 walk. Instruction fetch and shared address APIs remain strict.

This adapter still refuses legacy prefixes, segment/address-size overrides, other widths, unsupported MOV forms and five-level paging. It validates permissions, RAM page tables, cache type, full DWORD extent, APIC offset and final-data NPF evidence before any register or APIC mutation. It does not claim general MMIO instruction emulation or full guest exception reflection.

## TLB and reset lifecycle

VMRUN reads but does not clear TLB_CONTROL. Resident dispatch now consumes an existing full-flush request only after an actual valid same-CPU VMRUN/VMEXIT, before processing INIT, EFER or another mutation that may request the next flush. Invalid entry retains the request. First entry, ASID/root publication, monitor-supplied controls, INIT and changed EFER request a full flush and invalidate clean bits. SIPI retains INIT's request. Ordinary unchanged resumes use zero.

This removes the established repeated full-flush cost without narrowing required invalidation. Returning diagnostic assembly retains its independent per-entry flush. No resident post-arm NPT writer was found; host scratch mappings retain explicit local INVLPG. Future shared live NPT changes require explicit invalidation and appropriate multi-CPU ownership. No physical latency reduction was measured, and this was not established as the freeze's cause.

INVLPGB's generic VMCB enable/global-ASID contract was reviewed, but the Model44h PPR does not advertise that instruction or guest enable capability. No unsupported target enable was added. Direct guest CR, INVLPG, INVPCID, PAT and MTRR behavior retains its hardware/software invalidation responsibilities.

## Evidence and limits

Original rendered PDF pages, including tables and crossreferences, were inspected: AMD APM2 rev3.44 PDF117-120 (printed55-58), 230-231 (168-169), 553-554 (491-492), 564-569 (502-507), 571 (509), 580-581 (518-519), 590-592 (528-530); APM3 rev3.37 PDF44-53 for prefixes/addressing, 473 (438) RDMSR, 544-547 (509-512) writeback/WRMSR; target PPR57896 rev3.00 pages87-99,117,186. Page offsets were verified individually, not inferred globally. Hashes, retained image paths and additional dependencies are in `work/native-after-cache-2026-09-14/efer-state-transitions-review.md`, `complete-efer-contract.md` and `independent-complete-efer-review.md`.

The batch includes full host suites, linked production/diagnostic audits, SMP emulator profiles and terminal fixtures; exact result files and package evidence govern acceptance. Model tests exercise all 32 combinations of the five expanded feature gates and instruction/fault/reset transactions. They do not prove physical execution of every Zen5 control. Compatibility/paged-legacy intercepted instruction handling, initial FFXSR capture, general debug/events/MMIO, Windows boot and Hyper-V/VBS coexistence remain bounded or unproven. No clock-ordering, VM-exit latency or protected-Windows measurements are claimed. No Windows protection or motherboard setting changes are part of this batch.
