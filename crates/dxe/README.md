# DXE native child

This crate is the SVM-specific EFI child image: admission, resource
allocation, and the launch of the resident runtime. The CPU and VM-exit runtime
belongs in [`../hypervisor`](../hypervisor/README.md). The card option-ROM
driver that binds the card, delivers this image and observes the firmware
lifecycle is [`../card-loader`](../card-loader/README.md); the records the two
exchange live in [`../card-abi`](../card-abi/README.md).

## Where to work

| Source directory | Responsibility |
| --- | --- |
| `diagnostics/` | Resident launcher failure records. The records exchanged with the loader (`ResidentBootOptions`, `NativeResult`) live in [`../card-abi`](../card-abi/README.md). |
| `memory_attributes/` | Memory Attribute Protocol provider, registration, firmware access, and the F7 table qualification path. |
| `native/admission/` | Entry boundary capture and CPU, memory, cache, and rendezvous admission evidence. |
| `native/resources/` | Firmware-owned tables, image ranges, guest pages, arena allocation, and cache preparation. |
| `native/transition/` | The assembly transition, its fixed Rust state layout, and restoration canary. |
| `native/resident/` | Native callback activation, retained raw payload allocation, per-CPU observations/preparation and separately audited resident assembly. |
| `native/entry.rs`, `native/child_result.rs`, `native/returning.rs` | Native image entry, parent mailbox, and admitted returning execution. |
| `fixtures/` | Native transition fixtures and their negative cases. |

`lib.rs` exposes only the grouped library namespaces, for example
`native::transition::state`, `native::admission::cpu` or
`diagnostics::resident_boot`; there are no root compatibility aliases. Library
files that are also compiled by a test through `#[path]` name their siblings
with `super::` (for example `native::admission::cache_rendezvous`), and that
test's crate root provides the same sibling names.

`main.rs` selects the binary-only modules with explicit paths and feature gates.
These include image entry, resource ownership, and fixtures. They do not become
part of the library merely because they share a directory with public modules.
Assembly lives beside the Rust contract it implements and is selected by
`build.rs`.

## Entry flow

Every image of this package is a `native-*` feature selection; a UEFI build
without one is rejected. `cargo xtask resident` builds the production resident
image (`native-resident-boot`).

The native returning child follows:

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
