# Card loader ABI

This `no_std` crate owns the wire contract between the card option-ROM loader
in [`../card-loader`](../card-loader/README.md), the EFI child image it starts
(today [`../launcher`](../launcher/README.md)), and the resident runtime in
[`../hypervisor`](../hypervisor/README.md). It has no dependencies and knows
nothing about UEFI or SVM, so any child payload can use it without pulling in
the hypervisor.

| Module | Contract |
| --- | --- |
| `boot_options` | `ResidentBootOptions`: the 128-byte load-options record the loader hands to a resident child, and the preparation words the child reports back in it. |
| `endpoint` | `TerminalEndpoint`: the admitted card PCI function (configuration page, BAR0 page, build IDs), its identity constants and the configuration DWORD read. |
| `envelope` | The 128-byte header of the card's 1 MiB payload slot: the `SVMBPE01` (resident) and `SVMPE001` (returning, no loader builds it any more) magics, field offsets, flags and the PE policy limits; `Envelope::parse`, `parse_pe` / `parse_pe_kind`, `PeMetadata`, `ImageKind` and `EnvelopeError`, the parser the loader and `xtask` share. |
| `journal` | `JournalIo`, `CommitError`, the eight-DWORD record commit, and the 19-DWORD diagnostic payload and its bank commit. |
| `native_result` | `NativeResult`: the 128-byte result mailbox of a returning child. No image produces one any more; the loader's `delivery::child_image` still sizes the child's LoadOptions with it. |
| `package` | The `SVMRELO1` relocatable package that `cargo xtask resident` writes and the launcher embeds: `ARENA_BYTES`, `HANDOFF_OFFSET`, `Package::{parse, load}`, `PackageError`, `is_valid_arena`. Pure `core`; used by the native resident launcher. |

Every struct here is read by another binary: field order, sizes, magics and
version numbers are ABI. The crate root exports the modules only; users name
items by their module path, for example
`svmvisor_card_abi::journal::commit_record`.
