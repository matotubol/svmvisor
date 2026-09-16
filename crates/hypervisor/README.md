# Hypervisor core

## Current native implementation

The working tree replaces guest APIC passthrough with x2APIC/x2AVIC and owned
IOMMU interrupt routing. This rewrite is incomplete; its host checks do not
establish native boot or device-delivery correctness. Start with
[the handoff](../../docs/handoff-2026-09-16.md) and
[the rewrite checkpoint](../../docs/x2avic-rewrite-design-2026-09-16.md).
The older native APIC/startup reports below describe historical implementations,
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
the current native probe returns to firmware. See the
[multi-exit contract](../../docs/native-multi-exit-contract.md) for its scope.

The [native SYSCFG owner](../../docs/native-syscfg-writes.md) supports the reviewed Windows fixed-MTRR save/restore control sequence and preserves detailed refusal operands.

## Code navigation

| Namespace | Responsibility | Starting points |
| --- | --- | --- |
| `arch::x86_64` | CPU capability evidence, MSR indices and fields, register and descriptor formats, extended state | `capabilities`, `msr`, `registers`, `descriptors`, `xstate` |
| `boot` | Supplied firmware observations, admission policies, and handoff contracts | `preflight`, `probe`, `handoff`, `memory`, `descriptors`, `xstate` |
| `guest` | Constrained guest register state and page ownership | `state`, `pages` |
| `host` | Host descriptor and paging contracts | `descriptors`, `paging` |
| `memory` | Validated addresses, reserved layout, nested page tables, MTRR decoding | `address`, `layout`, `npt`, `mtrrs` |
| `svm` | VMCB and permission maps, exit classification, CPUID and hypercall emulation, x2AVIC | `vmcb`, `permission_maps`, `exit`, `dispatch`, `emulation`, `x2avic` |
| `sync` | Non-blocking lock for state shared between CPUs | `TryLock` |

Each directory has one `mod.rs` registry; implementation files are compiled
once. Prefer grouped paths in new code, for example
`svmvisor_hypervisor::svm::dispatch`. Existing root paths such as
`svmvisor_hypervisor::dispatch` and `svmvisor_hypervisor::guest_state` remain
aliases to the same modules and types so consumers can migrate separately.

The physical assembly loop and firmware adapter currently remain under
`crates/dxe`. The pure Rust dispatcher models stopped-guest state changes; its
success alone is not evidence of native entry or safe firmware return. Preserve
that distinction when adding a resident runtime.

## Working on the core

Run from the workspace root on the current Windows development host:

```powershell
cargo test --package svmvisor-hypervisor --target x86_64-pc-windows-msvc
cargo check --package svmvisor-hypervisor --target x86_64-unknown-uefi
```

These commands exercise the existing host tests and check the firmware target
without programming hardware. Tests remain in `tests/`; their existing names
identify the implementation contract they cover. Firmware integration and
physical activation still require the DXE validation workflow.

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
