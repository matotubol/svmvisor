# xtask

The one host-side entry point of the build pipeline. `.cargo/config.toml`
aliases it, so from anywhere in the workspace:

```powershell
cargo xtask resident --output target/native-resident/<fresh-name> [--low-runtime]
cargo xtask sources
cargo xtask card-dev | card-snapshot | card-loader-dev   # see "Card development loop"
```

Std-only apart from `sha2`; nothing here runs on the target. Only
`card-dev --flash` and a live `card-snapshot` reach hardware, through the
scripts in `firmware/card`.

## `cargo xtask resident`

Builds the production native-resident image and audits what was actually
linked. The output directory must not exist; it keeps every artifact and one
`<step>.log` per tool invocation, also when a step fails.

1. Hash and copy every build input into `source/` (`source-manifest.json`).
2. `cargo rustc` the `crates/resident-payload` staticlib for
   `x86_64-unknown-none` (`-C relocation-model=static -C code-model=small`)
   -> `payload.a`.
3. `clang --target=x86_64-unknown-none` assembles `runtime.S`, `irq.S` and
   `fault.S` from `crates/dxe/src/native/resident/`.
4. `ld.lld --gc-sections --emit-relocs -T crates/resident-payload/payload.ld`
   -> `payload.elf`; `llvm-objcopy -O binary` -> `payload.bin`.
5. Package the retained relocations (`src/relocations.rs`) -> `payload.reloc`,
   the `SVMRELO1` container that `crates/firmware-handoff/src/layout.rs` loads
   and `crates/dxe/build.rs` embeds through `SVMVISOR_RESIDENT_PAYLOAD`.
6. Audit the linked payload (`src/audit.rs`): no undefined symbols
   (`llvm-nm`); over the `llvm-objdump` disassembly, no FP/SIMD/xstate
   instruction, debug registers touched only by the exact DR0-3 reset helper,
   all 256 host fault frames, the capture sequence, the IRQ window gates and
   the #SX shape.
7. `cargo build --locked -p svmvisor-dxe --target x86_64-unknown-uefi
   --release --features native-resident-boot[,native-resident-low-runtime]`
   -> `driver.efi`, which must be an EFI runtime driver (PE subsystem 12).
8. Audit `physical.S` (the copied AP wait span lives in `.rdata`, has no COFF
   relocations and is at most 3840 bytes) and disassemble `boot.S`.
9. Re-hash the inputs; any change during the build fails it. Write
   `summary.json` (profile, audit results, SHA-256 of every artifact).

`firmware/card/verify-resident-build.py` consumes the output directory.

`cargo xtask sources` prints the current source manifest, the same JSON a
build records as `source-manifest.json`.

## Card development loop

For a card running the development loader (`card-resident-dev-loader`, see
[../../firmware/card/README.md](../../firmware/card/README.md), "Fast iteration"):

```powershell
cargo xtask card-dev [--any-runtime] [--flash] [--adapter-khz N]
cargo xtask card-snapshot [--input <log>] [--manifest <manifest.json>]
cargo xtask card-loader-dev
```

* `card-dev` runs the resident build above (in-process) into
  `target/card-dev/<utc>-<id>/resident`, with `--low-runtime` by default:
  this repository targets one board, whose firmware only retains the runtime
  allocation below 1 GiB (`--any-runtime` builds the generic profile;
  `card-dev.json` records `low_runtime`). It packages the payload slot with
  `firmware/card/package-payload.py --resident` into `payload/`, runs
  `verify-resident-build.py` and the offline `flash-card.ps1 -Action
  CheckPayload`, then prints the header digest, sizes, the slot sectors and the
  exact flash command. It writes `card-dev.json` and the text pointer
  `target/card-dev/latest`. Only with `--flash` does it run `flash-card.ps1
  -Action ProgramPayload -ConfirmFlash` (`--adapter-khz`, default 1000,
  100..30000): that is the only hardware access, and nothing is activated.
* `card-snapshot` runs `firmware/card/read_snapshot.py` (`--live` unless
  `--input` is given; its other options pass through) with `--output-dir
  target/card-snapshots/<utc>-<id> --summary`, and updates
  `target/card-snapshots/latest`. `--live` reads the card over JTAG.
* `card-loader-dev` builds the development loader (`--package
  svmvisor-card-loader --profile dxe --features card-resident-dev-loader
  --target x86_64-unknown-uefi`) and packs the 32 KiB
  option ROM with `crates/rompack` using the same arguments as
  `firmware/card/build-card.ps1`, into `target/card-dev/loader/<utc>-<id>/`.
  `build-card.ps1 -DevLoader -BuildFpga` then produces the one bitstream.

Needs `python` and a PowerShell host (`pwsh`, else `powershell`) on `PATH`.

## Required tools on `PATH`

* `clang`, `ld.lld`, `llvm-objcopy`, `llvm-nm`, `llvm-objdump` (LLVM)
* `cargo` with the Rust targets `x86_64-unknown-none` and
  `x86_64-unknown-uefi` (`rustup target add x86_64-unknown-none x86_64-unknown-uefi`)

## Tests

```powershell
cargo test -p xtask
```

covers the card-loop argument, timestamp and header helpers, the relocation packager on synthetic ELF fixtures, every audit on
passing and mutated disassembly, and the JSON writer against a Python-generated
reference (`src/expected-json.txt`).
