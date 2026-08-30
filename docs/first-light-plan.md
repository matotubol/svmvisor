# First-light plan

## Decision

The next physical milestone is **one automatically dispatched, resident,
record-only DXE option ROM that records bounded evidence and returns to native
boot**. This milestone is called **physical first light**.

Physical first light is not:

- another M0b USB inventory boot;
- target qualification or candidate production launch;
- an FPGA requester, DMA, or arbitrary-TLP experiment;
- persistent host allocation or takeover;
- an SVM control-state write, `EFER.SVME` enable, or `VMRUN`; or
- execution of an untrusted guest.

The full [bare-metal bring-up roadmap](minimal-baremetal-bringup-roadmap.md)
remains authoritative for later milestones. This document defines the smaller
near-term cut line so later launch and containment gates are resolved when the
implementation reaches them. The current machine-readable planning snapshot is
[development-gate-ledger-v1.json](development-gate-ledger-v1.json).

## Current position

- Schema v4 remains the sole canonical M0b bundle.
- The finalized, dual-verified schema-v6 bundle is a noncanonical promotion
  candidate. Its one-shot USB authorization is consumed and permits no retry.
- No schema-v7 M0b USB capture is planned. A future promotion or derived
  qualification decision must not rewrite any raw evidence.
- The current DXE entry is scaffolding that returns `SUCCESS` without lifecycle
  work.
- The current transitional Squirrel build still contains inherited requester/
  DMA-capable logic and is forbidden for physical svmvisor bring-up.

An explicit V6 promotion decision is the next evidence-governance checkpoint.
It is not a boot, flash, launch, control-state-write, or `VMRUN` authorization.

## Current work item

The immediate checkpoint is the explicit V6 promote-or-reject decision. After
that decision, the first implementation task is the **manifest-gated flash path
and pinned non-enumerating recovery image**, followed by the completion-only
Squirrel endpoint. It is FPGA delivery/recovery work, not SVM EFI work.

The QEMU/OVMF harness and resident record-only DXE trace may be developed in
parallel because they cannot become a physical delivery path until the recovery,
endpoint, bypass, BAR0, and JTAG gates pass.

## Authorization boundaries

Each boundary needs its own decision. Passing an earlier boundary never
implicitly authorizes a later one.

| Boundary | Purpose | Current rule |
| --- | --- | --- |
| M0b USB inventory | Read-only platform inventory | V6 run consumed; do not boot that medium again |
| Emulated DXE | Raw-EFI and automatic option-ROM testing in QEMU/OVMF | Next development track; no physical authority |
| Physical first light | Completion-only card plus resident record-only DXE trace | Blocked until the entry gate below passes and a separate run is authorized |
| Synthetic `VMRUN` | Reversible one-processor SVM/NPT test | Separate later authorization after host/reset/world-switch gates |
| Persistent trusted launch | Continue clean EFI/Windows as a guest | Separate later authorization after boot-policy, lease, watchdog, and recovery gates |
| Untrusted containment | Run guest-mutable or hostile code | Forbidden until AMD-IOMMU and platform/SMM/I/O containment pass |

## When the seven V6 blockers become walls

The seven blocker IDs describe missing evidence. They are not seven serial M0b
versions and do not all block physical first light.

| Blocker | First-light treatment | Hard wall |
| --- | --- | --- |
| AMD-IOMMU DTE/requester ownership and PCI isolation | Record topology if useful; make no ownership or isolation claim | Before any untrusted guest; full ownership is a later AMD-IOMMU milestone |
| Inherited memory-encryption state | Adjudicate the existing V6 `SYS_CFG`/CPUID evidence against the pinned PPR first; collect more only if that review proves necessary | Before constructing authoritative private host mappings or enabling SVM |
| Secure Boot databases, option-ROM policy, and TCG log | Inventory and hash them in the resident trace; declare the exact development trust state | Signed-policy proof is required by the Milestone 3 pass gate and later production use |
| Driver/SysPrep/Boot/recovery/hotkey namespace | Snapshot it in the same resident trace | Complete state-aware enforcement before persistent ReadyToBoot launch |
| Firmware event ordering | Measure it in the same resident trace; absence, duplication, or incompatible order stops this architecture | Before relying on ReadyToBoot/AfterReadyToBoot for launch and sealing |
| Direct watchdog and durable attempt lease | Select and document feasible mechanisms now; do not arm or mutate them during first light | Before persistent `VMRUN` |
| MP Services pre-measurement preservation | Do not dispatch APs in the first-light trace; retain V6 as post-dispatch inventory under the exact F7 audit | Before relying on opaque MP dispatch for pre-state, or before later AP/SMP SVM work |

Operationally, these become four workstreams: existing-evidence adjudication,
one resident DXE trace, persistent-launch recovery mechanics, and later
untrusted-guest containment.

## Physical first-light critical path

The two implementation lanes may proceed in parallel until they meet at the
physical run gate.

### Evidence and recovery lane

1. Make an explicit V6 promotion decision and freeze M0b USB collection.
2. Establish immutable source provenance for every candidate artifact.
3. Prove card-absent native Windows and external WinPE/WinRE recovery, and keep
   the BitLocker recovery material offline.
4. Specify and physically prove an immutable, visible `CARD_BYPASS` path.
5. Make flashing manifest-gated and build, audit, flash, and read back the pinned
   non-enumerating recovery image on an isolated fixture.

### Firmware and endpoint lane

1. Replace the inherited PCILeech control/TX design with the first-party
   completion-only endpoint.
2. Prove one guarded completion TX path and zero requester/DMA/arbitrary-TLP
   sources through simulation, synthesis, timing, and netlist-negative tests.
3. Implement the BAR0 phase journal and read-only USER2/JTAG snapshot.
4. Implement one resident record-only DXE trace driver.
5. Pass the raw-EFI and automatic-ROM QEMU/OVMF ladder before physical use.

## Resident first-light trace contract

The driver may:

- identify its owning PCI function and verify immutable build IDs;
- read and hash the Secure Boot databases and selected boot variables;
- read TCG2 capability, PCR/event-log metadata, and the existing event log;
- snapshot the Driver/SysPrep/Boot/recovery/hotkey namespace;
- resolve `BootCurrent` to the exact `Boot####` device path when available;
- register bounded record-only lifecycle callbacks; and
- publish bounded evidence through preallocated memory and the BAR0/JTAG
  diagnostic path, with any file output completed while its firmware protocol is
  valid.

It must not:

- write UEFI variables or boot policy;
- write PCI configuration, IOMMU MMIO, watchdog, or other platform control
  registers;
- write `VM_CR`, `EFER`, `VM_HSAVE_PA`, another MSR, or control registers;
- call MP Services to start an AP;
- reserve a persistent host runtime or alter the native memory map for launch;
- introspect a process or execute untrusted code; or
- execute `VMRUN`.

A hang or incomplete trace is a failed diagnostic run. Recovery is manual
power-cycle plus the already-proved bypass/recovery path; first light does not
pretend that the later direct watchdog and lease already exist.

## Entry gate for one physical first-light run

All of the following must be true before a physical run is separately
authorized:

- the V6 promotion question is resolved and M0b USB collection is frozen;
- candidate source and tool inputs have immutable provenance;
- card-absent recovery, offline BitLocker recovery material, and external media
  are proved;
- the non-enumerating recovery image and manifest-gated flash/readback path pass;
- `CARD_BYPASS` is latched, visible, immutable until reset, and physically
  proved;
- the completion-only endpoint passes its RTL, implementation, timing, and
  netlist-policy gates;
- BAR0 and USER2/JTAG preserve the last committed phase through a simulated
  hang;
- the resident trace passes its QEMU/OVMF positive and negative tests; and
- a new authorization names the exact bitstream, ROM, DXE, manifests, machine,
  trust state, permitted writes, recovery procedure, and one-shot or bounded run
  count.

## Definition of done

Physical first light is complete when one authorized run proves all of the
following without SVM or persistent takeover:

- firmware automatically reads and dispatches the exact option ROM;
- the record-only DXE entry and registered callbacks execute in the observed
  order;
- the bounded Secure Boot/TCG, boot-namespace, and lifecycle trace is retained;
- the card emits no requester/DMA/arbitrary-TLP traffic and records no TX policy
  violation;
- native clean boot or the authorized recovery path remains available; and
- the returned evidence is finalized without rewriting the raw record.

Only then does work cross into the persistent host substrate, reset mechanics,
world-switch contract, and reversible synthetic `VMRUN` milestones.
