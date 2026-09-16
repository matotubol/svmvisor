# Native failure capture - 2026-09-16

The compatibility-image snapshot reached Windows checkpoints on all 24 CPUs but
retained no first-fault bank, with 23/24 terminal acknowledgements. This change
addresses evidence loss; it does not identify or repair that boot failure.

## Implemented

- Each resident CPU image has an atomic first-fault latch, populated before the
  shared PCI lifetime lock is attempted. First writer wins; recursive capture
  returns immediately. Publication failure retains the payload for later retry.
- Subsequent diagnostic publication first flushes that pending payload. Terminal
  entry and host-fault IST independently make at most 65536 lock attempts. Neither
  depends on all-CPU acknowledgement. Every attempt retains route/UC validation
  and permanent endpoint revocation; no shared lock is stolen.
- The terminal owner alone uses banks beyond the admitted CPU count. On this
 24 CPU machine, banks 24-31 and their sticky counterparts 56-63 hold the exact
  software stop, raw stopped VMCB delivery/nRIP/CR 3, RAX/RCX/RDX/CR 0/EFER/CS/CPL,
  and the last five pre-dispatch exits (RIP, code, INFO 1/2, RCX, EDX:EAX).
  Ownership is established by terminal claim, before any kick/preflight/barrier
  failure. No real CPU bank is reused. The decoder labels these extension banks.
- History is a fixed five-entry CPU-local ring (240 bytes), with no guest reads or
  extra transport on ordinary exits. Spare-bank capacity decreases above 24 CPUs;
  at 32 CPUs only the normal first-fault and progress banks are exported.
- Barrier records expose the missing CPU mask and latch state/failure counter.
  States:0 empty,1 interrupted/incomplete writer,2 retained pending export,
 3 published. Failures count unsuccessful bounded flush operations/attempts as
  recorded by the owner, not elapsed time. The legacy aggregate still requires
  the complete barrier; rich bank records do not.

## Validation and limits

Focused terminal tests cover contention, a missing peer, first-fault immutability,
reentrant capture, and existing complete/partial transport commits. The resident
UEFI target check passed. Focused decoder tests cover extension ownership,
full-width fields and existing diagnostic frames. Production build/audit and
physical delivery are recorded separately in the current handoff.

This is software transport validation, not a measured native timing claim.
The ring adds bounded stores on each exit; failure-only extension exports add
bounded PCI traffic. Native overhead and actual next-boot evidence remain
unmeasured. No guest control, RIP, exception policy or Windows protection changes.

Reset, machine-check, permanent endpoint revocation, invalid route/cache type,
a fault while the same CPU holds the shared guard, or interruption during latch
construction can still prevent export. No unsafe PCI write or lock recovery is
attempted in those cases. Host-fault capture remains independent of STATE and
cannot manufacture a peer acknowledgement. Software VMCB fields are explicitly
raw observations; invalid-entry save fields are not claimed architecturally valid.
The firmware/card transport is not a general containment or sandbox guarantee.
