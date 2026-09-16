# Native x2AVIC replacement: integration design

Status: source audit and working-tree integration, 2026-09-16. No firmware
programming or physical execution is represented by this note. The implementation
checkpoint below supersedes the original design-only status; incomplete source
ownership is explicitly not a boot-ready replacement.
Source inspected on main at `5197fab8dfd62bb0a6af437aac7977821fc217ea`.
The exact currently flashed diagnostic image and its limited evidence remain in
[the handoff](handoff-2026-09-16.md).

## Working-tree implementation checkpoint

The runtime now contains a per-image AVIC backing page, shared-table alias at
`f4000`, strict captured x2APIC/CPUID admission, native x2AVIC VMCB configuration,
and accelerated ordinary x2APIC MSRPM permissions. APIC_BASE is a logical
shadow; current-count reads use an ordinary intercepted MSR completion. AVIC
trap handling does not advance RIP or repeat hardware EOI effects. Guest xAPIC
NPF handling and guest physical ICR forwarding were removed from the runtime.
The earlier independent native/fixture modules remain while the full replacement
is being completed, per the integration owner's instruction.

The physical capture backend installs returning IRQ gates and uses an actual
captured vector, virtual IRR publication, and ordered physical level EOI
bookkeeping. Its provisional count-driven timer backend supports one-shot and
periodic timer writes; other active LVT source configurations stop explicitly.
This is not an alternative production routing profile. The user subsequently
selected full x2AVIC plus IOMMU interrupt ownership. Direct IOMMU level EOI,
device-route programming and global producer quiescence for guest INIT remain
unimplemented integration obligations. A live guest INIT currently stops at
startup stage14 before changing target CPU/backing/source state; it must not be
described as supported native startup or a finished Windows boot implementation.

Focused executed checks: `permission_maps::x2avic_profile_*` and
`native_guest_startup::x2avic_cpu_startup_*` passed on the Windows host. The latter
checks CPU-only INIT/SIPI control preservation with supplied quiescence; it does
not establish hardware producer exclusion. Windows-target Rust checks passed
with both `resident-runtime` and `resident-runtime-test` features at this
checkpoint. The final linked integration still needs its native image audit.
No native timing, interrupt-loss, Windows boot,
IOMMU or physical-image claims follow from these host checks.

## Required outcome and unresolved entry conditions

Replace the production guest native-LAPIC passthrough/MMIO-emulation policy with
one x2APIC/x2AVIC owner. Do not retain a selectable native or legacy fallback.
Keep physical host APIC operations required to bootstrap CPUs, acknowledge real
sources, program timers, and notify another resident host.

Two conditions must be resolved before treating this design as executable:

1. The parent's live measurement on the target reports AVIC and x2AVIC but
   CPUID.1:ECX[21] clear. Do not infer absence of x2AVIC from a PPR reset-value
   table; equally, do not remove the current x2APIC admission gate or write a
   feature-override MSR without verified processor-specific authority. Boot-time
   observations and the complete feature-control rule are required.
2. The current successful-EBS-return hook preserves the loader's existing APIC
   interface. `physical_boot.rs` deliberately does not promote the BSP in the
   boot profile. A loader that selected xAPIC earlier may keep using MMIO after
   this hook. Advertising x2APIC only after EBS, or merely setting the host
   APIC_BASE bit, does not establish that the loader has changed interfaces.
   An x2APIC-only guest needs an established x2APIC entry state or a verified
   earlier loader transition point. An xAPIC handoff is unsupported until this
   is established. Transitional AVIC xAPIC support would be extra scope requiring
   an explicit decision; it must not be silently introduced as a fallback.

Architecture encodings and hardware claims use the independent
[rendered manual review](x2avic-hardware-review-2026-09-16.md), including its
hashes, page indices, printed pages and cross-references. Its reviewed APM2
rev.3.44 section 15.29 spans printed pp563-583, PDF pages625-645, indices624-644;
SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
This source audit does not substitute extracted text or existing comments for
that review. The software physical interrupt bridge and complete IOMMU setup
remain unverified architectural dependencies.

## Current owners and replacement boundaries

| Current owner | Concrete change |
| --- | --- |
| `svm/vmcb.rs` | Add validated x2AVIC configuration and exit interpretation to the existing byte-exact VMCB owner. Native boot currently requires interrupt controls zero. Generic event validation explicitly rejects AVIC; give the native accelerated profile its own complete validity contract rather than accepting extra bits globally. |
| `host/resident/runtime.rs` | Replace `State.icr` plus physical `apic_base` guest ownership with one virtual LAPIC owner, hardware backing page and source/timer service state. Keep a separate immutable host routing identity. Remove callbacks that execute guest APIC register accesses on the physical LAPIC. |
| `svm/ipi.rs` | Retain target-owned CPU reset/SIPI and bounded mailbox concepts, rework them for virtual destinations and AVIC state. Delete native forwarding, physical destination-width matching, and guest ICR overlays once superseded. Fixture IPI code has separate callers. |
| `svm/native_apic_reset.rs` | Replace physical guest-INIT reset with virtual backing/state reset. Real host source retirement is a separate obligation; guest INIT must not reset the host LAPIC. |
| `dxe/native/resident/physical_boot.rs` | Keep actual AP bootstrap and post-firmware survey. Establish and check the host mode separately from the guest mode. Do not make guest startup send hardware INIT/SIPI to a running physical guest owner. |
| `dxe/native/resident/activation.rs`, `launch.rs`, `allocation.rs` | Retain the existing runtime pool allocator and lifetime proof; extend closure and directory validation for the actual AVIC allocation. No VM-exit allocation. |
| `dxe/native/resident/runtime.S`, `host/resident.rs` | Replace the assumption that guest-live CR8 and physical LAPIC own ordinary delivery. Add an audited physical interrupt entry/service boundary if software delivery is selected. Preserve all guest GPRs before calls, private stack/TLS, live guest xstate, and nonreturning terminal behavior. |

## Retained memory plan

The existing pool has one aligned 1 MiB resident image per CPU and excludes the
whole pool from guest NPT. Per-image BSS already owns the VMCB, auxiliary VMCBs,
HSAVE, stack, page tables and state; add an aligned backing page there. Do not
allocate one from reclaimed boot-services storage or share a backing page
between vCPUs. Hardware-modified data cannot be held through ordinary mutable
Rust references across VMRUN or remote delivery.

Reserve new shared table pages below the current `CACHE_OWNER_OFFSET`, with a
single named layout contract shared by the runtime, DXE validation and linker.
For example, move the end-of-image bound down by the *verified* table storage
size, assign that range in slot zero, and map identical local aliases in every
private host root. Compute physical addresses from pool base and table offsets;
do not confuse a local alias with the hardware physical operand. The physical
table entries must be indexed by the documented guest destination identity, not
by dense CPU slot. Validate holes, maximum identity, extent and reserved bits.
The manual review decides exact table count, size and encoding.

The existing high aliases are occupied: `f5000..f7fff` shared cache owner,
`f8000..fafff` cache capture, `fb000` diagnostic config, `fc000` diagnostic BAR,
`fd000` physical LAPIC, `fe000` startup/terminal shared page, `ff000` scratch.
Do not overwrite diagnostic aliases. Reclaim `fd000` only once no host xAPIC
operation uses it. Align `payload.ld`'s current `1ff000` bound with the stricter
runtime/DXE image bound; prove the final linked image fits before publishing.

Each vCPU is pinned to its current pCPU. Publish routing/backing state before
marking an entry valid/running. On exits, stop, INIT and parked AwaitSipi, use the
manual's running-state and notification protocol; do not claim a vCPU is running
merely because its physical CPU has not migrated. Shared table changes and
backing-page interrupt publication need explicit atomic ordering and race tests.

## Interrupt delivery is part of the replacement

The independent review has identified that AVIC virtual EOI updates virtual
state, level EOI needs VMM service, timer programming remains unaccelerated, and
device acceleration is an IOMMU facility. Therefore setting VMCB enable bits
while retaining direct physical IRQ delivery is not a coherent implementation.

The smallest candidate avoiding a new IOMMU owner is a software source bridge:

- Capture actual physical vectors through an owned host interrupt boundary.
  Classify edge versus level and physical source before publishing into virtual
  IRR; the bridge must never infer a vector from the INTR exit code alone.
- Maintain explicit physical source completion state. Edge-source physical
  acknowledgment and level-source completion require different handling. A
  virtual IPI's EOI must never acknowledge an unrelated physical ISR entry.
  Level-source handling must preserve IOAPIC remote-IRR/retrigger semantics.
- Program a host timer backend from virtual timer state, deliver expiration to
  the backing page, and implement count/divider/mask/periodic and current-count
  semantics. Hardware x2AVIC register acceleration is not a timer backend.
- A target receives its virtual IRQ while running, stopped or awaiting entry
  through the same backing-page publication/notification protocol. HLT must wake
  on eligible virtual events; stopped guests must not lose device interrupts.

This is a candidate architecture, not an admitted generic bridge. Before code
can honestly claim real MSI/IOAPIC support, specify the source-routing owner:
guest IOAPIC/MSI destination programming, physical versus virtual logical
destination matching and lowest-priority/PPR selection cannot be assumed equal.
Either validate and implement those transformations in this bridge, or implement
the documented IOMMU interrupt-remapping/guest-delivery route. Neither exists in
the current resident owner. The implementation must choose and complete one;
silently accepting only fixed edge interrupts does not satisfy the requested
native Windows boot replacement.

## Exit and state contract

| Operation | Authoritative state and completion |
| --- | --- |
| Accelerated register/IPI operation | Hardware backing page and documented VMCB/table protocol. Software must not perform a second operation or advance RIP again. |
| Unaccelerated AVIC access/incomplete IPI | Decode documented exit reason and whether hardware already stored/completed the instruction. Service only that reason. Unknown reasons stop with raw fields preserved. |
| APIC_BASE/x2APIC MSR fault | Logical guest mode and reviewed register rules; validate stopped instruction and event state before preparation. Legal completion advances exactly once; #GP keeps original RIP; unsupported policy preserves state. |
| INIT/SIPI | Receiver owns VMCB, GPRs, virtual LAPIC, pending source accounting, EFER/cache/reset/DR state. Prepare all fallible work before reset; quiesce hardware references before clearing backing state; acknowledge mailbox only after commit. AwaitSipi never enters the guest. Duplicate SIPI must not restart a running target. |
| External IRQ | Capture real vector/source before virtual publication; preserve physical acknowledgment state and virtual pending state across ordinary VM exits and guest faults. |
| Terminal stop | Make accelerated routing incapable of resuming stopped guests; retain first fault and existing all-CPU terminal barrier. Physical notifications are host control, not guest reset. |

`NativeStartupTarget::validate` currently rejects all virtual controls outside
the TPR nibble. Its validation and `Vmcb` event/GP checks must change together.
Existing guest event-replay limitations remain explicit; enabling AVIC must not
turn unresolved interrupted delivery into successful instruction completion.
Guest INIT itself does not require an xAPIC downgrade: the independent review
verifies APM2 section16.10, printed p657/PDF719/index718, preserves APIC_BASE
AE/EXTD while resetting the other specified LAPIC state. This does not resolve
the earlier loader handoff mode choice.

Guest CPUID must describe the admitted interface consistently: x2APIC support,
APIC ID/topology, guest APIC version and logical destination identity. Preserve
the real 24-CPU inventory rather than synthesizing slot IDs in one interface.
Nested SVM stays unavailable; the existing zeroed SVM capability leaf is not a
requirement to advertise host AVIC to Windows. Update `native_boot_cpuid` only
after its execution contract exists, including the masked native bit21 case.

## Deletion and cleanup map

Delete only as the replacement takes over the same caller; finish the batch
without a production legacy or native passthrough selector.

- Delete `svm/native_apic_reset.rs` and its module export. Replace relevant reset
  assertions in `native_apic_reset.rs` and `native_resettable_state.rs` tests
  with virtual-state/source-retirement checks.
- Delete the native half of `svm/xapic.rs`: `NativeMmioError`,
  `NativeMmioFailure`, `handle_native_mmio*`, `native_mmio_inner`, `NativeMov`,
  `NativeSegment`, the bounded native MOV decoder and its register helpers.
  Remove `terminal::apic_failure`'s coupling to that deleted error enum; retain
  historical wire decoding for already captured diagnostic images.
- Delete native xAPIC NPF dispatch from `host/resident/runtime.rs`, physical
  guest APIC callbacks, physical INIT reset writes and physical guest mode
  following. Delete `handle_native_x2apic_write`, `xapic_access`,
  `handle_native_guest_apic_msr`, the native passthrough MSRPM setup and obsolete
  `enable_native_startup_interrupts`. Replace shared names only where their
  remaining semantics still match.
- Remove obsolete native decoder/passthrough tests:
  `native_xapic.rs`, `native_xapic_startup.rs`, native destination-width/routing
  matrices and native APIC presentation/independent tests. Port their useful
  stopped-state, duplicate-SIPI and target-ownership assertions into focused
  replacement tests instead of retaining tests solely to keep dead APIs alive.
- Retire `--guest-startup` and the old fixed-passthrough `--smp-activate` build
  product once replaced. Fold startup ownership into the one native boot
  profile, removing the old `native-resident-guest-startup` capability split and
  obsolete native-passthrough rejection fixtures. Keep preparation-only and
  unrelated returning diagnostics if they have real callers.
- In `tools/native-resident/fixture`, remove `guest-xapic`,
  `guest-xapic-upgrade`, physical-forwarding/physical-reset-refusal witnesses,
  and their selection/output checks in `run.py`. Rewrite guest interrupt, APIC
  contract and startup fixtures for the new owner; do not count old emulator
  native-forwarding witnesses as x2AVIC execution evidence.
- Update README, crate guides and build instructions to one supported native
  contract. Mark the historical `native-xapic-startup`, `native-init-sx-wakeup`,
  `native-guest-apic-model`, `native-bootstrap-apic-admission` and physical
  evidence reports as superseded where appropriate; preserve exact-build
  evidence rather than rewriting history. Remove obsolete active instructions.

The synthetic `FixtureApic`, `LocalApic`, scheduler and fixture halves of
`xapic.rs`/`ipi.rs` remain independently used by the emulator validation platform;
they are not a production fallback. Remove a fixture component only when the
corresponding fixture consumer is also retired. Likewise retain guest memory
fetch/walk code still used by CPUID, cache, MSR or diagnostic interception.

## Bounded implementation and validation

### Proposed closure for the exclusive first-boot profile

Require CPUID x2APIC/AVIC/x2AVIC on every admitted CPU and an already-enabled
x2APIC guest interface at the captured loader handoff. Reject an xAPIC handoff
before guest continuation; do not promote its guest mode silently. The current
live target does not satisfy the CPUID prerequisite. Establishing an applicable
firmware configuration is a physical-test prerequisite, not a fallback path.

Use pinned one-to-one guest/host APIC IDs. The hardware review establishes one
4 KiB physical table for the measured non-extended x2AVIC profile, maximum
guest ID511, and no logical APIC table/V_APIC_BAR. Thus reserve one shared page
below `f5000` and one backing page in each image, subject to the linked fit
check. IsRunning describes assignment to the physical core and remains true
during ordinary VM-exit service; it is not a guest-mode bit.

The proposed IRQ bridge acknowledges physical edge interrupts after transfer
to virtual IRR and holds level physical ISR entries until matching guest EOI.
It tracks `physical_held` and `guest_eoi_completed` vector sets, then issues
physical EOI only while the highest physical ISR vector is completed. This avoids
acknowledging an unrelated physical source solely because a virtual IPI EOI
occurred. To make that proposal complete:

- Edge guest EOI can remain accelerated because its physical source was already
  acknowledged. Use the documented level EOI exit for level sources. Do not
  falsify guest TMR as a shortcut to forcing an EOI exit.
- A level source captured while the same vector is already in virtual ISR must
  not be completed by the earlier event's EOI. Keep queued versus in-service
  source association, or refuse an unsupported conflict before publication.
  In particular, do not change an existing edge event's TMR to level silently.
- Define INIT retirement of outstanding physical-source state before clearing
  virtual ISR/IRR; an active physical ISR must not be stranded by virtual reset.
- Identity IDs/LDR solve destination encoding, but physical lowest-priority
  routing still uses physical PPR while AVIC maintains virtual PPR. A fixed-
  destination delivery profile can avoid that mismatch. Arbitrary guest
  IOAPIC/MSI lowest-priority programming requires a routing owner or a checked
  unsupported-case outcome; trusted guest ownership alone is not such a check.
- Do not call a Rust IRQ handler while the interrupted host code holds a mutable
  reference to the same virtual APIC or VMCB. Capture into private interrupt
  records, then service them at an explicit unborrowed boundary.

These refinements close allocation/mode ambiguity and allow bounded primitive
implementation to proceed. The remaining interrupt issues are acceptance
criteria for runtime integration, not reasons to add an alternate APIC backend.

Once the two entry conditions and complete source-delivery route are resolved,
ownership splits cleanly into: (1) VMCB/backing/tables and exit contracts,
(2) DXE allocation/admission/activation, (3) runtime interrupt/MSR/startup and
assembly integration. Integrate one production profile and remove obsolete
paths in the same batch; no unused future abstraction or fallback flag.

Use focused tests for reserved bits/extent, no-change refusal, shared routing
publication versus stop/INIT, mode handoff, timer and edge/level EOI ownership.
Run the native linked-image/stack audit because assembly and resident storage
change. Run only fixtures capable of exercising the changed contract; emulator
absence of x2AVIC is a coverage gap, not a passing hardware test. Native boot,
interrupt timing, exit costs, loss/races and Windows protections remain
unmeasured until tested on the exact built/flashed image. Successful Windows
boot would not prove sandbox containment, Hyper-V/VBS support or invisibility.


## INIT producer review and completed cleanup checkpoint

This checkpoint supersedes the earlier physical guest-reset and guest
passthrough proposals. The exclusive runtime uses x2AVIC; full IOMMU source
ownership remains incomplete, and active guest INIT is not implemented safely.
The user's BIOS x2APIC change is pending measurement on the next reboot.

### Producer exclusion evidence

Reviewed visually in `docs/48882-3.11.pdf`, AMD IOMMU rev3.11 (April 2026),
SHA256 `f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22`.
Printed pages equal PDF one-based pages; zero-based indices are one less.
Rendered evidence is `work/x2avic-iommu-review-2026-09-16/p{page}.png`.

- Section 2.2.8, pp120-121: guest interrupt delivery directly targets the
  backing-page system physical address. The CPU physical-ID table is therefore
  not a gate for all producers.
- Section 2.3.2, p121: update a live IRTE atomically and invalidate cached
  interrupt information when its cacheable fields change. Cross-reference
  section 2.4.5, p131: INVALIDATE_INTERRUPT_TABLE invalidates cached interrupt
  information for DeviceID, including a cached backing-page pointer.
- Section 2.4.1, pp123-124, Table34: COMPLETION_WAIT orders prior commands;
  its completion-store ordering and Flush bit have explicit requirements.
  Its delegated ordering rule is section 2.4.11, pp138-139.
- Sections 2.4.2 and 2.4.3, pp125-126, plus pp130-131: device-table and
  translation invalidations have different scopes; one cannot substitute for
  all of them.
- Section 2.4.11.1/.2, pp138-139: command ordering and completion of
  translation-dependent DMA reads/writes are described. These pages do not
  explicitly say an interrupt-table invalidation plus COMPLETION_WAIT drains
  already-issued GA atomic IRR updates and doorbells. That dependency remains
  unresolved; the translation-page reclamation example is not proof for GA.

Thus a software mailbox lock, clearing IsRunning/Valid, a CPU fence, or an
unqualified invalidation completion must not authorize `BackingPage::reset_stopped`.
A bounded cross-CPU stop would exclude further guest ICR issuance but still
needs a documented completion boundary for prior CPU AVIC operations, local
source retirement and IOMMU writes. Switching to a fresh page still requires a
source-retarget transaction and a defined ordering for interrupts already aimed
at the old page; it is not an established substitute for producer exclusion.
No unused proof-token API was added. The current CPU-only `apply_x2avic` helper
retains an explicit caller precondition; its tests are not a hardware-reset
proof. The runtime's active INIT stop remains incomplete implementation, not a
supported guest outcome or a completed boot milestone.

### Cleanup actually performed

Removed from `svm/ipi.rs`: `handle_native_x2apic_write`,
`prepare_native_apic_access`, `native_guest_apic_msr`,
`handle_native_guest_apic_msr`, `handle_native_x2apic_startup_access`,
`handle_native_apic_base`, `NativeIcr::xapic_access`, legacy write admission,
the duplicate ICR overlay and its enable/reset operations, and their unused
error variants. `route_x2avic_startup` has no physical forwarding callback and
accepts only INIT/SIPI with an x2APIC destination profile. It preserves checked
identity routing, atomic broadcast FIFO publication and notification after
releasing the route guard. The independent synthetic IPI owner remains used.

Removed the unused `NativeApicReset` physical guest-reset plan. Retained
`admit_hidden_native_apic_state` for host extension-state validation; it must
not be used as a guest INIT implementation. Host xAPIC profile observation and
APIC-base inventory validation remain because DXE bootstrap still calls them.
They do not provide a guest passthrough backend.

Deleted obsolete test files: `native_startup.rs`, `native_xapic_startup.rs`,
`native_guest_apic_presentation.rs`, `native_destination_width_independent.rs`,
`native_init_deassert_independent.rs`, `native_routing_policy_matrix.rs`,
`virtual_apic_independent.rs`, and `native_resettable_state.rs`. Removed old
passthrough tests from `native_guest_startup.rs` and `native_destination_routing.rs`,
and physical-reset tests from `native_apic_reset.rs`. Preserved tests for actual
CPU-state/FIFO/host-admission owners and added focused x2AVIC transport tests for
full-width identity, publication-before-notification, refusal and all-or-none
broadcast. The removed native interrupt-control test belonged to the removed
`enable_native_startup_interrupts` API.

Validation: 11 focused tests in those three retained test targets passed on
`x86_64-pc-windows-msvc`; `cargo check -p svmvisor-hypervisor --features
resident-runtime --target x86_64-pc-windows-msvc` passed. These establish software
contracts only. No native device delivery, INIT reset, timing, Windows boot,
protection compatibility or direct-IOMMU route was measured in this batch.
