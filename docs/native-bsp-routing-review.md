# Post-EBS BSP routing and native INIT review

The physical BIOS-logo hang captured on 2026-09-14 reached successful
ExitBootServices and then stopped in our BSP startup preflight with failure42.
It did not reach the physical INIT/SIPI sends. Frozen capture analysis is in
`work/physical-hang-capture-2026-09-14/analysis.json`. The recorded image lacks
the failed predicate and register values; ExtApicIdEn-clear is a hypothesis,
not an observed hardware value.

The new batch is `work/native-bsp-routing-2026-09-14`. Its architecture report
links the code to the local AMD APM2 rev3.44 and PPR57896 rev3.00 for
Family1Ah Model44h B0. The old inventory-wide four-bit routing limit was
inappropriate for the all-excluding-self physical bootstrap and per-target
guest startup owner. BSP control remains unchanged; exact signature/version/
feature/reserved-control validation and complete CPU identity/base admission
remain. Runtime publishes actual per-target four/eight-bit matching. Ambiguous
guest startup requests still stop before command publication. Ordinary IPIs
continue to use the native hardware semantics.

New USER2 diagnostics preserve legacy stage0x80 and add versioned stage0x81
for a precise BSP routing refusal. The snapshot reports the failed predicate,
processor count and observed/expected DWORDs. It does not fit a complete CPU
inventory or all APIC registers. Full sampled BSP fields stay in retained local
storage; this is not a new post-loader transport. Invalid records remain raw.

The extended manual review found a separate later guest-INIT defect: APM2
Table14-1 resets DR0-3, DR6 and DR7 on INIT. The former code retained them.
The corrected target commit resets VMCB DR6/7 and clears guest-live DR0-3
through an audited CPU-local helper. Ordinary exits, private INIT/#SX wakeups
and duplicate SIPIs retain debug state; rejected preparation does not mutate it.
Extended state and separately retained MSRs are not indiscriminately reset.

CR0/CR3/CR4/EFER physical trampoline sequencing was checked against the manual,
including INIT-retained CD/NW, PAE/CR3/LME setup before PG, and the immediate
far transfer. This is a bounded source review, not physical execution proof.
The batch retains exact page images for tables whose merged cells matter.

An independent core review also reproduced rejection of legal guest paging
metadata: the strict host parser rejects upper software-available entry bits,
and the resident guest fetch path reused that policy. The correction belongs
in the guest adapter, with level/access/PKE-aware treatment of those bits;
host admission and actual reserved-address/NXE/large-page checks stay strict.
The architecture report records the relevant diagrams, protection-key rules,
and a baseline reproducer. This issue is after guest entry and cannot explain
the captured pre-INIT failure42. Its occurrence in a physical Windows mapping
has not been measured.

See the batch's final candidate and validation records for the precise tested
source and artifacts. Intermediate builds are explicitly marked superseded.
No image inherits the predecessor's physical readback or boot evidence.
Windows guest boot, native timing fidelity, protected-Windows compatibility,
Hyper-V/VBS coexistence and untrusted-workload containment remain unproven.
