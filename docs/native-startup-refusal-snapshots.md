# Startup refusal snapshots

This batch captures the rejected check before its evidence is lost. It does not
remove route ownership checks or promise that the current Windows boot will pass.
The previous physical record was kind13/code143 with ICR `0x000000100000c500`.
That old capture cannot retrospectively identify the source or the failed check.

## Source-side routing (terminal kind14)

The native ICR owner retains the exact predicate, full canonical ICR, immutable
source APIC ID, and (where applicable) the rejecting recipient's ID, destination
mode, successful guest-INIT count, and last mode-commit cause. Recipient evidence
is captured under the existing route lock before it is released. No diagnostic
reads remote hardware, advances guest RIP, publishes a refused command, or sends
an additional IPI. The source adapter clears stale evidence before each access.

Predicates: 1 destination form; 2 self destination; 3 destination unassigned;
4 recipient not ready; 5 recipient mode invalid; 6 broadcast; 7 foreign match;
8 duplicate match; 9 no match; 10 selected self; 11 mailbox mismatch;
12 queue busy; 13 route busy; 14 INIT vector. Normal route contention retains
the existing bounded retry behavior; exhaustion still uses its existing generic
retry-count record. Nonrouting MSR failures retain their existing diagnostics.

Terminal version1 metadata keeps stage0x83, slot5, context11, kind4 and version4.
For kind14 context bits3:0 are the predicate, bit4 selects x2APIC versus xAPIC,
bit5 selects wide fallback, and bits10:6 contain the recipient INIT count capped
at31 (31 means at least31). Payload bits31:0 hold the canonical ICR low DWORD;
bits39:32 destination ID; bits47:40 source ID; bits55:48 recipient ID;
bits58:56 mode (0 unknown,1 xAPIC8,2 extended4,3 extended8,4 x2APIC32);
bits60:59 cause (0 observed,1 guest control,2 guest INIT,3 guest promotion).
Bits63:61 are zero. Predicates4/5/6/7/8/10/12 have recipient evidence.
If any ID exceeds255, the full64-bit ICR is exported instead, and the decoder
explicitly marks identity/history omitted. No ID is silently truncated.

Mode history occupies unused space in the existing64-byte mailbox, offset40.
Its shared address, alignment, queue offsets, and terminal-control offset remain
unchanged. Mode/history commits and reads use the same existing route lock.

## Target-side startup (terminal kind15)

The record captures APIC reset preflight failures, pending-state failures and
startup-service stages. A failed Version check retains the checked value instead
of rereading the register for diagnostics. Unsupported CPU signatures are
distinguished from unsupported Version layouts. The outer dispatch preserves
the detailed stop instead of overwriting it with generic startup-service failure.

Context bits2:0 select reason: 1 mode,2 layout,3 register unavailable,
4 register state,5 CPU signature,6 pending state,7 service stage. Bits9:3
are register offset/16, pending-error subcode, or service stage. Bit10 is wide
fallback. Normal payload is observed DWORD then full32-bit target APIC ID.
Wide values retain all64 bits and explicitly omit identity. The triggering
VMEXIT and RIP are not exported in this format; the decoder does not invent them.

Service stages:1 INIT acknowledgment,2 route table,3 current mode,4 mode commit
preparation,5 target application,6 EFER reset,7 ICR reset,8 mailbox completion,
9 bounded wait exhaustion. Their value is1 for AwaitSipi,0 for Running.
Pending subcodes reuse the existing terminal pending-error mapping0..13.

Update, 2026-09-16 (the
[x2AVIC completion record](x2avic-completion-2026-09-16.md) has details):
- Retired stages: 3 (current mode) and 7 (ICR reset) are retired xAPIC-era
  values, and 14 was the interim guest-INIT refusal. All three remain
  decodable. They are not to be reused, and no `StartupStage` value emits
  them.
- New stages:
  - 10: guest INIT LAPIC preparation refused; nothing changed.
  - 11: guest INIT LAPIC commit failed (terminal).
  - 12: guest INIT CPU commit refused (terminal).
  - 15: an owner that arm always installs is missing.
- Stage 13 (cache replay) already existed; the decoder now accepts it.
- Stage 6 is now checked during guest INIT preparation.
- Values: stages 12, 13 and 15 carry the AwaitSipi flag. Stages 10 and 11 carry
  `init_error_code`, in which bits 31:28 give the source:
  - 1: backing-page identity; the error code is in bits 7:0.
  - 2: host IRQ bridge; `irq_error_code` is in bits 20:0.
- The value stays within 32 bits, so the export keeps the target APIC ID.

## Validation and limitations

Host routing tests cover all4^3 destination-mode combinations across six
inventory orders and three sources for IDs0/16/32. They assert exact refusal
evidence and that refused commands do not enter target queues. Saved evidence
survives a later mode change. Rust-generated wire vectors exercise the Python
decoder through the CRC-bearing512-bit frame and its actual DWORD rotation,
including count saturation, full-width fallback and legacy kind13 records.

The same batch includes the earlier offline reset-preflight correction:
architecturally writable TPR/ESR/timer values need not already equal reset values.
Reserved width bits, held IRR/ISR/TMR, enabled SVR and unsafe LVT state still
refuse. Evidence and reviewed original AMD manual pages are retained under
`work/native-offline-ownership-2026-09-15`. This does not fix an alias by changing
physical APIC410 or introducing inconsistent guest-only shadow state.

Executed fixtures and exact build/source hashes are retained separately under
`work/native-refusal-diagnostics-2026-09-15`. Emulator success is not Zen5 silicon
or Windows boot proof. No native exit-latency/timing baseline was measured;
diagnostics use bounded integer operations and no extra device reads. Full
inventory, all registers, guest RIP and multiple stop contexts cannot fit the
existing snapshot. No motherboard BIOS or Windows protection settings change.
