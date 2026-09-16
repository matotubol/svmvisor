# x2AVIC interrupt delivery review - 2026-09-16

## Result and scope

x2AVIC can own guest interrupt prioritization, IRR/ISR, fixed edge IPIs and
ordinary edge EOI. It does **not** by itself convert physical device interrupts
into guest interrupts or implement the LAPIC timer. Replacing native guest LAPIC
access therefore requires a real physical interrupt capture/acknowledgement
owner and a timer backend. Merely setting the AVIC VMCB bits loses that ownership.

The smallest proposed first-boot implementation keeps every vCPU permanently
pinned to its original physical CPU, preserves destination IDs, and forwards
captured physical interrupts to that CPU's AVIC backing page. It does not need
IOMMU virtual interrupt delivery. The bridge now has a bounded ledger and
assembly capture implementation; native execution remains unproven. Firmware,
hardware routing and protection settings were not changed in this review.

Admission still needs physical/guest APIC routing compatibility. Physical ID
equality is insufficient for logical destinations: x2APIC logical IDs are derived
from the 32-bit ID and do not use the xAPIC flat/cluster LDR/DFR contract.
The supplied native CPUID result has AVIC and x2AVIC but lacks CPUID.1:ECX[21].
This review does not establish that physical x2APIC or accelerated guest x2APIC
access can work under that firmware policy. Do not silently advertise that
combination as a proven configuration. See the separate hardware review.

## Verified manual evidence

Reviewed rendered complete page images from local AMD APM volume 2,
publication 24593 revision 3.44, March 2026, SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Applicability: architectural SVM/AVIC/x2APIC rules, conditional on the respective
processor feature bits. PDF page numbers below are one-based, not page indices.
Scratch renders are in `work/x2avic-delivery-review/`.

| Rule / reference chain | PDF pages | Printed pages |
| --- | --- | --- |
| 15.13/15.13.1: physical INTR interception leaves interrupt pending for host acceptance | 584 | 522 |
| 15.17 Table 15-10 including note: GIF gates INTR, INIT and NMI | 592 | 530 |
| 15.21.1-2: saved host IF gates physical interrupts with V_INTR_MASKING; physical TPR and virtual TPR are separate | 595 | 533 |
| 15.29.1 device-delivery boundary -> 15.29.6.2 device delivery -> doorbell | 626, 639-641 | 564, 577-579 |
| Complete 15.29.3.1 Table 15-22 and adjacent EOI/IPI explanations | 628-631 | 566-569 |
| 15.29.6.1 IPI algorithm -> 15.29.9.1 Tables 15-25/26/27 | 638-639, 642-643 | 576-577, 580-581 |
| 15.29.9.2 -> complete Tables 15-28/29: EOI vector in EXITINFO2[7:0] | 643-644 | 581-582 |
| 15.29.10: x2AVIC MSR accesses, higher-priority MSR/access checks, ICR layout | 644-645 | 582-583 |
| 16.4.1 timer behavior -> Figures 16-8/9/10/11 and complete divide Table 16-3 | 698-700 | 636-638 |
| 16.6.3 acceptance -> complete IRR/ISR/TMR Figures 16-23/24/25 -> EOI and optional SEOI/IER | 709-711, 714-716 | 647-649, 652-654 |
| x2APIC capability/mode transitions -> initialization/access rules -> complete register Table 16-6 -> ICR/LDR semantics | 716-724 | 654-662 |

The timer chapter delegates implementation-specific clock details to the PPR.
The local `57896-3.00_PPR.pdf`, revision 3.00, August 28 2024, SHA256
`643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`,
was visually reviewed at PDF/printed pages 1 and 55. It applies specifically to
Family 1Ah Model 44h revision B0; exact CPUID/stepping applicability must match
before using its implementation-specific claims. Section 2.1.11.2.1.13 describes
the physical counter/divider behavior; this proposal uses that counter directly,
not a guessed conversion from TSC ticks. Its referenced register definitions
were not exhaustively traced, so no new timer-frequency constant is verified here.

The device-delivery cross-reference to IOMMU publication 48882 is deliberately
outside the proposed non-IOMMU path. No IOMMU feature, table, current ownership,
or interrupt-remapping configuration is established by this review.

## Physical IRQ bridge

1. Keep host physical LAPIC ownership private. Enable physical INTR interception
   and V_INTR_MASKING. Enter with saved host IF=1 and a monitor-owned physical
   TPR that admits physical interrupts; guest IF/TPR are controlled by x2AVIC.
   Host GIF remains closed around Rust mutation and the world-switch window.
   This replaces `enable_native_startup_interrupts`' current IRQ passthrough.
2. On INTR exit, the interrupt is still pending, and exit information is not an
   accepted vector. Use a bounded assembly-only host acceptance window with the
   private IDT installed. Its vector-specific returning gates capture the actual
   accepted vector and physical TMR bit, then close acceptance before touching
   shared state. Do not reuse terminal gates or infer a vector from exit code.
   The exact STI/STGI/window instruction sequence needs its own assembly review;
   never wait indefinitely for a vector and never drain an unbounded IRQ stream.
3. For an accepted edge interrupt, atomically publish its virtual IRR bit and
   edge trigger state, then acknowledge the actual top physical ISR vector once.
   Preserve a physical/virtual pending ledger across stopped work. Guest
   edge EOI remains accelerated and must not acknowledge physical LAPIC again.
4. For an accepted level interrupt, publish virtual TMR+IRR and retain physical
   in-service ownership until that guest interrupt completes. AVIC_NOACCEL EOI
   supplies the vector in EXITINFO2[7:0]. Table 15-22 classifies this as a trap:
   hardware already performed the guest EOI action. Do not clear a second guest
   ISR bit, emulate the instruction again, or advance RIP again.
5. Keep physical-held and guest-completed bitmaps. At guest EOI, mark the matching
   held physical vector complete. With host interrupt acceptance closed, issue
   ordinary physical EOI only while the highest physical ISR vector is both
   held and complete. Lower completed vectors remain queued until higher held
   vectors complete. This prevents acknowledging the wrong source and requires
   no optional SEOI feature. Each scan/pop is bounded by the vector bitmap size.
6. Distinguish a physical spurious-vector delivery with no corresponding ISR bit:
   do not issue physical EOI for it. Preserve terminal behavior for unsupported
   nonmaskable events; an ordinary interrupt acceptance window does not make
   arbitrary NMI/SMI/INIT handling safe.
7. On reentry, AVIC evaluates the backing page automatically. For a running
   remote target, publish state atomically before its AVIC doorbell. For stopped
   targets, retain state for the next VMRUN. Never use the old V_IRQ injection
   interface: AVIC ignores V_IRQ/V_INTR_PRIO/V_INTR_VECTOR/V_IGN_TPR.

Why the physical completion ledger is necessary: the host can accept higher
level interrupt H while the guest still handles lower level L with IF=0. Guest
EOI(L) then occurs while physical ISR's highest bit is H. A direct ordinary
physical EOI would acknowledge H incorrectly. The ledger postpones physical
EOI(L) until H has completed. This adds source-rearming latency that must be
measured. Physical SEOI is an alternative only after its advertised availability
and enabled state are verified; its existence must not be assumed on this CPU.

The bridge must not overwrite pending bitmap words with non-atomic stores while
another CPU or AVIC hardware can update them. Mixed edge/level sources sharing
one vector need an explicit coalescing/trigger rule and tests; do not quietly
overwrite a held level source's TMR state with an unrelated edge source.
The implemented bounded ledger refuses level capture when that vector already
exists in guest IRR or ISR, and refuses another captured source at a held level
vector. This avoids associating the earlier virtual event's EOI with the newly
arrived physical source. Arbitrary mixed-trigger vector sharing is unsupported.
Accelerated IPIs sharing such vectors remain an admission/coverage restriction.

Physical lowest-priority device selection also consults physical priority state,
which is not equal to the guest's virtual PPR. Identity IDs and logical routing
do not establish identical destination arbitration. Fixed physical delivery is
the bounded bridge's straightforward routing case; broader destination behavior
requires routing ownership or explicit exact-platform qualification.

## Timer backend

For the pinned first-boot profile, dedicate each CPU's existing physical LAPIC
timer to its single guest timer. This avoids a new scheduler clock conversion or
reserved interrupt vector. The guest accesses only its virtual x2APIC interface;
the monitor validates and applies timer programming to the physical backend.

- AVIC timer LVT, divide and initial-count writes trap after backing-page writes.
  Apply supported semantics exactly once to the physical backend and preserve
  the guest-visible virtual register state.
- Current-count read is a faulting AVIC access. Read the owned physical counter,
  return the defined zero-extended value and complete that RDMSR once using the
  verified stopped instruction continuation. Writing current count is invalid.
- Use the guest timer vector for the physical timer. The ordinary IRQ capture
  path acknowledges its physical edge interrupt and sets virtual IRR. Guest
  masking and APIC software enable require explicit timer delivery behavior;
  changing one must not accidentally restart the counter.
- Initial count zero stops the counter. Preserve periodic reload and all eight
  documented divisors. Capture outstanding timer state at admission or require
  a quiescent handoff; do not discard a pending firmware/loader timer interrupt.
- TSC-deadline mode is not established by these AMD pages. Do not advertise it
  without a separate architectural and actual-CPU capability basis.

This is software ownership of a physical timer behind a virtual interface, not
guest physical LAPIC register passthrough. It still needs actual timer/HLT wakeup
tests; AVIC alone is not a timer implementation.

## INIT/SIPI, HLT and admission

INIT/SIPI and non-fixed IPIs require software handling. Retain one target-owned
startup mailbox/state machine. A completed AVIC ICR trap supplies a completed
guest instruction; source handling must publish the command once, not resend an
ICR to the real CPU as a guest operation. Physical INIT/#SX used privately to
wake a stopped host is a separate mechanism with its existing ownership rules.

Guest INIT preserves x2APIC mode (16.10) but resets the other defined APIC state.
Do not disable the physical host APIC or zero its ISR to impersonate guest INIT.
An active device/level completion ledger must be resolved under an explicit
policy before resetting guest backing state; until that policy exists, refuse
active interrupt reset with retained diagnostic evidence. AVIC target-not-running
exit can already follow IRR publication, so waking the target must not duplicate
the IPI. Invalid pointer/target exits require different handling from that case.

HLT must park the virtual CPU with a lost-wakeup-safe transition and reentry
check. AVIC physical-table IsRunning, pending IRR, physical host IRQ acceptance
and startup notifications must agree. Existing guest-direct HLT assumptions
cannot carry over without review.

Before committing all CPUs: verify IDs/routing and physical APIC access mode;
private IDT/gates/stacks; empty or explicitly imported physical ISR ownership;
retained AVIC pages/tables; physical timer ownership; and interrupt-remapping
state compatible with unchanged device destinations. Guest-programmed logical
IOAPIC/MSI delivery cannot be claimed correct under mismatched physical xAPIC
routing. Fixing that mismatch may require physical x2APIC firmware policy or
explicit routing translation, rather than merely an interrupt capture bridge.

## Implementation and validation boundaries

Reuse `host/resident/runtime.rs`, its assembly in DXE `native/resident/runtime.S`,
the descriptor owner and existing startup mailbox owner. Replace the native
guest LAPIC passthrough path. The actual private host LAPIC backend remains a
necessary resource owner; removing all host APIC accesses would prevent physical
IRQ acknowledgement and timer operation. Fixture software APIC code is not
evidence that the new physical owner already exists.

Required focused checks: edge capture/one physical EOI; level capture/no early
EOI; L/H ordering case above; completion bitmap preservation/refusal; AVIC EOI
trap versus current-count fault continuation; running/stopped target IPI race;
timer zero/divide/one-shot/periodic/current count; IF/TPR blocking; HLT wakeup;
INIT with held device interrupt; and physical routing admission. Assembly changes
need the resident audit. Exact new hardware image must separately demonstrate
device I/O, timer delivery, SMP startup and Windows boot.

No new native interrupt latency, exit frequency, source-rearming delay, loss rate
or Windows compatibility measurements were performed. Hyper-V/VBS coexistence,
untrusted workload containment and sandbox readiness remain unsupported or
unproven. No Windows protection should be changed to make this bridge appear to
work. A build or emulator pass cannot establish native x2AVIC execution.

### Implemented local checks

`svm/native_irq.rs` supplies the per-CPU physical-held/guest-completed ledger,
read-only capture preparation, explicit completion/acknowledgement commits and
physical x2APIC ISR/TMR/EOI primitives. Four focused unit tests pass, including
the lower-guest-EOI/higher-physical-ISR counterexample and state-preserving
refusals. `native/resident/irq.S` assembles successfully for
`x86_64-unknown-none`; all 224 returning gates have vector-specific capture,
window validation and saved-IF clearing, and preserve GPRs. Unexpected IRQ entry
outside the capture window chains to the existing terminal vector handler.
The helper is bounded and returns `u32::MAX` if no IRQ was accepted. It neither
calls Rust nor acknowledges a source. Runtime wiring, complete linked-image
audit and actual interrupt acceptance remain separate integration checks.

### Routing enforcement investigation

The bridge cannot enforce a fixed-delivery restriction by reading the initial
routes once. Source inspection found these concrete bypasses:

- `IdentityNestedPageTables` has one trapped 4 KiB page and one protected write
  range, not a retained set covering every IOAPIC, MSI-X table and IOMMU control.
- `handle_diagnostic_ecam` revokes diagnostic publication and restores writes
  for the full protected ECAM range; later guest MSI programming is native.
- `diagnostic_runtime::handle_io` forwards CF8-CFF configuration cycles after
  diagnostic revocation. It does not validate MSI capabilities or routing.
- MSI-X message addresses/data reside in device BAR memory, outside ECAM.
  Trapping PCI configuration alone does not intercept those writes.

The archive's `native-platform-inventory.json`,
`native-diagnostics-build-plan/preflash-os-inventory.json`, and
`native-returning-preflash/os-inventory.json` were inspected read-only. They
contain CPU/platform and OS PnP identity/status information, not MADT/IVRS,
IOAPIC redirection entries, PCI capability snapshots, MSI-X table locations or
IOMMU interrupt-remapping state. No actual route inventory was found in the
searched current native reports or archived native inventory files.

Exact missing evidence for a bounded machine-specific owner is: MADT IOAPIC
addresses and GSI ranges; complete native PCI function/configuration inventory,
MSI capability shape and MSI-X BAR/table/PBA locations; current IOAPIC routes and
enabled MSI/MSI-X entries; ECAM segment/aperture coverage; IVRS IOMMU/requester
topology and live interrupt-remapping enable/table ownership. Initial snapshots
would parameterize an owner, but do not replace continuing write interception.

A coherent bounded implementation can freeze the discovered device topology,
trap its IOAPIC selector/window, all PCI configuration paths and MSI-X table
pages, and reject BAR relocation/hotplug/SR-IOV or unsupported writes before
forwarding. It must serialize each route, respect mask/program/unmask sequences,
and validate an enabled route's delivery mode/destination/vector before it can
generate a physical interrupt. Guest accesses to IOMMU routing controls/tables
also need ownership or refusal. This needs additional NPT slots, retained route
metadata and a checked MMIO/configuration instruction handler; the current
diagnostic revocation handler cannot supply it. Adding an uncalled policy module
or an unchecked `fixed_only` admission flag would not close these bypasses.

IOMMU direct virtual interrupt delivery is not a smaller established alternative:
its capability, requester topology, DTE/IRTE ownership, table invalidation,
not-running notifications and coexistence with Windows are all unimplemented
and unmeasured here. No production fixed-only routing guarantee is established
by this batch. Do not claim that the bridge admits arbitrary native device
programming or that the missing owner can be replaced with documentation.
