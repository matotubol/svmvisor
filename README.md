## Source map

```text
crates/
  dxe/                 UEFI delivery, native admission/resources, returning guest
  hypervisor/          UEFI-independent CPU, memory, SVM and handoff primitives
  memory-attributes/   Memory Attribute Protocol implementation
  resident-payload/    Resident payload staticlib and its linker script (standalone workspace)
  firmware-handoff/    Checked relocation loader and handoff layout (standalone workspace)
  rompack/             PCI option ROM packager
  xtask/               `cargo xtask`: resident payload build, packaging and audits
firmware/card/         Completion-only card endpoint, packaging and snapshots
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
| Resident payload crate and link script | `crates/resident-payload/` |
| Resident build, relocation packaging and linked-code audits | `crates/xtask/` (`cargo xtask resident`) |
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
cargo test --locked -p xtask
cargo test --locked -p svmvisor-rompack
cargo test --locked --manifest-path crates/firmware-handoff/Cargo.toml
python -m unittest discover -s firmware/card -p "test_*.py"
```


DXE features select separate firmware images; `--all-features` is intentionally
invalid. The default build is the lifecycle driver. Native returning, child
delivery and transition fixtures have distinct build requirements described in the
[DXE guide](crates/dxe/README.md).


The audited production resident image is built by the [xtask](crates/xtask/README.md):

```powershell
cargo xtask resident --output target/native-resident/<fresh-name>
```

Use a fresh output directory. This builds and audits an image without programming
hardware. Card packaging, routed FPGA checks and full flash readback are separate
steps documented under [firmware/card](firmware/card/README.md).
Source reorganization does not make a new binary inherit an older image's
physical test result.

## Development boundaries

DXE owns UEFI allocation, protocols and lifecycle. The hypervisor crate stays
`no_std` and independent of UEFI. Keep firmware calls, allocation, floating point
and unbounded work out of persistent host and VM-exit paths. Privileged operations
need explicit safety contracts and validated address/state boundaries; see
