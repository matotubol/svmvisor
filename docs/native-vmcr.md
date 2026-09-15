# Native VM_CR and intercepted MSR coverage

The September14,2026 capture reached an intercepted VM_CR access on CPU0 after all CPUs activated. The flashed EFER batch no longer stopped at EFER in this attempt. The record identifies C0010114h but contains neither direction nor attempted value nor RIP; it does not identify a Windows boot stage. The new batch closes this missing VM_CR execution contract and corrects OSVW interception discovered in the broader audit.

## VM_CR virtual profile

Native CPUID already withholds SVM and SVM-Lock. VM_CR therefore reports a fixed virtual unavailable-SVM profile: SVMDIS1, LOCK0, R_INIT0, value0x10. This differs intentionally from the physical target's reset value0. APM2 section15.31 defines read-only SVMDIS when SVM-Lock is absent; the target PPR defines LOCK as read-only. Writes0/8/16/24 complete without changing the virtual value. No mutable per-CPU VM_CR state or physical host register access is necessary. INIT cannot enable virtual SVM or change the fixed profile; EFER's private hardware SVME remains separate from guest-visible SVME0.

VM_CR63:5 are explicitly MBZ in APM2 Figure15-27: write-one queues #GP(0). Guest RIP and input registers remain unchanged for fault retry. Target Reserved bits0/2 use the PPR write-as-read contract; nonzero attempts remain explicit refusal. R_INIT1 is also refused because guest INIT-to-#SX behavior is not implemented. Mixed MBZ/control writes fault before the unsupported-value check. The handler never changes the monitor's physical R_INIT, VM_CR lock, EFER or HSAVE state.

The existing typed MSR instruction owner validates exit0x7c and exact EXITINFO1 read/write0/1. Same-CPU NRIPS in admitted long64/CPL0 supplies the actual2..15byte boundary before instruction-memory fetching. Other supported profiles use the exact unprefixed byte fallback. Both share stopped-event checks with EFER; TF, pending/interrupted delivery, unsupported modes and malformed evidence remain explicit refusals. Success preserves unrelated GPRs, zero-extends EAX/EDX on read, and retires RF through the existing instruction owner. No TLB flush is requested for a fixed-register read or ignored write.

VM_CR refusal telemetry uses context kind12, with fixed C0010114h index, direction and detailed cause. Hardware next-RIP evidence is distinguished from byte-path evidence. The decoder retains older context meanings and rejects invalid encodings.

## OSVW and remaining MSR surface

The broader source/manual audit found that the C00101xx blanket intercept also blocked C0010140/141 OS-visible workaround registers even though native CPUID advertises OSVW. These two exact indices now execute directly on the native CPU, preserving firmware values and hardware read/write/fault behavior. This follows the existing trusted native boot policy; it does not establish containment against hostile privileged guest writes. No other C00101xx addresses were opened by that change.

SMM, IGNNE, HSAVE, SVM key, AVIC/SEV controls, TSC ratio and other currently unowned accesses retain explicit refusal. MSRPM-external addresses include real target SMCA registers as well as nonexistent indices, so blanket #GP injection is not valid. More per-register ownership is still needed if these are accessed. No host save address or doorbell is passed through merely to make boot progress.

## Evidence and limits

Original rendered pages were reviewed including complete tables and crossreferences: APM2 rev3.44 PDF562-563/580/645-648 (printed500-501/518/583-586), PPR57896 rev3.00 target Family1Ah Model44h B0 pages26-27/215-216, and APM3 rev3.37 PDF546-547 (printed511-512). OSVW capability/register and MSRPM references are recorded separately in the MSR surface review. Full document hashes, image paths, and applicability appear in work/native-after-efer-2026-09-14/vmcr-architecture-review.md and native-msr-surface-review.md.

Model tests cover all64 write bits, mixed fault priority, all valid hardware lengths, upper-register handling, malformed evidence, faults and transactional refusals. Executed VM_CR fixtures are required in the2CPU xAPIC,24CPU xAPIC and2CPU x2APIC validation profiles; exact results belong to the batch evidence, not this description. TCG lacks NRIPS, so its executed fixtures validate the byte fallback. Independent source and linked-image review and complete card readback are separate gates.

No physical Windows boot, Hyper-V/VBS coexistence, guest SKINIT/SMM, general debug completion or unrestricted MSR compatibility is established. No native timing distribution or protected-Windows measurement was performed. No firmware/Windows protection settings are changed. A successful next boot would still not certify malware containment.

## Executed fixture correction and final validation

The first candidate fixture placed its VM_CR #GP test after switching to a loader root that no longer mapped the old GDT. The trace showed correct #GP injection followed by #PF when the handler reloaded its CS descriptor through that unmapped GDT. This was a fixture mapping/order defect, not a VM_CR fault-injection failure. The fixture now runs before the virtual-map transition while its descriptor tables remain mapped; production hypervisor payload code was unchanged by this correction.

The final 2CPU xAPIC, 24CPU xAPIC and 2CPU x2APIC runs all passed their VM_CR read-only write/readback and #GP retry witnesses, alongside the repeated INIT/startup workload. The failed candidate evidence remains preserved. Final builds, tests and run artifacts use the final suffix; independent source and linked reviews bind those final manifests. Physical Windows boot remains unproven.
