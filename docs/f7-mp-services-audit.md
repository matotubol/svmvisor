# Gigabyte F7 MP Services audit for M0b slice 2B

## Decision

This source-assisted binary review clears the MP Services boundary only for a
cold-boot, pre-`ExitBootServices` schema-v3 capture on the immutable M0a target:

- Gigabyte `B850 AORUS ELITE WIFI7`;
- BIOS version `F7`;
- AMD Ryzen 9 9900X, 12 cores and 24 logical processors; and
- M0a manifest SHA-256
  `c4fa28c1ba43aabd1fb67697706ddd41d6a4e984be5745a939720029c90a192c`.

On the reviewed F7 default path, idle APs are parked in an MWAIT loop and
`EFI_MP_SERVICES_PROTOCOL.StartupThisAP()` wakes one AP by writing its
`StartupApSignal`. It does not send INIT/SIPI for an ordinary steady-state
dispatch. Windows `bootmgfw.efi`, `winload.efi`, and other Windows EFI images do
not implement this firmware protocol and are not analysis inputs for this gate.

The review also found that a finite `StartupThisAP()` timeout is unsafe for the
record-only contract: when it expires, this F7 implementation resets the AP and
can enter its INIT/SIPI path. The probe must therefore use a blocking call with
no event and `TimeoutInMicroseconds == 0`.

## Bound artifacts

The locally archived vendor image is version-matched to the live target's M0a
record. It is not a cryptographic measurement of a live SPI readback, so this
decision must be revisited after any BIOS update or unexpected firmware-state
change.

| Artifact | Identity |
| --- | --- |
| Gigabyte F7 image | `C:\Users\mato\Documents\svm-helpers\bios-f7\bios\B850AELITEWF7.F7`, 33,554,432 bytes, SHA-256 `41a4a72c5166dc2c2f3ad8db9d8636f5e1a3b83b213375458cf9e9530e9a17b8` |
| Extracted `CpuDxe` PE | FFS GUID `1A1E4886-9517-440E-9FDE-3BE44CEE2136`, `C:\Users\mato\Documents\svm-helpers\bios-f7\analysis\MpServicesCpuDxe_F7.bin`, 95,232 bytes, SHA-256 `596c7c2e4914c3cd452fcb5366c2e7d092bb5617ded8b3bc05421c9199a20f81` |
| PEI PCD database | extracted `369 PcdPeim\0 Raw section\body.bin`, 8,376 bytes, SHA-256 `81a27622a4cb285862fda1f17277fa5ef293f1c80af453b2ed78deac102a7189` |
| AMD EDK II reference | `C:\Users\mato\Documents\svm-helpers\amd-edk2`, commit `807227a97f3967680d1051b8a59bacc4f1a8432e` (`turin_poc`) |
| Current TianoCore reference | `docs/vendor/edk2`, commit `82cfea329cc2214df006edc067ed852f4d86a314` |

The F7 image contains the AGESA identification string
`ComboAm5PI 1.2.0.3g`.
The extracted module GUID exactly matches the `FILE_GUID` declared by both the
AMD and current TianoCore `UefiCpuPkg/CpuDxe/CpuDxe.inf`, anchoring the public
source lineage to the target module.

Public source entry points used for the mapping are AMD's pinned
[`CpuMp.c`](https://github.com/openSIL/amd-edk2/blob/807227a97f3967680d1051b8a59bacc4f1a8432e/UefiCpuPkg/CpuDxe/CpuMp.c),
[`DxeMpLib.c`](https://github.com/openSIL/amd-edk2/blob/807227a97f3967680d1051b8a59bacc4f1a8432e/UefiCpuPkg/Library/MpInitLib/DxeMpLib.c),
[`MpLib.c`](https://github.com/openSIL/amd-edk2/blob/807227a97f3967680d1051b8a59bacc4f1a8432e/UefiCpuPkg/Library/MpInitLib/MpLib.c),
and
[`MpFuncs.nasm`](https://github.com/openSIL/amd-edk2/blob/807227a97f3967680d1051b8a59bacc4f1a8432e/UefiCpuPkg/Library/MpInitLib/X64/MpFuncs.nasm).
The current upstream comparison is TianoCore's
[`MpLib.c`](https://github.com/tianocore/edk2/blob/82cfea329cc2214df006edc067ed852f4d86a314/UefiCpuPkg/Library/MpInitLib/MpLib.c).

## Source-to-binary mapping

AMD's EDK II fork provides the closest source model for the F7 binary:

1. `UefiCpuPkg/CpuDxe/CpuMp.c:362` forwards the protocol call to
   `MpInitLibStartupThisAP()`.
2. `UefiCpuPkg/Library/MpInitLib/DxeMpLib.c:845` forwards it to
   `StartupThisAPWorker()`.
3. `MpLib.c:2908` validates the target, calculates the timeout, calls
   `WakeUpAP()`, and waits through `CheckThisAP()` in blocking mode.
4. `MpLib.c:1219` sets the selected AP's function, argument, state, and
   `StartupApSignal`. It calls `SendInitSipiSipi()` only when the reset-vector
   path is required.
5. `MpLib.c:306` obtains `PcdCpuApLoopMode`; `MpLib.h:110` defines value `2` as
   `ApInMwaitLoop`.
6. `MpLib.c:1433` defines timeout zero as infinity.
7. `MpLib.c:1716` sends an expired finite timeout through
   `ResetProcessorToIdleState()` at `MpLib.c:1654`, which calls `WakeUpAP()` in
   AP-reconfiguration state.

The exact F7 `CpuDxe` has the same older three-state AP-initialization shape as
that AMD fork (`Config = 1`, `Reconfig = 2`, `Done = 3`). Binary Ninja mapped:

| F7 RVA | Meaning |
| --- | --- |
| `0x37c0` | MP Services protocol installation |
| `0xf1e0 + 0x18` | `StartupThisAP` protocol slot |
| `0x42d8` | `StartupThisAP` entry |
| `0x92f0` | worker / `WakeUpAP` path |
| `0x9940` | blocking completion and timeout check |
| `0x4ea8` | local-APIC ICR write helper |
| `0x9bc0` | `MpInitLibInitialize`-matching initialization path |

The protocol GUID bytes for
`3fdda605-a76e-4f46-ad29-12f4531b3d08` occur at RVA `0xf010`. The normal
non-reset branch writes `WAKEUP_AP_SIGNAL` (`STAP`) at RVA `0x95fd`, then calls
the signal-handshake loop at RVA `0x96ba` -> `0x9108`. The reset branch programs
INIT delivery value `0x4500` and then a SIPI vector derived from the below-1-MiB
wake buffer. Its `AuthenticAMD` branch emits INIT plus one SIPI; the second SIPI
belongs to the non-AMD branch.

The F7 PCD database has no SKU delta (`Length == LengthForAllSkus`). Local token
`0x583` has entry `0x0100202c`; its UINT8 storage at database offset `0x202c` is
`0x02`. F7 `CpuDxe` queries token `0x583` and stores it in the field whose use
matches `PcdCpuApLoopMode`. The target's captured CPUID leaf 1 ECX value
`0x76d8320b` also reports MONITOR/MWAIT support, so `GetApLoopMode()` does not
downgrade the configured MWAIT loop to HLT.

The initialization path reads token `0x583` at RVA `0x9c33`, stores the selected
loop mode at `0x9e6e`, initializes `WakeUpByInitSipiSipi` false at `0x9e78`, and
completes its internal AP initialization with `InitFlag = ApInitDone`. Only then
does the module install MP Services at RVA `0x37ce`. Consequently the first
application-visible cold-DXE call begins with MWAIT mode, signal wake selected,
and AP initialization done unless an intervening S3 or other firmware state
transition changes that state.

## Capture policy and remaining limits

An M0b schema-v3 physical capture under this decision must:

- start from a real cold power-on boot, not S3 resume;
- remain on the exact M0a-bound board and BIOS `F7`;
- dispatch enabled, healthy APs sequentially before `ExitBootServices`;
- pass no event and no finite MP Services timeout;
- perform only the already reviewed CPUID, `WhoAmI`, and conditional read-only
  `VM_CR` callback; and
- treat a hang as a failed run requiring manual power-cycle recovery, with no
  successful evidence record assumed.

S3 firmware can set `WakeUpByInitSipiSipi`, and a dynamic PCD setter or other
unexpected firmware state could change the default path. The schema therefore
continues to describe MP dispatch conservatively as firmware-controlled, records
post-dispatch observations, and does not make a general pre-dispatch `VM_CR`
preservation claim. This target-specific decision authorizes neither SVM
enablement nor `VMRUN`, and says nothing about a later live AMD-IOMMU read policy.

Coreboot's generic
[`src/cpu/x86/mp_init.c`](https://github.com/coreboot/coreboot/blob/3cec8d1e4dc1b16fdb783959cd2950fb1e1324ba/src/cpu/x86/mp_init.c)
and
[`src/cpu/x86/sipi_vector.S`](https://github.com/coreboot/coreboot/blob/3cec8d1e4dc1b16fdb783959cd2950fb1e1324ba/src/cpu/x86/sipi_vector.S),
plus openSIL's
[`xUSL/CCX/Common/Ccx.c`](https://github.com/openSIL/openSIL/blob/632b97619c4504a71323349da89be42988e8e343/xUSL/CCX/Common/Ccx.c),
are useful corroboration for initial low-level AP launch. They are not the
primary implementation reference for the F7 UEFI `StartupThisAP()` service.
