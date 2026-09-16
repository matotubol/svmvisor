# Two-CPU UEFI ownership

> Historical (2026-09-16): the synthetic QEMU harness and emulator-only APIC models this report relies on were retired; its run commands no longer exist.

Status: gate 2 completed for the pinned emulator, 2026-09-13.
The final matrix, regressions and hash-bound execution audit passed. This records gate 2 of
[OS boot readiness](os-boot-readiness.md); later gates remain incomplete.

The combined profile now transfers two admitted q35/OVMF processors from
firmware into the existing concurrent runtime. The BSP allocates the payload
arena, discovers both CPUs, runs a small returning AP identity callback, and allocates a
SIPI-addressable page, validates ownership, and successfully exits Boot
Services. The private runtime starts the AP with directed INIT/SIPI and runs
the established timer, IPI, startup and HLT conformance workload on both CPUs.
No nonreturning firmware MP callback crosses this boundary.

## Profile and authorization

`tools/synthetic-harness/build.ps1 -UefiSmp` selects the combined payload;
`tools/synthetic-harness/run-uefi.ps1 -UefiSmp` supplies the disposable firmware
driver and launcher. Firmware `SVMVISOR_UEFI_SMP=1` is a compile-time opt-in.
The existing exact binary LoadOptions token remains mandatory. The token
authorizes this fixture; it is not cryptographic proof of emulator identity.
Legacy flat and single-CPU UEFI profiles retain their existing entry paths.

The admitted machine is `pc-q35-10.1`, 256 MiB, TCG with `thread=multi`, one
socket/two cores/one thread per core, and
`max,svm=on,hypervisor=off`. The first complete run used the AVX xstate profile
with RDTSCP available. Network is disabled and the ESP runs through QEMU's
snapshot drive mode with a per-run variable-store copy. These two emulated
processors are the executing host CPUs for two nested guest CPUs, not the
development PC's physical core inventory.

| Pinned artifact | SHA256 |
| --- | --- |
| Corrected QEMU executable | `c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047` |
| `edk2-x86_64-code.fd` | `33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a` |
| Pristine `edk2-i386-vars.fd` template | `5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e` |

## Firmware boundary and complete ownership record

`tools/synthetic-harness/firmware-handoff/src/smp.rs` owns MP Services discovery
and low-page allocation. Admission requires exactly two total and enabled
healthy processors, BSP handle index 0, and firmware processor IDs matching the
explicitly checked APIC IDs 0 and 1. Other numbering/topologies are refused.
Direct CPUID executes on each CPU and supplies `AuthenticAMD`, a matching
nonzero signature, and the executing CPU's APIC ID; Windows identity services
are not involved.

The AP callback performs bounded CPUID and stores only. It publishes completion
with release ordering and returns. The BSP uses blocking `StartupThisAP` with
no event and a one-second firmware timeout, requires success and exactly one
completed callback, then acquires the captured data. All protocol guards are
dropped before ExitBootServices (EBS). The callback record is never reused as
resident AP storage.

The loader allocates one `LoaderCode` page through
`AllocateType::MaxAddress(0xfffff)`, or an explicit `SVMVISOR_SIPI_PAGE` request.
It rejects zero, misalignment, addresses at/above 1 MiB, allocation failure,
overlap, missing LoaderCode coverage, read-protected backing or absent WB
capability. A WB capability bit is not an effective physical cache-policy
measurement. The requested page is not hardcoded to the flat fixture's 0x8000.
All later pre-EBS preparation failures release the low page and arena.

`firmware-handoff/src/ownership.rs` normalizes a preliminary memory map and
admits the full arena and low page before EBS. That map's pool owner is dropped
before EBS. After successful EBS, the same encoder retains the actual final map
and SMP metadata. No allocation, protocol operation, FreePages, firmware return
or other Boot Services call follows success. Post-EBS failures are terminal.

`crates/hypervisor/src/boot/ownership.rs` owns the pointer-free record and its
validation. The 4096-byte handoff page retains the existing 64-byte image
prefix, followed by a versioned ownership record:

| Record | Header | Descriptor stride | Maximum descriptors |
| --- | --- | --- | --- |
| Version 1, no SMP | 64 bytes | 32 bytes | 124 |
| Version 2, two-CPU SMP | 160 bytes | 28 bytes | 138 |

Version 2 adds the low-page base/length, returned-callback count and two CPU
identities. Each packed descriptor contains type/base/page-count/attributes at
offsets 0/4/12/20. All descriptor fields remain present; only reserved ABI
padding is omitted. Eight unused trailing bytes remain zero at maximum
capacity. Encoding is field-by-field little endian. Neither dropping nor
coalescing descriptors is used to fit the map. Version 1 bytes and its capacity
are unchanged. Excess count, malformed metadata, nonzero reserved bytes and
invalid memory spans refuse transactionally without changing the output page.

The private consumer copies the final record into owned image BSS before
replacing firmware mappings. Its projected allocation map reserves both the
arena and SIPI page and preserves total described bytes. Generic guest/NPT
admission excludes monitor storage. This internal projection is not a memory
map published to a Windows allocator; that is the later memory-contract gate.

## Resident AP startup and concurrency

The [pinned OVMF research](../work/uefi-smp/research/ovmf-ap-contract.md) traces
the shipped firmware to `edk2-stable202408`, commit
`b158dad150bf02879668f72ce306445250838201`, through QEMU's prebuilt firmware
update `065e2ecf79cdf7da94542caab2b847de57035d8c`. The later EDK II submodule at
QEMU v10.1.0 is not the provenance of these prebuilt bytes. This is verified
upstream release provenance, not a locally reproduced firmware build.

That firmware prepares reserved AP loop code, private page tables and stacks
before EBS. Its EBS callback moves APs to that reserved sequence. The completion
decrement precedes the final CR3/RSP writes and idle instruction, so successful
EBS does not prove that the AP retired HLT. Resident directed INIT establishes
the architectural takeover boundary even while the reserved sequence finishes.
The runtime does not resume the firmware AP stack or dispatch another MP call.

`host_smp`, `host_memory` and `host_lapic` reuse the existing startup, mapping,
interrupt and timer owners. Startup receives the admitted page/vector and
patches the protected-mode target, temporary GDT base and CR3 in the copied
trampoline. Its temporary mapping is writable during preparation, executable
during startup and removed after the AP joins. The resident AP installs owned
descriptors, stack, HSAVE and execution state, then checks its identity against
the pre-EBS capture. INIT is not a wholesale xstate reset; existing per-CPU
xstate initialization/restoration remains necessary.

An independent P1 publication finding was corrected before the complete run:
the AP now acquires the entry publication before reading BSP-written resident
metadata. The earlier phase value alone did not publish writes made after that
phase was set. The release/acquire pair now covers those writes and the shared
execution preparation.

The existing concurrent controller/scheduler keeps each CPU's VMCB, GPR frame,
guest and host xstate, clock state and interrupt ownership separate. The new
audit checks 16 recorded ranges for arena containment and cross-CPU
non-overlap. Host xstate uses the actual `HostScratch` extent returned by
`State::host_backing()`, not the small `State` metadata object. Its host-stack
evidence samples each live RSP address; that marker
alone is not a complete stack-span audit. Separate existing stack layout,
guest-stack canaries and host restoration checks remain in use.

Remote IPIs notify the target owner; that owner updates its guest pending-event
state. Host timer/IPI acknowledgements remain distinct from guest handler,
EOI and IRETQ completion. Refused preparation does not advance guest RIP or
discard pending events. Guest HLT follows the existing checked parking/wakeup
policy. Unknown exits remain terminal; this integration does not add a general
OS exit policy. Shared IOIO interception remains enabled with scalar and string
port I/O refused unless a future explicit completion owner is admitted.

## First complete execution and preserved development failures

The first complete run is
`target/synthetic-harness/uefi-runs/6688e7fabedb4051bdf65461d81a421c`.
Its `result.json` and raw `debug.log` record exit code 33, no timeout,
`concurrent.validFixture=true`, and the full ordered pre/post-EBS markers.
Both CPUs reported signature 0x663. The arena was 0x200000, the allocated low
page 0x9f000 and SIPI vector 0x9f. Both preliminary and final maps contained all
131 descriptors; the arena and low-page descriptors were type 1 with
attributes 0xf.

| First-run observation | Completed |
| --- | --- |
| Guest entries, CPU0 / CPU1 | 305 / 303 |
| Concurrent two-CPU sessions | 8 |
| Bidirectional guest IPI handler/EOI/IRETQ completions | 64 |
| Guest AP real16/protected32/long64 startup | 1 |
| IPI HLT wake sessions | 16 |
| Deliberate publication after drain, before VMRUN | 1 |
| Guest timer handler/EOI/IRETQ completions | 32 |
| Guest-HLT sessions with actual host HLT source witnesses | 32 |
| Watchdog-only host wakes without guest delivery | 4 |
| Actual host idle returns, CPU0 / CPU1 | 36 / 36 |

These counts belong to that development run only. Its payload SHA256 is
`7d3032571f2d8df71beb124d6f48299659ca539aa2af09bee2c523e6b1eddf04`;
driver SHA256 is
`41f271ecde21033110e2167ad83c2912d039c4cc39aa13245addf1b2fcd46ac3`.
The final audit independently verified this payload against the frozen image;
each final run retains its own driver and launcher hashes.

The initial refusal at run `ae212289b6e24d528b3e36b859815aad` and diagnostic
run `393082a11e104b058a795255a5388dcc` remain preserved. The diagnostic run
reported `uefi-ownership-error=DescriptorCount`: the complete 131-entry map
exceeded the original bounded capacity. The packed version 2 representation
fixed capacity without omitting map content or weakening ownership checks.

The first matrix's one-CPU refusal returned to OVMF and timed out after correctly
refusing admission. It remains preserved in `development-matrix-1` and run
`9cf05184e66e452dacd4da7043ca6fd3`. The disposable launcher now terminates with
exit code 35 on failed driver load/start; the final matrix verifies explicit
termination for every refusal. No timed-out run is counted as passing.

## Final validation and remaining gates

Development checks in `work/uefi-smp/development-validation-1` passed 304 core
tests (`core-tests-2.log`), 15 firmware-handoff tests (`firmware-tests.log`),
254 DXE tests (`dxe-tests.log`) and the core UEFI target check
(`uefi-check.log`). Their tested core and firmware source is included in the
final source snapshot; the subsequent launcher fix is covered by executed
positive/refusal and legacy regression profiles.

`work/uefi-smp/final-validation/summary.json` binds the final results and review.
The matrix passed ten positive runs: AVX/SSE/FX twice each, AVX and FX with
RDTSCP disabled, arena 4 MiB with page 0x90000, and arena 3 MiB with page 0x98000.
Nine negatives passed: one/three CPUs; zero, unaligned, out-of-range and
unavailable low pages; missing SMP metadata; corrupted ownership and handoff.
Across the ten positives, raw logs record 6,060 guest entries, 1,060 interrupt
exits, 640 guest IPI completions, 320 timer completions, 160 IPI HLT sessions
and 735 actual host idle returns. These totals are conformance counts, not
physical performance measurements.

All 204 corrupted-evidence controls were rejected. The strict checker verifies
ordered unique firmware/runtime transitions, CPU identities, page/vector/root,
profile bits and owned addresses, then reuses the existing concurrent checker.
An independent raw-log audit recomputes metrics and binds every result to its
trace and artifacts. The linked-image audit verifies the 134-byte trampoline,
three patch fields and all 3,546 packaged relocations.

Regressions passed 33 flat concurrent, 33 AMD CPU model, 34 I/O and 35 legacy
cases. The final payload SHA256 is
`7d3032571f2d8df71beb124d6f48299659ca539aa2af09bee2c523e6b1eddf04`.
Source snapshot and changes: `work/uefi-smp/source-manifest.json`,
`changed-sources.json` and `implementation.patch`. Independent review:
`work/uefi-smp/independent-review.md`. The verified archive is
`C:/Users/mato/Documents/Codex/2026-09-10/svmvisor-bios-f7-analysis/outputs/uefi-smp-2026-09-13`.

The missing-SMP negative preserves every packed descriptor and zeros only the
mandatory SMP extension in a declared version 2 record. It tests malformed
v2 refusal, not a valid v1 record with a silently truncated map. Host tests also
exercise 131/max-138 complete maps and 139-entry transactional refusal.

Timing counters report raw multi-threaded TCG TSC intervals, per-CPU
monotonicity, ordered shared handoffs, deadline lateness and source-witnessed
idle returns. No native latency calibration, physical cross-CPU synchronization
bound, observation-on/off comparison or Windows time-source consistency has
been measured. Clock-ratio execution is explicitly skipped where unsupported.
The legacy single-CPU timing/interrupt fields in the runner's result remain
`not-completed`; the new run's coverage is the separate `concurrent` object.
Bounded counters and terminal diagnostics are conformance evidence, not a
complete behavior trace or evidence of zero lost security observations.

Gate 3 still requires a real admitted loader continuation with complete
required architectural state. Gate 4 must publish the memory reservation to
the consuming allocator and establish the full platform memory/runtime-service
contract. Gate 5 must admit a coherent OS CPU, ACPI, device, interrupt and clock
profile. Windows has not booted here. Hyper-V, VBS, HVCI, PatchGuard and Secure
Boot compatibility remain untested. No Windows protections were changed.

No physical activation, flashing or malware execution was performed. The older
returning 65-entry physical probe remains evidence for only its exact image.
This emulator result establishes neither physical SMP support, device/DMA or
network containment, complete malware-analysis coverage nor undetectability.

## Normative references

AMD APM volume 2 revision 3.44, March 2026, sections 14.1/14.1.3 and
16.5, Tables 14-1/16-4, govern INIT/SIPI and processor-state effects. Local PDF SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.

UEFI 2.11 sections 7.2.1/7.2.2/7.2.3/7.4.6 and Table 7.10 govern allocation,
the final map and EBS lifetime. Local PDF SHA256:
`a64b8e442004b91becc3de9afaf8ca61b259a9a3b436accb6b3711ab5400cee9`.
PI 1.8A, March 2024, volume 2 MP Services `StartupThisAP` and Table 13.7
provide the blocking completion/timeout contract. Current PI 1.9 was located
but its full chapter returned HTTP 403; no full-current-text inspection is
claimed. The pinned historical firmware source corroborates the selected
contract; retained primary-source research is linked above.
