# Hypervisor core

## Current native implementation

The working tree replaces guest APIC passthrough with an exclusive
x2APIC/x2AVIC guest interface. Device interrupts reach the guest through the
host IRQ bridge: `host::resident` captures physical vectors, and
`svm::x2avic::irq` owns their physical EOI ordering. Windows keeps using the
physical IOMMU natively; direct IOMMU interrupt posting is a later batch.

The register model, IPI fan-out, level-EOI handling and guest INIT are
implemented. The tree is unflashed, and its host checks do not establish native
boot or device-delivery correctness.

Start with [the handoff](../../docs/handoff-2026-09-16.md) and
[the completion record](../../docs/x2avic-completion-2026-09-16.md), which
lists the interception profile, decisions, stop codes and validation. The
older native APIC/startup reports below describe historical implementations,
not an alternative production backend.

`svmvisor-hypervisor` is the `no_std`, UEFI-independent core. It owns CPU and
memory models, validation, AMD SVM structures, and bounded VM-exit handling.
`crates/dxe` owns firmware allocation, protocols, lifecycle events, and the
current native transition adapter. Keep firmware calls out of this crate.

x2APIC is the only supported interrupt-controller interface, for both the
host bootstrap and the guest. The
[post-EBS activation check](../../docs/native-startup-activation.md),
[guest restart profile](../../docs/native-guest-startup.md),
[INIT/#SX path](../../docs/native-init-sx-wakeup.md),
[native xAPIC](../../docs/native-xapic-startup.md) and
[guest APIC contract](../../docs/native-guest-apic-model.md) reports describe
earlier passthrough and xAPIC-era implementations.

The physical returning probe has completed 65 guest entries and exits: 32 CPUID
and QUERY rounds, followed by STOP, with firmware restoration, cleanup, and
canary checks complete. That result proves the tested bounded path. A resident
hypervisor that keeps Windows running as its guest is still to be implemented;
the current native probe returns to firmware. The detailed multi-exit contract
report was never imported into this repository.

The [native SYSCFG owner](../../docs/native-syscfg-writes.md) supports the reviewed Windows fixed-MTRR save/restore control sequence and preserves detailed refusal operands.

## Code navigation

| Namespace | Responsibility | Starting points |
| --- | --- | --- |
| `arch::x86_64` | CPU capability evidence, MSR indices and fields, local APIC register space, physical x2APIC access and the AVIC doorbell, register and descriptor formats, extended state | `apic`, `capabilities`, `msr`, `registers`, `descriptors`, `xstate` |
| `boot` | Supplied firmware observations, admission policies, and handoff contracts | `preflight`, `probe`, `handoff`, `memory`, `descriptors`, `xstate` |
| `guest` | Constrained guest register state and page ownership | `state`, `pages` |
| `host` | Host descriptor and paging contracts; the resident runtime ABI and its bounded fetch/terminal owners | `descriptors`, `paging`, `resident` |
| `memory` | Validated addresses, reserved layout, nested page tables, MTRR decoding | `address`, `layout`, `npt`, `mtrrs` |
| `svm` | VMCB and permission maps, exit classification, CPUID and hypercall emulation, native cache/SYSCFG/PAUSE owners, x2AVIC | `vmcb`, `permission_maps`, `exit`, `dispatch`, `emulation`, `native_cache`, `x2avic` |
| `sync` | Non-blocking lock for state shared between CPUs | `TryLock` |

Each directory has one `mod.rs` registry; implementation files are compiled
once. Paths follow the directories, for example
`svmvisor_hypervisor::svm::dispatch` or `svmvisor_hypervisor::memory::address`;
the crate root exports no compatibility aliases.

`svm::x2avic` is the guest interrupt controller:

| Module | Owner |
| --- | --- |
| `x2avic` (`mod.rs`) | Capability admission, `NativeX2AvicProfile`, VMCB control constants, guest APIC version, derived logical x2APIC ID |
| `x2avic::registers` | MSRPM interception profile; `GuestX2Apic` (APIC_BASE shadow and every intercepted x2APIC access); LVT/timer mirroring; `CapturedInterface` (loader register state admitted at arm); guest-INIT LAPIC preparation and commit |
| `x2avic::ipi` | Admitted CPU inventory, AVIC_INCOMPLETE_IPI policy, fixed-IPI fan-out with doorbells |
| `x2avic::irq` | Host IRQ bridge: physical capture, held level sources, bounded physical EOI drain, software and level EOI, INIT retirement |
| `x2avic::backing` (re-exported `BackingPage`) | Per-vCPU hardware backing page: IRR/TMR publication, software EOI, INIT image |
| `x2avic::table` (re-exported `PhysicalIdTable`) | Shared physical-ID table and IsRunning |
| `x2avic::exit` (re-exported `AvicExit`) | AVIC_INCOMPLETE_IPI / AVIC_NOACCEL decoding |
| `x2avic::startup` | Software INIT/SIPI mailbox transport and target CPU-state commit |

`arch::x86_64::apic` holds the register offsets, the x2APIC MSR numbering and
the AVIC doorbell primitive. It also holds `PhysicalX2Apic` and `HostX2Apic`,
which are the owners' only path to physical registers. Non-APIC MSRs stay in
`arch::x86_64::msr`.

`host::resident::runtime` connects these owners to the arm, INTR, MSR,
401h/402h and guest-INIT paths. `host::resident::terminal` owns the typed stop
reasons (`X2AvicStop`, `IrqSite`) and the startup service stages
(`StartupStage`).

The physical assembly loop and firmware adapter currently remain under
`crates/dxe`. The pure Rust dispatcher models stopped-guest state changes; its
success alone is not evidence of native entry or safe firmware return. Preserve
that distinction when adding a resident runtime.

## Working on the core

Run from the workspace root on the current Windows development host:

```powershell
cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc
cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime --lib
cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime-test --lib
cargo check --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime
cargo check --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime-test
cargo check --locked -p svmvisor-hypervisor --target x86_64-unknown-uefi
```

These commands exercise the host tests and check the firmware target without
programming hardware. Tests remain in `tests/`; their names identify the
implementation contract they cover.

`resident-runtime` compiles `host::resident::runtime`, which references symbols
from `tools/native-resident/payload.ld` and the resident assembly. Its unit
tests link against inert test-only stand-ins, so run them with `--lib`; the
integration-test binaries cannot link that feature. Those unit tests include
the x2AVIC runtime glue tests (MSR outcome mapping, the incomplete-IPI and
AVIC exit plans, guest-INIT ordering and refusals) and the terminal
stop-encoding test.

`resident-runtime-test` adds the retired emulator fixture's port-E9h output,
including the IPI-drop and guest-INIT lines. No current build script selects
it. `cargo check` and the `--lib` tests cover it, with its terminal-return
tests excluded.

Only `tools/native-resident/build.py` builds and audits the linked payload.
Firmware integration and physical activation still require the DXE validation
workflow.

Follow [CONTRIBUTING.md](../../CONTRIBUTING.md): use validated address types,
bounded work, explicit state ownership, and precise safety contracts for
privileged code. Keep allocation, formatting, firmware calls, and floating point
out of persistent host and VM-exit paths. Add a module with its first real
implementation rather than reserving empty future layers.

`arch::x86_64::clock` owns the bounded clock capability/restoration plan.
`svm::events::PendingExternalInterrupt` and the VMCB owner manage one pending
virtual maskable interrupt, and `Vmcb::resolve_exception_delivery_after_exit`
adds opt-in bounded exception combination and terminal shutdown.
`guest::continuation` holds the captured loader continuation record.

The QEMU synthetic harness and its emulator-only APIC, IPI and scheduler
models were retired on 2026-09-16; see the root [README](../../README.md).
