# svmvisor

## Current checkpoint — 2026-09-16

See [the current handoff](docs/handoff-2026-09-16.md) first. The latest image
activates all 24 CPUs, then stops on a cache-owner MSR write refusal.
Windows guest boot remains unresolved. Earlier milestone descriptions below
are historical.


An AMD SVM hypervisor project delivered through a UEFI DXE driver in a PCI
option ROM, for controlled analysis on systems you own or are authorized to use.

The physical machine has completed the bounded multi-exit guest: 32 CPUID/query
rounds and a final STOP, with **65 entries/exits and 64 resumptions** in one
transition call. Its snapshot matched both image IDs and reported zero refusal,
complete restoration and cleanup, and passing canary checks. See the
[multi-exit contract](docs/native-multi-exit-contract.md) for the exact result.
The guest currently returns to firmware; running Windows inside a resident
hypervisor remains the next major objective.

The immediate milestone is Windows booting under the native DXE hypervisor on
this machine. The [native loader handoff](docs/native-loader-handoff.md) now has
an explicit boot profile that activates after the original ExitBootServices
returns success, with owned AP bootstrap roots and retained waiting code.
Disposable emulator checks exercise failed EBS retry, replacement/reclamation of
loader-owned paging memory, nonidentity runtime mapping with old aliases removed,
and repeated AP INIT/SIPI. Physical Windows boot remains untested.

The [native xAPIC startup report](docs/native-xapic-startup.md) records native
xAPIC/x2APIC routing and the [INIT/#SX wakeup path](docs/native-init-sx-wakeup.md).
The [Ryzen encryption admission](docs/native-ryzen-encryption-admission.md)
distinguishes advertised capabilities from enabled unsupported encryption.
The [broadcast bootstrap](docs/native-broadcast-startup.md) handles the Ryzen
reset-width interval and serializes runtime destination changes with startup
publication. Initial BSP routing and unique per-command matching remain gates.
[Loader control admission](docs/native-loader-control-admission.md) now includes
FSGSBASE and PCID. The [resident card handoff](docs/native-resident-card-delivery.md)
uses a separate retained-image contract and explicit armed acknowledgement.
Physical Windows boot and Hyper-V/VBS coexistence remain unproven/unsupported;
no Windows protections have been changed.

The [OS boot readiness contract](docs/os-boot-readiness.md) records the current
two-CPU emulator checkpoint, source-backed exit coverage, and ordered gates
for integrating UEFI continuation with the concurrent runtime.

The [two-CPU UEFI ownership fixture](docs/uefi-smp-ownership.md) now joins
firmware CPU admission and resident AP startup with the concurrent timer,
IPI and HLT workload under one pinned emulator profile.

The [AMD CPU model](docs/amd-cpu-model.md) adds native-facing CPUID responses
and per-guest XCR0 switching, with a dedicated two-CPU emulator fixture.

The [shared I/O boundary](docs/io-intercept-boundary.md) enables IOIO protection
and proves stopped IN/OUT refusal with a live disposable endpoint witness.

## Source map

```text
crates/
  dxe/                 UEFI delivery, native admission/resources, returning guest
  hypervisor/          UEFI-independent CPU, memory, SVM and handoff primitives
  memory-attributes/   Memory Attribute Protocol implementation
firmware/squirrel/     Completion-only card endpoint, packaging and snapshots
tools/                 Host checks, emulator harnesses and ROM packaging
docs/                  Architecture contracts and retained experiment reports
```

Start with the [DXE guide](crates/dxe/README.md) or the
[hypervisor guide](crates/hypervisor/README.md). Both map directories to
responsibilities and show the relevant feature profiles and commands.

| Area | Where to work |
| --- | --- |
| UEFI driver binding, PCI access and lifecycle | `crates/dxe/src/firmware/` |
| Parent image delivery and result reporting | `crates/dxe/src/delivery/`, `diagnostics/` |
| Native admission, allocations and returning transition | `crates/dxe/src/native/` |
| Emulator-only DXE execution paths | `crates/dxe/src/fixtures/` |
| CPU representations and extended state | `crates/hypervisor/src/arch/x86_64/` |
| Guest/host state, paging and memory ownership | `crates/hypervisor/src/guest/`, `host/`, `memory/` |
| VMCB, exit dispatch and emulation | `crates/hypervisor/src/svm/` |
| Supplied firmware evidence and handoff contracts | `crates/hypervisor/src/boot/` |

Use grouped Rust paths in new code, such as `svmvisor_hypervisor::svm::dispatch`
and `svmvisor_dxe::native::admission`. Existing root module paths remain aliases
to the same implementations so callers can migrate without duplicate code.

## Build and test

From the workspace root on the current Windows development host:

```powershell
cargo test --locked -p svmvisor-hypervisor
cargo test --locked -p svmvisor-dxe --features native-returning
cargo test --locked -p svmvisor-dxe --features card-returning-loader
cargo test --locked -p svmvisor-dxe --features memory-attribute-probe
cargo build-dxe
```

DXE features select separate firmware images; `--all-features` is intentionally
invalid. The default build is the lifecycle driver. Native returning, child
delivery and emulator fixtures have distinct build requirements described in the
[DXE guide](crates/dxe/README.md).

The native image also requires its linked stack/control-flow audit:

```powershell
python tools/native-stack-audit/run.py --output work/native-stack-audit-new
```

Use a fresh output directory. This builds and audits an image without programming
hardware. Card packaging, routed FPGA checks and full flash readback are separate
steps documented under [firmware/squirrel](firmware/squirrel/README.md).
Source reorganization does not make a new binary inherit an older image's
physical test result.

## Development boundaries

DXE owns UEFI allocation, protocols and lifecycle. The hypervisor crate stays
`no_std` and independent of UEFI. Keep firmware calls, allocation, floating point
and unbounded work out of persistent host and VM-exit paths. Privileged operations
need explicit safety contracts and validated address/state boundaries; see
[CONTRIBUTING.md](CONTRIBUTING.md).

The card endpoint is completion-only. Historical requester/DMA-capable images
are not the current delivery substrate.

The obsolete M0b USB collector and its build/media tooling have been removed.
Recorded hardware measurements and frozen evidence bundles remain historical
records. Older experiment reports preserve the paths and conclusions from their
original checkpoints; use the crate guides for current code navigation.

The emulator now delivers #UD/#GP/#PF to guest handlers and continues through
IRETQ, with nested-delivery refusal and checked stacks. See the
[guest exception continuation report](docs/guest-exception-continuation.md) for execution evidence and remaining OS-boot gaps.

Product direction: a hypervisor foundation for malware analysis. See
[analysis architecture direction](docs/malware-analysis-direction.md) for the
requirements this introduces now and the later observation pipeline.

The emulator resident handoff now retains the final EBS map in owned runtime
storage and checks the proposed guest reservation and complete NPT backing
allowlist. See [resident ownership handoff](docs/resident-ownership-handoff.md) for
validated behavior and the remaining Windows/native integration boundary.

The emulator also resumes a captured benign integer loader continuation and
checks guest allocation/refusal against an owned map. See
[loader continuation](docs/loader-continuation.md) for the contract, shared
architecture, and remaining Windows integration work.

[Timestamp conformance](docs/timing-contract.md) records the bounded emulator
RDTSC/RDTSCP checks, the verified QEMU interception gap, and remaining clock
ownership and physical measurement work.

[Clock ownership](docs/clock-ownership.md) adds capability-gated AUX/ratio
switching, host restoration checks and guest MSR refusal to that harness.

[Virtual interrupt delivery](docs/external-interrupt-delivery.md) adds bounded
pending-event ownership and guest handler/IRETQ checks, with QEMU gaps retained
separately from physical interrupt and APIC work.

The [QEMU SVM backend correction report](docs/qemu-svm-corrections.md) records
verified CR8, interrupt-priority/delivery and RDTSCP intercept corrections in a
separate test backend, with stock failure evidence preserved.

The [bounded local APIC controller](docs/local-apic-controller.md) adds internal IRR/ISR/PPR,
EOI and a deterministic one-shot timer with real guest delivery fixtures.
Guest APIC register access and physical timer scheduling remain separate work.

The [partial x2APIC register fixture](docs/x2apic-register-fixture.md) now exercises actual guest
MSR accesses for TPR/PPR/EOI and interrupt bitmaps over that controller.
The [checked MSR faults and bounded APIC modes](docs/apic-modes-msr-faults.md) now add
guest #GP retries and Disabled/xAPIC/x2APIC transitions. Full APIC admission remains pending.

[Guest CR8 write synchronization](docs/cr8-synchronization.md) adds owned TPR updates
and records the newly observed QEMU invalid-operand fault gap.

The [QEMU CR8 fault correction](docs/qemu-cr8-fault-correction.md) now passes
both intercepted and direct-write fault/IRETQ suites on a separately pinned backend.

The [bounded xAPIC MMIO fixture](docs/xapic-mmio-fixture.md) adds fixed BSP identity
and real trapped MMIO access to the same priority/interrupt state used by MSRs
and CR8. The [SVR and timer fixture](docs/apic-svr-timer-fixture.md) adds software
enable, one functional timer LVT, truthful version and divided one-shot/periodic
countdowns using supplied ticks. Generic APIC capability and platform admission
remain pending.

The [clock-driven timer and HLT fixture](docs/apic-clock-hlt-fixture.md) now
samples the actual host TSC while stopped, wakes guest-programmed timers and
checks handler/EOI/IRETQ continuation. Its admitted clock ratio is synthetic;
physical timer calibration remains separate work.

The [running-guest preemption fixture](docs/apic-running-preemption-fixture.md) now uses an owned emulator
LAPIC timer to interrupt an integer loop and continue through guest EOI/IRETQ.
It records explicit post-EBS timer takeover and source acknowledgement;
physical platform scheduling and Windows compatibility remain unestablished.

The [timer/fault overlap fixture](docs/event-overlap-fixture.md) now preserves pending timer interrupts across guest #UD/#GP/#PF handlers and verifies IF/TPR blocking, STI shadow, EOI and IRETQ on both APIC buses. Windows boot and physical compatibility remain unestablished.

The [interrupted delivery fixture](docs/interrupted-delivery-fixture.md) adds
actual faults inside IRQ handlers with a second pending timer, bounded IDT
exception combinations, terminal double-fault handlers and explicit guest
shutdown. Its 35-profile emulator matrix passes.

The [two-CPU startup/IPI fixture](docs/multicore-ipi-fixture.md) adds guest
INIT/SIPI, an executed real16/protected32/long64 transition, and bidirectional
xAPIC/x2APIC interrupts with EOI/IRETQ. Two guest CPUs retain separate execution
state while cooperatively sharing one host CPU. Physical concurrent SMP and
general OS topology admission remain separate milestones.

The [concurrent SMP fixture](docs/concurrent-smp-fixture.md) runs two guest CPUs
on two emulated host CPUs with private execution state, bidirectional running-
target interrupts and a controlled request-after-drain race. This is a bounded
multithreaded TCG milestone; physical SMP and Windows compatibility remain
unestablished.

The [concurrent guest startup and HLT fixture](docs/concurrent-startup-fixture.md) integrates target-owned INIT/SIPI, executed real-mode startup and bidirectional HLT wakeups on the existing two-host-CPU emulator. Physical host sleep and Windows compatibility remain unestablished.

The [concurrent timer and host idle fixture](docs/concurrent-idle-fixture.md) adds timer preemption and actual host HLT wakeups with separate timer/IPI evidence on both emulator CPUs. General OS-boot admission and physical compatibility remain separate work.

