# Native INIT/#SX wakeup — 2026-09-14

The explicit native guest-startup profile now uses physical INIT, redirected
to a private host #SX, for notifications between resident CPUs. Ordinary
maskable interrupts remain native. The profile no longer reserves F1, forces
SVR enabled, clears CR8 on entry, or intercepts ordinary TPR/EOI/source writes.
ICR readback remains owned because host notifications also use physical ICR.

This is implemented for the existing fixed-x2APIC, permanently assigned CPU
profile. It is not a completed Windows loader or actual-machine admission path.

## Execution contract

APM2 rev3.44 §§15.13.4,15.21.8/Table15-12,15.28 and15.30.1 govern the path.
Each destination installs its private IDT, sets VM_CR.R_INIT while preserving
unrelated bits, verifies readback, and only then publishes startup readiness.
The applicable PPR57896 rev3.00 p215 identifies per-thread R_INIT as writable.
No SVM lock or Windows protection is disabled. Directory ABI is now version5;
BridgeContext remains128bytes, with offset112 reserved and zero.

The sender validates the stopped ICR instruction and destination, publishes a
bounded mailbox command, unconditionally sends an edge INIT notification, and
completes the instruction. The target's INIT intercept produces VMEXIT(63h)
with INIT still pending. The saved VMCB/register frame is authoritative.
After saving guest auxiliary state, the host opens GIF with IF clear. R_INIT
consumes the held INIT and delivers fault-class #SX(error1). The private gate
executes CLGI first, verifies error1, increments its acknowledgment count,
drops exactly the error-code qword, and IRETQs without changing saved RIP or
guest GPRs. No host EOI is issued. A bounded loop drains arrivals; unhandled
host exceptions remain terminal.

The destination applies queued guest commands separately. Notifications can
coalesce and may arrive after a command has already been consumed. The queue,
not a one-notification-per-command assumption, determines startup work.
Running-target duplicate SIPI therefore provides a wake-only test. Guest INIT
still requires the admitted quiescent LAPIC state before any reset commit;
reset with a pending/in-service device interrupt is refused.

## Validation and evidence

The two-CPU fixture executes repeated real16→protected32→long64 guest startup.
Its separate IRQ workload programs guest vectorF1, holds it pending with IF=0
and TPR=F0h, then holds it in service inside its guest handler. INIT/#SX wakes
the host in both states; before/after snapshots must preserve native TPR/IRR/ISR.
The guest then issues EOI and returns through IRETQ. A further wake executes
with SVR software-disabled. A live XMM0 sentinel is retained across this sequence.

The level-source variant uses the disposable Q35 MC146818 RTC IRQ8 routed
through IOAPIC pin8 as level-triggered F1. It checks LAPIC TMR and IOAPIC
remote-IRR, keeps the source asserted across both wakeups, acknowledges the RTC
in the guest, and verifies remote-IRR clears only after guest EOI. This is actual
emulated device delivery, not a software model of an interrupt queue.

Final counts, executable hashes, failed attempts and exact traces are retained
under `work/native-apic-routing/summary.json`. The raw resident image receives
the existing complete linked no-FP/SIMD/xstate and undefined-symbol audit.

All ten final scenarios pass: 2/24/32-CPU repeated startup, edge and level
wakeup, held-interrupt reset refusal, ordinary single-CPU/SMP regressions,
and one-CPU/x2APIC-off admission refusal. The three multicore startup runs
execute110 AP restarts. Host tests pass367 core +282 DXE =649. The diagnostic
payload audit covers9,012 instructions; the production audit covers7,140,
with no prohibited extended-state instructions or undefined symbols. Production
is built/audited only. These counts do not include the separate2,059 backend
checks and do not imply Windows or physical execution.

The corrected QEMU10.1.0 backend is separately derived under
`work/qemu-init-sx`. INIT now has a target interrupt bit rather than the generic
RESET alias; selection respects GIF/SMM and higher-priority machine checks.
VM_CR, #SX error-code/contributory handling and INIT retention are implemented.
A full rebuild recompiles all CPU-layout users. Its 2,059 extracted-body checks
use stubs at CPU/IDT boundaries; the native fixture supplies executed VMEXIT,
private-IDT and IRETQ evidence. Source file hashes, not the surrounding git HEAD,
identify the materialized QEMU source.

## Remaining boundaries

- xAPIC/MMIO startup and APIC mode transitions remain unintegrated.
- Actual guest INIT still uses the old nonextended max-LVT5 quiescence check;
  the Ryzen target's extended APIC reset state needs separate implementation.
- Guest reset with pending device IRQs, external INIT ownership, CPU rebind,
  self/logical/broadcast startup destinations remain unsupported.
- #SX identifies redirected INIT, not its sender. External INIT coincident
  with a host notification cannot be distinguished by an error code.
- STGI also opens nonmaskable events; unowned host NMI/#MC paths remain terminal.
  SMM masks INIT, so no bounded physical wakeup latency is claimed. Repeated
  arrivals before the first handler CLGI are not exhaustively validated.
- Loader memory lifetime, normal Windows handoff, actual-platform admission,
  nonidentity runtime mapping and protected-Windows compatibility remain open.

No physical launch, reboot, firmware programming, Windows/ESP change, native
latency measurement or malware workload was performed. Emulator success does
not establish physical compatibility or analysis containment.

Pinned manuals and the independent review are recorded in
`work/native-apic-routing/init-sx-research.md`; Barevisor's source comparison is
informative, not the normative contract, in `barevisor-review.md` beside it.
