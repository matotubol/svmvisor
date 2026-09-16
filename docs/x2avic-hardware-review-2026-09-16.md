# x2AVIC hardware and manual review - 2026-09-16

## Verdict

The parent agent's live, read-only probe of the current Ryzen 9 9900X reports
`CPUID.8000000A: EAX=1 EBX=32768 ECX=0 EDX=FEBFBDFF`.
AVIC bit 13 and x2AVIC bit 18 are both set. This is affirmative CPU capability
evidence, not evidence of a working x2AVIC runtime or Windows boot. The probe
also reports `HypervisorPresent=False` and processor ID `178BFBFF00B40F40`.
Raw evidence is retained in `work/x2avic-2026-09-16/live-cpuid.json`; this was
an unpinned current-CPU observation, not an all-CPU survey. Physical MSRs and
IVRS/IOMMU capability registers were not read by that probe.

The same probe reports `CPUID.1.ECX=2128097803`, with x2APIC bit 21 clear.
That is a separate unresolved admission issue. Exact-product PPR 57896 p68
says this CPUID register can be overwritten by `CPUID_Features`; p240 exposes
its writable X2APIC control at `MSRC001_1004[53]`. Thus a firmware feature mask
is a plausible explanation, not an established cause. Do not treat the masked
CPUID value as proof that the processor lacks x2APIC, or force the physical
mode based only on the x2AVIC bit. Obtain per-CPU CPUID and the applicable
feature-control/APIC_BASE observations at firmware admission.

The PPR's p101 labels AVIC and X2AVIC as read-only with Reset 0, unlike nearby
Fixed 1 features. This prevented an initial unconditional capability claim;
it is not a valid denial in the face of the measured set bits. Actual CPUID
is the APM-prescribed capability detector.

The user requires an exclusive x2APIC+x2AVIC runtime. Hardware acceleration
does not remove timer, INIT/SIPI, level EOI, and device-interrupt ownership.
No legacy or physical-LAPIC passthrough fallback is recommended here.

## Sources, hashes, applicability and visual evidence

All hashes below were recomputed from the authoritative `docs` library.
Extracted text was used only to locate pages. The complete listed page images,
including relevant tables and continued tables, were visually inspected.
Images are retained under `work/x2avic-review-2026-09-16/`, except the already
rendered PPR p101 at `work/hwcr-verify-2026-09-16/57896-3.00_PPR.pdf.index-100.png`.

| Source | Revision / applicability | SHA256 |
| --- | --- | --- |
| `24593_3.44_APM_Vol2.pdf` | AMD64 APM2 3.44, March 2026; feature-dependent architectural rules | `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c` |
| `57896-3.00_PPR.pdf` | 3.00, August 28 2024; exact Family 1Ah Model 44h B0 | `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5` |
| `48882-3.11.pdf` | AMD IOMMU 3.11, April 2026; optional capabilities must be read on target | `f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22` |

| Reviewed rule / chain | Printed pages | PDF pages (one-based); indices (zero-based) |
| --- | --- | --- |
| APM 15.29.1-15.29.10, all AVIC prose/tables/figures | 563-583 | 625-645; 624-644 |
| APM 15.29.7 -> 3.3 CPU feature identification | 71 | 133; 132 |
| APM AVIC access tables -> x2APIC mode/access rules 16.8-16.12 | 654-659 | 716-721; 715-720 |
| APM APIC_BASE Figure16-2 | 630 | 692; 691 |
| APM INIT/#SX -> 15.21.8, complete Table15-12 | 535-536, 563, 583 | 597-598,625,645; 596-597,624,644 |
| APM AVIC exit names -> complete Appendix C Table C-1 | 756-758 | 818-820; 817-819 |
| PPR LAPIC capability, register access and mode enable | 51-52,68,101,121 | same PDF pages; 50-51,67,100,120 |
| PPR CPUID1 masking -> MSRC0011004, complete continued table | 240-241 | same PDF pages; 239-240 |
| APM device interrupt cross-reference -> IOMMU 2.2.8 and startup context | 119-121 | same PDF pages; 118-120 |
| IOMMU 2.2.8 -> 2.2.5.2-.3, complete Tables21-23, Figures16-20 | 92-98 | same PDF pages; 91-97 |

AMD's current public [APM 3.44 entry](https://docs.amd.com/v/u/en-US/24593_3.44_APM_Vol2)
was also checked. Local rendered documents above are the normative evidence.

## Concrete x2AVIC requirements

1. **Capability and mode are distinct.** APM15.29.7 p578 requires
   CPUID8000000A_EDX[13] for AVIC and [18] for x2AVIC. APM16.9 p654 separately
   uses CPUID1_ECX[21] for physical x2APIC. Guest CPUID, guest APIC_BASE and
   the accelerated MSR interface must be coherent from their first exposure.
2. **Entry controls.** Set AVIC enable and x2AVIC enable, VMCB0x60 bits31:30.
   AVIC requires nested paging. x2AVIC without AVIC causes INVALID entry.
   AVIC ignores V_IRQ, V_INTR_PRIO, V_IGN_TPR and V_INTR_VECTOR; an old
   V_IRQ injection path cannot remain authoritative. CR8 updates backing TPR
   and V_TPR even independently of V_INTR_MASKING (p570).
3. **Resident memory.** Each vCPU needs its own retained backing page. All
   vCPUs share a physical APIC ID table, indexed by guest physical APIC ID.
   VMCB0xE0 is backing page, 0xF8 is physical-table address plus maximum
   index in bits11:0. Pages must be 4 KiB aligned, in legal physical address
   ranges, mapped write-back. The backing page remains present for the VM's
   entire lifetime, including while its vCPU is not running (pp566,570-572).
4. **Table entries.** Physical-table entry bit63 is valid; bit62 IsRunning;
   bits61:52 reserved zero; bits51:12 backing-page address; bits11:0 host
   physical APIC ID. IsRunning means assigned to a physical core, including
   while that core handles a VM exit; it is not a guest-mode indicator.
   With measured CPUID8000000A_ECX=0, x2AVIC_EXT is absent: max guest index
   is 511 and the table occupies one page. Validate sparse IDs, not CPU count.
5. **No unused legacy structures.** V_APIC_BAR and logical APIC table are
   not used in x2AVIC. Logical IDs derive from the x2APIC ID using the formula
   on p574. x2AVIC MSR interception and architectural access checks take
   precedence over AVIC permission checks (p583); MSRPM must actually allow
   the accesses intended to accelerate.
6. **Concurrency.** Hardware modifies backing-page IRR/ISR/PPR and table
   lookup runs across CPUs. Software initialization/reset and IRR updates
   need explicit publication and race rules. Do not infer single-writer
   ownership from the vCPU's dedicated physical core.

## What the hardware handles, and required exits

APM Table15-22 spans pp566-568. TPR and CR8 synchronize with V_TPR; hardware
maintains PPR. Fixed edge-triggered ICR and self-IPIs can be accelerated.
x2APIC ICR is a single 64-bit MSR mapped to backing offsets0x300/0x310.
SELF_IPI acceleration uses the same mechanism. Non-fixed or level-triggered
IPIs require software handling. INIT and SIPI therefore still need the
existing target-owned startup semantics or a concrete replacement.

Timer LVT, initial count and divider writes trap after the backing write;
current-count reads fault before access. SVR and other LVT writes also trap.
x2AVIC is not a timer scheduler. A runtime must implement these operations,
their side effects and source delivery; simply clearing their intercepts
does not make them physically execute as a complete virtual timer.

An accelerated EOI clears the highest virtual ISR bit and reevaluates pending
virtual interrupts. A highest in-service level interrupt causes an exit so
the VMM can emulate level completion (p569). This does not document an EOI
of the physical LAPIC. Mixing physical direct interrupt delivery with virtual
EOI would leave physical acknowledgement/source ownership unresolved.

`AVIC_INCOMPLETE_IPI=0x401`, `AVIC_NOACCEL=0x402` (Table C-1 p758).
The incomplete-IPI exit carries ICRH:ICRL in EXITINFO1, reason in
EXITINFO2[63:32], table index in [11:0] for reasons1-3. Importantly, some
destinations' IRR bits may already have been set before this exit; indiscriminate
resending can duplicate delivery. Reasons include invalid type, target not
running, invalid target/backing pointer and invalid vector (Tables15-25-27).
NOACCEL EXITINFO1 bit32 selects write; bits11:4 identify backing offset.
For EOI, EXITINFO2[7:0] identifies the vector (Tables15-28-29).

Instruction completion must distinguish Table15-22 traps from faults. The
backing write has already completed for a trap. Do not advance or replay an
unknown exit using the old ordinary-MSR completion path. Invalid operands,
unimplemented x2APIC MSRs and reserved bits retain architectural #GP rules
(APM16.11 pp657-659). Unknown semantic cases must preserve stopped state and
retain diagnostics, not resume speculatively.

## Mode entry and INIT

Physical x2APIC enable follows APM16.9: from disabled, first enable APIC with
AE=1/EXTD=0, then enter x2APIC with AE=1/EXTD=1. Direct disabled-to-x2APIC is
not an allowed transition. Leaving x2APIC requires clearing both bits. On
entry, LDR and upper ICR are not preserved; most other registers are preserved.

APM16.10 p657 states INIT preserves APIC_BASE AE and EXTD, hence preserves
x2APIC mode, while resetting the other APIC registers. Thus guest INIT does
not intrinsically require a legacy xAPIC fallback. It does require a correct
reset image and exclusion from concurrent virtual interrupt delivery.
The complete per-register reset image and pending-IPI/reset race protocol are
not approved by this review; follow Chapter16/PPR individual registers when
implementing that owner.

Physical INIT notification still follows GIF, INIT intercept and VM_CR.R_INIT
(Table15-12). Intercepted INIT remains pending; redirected INIT becomes #SX
and is no longer pending. x2AVIC does not replace this physical-host mechanism.

An exclusive runtime still needs a coherent loader handoff: switching guest
APIC mode after Windows has selected an MMIO implementation cannot make that
loader issue x2APIC MSRs. Establish mode before the loader's choice, or prove
the captured continuation is already compatible. This review has not
established that handoff point on the physical image.

## Device interrupts and IOMMU boundary

APM p564 expressly delegates device interrupt acceleration to IOMMU support;
pp577-578 describe IRR updates and doorbell delivery. This is not a statement
that setting VMCB x2AVIC automatically redirects existing IOAPIC/MSI traffic.

The normative cross-reference, IOMMU3.11 Table21 pp92-94, requires compatible
GASup/GAMSup and GAEn/GAMEn programming. GASup alone is insufficient for the
virtualized guest-APIC mode in that table. Per-device 128-bit IRTEs select
GuestMode=1 and identify the backing page, vector, physical destination and
IsRun. The concrete IOMMU procedure atomically updates backing IRR and sends
the doorbell; software owns DTE/IRTE setup, publication and invalidation.
APM's high-level physical-table narrative must not substitute for these exact
IOMMU data structures. All IOMMUs must agree on IRTE format size (p92).

IOMMU x2APIC addressability is separately enumerated by XTSup; Section2.2.5.3
p97 requires XTEn and GAEn for its extended format. Destination upper bits
must be zero without the capability/enabled mode (Figure20 p98).

No target IOMMU GASup/GAMSup/XTSup, IVRS topology, DTE/IRTE ownership, Windows
IOMMU coexistence, IOAPIC/MSI delivery or interrupt remapping state was
measured here. A physical-to-virtual software interrupt bridge would need its
own verified capture/acknowledgement/level-source/EOI contract; none is
approved by this review. The full IOMMU programming chain is unresolved:
MMIO control and capability registers, DTEs, command invalidation, error logs,
SMI filtering, firmware setup and device/source routing. The reviewed pages
identify those dependencies; they do not approve enabling an IOMMU.

## Implementation and evidence limits

The initial review was read-only; the bounded core implementation below was
subsequently requested. No device was programmed and no new image was tested. The target
is the existing 24-thread Ryzen9 9900X / Gigabyte B850 AORUS ELITE WIFI7 F7.
The parent probe establishes present CPU feature bits only. Per-CPU admission,
an x2APIC-compatible loader continuation, timers and external IRQ ownership
remain concrete implementation gates. Keep one authoritative APIC owner and
remove superseded passthrough routing only as its actual replacement is wired.

No exit-frequency, latency, interrupt-delivery timing or clock-consistency
measurements were performed. Windows boot, Hyper-V/VBS/HVCI compatibility and
the exact flashed candidate's failure cause are not established by these
findings. Protected configurations must remain honestly classified; no
protection changes are authorized by this review. Successful boot, if later
achieved, will not establish sandbox containment or undetectability.

## Bounded core implementation (same batch)

Implemented `svm/x2avic.rs` and explicit `Vmcb` native-profile entry methods:
exact CPU feature admission, checked backing/table physical bindings, 4 KiB
atomic memory formats, unique one-to-one ID table population, assigned-core
publication, bounded ISR inspection, atomic IRR enqueue with mixed-trigger
refusal, six-standard-LVT reset initialization, preintercept software EOI/PPR
completion, and strict decoding of 0x401/0x402 exit information. The profile
requires NPT, physical INTR/INIT interception and V_INTR_MASKING, excludes
unused/alternative controls, and is compared to the exact expected page
bindings before entry. Generic synthetic IRQ APIs continue to reject AVIC.
Native #GP injection accepts only the strict encoded native profile and
preserves RIP. CPU INIT-state setup retains the x2AVIC enable bits.

The runtime, DXE allocation/publication and physical IRQ owners are being wired
by the other agents in this batch; these core primitives alone are not a
working interrupt controller. Every source must serialize trigger-mode reuse
for its vector. No method acknowledges a physical device or rings a doorbell.

Additional reset/EOI references visually checked: APM2 Table16-2 and adjacent
version descriptions pp631-634 (PDF693-696, indices692-695), and complete
TPR/PPR descriptions pp650-651 (PDF712-713, indices711-712). The software EOI
method is for an MSRPM preintercept only. An AVIC_NOACCEL EOI trap has already
performed its backing ISR action; its EXITINFO2 vector identifies the level
source requiring completion and must not cause a second virtual EOI.

Validation performed: `cargo test --locked -p svmvisor-hypervisor --test
x2avic` passed seven focused host tests, covering real missing-capability
refusal, page/ID bounds and aliasing, table encoding, trigger conflicts,
EOI priority and pending preservation, exit decoding, exact VMCB bindings,
generic profile refusal, stopped #GP injection and concurrent atomic IRR
updates to the same bitmap bank without lost bits. `cargo check --locked
-p svmvisor-hypervisor --target x86_64-unknown-uefi` passed. These are inert
format/state checks, not AVIC execution, physical timing or boot evidence.

### Active INIT remains a synchronization gate

`reset_stopped` requires every possible hardware/software producer quiesced;
clearing a physical-table valid bit and issuing a fence was not established
as an architectural drain of in-flight AVIC IRR updates. The safe initial
scope is initialization before table publication. A full guest-CPU rendezvous
may be part of an eventual active-INIT protocol, but the reviewed x2APIC
ordering section does not by itself prove that a fence after VMEXIT drains
all remote AVIC updates. Do not silently label this protocol verified.

Informative Linux cross-check pinned to
`9b87fdc9af2fbfcdb5c24a64139685ef80f6573f`, retained under
`work/x2avic-review-2026-09-16/linux-reference/`: `kvm_lapic_reset` at
`arch/x86/kvm/lapic.c:2966` resets IRR/ISR/TMR through register stores;
`avic_apicv_post_state_restore` at `arch/x86/kvm/svm/avic.c:888` updates DFR/LDR.
This source excerpt does not establish the whole KVM synchronization chain
and does not override the unresolved architectural requirement.

## INVLPGB / TLBSYNC capability clarification

The user's subsequent request concerns TLB shootdowns, not replacement of
general IPIs, scheduling, INIT/SIPI or cache/MTRR rendezvous. The second raw
probe is retained in `work/x2avic-2026-09-16/live-cpuid-80000008.json`:
EAX `00003030`, EBX `791EF257`, ECX `00005017`, EDX `00010000`.
CPUID80000008_EBX[3] is clear. Exact-product PPR p99 (PDF99/index98), visually
read as a complete table, calls INVLPGB Fixed 0. Thus this target currently
does not advertise INVLPGB or TLBSYNC; they must not be executed here.

The cross-reference APM2 5.5.3/6.6.2 pp159/182 (PDF221/244, indices220/243)
was followed to APM3 `24594_3.37_APM_Vol3.pdf`, rev3.37 July2025, SHA256
`c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`.
INVLPGB pp397-399 (PDF432-434/indices431-433) and TLBSYNC p495
(PDF530/index529), including exception tables, were visually reviewed.
Both specify #UD when that feature bit is zero. General semantics: INVLPGB
selects ASID/PCID/VA/global scope, broadcasts asynchronously, and TLBSYNC on
the initiating logical CPU waits for its prior broadcasts. Optional nested
translation invalidation requires CPUID80000008_EBX[21]. Guest use requires
separate VMCB enable; implicit guest-ASID replacement demands a global guest
ASID allocation if direct guest use is enabled. These features do not supply
cache/MTRR synchronization. The full enabled-feature chain (EFER.TCE,
guest-enable/intercepts and all mode-specific exceptions) was not approved
because the exact target fails capability admission; no code was added.

## Independent IOMMU x2APIC exception check

A bounded second review of `x2avic-iommu-owner-2026-09-16.md` independently
found no documented exception permitting XTEn=0 on a physical x2APIC system
merely because all host APIC IDs fit eight bits. This is not a claim that
such silicon behavior is impossible; it is not an admitted documented profile.

The same hashed IOMMU3.11 source above was visually rechecked: complete
IRTE descriptions pp92-98, complete Control register pp217-222, complete EFR
pp225-228, firmware precedence pp287-288/290 and reporting pp296-297.
PDF numbers equal printed pages, and indices are one less. Additional images
are in `work/x2avic-iommu-review-2026-09-16/` and this review's image directory.
Section2.2.5.3 p97 requires XTEn=1 and GAEn=1 for system x2APIC. Control bit50
is reserved when XTSup=0 (p218). EFR XTSup=0 states that x2APIC interrupts
are not supported (p228). Figure20's upper-destination-bits-zero rule is an
encoding constraint, not an exception to the system-mode requirement.
IVRS overrides corresponding hardware reports (p288); Type11 reporting is
preferred over Type10/MMIO (p290). The exact captured EFR image
`246577EFA2254AFA` has XTSup=0, hence cannot admit that proposed profile.
PPR57896 exact-name searches found only contents/index IOMMU mentions, not
an applicable IOMMU feature-register exception. APM16.8 backward compatibility
and the implementation-specific doorbell mechanism do not waive the IOMMU
rule. BIOS changes may alter reported capability; none was measured here.

## Native entry TPR consistency correction

The runtime's atomic `write_register_stopped` receiver changed to shared
`&self`, retaining exclusion of writers to the same register. This avoids
claiming a whole-page exclusive reference while remote producers may update
different atomic IRR words. It does not authorize live full-page reset.

The guest TPR captured into the backing page must also seed VMCB V_TPR with
`TPR >> 4` before enabling x2AVIC. Additional visual review: APM2 printed
pp503/507/527/533-534/740, PDF565/569/589/595-596/802, zero-based indices
564/568/588/594-595/801. Section15.21.2 says VMRUN loads V_TPR; AVIC p570
says CR8 reads return V_TPR. The AVIC ignored-field lists and TableB-1 do not
exclude V_TPR. Therefore a captured nonzero backing TPR with V_TPR=0 is
already an inconsistent guest CR8 view. These pages do not explicitly settle
whether VMRUN also overwrites backing TPR; that stronger claim is unnecessary
and is not made. The existing pre-enable TPR setter can seed the nibble,
which `enable_native_x2avic` preserves. No code was changed in this review.
