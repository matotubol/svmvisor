# Card option-ROM loader

This crate is the UEFI driver in the card's option ROM. It binds the card's PCI
function, reads the payload slot through BAR0, starts the EFI child image found
there through the firmware's own image services, and journals what happened. It
knows nothing about SVM: it depends on [`../card-abi`](../card-abi/README.md)
and `uefi-raw` (plus `sha2` for the resident loaders). The child it starts
today is the native resident launcher in [`../launcher`](../launcher/README.md).

## Where to work

| Source directory | Responsibility |
| --- | --- |
| `firmware/` | Option-ROM driver binding, PCI I/O, BAR mapping, CPU sampling, and lifecycle events. |
| `delivery/` | Card payload validation, EFI child loading, and parent-side ownership and cleanup. |
| `diagnostics/` | Journal serialization and lifecycle traces. The record exchanged with a child (`ResidentBootOptions`) lives in [`../card-abi`](../card-abi/README.md). |

`lib.rs` exposes only the grouped library namespaces, for example
`delivery::child_image` or `diagnostics::trace`; there are no root
compatibility aliases.

`main.rs` selects the binary-only modules (`firmware/*`, `delivery/adapter.rs`)
with explicit paths and feature gates. They do not become part of the library
merely because they share a directory with public modules.

## Images

One package builds three images, selected by feature (`cargo build-card-loader
[--features <feature>]`, profile `rom`):

| Feature | Image |
| --- | --- |
| none | Record-only driver: binds the card and journals lifecycle events. |
| `card-resident-loader` | Starts the resident child pinned by `SVMVISOR_CARD_PE_HEADER`. |
| `card-resident-dev-loader` | Starts the resident child described by the header found in the payload slot. |

The two resident loaders are mutually exclusive; `card-resident` is the
internal feature they share.

## Entry flow

A resident loader image follows:

```text
main.rs: efi_main
  -> firmware/driver.rs: install and bind
  -> firmware/lifecycle.rs: register the firmware lifecycle events
  -> delivery/adapter.rs + delivery/child_image.rs: validate and start child
  -> delivery/adapter.rs: journal the load/arm record, retain an armed child
  -> firmware/lifecycle.rs: journal lifecycle events unless a retained child owns the journal
```

The resident loader has two mutually exclusive builds: `card-resident-loader`
compiles in the exact 128-byte payload header (`SVMVISOR_CARD_PE_HEADER`);
`card-resident-dev-loader` adopts the header found in the card's payload slot
after the same `Pin::parse_resident` policy (`execute_resident_dev`), for the
fast iteration loop in `firmware/card/README.md`. Both share the internal
`card-resident` feature; the child stays bound to the header's SHA-256.

## Loading a different child

The resident loaders start whatever image the payload slot holds, as long as it
meets the policy in `svmvisor_card_abi::envelope` (`Envelope::parse` for the
header, `parse_pe_kind` for the image; `delivery/child_image.rs` binds the two
with the SHA-256). Nothing in that policy is specific to the hypervisor. A
child must be:

- A PE32+ x86-64 image (machine `0x8664`, optional-header magic `0x20b`) with
  PE subsystem 12, EFI runtime driver: the firmware loads it as runtime
  services code and data. It has a 240-byte optional header with 16 data
  directories, keeps its relocations, uses 4096-byte section and 512-byte file
  alignment, has 1 to 16 sections and an image size of at most 16 MiB.
- Wrapped in the 128-byte `SVMBPE01` header (`Pin::parse_resident`): version 1,
  header and payload offset 128, the PE's byte length, the 1 MiB slot size,
  flags 4, the SHA-256 of every PE file byte, and the PE metadata (entry RVA,
  image, header and alignment sizes, section count) the loader compares with
  what it parses. `firmware/card/package-payload.py --resident` writes it;
  `svmvisor_card_abi::envelope` documents the layout field by field and owns
  its constants and parser, so a Rust packager can check its output with the
  code the loader runs. The loader test
  `optional_python_actual_slot_matches_rust_parser` does that for the script:
  set `SVMVISOR_CARD_PE_TEST_SLOT` to a `payload-slot.bin` it wrote.
- At most 1 MiB minus the header (`SLOT_BYTES - HEADER_BYTES`), at least 512
  bytes.

The loader hands the child a 128-byte
`svmvisor_card_abi::boot_options::ResidentBootOptions` as its LoadOptions:
the journal's BAR0 page, the boot ID, and in version 3 the admitted
`TerminalEndpoint`. The child answers in the same record before it returns:

- `rust_entered = 1` as soon as it runs, so the loader can tell a firmware
  refusal to start the image from the child's own error;
- `armed = 1` with `failure = 0` and empty `preparation_*` words when it
  returns `EFI_SUCCESS`. The loader then retains the image, the options and
  the controller until reset. Success without that acknowledgement, or with
  changed input fields, is reported as a protocol error;
- on refusal an error status, `failure`, and optionally the `preparation_*`
  words saying where it stopped. The firmware unloads a driver that returns an
  error.

A child that wants to write the card journal uses
`svmvisor_card_abi::journal` with its own `JournalIo`.
