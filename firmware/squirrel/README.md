# Screamer PCIe Squirrel gateware

This directory owns the FPGA side of the delivery chain for the LambdaConcept
Screamer PCIe Squirrel. The board contains an XC7A35T-FGG484 FPGA and a 256 Mbit
SPI configuration flash.

The SPI flash is not a passive PCI option-ROM chip. It stores the FPGA
configuration image. Gateware must configure the PCIe endpoint's Expansion ROM
BAR and answer ROM read requests with the packaged DXE image.

> **Safety status:** the current transitional build still uses the inherited
> PCILeech control/TX stack and remains requester/DMA capable. Do not use it for a
> physical svmvisor bring-up. The first required gate is a new completion-only
> top level with no requester or arbitrary-TLP path, as specified in the roadmap.

The current repository step is defined by the
[first-light plan](../../docs/first-light-plan.md): make flashing manifest-gated,
build and read back the pinned non-enumerating recovery image, then implement the
completion-only endpoint and BAR0/JTAG observability. Resident record-only EFI
trace work may proceed in parallel, but no physical option-ROM candidate is
eligible before these hardware gates pass.

## Source boundary

- `rtl/` contains the svmvisor-owned read-only ROM leaf and completion-TLP
  responder.
- `vivado/` contains small batch-mode orchestration scripts.
- `openocd/` contains the checked-in Squirrel flashing configuration.
- `config.psd1` pins the upstream PCILeech Squirrel board support, LambdaConcept
  OpenOCD bundle, flash helpers, and their hashes.
- Generated projects, downloaded tools, and build output stay under
  `target/`.

The upstream source is used as board support rather than copied into the
repository. This keeps generated Xilinx IP and third-party HDL out of the
first-party source tree.

## Commands

For the new first-party completion-only implementation, use `build-endpoint.ps1`
and `test-endpoint.ps1`. See [implementation and current limits](../../docs/completion-only-endpoint.md).
The `build.ps1` commands below describe the older transitional build.

Install the pinned upstream source, OpenOCD, and the XC7A35T BSCAN-SPI proxy:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\firmware\squirrel\bootstrap.ps1
```

The process-scoped execution-policy override is needed on machines that retain
Windows PowerShell's default restricted policy. It does not change the system or
current-user policy.

Build the Rust DXE image and wrap it in a PCI 3.0 UEFI option-ROM header:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\firmware\squirrel\package-rom.ps1
```

The ROM uses the pinned base endpoint identity `10ee:0666`, class `ff0000`, and
is written to `target/firmware/squirrel/rom/svmvisor-dxe.rom`. An 8 KiB
`svmvisor-dxe.mem` image is emitted beside it for FPGA initialization, with
unused bytes padded to `0xff`. Packaging is done by the project-owned Rust tool
in `tools/rompack`; builds do not read or execute anything from `docs/`.

Run the focused RTL simulation:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\firmware\squirrel\test-endpoint.ps1 -Suite completer
```

The simulation covers the ROM pipeline, wire byte order, byte-enabled firmware
reads, and completion splitting at the default 64-byte Read Completion
Boundary.

Use `build-endpoint.ps1` for the current completion-only candidate. The legacy
workflow below targets the obsolete 4 KiB transitional design, is incompatible
with the current lifecycle payload, and must not be used for physical bring-up.

Legacy check of the configured Vivado installation and source checkout:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\firmware\squirrel\build.ps1 -CheckOnly
```

Generate the Vivado project without starting synthesis:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\firmware\squirrel\build.ps1 -GenerateOnly
```

Build the Squirrel gateware with the 4 KiB Expansion ROM BAR and packaged DXE
image using Vivado 2026.1:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\firmware\squirrel\build.ps1
```

That build can take roughly an hour. Its output is
`target/firmware/squirrel/svmvisor-squirrel.bin`.

Do **not** flash that output into the target: it is the transitional,
requester-capable image described in the safety warning above. The checked-in
`flash.ps1` is retained only for explicitly selected known-good recovery images
until the completion-only build emits a manifest carrying a passed netlist-policy
gate. The script now requires a manifest and verifies its part, image size,
SHA-256, image kind, and policy result before loading any hardware tools.

## Recovery build and flash guard

`build-recovery.ps1` builds a separate first-party top level for
`xc7a35tfgg484-2`. It uses no upstream HDL or PCIe IP. Its only fabric outputs
are fixed opposite LED levels, deasserted FT2232 reset, asserted FT601 reset,
inactive FT601 strobes, and inactive PCIe wake. Unused user IO is undriven.
Dedicated configuration JTAG is left available; actual update-port operation
and LED behavior still need the isolated-fixture test.

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\firmware\squirrel\build-recovery.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\firmware\squirrel\test-flash-manifest.ps1
```

The 2026-09-08 candidate built successfully in Vivado 2026.1. Its routed
netlist passed an exhaustive GND/VCC/OBUF primitive policy and all nine expected
output levels; DRC reported zero errors. The 199,580-byte image and exact
manifest are pinned in `recovery-pin.psd1`. The artifact directory recorded
there contains the image, routed checkpoint/netlist, reports, log, and manifest.
**Physical programming, readback, and recovery behavior remain untested.**

For a hardware-free check of this candidate:

```powershell
$pin = Import-PowerShellDataFile .\firmware\squirrel\recovery-pin.psd1
$candidate = $pin.ArtifactDirectory
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\firmware\squirrel\flash.ps1 `
    -ImagePath "$candidate/svmvisor-recovery.bin" `
    -ManifestPath "$candidate/manifest.json" -Recovery -CheckOnly
```

Recovery mode accepts only the separately pinned image/manifest pair. Rebuilds
never replace that pin automatically. Experimental mode requires
`image_kind = completion_only` and `completion_only_netlist_policy = PASS`;
the transitional build does not produce this manifest and is ineligible.
These are local artifact-integrity checks, not cryptographic approval of an
untrusted manifest. A physical run still needs the reviewed candidate and
fixture procedure described in the first-light plan.

When physically authorized, the programming path snapshots and rehashes the
image, uses the pinned OpenOCD helper's write/verify operation, retains an
image-length SPI readback, and compares its SHA-256. It leaves the BSCAN proxy
loaded instead of automatically activating the new image. It does not verify
unused flash bytes or establish that recovery works after a power cycle.

Programming or recovery requires the board to be powered from PCIe and its update
USB-C port to be connected. On Windows, FTDI interface 0 must use the WinUSB
driver (for example, assigned with Zadig).

## Intended end-to-end build

```text
svmvisor-dxe.efi
    -> UEFI PCI option-ROM wrapper
    -> FPGA ROM initialization data
    -> Squirrel gateware bitstream
    -> SPI flash via OpenOCD
```

The next gateware milestone is removal of the inherited requester/DMA and raw-TLP
paths. Physical enumeration and ROM dispatch happen only after the replacement
completion-only endpoint passes its simulation, synthesis, timing, and netlist
policy gates.

The complete staged bring-up, diagnostics, recovery, and acceptance gates are
in
[`docs/minimal-baremetal-bringup-roadmap.md`](../../docs/minimal-baremetal-bringup-roadmap.md).
