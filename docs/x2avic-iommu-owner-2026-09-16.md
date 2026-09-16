# AMD IOMMU interrupt owner review - 2026-09-16

> **Status update, 2026-09-16:** this review is historical. By user decision,
> the IOMMU discovery and trap code was removed, together with the
> `native_mmio` and `native_sources` owners described below.
> `check_iommu.py` remains as a read-only tool. Windows keeps the physical
> IOMMU, and GA posting is deferred. See the
> [x2AVIC completion record](x2avic-completion-2026-09-16.md).

## Current result

The exact-machine IVRS advertises guest virtual APIC support, but does not
advertise IOMMU x2APIC support. This is firmware capability evidence, not a
live IOMMU state observation. No IOMMU register was written or mapped by this
review. No production IOMMU owner has been installed.

`tools/native-resident/check_iommu.py` now decodes an actual saved IVRS, checks
its checksum/length/bounded entries, retains requester aliases and special
device mappings, and evaluates the system-x2APIC/direct-AVIC capability gates.
Its actual caller is the command below; nine focused tests pass. It is a
read-only host evidence tool, not an uncalled runtime abstraction.

```powershell
python -m unittest discover -s tools/native-resident -p test_check_iommu.py
python tools/native-resident/check_iommu.py work/x2avic-2026-09-16/acpi/IVRS.bin --output work/x2avic-2026-09-16/acpi/ivrs-admission.json
```

The real-table invocation returns exit status 1 because XTSup is clear. Exit 0
means only that the required capabilities are advertised; it never means
configuration, hardware ownership or Windows boot has been established.

## Source and visual review provenance

Primary local source: `docs/48882-3.11.pdf`, AMD IOMMU specification rev 3.11,
April 2026, SHA256
`f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22`.
It applies subject to actual optional capability bits and firmware overrides.
All IOMMU pages below use identical printed and one-based PDF page numbers;
zero-based PDF indices are one less. Complete page images and continued tables
were inspected under `work/x2avic-iommu-review-2026-09-16/pNNN.png`.

| Rule and followed reference chain | Printed/PDF pages |
| --- | --- |
| Device interrupt mechanism, table selection and formats: 2.2.8 -> 2.2.5.2/.3; Tables 21-23, Figures 16-20 | 92-98,119-121 |
| DTE independence, all Table 7 fields, Tables 5/6/8/9/10, DTE publication changes | 60-77 |
| Incoming MSI indexing and special address controls: Table 3 -> Table 19 | 50-51,89-90 |
| IRTE update -> invalidation commands -> completion -> ordering: 2.3.2,2.4.1/.2/.5/.11 | 121-126,130-131,138-139 |
| Guest not-running notification and complete GA log state machine: 2.7.1-.4, Table 76 | 186-191 |
| Control bits and full EFR table, including continued rows | 218-221,225-228 |
| Firmware precedence and all IVRS/IVHD/IVMD entry formats and footnotes: chapter 5, Tables 85-113 | 287-309 |
| vIOMMU EOI bus-cycle restriction; this is not automatically applicable to ordinary GA mode | 199 |

Additional CPU reference: `24593_3.44_APM_Vol2.pdf`, AMD64 APM2 3.44,
March 2026, SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Reviewed rendered PDF pages 640-641 (printed 578-579, zero-based 639-640)
for device/doorbell cross-reference, and PDF 716-721 (printed 654-659,
indices 715-720) for x2APIC compatibility, enable and register-access rules.
The broader CPU chain is in `x2avic-hardware-review-2026-09-16.md`.
Text search of exact-product PPR 57896 rev3.00 found no IOMMU capability
register chapter establishing an exception to the IOMMU rule below.

AMD's [public IOMMU specification](https://docs.amd.com/api/khub/documents/GD6kOXjzWsek8QUbn_qMvg/content)
also contains the same x2APIC enable requirement. The local rendered revision
above is the normative source used here.

## Exact captured platform

Source: Windows `GetSystemFirmwareTable`, read-only capture reported by the
parent agent, `work/x2avic-2026-09-16/acpi/manifest.json`.
IVRS is 484 bytes, revision 2, valid checksum; SHA256
`5e652ea80ecfe5083fbc63f9e882cc69825357f085b7b8972e682dd0239d892b`.
Board: Gigabyte B850 AORUS ELITE WIFI7. CPU: Ryzen 9 9900X, as separately
observed by the parent CPU inventory.

One IOMMU is described in three formats, not three hardware IOMMUs:

| Property | Captured value |
| --- | --- |
| PCI segment/device/capability | segment 0, DeviceID 0002h (00:00.2), capability 0040h |
| MMIO base | F7600000h |
| IVHD offsets/types/lengths | 48/10h/68, 116/11h/84, 296/40h/188 |
| Type11/40 EFR image | 246577EFA2254AFAh, identical |
| EFR2 image | 0 |
| GASup/GAMSup/XTSup | 1 / 001b / 0 |
| IVinfo | 00203043h; EFRSup=1, DMA remap support=1 |

Chapter 5 pp288/290 makes IVRS information authoritative over corresponding
hardware feature reports and recommends Type11 feature information over
Type10/MMIO. A later live MMIO read showing a different XTSup cannot silently
override the firmware contract.

Special entries identify IOAPIC ID20h with requester00A0h, IOAPIC ID21h with
requester0001h, and HPET0 with requester00A0h. IOAPIC20h DTE settings are D7h;
these include firmware-selected special-interrupt pass controls and must not
be replaced by uniform zeros. Alias range FF00h-FFFFh uses requester00A5h.
Type40 additionally identifies AMDI0020 UID ID00-ID03 with source00A5h.
The ordinary range0003h-FFFEh overlaps that alias range in the firmware bytes;
the decoder preserves this fact and does not claim that every resulting DTE
assignment has already been resolved.

Three type22 IVMDs require exclusion for DeviceIDs0000h-0FFFh: 947A5000h/1000h,
947A4000h/1000h and 8DA83000h/1000h. ExclusionRange=1 means IR/IW/Unity are
ignored (Table113); these entries are not read/write-denied RAM claims.

## x2APIC capability decision and unresolved alternative

For the system-x2APIC IOMMU profile, require GASup=1, GAMSup=001b and XTSup=1.
All IOMMUs must use the same IRTE size and GA/GAM configuration. Enable
GAEn (Control17), GAMEn=001b (27:25) and XTEn (50) only after prerequisites
are established. Extended IOMMU-generated interrupt routing additionally uses
IntCapXTEn (51) and the corresponding interrupt-control registers.

IOMMU section2.2.5.3 p97 explicitly requires XTEn=1 and GAEn=1 when the
system has x2APIC enabled. The proposed profile with physical host x2APIC,
host APIC IDs below256, 128-bit GA IRTEs and XTEn=0 therefore remains
**unverified against an explicit normative requirement**. Figure18 proves
that an eight-bit GA destination representation exists; it does not itself
waive section2.2.5.3. APM16.8's general backward-compatibility statement does
not mention such an exception. The CPU doorbell transport is explicitly
implementation-specific in APM15.29.8.2. Do not label this profile impossible
silicon behavior, but do not activate it as a verified supported configuration.
Changing the BIOS x2APIC setting may change both CPU/IVRS reporting; that is
an unmeasured possibility, not an established repair.

This question concerns physical host routing. The user requires exclusively
guest x2APIC/x2AVIC, without a guest xAPIC or native-LAPIC passthrough fallback.
Eight-bit host encoding would not inherently violate that user requirement if
its architectural validity were independently established.

## Concrete owner contract for implementation

1. **Discover and reserve before guest enumeration.** Resolve all IVHD source
   IDs/aliases/special devices, PCI configuration paths, MSI/MSI-X capability
   and table locations, IOAPIC MMIO and MMIO widths. Keep original firmware
   tables for host admission. Publish only capabilities that an implemented
   guest IOMMU interface can actually honor. Merely hiding IVRS and retaining
   guest access to physical IOMMU controls is insufficient.
2. **Preserve existing DMA requirements.** DTE address-translation and
   interrupt-translation halves are independent (2.2.2.1). Mode000 permits
   identity DMA with IR/IW permission checks (p73); interrupt ownership need
   not automatically imply a new DMA containment project. However this
   platform advertises preboot DMA remapping and IVMD requirements. Do not
   destroy active DMA mappings/protection or clear that advertised contract
   without an explicitly reviewed lifecycle solution. Active tables/current
   controls and firmware handoff lifetime remain unobserved.
3. **Own DTEs/IRTEs and publication.** DTE IV=1, IntCtl=10b selects remapping;
   neither V nor IV may be confused with the other. All eligible requesters
   need retained DTEs; per-source remap tables require correctly aligned
   storage and bounded indices. New IRTE content is prepared while RemapEn=0
   and RemapEn is published last. Updating an active IRTE is atomic and any
   cached-field change requires INVALIDATE_INTERRUPT_TABLE. GuestMode1 IRTE
   contains the backing SPA directly, not an index into the CPU physical ID
   table. IsRun/Destination/GALogIntr/GAPPIDis/GATag are uncached (p93).
4. **Own commands and completion.** A retained 16-byte-entry command ring
   publishes contents before advancing tail. DTE changes use opcode2;
   interrupt table changes use opcode5; completion uses opcode1 and an owned
   coherent completion word. Polling has a fixed budget and reports failure.
   Reading a command head advance alone does not prove execution completion;
   commands may execute concurrently. Follow ordering2.4.11 and event/error
   handling. Event/GA log overflow must be observable and cannot be treated as
   successful routing.
5. **Translate source programming continuously.** Trap CF8-CFF, every admitted
   ECAM path, IOAPIC selector/window and MSI-X table pages. Serialize each
   mask/program/unmask sequence; validate vector, destination, trigger and
   delivery type before publishing an enabled source. Retain guest-visible
   register values separately from host requester/index programming. Reject
   unowned BAR relocation, alternate config paths, hotplug and SR-IOV before
   they bypass the source inventory. Unchecked MMIO instruction skipping is
   not a route owner.
6. **Direct delivery is single-target.** GuestMode1 IRTE has one backing SPA
   and doorbell destination; it has no DM/IntType arbitration fields. It does
   not implement dynamic lowest-virtual-PPR arbitration over arbitrary guest
   logical target sets. Multi-target/lowest-priority programming needs a
   separately reviewed software selection path or an explicitly justified
   single-target contract. Identity IDs do not solve this problem.
7. **Stopped CPUs and reset.** IsRun=0 still permits IOMMU IRR writes. GA logs
   identify requester and GATag, with retained IRR providing pending state.
   GA logging and notification must implement lost-wakeup-safe scheduling and
   bounded overflow recovery. Clearing the CPU AVIC table valid bit or using
   a CPU memory fence does not stop an IOMMU which holds a direct backing SPA.
   The reviewed completion rules cover translation-dependent fabric traffic;
   they do not by themselves prove a complete drain of outstanding GA atomic
   writes/doorbells. INIT backing reset must remain refused until sources and
   all writers are demonstrably quiescent.
8. **Level EOI remains separate.** GuestMode1 IRTE has no RqEoi or trigger
   field. A direct virtual device event does not imply a physical LAPIC ISR
   entry. Do not apply the physical-held interrupt completion ledger to it.
   The IOAPIC remote-IRR/EOI reverse mapping and virtual TMR setup need their
   own complete chipset/IOAPIC normative chain. Section2.10.5's explicit
   directed-IOAPIC EOI rule is scoped to vIOMMU mode, which has not been
   enabled here; it is not a universal GA proof.

The smallest coherent production replacement must close all relevant owners
above before admission. A `configured=true` flag, a standalone IRTE encoder,
or a one-time source snapshot cannot replace the live hardware and guest
programming contracts.

## Evidence limits and remaining dependencies

### Minimum machine evidence for a DMA-preserving takeover

Collect this in the DXE preparation and final post-firmware admission phases,
with exact image/platform/time provenance. A Windows snapshot after the OS
has configured its IOMMU cannot substitute for the firmware/loader state the
monitor will actually inherit. No production code should assume that a base
address observed before ExitBootServices still identifies the final tables.

| Minimum observation | Why implementation needs it |
| --- | --- |
| Per-CPU final CPUID x2APIC/AVIC/x2AVIC, APIC_BASE and actual physical IDs; fresh authoritative IVRS after any BIOS change | Determines whether the selected exclusive profile is admissible; BIOS changes may alter both CPU masks and IVRS. |
| Every IVRS-described IOMMU's PCI capability/header/base/enable, live Control0018, EFR0030, EFR2 at01A0 and status2020 | Distinguishes implemented features from active DMA, guest translation, SNP, segmentation, logging, exclusion and command operation; validates that the MMIO aperture really belongs to the discovered IOMMU. |
| DTE base0000, every enabled segmented DTE root if applicable, exact bounded DTE bytes for governed requesters and aliases | Establishes the inherited V/TV/Mode/IR/IW/DomainID/GV/IV/IntCtl/pass-bit policy. Copying only interrupt bits must preserve the actual translation and firmware-special policy. |
| Complete reachable translation tables for active DTEs, plus their memory-map allocation type, lifetime and current writer | Keeping a pointer into reclaimable firmware/loader memory is unsafe. Cloning requires walking validated bounded tables and retaining equivalent mappings. Active guest translation/PASID/ATS would add explicitly unsupported owners. |
| Exclusion base0020/limit0028 and all active address-routing features, together with IVMD ranges | Preserves firmware-required access/exclusion behavior and reveals routes which bypass ordinary page translation. |
| Command base0008/head2000/tail2008, event base0010/head2010/tail2018, enabled GA base00E0/tail-address00E8/head2040/tail2048, and pending status/errors | Establishes whether firmware/loader work is outstanding and who owns completion/log memory. Replacing an active command ring without draining it can lose required mapping changes. |
| Live DTE interrupt roots and enabled IRTEs, or proof that remapping is inactive, plus current IOAPIC routes and enabled MSI/MSI-X messages | Determines which interrupts already exist during takeover and how to transfer them without silent loss or wrong-source EOI. |
| Complete current PCI segment/config-path inventory, BARs, MSI/MSI-X shape/table/PBA bounds and IOAPIC version/redirection count | Makes ongoing guest source programming interceptable. Existing MADT/MCFG/IVRS give addresses and requester metadata but do not contain device register state. |
| Proof that inherited DMA-table/config writers have stopped, with a final revalidation before ownership commits | A coherent snapshot does not exclude later firmware or loader changes. Sources must be masked/quiesced under a reviewed transaction before switching table/routing ownership. |

The first two rows can decide admission before any takeover write. Subsequent
rows are needed only for an otherwise admissible platform. Discovery must be
read-only and refuse unsupported active modes; it must not enable hidden
features or stop devices merely to collect a report.

### Work possible now, and decisions that remain dependent

Possible now: the implemented IVRS decoder, a bounded firmware observation
record and validator integrated with the existing admission owner, retained
allocation planning, correct serialization of already-reviewed DTE/IRTE/
command formats with a real consumer, and source programming decode/validation
once the concrete PCI/IOAPIC inventory is supplied. None of these requires a
claim that hardware ownership is complete. The existing trap owner can be
refactored to support a retained list of resource pages without inventing
an `iommu_configured` flag or fake success state.

Dependent on live state: preserve versus clone each active translation tree;
the permissible command-ring drain/switch transaction; whether DMA is already
identity mapped; whether unsupported ATS/PASID/guest/SNP modes are active;
which source entries need transfer; and whether firmware/loader memory can
become guest allocatable. A plan to clear DTE.V or force Mode000 is a policy
change, not a preservation operation.

There is also an architectural decision that **no snapshot alone resolves**:
how Windows continues managing DMA after takeover. Preserving the initial
firmware mappings is insufficient when Windows subsequently allocates new DMA
buffers or changes permissions. Either a supported guest IOMMU interface must
translate Windows operations into the monitor-owned tables, or another
explicitly reviewed owner must supply the required ongoing mapping semantics.
Suppressing IVRS/its DMA-protection flag or replacing protected mappings with
unrestricted identity DMA does not satisfy DMA-policy preservation. That
lifecycle contract remains code/design work even after all observations exist.

No Windows boot, direct device delivery, interrupt latency, command completion
latency, exit frequency, source loss, source-rearming delay or GA overflow
behavior was measured. The decoder's tests prove file-format/refusal behavior
only. Exact-machine live Control/EFR/status, DTE roots, command/event/GA roots,
PCI capabilities, active MSI/MSI-X entries and IOAPIC redirection state remain
uncaptured. MMIO discovery should occur through the authorized firmware
admission path, not an arbitrary Windows physical-memory driver.

SMI filter2.1.5 and hardware error/event2.5 register chains are not fully
reviewed here. The IOAPIC specification, platform-specific interrupt fabric
and full device-source reprogramming chains remain unresolved. SNP/SEV-TIO,
guest vIOMMU, ATS/PASID, Hyper-V/VBS coexistence and arbitrary device hotplug
are unsupported by this owner proposal. No Windows protection was changed.
This trusted first-boot work establishes neither malware containment nor
sandbox readiness.


## Source ownership implementation follow-up

`svm/native_mmio.rs` now owns the bounded long64 DWORD MOV adapter formerly
embedded in the native xAPIC path. The fixture xAPIC owner remains separate.
The new native entry point validates the exact installed x2AVIC page bindings,
accepts DWORD-aligned device offsets, and uses a generic transactional device
callback. It verifies mode, guest instruction/data translation, UC composition,
NPF provenance, event conflicts and continuation before calling that owner.
The callback must complete all of its own validation before any hardware write;
a later callback error cannot undo a device write. Unsupported instruction
forms remain explicit stopped-state refusals. This is not a general MMIO emulator.

`svm/native_sources.rs` provides the retained shared 256-route owner for the
runtime AVIC EOI path. It fits the reserved 16KiB allocation and uses a bounded
try-lock, held through source completion. It records requester/index identity,
target CPU/vector and the original source vector plus qualified physical EOI
register address. Each CPU supplies its own checked UC mapping; a per-CPU host
virtual address cannot be retained as a shared source address. Direct device
completion does not inspect or acknowledge a physical LAPIC ISR entry.
Duplicate directed-EOI operations are coalesced, and a physical EOI key shared
by different virtual owners is rejected before publication. Mixed trigger
owners, occupied source identities, pending virtual vectors, unqualified level
sources and capacity overflow are refused without changing TMR or route state.
A callback failure poisons completion so partially issued EOI writes cannot be
replayed as though no side effects happened.

`BackingPage::prepare_trigger_stopped` atomically establishes TMR without setting
IRR, because IOMMU GA does not establish TMR. The physical route publisher must
exclude all writers of that vector and guest execution until source publication
finishes. This prerequisite is not supplied by the source-table lock alone.
Physical IRQ capture must reject vectors already owned by a direct route while
holding the same guard. Known source arbitration cannot make arbitrary mixed
IPI/device trigger use safe without control of those writers.

Installation supports a new, already-quiesced source only. Reprogramming an
existing route, clearing backing memory for INIT, and replacing destinations
remain refused until the outstanding GA writer drain is architecturally proven.
The runtime consumer does not create a hardware route or a configured flag.
Full PCI MSI/MSI-X/IOAPIC programming mediation and a physical IOMMU publisher
remain required; the callback boundary and source registry do not complete them.

### IOAPIC manual cross-reference and applicability

Primary AMD reference downloaded read-only to
`work/x2avic-ioapic-review-2026-09-16/43872.pdf`:
AMD SR5690/5670/5650 Register Programming Requirements, publication43872 rev3.05,
August2012, SHA256
`83ebb6874ba6cc29d8980620008aca0a53706b563cd471f82a0f12c8e3320521`.
Source: https://www.amd.com/content/dam/amd/en/documents/archived-tech-docs/programmer-references/43872.pdf

Rendered images actually reviewed: PDF1 cover; PDF119-122 / printed6-1..6-4
(Figure6-1, Tables6-1..6-6 and adjacent explanations); PDF133-135 /
printed6-15..6-17 (§6.5, complete Tables6-15/6-16). The reference chain connects
IOAPIC direct/indexed register access, remote-IRR behavior, EOI vector and full
redirection fields. It establishes for those products that a level source holds
remote IRR until a broadcast or direct EOI, and warns against simultaneous
broadcast and direct-EOI clearing. The reverse-source vector must survive guest
vector translation.

**This reference does not qualify the current B850 platform.** Table6-3 PDF120 /
printed6-2 labels both IRQ_PIN_ASSERTION and EOI_REGISTER at BAR+0x20. Whether
that is a publication error is unresolved. It cannot justify silently writing
the conventional +0x40 offset on this board. The implementation therefore takes
an explicitly platform-qualified physical register address and has no default
EOI offset or automatic platform qualification. The synthetic tests' +0x40
addresses are fixtures, not measured hardware evidence. No exact B850 IOAPIC
register/errata chain was found in the local library. Current-platform
qualification and simultaneous physical EOI exclusion remain admission work.

### Validation and measurement

Focused host tests: 24 native-MMIO tests, 10 independent decoder/CET regressions,
5 source-owner tests and 7 x2AVIC tests pass. The added native-MMIO test checks a
non-APIC-stride DWORD and wrong x2AVIC binding refusal before the callback.
Source tests exercise reverse-vector mapping with no physical ISR and no IRR
injection, deduplication, alias/trigger/pending-state refusal, lock lifetime,
partial-completion poisoning and retained-capacity bounds. Resident-feature
`cargo check` passed. No hardware source was programmed; no boot, IRQ latency,
exit frequency, source loss, EOI timing or Windows protection compatibility was
measured by this batch. Existing unsupported lifecycle and reset conditions
remain unsupported, and the result is not sandbox-readiness evidence.
