# Native boot compatibility review — 2026-09-16

This is the second external-review batch. Implementation commit `d9d0750` is
pushed to main and flashed as candidate `3a704487861c4b0792c93d16cfcb3a11`, with
full readback verified. See [delivery evidence](handoff-evidence/2026-09-16-compatibility-flash/).
No activation or Windows boot was performed; Windows boot remains unproven.

## Findings and changes

| Finding | Result |
| --- | --- |
| CPUID compatibility/user mode and prefixes | Confirmed and repaired for the admitted NRIPS profile. The two-byte path accepts long64 and compatibility16/32 at CPL0–3 without fetching guest code. Prefixes require checked instruction bytes and bounded existing RAM translation. Legacy unpaged modes require startup ownership. |
| INIT requires an already quiescent LAPIC | Confirmed unresolved limitation. Architectural INIT does not require that state. Removing the check would not reset pending interrupts: IRR/ISR/TMR are read-only, and software disable preserves old pending/in-service state. Active-LAPIC INIT needs a separate correct reset mechanism. |
| Current HWCR30 acceptance disproves the original diagnosis | Incorrect before/after comparison: the parent of commit `b681ee5` refused every changed HWCR value. The current handler includes the fix. Historical fault operands remain unproven; static attribution is not runtime proof. |
| Other advertised HWCR controls lack completion | Partly repaired: HWCR35/CpuidFltEn now has an owner when same-CPU NRIPS and GpOnUserCpuid are available. CPL>0 CPUID then injects #GP(0) without advancing RIP. Changes to HWCR10/18/25/26 and other unowned controls remain unsupported. |
| x2APIC promotion makes every NPF terminal | Overstated. ECAM repair runs before LAPIC-window decoding. x2APIC accesses use MSRs; removing old LAPIC MMIO access does not itself require a fault. Unowned NPFs and stale LAPIC MMIO remain unsupported. |
| INIT/SIPI shorthand and logical destinations | All-excluding-self is now supported through the existing startup mailboxes. Every destination queue is preflighted before publication under the shared routing guard. Logical destinations and explicit self-targeting remain unsupported. Table16-4 does not permit self/all-including-self shorthand for INIT/SIPI. |
| TSC_RATIO intercepted without completion | Confirmed unsupported nested-SVM control. Guest CPUID hides SVM/TscRateMsr. The misleading ownership comment is corrected; no passthrough or invented #GP was added. No trace establishes Windows accessed it or that this caused a watchdog timeout. |
| CF9 and partial CF8 I/O intercepted without completion | Confirmed and repaired for scalar CPL0 long64 I/O. Original port and byte/word/dword width are forwarded for accesses overlapping CF8–CFF, including CF9. HWCR20 prepares guest #GP before physical I/O. String/REP, other modes and single-step remain unsupported. |

Interrupted-event recovery is **still not implemented**. The first review batch
added diagnostic refusal for `EXITINTINFO.V=1`, preventing a retry from silently
losing an event. That is stopped-state correctness, not reinjection or Windows
compatibility completion. No captured physical trace yet attributes the boot
failure to this path.

MSR continuation has not received a general mode/prefix expansion. EFER/VM_CR
and cache-owned MSRs have separate hardware-nRIP paths restricted to long64;
APIC and other remaining owners use the existing two-byte fetch. Unpaged
16/32-bit startup is supported, but compatibility-mode and prefixed MSR
continuations remain gaps. CPL>0 RDMSR/WRMSR requires architectural #GP rather
than the ordinary CPUID completion behavior. The CPUID fix does not imply this
separate instruction family is fixed.

## Ownership and architectural boundaries

CPUID uses the stopped hardware nRIP and same-CPU admitted capability. Invalid
nRIP never falls back to instruction guessing. Prefix validation accepts ordinary
segment/66/67 prefixes and a final long64 REX, with a maximum 15-byte span. LOCK,
REP and unsupported redundant REX forms remain stopped. Compatibility fetch
adds checked CS.base to the instruction offset and reuses the existing page
walker and qualified temporary physical mapping. No new unrestricted mapping
or parallel translation implementation was introduced. Pending-event conflicts,
TF, VM86, legacy paging, LA57 and offset wrap remain explicit limitations.

The HWCR owner retains fresh masked physical read-modify-write/readback and
preserves every unowned field. Bit35 is enabled only with the matching CPUID
fault handler. Its #GP injection uses the existing checked EVENTINJ owner;
fault completion preserves RIP/GPRs/flags and records pending ownership.
No counter isolation, power-control virtualization or timing guarantee is implied.

I/O remains physical platform I/O, not a new chipset model. IN preserves the
unwritten portions of RAX and zero-extends EAX for DWORD. Writes other than the
standard CF8 DWORD selector revoke diagnostic-card publication conservatively.
HWCR20 refusal precedes that revocation and any physical port operation. CF9
forwarding has host tests only; no physical reset was issued by this batch.

Broadcast startup publishes existing INIT/SIPI commands only to admitted peers.
It sends no guest physical INIT. Native producers and target completion share
the routing guard; generic standalone mailbox APIs are not independently atomic
with broadcast publication. An unavailable queue leaves every recipient and
the source instruction unchanged. The existing private wake occurs after unlock.

## Primary-manual evidence

Critical tables and adjacent explanations were read as rendered page images.
Extraction was used for locating pages only. Image names under
`work/boot-review-2-2026-09-16/pages` use zero-based PDF indices.

- AMD APM Volume2, publication24593 rev3.44, March2026, general AMD64/SVM:
  SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
- AMD PPR57896 rev3.00, August2024, Family1Ah Model44h B0:
  SHA256 `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.

Reference chains reviewed for this batch:

| Rule | Printed pages | Zero-based PDF indices / chain |
| --- | --- | --- |
| IOPM port-byte overlap and IOIO exception priority | APM516–517 | 577–578, full15.10.1/15.10.2 and EXITINFO fields → PPR HWCR20 pp203–204, indices202–203 → EVENTINJ APM532–533, indices593–594 |
| HWCR35 user CPUID fault and support indication | PPR203–204,117 | 202–203 complete HWCR table →116 Fn80000021EAX17; complete CPUID reference chain in the companion CPUID evidence |
| INIT and allowed ICR shorthand | APM643–645 | 704–706 full16.5/Table16-4 → reset Table16-2 p631/index692 → LAPIC configuration/priority context pp630–634/indices691–695 |
| Why software disable cannot implement arbitrary INIT | PPR57–60 | 56–59: SVR preserves IRR/ISR; complete read-only ISR/TMR/IRR tables and continuation; EOI is not unconditional reset |
| x2APIC transition | APM656 | 717,16.9.1/Figure16-32, retained state and MSR interface |
| TSC_RATIO scope | APM585–586 | 646–647,15.30.5/Figure15-29, feature indication and guest scaling semantics |

Full CPUID decoding, modes, exception priority, register-width, prefix and fetch
cross-references are retained in the [CPUID review](handoff-evidence/2026-09-16-compatibility/cpuid.md).
The [independent I/O/startup review](handoff-evidence/2026-09-16-compatibility/io-startup-review.md)
also follows HWCR20 through the PPR I/O-trap priority reference on p208/index207.
Prior bit30 review
remains historical evidence for that narrower allowance; this document records
the subsequent bit35 extension. Neither manual evidence nor modeled tests
establishes physical operands at the old stop.

## Validation status

All 565 integrated default hypervisor tests and the production build pass. The production
audit covers 24,981 linked instructions with no FP/SIMD/xstate instructions or
undefined symbols. The final two-CPU all-excluding-self fixture passes on the
predecessor pinned emulator. Focused tests exercise CPUID mode/prefix lengths,
fault preparation and unchanged refusals; I/O overlap/width/HWCR20 behavior;
HWCR masked ownership; and broadcast queue-full rollback.

The predecessor QEMU TCG backend lacks NRIPS. Its compatibility CPUID fixture
failure was retained as an emulator coverage gap. The separate
[NRIP-save backend](../tools/qemu-nrip/README.md) is now implemented and built,
SHA256 `317a5ad522613359a56df8cc30dec9928fa665dcf5461181b81f09b3baabae2c`.
Independent review and 3,738 source-extracted C assertions pass. Actual two-CPU
fixtures pass for compatibility32/long64 prefixed CPUID, prefixed VM_CR MSRs
with #GP retry, broadcast restart, cache/MSR continuation and a feature-off
baseline. The feature-off compatibility CPUID negative control stops as expected;
its ordinary positive-harness result remains false. CPL3 and general software
EVENTINJ behavior are not established by those real execution fixtures.

No physical boot, active-LAPIC reset, physical CF9 reset, native timing baseline,
Hyper-V/VBS coexistence or malware containment was measured. Synthetic tests
and emulator success do not transfer physical proof to a newly built image.
