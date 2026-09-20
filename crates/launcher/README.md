# Native resident launcher

Two UEFI images take part in a boot, and the words are used strictly. The
**loader** ([`../card-loader`](../card-loader/README.md)) is the generic
driver in the card's option ROM: it binds the card, reads the payload slot,
starts the EFI child image it finds there and observes the firmware
lifecycle. It knows nothing about SVM. The **launcher** is this crate
(`svmvisor-launcher`): the SVM-specific EFI child image the loader starts. It
does admission and resource allocation and launches the resident runtime. The
CPU and VM-exit runtime itself belongs in
[`../hypervisor`](../hypervisor/README.md); the records loader and launcher
exchange live in [`../card-abi`](../card-abi/README.md).

## Where to work

| Source directory | Responsibility |
| --- | --- |
| `diagnostics/` | Resident launcher failure records. The record exchanged with the loader (`ResidentBootOptions`) lives in [`../card-abi`](../card-abi/README.md). |
| `native/admission/` | Entry boundary capture (`boundary.S`, `NativeBoundary`), MP Services inventory and the firmware memory map. |
| `native/resident/` | Native callback activation, retained raw payload allocation, per-CPU observations/preparation and separately audited resident assembly. |

`lib.rs` exposes only the grouped library namespaces, for example
`native::admission::cpu`, `native::resident::allocation` or
`diagnostics::resident_boot`; there are no root compatibility aliases.
`tests/native_cpu.rs` also compiles `native/admission/cpu.rs` through `#[path]`.

`main.rs` selects the binary-only module `native/resident/activation/` with an
explicit path and feature gate. It does not become part of the library merely
because it shares a directory with public modules. Assembly lives beside the
Rust contract it implements: `build.rs` assembles `resident/bridge.S`,
`resident/boot.S` and `resident/physical.S` (the first two include
`admission/boundary.S`); `cargo xtask resident` assembles `resident/runtime.S`,
`resident/irq.S` and `resident/fault.S` into the payload.

## Entry flow

The package builds one image, the resident child. The features form a chain
(`native-preflight`, `native-resident`, `native-resident-test`,
`native-resident-smp-prepare`, `native-resident-smp-activate`,
`native-resident-guest-startup`, `native-resident-boot`,
`native-resident-low-runtime`); a UEFI build without `native-resident` is
rejected. `cargo xtask resident` builds the production image
(`native-resident-boot`, with `--low-runtime` also
`native-resident-low-runtime`) and embeds the resident payload named by
`SVMVISOR_RESIDENT_PAYLOAD`.

The production image follows:

```text
main.rs: efi_main
  -> native/resident/activation/install.rs: read the loader's options, hook
     ExitBootServices, allocate and load the payload, admit the processors
  -> native/resident/boot.S: after the original ExitBootServices succeeds, capture
     the boundary (admission/boundary.S) and call activation/boot_handoff.rs
  -> native/resident/activation/physical_boot.rs + resident/physical.S: start the
     application processors and take each admitted processor into the payload
  -> native/resident/bridge.S + activation/callback.rs: the qualified callback
     capture through which a processor enters the resident runtime
```

Keep firmware calls and allocation on the DXE side of this boundary. New
persistent CPU state, guest runtime policy, and VM-exit handling should be owned
by `svmvisor-hypervisor`.
