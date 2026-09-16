# QEMU TCG NRIP-save validation backend

This patch adds SVM NextRIP support to the locally patched QEMU 10.1.0 validation
backend. It is a testing aid for native hypervisor instruction completion.
It does not establish native Windows boot, physical NRIP behavior, timing,
protection compatibility, interrupted-event recovery, or sandbox readiness.

## Source and build provenance

The upstream revision is `f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3`.
The required predecessor also contains the retained `qemu-init-sx` changes;
stock upstream alone is not the patch baseline. `provenance.json` pins each
changed file before and after this patch and the predecessor executable.
`nrip-save.patch` applies to that predecessor. The full archived source and
toolchain were copied into `work/qemu-nrip/source` and `work/qemu-nrip/toolchain`.
The older checkout and predecessor backend remain unchanged.

The prepared full source baseline is recorded in
`work/qemu-nrip/predecessor-source-manifest.json`. Build from the repository root:

```powershell
python tools/qemu-nrip/build.py --output work/qemu-nrip/build-new
```

The output directory must be new. The helper records the complete source tree,
archives changed source files, configures and compiles QEMU, verifies the source
did not change during compilation, and packages the fresh executable with the
pinned predecessor runtime's DLLs and firmware. `build-manifest.json` records
binary, source, tools and runtime hashes. The fallback Ninja graph only handles
the verified optional Windows symlink privilege failure; other configure
failures stop the build.

## Implementation contract

- TCG instruction metadata records the final decoded instruction length and an
  INT3/INTO/BOUND exception-origin tag. Length includes prefixes and trailing
  immediate/displacement bytes and remains relative for relocated blocks.
- The central VMEXIT owner restores guest RIP and publishes sequential NextRIP
  for implemented instruction intercepts, including CR/DR, MSR and IOIO. A
  control-transfer exit records sequential RIP, not its branch destination.
- 16-bit and 32-bit code use their instruction-offset widths; 64-bit code retains
  64 bits. CS.base is not included in the saved instruction offset.
- Special exception exits require the decoded INT3/INTO/BOUND origin, rather
  than assuming the exception vector identifies an instruction.
- Other exits, unavailable unwind metadata, and disabled NRIPS write zero,
  preventing a prior instruction's NextRIP from surviving an asynchronous exit.
- NRIPS is exposed through the existing TCG feature mask. Software EVENTINJ
  consumes supplied NextRIP with NRIPS enabled. Injected BP/OF perform software
  IDT privilege checks; feature-off delivery retains a current-RIP fallback.

No persistent CPU migration field was added. A compile-time check fixes the
VMCB NextRIP offset at `0xc8`.

## Verification and limits

Run the compiled source-extraction checks with a fresh output directory:

```powershell
python tools/qemu-nrip/test_nrip.py --source work/qemu-nrip/source --cc work/qemu-nrip/toolchain/mingw64/bin/gcc.exe --output work/qemu-nrip/tests-new
```

The successful run, `work/qemu-nrip/tests-03/result.json`, passed 3,738
assertions against the actual `svm_next_rip`, `cpu_vmexit` and event-injection C
bodies. Coverage includes 16 instruction classes, three code widths, lengths
1 through 15, wrapping offsets, exception-origin matching, stale clearing,
feature-off behavior, unavailable metadata, and supplied injection return RIP.
CPU state, unwind, physical memory and loop exits are stubbed. This is **not**
proof of the TCG decoder, host-return-address unwind, or IDT/IRET execution.
Real guest fixture results must be attributed to their exact backend binary.
The failed `tests-01` attempt is retained: its harness used automatic CPU state
across `longjmp`; `tests-02` corrected that harness lifetime issue.

### Accepted build and actual execution

The accepted fresh build is `work/qemu-nrip/build-03`, executable SHA256
`317a5ad522613359a56df8cc30dec9928fa665dcf5461181b81f09b3baabae2c`.
Its complete source manifest remained unchanged throughout compilation. The
final portable patch hash is recorded in `provenance.json`; source-extracted
checks were repeated successfully in `tests-03` after the header correction.

Actual two-CPU TCG runs using that binary and the audited resident test driver:

| Run under `work/qemu-nrip/` | Result |
| --- | --- |
| `cpuid-execution` | PASS: `66 67 0F A2` in compatibility32 and `66 67 48 0F A2` in long64, CPL0, on both AP restart generations. |
| `msr-execution` | PASS: prefixed VM_CR RDMSR/WRMSR completion, unprefixed faulting WRMSR with delivered #GP and retry, and all-excluding-self INIT/SIPI. |
| `cache-execution` | PASS: cache/MTRR continuation with NRIPS enabled; HWCR remains a per-CPU modeled fixture backend. |
| `feature-off-baseline` | PASS: ordinary VM_CR/fault/restart workload with NRIPS disabled. |
| `feature-off-cpuid` | Expected negative control: compatibility CPUID stops at exit72/F001/detail2 and times out. The ordinary harness summary remains `passed:false`; no failure was relabeled as a positive fixture pass. |

These runs exercise the real decoder and VMEXIT path for their stated workloads.
They do not exercise every intercept class, CPL3, offset wrap, or software
EVENTINJ return-address case in real TCG execution. Those cases retain the
source-test/static-review evidence and limitations above.

`build-01` was stopped because a review correction changed the source after its
fingerprint. `build-02` reached compilation and exposed a missing explicit
`tcg/insn-start-words.h` include. Neither is an accepted backend. The header was
added before the complete fresh `build-03`; failed logs remain available.

Portable review, build and execution summaries are under
[`docs/handoff-evidence/2026-09-16-compatibility`](../../docs/handoff-evidence/2026-09-16-compatibility/).
The first completed full compile attempt, `build-02`, exposed a missing
`tcg/insn-start-words.h` include that the bounded C harness could not detect.
The include was added and the patch/provenance regenerated before a fresh build;
`tests-03` records the corrected source hashes. Failed build evidence is retained.

Independent source review is recorded in
`work/qemu-nrip/independent-review.md`. It found and corrected the software
EVENTINJ dependency and feature-off BP/OF fallback before the final build input.

Remaining preexisting backend limitation: natural, un-intercepted INT3/INTO
collapse their origin to a boolean before interrupted IDT delivery.
`handle_even_inj` can consequently label their EXITINTINFO as TYPE4 rather than
TYPE3. This patch does not claim complete interrupted-event fidelity. Dynamic
coverage should distinguish INT3 from INT imm3, taken/untaken INTO, BOUND from
other #BR, nonzero CS.base, wrapping offsets, late instruction bytes, software
injection return RIP, and asynchronous exits following instruction exits.

## Normative evidence

AMD APM2 publication 24593, revision 3.44, March 2026, SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
This is an architectural software-model feature; no processor-specific PPR
claim is made. Complete rendered pages, including continuation rows and notes,
were read from the local reference PDF:

| Rule and reference chain | Printed pages | Zero-based PDF indices |
| --- | --- | --- |
| §15.7.1 NextRIP; §15.7.2 event classification; complete Fig15-1/Table15-1 | 508–511 | 569–572 |
| §15.8 and complete Table15-7 instruction intercepts and exceptions | 512–516 | 573–577 |
| §15.12 exception saved-RIP special cases, BP/OF/BR | 519–520 | 580–581 |
| §15.20 software injection NRIPS dependency, BP/OF privilege rule; complete EVENTINJ figure/table | 531–532 | 592–593 |

Rendered images are retained under `work/qemu-nrip/review-pages` and the earlier
CPUID review's `work/boot-review-2-2026-09-16/cpuid-pages`. Extracted text was used
only to locate pages, not as the authority for these rules.
