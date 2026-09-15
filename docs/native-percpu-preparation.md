# Native per-CPU preparation — 2026-09-13

Native MP discovery and separate resident runtime preparation now pass on 2,
24 and 32 CPUs in the pinned QEMU/OVMF validation environment. Each processor
has a separately relocated 1 MiB runtime copy with its own stacks, descriptor
tables, VMCBs, HSAVE, page tables, state and instruction-read aperture. All copies
exclude the complete retained monitor pool from guest nested translation.

**This prepares secondary processors; it does not activate SVM on them.** The
diagnostic preparation image deliberately returns without registering the
ReadyToBoot activation callback. The ordinary image still admits only one CPU,
and its actual callback/EBS/runtime-services regression passes with these changes.
Windows and physical resident SMP remain unexecuted.

## What changed

`native/resident/processors.rs` uses the existing PI MP protocol ABI to obtain
actual firmware processor numbers and CPUID identities. Every enabled healthy
AP completes a bounded, blocking, returning CPUID callback. Completion uses
release/acquire publication, the callback checks its actual WhoAmI identity,
and inventory is checked again before publication. The dense runtime slot,
firmware handle, and APIC ID remain distinct. The current admission requires
all processors enabled and healthy, common capabilities, IDs no wider than
eight bits, and at most 32 processors. Observation does not grant AP ownership.

The existing allocation owner now supports a single contiguous runtime pool
with one 1 MiB slot per processor. Multiprocessor pools are aligned to 2 MiB,
use legal RuntimeServicesCode AllocateAnyPages calls and trim only pages they
own. The complete pool stays below 1 GiB. Partial trim failures preserve exact
rollback ownership. One-CPU allocation retains its prior selection behavior.

The existing identity-NPT owner excludes the contiguous pool with its same
eight-page storage: complete 2 MiB leaves are absent and only partial endpoints
need 4 KiB tables. Aligned 24/32 MiB pools use four tables at the 40-bit aperture;
arbitrary admitted endpoints need at most six. Prior high-address small-pool
behavior remains supported. Capacity and ownership checks precede mutation.

Resident directory ABI version 2 records the whole pool, dense slot and APIC
identity. Its size is 160 bytes; the assembly BridgeContext remains 112 bytes.
Each relocated runtime verifies its assigned APIC ID before arm. Instruction
fetch excludes the entire pool, not merely the current CPU's copy. The diagnostic
preparation path does not exercise AP arm or AP instruction fetching; those
remain activation checks.

All private host mappings and every prepared NPT are checked before publication.
The EFI consumer verifies unchanged observed BSP state, performs an allocator
check, exits Boot Services, records the actual final runtime-code descriptors,
and calls GetTime. The runner verifies complete pool coverage in that final map.

## Executed checks

Evidence: `work/native-percpu/summary.json`, with per-build source snapshots,
disassembly/link logs, hashes, and per-run fresh firmware variables/fixture disks.

| Check | Result |
|---|---|
| Core host tests | 342 passed |
| DXE native-returning host tests | 282 passed |
| 2-CPU preparation | 1 AP callback; 2 private copies; full 2 MiB pool retained after EBS |
| 24-CPU preparation | 23 AP callbacks; 24 private copies; full 24 MiB pool retained after EBS |
| 32-CPU preparation | 31 AP callbacks; 32 private copies; full 32 MiB pool retained after EBS |
| 33 CPUs | Refused before pool allocation; observed native CPU state preserved |
| Ordinary activation image on 2 CPUs | Refused; observed native CPU state preserved |
| Ordinary image on 1 CPU | Resident callback ACK/return, allocator, EBS, GetTime and identity virtual map passed |
| Entire test payload | 3,663 linked instructions; no FP/SIMD/xstate changes or undefined symbols |
| Entire production payload | 3,218 instructions; same audit properties; build only |

Final preparation driver SHA256:
`d95a3f79b7526b58c75ebd6a5b533fb0205721e09dcc545033e2b924b44df913`.
Final executed single-CPU driver SHA256:
`cf6e4eb91f1426e35b468459c106b458060dd9968cddfe80d9d2fdbe4a13b33c`.
Production driver SHA256, built but not executed:
`452df46b8d80e4f58aa947341a5ecf6c665d3986ec4b791d7083e1deca92e8fc`.

No physical launch, firmware programming, reboot, Windows/ESP changes, or Windows
protection changes occurred. No multicore guest timing baseline was measured.
The previous single-CPU timing caveat remains: TLB_CONTROL requests a flush on
every entry. These fixture results do not establish actual-machine admission.

## PI 1.10 and the remaining startup boundary

The user supplied `C:/Users/mato/Documents/svmvisor/docs/UEFI_PI_Spec_1_10.pdf`:
Release 1.10, July 2026, SHA256
`ed35ab171e8aa66514e2f04013faf7912098b960bdf614f973a8b9d4a5ff09ea`.
The actual MP chapter was extracted and relevant pages visually inspected.
The detailed section/page map is `work/native-percpu/pi-1.10-review.md`.

II-13.4.5/Table 13.5 confirms blocking StartupThisAP completion and timeout
termination; Finished is ignored in blocking mode. II-13.4.8 permits AP-side
WhoAmI. II-13.4.1 explicitly warns that notification order within the EBS event
group is nondeterministic. An EBS notification therefore cannot be used as proof
that firmware has finished its AP transition. The current callbacks finish
before the first EBS attempt and retain no firmware ownership afterward.

The next activation work must own INIT/SIPI startup requests. Firmware or
Windows may issue INIT/SIPI after an AP callback returns; passing those requests
directly can reset a resident AP monitor. AMD APM 2 revision 3.44 sections
15.13.4 and 15.21.8 explain why INIT interception alone is not a complete
solution: intercepted INIT remains pending, and there is no general SIPI VMCB
intercept. Physical AP takeover and guest startup state need distinct owners,
with narrowly scoped xAPIC/x2APIC startup-write routing. A running AP cannot be
relabeled as a never-started AP to reuse the existing cold-start fixture.

After that come actual-platform admission and the Windows-loader/runtime-map
test. Malware analysis and general device emulation remain deferred.
