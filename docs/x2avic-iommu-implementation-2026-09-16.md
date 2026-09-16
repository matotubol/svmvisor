# IOMMU discovery and DMA mediation implementation checkpoint

> **Status update, 2026-09-16:** this checkpoint is historical. By user
> decision, the code described below was removed with its tests: the
> `native::resident::{iommu, iommu_boot}` discovery and trap code, and the
> `svm::iommu` capture and command code. Windows keeps the physical IOMMU, and
> GA posting is deferred. See the
> [x2AVIC completion record](x2avic-completion-2026-09-16.md).

## Scope and status

This batch adds native firmware discovery algorithms and the core command
mediation algorithms needed for the exclusively x2APIC/x2AVIC rewrite. It does
not establish a completed native IOMMU takeover, working device routing, or
Windows boot. The user reports enabling BIOS x2APIC for the next reboot; no
new CPU/IVRS/live-register observation has been made. Prior XTSup=0 remains
historical evidence, not a reason to skip source implementation.

The DXE activation owner integrates discovery separately. The entry points are
`native::resident::iommu::{rsdp_address,load_ivrs,discover_ivrs}`. All ACPI
memory reads go through its supplied `FirmwareReader`; the decoder contains no
raw physical-pointer dereference. The reader must prove readable RAM, current
paging and lifetime before access. Numeric address validation is insufficient.
Inventory owns no borrowed firmware pointer beyond the caller's retained copy.
It keeps original IVRS bytes, including IVMD exclusions, special requester
settings, aliases and Type40 HID declarations. It does not suppress IVRS or its
preboot DMA protection flag. Multiple descriptions of one unit are deduplicated
with Type11 capability precedence. Source alias overlap is not silently resolved.

`svm::iommu::capture` validates the PCI capability/header and enabled aperture
against IVRS before reading MMIO. It captures live control before/after, EFR,
EFR2, DTE root and enabled segmented roots, command/event roots and pointers,
exclusions, status, and GA roots/pointers when available. This sequence is an
observation, not a coherent snapshot, writer exclusion, or ownership token.
Control drift remains visible. Reserved/unsupported active segment encodings
are retained in Control and do not become supported by capture.

## Core command algorithm contract

`merge_dma` accepts ordinary host DMA translation and keeps the exact requested
V/TV, permissions, root, paging mode, DomainID, exclusion and fault-control bits
in words0/1 while preserving monitor-owned interrupt word2. It refuses
ATS/PASID/PPR, guest translation, dirty/accessed updates, CXL and vIOMMU fields.
It does not force identity translation, clear V or silently remove protection.
The backend must preserve guest memory ownership and native GPA=SPA semantics
for ongoing Windows DMA page tables and validate DomainID/root consistency.

`GuestCommandQueue` models a bounded live guest command consumer. It processes
at most64 entries per service, validates aligned wrapping head/tail and available
space, and retains unfinished work for another runtime service. Supported
commands are ordinary non-PASID INVALIDATE_IOMMU_PAGES, INVALIDATE_DEVTAB_ENTRY,
INVALIDATE_INTERRUPT_TABLE through the source owner, and COMPLETION_WAIT with
optional store. Completion-interrupt requests, ATS/PASID/other commands remain
explicitly unsupported. DTE invalidation also calls source mediation, so guest
changes to IV/IntCtl/interrupt roots cannot be silently ignored.

The backend must publish DMA words0/1 using one aligned atomic128 update,
preserve owned interrupt fields, and actually wait for physical completion. It
must validate guest completion-store RAM and never permit writes into monitor
storage. Failure retains the failed head, poisons the queue and prevents replay
of a partially published transaction. No RIP or register update occurs here;
the runtime must complete an intercepted tail write only after it accepts the
write and guarantees further service, and must stop on unsupported/error cases.

`CommandRing` reserves the submitted command plus a distinct CompletionWait,
uses a fresh coherent completion-store sequence, and caps completion polling
at100,000 iterations. RingIo must order all table/ring stores before MMIO tail.
Head movement alone never counts as completion. Halt, event indication and log
overflow remain visible and cause refusal; this component neither clears nor
consumes error logs. A failure poisons the queue and storage remains retained.
Completion is **not** a proof that GA IRR atomics/doorbells have drained.

These command components currently have focused test backends. They require an
actual connected runtime backend and retained resource owner before production
activation; their presence does not supply the missing hardware owner.

## Manual review provenance

All listed pages were inspected as full rendered page images, including
continued tables and adjacent explanations. Images are under
`work/x2avic-iommu-review-2026-09-16/`.

AMD IOMMU specification `docs/48882-3.11.pdf`, revision3.11, April2026, SHA256
`f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22`.
Applicability is conditional on actual hardware features and authoritative
firmware declarations. Printed pages equal one-based PDF pages; zero-based
indices are one less.

| Chain used | Printed/PDF pages |
|---|---|
| IVRS precedence and formats, Tables85-113, including complete continued tables and all device/IVMD footnotes | 287-309 |
| Table89 feature-image references -> PCI aperture/header -> Control/EFR and full continued tables | 205-228 |
| DTE independent portions -> Figure7/Table7 -> Tables5/6/8/9/10 -> atomic publication §2.2.2.2 and continuation -> complete segmentation Table13 | 60-79 |
| Commands -> CompletionWait/Table34 -> DTE invalidation/Table35 -> page invalidation/Table36 -> interrupt invalidation/Table38 -> ordering §2.4.11 | 122-131,138-139 |
| Command/event pointers and status, complete status field table -> complete GA head/tail definitions | 252-259 |
| GA base/tail-address observations | 233-234 |
| Physical system x2APIC requirement | 97 |

The Table36 size encoding reference was followed to complete Table14 on p80,
including both notes. The undefined all-ones size encoding is refused. The
mediator forwards the ordinary page-invalidation command without computing its
range; full translation-tree validation remains a dependency.
ATS/PASID-specific cross-references are outside the admitted algorithm profile.
Control and EFR fields captured raw are observations, not verified support for
their optional modes. Full SMI/event fault policy and live takeover remain open.

ACPI `docs/ACPI_Spec_6.6.pdf`, release6.6, SHA256
`8c7542dd4de974ae47bba71bb0336637fe1e3838daad7692370ab4cf218efd35`.
The user supplied this local reference during implementation after the official
PDF endpoint returned403. The local PDF resolves that availability dependency.
Applicability: UEFI ACPI2+ RSDP and XSDT, not the legacy ROM-search profile.
Rendered PDF pages174-181 (zero-based173-180), printed103-110, cover
§§5.2.5.2/.3 -> complete Table5.3 -> §5.2.6/Table5.4 -> complete signature
Tables5.5/5.6 (IVRS delegates to AMD's specification) -> §5.2.8/Table5.8.
The full XSDT checksum and both RSDP checksums are checked. RSDT is not used
as fallback when XSDT is malformed.

## Checks and evidence limits

Executed on the development host:

- `cargo test -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --test iommu`: eight tests passed.
- `cargo test -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-preflight --test native_iommu`: five tests passed.

Tests exercise DMA preservation/refusal, software command ordering and failure
poisoning, bounded completion-store waiting, ring wrap/overlap, firmware XTSup
admission, PCI-before-MMIO checks, preserved control drift, IVRS variants and
malformations, special/exclusion retention, and bounded ACPI memory traversal.
They do not measure native MMIO behavior, command or interrupt latency, device
loss/rearming delay, GA overflow recovery, or Windows boot. No hardware write,
flash, DMA policy change or protection suppression was performed by this work.

Remaining native dependencies: retained shadow DTE/IRTE/command/log storage;
firmware/loader writer exclusion and final post-firmware revalidation; physical
ring drain/switch; atomic128 publication backend; guest physical-IOMMU MMIO/PCI
register mediation including control/root lifecycle; ongoing Windows DMA-tree
ownership and consistency; guest event/completion interrupt relay; live device
programming/IRTE publication; and proven direct-GA writer drain before INIT.
Hyper-V/VBS coexistence, encrypted/guest translation, ATS/PASID, hotplug and
SR-IOV remain unsupported. Trusted first boot is the scope; neither sandbox
readiness nor undetectability is established.
