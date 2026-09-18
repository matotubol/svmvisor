# svmvisor

A bare-metal AMD SVM hypervisor written in Rust and assembly. It loads from a
PCIe card's option ROM as a UEFI DXE driver, takes over every core before the
operating system starts, and then runs that operating system as its only guest
on the real hardware.

**Status:** boots Windows 11 to the desktop on a Zen 5 desktop (Family 1Ah
Model 44h, 24 logical CPUs) and stays up under load. This is a personal
learning project for one machine, not a product.

## How it works

- The guest owns the hardware: real CPU features, memory, PCI devices and
  interrupts. Nested paging is an identity map; nothing is emulated except the
  local APIC interface.
- Interrupts use x2AVIC with virtual NMI. Physical interrupts are captured by a
  host IRQ bridge and re-presented to the guest's virtual APIC.
- The guest's INIT/SIPI sequence starts the other cores as guest CPUs, so
  Windows brings up all 24 processors itself.
- SVM is hidden from the guest (CPUID and `VM_CR`), so Windows sees a machine
  with virtualization turned off.
- The PCIe card doubles as a debug port: each CPU publishes progress and stop
  records to it, and a second PC reads them over USB/JTAG even when the target
  is hung.

## Layout

```text
crates/
  hypervisor/          no_std core: VMCB, exits, x2AVIC, nested paging, resident runtime
  dxe/                 UEFI DXE driver: admission checks, allocation, activation
  resident-payload/    resident image staticlib and linker script
  firmware-handoff/    relocation loader and handoff layout
  memory-attributes/   UEFI Memory Attribute Protocol
  rompack/             PCI option ROM packager
  xtask/               cargo xtask: build, audit, package, flash, snapshot
firmware/card/         FPGA card RTL, flashing scripts, snapshot decoder
docs/                  AMD/UEFI/ACPI manuals, indexed for lookup (see docs/README.md)
```

Most of the interesting code is in `crates/hypervisor/src/svm/` (VMCB, MSR and
I/O permission maps, x2AVIC) and `crates/hypervisor/src/host/resident/` (the
VM-exit dispatcher and the card diagnostics).

## Build, flash, debug

```powershell
cargo test --workspace            # host-side unit tests
cargo xtask card-dev              # build + audit + package the payload (no hardware)
cargo xtask card-dev --flash      # same, then program the card's payload slot
cargo xtask card-snapshot         # read the per-CPU records back from the card
```

Power-cycle the target after flashing. If it stops or hangs, take a snapshot:
every hypervisor stop carries a tag and its operands, decoded by
`firmware/card/read_snapshot.py`.

More detail: [hypervisor](crates/hypervisor/README.md),
[DXE driver](crates/dxe/README.md), [xtask](crates/xtask/README.md),
[card](firmware/card/README.md).
