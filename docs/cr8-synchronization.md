# Guest CR8 write synchronization

Historical checkpoint: the subsequent [QEMU correction](qemu-cr8-fault-correction.md)
resolves the recorded backend gap on a separately built emulator.

The opt-in APIC fixture now emulates supported MOV-to-CR8 writes at real SVM
exit 18h. It updates the existing LocalApic TPR and VMCB V_TPR together and
clears the TPR subclass, even when the priority class does not change. The
existing controller remains the sole IRR/ISR/PPR owner. No host CR8 is written.

## Supported contract

The handler accepts exact four-byte REX prefixes 44h/45h/4Ch/4Dh, 0F22h and
register MODRM C0h-C7h. This covers sixteen 64-bit GPR sources. RAX and RSP come
from the stopped VMCB, with the other fourteen registers from the saved frame.
It does not infer decode-assist availability or use undefined EXITINFO fields.
Other instruction encodings remain policy stops, including valid encodings
outside this deliberately bounded decoder; they are not fabricated #UD faults.

Admission requires 64-bit guest code, CPL0, V_INTR_MASKING, coherent owned TPR,
supported classic controls and no pending/armed or interrupted delivery.
The full source operand must be 0..15. All fallible checks precede changes.
Success commits TPR=operand<<4, V_TPR=operand and checked next RIP, preserving
all GPRs, flags and pending/service bitmaps. The existing arm path then makes
newly eligible interrupts available. Generic control-register dispatch is
unchanged. The fixture continues to intercept every CR8 write.

AMD specifies that normal CR8 exceptions precede the CR8 intercept. Consequently
an invalid operand or CPL in an exit18 snapshot is inconsistent evidence and
is refused unchanged. A real #GP exit belongs to the existing exception owner;
no additional CR8 injection abstraction or second event owner was added.

## Execution and the newly exposed backend gap

Sixteen guest sessions execute two CR8 writes each: a same-class write clearing
TPR subclass, then priority lowering that releases the retained interrupt.
Guest CR8/MSR readback, ISR/PPR, EOI, handler count and IRETQ remain checked.
Sixteen additional sessions exercise each GPR source, including VMCB RAX/RSP,
with unchanged saved registers/flags and restored guest stack.

Thirty-two invalid-operand probes (bit4 and bit63, sixteen rounds) expose a
QEMU gap: it exits at18h before checking the reserved operand bits, instead of
raising #GP first. The handler refuses all 32 cases with complete VMCB/frame and
controller preservation. Evidence reports coverage=incomplete, zero CR8 fault
retries and 32 invalid-operand refusals. The architectural #GP/IRETQ retry branch
is retained for a conforming backend but was not executed successfully here.

The reviewer verified the explanation in the pinned QEMU source:
`decode-new.c.inc` schedules the intercept before writeback, while the CR8 branch
of `system/misc_helper.c` masks the source without a reserved-bit fault check.
Disabling interception would not establish the missing fault semantics. The
backend binary remains unchanged; no QEMU correction is claimed in this batch.
See the independent contract/review record in the archived evidence.

The corrected-backend runner still requires all previously established priority,
nested-delivery, guest-write and RDTSCP corrections. It admits only this exact
newly measured gap and retains incomplete coverage; any other GAP still fails.
Mandatory markers and contradictory-result controls prevent missing coverage
from being reported as a completed fixture.

## Verification and next work

Six new unit tests cover 512 successful operand/register/REX combinations,
960 reserved-bit refusals, decoder rejection, mode/CPL/masking checks, pending
state, ownership, continuation and full transactional preservation. All 186 core tests, 251 returning-DXE tests, the UEFI target check and 35 emulator
profiles passed their bounded contracts, retaining incomplete CR8 fault coverage.
Four missing-marker and one contradictory-marker controls passed. Results are
retained with source and artifact hashes in
`outputs/cr8-synchronization/state.json` and its validation logs.
Two fresh GPT-6 Astra/High agents implemented and independently reviewed this
batch. The review also tightened the harness's faulting-byte provenance and
whole-controller refusal assertions. The superseded terminal CR8 probe was removed.

Normative sources: pinned AMD APM vol.2 rev.3.44 sections 15.7.1, 15.21.2,
Table 15-7 and 16.6.4; vol.3 rev.3.37 MOV(CRn), pp426-427. Their exact hashes and
QEMU base/source references appear in contract-review.md. No missing normative
fact blocked this bounded implementation; the fault-execution limitation is an
observed backend behavior, not a substituted architectural assumption.

Next: correct and independently validate the QEMU CR8 operand-fault ordering,
then expand guest APIC register/capability/MMIO coverage before general admission.
RESET/INIT, physical interrupt routing, timers/HLT scheduling, IOAPIC/MSI, NMI/SMP
and Windows compatibility remain pending. Default general APIC capability is
still withheld. No physical image was activated. Physical latency/drift,
undetectability and malware containment remain unestablished.
