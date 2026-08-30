# M0b live AMD-IOMMU read policy

Status: reviewed implementation contract for the next record-only M0b slice.
It authorizes only the bounded reads below. It does not authorize a PCI or MMIO
write, table or ring dereference, IOMMU ownership, SVM enablement, or `VMRUN`.

The architectural source is AMD publication 48882 rev. 3.11, preserved as
[`48882-3.11.pdf`](48882-3.11.pdf). PCI access uses
`EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL` as defined by the repository-local
[EDK II protocol header](vendor/edk2/MdePkg/Include/Protocol/PciRootBridgeIo.h).
There is no direct ECAM dereference, `CF8/CFC` fallback, device scan, or raw
volatile MMIO fallback.

## Canonical target witness

The schema-v3 capture
`target/evidence/m0b-probe-20260807T024628-000000000` (canonical when this
policy was reviewed; now preserved historical evidence under the promoted
schema-v4 canonical bundle) describes one IOMMU in
three IVHD forms (`10h`, `11h`, and `40h`). All three agree on:

- PCI segment `0`, DeviceID `0x0002`, decoded as BDF `00:00.2`;
- conventional PCI capability offset `0x40`; and
- IOMMU MMIO base `0x00000000f7600000`.

Type `11h` and `40h` also agree on EFR image
`0x246577efa2254afa` and EFR2 image `0x0000000000000000`.
The EFR image has `PCSup=1`, so AMD section 3.2 requires a 512-KiB-aligned
and 512-KiB-sized register aperture: `[0xf7600000, 0xf7680000)`.

The same-run MCFG allocation has segment `0`, buses `0..255`, and bus-zero
ECAM base `0xe0000000`. The derived function and capability witnesses are
`0xe0002000` and `0xe0002040`. MCFG base addresses are relative to bus zero;
the decoded interval for an allocation is therefore
`[base + (start_bus << 20), base + ((end_bus + 1) << 20))`. Both the ECAM
witnesses and the complete 512-KiB IOMMU aperture fall in same-run
`EfiMemoryMappedIO` descriptors that advertise `EFI_MEMORY_UC` and do not carry
`EFI_MEMORY_RP`.

These values are target evidence, not implementation constants. Every live
run must derive its units again from that run's validated IVRS.

## Unit derivation and preflight

1. Coalesce IVHD `10h`, `11h`, and `40h` records by
   `(segment, DeviceID, capability_offset)`. Require equal MMIO bases and equal
   Type `11h`/`40h` EFR images. Schema v4 supports exactly one resulting unit;
   zero or multiple unique units are unsupported rather than silently selected.
   A mismatch is compromised firmware evidence.
2. Decode DeviceID as `bus[15:8]`, `device[7:3]`, and `function[2:0]` with
   checked arithmetic. Retain every source IVHD type in the unit record.
3. Require exactly one MCFG allocation covering the unit's segment and bus.
   Derive ECAM witnesses with
   `base + (bus << 20) + (device << 15) + (function << 12) + register`, but do
   not dereference them.
4. Locate every `EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL` handle, retain only matching
   segments, and issue the exact IVRS-derived identity/capability reads below on
   each candidate. Accept exactly one full capability/base match. Open handles
   with `GET_PROTOCOL`; do not disconnect or bind a driver. Do not call the
   crate's generic `configuration()` resource-descriptor parser in this slice;
   same-run MCFG already supplies the bounded bus witness, and unknown firmware
   descriptor tags must not become a panic surface.
5. Before each possible access, require checked range arithmetic, an address
   below the CPUID physical-address limit, and same-run UEFI memory-map
   coverage. The ECAM witness and IOMMU aperture must be in
   `EfiMemoryMappedIO` descriptors that advertise `EFI_MEMORY_UC` and do not
   carry `EFI_MEMORY_RP`. Require 16 KiB initially; require the complete 512 KiB
   before access when the IVHD EFR image reports `PCSup=1`, and recheck that
   requirement against the live EFR.
6. Bound unique units, protocol handles, capability hops, and register records.
   Reject duplicate handles, cyclic capability lists, reserved encodings,
   arithmetic overflow, or an unstable configuration snapshot.

## PCI configuration read allowlist

Only `Pci.Read` is permitted.

| Offset | Width | Recorded purpose |
| --- | ---: | --- |
| `0x00` | 32 | Vendor and device ID |
| `0x04` | 32 | Command/status and Capability List status |
| `0x08` | 32 | Revision, programming interface, subclass, and class |
| `0x0e` | 8 | Header type |
| `0x34` | 8 | First conventional capability pointer |
| each bounded capability pointer | 16 | Capability ID and next pointer only |
| `cap+0x00` | 32 | IOMMU Capability Header |
| `cap+0x04` | 32 | IOMMU Base Address Low and `Enable` |
| `cap+0x08` | 32 | IOMMU Base Address High |
| `cap+0x0c` | 32 | IOMMU Range |
| `cap+0x10` | 32 | Miscellaneous Information 0, including `PAsize` |
| `cap+0x14` | 32 | Miscellaneous Information 1, only when `CapExt=1` |

Require PCI class `08h`, subclass `06h`, and programming interface `00h`.
The IVRS capability offset must be 4-byte aligned, leave room through `+0x14`,
and occur in the bounded, aligned, acyclic
conventional capability chain and identify `CapID=0x0f`, `CapType=3`. Rebuild
the live MMIO base from `cap+04h` and `cap+08h` and require exact IVRS
agreement. `cap+04h.Enable=1` means the capability block is locked until reset
and accepts MMIO accesses; it is not an ownership lock. If `Enable=0`, record
that PCI state and perform no MMIO read. PCI Status contains write-one-to-clear
fields and `Enable` is RW1S, but reads do not alter either; never write the
sampled dwords back.

## MMIO read allowlist

Only naturally aligned 64-bit `Memory.Read` operations through the selected
root bridge are permitted. This satisfies AMD section 3.4's power-of-two,
alignment, and maximum-width rules.

| Offset | Register | Gate |
| --- | --- | --- |
| `0x0000` | Device Table Base | MMIO enabled and base matched |
| `0x0008` | Command Buffer Base | same |
| `0x0010` | Event Log Base | same |
| `0x0018` | IOMMU Control | same; part of both snapshot passes |
| `0x0020` | Exclusion Base / Completion Store Base | same |
| `0x0028` | Exclusion Limit / Completion Store Limit | same |
| `0x0030` | Extended Feature Register | PCI `EFRSup=1` |
| `0x01a0` | Extended Feature 2 Register | PCI `EFRSup=1` |
| `0x2020` | IOMMU Status | one asynchronous sample |
| `0x0100 + 8*(n-1)` | Device Table Segment `n`, `1..7` | live `DevTblSegEn` is valid and no greater than live `DevTblSegSup`; read only active segments |

Require live EFR and EFR2 to match the preferred Type `11h`/`40h` IVHD images.
If they do not, retain the raw conflict, skip every feature-dependent access and
decode, and keep the slice blocked; do not guess which description is safe.

Read the stable configuration set twice and require exact equality. Re-read the
PCI base registers as the outer identity check. Status is hardware-updated and
is recorded once without an equality requirement. Its interrupt and overflow
fields are RW1C, but AMD defines clearing by a write; a pure read does not
acknowledge them. No read-modify-write operation is permitted.

Do not read command, event, PPR, or GA ring memory; device-table entries; page
tables; head/tail pointer registers; PPR/GA/alternate-log base registers;
performance counters; hardware-error registers; guest-APIC-log registers;
MSI-MMIO aliases; virtual-IOMMU registers; implementation-specific registers;
or reserved offset `0x1ff8`. Do not follow any configured physical pointer. Validate
enabled table and buffer base/length fields against IOMMU `PAsize`, CPUID
physical width, checked arithmetic, and the memory map, but retain only their
register values and descriptor witnesses.

Offsets `0x0020` and `0x0028` have exclusion-range semantics before SNP and
completion-store semantics after SNP. Until inherited SNP state is separately
measured, preserve both names and do not turn these values into an exclusion
bypass claim.

## Permitted evidence and mandatory non-claims

This slice may establish only that, at the sampling instant:

- the IVRS-selected PCI function exposes the expected live IOMMU capability;
- PCI capability enable/lock and live MMIO-base agreement were observed;
- live EFR/EFR2 values agree or disagree with the preferred IVHD images;
- global `IommuEn`, configured root/buffer register values, and command/event
  run or overflow status were observed; and
- the snapshot passed the explicit stability and address-range checks above.

It cannot identify who allocated or owns a configured table, prove exclusive
ownership, establish any DTE contents or mapping permissions, or prove
requester-specific DMA or interrupt remapping. It also cannot establish ACS,
peer-to-peer containment, slot isolation, SMM immutability, or persistence
after the probe returns.

Therefore `amd_iommu_ownership_claim`, `pci_isolation_claim`, launch
authorization, and qualification remain false/blocked after this slice. A
later separately reviewed DTE/requester and PCI-topology slice is required for
isolation evidence.

## Implementation and release gate

The hardware-independent unit derivation, decoders, snapshot comparison, and
negative tests are implemented in the `iommu` module. Schema v4, its valid
fixture, cross-field verifier rules, negative mutations, and finalizer support
are implemented.
The EFI adapter must use only the two protocol read surfaces above. Re-run the
PE/instruction safety gate and review the disassembly before preparing new
media. The completed schema-v4 and rejected schema-v5 payloads must not be
booted again; they were preserved before replacement. At
`2026-08-09T04:02:42.5037164Z`, the user separately authorized staging the
reviewed schema-v6 EFI and exactly one new cumulative record-only cold boot on
the same bound BIOS F7 machine under this policy and the reviewed MSR read
policy. The staged bytes passed read-back and the safety gate. This one-shot
decision did not broaden the bounded reads above. The schema-v6 boot completed
on 2026-08-09; its immutable record was finalized and dual-verified as a
noncanonical promotion candidate. The authorization is consumed: do not boot
the medium again, and do not retry.
