# Native loader handoff and retained AP paging

The later broadcast/routing checkpoint is documented in
[native-broadcast-startup.md](native-broadcast-startup.md). The evidence below
describes this earlier frozen loader checkpoint.

The `native-resident-boot` profile now prepares all CPUs before boot-service
exit and activates through the original service's successful return. An ordinary
loader using the installed Boot Services table no longer needs to discover and
call the diagnostic `ActivationInterface`. This is implemented and exercised
with a disposable EFI loader; it is not yet a physical Windows boot result.

## Handoff contract

`boot_handoff.rs` admits an identity-mapped, writable Boot Services table before
resource installation. The final commit owns the EBS function slot at HIGH_LEVEL,
copies the current table, calculates its CRC32 with the replacement slot and zero
CRC field, then writes the slot and new CRC. It does not change page permissions,
persist firmware edits or patch Windows. Other interposers must not concurrently
own this slot. A loader that cached an earlier EBS pointer can bypass this seam;
the installed-table call path is the supported entry contract.

`boot.S` forwards the original image handle and map key. Non-success returns the
original result directly, without activation. After success it preserves the
service's returned flags and uses the existing qualified native assembly boundary
to protect GPRs and admitted xstate before calling physical startup. All APs and
the BSP must complete before returning EFI_SUCCESS to the loader. A capture or
partial-start failure retains its bounded failure record and halts; it cannot
fabricate a retryable EBS failure after firmware has already shut down.

The boundary retains its existing constraints: flat native AMD x64 firmware,
admitted controls, XCR0=3/7 when OSXSAVE is active, XSS=0, and the documented x87
pointer-preservation assumptions. This change does not expand arbitrary initial
xstate support. No allocation, MP Services or other Boot Services call occurs
on the successful-return activation path. Firmware may clear IF during EBS;
the wrapper preserves firmware's return flags, not the caller's earlier flags.

The install-time BSP CR3 value is no longer a permanent requirement. Each MP
observer and the final BSP preflight walk that CPU's current admitted identity
closure, including code/data permissions, bootstrap storage, raw runtime arenas
and the LAPIC cache mapping. The captured BSP continuation owns its loader root.

## AP memory lifetime

The retained DXE image now contains one bounded, sparse, independent AP bootstrap
root (`BootstrapPaging`, 64 table pages). Its root and storage must fit below4GiB;
the leaf aperture remains40bits. It maps the actual runtime image, raw arenas,
temporary low SIPI page and the admitted UC LAPIC page when required. Existing
private per-CPU host roots are unchanged. Original image page permissions are
preserved, except that each copied AP wait-code page is executable in this AP root.

The physical trampoline loads this owned root before capture. Each AP's guest
continuation jumps into95 bytes of copied integer wait/fault code before
publishing completion. That copied span has no COFF relocations and uses numeric
BOOT fields, so UEFI runtime PE fixups cannot redirect a still-running wait loop.
The AP reads its actual guest CR3 and publishes it before completion; the BSP
verifies it equals the owned AP root and records the observation.

After every AP reaches that continuation, original firmware paging structures
are no longer an AP dependency. The loader may reclaim its own obsolete tables
after replacing its BSP root. The low LoaderCode page is consumed before
successful activation returns; no post-EBS FreePages call occurs. The retained
AP storage stays available until the OS replaces AP guest state with INIT/SIPI.
Raw monitor physical/private pointers are never firmware virtual pointers and
are not passed through ConvertPointer.

## Evidence

The frozen checkpoint is `work/native-loader-2026-09-14/summary.json`. It binds
source manifests, test logs, final build hashes, linked audits and individual
emulator scenarios. Earlier attempts remain separate and are not counted as
final evidence. The previous xAPIC checkpoint is untouched.

The loader fixture checks the installed table CRC using firmware's CRC service,
an actual stale-key EBS failure with no active CPUs, two final EBS notifications
before activation, nonvolatile GPRs and XMM6/XMM15 across success, and architectural
CPU state except EBS's legitimate IF change. It does not measure every xstate
component. The shared native capture contract and linked source audit supply
the remaining explicitly bounded preservation argument.

It installs an exclusively fixture-owned copy of the BSP PML4, then switches to
independent loader tables and fills the obsolete page with A5. It never overwrites
firmware-owned tables. Actual AP CR3 publications rule out a borrowed-root result
hidden by cached translations. A separate case changes the loader CR3 before EBS.
Runtime descriptors receive a512GiB virtual offset. After SetVirtualAddressMap,
the BSP removes every runtime identity PTE and reloads CR3, then calls GetTime
through the converted runtime table and function pointer. AP startup and xAPIC
NPF decoding still run afterward, exercising the current guest paging reader.

The final matrix covers2/24/32 CPUs, xAPIC, x2APIC, promotion and level IRQ paths,
older explicit-start/single/SMP profiles, and admission refusals. The production
boot image is built and audited, not executed. Emulator results belong only to
the pinned INIT/#SX QEMU backend and OVMF image recorded in each result.

## Remaining platform gates

The shared [Ryzen encryption owner](native-ryzen-encryption-admission.md) now
accepts the reviewed processor's advertised capability only with observed
disabled unsupported modes. SYS_CFG writes stop before hardware mutation.
Active SME/SEV/SNP/VMPL/multi-key modes are unsupported.

The real24-thread machine's extended xAPIC reset topology remains a blocker if
firmware leaves APs in xAPIC with IDs15 or higher: INIT clears extended destination
enable before SIPI. Existing x2APIC admission avoids that reset-width issue, but
promoting firmware-owned APs is not implemented here. Firmware's actual APIC
mode/version, initial xstate and writable table configuration still require
observation on the exact target. No physical boot, Windows image execution,
protection change, device/DMA isolation or timing baseline was performed.
Hyper-V/VBS nesting remains unsupported; Windows protection compatibility is
unvalidated. Unsupported exits continue to stop with authoritative VMCB/register
state; they do not advance RIP merely to continue booting.

## Primary references

UEFI2.11, local `UEFI_Spec_Final_2.11.pdf`, SHA256
`a64b8e442004b91becc3de9afaf8ca61b259a9a3b436accb6b3711ab5400cee9`:
§2.3.4.2 x64 ABI; §4.2 table headers/CRC; §7.4.6 printed203–204 successful
EBS, final notifications and retry restrictions; §8.4.1 printed236–237 runtime
mapping and PE fixups. These local pages were read directly. AMD APM2 rev3.44,
SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`,
§5.3.3/5.4 paging and §16.10 INIT state; PI1.10 MP Services references remain
in the prior lifetime review. The exact target PPR and encryption sections are
recorded in the linked Ryzen report.
