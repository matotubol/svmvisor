# Native interrupted-event review - 2026-09-16

## Outcome

The blanket claim that every valid EXITINTINFO already stopped native execution
was false. The original runtime called `clear_event_injection_after_exit` only
when `State.pending_fault` was set. That path correctly refused interrupted
delivery but returned false without a dedicated stop record. A naturally raised
event did not set that software flag. The diagnostic ECAM NPF repair could
restore writes, request a TLB flush and resume without checking EXITINTINFO or
reinjecting the interrupted event.

This is a confirmed source-level missing recovery/refusal branch. An external
interrupt has already been acknowledged when IDT delivery is interrupted; a
plain reentry need not recreate it. A concrete architectural trigger would be
an IDT-delivery access or its guest page-table access faulting against the
write-protected ECAM aperture. That mapping is atypical, and **no actual Windows
or physical trace establishes this trigger**. It is not the diagnosed HWCR stop.

Root owns the implementation in `host/resident/runtime.rs`. The bounded fix is
an explicit global stopped-state check before loader ACK, startup service and
the exit handlers. It diagnoses an interrupted injected fault as `0xf10c`,
another interrupted event as `0xf10f`, and invalid entry/shutdown as `0xf110`.
It does not implement event replay. Unsupported delivery now remains stopped
instead of silently resuming without a recovery owner.

No vmcb/events module change was made. The existing
`Vmcb::resolve_exception_delivery_after_exit` remains the sole bounded exception
combination owner. It accepts interrupted type-3 #UD/#GP/#PF/#DF plus intercepted
#NP/#SS/#GP/#PF. The native exception bitmap is zero, so blindly calling it is
not a native repair: normal secondary exceptions are hardware-owned, and NPF
is not a guest #PF. No speculative event framework or new intercept policy was
introduced.

## Manual evidence

Source: `docs/24593_3.44_APM_Vol2.pdf`, AMD publication 24593, revision 3.44,
March 2026. SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Applicability: AMD64 classic SVM architectural contract; not processor-specific
PPR evidence or proof of behavior on the Ryzen 9 9900X.

Relevant pages were rendered with Poppler and inspected as complete page images
under `event-pages/`. Text extraction was used only to locate sections. The
following table explicitly separates one-based PDF page from zero-based index
and the printed page number.

| Section / table | PDF page | Index | Printed | Verified rule |
| --- | --- | --- | --- | --- |
| 15.7 / 15.7.1 | 570 | 569 | 508 | Interception precedes exception combination/delivery; EXITINTINFO identifies interrupted delivery. |
| 15.7.2 / Figure 15-1 | 571 | 570 | 509 | External event already acknowledged; complete EXITINTINFO bit layout. |
| Table 15-1 / 15.7.2 / start 15.7.3 | 572 | 571 | 510 | Types 0/2/3/4, EV-clear payload undefined, aggregate prior exception; repair/replay differs from reflection/combination. |
| Rest of 15.7.3 | 573 | 572 | 511 | EXITINTINFO records event before IDT delivery and clears V after successful delivery. |
| 15.20 / Figure 15-5 / Table 15-11 | 593-594 | 592-593 | 531-532 | EVENTINJ input format, injected-event semantics and invalid cases; NMI, software INT and ICEBP caveats. |
| 8.2.9 / Table 8-3 | 313-314 | 312-313 | 251-252 | Contributory/PF combinations, shutdown during #DF, #DF zero error code and undefined restart RIP. |
| Table 8-2 including both footnotes | 309 | 308 | 247 | Full exception classes, including merged cells and newer exception classes; reference back to #DF. |
| 15.25.6 and preceding walk explanation | 613-614 | 612-613 | 551-552 | NPF is a nested translation event; guest faults are separate. Guest page walks can fault on A/D writes. |
| 15.14.3 | 587 | 586 | 525 | Intercepted shutdown leaves saved VMCB state undefined. |
| Appendix B / Table B-1 | 799-801 | 798-800 | 737-739 | Exception and instruction intercept controls, full misc1/misc2 rows. |
| Appendix B / Table B-1 | 803-804 | 802-803 | 741-742 | EXITCODE/INFO1/INFO2/EXITINTINFO offsets, EVENTINJ and adjacent control layout. |
| 15.13.4 | 585 | 584 | 523 | INIT intercept leaves INIT pending until GIF enables taking/redirection. |
| 15.17 / Table 15-10 and footnote | 592 | 591 | 530 | GIF holding and redirected INIT/#SX relationship. |
| 15.21.8 / Table 15-12 and adjacent explanation | 597-598 | 596-597 | 535-536 | INIT next instruction boundary; guest intercept takes priority over redirection. |
| 15.28 | 625 | 624 | 563 | #SX is redirected INIT; vector 30/error 1 and contributory classification. |
| 15.30.1 / Figure 15-27 | 645 | 644 | 583 | VM_CR.R_INIT bit 1 converts non-intercepted INIT to #SX. |

Reference chains followed for the actual conclusions:

- 15.7.1 -> 15.7.2 -> complete Figure 15-1/Table 15-1 -> all of 15.7.3 ->
  15.20 -> complete Figure 15-5/Table 15-11. This establishes the event-loss
  problem and why blindly copying the raw field is not a universal policy.
- The 15.7.2 combination example -> 8.2.9/Table 8-3 -> Table 8-2 and footnotes.
  This verifies the need for exception combination, not every error-code rule
  of the existing resolver. The latter was not changed or claimed reverified.
- NPF trigger -> 15.25.6 and preceding nested-walk explanation. The subsection
  additionally refers to SNP/RMP/VMPL fields (15.36.10) and decode assistance
  (15.10). Those branches are not used for this refusal; no new SNP or decode
  support is asserted and their detailed rules were not verified in this review.
- INIT terminology -> 15.13.4 -> 15.17/Table 15-10 -> 15.21.8/Table 15-12 ->
  15.28 -> 15.30.1 R_INIT. Product-specific INIT reassertion references to PPR/
  BKDG are not exercised or reverified here. No INIT reset/reassertion behavior
  changed in this batch.

The reviewed APM text establishes EVENTINJ as the injection input and
EXITINTINFO as delivery evidence. This review makes **no claim that hardware
always clears or always preserves EVENTINJ.V**. The new refusal works with
either behavior and does not infer delivery from that input bit.

## Native paths and precise scope

- Monitor-queued #GP: `pending_fault=true`; successful non-interrupted exit
  retires the old request. Interrupted exit preserves the VMCB/request and
  receives a stop reason. Failed entry/shutdown never prove delivery.
- Hardware event: `pending_fault=false` does not imply no interrupted event.
  The new global guard covers this before ECAM write restoration or reentry.
- INIT notification: exit `0x63` is the guest INIT intercept, not a guest #SX
  exception exit. Host `acknowledge_init` enables GIF temporarily and consumes
  the pending INIT through the host redirected #SX gate. The guard follows
  this acknowledgment and precedes guest startup mutation. Synthetic
  EXITINTINFO+0x63 tests prove guard behavior only; this review does not prove
  that overlap occurs on hardware. INIT's exact interruption boundary during
  event delivery remains unestablished.
- SHUTDOWN: no saved RIP/event decode authorizes continuation. The explicit
  `0xf110` outcome records zero RIP/details rather than interpreting undefined
  state. The raw assembly record may still contain undefined raw values.
- Unknown SVM-family instructions remain unsupported/stopped. Baseline misc2
  `0x7f` intercepts VMRUN, VMMCALL, VMLOAD, VMSAVE, STGI, CLGI and SKINIT;
  misc1 bit 26 adds INVLPGA: eight instruction intercepts in total. SHUTDOWN
  is the separate misc1 bit 31. This inventory is not nested-SVM support.

## Independent implementation review

Reviewed root's runtime diff containing `INITIAL_STATE`, diagnostics
`0xf10a` through `0xf10f`, and the follow-up `0xf110` early terminal gate.
No implementation edits were made by this reviewer. Specific findings resolved:

1. Global valid-event guard must apply even without `pending_fault`.
2. Shutdown/invalid entry must be classified before interpreting EXITINTINFO;
   root added `0xf110` with zero RIP/detail.
3. INIT intercept and host #SX acknowledgment must not be conflated in text.

The guarded helper runs before ACK/startup/handlers; no guest GPR, RIP, RSP,
flags, CR2, EVENTINJ or NPT repair occurs on its refusal. Existing pending-fault
ownership remains set on refusal. Completed delivery still uses the existing
VMCB retirement owner. The outer dispatch diagnostic fallback executes after
RAII route guards unwind, preserves existing stop reasons, and does not turn a
terminal result into reentry. No blocking finding remains in the reviewed diff.

## Validation and limits

Root runs and records implementation tests/build evidence. Its new host tests
exercise interrupted injected delivery, naturally raised IRQ snapshots,
unchanged VMCB on refusal, invalid/shutdown poisoned state, and preservation of
existing terminal reasons. These are synthetic stopped-state checks, not
executed IDT delivery or physical IRQ proof. Historical emulator exception
fixtures cover a different bounded reflection path and do not establish native
recovery support.

This review ran no privileged guest execution, flash, activation or timing
measurement. Native exit-service latency, interrupt latency and frequency are
unmeasured. Windows boot remains unproven; Hyper-V/VBS/HVCI compatibility is
not gained by these changes. No protection setting was changed. Arbitrary event
replay, interrupted INTR/NMI/software INT recovery, nested-SVM and analysis
containment remain unsupported. A diagnostic stop records incomplete execution,
not successful exception recovery or sandbox readiness.
