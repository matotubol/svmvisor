Current loader builds consume the `.reloc` executable package through
`SVMVISOR_PAYLOAD`, with checked runtime relocation and SVMUEFI2 handoff. See
[Relocatable emulator host](../../../docs/relocatable-emulator-host.md) for the
current format, allocation policy and commands. The flat-payload, fixed-address
and older header descriptions below are historical prototype documentation.

This standalone UEFI application is exclusively for the disposable QEMU/OVMF
harness. It does not change or install the physical DXE driver.

The default build is a direct application prototype. Building with
`--features driver` instead selects PE subsystem 11 (EFI boot-services driver)
and emits `uefi-driver-entry` from that driver's entry point. Use separate target
directories so the two artifacts cannot overwrite each other; both bins are
named `BOOTX64.efi` by Cargo.

For the driver path, first build this crate with the payload environment below
and `--features driver`. Then build `launcher/Cargo.toml` with `SVMVISOR_DRIVER`
set to the absolute driver image path. The launcher embeds and verifies the PE
subsystem of that image, calls UEFI `LoadImage` from its buffer and `StartImage`,
and emits `uefi-launcher-entry` and `uefi-driver-loaded`. Stage the launcher's
`BOOTX64.efi` as the emulator boot application. The driver then follows the same
payload handoff described below without returning to the launcher. This proves
firmware image loading and entry, not production Driver Binding integration.

Build with target `x86_64-unknown-uefi`, release profile, and both environment
variables set:

- `SVMVISOR_PAYLOAD`: absolute path to the linked flat emulator payload.
- `SVMVISOR_ENTRY`: decimal entry byte offset relative to payload base `0x100000`.
  Subtract `0x100000` from the absolute entry symbol reported by `llvm-nm`.

The output `BOOTX64.efi` is staged by the parent runner at `EFI/BOOT/BOOTX64.EFI`
in a disposable emulator boot volume. Never install this application on the
physical machine's EFI partition.

The loader reserves exactly 1 MiB at `0x100000` as LoaderCode, zeroes it, copies
the embedded payload and writes a 24-byte little-endian handoff at `0x1ff000`:
`SVMUEFI1`, reserved base `0x100000`, reserved extent `0x100000`. Payload bytes
must fit below this page; the parent must also verify the linked BSS end does
not overlap it. The payload receives the header pointer in RDI using SysV64 ABI.

The cached uefi 0.39.0 `boot::exit_boot_services(None)` implementation obtains
the final memory map and retries on an outdated key. The loader retains the
returned map, disables interrupts, emits `uefi-boot-services-exited`, and enters
the payload without further firmware calls. The payload must install its own
stack and address space before accessing assumptions of its bare-metal image.
LoaderCode is requested for the initial transfer under the firmware address
space; the payload subsequently installs its own restricted mappings.
