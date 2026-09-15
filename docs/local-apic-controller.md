# Bounded local APIC controller and one-shot timer

The core now owns pending and in-service interrupt bits, priority selection,
non-specific EOI and a one-shot timer. A real guest fixture exercises timer
delivery, a same-class interrupt held behind the in-service request, EOI and
IRETQ continuation. This is an internal controller with a synthetic test
interface, not an implemented guest APIC MMIO or x2APIC register surface.

## Ownership and transitions

`svm::local_apic::LocalApic` owns IRR, ISR, full-byte TPR, timer state and one
non-copyable `PendingExternalInterrupt`. It uses the existing VMCB delivery
helpers; it does not add a second injection path. All storage and operations
are bounded, with no allocation, firmware calls or work proportional to elapsed
ticks. The existing harness initialization, bridge, IDT and dispatcher are reused.

- Select the highest pending vector whose priority class strictly exceeds PPR.
  PPR is full TPR when its class equals/exceeds the highest ISR class; otherwise
  it is the ISR class with zero subclass. PPR is never written into V_TPR.
- Keep IRR while delivery is Armed. Only observation after an actual exit with
  successful consumption commits IRR to ISR. Hardware still gates IF and shadow.
- EOI clears only the highest ISR vector. It neither clears IRR nor proves IRETQ.
  Another edge for an already pending vector coalesces; a new edge while that
  vector is in service can remain in IRR for delivery after EOI.
- Freeze scheduling and timer mutations while Armed. Reject mismatched V_TPR
  before arming or retirement. Guest CR8 changes need a future synchronization
  policy; this fixture excludes them. Errors preserve controller/VMCB state.
- Retain the existing vector policy of 32 through 255. This is a bounded fixture
  policy, not a claim that the architectural APIC excludes every lower vector.

The owner must establish exclusive stopped-CPU/VMCB access and actual entry/exit
before observation. Invalid entry, interrupted delivery and conflicting state
remain refusals; there is no general nested-event recovery.

## Timer and guest fixture

Writing a nonzero initial count starts a one-shot; zero cancels it. Supplied
decrement ticks reduce its count to zero and generate at most one edge.
Masking suppresses generation, not counting: masked expiration is lost, and
unmasking does not replay it. Mask/cancel does not erase an accepted IRR bit.
No physical frequency, divider or timestamp source is inferred.

Sixteen fixture sessions each deliver timer vector 0x50 and queued vector 0x51.
The guest handler uses an exact VMMCALL query location and marker as a synthetic
EOI adapter. The monitor observes delivery first, verifies the highest ISR
target and validates instruction continuation before acknowledging it. An
invalid marker preserves registers, VMCB and controller state. The first
handler's IRETQ permits the second delivery; extra entries prove no duplication.
Stack-frame checks and host CR8 restoration checks remain active.

## Verification

164 core host tests, the UEFI target check and 251 returning-DXE host tests pass.
The five controller tests cover priority ordering across bitmap words, nested
ISR/EOI, same-vector coalescing/redelivery, refusal preservation, timer masking,
exact/oversized expiry and cancellation. These unit tests synthesize stopped
VMCB fields and are distinct from execution evidence.

On the pinned corrected QEMU, three corrected AVX/SSE/FXSAVE profiles plus
32 broader ownership, host-fault, xstate, relocation and RDTSCP-disabled profiles
pass. The new two controller markers are mandatory for completed IRQ evidence;
removing either is rejected. A stock normal run also passes its bounded checks
with prior backend gaps retained; the separate stock CR8 diagnostic still
reproduces its expected assertion. See [the backend report](qemu-svm-corrections.md)
for the unchanged emulator binary/source provenance.

One fresh GPT-6 Astra/High agent supplied normative and independent review;
the app's agent-thread limit blocked a second fresh agent. The coordinating
agent implemented and tested the batch. Review corrected EOI target validation
before continuation was committed. Exact artifacts, source hashes, test logs
and independent review are retained in external `outputs/local-apic-controller`.

Normative reference: pinned AMD APM volume 2 revision 3.44, sections 16.3.1,
16.4.1, 16.6.3–4, 15.21.4 and the AVIC EOI description. PDF SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
The extracted nested-interrupt prose has IRR/ISR inconsistencies; the explicit
priority/retirement description in 16.6.4 governs this model.

## Next boundary

Guest APIC register access, capability/mode policy and guest TPR synchronization
are next. Real timer scheduling/HLT wakeup, periodic/deadline timers, divider and
frequency ownership, level interrupts, IOAPIC/MSI routing, NMI and SMP remain
unimplemented. Physical latency/drift and Windows/Hyper-V/VBS compatibility are
unmeasured. No physical image was activated, and this batch does not establish
Windows guest boot, complete malware containment or invisibility.

The [partial x2APIC register fixture](x2apic-register-fixture.md) now exercises actual guest
MSR accesses for TPR/PPR/EOI and interrupt bitmaps over that controller.
It uses a fixed admitted mode; fault injection and full mode coverage remain pending.
