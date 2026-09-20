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

`svmvisor-hypervisor` is the `no_std`, UEFI-independent core. It owns CPU and
memory models, validation, AMD SVM structures, and bounded VM-exit handling.
`crates/launcher` owns firmware allocation, protocols, lifecycle events, and the
current native transition adapter. Keep firmware calls out of this crate.

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
`crates/launcher`. The pure Rust dispatcher models stopped-guest state changes; its
success alone is not evidence of native entry or safe firmware return. Preserve
that distinction when adding a resident runtime.
