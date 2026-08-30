# Milestone 0b record-only UEFI probe

The M0b implementation is a **removable-media inventory probe**, not a
hypervisor launch candidate and not the complete Milestone 0 pass gate. Slice 1
collects bounded CPU and UEFI facts. Slice 2A adds a bounded, record-only ACPI
directory and table capture. Schema v3 adds cardinality-bounded per-processor
CPUID/`VM_CR` consistency by starting enabled, healthy APs only for read-only
measurements. Schema v4 adds bounded live AMD-IOMMU PCI/MMIO inspection derived
only from the same-run IVRS, MCFG, and memory map. Schema v5 adds the reviewed,
per-processor system-register slice: 41 new allowlisted read-only `RDMSR`
sites for SMM configuration and lock state, IORRs, `TOP_MEM`/`TOM2`, SYS_CFG,
HWCR, MTRRs, and PAT. Its one authorized physical capture completed but was
preserved as rejected diagnostic evidence after exposing collector and
comparison-contract defects. Corrected schema-v6 / collector 0.6.0 software is
implemented and locally release-gated. Its separately authorized physical
capture completed and was strictly finalized and dual-verified as a
noncanonical promotion candidate. No slice writes hardware,
dereferences configured pointers, assesses ownership, proves SMM immutability,
or provides the resident firmware-event trace. Every result remains
`qualification_status = "blocked"`. The one-shot schema-v6 authorization is
consumed and permits no retry.

M0b collection is now frozen: later evidence closes a separate derived gate
ledger and does not add more removable-media M0b slices. The next firmware
artifact is the resident, record-only DXE trace defined by the
[first-light plan](first-light-plan.md); it may reach physical option-ROM
delivery only after the endpoint/recovery gates and still performs no SVM
enablement or `VMRUN`.

## Artifact boundary

`svmvisor-m0b-probe.efi` is a UEFI application built separately from
`svmvisor-dxe.efi`:

- the M0b application is run manually from explicitly prepared removable media;
- the DXE image remains the small EFI boot-service driver packaged into the
  4 KiB option-ROM aperture; and
- the M0b application is never passed to `rompack`, packaged by
  `firmware/squirrel/package-rom.ps1`, or flashed onto the current Squirrel.

This separation is mandatory. The current Squirrel integration remains
requester/DMA-capable and must not be flashed, while the inventory application
is much larger than the present option-ROM budget.

Build the application from the repository root:

```console
cargo build-m0b-probe
```

The output is
`target/x86_64-unknown-uefi/release/svmvisor-m0b-probe.efi`, with PE subsystem
`EFI_APPLICATION`. `cargo build-dxe` still emits an
`EFI_BOOT_SERVICE_DRIVER`.

## Exact write and instruction boundary

The application performs one permitted persistent operation: it creates and
flushes one new JSON evidence file on the same firmware-reported removable
volume from which it was loaded. It refuses to run when that volume is absent,
read-only, or not reported as removable, and refuses to overwrite an existing
output path. It also requires the canonical `svmvisor-m0b-target.txt` marker
prepared from one immutable M0a target-profile bundle.

Protocol inspection uses non-disconnecting UEFI `GET_PROTOCOL` opens. It never
requests exclusive protocol access, disconnects a controller, or reconnects a
driver stack. The fixed 88-byte binding marker is read into a 89-byte bounded
buffer, so corrupt on-media length metadata cannot drive an allocation.

Because UEFI FAT exposes no atomic create-new operation, the app checks both
the unique final and `.partial` paths twice, opens without delete/truncate
behavior, and verifies that the partial file is empty before writing. It flushes
the partial bytes, revalidates the exact Block I/O media ID and properties, then
uses one successful rename as the terminal commit to the accepted `.json`
name. A concurrent creator or physical media swap at the final instruction is
outside what this firmware API can make atomic; the host finalizer rejects any
`.partial`, basename mismatch, partial bytes, or invalid record.

It does not:

- write an MSR, control register, PCI configuration register, BAR, MMIO
  register, watchdog, UEFI variable, TPM PCR, or firmware table;
- enable SVM or execute `VMRUN`, `VMLOAD`, `VMSAVE`, `CLGI`, `STGI`, `SKINIT`,
  or `INVLPGA`;
- modify boot, Secure Boot, BitLocker, Windows, recovery, or FPGA state; or
- authorize candidate flashing, launch, process introspection, untrusted code,
  or a confidential-VM claim.

Slice 2A copies firmware-described ACPI bytes only after the entire requested
physical range is proven to lie within one descriptor from the same-run UEFI
memory-map snapshot whose type is `EfiACPIReclaimMemory` or
`EfiACPIMemoryNVS`. Checked arithmetic also keeps the range below the physical
address width reported by CPUID. These bounded memory reads do not authorize a
PCI configuration access, an IOMMU MMIO access, or following FADT pointers.

The only privileged CPU operation in this slice is one allowlisted instruction
site that reads AMD `VM_CR` (`C001_0114h`) at most once on each measured
processor. It is reached only after that same processor identifies itself as
`AuthenticAMD`, enumerates SVM in CPUID `8000_0001h`, and enumerates the SVM
capability leaf `8000_000Ah`. AMD APM Volume 2 rev. 3.44 Section 15.30.1 defines
that MSR and its `DPD`, `R_INIT`, `DIS_A20M`, `LOCK`, and `SVMDIS` fields. There
is no generic MSR API and no write counterpart in the probe.

Schema v3 obtains the BSP number through MP Services `WhoAmI`, measures it
directly, then invokes every enabled, healthy AP sequentially with blocking
`StartupThisAP`, no event, and `TimeoutInMicroseconds == 0` (blocking with no
MP Services timeout). The AP callback is
preallocated and performs only AP-safe `WhoAmI`, bounded CPUID, the conditional
named `VM_CR` read, and a release/acquire result handoff. It performs no
allocation, formatting, logging, filesystem or Boot Services table access, and
no application-issued control-state write. Its sole UEFI protocol call is the
AP-permitted MP Services `WhoAmI`. The actual callback `WhoAmI` processor
number is retained in each observation, and the BSP repeats the complete MP
Services enumeration after dispatch; a changed record aborts the capture.

That callback boundary does not make every firmware dispatch mechanism
physically read-only. PI leaves AP wake/termination to firmware, which may use
INIT/SIPI or reset; an AMD INIT can alter `VM_CR` state in some lock
configurations. Schema v3 therefore labels the observations as post-dispatch,
states that preservation of pre-dispatch `VM_CR` is not generally proven, and
retains this as a blocker.

The [exact Gigabyte F7 MP Services audit](f7-mp-services-audit.md) resolves the
narrower physical-run policy for this target. Its default PCD selects
`ApInMwaitLoop`, the captured CPU supports MONITOR/MWAIT, and an ordinary
cold-boot `StartupThisAP()` reaches the memory-signal wake path without
INIT/SIPI. The same audit found that a finite timeout can enter F7's AP-reset
recovery path, which is why timeout zero is mandatory. S3/resume, another BIOS
build, an altered PCD, or any broader pre-dispatch preservation claim remains
outside the decision. A dispatch error, failed callback identity, unhealthy
enabled processor, incomplete coverage, or changed post-dispatch enumeration
aborts the capture. With no MP Services timeout, a stalled AP can instead block
the BSP indefinitely and requires manual power-cycle recovery.

UEFI pool allocation, protocol opens, filesystem reads, and the one selected
file creation necessarily change transient Boot Services bookkeeping. They do
not constitute persistent platform/control-state authorization.

## Evidence collected by slices 1, 2A, the per-processor slice, and schema v4

The raw JSON preserves:

- CPUID leaves `0`, `1`, `7:0`, `8000_0000h`, `8000_0001h`,
  `8000_0002h`-`8000_0004h`, `8000_0008h`, `8000_000Ah`, `8000_001Eh`, and
  `8000_001Fh`, with every optional query bounded by the reported maximum leaf;
- the CPU vendor/brand and family/model/stepping;
- raw SVM feature bits plus decoded capability fields for SVM, NPT, SVM-lock,
  NRIP-save, VMCB-clean, FlushByAsid, and DecodeAssists, together with SVM
  revision, ASID count, and physical-address width;
- raw memory-encryption capability fields, C-bit position, and physical-address
  reduction without claiming that SME, SEV, or SNP is enabled;
- raw `VM_CR` and its five defined low fields, or an explicit
  `not-attempted` reason;
- UEFI vendor/revisions and every configuration-table GUID/root pointer without
  dereferencing unvalidated ACPI or SMBIOS data;
- MP Services processor numbers, firmware hardware IDs, enabled/healthy/BSP
  flags, and firmware locations, including an explicit count-consistency check;
  and
- a UEFI memory-map snapshot labeled as collection-time state, not the final
  `ExitBootServices` map.

Schema v2 additionally requires a valid ACPI 2.0 RSDP selected from the UEFI
ACPI 2.0 configuration-table GUID and both of its mandatory directory roots:

- complete checksum-valid RSDP, RSDT, and XSDT bytes, with signatures,
  revisions, declared lengths, pointer arithmetic, entry widths/counts, and
  duplicate pointers validated;
- a deduplicated directory containing the checksum-bound 36-byte header of
  every RSDT/XSDT target, with its physical address and root provenance;
- complete raw captures of exactly one whitelisted `APIC` (MADT), `MCFG`,
  `IVRS`, and `FACP` (FADT), with lowercase-hex bytes, exact byte length,
  SHA-256, common-header fields, checksums, and table-specific witnesses;
- bounded MADT entry decoding and an exact enabled processor hardware-ID set
  comparison against the MP Services observations from the same run;
- bounded MCFG allocation-window decoding without accessing ECAM; bounded IVRS
  IVHD/IVMD and device-entry decoding without accessing PCI configuration or
  MMIO; and selected FADT fields without following FACS/DSDT pointers or
  invoking any reset/control register.

MADT processor records with both `Enabled=0` and `OnlineCapable=0` are retained
verbatim but ignored for interrupt-controller-ID uniqueness and topology-set
comparison, as ACPI defines those entries as unusable. Enabled or
online-capable records must still have unique APIC/x2APIC IDs, and ACPI
processor UIDs remain globally unique.

The allocation and traversal limits recorded in every v2 result are 4 KiB per
RSDP, 1 MiB per SDT, 4 MiB cumulative ACPI bytes, 64 UEFI configuration-table
entries, 256 entries per root, 256 unique root pointers, 512 MADT entries, 256
MADT processor entries, 256 MCFG allocations, 256 IVRS blocks, and 4096 IVHD
device entries. Evidence JSON itself is capped at 16 MiB. A missing mandatory
root/table, malformed or truncated length, checksum failure, arithmetic
overflow, unsupported variable IVHD entry, duplicate pointer within one root,
or exceeded limit fails the capture rather than producing partial success. A
physical read also fails closed if its containing UEFI descriptor carries the
`EFI_MEMORY_RP` read-protect attribute.

IVRS decoding is pinned to the repository-local AMD *I/O Virtualization
Technology (IOMMU) Specification*, publication 48882 rev. 3.11 (April 2026),
SHA-256
`f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22`.
For each IVHD the evidence preserves the firmware-reported IOMMU DeviceID
(PCI BDF encoding), capability offset, MMIO base, PCI segment, device-coverage
entries, and feature fields. Type `11h` and `40h` IVHD blocks preserve both
64-bit extended-feature images; they are the preferred firmware feature
description when supplied according to rev. 3.11.

Those fields are firmware descriptions, not live hardware observations. Table
presence, DeviceID, capability offset, MMIO base, coverage entries, and IVHD
feature images do **not** establish that an IOMMU is enabled, who owns it,
whether DMA remapping is active, or whether any requester/slot is isolated.
This slice therefore keeps every AMD-IOMMU ownership and PCI-isolation claim
strictly `false`.

Schema v3 retains the root `cpu` and `vm_cr` objects as the MP-identified BSP
reference and preserves a complete CPU/`VM_CR` record for every enabled,
healthy processor. CPUID leaf 1 EBX[31:24] and the identity-bearing fields of
`8000_001Eh` are preserved raw but excluded from capability equality; their
low eight APIC-ID bits are checked against the PI-defined low byte of the same
processor's UEFI MP Services ID, with PI reserved bits required to be zero.
Interpreting `8000_001Eh` additionally requires AMD's topology-extension bit.
Its core-ID and node-ID fields are retained as per-processor identity but are not
equated with firmware package/core/thread location fields.
The callback's retained `WhoAmI` number binds the measurement to its dispatch
target. Every other collected capability bit and optional-leaf presence is
compared, and `VM_CR` status/raw value is compared exactly.

Schema v4 is governed by the separate
[PCI/MMIO read policy](m0b-iommu-read-policy.md). It revalidates and enumerates
the same-run IVRS/IVHD instances, derives the MCFG witness, and performs only
the bounded PCI capability and MMIO register reads in that policy. Those reads
can record live implementation, enable, and global run state, but not ownership
or requester-specific isolation. No unit is hardcoded from a diagram and no
runtime state is inferred from IVRS alone.

The v4 record binds the complete IVHD source set, selected unit, MCFG/ECAM
witness, and exact UEFI memory descriptors that authorize ECAM and MMIO reads.
It preserves the selected PCI identity, the bounded conventional-capability
chain, two identical raw capability snapshots, exact access counters, and one
of three fail-closed MMIO outcomes: PCI MMIO disabled, IVRS/live EFR conflict,
or observed. An observed result contains two stable allowlisted configuration
snapshots, one status sample, and decoded configured ranges with same-run
memory-map witnesses; configured table/buffer pointers are never dereferenced.
Every write/direct-access counter and every ownership/isolation claim must
remain zero or `false`.

Schema v5 is governed by the separate
[system-register read policy](m0b-msr-read-policy.md). It extends the shared
BSP/AP measurement body with 41 new named, non-inlined `RDMSR` sites — one per
reviewed MSR address, each pinned by the PE/instruction gate to its literal
address — and reuses the same blocking, timeout-zero MP Services dispatch, so
every enabled, healthy processor records the same allowlisted set: `MTRRcap`,
`MTRRdefType`, PAT, the `MTRRcap`-bounded variable and fixed MTRRs, SYS_CFG,
HWCR (`SmmLock`, `SmmPgCfgLock`), TOP_MEM, TOM2, SMM_BASE, SMMAddr, SMMMask,
and, only on the pinned-PPR family/model, the two IORR pairs. The record
preserves every raw value with decoded-field redundancy, per-run read counters
with writes pinned at zero, and explicit non-claims: no SMM immutability, no
inherited encryption state, and no ownership or isolation conclusion.
`SvmLockKey`, `SMM_KEY`, the AVIC doorbell, and every other MSR are never read.

The physical schema-v5 result exposed two defects in that software contract.
The collector decoded `SMMMask[TMTypeDram]` from bits 16:15 instead of the PPR's
bits 14:12, and whole-object equality incorrectly treated thread-scoped
`SMM_BASE` as configuration state that must match the BSP. The host verifier
also initially truncated the full `SMMAddr`/`SMMMask` TSeg field width. The
schema-v6 implementation corrects all three issues while preserving each raw
per-thread `SMM_BASE` and excluding only that field from the configuration-
equality projection. PAT equality remains architecturally required and MTRR
equality remains a conservative proxy for equivalent memory typing. Raw HWCR
and the core-scoped SMM/address-routing registers remain useful conservative
cross-processor evidence, not architectural validity invariants merely because
they were observed on every processor.

The corrected slice is still intended to retire the
`smm-lock-and-ppr-specific-msrs` and
`mtrrs-iorrs-tom-tom2-and-mmio-apertures` blockers from the record.

Unavailable leaves and failed firmware protocols are represented by explicit
status objects. They are never replaced by zero, `false`, an empty successful
record, or a qualification decision. AMD-specific decodes on another CPU vendor
are `not-applicable-vendor`; that vendor's raw extended leaves remain preserved
without assigning AMD meaning to reserved bits. The sink record includes the
Block I/O media ID used for the repeated same-medium checks.

The EFI application prints a `STARTING` banner. A handled failure prints its
status and, for parser failures, the typed ACPI source/error, then keeps the
screen visible for 20 seconds before returning to firmware. It does not create
a failure file or modify an existing evidence record.

## Target-profile binding and reviewed cold-boot capture

First verify the immutable M0a target-profile bundle, build both EFI artifacts,
and run the PE/instruction safety gate:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\target-profile\verify-bundle.ps1 `
    -BundleDirectory .\target\evidence\target-profile-20260806T195311767Z

powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\target\evidence\target-profile-20260806T195311767Z\verify-bundle.ps1 `
    -BundleDirectory .\target\evidence\target-profile-20260806T195311767Z

cargo build-m0b-probe
cargo build-dxe

powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\check-efi-safety.ps1 `
    -ProbeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-m0b-probe.efi `
    -DxeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-dxe.efi
```

Then select an existing, clean removable-volume root. Confirm the drive letter
and contents manually. Do not use a fixed-disk root or a subdirectory, and do
not remove or replace an existing `\EFI\BOOT\BOOTX64.EFI` or
`\svmvisor-m0b-target.txt` as part of this preparation. The preparation tool
validates both M0a verifiers, hashes `manifest.json`, refuses either destination
collision, and writes this exact binding marker:

```text
svmvisor-m0b-target-v1
<lowercase SHA-256 of the M0a manifest.json>
```

On the current lab host, the completed schema-v3 raw record remains finalized
as `target/evidence/m0b-probe-20260807T024628-000000000`, now preserved as
historical evidence. Its media files were preserved before schema-v4 staging
under
`target/evidence/removable-media-preservation-20260807T025543720Z`. The
one-shot cold-boot schema-v4 capture is complete. Its returned media files were
subsequently preserved and hash-verified before schema-v5 staging under
`target/evidence/removable-media-preservation-20260808T222640423Z`. The v4 EFI
SHA-256 is
`10ebafbfcba066e6bd0d80a289149b947c3bf65364e5927261cd45b698971efd`,
and its single returned raw record was finalized without modification as
`target/evidence/m0b-probe-20260807T050309-000000000`. The schema-v5 EFI
SHA-256 was
`4ff897015fbeb3989cc8dc465c9c18b00a0bfe58f63a13e631f9e239384eff4f`.
At `2026-08-08T22:34:48.382Z`, the user explicitly authorized exactly one
cumulative schema-v5 record-only cold boot on the bound BIOS F7 machine. That
boot completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T003925-000000000.json`, with 24/24 processor coverage,
1,008 allowlisted MSR reads, and zero MSR writes. Finalization stopped because
of the schema-v5 collector and contract defects described above. The immutable
result and capture-time artifacts are preserved under
`target/evidence/rejected-m0b-probe-20260809T003925-000000000`; this is rejected
diagnostic evidence, not a finalized or canonical bundle. The one-shot boot
authorization was consumed.

The following command is retained only as historical tooling documentation. No
further M0b preparation or boot is authorized for this frozen target; the V6
medium and its one-shot decision are consumed. A different target or schema
would require a new review and authorization before using this procedure:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\prepare-media.ps1 `
    -TargetProfileBundle .\target\evidence\target-profile-20260806T195311767Z `
    -ProbeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-m0b-probe.efi `
    -RemovableMediaRoot E:\
```

Preparation stops after creating those two files; it does not boot the target.
Do not rerun it against the completed schema-v6 medium or boot that medium
again. The historical capture policy required BIOS `F7`, a real cold power-on,
manual power-cycle recovery, and the exact
[`f7-mp-services-audit.md`](f7-mp-services-audit.md) boundary. It authorized no
FPGA delivery, SVM enablement, or `VMRUN`. The finalizer copies rather than edits
a raw record and refuses an existing output directory.
The rejected schema-v5 boot cannot be retried under its consumed authorization;
schema v6 received a separate explicit review and one-shot boot decision at
`2026-08-09T04:02:42.5037164Z`. The completed schema-v6 boot consumed that
decision; it permits no retry.

Treat both bundles as sensitive. Processor hardware IDs, CPU/firmware
fingerprints, physical addresses, and table roots can identify the lab system.

## Current canonical evidence

The only canonical M0b capture is the untouched physical schema-v4 bundle
`target/evidence/m0b-probe-20260807T050309-000000000`. Its raw JSON SHA-256 is
`ade24113dfa6ddfee03ee82fafc2d03e81d6c1d89c03095881a77fbcfd70859d`.
The unchanged `manifest.json` SHA-256 is
`1a8954379f99f25b47ff47a58e6a0bbfd440ba7d4220e133aab0d00eac70490c`;
its preserved probe EFI SHA-256 is
`10ebafbfcba066e6bd0d80a289149b947c3bf65364e5927261cd45b698971efd`,
and its M0a manifest binding is
`c4fa28c1ba43aabd1fb67697706ddd41d6a4e984be5745a939720029c90a192c`.
The record contains 24 enabled, healthy processor observations; MP counts,
callback identities, capability CPUID, and `VM_CR` are all consistent. Its live
AMD-IOMMU slice derived exactly one unit from the same-run IVRS and MCFG (PCI
`0000:00:00.2`, capability offset `0x40`, MMIO base `0xF7600000`), observed the
MMIO-enabled, base-locked PCI capability with live EFR/EFR2 matching the
preferred Type `11h`/`40h` IVRS images, kept two stable configuration
snapshots, and found the IOMMU globally disabled at this UEFI observation
point: `IommuEn`, command/event logging, and run status were all clear. It used
18 bounded PCI reads (64 bytes) and 17 bounded MMIO reads (136 bytes), with
zero PCI/MMIO writes, no direct ECAM/MMIO or `CF8/CFC` access, and no
configured-pointer dereference. Live capability and global state were observed;
ownership, requester DMA and interrupt-remapping isolation, and PCI/slot
isolation were not proven and remain `false`. The previous schema-v3 canonical
bundle, schema-v2 bundle, schema-v1 bundle, and retry bundles remain preserved
historical evidence but noncanonical. Promotion rewrites no raw record, and all
versions remain separately verifiable.

The one authorized schema-v5 system-register capture was physically completed,
but it was not finalized or promoted. It is preserved under
`target/evidence/rejected-m0b-probe-20260809T003925-000000000` because its
derived `TMTypeDram` value and cross-processor equality contract were defective;
the preserved raw MSRs remain diagnostic only. Corrected schema-v6 software
passes 98 locked workspace tests, 93 verifier-regression invocations, release
builds, and the 42-site instruction gate. Its 315,392-byte EFI SHA-256
is `1b394c5222df1df9099ad16bb5da8eb7c62cf4b56267237ccb2d438489414fbe`.
The exact tested bytes are preserved under
`target/evidence/m0b-probe-v6-software-preflight-20260808T232024707Z`.
The user authorized replacement of the consumed schema-v5 payload and exactly
one cumulative record-only schema-v6 cold boot at
`2026-08-09T04:02:42.5037164Z`. The boot completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T151657-000000000.json`, with 24/24 processor coverage,
true identity/CPUID/`VM_CR`/system-register consistency aggregates, 1,008 MSR
reads, and zero writes. Strict finalization and both current and copied
verifiers pass at `target/evidence/m0b-probe-20260809T151657-000000000`. The
raw SHA-256 is
`c7c71ab7d3af18021326e9da4ff3d4eb9edb78e80d97ab9f87948a0cefc44384`
and manifest SHA-256 is
`58d957d92744e457ce9c39c48b5ecd2ff058f84492793deb909d7e590901c89b`.
The schema-v4 bundle above remains canonical pending a separate explicit
promotion decision. Both one-shot authorizations are consumed and permit no
retry.

## Remaining derived gates after the frozen schema-v6 candidate

The corrected system-register slice still leaves MP Services AP-dispatch
pre-measurement control-state preservation unproven. The physical v4 record supplies bounded live
AMD-IOMMU capability, enable, and global-state evidence — PCI capability
lock and MMIO-base agreement, live EFR/EFR2 agreement with the preferred IVHD
images, stable snapshots, and a globally disabled IOMMU at the observation
instant — and the implemented v5 slice records the target-PPR SMM, MTRR,
IORR, and TOM/TOM2 raw register state per processor in rejected diagnostic
evidence. The finalized schema-v6 candidate corrects those derived fields and
the comparison contract while preserving every raw observation. The two
inventory IDs `smm-lock-and-ppr-specific-msrs` and
`mtrrs-iorrs-tom-tom2-and-mmio-apertures` are candidate-retired but remain open
in the canonical ledger until explicit promotion. The raw record intentionally
retains seven blocker IDs:
DTE/requester ownership and PCI/slot isolation; inherited memory-encryption
state; Secure Boot databases, option-ROM policy, and the TCG log;
boot-driver/Sysprep/recovery/hotkey namespace; firmware event ordering; the
direct watchdog and durable attempt lease; and MP Services pre-measurement
control-state preservation. The implemented read-only IOMMU work starts from enumerated
IVRS/IVHD instances and never from a hardcoded diagram address, and the
implemented system-register work reads only the allowlisted MSRs cited to the
pinned APM and PPR.

A removable UEFI application normally starts after the boot manager has already
signaled `ReadyToBoot`; it cannot prove
`ReadyToBoot -> AfterReadyToBoot -> BeforeExitBootServices/ExitBootServices`.
That proof needs a separately delivered resident DXE trace driver, preallocated
callbacks, and a diagnostic sink that remains valid at those phases. The
current Squirrel is not an authorized delivery mechanism.

This does not create a schema-v7 USB task. M0b collection is complete and frozen
even though its raw qualification field remains blocked. The seven IDs are
assigned to their first consumers in the
[first-light plan](first-light-plan.md): three are handled by one resident DXE
trace, encryption and AP-state evidence become walls before host/SVM work, the
watchdog/lease becomes a wall before persistent `VMRUN`, and IOMMU/PCI ownership
becomes a wall before untrusted execution. Physical record-only dispatch has its
own endpoint, recovery, observability, emulator, and explicit-authorization
gate.
