# M0b probe evidence tools

> **Frozen target status:** the current target's schema-v6 run is complete and
> its one-shot authorization is consumed. Do not prepare or boot that medium
> again and do not create schema v7 for the seven later gates. These tools remain
> documented for verification and historical reproducibility; the current work
> starts at the [first-light plan](../../docs/first-light-plan.md).

These Windows PowerShell 5.1 tools prepare one removable medium for the
record-only `svmvisor-m0b-probe`, then preserve one returned JSON record as a new,
hash-manifested evidence bundle. Schema v1 records the original CPU/UEFI slice;
schema v2 adds the bounded ACPI slice; schema v3 adds cardinality-bounded
per-processor CPUID/`VM_CR` consistency; schema v4 adds the bounded,
IVRS/MCFG-derived read-only live AMD-IOMMU slice; schema v5 adds the reviewed
per-processor system-register slice (41 new allowlisted read-only `RDMSR`
sites). The one authorized v5 physical result is preserved as rejected
diagnostic evidence after exposing collector and comparison-contract defects.
Corrected schema-v6 / collector 0.6.0 support is implemented and locally
release-gated. Its separately authorized cold-boot capture completed and was
strictly finalized and dual-verified as a noncanonical promotion candidate.
Finalized v1 through v4 bundles remain verifiable. The tools do not
format media, alter firmware or boot variables, qualify the target, authorize
launch, or rewrite either an M0a or M0b raw bundle.

## 1. Build the probe

From the repository root, build the dedicated removable-media application:

```powershell
cargo build --release `
    --target x86_64-unknown-uefi `
    --package svmvisor-m0b-probe
```

The expected image is
`target\x86_64-unknown-uefi\release\svmvisor-m0b-probe.efi`.

Build the DXE type-check companion and run the fail-closed PE/instruction gate
before preparing media:

```powershell
cargo build-dxe

powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\check-efi-safety.ps1 `
    -ProbeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-m0b-probe.efi `
    -DxeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-dxe.efi
```

The checker requires AMD64 PE32+ images, the correct EFI subsystems and
relocations, and a disassembly containing only the reviewed read-only `RDMSR`
site set — the AMD `VM_CR` site plus the 41 system-register sites from
[`docs/m0b-msr-read-policy.md`](../../docs/m0b-msr-read-policy.md), each pinned
to its literal address — but no state-changing SVM/MSR/control-register/port-I/O
instructions. Each applicable site may execute at most once on each measured
processor.
Its `llvm-objdump` Intel-syntax option is assembly notation only; it is not an
Intel platform assumption.

This checker covers application instructions, not firmware behavior. Every
physical run additionally requires a reviewed target-platform MP Services
policy and a separate explicit boot decision. The current Gigabyte F7
cold-boot policy is recorded in
[`docs/f7-mp-services-audit.md`](../../docs/f7-mp-services-audit.md); it excludes
S3/resume and requires timeout zero.

## 2. Prepare removable media

Supply an existing five-file M0a target-profile bundle, the exact built EFI,
and an existing removable-volume root. The root must be reported by Windows as
`DriveType.Removable`; a subdirectory is rejected.

For the current lab state, the completed schema-v4 raw record is finalized as
`target\evidence\m0b-probe-20260807T050309-000000000`. The previous schema-v3
record remains finalized as
`target\evidence\m0b-probe-20260807T024628-000000000`, preserved as historical
evidence, and its media files are preserved under
`target\evidence\removable-media-preservation-20260807T025543720Z`. Before
schema-v5 staging, the returned schema-v4 EFI, marker, and raw record were
preserved and hash-verified under
`target\evidence\removable-media-preservation-20260808T222640423Z`; the v4 EFI
SHA-256 is
`10ebafbfcba066e6bd0d80a289149b947c3bf65364e5927261cd45b698971efd`,
and the returned raw record exactly matches the finalized `raw-probe.json`.
The reviewed schema-v5 EFI SHA-256 was
`4ff897015fbeb3989cc8dc465c9c18b00a0bfe58f63a13e631f9e239384eff4f`
and the user explicitly authorized exactly one cumulative schema-v5
record-only cold boot at `2026-08-08T22:34:48.382Z`. It completed on 2026-08-09
and returned `svmvisor-m0b-20260809T003925-000000000.json` with 24/24 processor
coverage, 1,008 allowlisted MSR reads, and zero MSR writes. Finalization stopped
after the returned target exposed the schema-v5 defects. The immutable result
and capture-time artifacts are preserved under
`target\evidence\rejected-m0b-probe-20260809T003925-000000000`; they are not a
finalized bundle and must not be promoted. The one-shot authorization is
consumed. Corrected schema-v6 / collector 0.6.0 software passes the locked
workspace tests, verifier regressions, release build, and EFI instruction gate.
Its 315,392-byte EFI SHA-256 is
`1b394c5222df1df9099ad16bb5da8eb7c62cf4b56267237ccb2d438489414fbe`;
the tested bytes are preserved under
`target\evidence\m0b-probe-v6-software-preflight-20260808T232024707Z`.
The user authorized replacing the consumed v5 payload and exactly one
cumulative record-only v6 cold boot at `2026-08-09T04:02:42.5037164Z`. The
boot completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T151657-000000000.json`, with 24/24 processor coverage,
true consistency aggregates, 1,008 MSR reads, and zero writes. Strict
finalization and both verifier copies pass at
`target\evidence\m0b-probe-20260809T151657-000000000`; raw SHA-256 is
`c7c71ab7d3af18021326e9da4ff3d4eb9edb78e80d97ab9f87948a0cefc44384`
and manifest SHA-256 is
`58d957d92744e457ce9c39c48b5ecd2ff058f84492793deb909d7e590901c89b`.
The one-shot authorization is consumed and permits no retry. Schema v4 remains
canonical until a separate explicit promotion decision.

The following is a generic preparation example only for a different target or
schema that has received a new review and authorization. The completed
schema-v6 medium now contains its returned record; do not rerun this command for
that capture and do not boot the medium again.

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\prepare-media.ps1 `
    -TargetProfileBundle .\target\evidence\target-profile-<run-id> `
    -ProbeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-m0b-probe.efi `
    -RemovableMediaRoot E:\
```

Preparation first runs both the current repository M0a verifier and the
verifier copied into the M0a bundle. It refuses any destination collision, then
creates only these selected media files:

- `\EFI\BOOT\BOOTX64.EFI`, copied byte-for-byte from `ProbeEfiPath`;
- `\svmvisor-m0b-target.txt`, the probe authorization and profile-binding
  marker.

The marker is exactly 88 ASCII bytes, with LF (not CRLF) and no BOM or trailing
data:

```text
svmvisor-m0b-target-v1\n
<lowercase SHA-256 of the exact M0a manifest.json bytes>\n
```

The marker is written only after the EFI copy succeeds. The script never
formats the medium and never overwrites either selected file.

Both host entry points also parse the supplied PE/COFF header and require an
AMD64 PE32+ `EFI_APPLICATION` image with a non-empty base-relocation directory.
This catches an accidental DXE driver or unrelated artifact; it is a type check,
not code attestation.

Preparation stops after those two files are created. It does not boot the
medium. If either file already exists, retain it and stop for review; do not
delete, truncate, or bypass the script's overwrite refusal. A reviewed capture
must begin from cold power-on on the exact M0a-bound F7 target, with manual
power-cycle recovery available for an indefinitely stalled AP. It must return
exactly one
committed root filename of the form
`svmvisor-m0b-YYYYMMDDTHHMMSS-NNNNNNNNN.json`; a `.partial`, renamed file, or
additional candidate is not evidence.

## 3. Finalize one raw result

Copy the single committed JSON result without editing or renaming it. Finalize
it alongside the same M0a bundle and the exact EFI bytes used for the run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\finalize-evidence.ps1 `
    -RawEvidencePath E:\svmvisor-m0b-<timestamp>-<nanoseconds>.json `
    -TargetProfileBundle .\target\evidence\target-profile-<run-id> `
    -ProbeEfiPath .\target\x86_64-unknown-uefi\release\svmvisor-m0b-probe.efi `
    -OutputDirectory .\target\evidence\m0b-probe-<run-id>
```

`OutputDirectory` must not exist and its parent must already exist. The
finalizer validates both M0a verifiers, strict false-Boolean safety fields, the
M0a-manifest binding, removable-sink facts, and the raw filename. It builds and
verifies a private staging directory before atomically renaming it to the
requested output path.

The finalized bundle contains exactly eight immediate files and no
subdirectories:

- `raw-probe.json`: unchanged raw UEFI JSON bytes;
- `target-profile-manifest.json`: unchanged M0a manifest bytes, allowing the
  profile binding to be recomputed independently;
- `svmvisor-m0b-probe.efi`: unchanged supplied EFI bytes;
- exactly one schema selected by the raw record's integer `schema_version`:
  `m0b-probe-v1.schema.json`, `m0b-probe-v2.schema.json`,
  `m0b-probe-v3.schema.json`, `m0b-probe-v4.schema.json`, or
  `m0b-probe-v6.schema.json` (schema v5 is withdrawn from finalization);
- `prepare-media.ps1`;
- `finalize-evidence.ps1`;
- `verify-bundle.ps1`;
- `manifest.json`, which hashes and sizes the preceding seven files.

The raw UEFI result cryptographically binds the exact M0a manifest, but it does
not self-authenticate the EFI image that produced it. The finalized manifest
preserves the supplied EFI hash; custody of the prepared medium and confirmation
that `ProbeEfiPath` is the image that was booted remain required provenance.
Schema-v6 finalization support is part of the current remediation and is not a
completed release claim here.

## Verification and tests

Recheck a finalized bundle without the source repository or the M0a files:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\verify-bundle.ps1 `
    -BundleDirectory .\target\evidence\m0b-probe-<run-id>
```

The verifier dispatches strictly on schema version and is self-contained for
the manifest, binding, exact structural shape, CPUID cross-field decoding,
media safety facts, and fail-closed Boolean contract. For v2 it also recomputes
the RSDP/SDT checksums, hashes and decoded witnesses; validates RSDT/XSDT
directories and address arithmetic; decodes bounded MADT, MCFG, IVRS and FADT
content; and independently checks MADT/MP enabled-ID consistency. For v3 it
also recomputes enabled/healthy coverage, BSP and APIC identity, every masked
raw-CPUID comparison, exact raw/status `VM_CR` equality, and all reported
consistency witnesses, including each callback's actual `WhoAmI` result and the
matching pre- and post-dispatch MP Services enumerations. For v4 it also
recomputes the IVRS-derived locator, ECAM and memory bindings, the bounded PCI
capability chain, stable PCI/MMIO snapshots, the allowlisted read plan and
counters, and the configured-range decodes, with every ownership and isolation
claim pinned false. The historical v5 contract also recomputes every
system-register decoded field from each raw MSR value, the per-processor gate
outcomes and its whole-object cross-processor equality, the run-wide
read-counter totals, and the `system_registers_consistent` aggregate, with
writes pinned at zero. That contract is not suitable for promotion: the
collector decoded `SMMMask[TMTypeDram]` from bits 16:15 rather than 14:12 and
the equality projection incorrectly included thread-scoped `SMM_BASE`.
Schema-v6 remediation corrects the decode and excludes only `SMM_BASE` from
configuration equality. PAT and MTRR consistency remain checked; exact HWCR
and core-scoped-register equality is conservative evidence rather than an
architectural validity invariant. Schema v3 treats
`VM_CR` as post-dispatch state: the callback issues no control-state write, but
the opaque firmware dispatch may use INIT/SIPI or reset and pre-dispatch
preservation is not proven. Formal consumers should additionally validate
`raw-probe.json` against the one copied schema with a JSON Schema Draft 2020-12
implementation.

MADT records with neither `Enabled` nor `OnlineCapable` set remain fully
validated and recorded, but are excluded from APIC/x2APIC-ID collision checks
and the enabled-ID comparison. Usable records retain strict ID uniqueness;
processor UID uniqueness remains global.

V2 records the following fail-closed limits: 4 KiB per RSDP, 1 MiB per SDT,
4 MiB cumulative ACPI bytes, 64 UEFI configuration-table entries, 256 entries
per root, 256 unique root pointers, 512 MADT entries, 256 MADT processor
entries, 256 MCFG allocations, 256 IVRS blocks, 4096 IVHD device entries, and a
16 MiB raw-JSON preparse/render cap. The probe permits each physical ACPI read
only when the whole checked range lies within one same-run
`EfiACPIReclaimMemory` or `EfiACPIMemoryNVS` memory descriptor and below the
CPUID-reported physical-address width. A descriptor marked `EFI_MEMORY_RP` is
rejected before any copy.

The v2 IVRS record is a firmware description pinned to AMD IOMMU publication
48882 rev. 3.11 (April 2026), local PDF SHA-256
`f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22`.
It preserves IVHD DeviceID/BDF encoding, PCI capability offset, MMIO base,
segment, device coverage, and Type `11h`/`40h` extended-feature images. The
schema-v2 probe performs no PCI configuration or IOMMU MMIO reads.
Consequently, table
presence and those fields do not prove live support, enablement, ownership, DMA
remapping, or isolation; the corresponding claims remain strictly `false`.

Run the checked-in negative regressions under Windows PowerShell 5.1:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\m0b-probe\tests\run-negative-tests.ps1
```

The established suite makes 93 verifier invocations: five accepted baselines,
12 accepted semantic variants, and 76 expected rejections across schemas v1
through v6. It explicitly withdraws v5 and covers the corrected v6
`TMTypeDram` decode, full-width TSeg fields, distinct thread-specific
`SMM_BASE`, exact BSP copying, honest non-SMM inconsistency, witnesses,
aggregates, counters, blockers, gates, and zero-write contract.
They also retain valid UID-distinct disabled
MADT placeholders that share interrupt-controller ID 0.
They then require rejection of v1 authorization/binding/duplicate-property/
reserved-bit/bundle mutations and v2 duplicate enabled or online-capable MADT
IDs, malformed MADT length, truncated FADT, bad checksum, MCFG address overflow
or misalignment, duplicate XSDT pointer, falsified topology, false IOMMU
ownership, misaligned IVHD bases, reserved fixed or variable IVHD entry types,
Type `40h` fixed-after-variable ordering, and invalid F0h UID format/presence
mutations. V3 cases additionally reject a finite AP timeout, missing or remapped
AP observations, CPUID/APIC/`VM_CR` mismatches with falsified witnesses, and
false aggregate consistency claims.
V4 cases additionally reject locator, ECAM, capability-chain, stable-snapshot,
read-counter, configured-range, ownership, and isolation tampering.
V5 cases additionally reject MSR-write claims, falsified register decodes,
falsified read counters and match witnesses, false aggregates, tampered blocker
lists, missing observations, out-of-bound `VCNT`, and IORR gate violations.

Raw M0a and M0b evidence contains a stable CPU, firmware, topology, and memory
fingerprint. Treat both bundles and the removable medium as sensitive lab data.
Every M0b raw record remains `qualification_status = "blocked"`; unavailable
records and the explicit blocker list are evidence of unresolved work, not
authorization to proceed.

The current canonical M0b capture is the untouched physical schema-v4 bundle
`target\evidence\m0b-probe-20260807T050309-000000000`, with raw JSON SHA-256
`ade24113dfa6ddfee03ee82fafc2d03e81d6c1d89c03095881a77fbcfd70859d`
and manifest SHA-256
`1a8954379f99f25b47ff47a58e6a0bbfd440ba7d4220e133aab0d00eac70490c`.
The previous schema-v3 canonical bundle, schema-v2 bundle, schema-v1 bundle,
and retry bundles remain preserved historical evidence, noncanonical, and
separately verifiable.

Schema v4 implements only the accesses in the
[reviewed live AMD-IOMMU read policy](../../docs/m0b-iommu-read-policy.md).
Its physical capture observed the IVRS-derived unit at PCI `0000:00:00.2`
(capability offset `0x40`, MMIO base `0xF7600000`) with an MMIO-enabled,
base-locked PCI capability, live EFR/EFR2 matching the preferred IVRS images,
stable configuration snapshots, and a globally disabled IOMMU at the UEFI
observation point — 18 bounded PCI reads (64 bytes), 17 bounded MMIO reads
(136 bytes), zero PCI/MMIO writes, and no direct ECAM/MMIO, `CF8/CFC`, or
configured-pointer access. The slice is record-only and cannot claim IOMMU
ownership or PCI isolation from global registers alone; every such claim
remains `false` and qualification remains blocked.

Schema v5 implements only the reads in the
[reviewed system-register read policy](../../docs/m0b-msr-read-policy.md):
41 new allowlisted, PE-audited read-only `RDMSR` sites covering the SMM
configuration and lock state, IORRs, `TOP_MEM`/`TOM2`, SYS_CFG, HWCR, MTRRs,
and PAT, measured per enabled processor with writes pinned at zero. Its one
authorized physical capture completed, but the collector's wrong
`TMTypeDram[16:15]` decode and the contract's inclusion of thread-scoped
`SMM_BASE` in configuration equality require rejection. The immutable record is
preserved under
`target\evidence\rejected-m0b-probe-20260809T003925-000000000`, while schema v4
remains canonical. Corrected schema-v6 software is locally release-gated; its
physical result is finalized and dual-verified as a noncanonical promotion
candidate. Both v5 and v6 one-shot authorizations are consumed and permit no
retry. The two intended inventory blockers are candidate-retired pending
explicit promotion. The slice is record-only and claims neither SMM
immutability nor inherited encryption state.
