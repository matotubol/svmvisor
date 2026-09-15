# Executed resident DXE continuation — 2026-09-13

The actual resident DXE driver now enters its captured ReadyToBoot callback as
a guest, returns to firmware, and continues through ExitBootServices. A benign
EFI consumer also calls GetTime, installs an identity SetVirtualAddressMap,
calls GetTime again, and observes continued resident CPUID dispatch.

This is executable evidence on the pinned single-CPU QEMU/OVMF validation
backend. It is not a Windows boot or a physical-machine result. The EFI consumer
loads the actual driver and explicitly signals the real ReadyToBoot event group;
this does not test option-ROM delivery or the firmware's natural event timing.

## Implementation

DXE legally allocates RuntimeServicesCode with AllocateAnyPages, selects a
retained 1 MiB extent within one 2 MiB window below 1 GiB, relocates a separately
linked raw payload, and registers the callback. Allocation failure/rollback
preserves exact ownership. If event cleanup fails, the driver remains loaded
and inert instead of returning an error that could unload its live callback.

The core owns private page tables, GDT/TSS/IDT, guarded normal/fault stacks,
VMCB/HSAVE/auxiliary state, the integer world switch, and the exit dispatcher.
Its persistent call graph has no firmware calls or external linked symbols.
The raw allocation is not a registered runtime PE, so EDK's PE relocation list
does not rewrite its host pointers during SetVirtualAddressMap. The shim is an
EFI runtime driver; its continued guest lifetime is separate from the core.

CPUID and logical EFER completion reuse the existing transactional handlers.
The first backend attempt correctly refused the absent NRIP-save capability.
The final implementation instead fetches the two instruction bytes through
the stopped guest's current page tables. A temporary supervisor RO/NX alias
admits only retained RAM outside the monitor, verifies current MTRR/PAT caching,
and disappears before guest entry. Each byte is translated separately, including
across noncontiguous pages. Prefixed encodings remain explicitly unsupported.

Guest xstate remains live; the complete linked host contains no FP/SIMD or
xstate-changing instructions. Only the original callback epilogue restores its
captured xstate before returning to the firmware caller. Native I/O, interrupts,
HLT and other admitted boot operations run directly under the native exit policy.

## Evidence

The retained bundle is `work/resident-activation/summary.json`. Each final build
also contains its source snapshot, source hashes, link/disassembly logs and
artifact hashes. Earlier failed attempts remain retained.

| Check | Result |
|---|---|
| Core host tests | 338 passed |
| DXE native-returning host tests, including resident preparation | 278 passed |
| Original capture assembly compared with frozen prior source | Unchanged |
| Resident assembly seam | 102 instructions, 316 bytes |
| Entire test payload | 3,575 instructions; no FP/SIMD/xstate changes; no undefined symbols |
| Entire production payload | 3,130 instructions; same audit properties; build only |
| Callback ACK and original firmware return | Passed |
| Checked CPU state and Win64 GPR/XMM witness | Passed |
| Allocator use after callback | Passed; no runtime-reservation overlap |
| ExitBootServices with continued resident dispatch | Passed |
| GetTime and identity SetVirtualAddressMap | Passed |
| Two CPUs / SVM disabled | Refused before registration; checked native CPU state preserved |

The final virtual-map run observed 108 total exits at its last witness,
including 95 preceding CPUID completions and 11 MSR completions. Counts are
fixture observations, not a performance baseline. `TLB_CONTROL=1` currently
flushes on every entry; timings must not be presented as optimized overhead.

Test driver SHA256:
`b6f242f3572bf27212dc5afd8c1449c75ecc0df0c093d704222a06c4e12fc909`.
Production driver SHA256 (built/audited, not executed):
`fd427350151d192e56c730aaa6b325ae82d09dab30805db1199ccaf815a66e02`.

The corrected QEMU executable is pinned to
`c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`;
OVMF code is pinned to
`33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a`.
No new backend patches were needed for this result. Every run uses fresh
firmware variables, a generated fixture-only disk, and no network interface.

## Before the Windows first-boot attempt

1. Transfer native multicore startup into per-CPU resident ownership. This
   executable profile admits exactly one enabled processor; existing synthetic
   SMP fixtures do not establish native AP continuation.
2. Extend admission to the actual machine's CPU and memory-encryption/routing
   profile using retained platform evidence. Current admission refuses any
   advertised encryption capability, requires flat four-level native mappings,
   and refuses instruction/page-table backing below 1 MiB. It cannot be labelled
   a ready image for the existing multicore machine.
3. Exercise the Windows loader and its runtime mappings with the resulting
   native boot policy, then package and validate the exact hardware candidate.
   Identity SetVirtualAddressMap does not prove Windows' virtual address layout,
   VBS/Hyper-V coexistence, or every loader instruction/MSR transition.

Malware analysis, general device emulation, containment and telemetry are not
prerequisites for this first-boot milestone. No physical SVM launch, firmware
programming, reboot, Windows/ESP change, or Windows protection change occurred
in this batch. Prior physical proof remains limited to its exact returning image.

Architectural references: local AMD APM volume 2 revision 3.44 (SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`),
UEFI 2.11 sections 7.1, 7.2, 7.4.6 and 8.4.1, and pinned EDK II
`82cfea329cc2214df006edc067ed852f4d86a314`, RuntimeDxe virtual-map relocation code.
