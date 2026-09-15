# Fresh High-mode task handoff — 2026-09-13

The user requested a fresh task with High reasoning and fresh High-reasoning
agents to continue the next steps toward Windows first boot under the DXE
hypervisor. Continue implementation autonomously; this is a handoff, not a
request to repeat completed preparation or merely produce another plan.

## Authoritative workspace and working rules

Use **C:/Users/mato/.codex/worktrees/7a58/svmvisor** for every repository read,
edit, command and test. The app may assign the new task another initial checkout;
that is not the authoritative implementation. This source tree is intentionally
dirty and has substantial untracked code/evidence. HEAD is detached at
`67cd0563376a44c0dad9df627a97c8c9ac0dfa60`; the commit alone does not contain
the current work. Do not reset, clean, overwrite, commit or reconstruct it from
the saved project's default checkout. The prior task has stopped editing.

Read AGENTS.md, README.md, CONTRIBUTING.md, both crate guides, and
docs/malware-analysis-direction.md. User priority: first boot the existing
Windows machine under DXE. Malware analysis, general device emulation,
containment and telemetry are deferred. Emulator fixtures are validation tools.
Do not silently disable Windows protections or claim unsupported compatibility.

Use fresh subagents with GPT-6 Astra and High reasoning, as recorded in AGENTS.md;
give concrete independent implementation/review ownership. Do not reuse previous
agents. Root alone runs serialized builds, tests, links and QEMU executions.
Keep firmware allocation/protocols in DXE, persistent CPU/exit code in the core.
Reuse real existing owners, with actual callers and meaningful tests. Check
uncertain architecture against the local primary references. No physical launch,
programming, reboot or Windows/ESP modification was performed in these batches.

## Completed checkpoint

Read **docs/native-percpu-preparation.md** and **docs/native-resident-activation.md**.

The actual resident single-CPU driver has executed its captured ReadyToBoot
callback as a guest, acknowledged it, returned to firmware, continued through
ExitBootServices, GetTime, identity SetVirtualAddressMap and another GetTime.
The benign EFI consumer manually signals the real event group; this does not
prove natural option-ROM delivery or Windows loader behavior.

Native per-CPU preparation passes on 2, 24 and 32 emulated CPUs. Each AP completes
a bounded returning CPUID observation, while the BSP prepares a separately
relocated 1 MiB raw runtime copy for each CPU in one retained pool. Each copy has
private paging/descriptors/stacks/VMCBs/HSAVE/state. Every NPT excludes the entire
pool; the actual final EBS map covers its complete runtime-code reservation.
**Secondary CPUs have NOT entered their resident runtime.** The preparation
profile does not arm CPUs, enable SVM or register activation. The ordinary
activation profile still refuses more than one CPU.

Current final tests: **342 core + 282 DXE = 624 passing host tests**. Live final
matrix: 2/24/32-CPU preparation/EBS, 33-CPU refusal, ordinary 2-CPU refusal, and
ordinary single-CPU resident callback/runtime-services regression. Complete
linked payload audits: 3,663 test instructions, 3,218 production instructions;
no FP/SIMD/xstate-changing instructions or undefined symbols. Production image
was built/audited, not executed. TLB_CONTROL still requests a flush each entry;
no optimized timing baseline or physical/multicore guest performance measured.

Frozen evidence (never modify):

- work/native-percpu/summary.json SHA256
  `dbfb8d68f2876f1dd0b4c9ee26d536bfda25b61932b2392e2db91a7dcf55deab`.
- work/native-percpu/final-source/ has 486 files;
  final-source-manifest.json SHA256
  `13bdf50f9a4440166e365659aa88548a9700db2a096778b83be2143d4776cf06`.
- work/resident-activation/summary.json SHA256
  `1d1ae8ea7ba2b455153e1b12ef8638ffba0e8f280625fe14fedf8330b81b7d7b`.
- Earlier work/full-continuation and all returning/UEFI-SMP evidence remain intact.

Final per-CPU preparation driver:
`d95a3f79b7526b58c75ebd6a5b533fb0205721e09dcc545033e2b924b44df913`.
Final executed single-CPU driver:
`cf6e4eb91f1426e35b468459c106b458060dd9968cddfe80d9d2fdbe4a13b33c`.
Final production driver, build only:
`452df46b8d80e4f58aa947341a5ecf6c665d3986ec4b791d7083e1deca92e8fc`.
Physical proof is still limited to the older exact 65-entry returning image.

## Immediate next implementation work

Own native **INIT/SIPI startup requests**, then establish actual per-CPU
activation/continuation. Firmware and Windows can reset APs after an MP callback
returns; simply cloning the callback onto every CPU can reset a resident monitor.
Do not mistake returned observation callbacks or prepared storage for takeover.

AMD APM2 rev3.44 sections 15.13.4 and 15.21.8/Table15-12: intercepted INIT remains
pending and can intercept again. VM_CR.R_INIT conversion to #SX is not an
implemented shortcut; it requires an explicit owner. There is no general SIPI
VMCB intercept. Separate physical AP startup from guest INIT/SIPI state changes.
Narrow xAPIC/x2APIC ICR startup-write routing is real necessary boot work, not a
requirement to build a general device emulator. Preserve ordinary native APIC
behavior and explicitly refuse unowned cases. Do not label a Running AP as Cold
just to reuse the existing one-time fixture initializer.

PI 1.10 confirms same-group EBS callback ordering is nondeterministic. Starting
physical APs from this driver's EBS notification can race firmware AP relocation.
Successful EBS **return** is the boundary used by the previous benign SMP fixture;
an unmodified Windows loader still needs the correct native startup integration.
Before irreversible AP activation, all storage must be admitted and partial-start
failure handling must retain active resources rather than unloading/freeing them.

Useful fresh-agent split: (1) native startup-request routing and running/cold
state semantics, (2) physical AP entry/per-CPU activation integration, (3) independent
architectural review when capacity permits. Root should integrate real callers
and executable conformance evidence, rather than another preparation-only batch.
After SMP activation come actual-machine CPU/encryption/routing admission and the
Windows loader/runtime-map test with the exact hardware candidate.

## Source map and current constraints

- crates/dxe/src/native/resident/activation.rs: actual installation/callback;
  processor inventory, pool preparation and test-only smp-prepare branch.
- processors.rs: actual PI MP inventory; max32, all enabled/healthy; firmware
  numbers and APIC IDs distinct. CPUID-only returning AP callbacks. This is not
  AP-local CR/MSR/cache admission or persistent ownership.
- allocation.rs: RuntimeServicesCode AnyPages pool, exact trim/rollback,
  one1MiBslot/CPU, >1CPU pool aligned2MiB, <=32MiB and wholly below1GiB.
- core host/resident.rs: directory ABI version2,160bytes, complete pool/slot/APIC
  binding; BridgeContext remains112bytes. PrepareRuntime is six Win64 arguments:
  base, output-directory pointer, poolbase, poolbytes, slot, APICid.
- core host/resident/runtime.rs: one separately relocated instance per CPU,
  private integer world switch/dispatch. ArmRuntime takes original EFER,
  resume/ACK/afterACK sites, descriptor pointer/count; checks assigned APIC ID.
  AP arm, copied RAM metadata and instruction fetch still need execution proof.
- core memory/npt.rs: existing IdentityNpt::new API, eight-page TableStorage,
  whole contiguous pool exclusion using absent2MiBleaves plus endpointPTs.
  Preserves the prior high-address <=1MiB profile. Do not broaden strict
  synthetic Npt globally or lose transactional refusal.
- core host/resident/fetch.rs + runtime aperture: current guest page tables,
  two exact bytes independently fetched through a temporary supervisorRO/NX
  window. Rejects whole monitor pool, MMIO and non-WB reads; no NRIP requirement.
  Guest G_PAT, not host PAT, governs instruction mapping checks. Prefixes and
  instruction/table physical backing below1MiB remain unsupported.
- core svm/dispatch.rs: NativeEfer and native CPUID completion; currently native
  long64 profile. NativeEfer cannot simply be reused for real16 AP startup.
  Preserve logical/backing EFER.SVME separation and stopped-state refusal.
- crates/dxe/src/native/resident/runtime.S: integer world switch. Guest xstate
  stays live, host owns no FP/SIMD; generic AP trampolines that FNINIT/LDMXCSR
  cannot be inserted into an already-running guest without a correct owner.
- Existing physical mechanisms to inspect/reuse:
  tools/synthetic-harness/rust/src/host_smp.rs, tools/synthetic-harness/host-smp.S,
  tools/synthetic-harness/firmware-handoff/src/smp.rs.
  They have fixture assumptions (IDs0/1, one stack, fixed LAPIC, post-EBS takeover).
  core svm/ipi.rs only supports one cold startup of identity1; it is not running
  reset/offline/rebind handling. native/admission/cpu.rs ApObservation explicitly
  requires a returning state-preserving observation; do not hide activation there.

Current admission rejects any advertised encryption capability, full APIC IDs
above255, unsupported initial controls, and multi-CPU activation. Actual-machine
admission is unfinished; do not disable protections to bypass it. Identity virtual
map evidence is not Windows virtual-map/VBS/Hyper-V coexistence evidence.

## Primary references, including newly supplied PI PDF

Library: C:/Users/mato/Documents/svmvisor/docs. Vendor references are there,
not under the implementation worktree's docs/vendor.

- UEFI_PI_Spec_1_10.pdf, July2026, SHA256
  `ed35ab171e8aa66514e2f04013faf7912098b960bdf614f973a8b9d4a5ff09ea`.
  User just supplied it; **it is no longer missing**. Read
  work/native-percpu/pi-1.10-review.md for verified printed/PDF pages and rendered
  review. II-13.4.5/Table13.5 governs StartupThisAP; II-13.4.8 WhoAmI;
  II-13.4.1 EBS order. Finished is ignored in blocking mode; blocking timeout
  terminates the procedure. A prototype typo in II-13.4.3 does not override the
  established scalar ProcessorNumber ABI used by pinned EDK.
- 24593_3.44_APM_Vol2.pdf SHA256
  `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
  Markdown extraction under amd64_apm_vol2_markdown; validate critical text
  against the PDF. Processor PPR/BKDG references are also in the library.
- UEFI2.11 and vendor/edk2 HEAD82cfea329cc2214df006edc067ed852f4d86a314.
  Shipped OVMF provenance instead refers to edk2-stable202408
  b158dad150bf02879668f72ce306445250838201. See exact retained excerpts and
  work/uefi-smp/research/ovmf-ap-contract.md. Do not conflate those revisions.

## Validation commands and environment

Windows PowerShell; execute with explicit authoritative workdir. No reset/clean.
For each new batch create a fresh evidence directory and snapshot the prior
frozen source; preserve failed attempts and record actual final source/artifact
hashes. Existing finalizers are one-shot scripts for their completed batches,
not commands to rerun into those directories.

    cargo test -p svmvisor-hypervisor --target x86_64-pc-windows-msvc
    cargo test -p svmvisor-dxe --features native-returning --target x86_64-pc-windows-msvc
    python tools/native-resident/build.py --output <fresh-dir> --test-output
    python tools/native-resident/build.py --output <fresh-dir> --test-output --smp-prepare
    python tools/native-resident/run.py --output <fresh-run> --driver <driver.efi> --features virtual-map
    python tools/native-resident/run.py --output <fresh-run> --driver <prepare-driver.efi> --features smp-prepare --cpus 24

The builder snapshots inputs, rejects source mutation during build, links and
audits the complete raw payload, and builds an EFI runtime driver. The runner
pins QEMU `c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`
at work/qemu-smp-ignne/build-final/runtime/bin/qemu-system-x86_64.exe and OVMF
code `33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a`.
It uses only fresh generated fixture media/variables, no networking or physical
disk. Do not invent NRIP support: this backend does not implement it; the actual
bounded instruction fetch removed that requirement in the previous batch.

No implementation/test process is still running in the handing-off task.
