# svmvisor card firmware

Drives the FPGA PCIe card (PCILeech Squirrel, XC7A35T + IS25LP256D SPI flash).
The card does three things:

- serves the hypervisor DXE loader to the host as a **PCIe option ROM** (baked
  into the FPGA bitstream),
- serves the hypervisor payload from a **read-only flash-backed BAR** (the 1 MiB
  payload slot at flash offset `0x400000`),
- captures diagnostics (journal / per-CPU snapshot) read back over JTAG.

The flash image is 5 MiB: the FPGA configuration bitstream below `0x400000`
(which contains the ROM/loader) and the 1 MiB payload slot at `0x400000`.

## Source boundary

- `rtl/` – svmvisor-owned completion-only endpoint (read-only ROM leaf, payload
  SPI reader, journal/snapshot transports). See [rtl/README.md](rtl/README.md).
- `vivado/` – batch-mode synthesis scripts (`endpoint.tcl`, `recovery.tcl`).
- `openocd/` – checked-in OpenOCD flashing/readback configurations.
- `config.psd1` – pins the upstream Squirrel board support, OpenOCD bundle,
  flash proxy, and their hashes.
- `card-payload.ps1`, `openocd/card-payload-*.cfg` – payload-slot-only flashing for the
  development loader (see "Fast iteration"); `test-card-payload.ps1` and
  `tests/test_payload_flash_cfg.py` test them offline.
- `candidate-pin.psd1` – pins the one built card candidate consumed by
  `flash-card.ps1` (re-baselined per build, see below).
- `recovery-pin.psd1` – pins the recovery bitstream candidate.
- Generated projects, downloaded tools and build output stay under `target/`.

## The six jobs, in order

All PowerShell commands run from the repository root. Prefix each with
`powershell.exe -NoProfile -ExecutionPolicy Bypass -File` on machines that keep
the default restricted policy.

### 1. Setup (one time)

Fetch the pinned upstream gateware, OpenOCD and the XC7A35T BSCAN-SPI proxy:

```powershell
.\firmware\card\bootstrap.ps1
```

Tool/board pins live in `config.psd1`.

### 2. Build and audit the resident payload

```powershell
cargo xtask resident --output target/native-resident/<fresh-name>
```

Builds the production resident image and audits what was actually linked. The
output directory is the audited **resident build directory** used below. See
[../../crates/xtask/README.md](../../crates/xtask/README.md).

### 3. Build the card candidate

Offline packaging (no bitstream); add `-BuildFpga` to also synthesize and route
the endpoint in Vivado (~1 hour):

```powershell
.\firmware\card\build-card.ps1 `
    -PayloadPath <reviewed native-child.efi> `
    -PayloadSha256 <64-hex sha256 of that file> `
    -PayloadEvidencePath <child build evidence file> `
    -ResidentBuildPath target/native-resident/<fresh-name> `
    [-BuildFpga]
```

Only `NativeResidentBoot` is produced. It runs `verify-resident-build.py`
against the resident build, packages the PE payload
(`package-payload.py`), rebuilds the DXE loader with the payload PE-header pin,
and packs the 32 KiB option ROM. With `-BuildFpga` it also produces the
configuration bitstream and the combined 5 MiB image.

### 4. Verify the build

```powershell
python firmware/card/verify-resident-build.py --evidence target/native-resident/<fresh-name> --image <native-child.efi>
```

Consumes the audited resident build directory and confirms the image and
retained sources match it. Its current-source check runs `cargo xtask sources`
from the repository root and compares the JSON to the audit checkpoint.

### 5. Flash and read back

Offline dry run first (accesses no hardware):

```powershell
.\firmware\card\flash-card.ps1 -Action CheckOnly
```

Then, when physically authorized:

```powershell
.\firmware\card\flash-card.ps1 -Action Program -ConfirmFlash
```

Program double-backs-up the current 5 MiB, verifies the card still holds the
pinned known-working image, writes the exact 80 sectors, and does a full 5 MiB
readback. It never activates the new image.

Read diagnostics back (offline decode of retained output, or `--live` over the
programming USB using `openocd/read_snapshot.cfg`):

```powershell
python firmware/card/read_snapshot.py --input <retained openocd output>
python firmware/card/read_snapshot.py --live
```

Roll a card back to a prior Program session's known-working backup:

```powershell
.\firmware\card\flash-card.ps1 -Action Restore -RestoreSession <32-hex session id> -ConfirmFlash
```

### 6. Recovery (un-brick)

Build the separate non-enumerating recovery bitstream and flash it only through
the pinned recovery guard:

```powershell
.\firmware\card\build-recovery.ps1
.\firmware\card\test-flash-manifest.ps1
$pin = Import-PowerShellDataFile .\firmware\card\recovery-pin.psd1
.\firmware\card\flash.ps1 -ImagePath "$($pin.ArtifactDirectory)/svmvisor-recovery.bin" `
    -ManifestPath "$($pin.ArtifactDirectory)/manifest.json" -Recovery -CheckOnly
```

`flash.ps1` accepts only the pin's image/manifest pair and requires
`image_kind = completion_only`/`non_enumerating_recovery` with a passed netlist
policy. Rebuilds never update `recovery-pin.psd1` automatically.

## Fast iteration (dev loader)

The pinned loader (`card-resident-loader`) has the payload's exact 128-byte
header compiled in, so every hypervisor change means a new loader, a new ROM, a
new bitstream (~1 h of Vivado) and a full 5 MiB flash. The **development
loader** (`card-resident-dev-loader`) instead adopts the header it finds at the
start of the payload slot, so a hypervisor change needs only the 1 MiB payload
slot rewritten. The ROM and the bitstream stay constant. Pinned mode remains the
default everywhere and is unchanged.

### One-time setup

1. `cargo xtask card-loader-dev` – builds the dev loader and packs the 32 KiB
   option ROM exactly like `build-card.ps1` does (quick check that it builds
   and fits; prints `loader_sha256`/`rom_sha256`).
2. `cargo xtask card-dev` – any current payload, to seed the slot.
3. Build the dev bitstream (the one remaining slow step):

   ```powershell
   $b = Get-Content target\card-dev\latest
   .\firmware\card\build-card.ps1 -PayloadPath "$b\resident\driver.efi" `
       -PayloadSha256 (Get-FileHash "$b\resident\driver.efi").Hash.ToLower() `
       -PayloadEvidencePath "$b\resident\summary.json" -ResidentBuildPath "$b\resident" `
       -DevLoader -BuildFpga
   ```

   `-DevLoader` builds the loader with `card-resident-dev-loader`, sets no
   `SVMVISOR_CARD_PE_HEADER`, and records `loader_mode = dev` /
   `loader_feature` in `manifest.json`. Its `loader_sha256` must equal the one
   step 1 printed (the loader build is reproducible).
4. Flash that full 5 MiB candidate with the existing careful procedure (job 5:
   re-baseline `candidate-pin.psd1`, `CheckOnly`, then `Program -ConfirmFlash`).
   Keep that session: its `before-a.bin` is your full-image way back.
5. Re-baseline `candidate-pin.psd1` `KnownWorking` to the new combined image.
   Note that after the first `ProgramPayload` the card's 5 MiB no longer hashes
   to `KnownWorking`; a later full `Program` needs `KnownWorking` recomputed
   from a fresh full backup, or a `Restore` first.

### The loop

```powershell
# edit the hypervisor, then:
cargo xtask card-dev --flash            # build + audit + package + offline check + payload flash
# power-cycle the target machine (nothing is activated for you)
cargo xtask card-snapshot               # read_snapshot.py --live into target\card-snapshots\<utc>-<id>
```

`cargo xtask card-dev` without `--flash` does everything offline and prints the
exact `flash-card.ps1 -Action ProgramPayload ...` command. `--adapter-khz N`
selects the JTAG clock of the payload stages (default 1000). The parent journal
record (detail 8, phase `0x10`) decodes `loader_mode: dev|pinned` (bit 11 of
journal word 4), so a snapshot shows which loader delivered the payload.

`ProgramPayload` (`card-payload.ps1`, `openocd/card-payload-{backup,program}.cfg`):

1. same FPGA/flash identity and 512-sector geometry check as the full procedure
   (`card-transport.cfg`, unchanged and still hash-pinned, always at 1000 kHz);
2. reads the 1 MiB slot twice (`slot-before-a/b.bin`), requires identity, keeps
   them as the session backup;
3. plans 64 KiB sectors: **touch** every slot sector whose current bytes differ
   from the expected image – the sectors covering header + child, plus any sector
   beyond the new extent that is not already `0xff` (stale bytes of a longer old
   payload); **erase** the touched sectors that are not already erased; **write**
   the touched sectors whose expected content is not all `0xff`. Example: a
   150 KiB child replacing a 300 KiB one erases sectors 64..68 and writes 64..66;
4. re-verifies the slot against the backup, then erases/writes only those
   sectors (`flash erase_sector` / `flash write_bank` per sector);
5. reads the full 1 MiB slot back; the host requires SHA-256 identity with the
   expected slot image (header + child + `0xff`);
6. records hashes, sector lists, byte counts and timings in the session's
   `result.json` under `target/firmware/card/card-payload-sessions/<id>/`.

`-Action CheckPayload -BuildPath <dir>` performs every offline step of the above
(pins, slot self-consistency, payload manifest, `verify-resident-build.py`
against the build's `resident/` directory) and touches no hardware.

### What is kept, what is given up

Kept: the child is still bound to the header's SHA-256 and PE metadata by the
loader (same parser, same refusal status as the pinned loader); every tool and
cfg executed is size/SHA-256 pinned; explicit `-ConfirmFlash`; the operation
lock shared with the full procedure; a double-read backup of the slot in every
session; full-slot readback compare; no activation. Range confinement is
asserted three times: sector numbers are validated as integers 64..79 in
`card-payload.ps1`, serialized as digits and spaces only, and re-validated in
Tcl before the transport is even opened. The FPGA configuration below
`0x400000` is never erased or written, so an interrupted payload flash cannot
brick the card: the loader refuses a half-written slot (digest mismatch) and
the machine boots without the hypervisor.

Given up: the ROM no longer pins one exact payload. Whatever structurally valid,
self-consistent `SVMBPE01` slot is in flash gets loaded at boot; anyone who can
write the SPI flash (JTAG/USB access to the card) chooses the hypervisor. The
payload build is not candidate-pinned or independently reviewed either.
Neither loader inspects the slot tail beyond header + child; only the flash
procedure enforces `0xff` there. This is a bench mode for your own machine.

### Getting back to a known-good payload

```powershell
.\firmware\card\flash-card.ps1 -Action RestorePayload -RestoreSession <32-hex payload session id> -ConfirmFlash
```

writes that session's `slot-before-a.bin` back with the same discipline (fresh
double backup, minimal sectors, full-slot readback). `RestorePayload` is a
separate action rather than an extension of `Restore`, because `Restore` is
bound to the pinned candidate and the `KnownWorking` 5 MiB image, which the dev
loop deliberately leaves. For everything else (loader, bitstream), use the full
`Restore` of a full `Program` session, or the recovery bitstream (job 6).

### What still needs Vivado

RTL changes and **loader** changes (anything under `crates/dxe` that the
`card-resident-dev-loader` image links, including the header policy) still need
a new ROM and therefore a new bitstream plus a full flash. A possible future
shortcut, not implemented: `updatemem` can replace the ROM block-RAM init data
in an existing routed bitstream without re-running synthesis and routing.

### First live run checklist

- [ ] `cargo xtask card-dev` (no `--flash`) passes, or run
      `flash-card.ps1 -Action CheckPayload -BuildPath <dir>` yourself first.
- [ ] You hold a full 5 MiB backup session (`before-a.bin`/`before-b.bin` from a
      `Program` run) of the dev bitstream + a known-good slot.
- [ ] The card runs the **dev** bitstream: a pinned loader refuses any payload
      other than its own (journal status `COMPROMISED_DATA`, stage 0).
- [ ] First `ProgramPayload` at the default 1000 kHz. Read `result.json`: sector
      lists inside 64..79, `status = verified_not_activated`, timings.
- [ ] Power-cycle, `cargo xtask card-snapshot`, confirm `loader_mode: dev`.
- [ ] Try `RestorePayload` once, so the way back is proven before you need it.
- [ ] Only then try `--adapter-khz 5000`, `10000`, ... A marginal clock fails the
      identity of the two backup reads, the pre-write verify or the final
      readback compare; on any failure drop back to 1000 kHz and rerun (the
      sector plan is recomputed from a fresh backup, so reruns are safe).
- [ ] Nothing here has been run on hardware yet: `adapter speed` after the
      transport's `init`, per-sector `flash erase_sector`/`write_bank` on the
      `jtagspi` bank, and the timing estimates are unverified until this run.

## Safety model

- **Double backup, full readback, no activation.** Program/Restore read the
  whole 5 MiB twice before erase, verify the card still holds the pinned
  known-working image, write only the 80 target sectors, read the entire 5 MiB
  back and compare its SHA-256, then stop with the BSCAN proxy loaded — they
  never issue a reset-to-configuration or activation command. Target Windows
  configuration (Hyper-V/VBS coexistence is unsupported) is a separate operator
  action.
- **Pins everywhere.** Every consumed artifact (image, tools, cfgs, validation
  library) is size- and SHA-256-checked before use. Hardware/tool pins live in
  `flash-card.ps1`; the one candidate's pins live in `candidate-pin.psd1`.
- **What `CheckOnly` proves.** Only that the pinned candidate, its evidence, the
  independent resident audit, and the firmware fixtures are internally
  consistent offline. It accesses no hardware and does not prove the image runs
  on the card.

## Re-baselining `candidate-pin.psd1`

`candidate-pin.psd1` names one built candidate and its evidence. After a fresh
`cargo xtask resident` + `build-card.ps1` run, recompute every value in it
(`Candidate`, `ResidentBuild`, `KnownWorking`, and each `Inputs` hash/byte
count) from the new candidate and evidence directories. `flash-card.ps1` loads
and strictly validates the file (exact key set, lowercase 64-hex hashes, byte
counts, no path escapes) and refuses on any mismatch or missing artifact.

## When Vivado is needed

AMD Vivado 2026.1 (`config.psd1: VivadoRoot`) is required only for:

- `build-card.ps1 -BuildFpga` – full endpoint synthesis and route (~1 hour;
  re-run whenever the loader/ROM changes, see rtl/README.md),
- `build-recovery.ps1` – recovery bitstream synthesis and route,
- `test-endpoint.ps1` – RTL simulation (`xvlog`/`xelab`/`xsim`; fast).

Every other job here is host-only and needs no Vivado.
