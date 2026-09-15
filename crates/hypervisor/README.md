# Hypervisor core

`svmvisor-hypervisor` is the `no_std`, UEFI-independent core. It owns CPU and
memory models, validation, AMD SVM structures, and bounded VM-exit handling.
`crates/dxe` owns firmware allocation, protocols, lifecycle events, and the
current native transition adapter. Keep firmware calls out of this crate.

The [post-EBS activation check](../../docs/native-startup-activation.md) now
executes separately retained resident runtimes on native BSP/AP continuations
under the corrected backend. The native ICR owner forwards ordinary x2APIC
writes and refuses unowned startup/reset without changing stopped state.
Windows loader integration remains open.
The [guest restart profile](../../docs/native-guest-startup.md) adds target-owned
Running/AP INIT and SIPI with a retained shared mailbox and legacy unpaged
instruction handling. The [INIT/#SX path](../../docs/native-init-sx-wakeup.md)
replaces its earlier IRQ reservation. [Native xAPIC](../../docs/native-xapic-startup.md)
now shares that owner and follows the physical APIC mode; Windows execution
and actual-platform admission remain separate.

The [guest APIC contract](../../docs/native-guest-apic-model.md) now separates
guest destination IDs from physical AMD extended controls. Native boot exposes
conventional eight-bit xAPIC and enabled x2APIC, hides the extended register
interface, and keeps physical eight-bit routing stable across guest INIT.
Exact-image tests and physical boot results remain separate evidence.

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
| `arch::x86_64` | CPU capability evidence, register and descriptor formats, extended state | `capabilities`, `registers`, `descriptors`, `xstate` |
| `boot` | Supplied firmware observations, admission policies, and handoff contracts | `preflight`, `probe`, `handoff`, `memory`, `descriptors`, `xstate` |
| `guest` | Constrained guest register state and page ownership | `state`, `pages` |
| `host` | Host descriptor and paging contracts | `descriptors`, `paging` |
| `memory` | Validated addresses, reserved layout, nested page tables | `address`, `layout`, `npt` |
| `svm` | VMCB and permission maps, exit classification, CPUID and hypercall emulation | `vmcb`, `permission_maps`, `exit`, `dispatch`, `emulation` |

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

The emulator now delivers #UD/#GP/#PF to guest handlers and continues through
IRETQ, with nested-delivery refusal and checked stacks. See the
[guest exception continuation report](../../docs/guest-exception-continuation.md) for execution evidence and remaining OS-boot gaps.

The emulator resident handoff now retains the final EBS map in owned runtime
storage and checks the proposed guest reservation and complete NPT backing
allowlist. See [resident ownership handoff](../../docs/resident-ownership-handoff.md) for
validated behavior and the remaining Windows/native integration boundary.

The `guest::continuation` record and window-aware `guest::pages` builder now
support the harness's captured integer loader continuation. Shared VMCB setup
and NPT auditing keep the harness paths consistent. See
[loader continuation](../../docs/loader-continuation.md) for ownership and evidence.

`arch::x86_64::clock` owns the bounded clock capability/restoration plan.
Its opt-in CPUID policy uses the existing dispatcher; the default diagnostic
policy is unchanged. See [clock ownership](../../docs/clock-ownership.md) for
executed AUX checks and the unexecuted ratio-switching boundary.

`svm::events::PendingExternalInterrupt` and the existing VMCB owner manage one
pending virtual maskable interrupt. Hardware readiness and guest handler proof
are exercised separately; see [virtual interrupt delivery](../../docs/external-interrupt-delivery.md).

The [bounded local APIC controller](../../docs/local-apic-controller.md) adds internal IRR/ISR/PPR,
EOI and a deterministic one-shot timer with real guest delivery fixtures.
Guest APIC register access and physical timer scheduling remain separate work.

The [partial x2APIC register fixture](../../docs/x2apic-register-fixture.md) now exercises actual guest
MSR accesses for TPR/PPR/EOI and interrupt bitmaps over that controller.
The [checked MSR faults and bounded APIC modes](../../docs/apic-modes-msr-faults.md) now add
guest #GP retries and Disabled/xAPIC/x2APIC transitions. Full APIC admission remains pending.

[Guest CR8 write synchronization](../../docs/cr8-synchronization.md) adds owned TPR updates
and records the newly observed QEMU invalid-operand fault gap.

The [QEMU CR8 fault correction](../../docs/qemu-cr8-fault-correction.md) now passes
both intercepted and direct-write fault/IRETQ suites on a separately pinned backend.

The [bounded xAPIC MMIO fixture](../../docs/xapic-mmio-fixture.md) adds fixed BSP
identity and real NPF completion over the same controller used by MSRs and CR8.
The [SVR and timer fixture](../../docs/apic-svr-timer-fixture.md) now adds software
enable, a timer-only LVT/version and divided supplied-source scheduling. Generic
APIC capability and full platform admission remain withheld.

`svm::apic_scheduler::ScheduledApic` adds an owned clock-service boundary and
checked HLT parking/wakeup over the existing APIC handlers. The
[clock-driven timer and HLT fixture](../../docs/apic-clock-hlt-fixture.md)
records actual stopped-host TSC polling and guest EOI/IRETQ continuation;
physical calibration remains pending.

The [running-guest preemption fixture](../../docs/apic-running-preemption-fixture.md) now uses an owned emulator
LAPIC timer to interrupt an integer loop and continue through guest EOI/IRETQ.
It records explicit post-EBS timer takeover and source acknowledgement;
physical platform scheduling and Windows compatibility remain unestablished.

The [timer/fault overlap fixture](../../docs/event-overlap-fixture.md) now preserves pending timer interrupts across guest #UD/#GP/#PF handlers and verifies IF/TPR blocking, STI shadow, EOI and IRETQ on both APIC buses. Windows boot and physical compatibility remain unestablished.

`Vmcb::resolve_exception_delivery_after_exit` adds opt-in bounded exception
combination and terminal shutdown while strict ordinary reflection is retained.
The [interrupted delivery report](../../docs/interrupted-delivery-fixture.md)
records its exact scope, actual nested IRQ-handler faults, double-fault and
shutdown evidence, and unsupported interrupted external delivery.

