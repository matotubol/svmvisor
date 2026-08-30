# svmvisor

A bare-metal AMD SVM Type-1 hypervisor delivered as a UEFI DXE driver from a
PCI option ROM. Its intended use is controlled malware analysis on systems you
own or are authorized to inspect.

The platform-inventory prelude is complete through a finalized, dual-verified
schema-v6 M0b promotion candidate; schema v4 remains canonical pending an
explicit promotion decision. Implementation is now **pre-first-light**. The DXE
driver remains buildable scaffolding only: it does not yet register lifecycle
callbacks, reserve persistent memory, enable SVM, or execute `VMRUN`. The current
transitional Squirrel image remains requester/DMA-capable and is forbidden for
physical svmvisor bring-up. See the [first-light plan](docs/first-light-plan.md)
for the one current execution path and the
[development gate ledger](docs/development-gate-ledger-v1.json) for current
machine-readable status.

## Layout

```text
svmvisor/
├── crates/
│   ├── dxe/          # UEFI lifecycle; record-only callbacks first
│   ├── hypervisor/   # no_std persistent host and VM-exit runtime
│   └── m0b-probe/    # frozen removable-media UEFI inventory application
├── firmware/
│   └── squirrel/     # FPGA source boundary, Vivado build, OpenOCD flash
├── tools/
│   ├── m0b-probe/    # Removable-media preparation and evidence verification
│   ├── rompack/      # Host-side .efi to PCI option-ROM packer
│   └── target-profile/ # Read-only Milestone 0a evidence collector
├── docs/             # existing specifications and vendor reference material
└── .cargo/           # UEFI target/linker configuration
```

The boundary is deliberate:

- `svmvisor-dxe` may depend on `uefi` and owns the firmware lifecycle.
- `svmvisor-hypervisor` must not depend on UEFI. CPU setup, VMCB, NPT, and
  VM-exit handling will begin as modules in this crate rather than separate
  crates.
- New top-level crates should be added only when a component has a genuinely
  independent build target or test boundary.

## Build

Check the host-independent hypervisor library:

```console
cargo check --package svmvisor-hypervisor
```

Build the DXE image:

```console
cargo build-dxe
```

Build the separate record-only M0b inventory application:

```console
cargo build-m0b-probe
```

The images are written to:

```text
target/x86_64-unknown-uefi/release/svmvisor-dxe.efi
target/x86_64-unknown-uefi/release/svmvisor-m0b-probe.efi
```

The Rust target defaults to a UEFI application. The DXE package's build script
sets only `svmvisor-dxe.efi` to `efi_boot_service_driver`; the removable M0b
probe keeps the application subsystem. Its scope, evidence binding, and run
procedure are documented in [`docs/m0b-probe.md`](docs/m0b-probe.md).

M0b schema v2 adds a bounded ACPI 2.0 RSDP, RSDT/XSDT directory, complete raw
APIC/MCFG/IVRS/FACP captures, and a MADT-to-MP-Services enabled-processor-ID
comparison. Schema v3 identifies the BSP through MP Services and measures every
enabled, healthy AP through a blocking, sequential `StartupThisAP` callback with
`TimeoutInMicroseconds == 0` (no MP Services timeout). Each callback performs
only maximum-leaf-gated CPUID, an AP-safe `WhoAmI`, and the same conditional
read-only `VM_CR` access. The UEFI MP dispatch mechanism itself is opaque and
may use INIT/SIPI or reset, so schema
v3 explicitly describes post-dispatch state and does not prove that pre-dispatch
`VM_CR` was preserved. The [exact F7 MP Services audit](docs/f7-mp-services-audit.md)
establishes a narrower target policy: a cold-boot F7 dispatch uses the parked-
MWAIT signal path, while a finite timeout can invoke reset recovery. Schema v3
therefore permits only the reviewed cold-boot path with timeout zero; S3/resume
and other BIOS builds remain outside that decision. No slice reads PCI
configuration or IOMMU MMIO registers through schema v3. Schema v4 adds the
separately reviewed, IVRS/MCFG-derived live AMD-IOMMU read slice: bounded
`EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL` PCI and MMIO reads, two stable configuration
snapshots, one status sample, and range validation without dereferencing any
configured pointer. It performs no PCI/MMIO write or direct ECAM/MMIO access
and leaves ownership, requester DMA/interrupt remapping, and PCI isolation
unclaimed. Schema v5 adds the separately reviewed, per-processor
system-register slice under [`docs/m0b-msr-read-policy.md`](docs/m0b-msr-read-policy.md):
41 new allowlisted read-only `RDMSR` sites (SMM configuration and lock state,
IORRs, `TOP_MEM`/`TOM2`, SYS_CFG, HWCR, MTRRs, and PAT) measured on every
enabled processor through the same blocking, timeout-zero MP Services dispatch,
with no MSR writes. Its one authorized physical capture completed, but exposed
an incorrect `SMMMask[TMTypeDram]` decode and a contract that incorrectly
required thread-scoped `SMM_BASE` to be equal across processors. That immutable
result is preserved as rejected diagnostic evidence. Corrected schema-v6 / collector
0.6.0 software is implemented and locally release-gated. Its separately
authorized physical capture completed and was strictly finalized and
dual-verified as a noncanonical promotion candidate. It retains
every per-thread `SMM_BASE` but excludes only that field from configuration
equality. PAT and MTRR consistency remain checked; HWCR and core-scoped
register equality are conservative evidence rather than architectural validity
invariants. Neither version claims SMM immutability or inherited encryption
state. Existing v1 through v4 bundles remain independently verifiable.

The single current canonical M0b capture is the untouched, physically collected
schema-v4 bundle `target/evidence/m0b-probe-20260807T050309-000000000`. Its raw
JSON SHA-256 is
`ade24113dfa6ddfee03ee82fafc2d03e81d6c1d89c03095881a77fbcfd70859d`,
and its manifest SHA-256 is
`1a8954379f99f25b47ff47a58e6a0bbfd440ba7d4220e133aab0d00eac70490c`.
All 24 enabled processors were measured and the identity, capability CPUID, and
`VM_CR` consistency aggregates are true. Its IVRS/MCFG-derived live AMD-IOMMU
slice observed one unit at PCI `0000:00:00.2` (capability offset `0x40`, MMIO
base `0xF7600000`): the PCI capability was MMIO-enabled and base-locked, live
EFR/EFR2 matched the preferred IVRS Type `11h`/`40h` images, both configuration
snapshots were stable, and the IOMMU was globally disabled at the UEFI
observation point. The slice used 18 bounded PCI reads (64 bytes) and 17
bounded MMIO reads (136 bytes), with zero PCI/MMIO writes, no direct ECAM/MMIO
or `CF8/CFC` access, and no configured-pointer dereference. Live capability and
global run state were observed, but ownership, requester DMA and interrupt-
remapping isolation, and PCI/slot isolation were not proven; every such claim
remains `false` and qualification remains blocked.

The schema-v4 collector, schema, and verifier for the IVRS-derived live
AMD-IOMMU inspection are implemented under the
[reviewed PCI/MMIO read policy](docs/m0b-iommu-read-policy.md). The physical
schema-v4 capture has been returned from the bound `D:\` removable medium,
strictly finalized without modifying the raw bytes, and verified with both the
current and copied verifiers. The previous schema-v3 canonical capture,
schema-v2 capture, schema-v1 capture, and retries remain preserved historical
evidence: noncanonical, untouched, and independently verifiable.

The schema-v5 EFI for the per-processor system-register slice was prepared
under the [reviewed MSR read policy](docs/m0b-msr-read-policy.md), SHA-256
`4ff897015fbeb3989cc8dc465c9c18b00a0bfe58f63a13e631f9e239384eff4f`.
The user authorized exactly one cumulative record-only cold boot at
`2026-08-08T22:34:48.382Z`; it completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T003925-000000000.json` with 24/24 processor coverage,
1,008 allowlisted MSR reads, and zero writes. Finalization stopped because the
collector decoded `SMMMask[TMTypeDram]` from bits 16:15 instead of 14:12 and the
comparison contract treated the PPR's thread-scoped `SMM_BASE` as uniform
configuration. The immutable result and capture-time artifacts are preserved
under `target/evidence/rejected-m0b-probe-20260809T003925-000000000`. They are
not a finalized bundle, are not canonical, and must not be promoted. Corrected
schema-v6 / collector 0.6.0 software now passes 98 locked workspace tests, the
full 93-invocation verifier regression matrix, both V4/M0a verifier pairs,
release builds, and the 42-site EFI instruction gate. The new 315,392-byte EFI
SHA-256 is
`1b394c5222df1df9099ad16bb5da8eb7c62cf4b56267237ccb2d438489414fbe`;
the exact tested bytes are preserved under
`target/evidence/m0b-probe-v6-software-preflight-20260808T232024707Z`.
The user authorized replacement of the consumed schema-v5 payload and exactly
one cumulative record-only schema-v6 cold boot at
`2026-08-09T04:02:42.5037164Z`. That boot completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T151657-000000000.json`: all 24 enabled processors were
observed, all identity/CPUID/`VM_CR`/system-register consistency aggregates are
true, and the run performed 1,008 allowlisted MSR reads with zero writes. The
immutable record was finalized and both verifier copies pass at
`target/evidence/m0b-probe-20260809T151657-000000000`; its raw SHA-256 is
`c7c71ab7d3af18021326e9da4ff3d4eb9edb78e80d97ab9f87948a0cefc44384`
and manifest SHA-256 is
`58d957d92744e457ce9c39c48b5ecd2ff058f84492793deb909d7e590901c89b`.
The authorization and staging records remain in the preflight directory. Both
schema-v5 and schema-v6 one-shot authorizations are consumed and permit no
retry. Schema v4 remains canonical until a separate explicit promotion review.

Squirrel gateware setup and commands are documented in
[`firmware/squirrel/README.md`](firmware/squirrel/README.md). Vivado 2026.1 is
used for the FPGA build. The OpenOCD configuration is retained for recovery and
eventual manifest-gated programming through the integrated update port; the
current requester-capable output must not be flashed.

## Current step: build the safe delivery substrate

Review the finalized, dual-verified schema-v6 candidate and decide explicitly
whether to promote it. Promotion would retire its two now-collected inventory
items in the canonical ledger, but would not qualify the target or authorize
another boot, SVM enablement, or `VMRUN`.

The current implementation task is **not SVM EFI code**. First make flashing
manifest-gated and build the pinned, non-enumerating recovery image. Then
implement and verify the completion-only Squirrel endpoint with the inherited
PCILeech requester, DMA, raw-TLP command, and FT601 paths removed; add the BAR0
phase journal and read-only JTAG snapshot next.

In parallel, build the QEMU/OVMF harness and one resident, record-only DXE trace
covering Secure Boot/TCG inventory, the complete boot namespace, and firmware
event ordering. No enumerating candidate is physically flashed until the
Milestone 1, Milestone 2, and emulated-DXE gates pass. The resulting physical
first-light run records and returns; it enables no SVM, executes no `VMRUN`,
installs no persistent host, and runs no untrusted code.

The concise, authoritative near-term sequence is the
[first-light plan](docs/first-light-plan.md). Development rules that survived
the retired M0 starting document are in [CONTRIBUTING.md](CONTRIBUTING.md).
Verify that its machine-readable state still matches the immutable bundles with:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\verify-development-gate-ledger.ps1
```

The selected ReadyToBoot-to-VMRUN architecture, milestone gates, debugging
contract, recovery rules, Windows continuation, and INIT/SIPI plan are documented
for the later walls in
[`docs/minimal-baremetal-bringup-roadmap.md`](docs/minimal-baremetal-bringup-roadmap.md).
