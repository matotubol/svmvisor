# Post-EBS cache survey execution and linked review

All three final four-CPU runs passed. The native production image built and passed the linked integer audit; it was not executed on hardware.

## Exact evidence

- `execution-review.json` binds every final execution image to its builder image and independently verifies all 288 frozen source files against the current authoritative tree. All four final builds share source-manifest SHA256 `b564b987f56e9b0d991e00f1edab89ceb48c61965955179bc4c943fb3c1587dd`.
- Production: `production-final-build/driver.efi`, SHA256 `87953f4584a32d67cf7fe54e8a2a9ded6ccf14fe0a2f71a672841a1d3167d04c`; built with `--boot --low-runtime`, without test or survey features.
- Final run records: `pass-final-run/summary.json`, `mismatch-final-run/summary.json`, `drift-final-run/summary.json`. Their corresponding `*-final-build` directories retain source snapshots, code and full build/audit logs.
- Pinned corrected QEMU: SHA256 `677158d2f10933bfc8770e3741a3c6ebf33466d1f7f71fee87e6aec3e009b240`; EDK II firmware SHA256 `33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a`; four CPUs, 256 MiB, `max,svm=on,hypervisor=off`.

## Executed outcomes

| Case | Actual execution evidence | Outcome |
| --- | --- | --- |
| Positive | Before driver installation, BSP disabled variable MTRR7 base is zero. A late EBS notification writes and reads back `0x12345006`, without changing its disabled mask. The post-EBS BSP sample contains that new raw value. AP samples arrive in explicit reverse release order 3,2,1; AP1 also has a bounded pause delay. All four samples precede admission; admission precedes every arm/entry. | Four real resident guest acknowledgements; loader root replacement/reclamation, nonidentity runtime mapping, and two guest INIT/SIPI restart generations on each AP all pass. |
| Bank mismatch | Same late BSP evidence and all four sample completions. Modeled CPU1 fixed MTRR7 differs by a valid extended tuple. | BSP rejects agreement, operation6/predicate12/index0x26c, activation result48. No admission, arm, entry marker, or guest acknowledgement. |
| Local arm drift | Same late BSP evidence and complete bank admission. The modeled CPU1 raw variable7 base changes only in its stage1 arm observation. | CPU1 arm returns12; callback propagates refusal; activation result33. No entry marker or guest acknowledgement on any CPU. |

Terminal negative runs are deliberately stopped and are killed by the harness after a 20-second timeout. Their pass condition requires specific refusal evidence and zero entry; timeout alone is not success. AP error characters can interleave the BSP debug failure text, so the harness additionally requires the stable terminal activation result and ordered survey witnesses.

## Production linked review

`linked-survey-audit.py`, `production-dxe-selected-assembly.txt`, and `production-final-build/linked-review-index.json` contain an independent Capstone decode of the actual final PE bytes. A separate `/map` and `/lldmap` relink produced a byte-identical driver (same full SHA256), allowing symbol resolution without trusting a sibling assembly emission.

The checked bytes establish:

- Callback calls the survey at preferred VA `0x1400056d6`; its error branch precedes runtime arm (`0x1400062b7`) and entry (`0x140006304`).
- The AP wait compares its release field to 2 at `0x14000d498`, checks admission before returning, and has a bounded `0x7fffffff` pause loop.
- BSP pass1 calls `finish_cache_survey` at `0x14000dc4d` and branches away on error before activation release stores.
- Owner initialization at `0x14000c7ec` precedes admission publication at `0x14000c8a4`.
- All new sample/park/admit closure calls resolve to linked cache functions, integer memcpy, or bounded panic-stop code. No allocation, firmware-service calls, unresolved indirect calls, recursion, dynamic RSP changes, FP or SIMD occur in these new closures.

The sample/park closure contains 12 linked functions. Summing every fixed frame, even mutually exclusive ones, plus 136 bytes per function for a return and full red-zone allowance gives 2,760 bytes. Add the live callback frame (1,368), another 136-byte allowance, captured boundary maximum (1,656), and 256 bytes for the surrounding integer bridge: **6,176 bytes**, below **126,976 bytes** available above the first page of each 128-KiB AP bootstrap allocation. The loop itself holds a 72-byte frame. BSP finish/admit closure bound is 2,504 bytes, before its caller's existing frame. These are bounds for the newly added paths, not a recertification of every older activation branch.

The production payload's whole linked integer audit checks 23,384 instructions, zero undefined symbols, and no FP/SIMD/xstate instructions. Its default guest debug-reset and all 256 host-fault-vector audits pass. Production ELF, flat payload, and EFI bytes contain no `capture_fixture` symbol or survey fixture markers. The production exact-target capture/admission checks remain present.

## Scope and remaining work

QEMU supplies actual standard MTRR/PAT reads. The fixture explicitly models the AMD target signature, 48-bit profile, per-core topology, hidden fixed RdMem/WrMem attributes and AMD-only routing fields that QEMU cannot supply. Mismatch and arm-drift failures are modeled observation injections. The late BSP EBS value change is an actual MSR write/readback; AP sample/release ordering and guest entry/exit are actual emulated execution.

This proves the exercised survey barrier, freshness, owner-before-entry ordering and local refusal control flow. It does not prove hardware cache coherence, real AMD hidden-field behavior, SMM cooperation, all-CPU F7 synchronization, Windows boot, VBS/Hyper-V compatibility, or malware containment. No Windows payload, physical disks/network, firmware programming, protection changes, or timing baseline were used.

Historical failed/intermediate evidence remains intact: `pass-build` captured the initial fixture feature dependency link failure; `pass-build-02`, `pass-run-01`, `mismatch-build`, and `mismatch-run` are superseded candidate results. Final evidence is explicitly named above.

## Reproduction

Build each fixture using `python tools/native-resident/build.py --output work/NEW --boot --test-output --cache-survey pass` (replace pass with mismatch or drift). Run it with `python tools/native-resident/run.py --output work/NEW-RUN --driver work/NEW/driver.efi --cache-survey pass --cpus 4 --init-sx-backend work/qemu-init-sx/build-attempt-03 --timeout 20`. Build the native candidate separately with `--boot --low-runtime` and no survey/test options.
