# Checked MSR faults and bounded APIC modes

Subsequent work: [guest CR8 synchronization](cr8-synchronization.md) implements
bounded writes and records a newly exposed backend fault-ordering gap.

The opt-in APIC fixture now delivers #GP(0) for rejected MSR operations and
supports Disabled, xAPIC and x2APIC mode transitions at the fixed BSP base.
`FixtureApic` owns the existing `LocalApic`; there is no second IRR/ISR owner.
Rejected admission returns that controller unchanged. No host APIC MSR is used.

## Fault and mode contract

The MSR adapter classifies faults without changing RIP, GPRs or controller state.
Its caller queues #GP only after validating an actual stopped MSR exit, exact
RDMSR/WRMSR direction and bytes, canonical fault RIP, pending event state and
supported SVM controls. The existing event owner supplies EVENTINJ and refuses
pending V_IRQ or interrupted delivery. Queuing is not proof of delivery.
Successful emulation alone requires a valid sequential continuation; a fault at
the last canonical instruction address can still be delivered without skipping it.

The guest handler checks error code zero and saved RIP/CS/RSP/SS, repairs the
operand, and uses IRETQ to retry the same instruction. The next real MSR exit
must be at the original RIP with one handler invocation. The monitor then
emulates the repaired access and checks the final guest stop. A not-present #GP
gate exercises #NP during delivery; retained delivery evidence causes refusal.

| Current mode | Permitted next modes |
| --- | --- |
| Disabled | Disabled, xAPIC |
| xAPIC | Disabled, xAPIC, x2APIC |
| x2APIC | Disabled, x2APIC |

Illegal mode encoding, prohibited transitions and reserved APIC_BASE bits require
#GP(0). All x2APIC MSRs are gated outside x2APIC mode; reserved slots also fault.
Architectural registers outside the implemented subset remain explicit policy
stops. Base relocation and BSC changes likewise stop without inventing #GP.
Mode writes are refused while delivery is armed. Disabled mode blocks monitor
queueing/arming, while retaining the controller's pending state.

Disable/re-enable register retention is a **bounded fixture policy**. AMD's cited
text specifies retention on xAPIC-to-x2APIC enable, but does not settle every
register's AE-disable behavior. This is not hardware fidelity evidence. RESET,
INIT and xAPIC MMIO are not implemented. Default CPUID still withholds general
APIC/x2APIC capability, so these modes do not imply a complete guest APIC.

## Validation

The emulator runs ten rejected-access cases over sixteen rounds: EOI read,
reserved TPR write, nonzero EOI write, disabled/xAPIC access gating, two prohibited
transitions, illegal mode encoding, reserved MSR and reserved APIC_BASE bit.
That gives 160 actual #GP repair/IRETQ/retry sessions per completed fixture run.
Sixteen mode cycles each execute eleven MSR accesses. Existing sixteen TPR/EOI/
bitmap sessions remain, plus two terminal policy probes for relocation and CR8.
The shared evidence parser requires the fault, nested-refusal and mode markers.

All 180 core tests, 251 returning-DXE tests, the UEFI target check and 35
patched-backend emulator profiles passed. Stock normal execution preserves the
known backend gaps; its separate CR8 diagnostic reproduces the expected backend
assertion. Five missing-marker controls fail closed. Exact results are retained
in the external batch's state.json and validation logs. A fresh Astra/High agent
implemented modes and independently reviewed fault delivery. Root reviewed the
mode changes; review corrections retained admission ownership on errors and
separated fault-RIP validity from successful continuation validity. The app's
agent-thread limit prevented a second fresh reviewer; that limitation is recorded.

## Sources and remaining work

AMD APM volume 2 revision 3.44, sections 15.11, 15.20, 16.9–16.11, Figure 16-32,
Table 16-6 and Appendix B; retained PDF SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
AMD volume 3 revision 3.37, RDMSR p438, WRMSR pp511–512 and Table B-1;
PDF SHA256 `c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`.
The preceding [register report](x2apic-register-fixture.md) records its archive
retrieval after the official AMD endpoint failed. This batch reuses the pinned
local primary documents and unchanged [patched QEMU](qemu-svm-corrections.md).

Next: guest CR8 write synchronization, remaining APIC register/capability policy
and xAPIC MMIO, then timer scheduling/HLT wakeup. Timer MSRs, physical routing,
IOAPIC/MSI, NMI, SMP and protected Windows compatibility remain pending.
No physical image was activated. No physical latency/drift, undetectability,
Windows guest boot or malware containment claim follows from these TCG tests.
