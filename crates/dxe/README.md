# DXE firmware integration

## Current native implementation

The working tree moves to an exclusive x2APIC/x2AVIC guest interface.
Device interrupts reach the guest through the resident host IRQ bridge. The
IVRS discovery module (`native/resident/iommu.rs`) and the IOMMU MMIO trap
module (`iommu_boot.rs`) were removed, so the boot profile no longer discovers,
traps or gates on the AMD IOMMU; Windows keeps using it natively.

The profile is code-complete and host-tested, but it has not been flashed or
boot-tested. The [current handoff](../../docs/handoff-2026-09-16.md) separates
the working tree from the last verified diagnostic image. The
[completion record](../../docs/x2avic-completion-2026-09-16.md) lists its
layout, stop codes and validation. Older physical milestones and APIC profile
descriptions below are historical evidence, not rewrite validation.

This crate owns UEFI driver binding, image delivery, admission, resource
allocation, and firmware lifecycle observation. The CPU and VM-exit runtime
belongs in [`../hypervisor`](../hypervisor/README.md).

The current physical milestone is a **bounded returning guest**: 32 CPUID/query
rounds and a final STOP completed 65 guest entries and exits, followed by firmware
restoration, cleanup, and the canary checks. Windows is not running inside a
resident hypervisor yet. The detailed multi-exit contract report was never
imported into this repository.

## Where to work

| Source directory | Responsibility |
| --- | --- |
| `firmware/` | Option-ROM driver binding, PCI I/O, BAR mapping, CPU sampling, and lifecycle events. |
| `delivery/` | Card payload validation, EFI child loading, and parent-side ownership and cleanup. |
| `diagnostics/` | Journal serialization, lifecycle traces, child result records, and outcome classification. |
| `memory_attributes/` | Memory Attribute Protocol provider, registration, firmware access, and the F7 table qualification path. |
| `native/admission/` | Entry boundary capture and CPU, memory, cache, and rendezvous admission evidence. |
| `native/resources/` | Firmware-owned tables, image ranges, guest pages, arena allocation, and cache preparation. |
| `native/transition/` | The assembly transition, its fixed Rust state layout, and restoration canary. |
| `native/resident/` | Native callback activation, retained raw payload allocation, per-CPU observations/preparation and separately audited resident assembly. |
| `native/entry.rs`, `native/child_result.rs`, `native/returning.rs` | Native image entry, parent mailbox, and admitted returning execution. |
| `fixtures/` | Native transition fixtures and their negative cases. |

`lib.rs` exposes only the grouped library namespaces, for example
`native::transition::state`, `native::admission::cpu` or
`diagnostics::outcome`; there are no root compatibility aliases. Library files
that are also compiled by a test through `#[path]` name their siblings with
`super::` (for example `native::admission::cache_rendezvous`), and that test's
crate root provides the same sibling names.

`main.rs` selects the binary-only modules with explicit paths and feature gates.
These include firmware driver state, image entry, resource ownership, and
fixtures. They do not become part of the library merely because they share a
directory with public modules. Assembly lives beside the Rust contract it
implements and is selected by `build.rs`.

## Entry flow

The card returning-loader image follows:

```text
main.rs: efi_main
  -> firmware/driver.rs: install and bind
  -> delivery/adapter.rs + delivery/returning.rs: validate and start child
  -> diagnostics/: classify the returned child result
  -> firmware/lifecycle.rs: journal firmware lifecycle events
```

The separately built native returning child follows:

```text
native/admission/boundary.S: capture original firmware state
  -> main.rs: svmvisor_native_efi_main_inner
  -> native/entry.rs: attach result mailbox and collect admission evidence
  -> native/returning.rs: admit CPU/cache/memory and prepare owned resources
  -> native/transition/run.S: execute the bounded guest and restore host state
  -> native/transition/canary.*: check restored execution state
  -> native/returning.rs + native/child_result.rs: clean up and publish result
```

Keep firmware calls and allocation on the DXE side of this boundary. New
persistent CPU state, guest runtime policy, and VM-exit handling should be owned
by `svmvisor-hypervisor`; the currently proven returning transition remains here
with its firmware restoration and admission contracts.

The [native resident continuation report](../../docs/native-resident-continuation.md)
records the Windows-first work. The preparation library is exposed by
`native-preflight`; it does not select a resident firmware image or enable SVM.
The [resident activation result](../../docs/native-resident-activation.md)
records executed single-CPU callback/EBS checks. The
[per-CPU preparation result](../../docs/native-percpu-preparation.md) covers
inventory and separate runtime copies, without claiming AP activation.
The [post-EBS activation check](../../docs/native-startup-activation.md) adds
actual AP entry through an explicit diagnostic consumer, with x2APIC startup
refusal and retained per-CPU continuations. It is not a Windows loader hook.

## Build profiles

Features select different images and must not be combined indiscriminately.
`--all-features` is intentionally invalid.

| Feature | Image behavior |
| --- | --- |
| No features | Resident option-ROM driver records firmware lifecycle events. |
| `card-returning-loader` | Parent loader validates the pinned child envelope, starts it, and records its result. Requires `SVMVISOR_CARD_PE_HEADER` for UEFI builds. |
| `card-resident-loader` | Separate subsystem12 resident child envelope and explicit EBS-hook armed acknowledgement; retains child/input/controller ownership through reset. See the resident card delivery report. |
| `card-load-only` | Validates and prepares a payload without executing it. Requires `SVMVISOR_CARD_PAYLOAD_SHA256` for UEFI builds. |
| `native-preflight` | Separate native child captures entry state and observes admission evidence. |
| `native-resident`, `native-resident-smp-activate`, `native-resident-guest-startup` | Layers of the boot profile below (raw resident payload, post-EBS physical x2APIC INIT/SIPI with per-CPU continuation, guest INIT/SIPI ownership). `tools/native-resident/build.py` no longer builds them on their own. |
| `native-resident-test`, `native-resident-smp-prepare` | Former QEMU diagnostic output and preparation-only profiles; no remaining build script selects them. |
| `native-resident-boot` | Native loader EBS interposer, broadcast physical x2APIC bootstrap and guarded guest startup; the only profile `tools/native-resident/build.py` builds. Optional card options provide BSP-only pre-Windows journal evidence. |
| `native-resource-observe` | Adds native resource and cache observation to preflight. |
| `native-returning` | Admitted returning guest, including the F7 memory-attribute path and full restoration checks. |
| `memory-attribute-provider`, `memory-attribute-firmware`, `memory-attribute-probe`, `memory-attribute-f7` | Layered protocol and qualified firmware-access support; dependencies are defined in `Cargo.toml`. |
| `native-transition-test` and `native-transition-*` variants | Disposable emulator tests, including multi-exit and deliberately failing guest cases. |

The native child and card loader are separate EFI images. Native returning and
transition fixtures are mutually exclusive. Negative multi-exit variants are
selected individually; the existing compile-time checks enforce these rules.

## Local checks and artifact provenance

Run focused host checks from the repository root, for example:

```powershell
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-returning
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features card-returning-loader
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features memory-attribute-probe
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-preflight
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-resident-boot
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-resident-low-runtime
```

`cargo build-dxe` selects the size-constrained UEFI profile. Native child and card
package builds also need their existing artifact preparation and audit tools;
building the Rust crate alone does not validate an installable card image.

Changes to assembly, record layouts, admission policy, or their source paths must
be reflected in the active audit and fixture tools. Historical manifests describe
the exact source tree and binaries from their recorded run; leave those records
intact and produce new evidence for rebuilt artifacts. Directory organization
does not transfer the physical result of an older image to a newly built image.

## Normal native loader profile

`native-resident-boot` selects SMP guest startup plus the successful-EBS-return
interposer and never emits diagnostic output.
Use `python tools/native-resident/build.py --output work/NEW --boot` to build
and audit without executing firmware.

The build completes every check: the payload link, relocation packaging, the
undefined-symbol check, and the debug-register, host-fault and FP/SIMD/xstate
audits. Guest INIT now calls `svmvisor_resident_reset_guest_debug`, so the
linker keeps the helper that the debug-register audit requires.

Each linked payload must end at or below `X2AVIC_BACKING_ALIASES_OFFSET`: 0xd4000
from the slot base, or 0x1d4000 as linked. `payload.ld`, the runtime `prepare`
and `directory_valid` all check this bound.

In every private host root, pages 0xd4000-0xf3fff are RW/NX aliases of the
pool slots' AVIC backing pages; pages for absent slots stay unmapped. Before any
root is used, `common_backing_offset` requires one pool and one backing offset
across all directories. `host_closure` walks every alias at install, in the AP
admission observer and in each CPU's callback before arm.

The resident directory is version 10, and host APIC IDs must be at most 254.

For the specific Ryzen first-boot platform, `--boot --low-runtime` explicitly
selects `native-resident-low-runtime`: RuntimeServicesCode AllocateMaxAddress
with uppermost reservation byte below1GiB. UEFI2.11 §7.2.1 limits its generic
AnyPages mandate to drivers not targeted for a specific implementation. This
profile needs actual firmware allocation success and all existing runtime-map,
ownership and RW/X/WB checks; it does not claim portable firmware support.
The ordinary build still uses AnyPages. A24CPU pool requests26MiB and retains
24MiB after alignment trimming. Both profiles reject malformed ranges and
release owned allocations on failure. See `docs/native-resource-placement.md`.
Every CPU must already run an enabled x2APIC (CPUID.1:ECX[21] and APIC_BASE
EXTD) with AVIC/x2AVIC; there is no xAPIC path. The BIOS x2APIC setting is
therefore a physical prerequisite. IOMMU capabilities such as IVRS XTSup are
no longer admission inputs. `docs/native-bootstrap-apic-admission.md`
records the earlier xAPIC-era admission. See [loader handoff](../../docs/native-loader-handoff.md)
for owned AP paging, actual witnesses and remaining physical platform gates.
