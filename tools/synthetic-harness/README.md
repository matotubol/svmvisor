# Isolated SVM emulator smoke

The [shared I/O boundary](../../docs/io-intercept-boundary.md) is exercised by
`build.ps1 -IoIntercept` and `run-io.ps1 -QemuPath <pinned-backend>`. It executes
592 bounded cases per run. `-MissingEndpoint` requires failed endpoint liveness;
the separate `build.ps1 -IoInterceptBypass` / `run-io.ps1 -BypassBoundary` control
must detect the deliberately bypassed OUT side effect. Final evidence belongs
to `work/io-intercept/final-validation`; these are disposable emulator images.

The [bounded xAPIC MMIO fixture](../../docs/xapic-mmio-fixture.md) now exercises
real final-access NPFs through the shared APIC owner. It uses the unused guest
VA `0xc000`; existing guard and NPF probes remain unchanged. Current full
validation uses `work/interrupted-delivery/final-validation/run-regressions.ps1` with the
pinned CR8-corrected runtime and serialized builds; its output paths must be
fresh when repeating the run.

The [interrupted delivery fixture](../../docs/interrupted-delivery-fixture.md)
executes nested faults inside IRQ handlers with a second pending timer, bounded
IDT exception combinations, terminal #DF handlers and actual guest shutdown
intercepts. All 35 regression profiles pass; the exact supported subset and
frame/ownership/evidence checks are recorded in that report.

Previous [SVR/LVT/timer validation](../../docs/apic-svr-timer-fixture.md) is under
`work/apic-svr/final-validation`: 32 dual-bus sessions, real interrupt/IRETQ
checks and deterministic one-shot/periodic divided source ticks. The earlier
`work/apic-svr/validation` results are explicitly superseded. Repeated runs
must use fresh output directories and serialize shared builds.

The [clock-driven timer and HLT fixture](../../docs/apic-clock-hlt-fixture.md)
adds actual serialized host TSC polling, 64 completed sessions with 96
interrupt/EOI/IRETQ continuations, and 12 bounded no-wake cases. Its report
records frozen-source execution metrics and its completed regression matrix.
The separate source blob reuses the same owned guest CODE page after earlier
suites finish. The admitted clock ratio is synthetic, and polling occurs
only while stopped; it does not preempt a continuously running guest.

The [PCI ROM-to-guest path](../../docs/pci-rom-to-guest.md) now exercises OVMF ROM
discovery and actual Driver Binding callbacks, including Stop/rebind. Use
`run-uefi.ps1 -PciRom` for this mode; its launcher does not embed a driver.
`test-pci.ps1` runs all six cases, including BAR0 counter transactions and
intentional failed-Start cleanup.

Use `./tools/synthetic-harness/run-uefi.ps1 -ProductionDxe` to exercise the real
DXE crate's emulator feature. [DXE emulator verification](../../docs/dxe-emulator-handoff.md)
documents its scope and the missing-token and malformed-header tests.

The [OVMF driver handoff](../../docs/uefi-host-handoff.md) now boots the same
payload through an EFI boot-services driver and ExitBootServices. Run it with
`./tools/synthetic-harness/run-uefi.ps1`; add `-RejectHandoff` for the negative
header test. The flat Multiboot profiles below remain available separately.

Run all current checks with `./tools/synthetic-harness/test.ps1`.
The [repeated-session verification](../../docs/repeated-emulator-sessions.md)
records the latest seven-profile result, 32 sessions per profile, and both host
stack guard checks. `-HostGuard`, `-StackOverflow` and `-DfGuard` are additional
terminal profiles. The stack-overflow test verifies actual escalation onto IST1.

The normal `-RustCore` profile and terminal `-HostFault`, `-DoubleFault`, and
`-HostWriteProtect` profiles use restricted host mappings. See
[restricted-host verification](../../docs/restricted-host-emulator.md) for current
evidence and fault-profile commands. The original assembly smoke is also retained.

Earlier profile development: the original assembly backend smoke described below
and the Rust dispatcher integration enabled by `-RustCore`.

## Rust dispatcher integration

```powershell
rustup target add x86_64-unknown-none
./tools/synthetic-harness/build.ps1 -RustCore
./tools/synthetic-harness/run.ps1 -RustCore -QemuPath './target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe'
```

The separate static library in `rust/` links the production hypervisor crate.
`rust-bootstrap.S` supplies the SysV entry/exit bridge. This profile runs the
core's capability validation, VMCB setters, register frame, CPUID/hypercall
policies and transactional dispatcher. The guest checks the test vendor and
ABI response, then requests stop. Eleven untouched GPRs carry distinct sentinel
values checked after every exit; the guest checks the other three frame GPRs
through CPUID results. RAX, RIP and RSP belong to the VMCB.

QEMU system TCG 10.1 omits NRIPSAVE from its advertised SVM features (see
[CPU feature definitions](https://github.com/qemu/qemu/blob/v10.1.0/target/i386/cpu.c)).
The integration therefore fetches the exact instruction bytes within its owned
identity-mapped guest fixture and calls `handle_exit_with_instruction`. This
accepts only unprefixed CPUID or VMMCALL matching the observed exit, and checks
continuation arithmetic and canonicality. It does not claim nRIP support.
The original nRIP API retains its capability requirement.

The runner explicitly disables the emulated CPUID hypervisor advertisement
with `hypervisor=off`; this allows testing the production capability validator
inside a known emulator and does not establish absence of a real hypervisor.
Unencrypted RAM is a fixture assumption. This remains an integer-only test; its bootstrap host mappings are replaced by
image-only RX/RO/RW mappings before guest setup. It has
no SIMD state switch and harness-local raw initialization of VMCB fields absent
from the safe API. Guest mappings now use the checked Rust builders, and host
faults have terminal reporting. Recovery/resumption from a host fault is absent.

## Checked mappings and terminal host faults

The Rust profile installs guest page tables, NPT and guest descriptors through
`rust/src/memory.rs`. Code at guest VA/GPA `0x1000` has separate host backing
with NPT RX permissions. Stack, IST, GDT, TSS and page-table backing are RW/NX.
Host code, host tables and the VMCB have no guest mappings. Guest table backing
allows the accessed/dirty updates required by the hardware walker.

After CPUID/query/stop, three additional guest entries require:

- Access to guest VA `0x6000`: guest paging permits it, NPT omits backing;
  exit `0x400`, fault GPA `0x6000`.
- Execution at the stack page: NPT denies execution; exit `0x400`, fault GPA
  `0x8000`, instruction-fetch flag set.
- Access to guard VA `0x7000`: guest paging omits it; intercepted guest page
  fault exit `0x4e`, fault address `0x7000`.

`rust/src/host.rs` installs checked GDT/TSS/IDT images and all 256 assembly
terminal handlers. Double fault selects a dedicated 16 KiB IST1 stack.
A separate negative profile runs the same six guest entries, then executes
host UD2. It must report vector 6 and exit with status 35:

```powershell
./tools/synthetic-harness/build.ps1 -HostFault
./tools/synthetic-harness/run.ps1 -HostFault -QemuPath './target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe'
```

Both profiles passed on 2026-09-09 with empty stderr. Retained evidence:

| Profile | Run directory under `target/synthetic-harness/runs/` | Status |
| --- | --- | --- |
| Rust mappings/dispatcher/faults | `380cb3f6163044b98e35701ee5d4b2fd` | 33 |
| Host UD2 after guest exits | `eebab16b38ee429b90d1f90061c889c6` | 35 |

Each `result.json` retains image/QEMU hashes and the full trace. Image SHA-256:

- Rust: `5CF9F886E9B4D2E9F320B1DF0905C5D7054B0973CEEB19B13BEF853EA802C282`.
- Host fault: `961CEB33A52A66192AE5FAFEBFA5476C71F8CF7A6630D1A7D682FB6F1DEF01A6`.

The existing 65 core tests were unchanged by this harness integration. Both
bare-metal Rust profiles and their assembly bridges built successfully.
The host probe establishes vector-6 delivery after VMEXIT; it does not prove
all 256 vectors, actual double-fault delivery, asynchronous handling or recovery.
This table records the earlier run before host mapping restrictions and the
double-fault probe were added; see the current verification link above. A full
physical world switch remains outside this fixture. No firmware was changed.

## Original assembly backend profile

This assembly-only backend check boots inside QEMU TCG, enables emulated SVM
and NPT, executes an emulated `VMRUN`, and requires the guest's `HLT` to produce
exit `0x78` at the expected RIP. A matching trace and debug-exit process status
33 are both required. The image has no UEFI entry point and is not a card ROM.

This establishes emulator backend viability. It does **not** execute the Rust
hypervisor core, validate hardware SVM, or supply the final private host fault
environment. The smoke maps the first 1 GiB with permissive identity host/NPT
tables, with interrupts disabled. Unexpected faults may terminate through a
triple fault; the runner treats every missing success marker or other exit as
failure. The process is terminated after 30 seconds. These deliberately simple
tables and absent recovery are unsuitable for a physical launch.

## Build and run

The installed LLVM `clang`, `ld.lld` and `llvm-objcopy` suffice; NASM and an
additional Rust target are unnecessary. From the repository root:

```powershell
./tools/synthetic-harness/bootstrap-tools.ps1
./tools/synthetic-harness/build.ps1
./tools/synthetic-harness/run.ps1 -QemuPath './target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe'
```

Tool setup downloads pinned QEMU 20250826 and 7-Zip 26.03 artifacts, verifies
their hashes and extracts them under `target/synthetic-tools`. It does not run
the installers or register a system-wide installation. LLVM must already be
installed. The QEMU installer checksum is the distributor's published SHA-512;
the 7-Zip SHA-256 values pin the official release downloads observed during
setup.

Use the verified portable QEMU tree prepared by the repository's tool setup.
The runner fixes `pc,accel=tcg`, one CPU, `max,svm=on` and 64 MiB RAM. It attaches
no disk, network interface or physical device and does not select a hardware
accelerator. Its file outputs live under `target/synthetic-harness/runs/` in a
fresh directory and include trace, stderr, hashes and process status.

Expected trace:

```text
boot64
svm_available npt_available
vmrun
vmexit=0000000000000078
PASS svm-npt-hlt
```

## Observed execution

The first run passed with the complete expected trace and process exit 33 on
2026-09-09 using the portable QEMU 10.1.0 build dated 2025-08-26. Retained evidence:

`target/synthetic-harness/runs/b6b7654bf4314c558dbf8e7f7bea01cd/result.json`

The executable identifies itself as
`10.1.0 (v10.1.0-12094-g5fa1466eb8-dirty)`; it is the distributor's build,
not an independently reproduced upstream-tag binary.

The flat image SHA-256 was
`E3D288F4A16EC09E15E502BE73BAFB7C3D44602A9315EEAB33BF0D055567D637`.
QEMU stderr was empty. This was an actual software-emulated guest entry and
HLT exit with NPT enabled, using the independent assembly fixture above.

## Primary source checks

The initial review used QEMU tag `v10.1.0`:

- [`svm_helper.c`](https://github.com/qemu/qemu/blob/v10.1.0/target/i386/tcg/system/svm_helper.c#L144)
  implements TCG `helper_vmrun`; lines 269–277 load nested CR3 and enable NPT.
- [`excp_helper.c`](https://github.com/qemu/qemu/blob/v10.1.0/target/i386/tcg/system/excp_helper.c#L419)
  walks NPT for guest page tables and final addresses, combines permissions,
  and emits nested-page-fault exits at lines 496–514.
- [`multiboot.c`](https://github.com/qemu/qemu/blob/v10.1.0/hw/i386/multiboot.c#L183)
  rejects ELF64 through its ELF path. Its address-bearing Multiboot flat-image
  path, lines 210–266, loads the header-specified ranges and clears BSS. Our
  ELF64 intermediate is converted to that flat format; entry starts in 32-bit
  protected mode and explicitly enters long mode.
- The [QEMU download page](https://www.qemu.org/download/#windows) links the
  [Windows distributor](https://qemu.weilnetz.de/w64/2025/), which publishes
  dated installers and SHA-512 sidecars. A publisher checksum authenticates
  consistency with that download, not correspondence to an independently
  reproduced binary from the upstream tag.

VMCB byte offsets and SVM setup follow the repository's pinned AMD APM volume 2
revision 3.44, chapter 15 and Appendix B. This smoke uses independent assembly
constants; a later integration must replace those with the checked Rust core
and test its real entry/exit register contract.

The [running-guest preemption fixture](../../docs/apic-running-preemption-fixture.md) now uses an owned emulator
LAPIC timer to interrupt an integer loop and continue through guest EOI/IRETQ.
It records explicit post-EBS timer takeover and source acknowledgement;
physical platform scheduling and Windows compatibility remain unestablished.

The [timer/fault overlap fixture](../../docs/event-overlap-fixture.md) now preserves pending timer interrupts across guest #UD/#GP/#PF handlers and verifies IF/TPR blocking, STI shadow, EOI and IRETQ on both APIC buses. Windows boot and physical compatibility remain unestablished.

