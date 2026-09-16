# OS boot readiness contract

> Historical (2026-09-16): the synthetic QEMU harness and emulator-only APIC models this report relies on were retired; its run commands no longer exist.

Current user priority, 2026-09-13: **boot the existing Windows installation under
the native DXE hypervisor first**. Existing emulators validate individual
mechanisms; constructing a general emulated machine or a malware-analysis
platform is not the prerequisite. The coverage inventory below describes the
existing fixtures, not a requirement to trap and emulate every native device.

The next native path is ReadyToBoot callback capture -> owned resident host ->
guest ACK/epilogue -> return from the callback inside the guest -> normal
firmware/Windows loader -> ExitBootServices -> Windows. The
[current native continuation report](native-resident-continuation.md) records
implemented preparation and assembly validation. Gate 3 remains open until
this path actually executes; neither a host test nor the earlier synthetic
integer continuation satisfies it.

Status: gates 1 and 2 completed in the bounded emulator, 2026-09-13. Windows has not booted under the
resident runtime. The original readiness batch changed documentation only;
the subsequent [AMD CPU model implementation](amd-cpu-model.md) now supplies
an admitted native-facing CPUID fixture and dynamic guest XCR0 ownership.
Existing physical
evidence remains the exact returning 65-entry probe image.

## Current checkpoint

The completed concurrent idle fixture demonstrates
two guest CPUs, cold AP startup, IPIs, timer preemption, guest HLT wakeups and
actual host HLT returns under the pinned corrected QEMU TCG backend. Its frozen
summary records 265 core tests, 251 DXE tests, 32 concurrent positive runs,
one topology refusal and 35 legacy cases. These are retained results, not new
tests performed for this document.

Image SHA256: `cf30121082a2b485ac0fc3c1760bb729d12b62d69310465e2f0afb49a248c1fe`.
QEMU SHA256: `c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`.
Evidence: `work/concurrent-idle/final-validation/summary.json` and the verified
archive `C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/concurrent-idle-2026-09-13`.

The development PC reports AMD Ryzen 9 9900X, 12 physical cores and 24 logical
processors. The concurrent harness deliberately requests one socket, two cores,
one thread per core from QEMU. Those two emulated processors host two svmvisor
guest CPUs. They are not additional physical processors. Arbitrary CPU counts,
hotplug and migration are outside the tested topology.

## Machine profile decision

There is currently **no combined OS-capable machine profile**:

| Existing runner | Machine / memory / CPUs | Proven scope |
| --- | --- | --- |
| `tools/synthetic-harness/run-concurrent.ps1` | pc, TCG multi-thread, 64 MiB, exactly 2 | Flat Multiboot concurrent fixture |
| `tools/synthetic-harness/run-uefi.ps1` | q35, TCG, 256 MiB, 1 | Owned post-ExitBootServices handoff and bounded continuation |
| `tools/synthetic-harness/run-uefi.ps1 -UefiSmp` | pc-q35-10.1, TCG multi-thread, 256 MiB, exactly 2 | Admitted firmware ownership, resident INIT/SIPI, concurrent timer/IPI/HLT |

QEMU's `max,svm=on,hypervisor=off` describes the processors on which svmvisor
runs. It does not describe the CPUID policy svmvisor exposes to its own guest.
The original fixture policy is `SvmVisorTest`; the separate AMD model has a
native-facing identity. Neither establishes a complete OS execution contract.

The [two-CPU UEFI ownership fixture](uefi-smp-ownership.md) now executes the
combined firmware-to-runtime ownership and CPU rendezvous profile. Ten positive
runs and nine expected refusals use pinned firmware, variable template and
machine version. Complete loader continuation, allocator-visible reservation
and OS platform admission remain later gates.

For a separately selected Windows 11 emulator validation candidate, budget at least two vCPUs, 4 GiB RAM and a
64 GiB virtual disk, with Secure Boot capability and a virtual TPM. These are
minimum VM requirements, not proof our emulated CPU or platform qualifies.
Pin the Windows edition/build/media hash, firmware, CPU feature model, device
inventory and protection configuration before admission. See Microsoft's
[Windows 11 requirements](https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements).
No Windows image or mutable firmware variables are created by this contract.

## Exit and execution coverage

The table is a readiness inventory, not a replacement for each linked fixture's
instruction-level contract. The shared dispatcher is not the complete fixture
runtime: specialized callers own APIC, events and scheduling. A core helper or
permission bitmap alone is not evidence that its intercept is enabled and its
handler reachable in a runnable profile.

| Area | Current owner / evidence | OS integration requirement |
| --- | --- | --- |
| CPUID | `svm/emulation.rs`, `svm/dispatch.rs`: fixed diagnostic leaves; exact instruction completion | Define coherent CPU identity, topology, FPU/SSE/NX/XSAVE and dependent MSR/control behavior; no indiscriminate host forwarding |
| VMMCALL / VMRUN | `svm/exit.rs`, diagnostic query/stop ABI; VMRUN intercepted | Keep private ABI scoped; nested SVM/Hyper-V requires separate architecture and evidence |
| MSRs | `svm/x2apic.rs`, harness `clock.rs`, intercepting MSRPM | Inventory every exposed feature's MSRs, ownership and architectural fault behavior; fixture APIC/clock support is not a general MSR model |
| I/O ports | Shared `execution.rs` enables IOIO with deny-all IOPM; [real IN/OUT boundary fixture](io-intercept-boundary.md) proves stopped refusal and a live endpoint side-effect witness | All ports remain terminally refused; explicit device/port completion owners are still required |
| HLT | clock/HLT, concurrent idle callers park/wake guest; generic dispatch stops | Integrate per-vCPU event eligibility and actual host idle into UEFI profile; never treat all HLT exits as final completion |
| CR / DR | Narrow MOV CR8 write adapter in `svm/x2apic.rs`; no general control/debug-register emulation contract | Decide native execution versus intercept per register and mode, including TLB/ASID invalidation and fault semantics |
| XSETBV / xstate | Intercept enabled; bounded existing extended-state preservation profiles | OS feature policy and dynamic XCR0 transitions require validation; host AVX test profile does not imply guest AVX advertisement |
| NPT / byte fetching | `memory/npt`, `guest/pages`; bounded backing audit; xAPIC MMIO accepts exact MOV forms | General GPA map and checked instruction fetch across mutable guest paging; classify RAM/MMIO/ownership faults without inventing guest #PF |
| Exceptions | [guest exception continuation](guest-exception-continuation.md), interrupted delivery: bounded #UD/#GP/#PF, selected delivery faults, terminal #DF/shutdown | Extend supported delivery combinations explicitly; preserve interrupted-event state, no blind retry or RIP advance |
| External IRQ / interrupt windows | event overlap, preemption, concurrent timer/IPI fixtures | Integrate device routes and pending-event ownership; bounded LAPIC tests do not implement a platform interrupt fabric |
| NMI | Interrupted external/NMI delivery remains refused by current contract | Define NMI blocking, delivery and interrupted-delivery policy; no supported OS NMI claim |
| APIC / startup | Partial `svm/local_apic.rs`, `x2apic.rs`, `xapic.rs`, scheduler and IPI owners; startup | Match ACPI MADT, CPUID IDs and AP startup; IOAPIC/PIC routes, omitted registers, INIT of running CPUs and general MMIO decoding remain separate work |
| Invalid VMCB / shutdown / unknown exits | `svm/exit.rs`, generic terminal outcomes and bounded shutdown fixtures | Stop with diagnostic state and incomplete result; never skip an unsupported instruction to obtain boot progress |
| Clocks | [clock ownership](clock-ownership.md), synthetic APIC deadlines, measured raw TSC intervals | Consistent advertised time sources, cross-CPU ordering and timeout behavior; no native calibrated latency claim |
| PCI / MMIO / storage / DMA / network | Delivery fixtures and bounded APIC alias do not form a guest device model | Explicit GPA/device inventory, interrupt routing and ownership; NPT alone does not isolate DMA or persistent/network effects |

For every implementation row, acceptance must name the trigger/intercept,
authoritative stopped VMCB/register state, owned memory, register/RIP/flags
effects, pending-event handling, resume prerequisites and refusal outcome.
Preparation rejection must leave guest state unchanged. Completion advances
only a validated instruction; reflected faults retain architectural fault RIP.
Record bounded exit counters, stop reason and telemetry loss without allocation
or unbounded logging in the exit path.

## Ordered implementation gates

0. **Define the AMD guest CPU model now.** User clarification on 2026-09-13:
   CPUID must be an early prerequisite, not postponed until OS boot. The OS
   profile should use `AuthenticAMD` with coherent basic/extended maximum
   leaves, family/model policy, features, cache/topology/address-width data,
   and feature-dependent subleaves. This means complete behavior for the chosen
   virtual CPU model, not advertising every feature of the physical Ryzen.
   Inventory basic leaves 0/1/7 and extended 0x80000000/1/8, brand/cache/topology
   leaves as applicable to that selected model, and leaf 0xD if XSAVE is exposed.
   Pin modern leaf definitions in the current AMD APM/PPR; the archived
   [AMD CPUID specification](https://www.amd.com/content/dam/amd/en/documents/archived-tech-docs/design-guides/25481.pdf)
   corroborates vendor/basic conventions but is not a complete modern leaf list.
   Define unsupported/reserved leaf and subleaf behavior, per-vCPU APIC IDs,
   and state-dependent answers such as OSXSAVE. Cross-check each advertised
   bit against instruction execution, CR/MSR policy, exception behavior and
   state preservation. Hidden features may still execute unless separately
   constrained: CPUID filtering is not instruction enforcement. Keep the
   diagnostic fixture policy separate until the new model has a real admitted
   caller and positive/refusal tests. Do not expose SVM, encrypted-memory or
   other unsupported facilities simply by forwarding host leaves. CPUID model
   design can proceed alongside gate 1; its runtime admission depends on the
   matching execution owners, not just a leaf table.
1. **Close and prove the I/O intercept boundary.** Reuse shared `execution.rs`
   and existing VMCB intercept representation. Add a benign real IN/OUT fixture
   whose intercepted access stops before completion, with RIP/register evidence
   and a forbidden side-effect witness. Prove a disposable benign emulator
   endpoint is live with an allowed control, then prove the trapped OUT did
   not change it; IN must preserve the stopped destination register/RIP/flags
   and never reach a post-instruction guest marker. Cover port-span boundaries,
   operand/direction metadata, and explicit string/REP refusal if scalar-only;
   keep unknown ports refused. This is the immediate bounded intercept batch; it
   does not require constructing a new device abstraction.
   **Completed for the shared emulator boundary:** see the
   [I/O interception contract and final evidence](io-intercept-boundary.md).
   No general guest I/O emulation or native Windows policy is implied.
2. **Integrate two-CPU UEFI ownership.** Reuse firmware-handoff, `host_smp`,
   `host_lapic` and existing scheduler/controller owners. Establish both CPU
   identities and allocate/admit owned storage before successful EBS. Allocate/reserve a
   SIPI-addressable low page before EBS, validate its ownership and pass the
   admitted page/vector. All firmware MP procedures must have returned before
   EBS; do not leave a nonreturning firmware MP callback across that boundary.
   Validate the pinned OVMF AP-idle contract and perform resident AP startup
   after EBS, with no subsequent Boot Services calls. This startup transition
   is new integration work, not established by the existing cold-AP fixture. Prove
   per-CPU stacks, VMCBs, HSAVE, xstate and interrupt ownership, then repeat
   startup/timer/IPI/HLT evidence in this one profile. Refuse missing/extra CPUs.
   **Completed for the pinned emulator profile:** see
   [two-CPU UEFI ownership and final evidence](uefi-smp-ownership.md). The AP's
   firmware callback returns before EBS; resident startup uses the allocated
   low page and retained complete map. No physical SMP or Windows support is implied.
3. **Execute the native callback continuation.** Preserve the actual ReadyToBoot
   stack, CR3, GDT/IDT, TLS/system/debug and extended state, enter its ACK
   trampoline, then return to firmware as a guest. Preparation/transactional
   state commit and assembly seam checks are implemented. Persistent host
   linkage, actual capture admission and the executed callback return remain.
   Validate with a benign EFI continuation that checks its original state.
4. **Retain the resident host across Windows takeover.** Acquire legal runtime
   code/data through a runtime-driver delivery contract before ReadyToBoot;
   drivers must not allocate EfiReservedMemoryType. A separate raw payload
   avoids treating the original loaded DXE PE as immutable through automatic
   runtime-image virtual-address fixups. Validate the actual memory map,
   exclude monitor pages in NPT, and prove EBS/allocator/runtime-service
   transitions preserve the host. The bounded native map builder is implemented;
   it does not itself allocate or publish an OS-visible reservation.
5. **Integrate the existing native machine's Windows boot path.** Establish
   per-CPU resident ownership and coherent topology/startup. Select native
   execution versus interception for real I/O, APIC/interrupts, CR/DR, MSRs and
   xstate; the fixture deny-all I/O policy is not the native boot policy.
   Complete EFER handling and the required boot exits, then execute the selected
   Windows loader and record milestones/first unsupported exit. A full device
   emulator, malware analysis, telemetry and untrusted-workload isolation are
   subsequent work. The actual Windows/protection configuration must be recorded;
   no settings are silently disabled to get a boot result.

UEFI's final successful memory map and EBS boundary are normative: LoaderCode
can be reused by the OS after EBS; retaining a private copy does not reserve
the original arena. Boot Services and device-handle protocols cannot be called
after EBS. See local UEFI 2.11 Table 7.10 and sections 7.2.3/7.4.6, plus
[resident ownership](resident-ownership-handoff.md) and
[loader continuation](loader-continuation.md).

## Compatibility and measurement boundaries

Windows boot, Hyper-V, VBS, HVCI, PatchGuard compatibility and Secure Boot
integration are untested here. Nested virtualization is not implemented;
advertising SVM is not an acceptable shortcut. Record each configuration's
actual enabled state; do not disable protections to manufacture a pass.
Microsoft's [nested virtualization guidance](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/nested-virtualization)
does not establish support for this independent hypervisor.

Existing timing evidence is QEMU raw TSC data and includes documented bridge/
arming overhead. Native baseline comparisons, observation-on/off comparisons,
OS clock consistency, device isolation and disposable-machine reset remain
unmeasured. No telemetry in this batch is an analysis result, containment proof
or claim of undetectability.

## Review and provenance

Primary architecture source: AMD APM volume 2 revision 3.44, March 2026,
SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`;
SVM interception/exit semantics chapter 15 and APIC chapter 16.
UEFI 2.11 SHA256
`a64b8e442004b91becc3de9afaf8ca61b259a9a3b436accb6b3711ab5400cee9`.
Microsoft pages checked 2026-09-13. Source and evidence hashes for this review
are recorded in `work/os-boot-readiness/source-manifest.json`.
Independent review: `work/os-boot-readiness/independent-review.md`.
Documentation validation does not add runtime coverage; no binary was changed
or new guest executed for this readiness assessment.
