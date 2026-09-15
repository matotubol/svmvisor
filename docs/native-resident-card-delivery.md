# Native resident card delivery and first-boot evidence

This is the local delivery implementation and operator contract for the first
trusted Windows boot. It does not report a programmed card or Windows execution.
The historical physical result remains the exact returning image with 65/65
entries/exits. The resident child does not inherit that result.

The replacement candidate is
`target/firmware/squirrel/native-resident/ec762c3d176248e4bda8a2532b1e0f53`;
its combined 5 MiB SHA256 is
`8c4f30fcff04cd2f4b2852048ffab3204287ba7a8e31ac019c40528046f53698`.
FPGA ID `968cbbd8710939f4`, ROM ID `f56dfa81fa7121ea`. Its exact production
parent and child match the passing actual firmware-chain test. The original
`275a28...` candidate below is rejected historical evidence. The final local
result belongs in `work/native-firstboot-2026-09-14/summary.json`.

The new candidate-bound `firmware/squirrel/card-resident-test.ps1` defaults to
`CheckOnly`, which checks files/audits and accesses no hardware. Once a target
configuration and programming action are deliberately selected, `-Action Program
-ConfirmFlash` preserves the prior full image through two matching 5 MiB backups,
immediate prewrite verification and full readback. The prior working image pin
is `4d3d738898a4e49b098a4ae7f940894c7a7c9c4a38e1607345c3551ce1e3efc7`.
Restore uses `-Action Restore -ConfirmFlash -RestoreSession <new-program-session>`;
`-Action CheckOnly -RestoreSession <new-program-session>` checks that recovery
source without hardware access. Actual programming and activation have not run.

## Delivery and ownership

The existing completion-only endpoint and BAR1 slot remain the substrate. The
current production resident PE is 153,088 bytes (4,837,376 bytes loaded), too large for the installed 32 KiB option-ROM
aperture. The separate small parent continues to load the complete child through
UEFI LoadImage/StartImage from the existing 1 MiB SPI slot at offset 4 MiB.
No FPGA requester path, arbitrary memory access, or new flash layout is added.

`card-resident-loader` selects a separate parent policy in the existing PE owner.
The 128-byte envelope is `SVMBPE01`, version1, flags4 and EFI runtime subsystem12;
its remaining size, digest and PE section fields retain the existing checked
layout. The returning `SVMPE001`, flags2, subsystem11 policy stays separate.
Neither a subsystem edit nor an updated old returning pin is a resident package.

Both policies retain firmware image authentication. Resident delivery passes a
128-byte `ResidentBootOptions` (`SVMBOT01`) in LoadedImage.LoadOptions. The child
copies only the numeric BAR0 address and boot ID, marks entry after validating
that writable record, and explicitly acknowledges arming only after installing
the successful-EBS-return hook. Successful StartImage without that acknowledgement
is a protocol failure, not evidence of arming or guest entry.

Every non-error StartImage return, including a warning or malformed acknowledgement,
retains the child and its RuntimeServicesData input pool until reset. Cleanup
refuses before controller decode/protocol ownership can be released. Parent
binding remains successful for that retained state so firmware teardown cannot
invalidate a possibly installed hook. The journal reports the separate delivery
failure. Failed LoadImage and auto-unloaded child error returns retain their
normal cleanup rules. An entered child must never return an EFI error after
installing a live callback or hook.

The parent registers all fallible lifecycle events before starting the resident
child. Its lifecycle observation is suppressed while the child owns the journal
and after retention; the ExitBootServices bookkeeping still runs. Callbacks read
separate atomic state, never borrow the PE owner's mutable State across nested
firmware calls. Resident GET_PROTOCOL uses the UEFI2.11 rule that CloseProtocol is
not required; no interface reference survives the immediate use.

## Concrete offline package procedure

Use a new output directory for the existing production builder:

```powershell
python tools/native-resident/build.py --output work/NEW-PRODUCTION --boot
```

No `--test-output` image belongs in a physical bundle. The existing raw payload
linked audit and AP copied-code audit must pass. The consuming verifier binds the
complete source checkpoint, retained source copies, driver, linked payload,
relocations and audit outputs to that production build:

```powershell
python firmware/squirrel/verify-resident-build.py --evidence work/NEW-PRODUCTION --image work/NEW-PRODUCTION/driver.efi
```

The existing card builder accepts `-PayloadKind NativeResidentBoot`,
`-ResidentBuildPath` pointing to that complete directory, `-PayloadPath` and its
exact lowercase SHA256, and `-PayloadEvidencePath` pointing to its summary.json.
It creates a fresh `target/firmware/squirrel/native-resident/<session>` with the
child, SVMBPE01 header, 1 MiB slot, parent EFI, 32 KiB ROM/memory file, recipe/build
IDs, source copies, tool identities and manifest. `-BuildFpga` invokes the existing
completion-only endpoint flow and constructs a separate 5 MiB combined review
image. It performs no card access, programming or reboot. The consuming audit is
checked before and after packaging; a source change requires a new child build.

A physical candidate additionally needs the actual parent/child firmware image
service fixture, exact package review, routed timing and completion-only netlist
review, and a new candidate-bound programming/recovery procedure. Historical
`card-multi-exit-test.ps1` and other golden programmer pins remain immutable.
Do not use a historical programmer to install this new image. A fresh programmer
must preserve the established two matching 5 MiB backups, immediate prewrite
verification, complete programming/readback and session-bound full restore.
The non-enumerating recovery image alone restores only its configuration sectors.

## First-boot observations and recovery

Before selecting a target configuration, collect its current Windows facts with
the existing read-only `tools/target-profile/collect-windows.ps1`. Retain the exact
Windows build, firmware, CPU topology, Secure Boot and DeviceGuard values; failed
queries remain unknown. Its old milestone qualification field is historical and
does not add malware-analysis prerequisites to first boot. Nested Hyper-V/VBS/HVCI
is unsupported by this resident runtime. A running VBS/HVCI target therefore
requires a deliberate target/configuration decision before this boot experiment;
this procedure changes no protection setting.

The configured target must also pass its actual firmware MP inventory, initial
control/xstate, memory-ownership/cache and image-authentication admission. Those
are measurements of this boot, not facts inferred from a normal Windows desktop
or the earlier returning image. Unsupported initial state must remain a refusal.

After the exact reviewed card has been programmed and full readback verified,
activation is a separate user-controlled cold boot, retaining the programmer's
recovery session and update USB access. Programming leaves the volatile BSCAN
proxy loaded; power removal sufficient to reconfigure the card is required.
Retain the operator's boot outcome and perform the existing bounded USER2 read
with the exact new candidate manifest. Live capture is available through
`firmware/squirrel/read_snapshot.py --live --manifest <candidate-manifest>`;
`--input <retained-log> --manifest <candidate-manifest>` rechecks offline.
The reader requires consecutive identical CRC-valid frames and both matching
FPGA/ROM IDs. A normal desktop without this identity and activation witness is
not proof that Windows ran under this image.

Detail8 has two explicit wire formats. Snapshot DWORDs10/11/9 carry journal
DWORDs4/5/6; journal TSC words are not exported by the current USER2 frame.

| Phase | Exported evidence |
| --- | --- |
| 0x10 parent | Delivery stage, entered/armed bits and exact child failure or EFI status. Arming is separate from actual activation. |
| 0x13 stage1 | Hook armed. |
| 0x13 stage2 | Original ExitBootServices returned successfully. |
| 0x13 stage3 | BSP is about to release the recorded AP slot. |
| 0x13 stage4 | BSP is about to capture its resident continuation. |
| 0x13 stage5 | All assigned CPUs activated, immediately before returning to the OS loader. |
| 0x13 stage0x80 | Activation failure; exact bounded startup failure code. |

The BSP validates and writes these bounded records only before the Windows
loader resumes. There are no AP or persistent-host BAR writes: OS BAR relocation
and decode changes are not owned. A failed journal access stops observation;
a stale earlier stage must not be promoted to success. Stage5 does not prove that
Windows reached its desktop or that a later guest exit succeeded. Production
terminal exit code/RIP/info remain in retained RAM; there is currently no physical
post-loader export for those fields. An unexplained hang after stage5 may require
another reviewed diagnostic batch. No automatic repeated boot or flash follows.

A failed experiment uses the exact new programming session's reviewed restore
procedure to recover the preceding full 5 MiB working image, followed by full
readback and a deliberate activation. Do not substitute an older backup/session.
No changes here implement containment, device/DMA isolation or malware execution.
No physical timing or Windows protection compatibility result is claimed.

## Source and verification scope

Local UEFI2.11 `UEFI_Spec_Final_2.11.pdf`, SHA256
`a64b8e442004b91becc3de9afaf8ca61b259a9a3b436accb6b3711ab5400cee9`,
sections4.1, 7.3.9, 7.4.2-7.4.5 and9.1.1 define image services and lifetime. The
local chapter7/page172 extracts and current repository image owner were reviewed.
The processor, EBS activation and exit policies remain those in
`native-broadcast-startup.md` plus the current loader-control batch.

New host scenarios cover resident arming, missing/corrupt acknowledgements,
warning retention, entered refusal, firmware security denial and changed BAR
inputs. Package tests distinguish subsystem/envelope policies and bind slot
hashes. Reader tests cover wire rotation, each boot stage, invalid fields and
full-width EFI status. The coordinating result must record which checks actually
ran and their exact source/artifact checkpoint; this runbook itself is not a pass.

## 2026-09-14 initial candidate: rejected by firmware integration

The first completed offline build was
`target/firmware/squirrel/native-resident/275a28af07304c86aca37438131e893a`.
Its actual production firmware-chain test failed; it is not a hardware candidate.
The exact production child is
`work/native-firstboot-2026-09-14/build-release-production/driver.efi`, SHA256
`7a643b117d025207624906136c01414443d5dcc1ebf14fec85c8dc2a4669ee6e`.
Its five production/test profile builds share the same 254-file source manifest.
The complete 5 MiB combined image SHA256 is
`b4b157d7fe09f102779e0d29f3bfdb2600e3dc5b5cefd518b523f41ff32054b6`.
The image has not been programmed or activated. The actual firmware test
`work/native-firstboot-2026-09-14/card-positive-01` passed image retention,
runtime descriptor coverage, Stop refusal and a genuine stale-key EBS failure.
On the successful retry, AP startup stopped with stage0x80/detail33. The copied
AP waiting code and BSP completion check still expected the diagnostic CPUID
leaf0x4fff0000, whose response exists only in test builds. The production child
correctly kept CPUID native, so this fixture exposed a production-only failure.
Earlier successful test-output execution did not cover that distinction. The
candidate review records the failure and its programmer refuses this candidate.

The final local regression recorded 414 core and 290 native DXE checks, plus
21 returning-parent and 12 resident-parent test executions in separate feature
runs (some checks overlap). Package, evidence-consumer and snapshot-decoder
suites passed 44 checks. The native execution matrix passed 11 scenarios with
118 AP restarts; actual FSGSBASE execution passed, while the frozen backend
cannot emulate PCID. See `native-loader-control-admission.md` for that boundary.
The native matrix loads the child directly; it alone does not prove nested
parent/child image-service lifetime. The separate firmware-chain result and
routed review must be recorded with the final candidate evidence.

The read-only platform observation in
`work/native-firstboot-2026-09-14/platform-observation.json` records a Ryzen 9
9900X, 24 logical processors, Windows 11 Pro build26200, running VBS and HVCI
(DeviceGuard status2 and running service2). Secure Boot status could not be
read with the available permissions and remains unknown. No setting changed.
This protected installation is outside the resident runtime's supported scope;
preparing this candidate does not resolve that target configuration decision.

## Production startup acknowledgement correction

The replacement binds completion to the existing `NativeBootstrapAck` owner.
Only the immutable guest epilogue reached after that verified one-shot VMMCALL
sets the matching CPU's bit in a shared completion word. The bounded assembly
lookup uses the admitted immutable CPU inventory. Refused callbacks skip that
publication. The copied AP continuation and BSP require their own bit before
publishing the existing completion record. No diagnostic CPUID leaf is exposed
in production. The completion word is a trusted boot witness, not attestation
against later guest software.

The exact linked epilogue is 130 bytes within its admitted 256-byte mapping;
the copied AP wait code has zero relocations. The replacement production child
SHA256 is `f05bc4e826b31d22723e85103b0afa2b80c310666b6ba73a662503a2a73aee44`.
The actual firmware-chain result `card-positive-03` under
`work/native-firstboot-2026-09-14` passed two-CPU stages1 through5, a real stale-key
EBS failure followed by successful retry, retained runtime image/options/journal
coverage, refused Stop without decode release, and CPUID/EFER execution after
return to the loader. An independent before/after check confirms the diagnostic
CPUID leaf stays native. Header and digest rejection cases also passed.

The firmware service clears IF on successful EBS in this OVMF configuration.
The fixture logs that single observed RFLAGS difference and uses the existing
native loader test's comparison scope; all other compared controls, tables and
segments must match. Preserving the original EBS return flags is distinct from
forcing the service's entry flags back on. This is an emulator observation,
not a claim about the target firmware or Windows.

`validation-v3.json` is the replacement source regression: 704 core/native tests
plus 33 separate delivery test executions, five builds with one 254-file source
checkpoint, and 11 native execution scenarios / 118 AP restarts. Earlier v2
results and the failed first card image remain historical evidence.
