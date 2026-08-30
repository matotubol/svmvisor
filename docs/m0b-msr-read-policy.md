# M0b per-processor system-register read policy

Status: reviewed read contract used by schema v5 and carried forward into the
corrected schema-v6 remediation. The one authorized schema-v5 physical capture
completed, but its immutable result is preserved as rejected diagnostic
evidence because the collector and comparison contract were defective. Schema
v6 / collector 0.6.0 software is implemented and locally release-gated. Its
separately authorized cold-boot capture completed and was strictly finalized
and dual-verified as a noncanonical promotion candidate. This policy authorizes only the bounded `RDMSR` reads
below; by itself it authorizes neither a physical boot nor a write to any MSR
or control register, SVM enablement,
`VMRUN`, a PCI or MMIO access, an SMM entry, or a physical-pointer dereference.

The architectural sources are the repository-local AMD APM Volume 2 rev. 3.44
([`24593_3.44_APM_Vol2.pdf`](24593_3.44_APM_Vol2.pdf)) and the AMD PPR for
Family `1Ah` Model `44h` B0, publication 57896 rev. 3.00
([`57896-3.00_PPR.pdf`](57896-3.00_PPR.pdf)), SHA-256
`643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
Register addresses and fields below are cited to those two documents; nothing
is taken from a diagram or a generic table.

## Canonical target witness

The canonical schema-v4 capture
`target/evidence/m0b-probe-20260807T050309-000000000` reports CPUID family 26
(`1Ah`), model 68 (`44h`), stepping 0 (B0) — exactly the PPR's coverage — with
24 enabled, healthy logical processors and true per-processor consistency
aggregates. It also records `VM_CR` on every processor and the global AMD-IOMMU
state. This slice reuses the same target and the same blocking, sequential,
timeout-zero MP Services dispatch reviewed in
[`f7-mp-services-audit.md`](f7-mp-services-audit.md); every enabled processor
observes the same register set through the shared BSP/AP measurement body.

## Scope and gates

Every read in this slice is an `RDMSR` at CPL0 with no write side effect. The
slice is reached per processor only under the existing
`should_read_vm_cr` predicate: the processor identifies as `AuthenticAMD`,
enumerates SVM in CPUID `8000_0001h`, and enumerates leaf `8000_000Ah`. Any
other outcome is an explicit `not-attempted` status with a static reason
string; unavailable or ungated registers are never fabricated.

Two narrower gates apply on top:

- The IORR quartet is read only when the CPUID family is 26 (`1Ah`) and the
  model is 68 (`44h`) — exactly the coverage of the pinned PPR 57896 rev. 3.00,
  which documents `MSRC001_001[6...9]`. Any other family or model records
  `not-attempted`.
- The eleven fixed-range MTRRs are read only when the same-run `MTRRcap[FIX]`
  bit is set. The variable MTRR pairs are read only for indices below the
  same-run `MTRRcap[VCNT]`; a `VCNT` greater than 8 fails the capture closed
  rather than reading beyond the reviewed site set.

`MTRRcap` (`00FEh`, read-only) is therefore read first on each processor and
bounds that processor's own site sequence.

## Register instance scope and comparison contract

PPR 57896 printed page 43 defines the core-register instance notation used by
the register tables. A suffix containing `_thread[1:0]` identifies distinct
per-thread instances. If `_thread[1:0]` is absent but `_core[...]` remains, the
register is one per-core instance shared by that core's sibling threads; it is
not thereby shared across all cores.

The allowlist therefore has these scopes:

- `SMM_BASE` (PPR printed page 213), HWCR (page 203), and PAT (page 171) are
  thread-scoped;
- SMMAddr/SMMMask (pages 213-214), SYS_CFG (page 202), IORRs and TOP_MEM
  (page 205), TOM2 (page 206), and the MTRRs (pages 123, 126-170, and 173) are
  core-scoped and shared only by sibling threads; and
- the `_n...` suffixes on IORR and MTRR rows select register-number instances;
  they do not change the core-sharing domain.

`SMM_BASE` is retained and decoded in every processor observation, but it is
excluded only from the cross-processor configuration-equality projection. The
PPR explicitly makes it thread-scoped and describes it as saved on SMM entry,
restored by `RSM`, and independently changeable through the SMM save state or
`WRMSR`; distinct logical-processor values are therefore legitimate evidence,
not a configuration inconsistency.

PAT equality remains required by AMD APM Volume 2 section 7.8.6, and the MTRR
comparison remains a conservative proxy for the APM section 7.7.5 requirement
that all processors characterize memory the same way. Exact comparison of raw
HWCR and the core-scoped SMM/address-routing registers is useful conservative
firmware evidence, but their PPR instance scope does not make equality across
threads or cores an architectural validity invariant. In particular,
`HWCR[SmmPgCfgLock]` is thread-scoped and the PPR says `RSM` clears it. An honest
difference must be recorded and interpreted, not used by itself to invalidate
the raw capture.

## MSR read allowlist

Each address has exactly one named, non-inlined `RDMSR` site in the PE image,
so the PE/instruction safety gate can pin every site to its literal address.

| MSR | Address | Citation | Recorded content |
| --- | --- | --- | --- |
| `MTRRcap` | `00FEh` | APM §7 "Identifying MTRR Features", Appendix A | raw; `VCNT`, `FIX`, `WC`, `SMRR` |
| `MTRRdefType` | `02FFh` | APM §7, Appendix A | raw; `MemType`, `FE`, `E` (`MtrrDefTypeEn`) |
| PAT | `0277h` | APM §7 "PAT Register", Appendix A | raw only |
| `MTRRphysBase0–7` | `0200h`+2n | APM §7, Appendix A | raw; `Type`, `PhysBase[47:12]` (n < `VCNT`) |
| `MTRRphysMask0–7` | `0201h`+2n | APM §7, Appendix A | raw; `PhysMask[47:12]`, `V` (n < `VCNT`) |
| `MTRRfix64K_00000` | `0250h` | APM §7, Appendix A | raw |
| `MTRRfix16K_80000`, `MTRRfix16K_A0000` | `0258h`, `0259h` | APM §7, Appendix A | raw |
| `MTRRfix4K_00000`–`MTRRfix4K_7` | `0268h`–`026Fh` | APM §7, Appendix A | raw |
| SYS_CFG | `C001_0010h` | PPR `MSRC001_0010` | raw; `MtrrFixDramEn` (18), `MtrrFixDramModEn` (19), `MtrrVarDramEn` (20), `MtrrTom2En` (21), `Tom2ForceMemTypeWB` (22), plus raw-only encryption-control bits `SMEE` (23), `SecureNestedPagingEn` (24), `VmplEn` (25), `HMKEE` (26) with no encryption-state claim |
| HWCR | `C001_0015h` | PPR `MSRC001_0015`; APM §15.32.1 | raw; `SmmLock` (bit 0), `SmmPgCfgLock` (bit 33) |
| `IORRBase0/1` | `C001_0016h`, `C001_0018h` | PPR `MSRC001_001[6...8]` | raw; `PhyBase[47:12]`, `RdMem` (4), `WrMem` (3) |
| `IORRMask0/1` | `C001_0017h`, `C001_0019h` | PPR `MSRC001_001[7...9]` | raw; `PhyMask[47:12]`, `Valid` (11) |
| TOP_MEM | `C001_001Ah` | PPR `MSRC001_001A` | raw; `TOM[47:23]` |
| TOM2 | `C001_001Dh` | PPR `MSRC001_001D` | raw; `TOM2[47:23]` |
| SMM_BASE | `C001_0111h` | PPR `MSRC001_0111` | raw; `SmmBase[31:0]`, with `SmmBase[3:0]` required to be zero |
| SMMAddr | `C001_0112h` | PPR `MSRC001_0112` | raw; `TSegBase[47:17]` |
| SMMMask | `C001_0113h` | PPR `MSRC001_0113` | raw; `TSegMask[47:17]`, `TMTypeDram` (14:12), `AMTypeDram` (10:8), `TMTypeIoWc` (5), `AMTypeIoWc` (4), `TClose` (3), `AClose` (2), `TValid` (1), `AValid` (0) |
| VM_CR | `C001_0114h` | APM §15.30.1 (existing v1/v3 site) | unchanged existing record |

That is 41 new sites and one existing site: 42 audited `RDMSR` sites per
processor measurement. Each executes at most once per measured processor, so a
24-processor run performs at most 1008 bounded reads. The evidence records
`msr_read_operations` and `msr_read_bytes` per run; `msr_write_operations` is
pinned at zero and any nonzero value fails verification.

## Never-read list

The following are explicitly excluded, with the reason recorded in the policy:

- `MSRC001_0118` `SvmLockKey`: PPR — "Reads of this register always return
  zero." No evidentiary value; SVM lock state is observed through
  `VM_CR[Lock, SvmeDisable]`.
- `MSRC001_0119` `SMM_KEY`: APM §15.32.2 — write-oriented unlock mechanism;
  not a readable state register.
- `MSRC001_011B` AVIC Doorbell: PPR — **Write-only, Error-on-read**.
- `MSRC001_0020` `PATCH_LOADER`: PPR — Write-only, Error-on-read.
- `MSRC001_0117` `VM_HSAVE_PA`, `MSRC001_0115` `IGNNE`, microcode, APIC,
  performance, debug, speculation-control, and every other MSR: outside this
  slice's reviewed scope.
- Every register not in the allowlist. The PE/instruction gate fails on any
  `RDMSR` site whose address is not one of the 41 literals above.

## Permitted evidence and mandatory non-claims

This slice may establish only that, per enabled processor at the sampling
instant:

- the allowlisted registers held the recorded raw values;
- the decoded fields (SMM lock and TSeg/ASeg configuration, IORR validity and
  ranges, TOM/TOM2 split, MTRR capability/default/variable/fixed state, PAT
  image, SYS_CFG memory-type controls) have the recorded values; and
- the configuration-equality projection, which excludes only the deliberately
  thread-specific `SMM_BASE`, was exactly equal or honestly unequal across all
  enabled processors; and
- every per-thread `SMM_BASE` value was retained without treating distinct
  values as a configuration mismatch.

It cannot prove SMM immutability (a lock bit is observed, not enforced),
correctness or completeness of firmware's DRAM/MMIO carving, absence of SMM or
firmware mediation, inherited memory-encryption enablement, or persistence
after the probe returns. PCI-BAR and data-fabric aperture enumeration beyond
the IVRS-derived IOMMU window remains with the later DTE/PCI-topology slice
tracked by `amd-iommu-dte-requester-ownership-and-pci-isolation`. Raw SYS_CFG
encryption-control bits are recorded without claiming SME/SEV/SNP state;
`inherited-memory-encryption-state` remains an open blocker.

Therefore `amd_iommu_ownership_claim`, `pci_isolation_claim`, launch
authorization, and qualification remain false/blocked after this slice.

## Implementation and release gate

Schema v6 corrects `SMMMask[TMTypeDram]` to bits 14:12, retains the full
`TSegBase[47:17]` and `TSegMask[47:17]` widths, and replaces the schema-v5
whole-object equality rule with the scope-aware configuration projection above.
The collector, schema, verifier, fixtures, negative mutations, and finalizer
version handling are aligned at schema 6 / collector 0.6.0. Fresh local gates
pass: 98 locked workspace tests, 93 verifier invocations, both current/copied
V4 and M0a verifiers, both release builds, and the PE/instruction audit with
exactly 42 literal-pinned `RDMSR` sites. The resulting 315,392-byte EFI
SHA-256 is
`1b394c5222df1df9099ad16bb5da8eb7c62cf4b56267237ccb2d438489414fbe`.
The exact tested bytes are preserved under
`target/evidence/m0b-probe-v6-software-preflight-20260808T232024707Z`.
The user separately authorized replacing the consumed schema-v5 payload and
exactly one cumulative record-only schema-v6 cold boot on the bound BIOS F7
machine at `2026-08-09T04:02:42.5037164Z`, under this policy and the reviewed
IOMMU read policy. `BOOT-AUTHORIZATION.md` in that preflight directory records
the exact decision. The boot completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T151657-000000000.json`. All 24 processors were observed;
the corrected `TMTypeDram[14:12]` and full-width TSeg decodes pass, every
non-`SMM_BASE` system-register field is exact across processors, all comparison
witnesses and aggregates are true, and the run records 1,008 MSR reads with
zero writes. Strict finalization and both verifier copies pass at
`target/evidence/m0b-probe-20260809T151657-000000000`; raw SHA-256 is
`c7c71ab7d3af18021326e9da4ff3d4eb9edb78e80d97ab9f87948a0cefc44384`
and manifest SHA-256 is
`58d957d92744e457ce9c39c48b5ecd2ff058f84492793deb909d7e590901c89b`.
The schema-v6 one-shot authorization is consumed and permits no retry. The
candidate remains noncanonical until a separate explicit promotion decision.

The earlier schema-v5 boot decision was recorded at
`2026-08-08T22:34:48.382Z`: the user authorized exactly one cumulative
schema-v5 record-only cold boot on the bound BIOS F7 machine using the staged
EFI SHA-256
`4ff897015fbeb3989cc8dc465c9c18b00a0bfe58f63a13e631f9e239384eff4f`.
The detailed scope and media binding are preserved in
`target/evidence/removable-media-preservation-20260808T222640423Z/BOOT-AUTHORIZATION.md`.
That boot completed on 2026-08-09 and returned
`svmvisor-m0b-20260809T003925-000000000.json`. Finalization stopped after the
returned machine exposed a verifier width defect, the collector's incorrect
`TMTypeDram[16:15]` decode, and the contract's invalid requirement that
thread-scoped `SMM_BASE` be equal. The immutable bytes and capture-time
artifacts are preserved under
`target/evidence/rejected-m0b-probe-20260809T003925-000000000`; they are not a
finalized bundle, are not canonical, and must not be promoted. The one-shot
authorization was consumed. No schema-v6 boot, retry, or other new physical
capture was authorized by the schema-v5 decision. The later, timestamped
schema-v6 authorization above supersedes only that historical status; it does
not broaden either read policy or authorize any write, qualification, launch,
or retry. That later authorization is now also consumed by the completed
schema-v6 capture and authorizes no additional physical run.
