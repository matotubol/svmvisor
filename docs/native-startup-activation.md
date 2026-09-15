# Native AP activation and startup ownership — 2026-09-13

Final validation completed 2026-09-14 (Europe/Amsterdam).

The diagnostic DXE profile now starts **2, 24 and 32 emulated host CPUs into
their own resident runtimes**, after successful ExitBootServices return. Each
AP enters through physical INIT/SIPI, captures its native long64 continuation,
executes VMRUN, acknowledges the callback and returns as a guest. The BSP enters
last and returns to the benign EFI consumer. GetTime, identity
SetVirtualAddressMap and another GetTime pass with all guest continuations
acknowledged. This advances beyond the previous preparation-only checkpoint.

This is an explicit consumer-controlled post-EBS seam, not an unmodified Windows
loader integration. The ordinary driver still refuses multiple CPUs. No physical
machine was launched, flashed or rebooted; Windows/ESP and protections were not
changed. Physical proof remains limited to the older exact returning image.

## Ownership and implementation

`native/resident/physical.rs` publishes a versioned configuration-table ABI.
Its `start` function requires BSP, IF=0 and successful EBS return. PI1.10
II-13.4.1 explicitly makes same-group notification order nondeterministic, so
an EBS notification or returned MP observation never grants AP ownership.

DXE admits every runtime slot and bootstrap resource before issuing physical
INIT. A second bounded returning MP observation checks each CPU's controls,
capabilities, current shared root, MTRRs/PAT, runtime mappings and low startup
page. The low-page allocation has exact rollback ownership before publication;
published resources remain retained on activation failure. Partial-start failure
retention was source-reviewed; no failure after an earlier successful AP entry
was deliberately injected in the final matrix.

The low-page integer trampoline installs per-CPU bootstrap stacks and descriptor
tables, preserves INIT-retained cache-control bits, enables x2APIC and calls the
existing assembly capture before any Rust call. MFENCE publishes its selected
bootstrap record before x2APIC ICR writes. AP launches are serialized until each
guest callback returns; repeated activation refuses without changing ownership.

The existing separately relocated 1 MiB runtime per CPU owns its private host
root, stacks, descriptors, VMCBs, HSAVE, instruction aperture and dispatcher.
Every NPT excludes the entire monitor pool. Runtime directory ABI **version 3**
keeps its 160-byte layout but adds inventory pointer/count to ArmRuntime's Win64
arguments. Each runtime checks its assigned APIC ID and copies its RAM metadata.
The complete persistent payload remains integer-only and firmware-independent.

The diagnostic consumer deliberately retains its low LoaderCode startup page
and original firmware paging structures until reset. Those lifetimes do not
establish safe reuse by an ordinary OS loader. Bootstrap stacks/descriptors are
retained in the runtime DXE image, outside the private monitor pool, so their
captured guest continuations remain accessible to the guest.

## Native startup-write policy

The existing `svm::ipi` owner now handles actual native x2APIC ICR writes without
using the synthetic APIC controller. Every activated CPU enters in verified
x2APIC mode; MSRPM intercepts ICR and APIC_BASE writes, preventing a switch to an
unowned xAPIC MMIO path. Ordinary ICR reads remain native.

For intercepted WRMSR, the stopped VMCB/frame and independently fetched exact
instruction bytes are authoritative. Opcode, pending-event, mode, CPL, reserved
bits, debug state and next-RIP checks precede a physical write. Admitted ordinary
ICR writes forward unchanged, then complete the instruction. Reserved ICR bits
prepare #GP at the original RIP. An unchanged APIC_BASE write completes without
altering hardware. Mode/relocation changes and INIT/SIPI remain stopped without
advancing RIP or changing registers/hardware.

The source is a captured Running guest; remote inventory slots are Assigned,
not falsely classified as Running or Cold. Startup toward either is refused.
This protects current resident CPUs but **does not implement architectural guest
reset or SIPI startup**. External INIT sources and xAPIC routing remain unowned.
An INIT VMCB intercept alone is not substituted: AMD APM2 15.13.4/15.21.8 says
intercepted INIT remains pending, and there is no general SIPI intercept.

## Two diagnosed failures

The original backend cleared AP MTRRs after firmware INIT, causing correct
write-back admission refusal. AMD APM2 rev3.44 Table14-1 requires INIT to retain
MTRRs and CR0.CD/NW. The isolated `work/qemu-init-preservation` candidate corrects
only AMD TCG INIT in `target/i386/helper.c`; RESET and other profiles are unchanged.
The exact same driver **and fixture** bytes fail AP cache admission on the old
backend and pass it on the new one (`run-attempt-02` versus `run-attempt-03`).
Actual-function extracted regression tests additionally pass 8,192 cases, an
old-body failure control, RESET and three scope controls. These are separate
from the live CPU evidence. The derived incremental build verifies inherited
objects/libraries and all 11,311 source files, changing only that helper source.

The next run exposed an overly strict native TR validator. AMD APM3 rev3.37 LTR
(printed422) marks the **GDT descriptor** busy after loading hidden TR state;
VMLOAD/VMSAVE (printed499/507) transfer hidden state. The validator now also
accepts captured available 64-bit TSS type9 and preserves its exact bytes, alongside
existing3/B. It does not rewrite type9 toB or admit new available16-bit type1.
Preservation and transactional rejection tests cover the correction. No QEMU
LTR change was made. All failed attempts are retained.

## Final validation

Evidence: `work/native-startup-activation/summary.json`, source snapshots,
per-build audits, exact artifact hashes and fresh per-run fixture media/variables.

| Check | Result |
|---|---|
| Core host tests | 351 passed |
| DXE native-returning host tests | 282 passed |
| 2 / 24 / 32 CPUs | All actual guest callback continuations; complete retained pool; runtime services and identity virtual map passed |
| Repeated Start | Refused with completion/failure masks unchanged |
| Ordinary native ICR write | Changed full hardware ICR readback; IF held clear afterward |
| Self INIT, self SIPI, remote AP INIT, APIC mode change | Exact terminal MSR refusal; no return past write |
| Terminal refusal accounting | Required prior activation, exact reason/MSR/stop, missing post-write success, then 10-second watchdog termination |
| SMP image on 1 or 33 CPUs / x2APIC disabled | Refused; native state witness preserved |
| Ordinary image on 2 CPUs | Refused; native state witness preserved |
| Ordinary single-CPU callback/runtime-services | Passed on old and corrected backends |
| Two-CPU preparation-only profile | Passed; no resident activation |
| Complete diagnostic payload | 5,431 linked instructions, no FP/SIMD/xstate changes or undefined symbols |
| Complete production payload | 4,780 instructions, same audits; built only |

Final SMP driver SHA256:
`107ac04e5e6d9c69f84addf56446c9cf7d82bba35bf683d23411aed28762be3d`.
Final executed ordinary driver:
`b21ebb5aa288ace0eda1b5ea337f61dd97f2ceb5ad9b2fab959865f56bc9f6c4`.
Production driver, not executed:
`996d17591b8c5e750839b63995f3c51cc869dcf549518124237a826d9db464f9`.
Corrected QEMU:
`07c6409a119ea0d48e880fe55e0ad004812aeff6242823e9975287a8f3147451`.
OVMF remains:
`33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a`.

Use fresh directories; never rerun finalizers into frozen evidence:

```powershell
python tools/native-resident/build.py --output work/new-smp-build --test-output --smp-activate
python tools/native-resident/run.py --output work/new-smp-run --driver work/new-smp-build/driver.efi --features smp-activate,virtual-map --cpus 24 --init-preserving-backend
```

## Remaining first-boot work

Native guest INIT/reset and real16 SIPI continuation, xAPIC startup routing,
and a correct native integration for an unmodified Windows loader remain open.
The current AP workload is a captured long64 diagnostic continuation, not the OS
SIPI vector. Actual-machine CPU/encryption/routing admission and protected Windows
compatibility remain untested; no protection was disabled to bypass admission.

No native timing baseline or multicore latency distribution was measured.
TLB_CONTROL still requests a flush on every entry. Counted startup/poll loops
are bounded diagnostic waits, not calibrated physical delays. AP guest xstate is
captured/restored by the existing boundary, but no separate AP xstate sentinel
comparison was executed. ICR readback proves forwarding, not ISR delivery.
Malware analysis and general device emulation remain deferred.
