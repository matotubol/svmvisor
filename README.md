# svmvisor

## Current checkpoint — 2026-09-16

See [the current handoff](docs/handoff-2026-09-16.md) first. The latest physical
capture reached Windows kernel execution across all 24 CPUs before stopping;
it did not retain the first-fault reason. A diagnostic image with improved
fault capture was subsequently flashed and read back successfully.

The uncommitted working tree replaces guest APIC passthrough with an exclusive
guest x2APIC/x2AVIC interface and is code-complete for the host IRQ bridge
profile. The resident host captures each physical interrupt and publishes it
into the owning CPU's AVIC backing page. Windows keeps using the physical IOMMU
as on bare metal; direct IOMMU interrupt posting is deferred. The x2APIC
register model, IPI fan-out with doorbells, level-EOI handling and guest INIT
are implemented, and the IOMMU discovery and source-route code was removed.

Host tests and linked-image audits pass. Nothing has been flashed or run
natively, and the BIOS x2APIC change has not been measured. Independent review
is in progress. See [the completion record](docs/x2avic-completion-2026-09-16.md).
Windows guest boot remains unresolved. Earlier milestones below are historical.


An AMD SVM hypervisor project delivered through a UEFI DXE driver in a PCI
option ROM, for controlled analysis on systems you own or are authorized to use.

The physical machine has completed the bounded multi-exit guest: 32 CPUID/query
rounds and a final STOP, with **65 entries/exits and 64 resumptions** in one
transition call. Its snapshot matched both image IDs and reported zero refusal,
complete restoration and cleanup, and passing canary checks. The detailed
multi-exit contract report was never imported into this repository.
The guest currently returns to firmware; running Windows inside a resident
hypervisor remains the next major objective.

The immediate milestone is Windows booting under the native DXE hypervisor on
this machine. The [native loader handoff](docs/native-loader-handoff.md) now has
an explicit boot profile that activates after the original ExitBootServices
returns success, with owned AP bootstrap roots and retained waiting code.
Disposable emulator checks exercise failed EBS retry, replacement/reclamation of
loader-owned paging memory, nonidentity runtime mapping with old aliases removed,
and repeated AP INIT/SIPI. Physical Windows boot remains untested.

x2APIC is the only supported interrupt-controller interface. The historical
[native xAPIC startup report](docs/native-xapic-startup.md) and
[INIT/#SX wakeup path](docs/native-init-sx-wakeup.md) describe the retired
xAPIC-era routing.
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

The [OS boot readiness contract](docs/os-boot-readiness.md),
[two-CPU UEFI ownership](docs/uefi-smp-ownership.md),
[AMD CPU model](docs/amd-cpu-model.md) and
[shared I/O boundary](docs/io-intercept-boundary.md) reports record milestones
of the retired synthetic emulator harness (see below).

## Source map

```text
crates/
  dxe/                 UEFI delivery, native admission/resources, returning guest
  hypervisor/          UEFI-independent CPU, memory, SVM and handoff primitives
  memory-attributes/   Memory Attribute Protocol implementation
firmware/squirrel/     Completion-only card endpoint, packaging and snapshots
tools/                 Host checks, audits, packaging and disposable QEMU fixtures
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
| Native transition test fixtures | `crates/dxe/src/fixtures/` |
| CPU representations, MSR/local APIC registers, physical x2APIC access and extended state | `crates/hypervisor/src/arch/x86_64/` |
| Guest/host state, paging and memory ownership | `crates/hypervisor/src/guest/`, `host/`, `memory/` |
| Resident runtime exits, stop reasons and per-CPU layout | `crates/hypervisor/src/host/resident/`, `host/resident.rs` |
| VMCB, permission maps, exit dispatch and emulation | `crates/hypervisor/src/svm/` |
| Guest x2APIC/x2AVIC: registers, IPIs, host IRQ bridge, INIT/SIPI | `crates/hypervisor/src/svm/x2avic/` |
| Resident payload link and build/audit script | `tools/native-resident/` |
| Supplied firmware evidence and handoff contracts | `crates/hypervisor/src/boot/` |

Rust paths follow the source directories, such as
`svmvisor_hypervisor::svm::dispatch` and `svmvisor_dxe::native::admission::cpu`.
Neither library exports root-level compatibility aliases.

## Build and test

From the workspace root on the current Windows development host:

```powershell
cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc
cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime --lib
cargo test --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime-test --lib
cargo check --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime
cargo check --locked -p svmvisor-hypervisor --target x86_64-pc-windows-msvc --features resident-runtime-test
cargo check --locked -p svmvisor-hypervisor --target x86_64-unknown-uefi
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-returning
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features card-returning-loader
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features memory-attribute-probe
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-preflight
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-resident-boot
cargo test --locked -p svmvisor-dxe --target x86_64-pc-windows-msvc --features native-resident-low-runtime
cargo test --locked -p svmvisor-memory-attributes
cargo build-dxe
python -m unittest discover -s tools/native-resident -p "test_*.py"
python -m unittest discover -s tools/native-stack-audit -p "test_*.py"
python -m unittest discover -s firmware/squirrel -p "test_*.py"
```

The `resident-runtime` and `resident-runtime-test` host tests are `--lib`
only: the integration tests would link the runtime without its payload linker
script and assembly.

The resident image is built and audited without programming hardware:

```powershell
python tools/native-resident/build.py --output work/<fresh> --boot --low-runtime
```

The script snapshots the sources, links the payload and packages its
relocations. It then runs the undefined-symbol, debug-register, host-fault and
FP/SIMD/xstate audits, builds the DXE driver around the payload and audits the
AP bootstrap assembly. Use a new output directory; the script refuses an
existing one. See the [DXE guide](crates/dxe/README.md). The
[completion record](docs/x2avic-completion-2026-09-16.md) lists the results of
this command matrix for the current tree.

DXE features select separate firmware images; `--all-features` is intentionally
invalid. The default build is the lifecycle driver. Native returning, child
delivery and transition fixtures have distinct build requirements described in the
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

Product direction: a hypervisor foundation for malware analysis. See
[analysis architecture direction](docs/malware-analysis-direction.md) for the
requirements this introduces now and the later observation pipeline.

## Retired synthetic emulator harness

The QEMU-based synthetic SVM harness, its emulator-only APIC, IPI and scheduler
models (`svm/xapic.rs`, `x2apic.rs`, `local_apic.rs`, `apic_scheduler.rs`), the
DXE `emulator-*` features and the QEMU resident fixture were retired on
2026-09-16. The native runtime requires x2AVIC, which QEMU TCG does not
emulate, so those fixtures could no longer exercise the production path. Their
per-fixture reports were removed (see git history); handoff documents and the
remaining dated reports stay as historical evidence.
`tools/synthetic-harness/` now keeps only the shared QEMU download, relocation
packaging and the `firmware-handoff` crate.
