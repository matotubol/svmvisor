# Card loader ABI

This `no_std` crate owns the wire contract between the card option-ROM loader
in [`../card-loader`](../card-loader/README.md), the EFI child image it starts
(today [`../dxe`](../dxe/README.md)), and the resident runtime in
[`../hypervisor`](../hypervisor/README.md). It has no dependencies and knows
nothing about UEFI or SVM, so any child payload can use it without pulling in
the hypervisor.

| Module | Contract |
| --- | --- |
| `boot_options` | `ResidentBootOptions`: the 128-byte load-options record the loader hands to a resident child, and the preparation words the child reports back in it. |
| `endpoint` | `TerminalEndpoint`: the admitted card PCI function (configuration page, BAR0 page, build IDs), its identity constants and the configuration DWORD read. |
| `envelope` | The 128-byte header of the card's 1 MiB payload slot: the `SVMCRD01`, `SVMPE001` and `SVMBPE01` magics, field offsets, flags and the PE policy limits; `Envelope::parse`, `parse_pe` / `parse_pe_kind`, `PeMetadata`, `ImageKind` and `EnvelopeError`, the parser the loader and `xtask` share. |
| `journal` | `JournalIo`, `CommitError`, the eight-DWORD record commit, and the 19-DWORD diagnostic payload and its bank commit. |
| `native_result` | `NativeResult`: the 128-byte result mailbox of the returning child, and the fixed multi-exit profile constants. |
| `package` | The `SVMRELO1` relocatable package an `SVMCRD01` envelope wraps: `ARENA_BYTES`, `HANDOFF_OFFSET`, `Package::{parse, load}`, `PackageError`, `is_valid_arena`. Pure `core`; shared by the loader's load-only image, the native resident launcher and the emulator handoff. |

Every struct here is read by another binary: field order, sizes, magics and
version numbers are ABI. The crate root exports the modules only; users name
items by their module path, for example
`svmvisor_card_abi::journal::commit_record`.
