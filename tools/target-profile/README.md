# Target-profile collector

This is the preserved Milestone 0a inventory tool. Its target capture is
complete; it is not the current implementation entry point. The current path is
the [first-light plan](../../docs/first-light-plan.md). This tool records what
Windows can observe about the proposed lab target without installing a driver or
changing firmware, boot configuration, virtualization state, or existing OS
configuration. Its only write is a new, explicitly selected evidence bundle.

Run it from the repository root:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\target-profile\collect-windows.ps1
```

By default, every run creates a new immutable-style bundle under
`target/evidence/target-profile-<UTC timestamp>/` containing:

- `target-profile.json`: collected facts, frozen scope, unresolved gates, and
  collection errors;
- `target-profile-v1.schema.json`: the exact schema used by that run;
- `collect-windows.ps1`: the exact collector bytes identified by the profile;
- `verify-bundle.ps1`: the self-contained manifest/safety-contract verifier; and
- `manifest.json`: file sizes and SHA-256 hashes.

The collector runs the copied bundle verifier before reporting success and
refuses to overwrite an existing evidence directory. Formal use should also run
the profile through a JSON Schema Draft 2020-12 validator.

Before PowerShell deserializes JSON, the verifier rejects duplicate decoded
object-property names, including escape-equivalent spellings such as `safety`
and `\u0073afety`. Its regression suite also rejects duplicate semantic
reference keys and duplicate manifest paths.

The script deliberately omits host names, system serial numbers, processor
identifiers, and BitLocker recovery material. It hashes the PCI instance ID by
default; pass `-IncludePciInstanceId` only when raw identity is required.

> **Do not publish a raw bundle.** Even with those omissions, the exact CPU,
> board, BIOS, Windows, and PCI fingerprint is stable identifying information.
> Treat the evidence directory as sensitive lab data.

## What the result does not prove

Windows CIM data is useful inventory, not an architectural SVM qualification.
The profile therefore leaves explicit gates for exact CPUID leaves, `VM_CR`,
SVM/NPT features, AMD-IOMMU/IVRS ownership, the direct watchdog, the physical
`CARD_BYPASS` design, complete-disk recovery, Secure Boot keys and option-ROM
policy, and firmware event ordering. Those require later read-only UEFI probes,
hardware documentation, or physical recovery exercises.

The collector never authorizes a physical FPGA candidate, an SVM control-state
write, process introspection, or execution of an untrusted guest. Every raw
profile has `qualification_status = "blocked"`.

## Verification and later adjudication

Recheck a copied bundle without collecting anything:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\tools\target-profile\verify-bundle.ps1 `
    -BundleDirectory .\target\evidence\target-profile-<UTC timestamp>
```

Never edit the raw profile to mark a gate complete. A later Milestone 0
finalizer will consume the immutable raw bundle plus separately hashed evidence,
validate their relationships, and emit a derived qualification decision. Until
that tool and its schema exist, unresolved gates remain blocked/unknown.

## Sources

The local source of record for AMD64 system programming is
[`docs/24593_3.44_APM_Vol2.pdf`](../../docs/24593_3.44_APM_Vol2.pdf), SHA-256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
Use the checked-in chapter extracts for retrieval, especially
[Chapter 5: Page Translation and Protection](../../docs/amd64_apm_vol2_markdown/chapters/05-page-translation-and-protection.md)
and
[Chapter 15: Secure Virtual Machine](../../docs/amd64_apm_vol2_markdown/chapters/15-secure-virtual-machine.md),
but verify tables and diagrams against the PDF.

The collected Windows fields follow Microsoft's read-only
[`Win32_Processor`](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-processor),
[`Win32_BIOS`](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-bios),
and
[`Win32_BaseBoard`](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-baseboard)
classes. Secure Boot state is queried with
[`Confirm-SecureBootUEFI`](https://learn.microsoft.com/en-us/powershell/module/secureboot/confirm-securebootuefi),
which can require an elevated shell; a permission failure is recorded rather
than hidden.

After the exact family/model/stepping is known, select and hash the target PPR and
Revision Guide from the
[AMD documentation hub](https://www.amd.com/en/search/documentation/hub.html).
For the frozen target (family 26, model 68, stepping 0) the PPR is now selected
and hashed: AMD publication 57896 rev. 3.00, *Processor Programming Reference
for AMD Family 1Ah Model 44h, Revision B0 Processors*, preserved at
`docs/57896-3.00_PPR.pdf` with SHA-256
`643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
The matching Revision Guide remains outstanding.
