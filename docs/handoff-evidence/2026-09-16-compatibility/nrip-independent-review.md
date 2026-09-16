# Independent NRIP backend review - 2026-09-16

Read-only comparison of main work/qemu-nrip/source against archived
C:/Users/mato/.codex/worktrees/7a58/svmvisor/work/qemu-init-sx/source.
No implementation edits or hardware operations performed by this reviewer.

Reviewed patch checkpoint SHA256:
1b29cfac9134ce99ea936ac3927d00ae3ec4d67913e15953046837ea23f9d6e3.
All ten before/after source hashes and portable patch hash independently match
tools/qemu-nrip/provenance.json. Baseline is locally patched QEMU10.1.0,
upstream revision f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3 plus retained predecessor
patches; it is not stock upstream. CPU header cpu.h itself remains unchanged.

## Findings and resolution

1. Initial patch exposed NRIPS but software EVENTINJ still pushed env->eip.
   Reported to implementer; corrected to consume supplied VMCB.NextRIP for
   software injection, and TYPE3 BP/OF retains its required DPL behavior.
2. First injection follow-up gave TYPE3 BP/OF next_eip=-1 with NRIPS disabled,
   while marking software delivery; delivery would push all-ones return RIP.
   Reported immediately; final reviewed checkpoint uses current-RIP fallback.

Final build follow-up: the sole patch delta from reviewed patch6c8792f3 is
`#include "tcg/insn-start-words.h"` in system/svm_helper.c, correcting the full
build missing-declaration failure. Independently verified by removing that line
in memory and regenerating the predecessor-normalized portable diff: it exactly
matches prior patch SHA2566c8792f3. All ten before/after current file hashes and
new patch hash match provenance. Implementer reports tests-03 passes3738
source-extracted checks; reviewer did not rerun that harness. Full build and
execution remain root-owned.

No remaining new blocking source defect found at this checkpoint. Build and
execution validation are separate and were not complete when this note was
written. This note is not a claim that all NRIPS semantics are exercised.

## Static checks

- Extra instruction-start metadata stores final decoded byte length only after
  instruction decoding, including prefixes/immediates/displacements. It remains
  relative, preserving PC-relative translated block relocation. Abandoned
  cross-page translation removes its unfinished marker and returns before write.
- INT3, taken INTO and BOUND carry an explicit exception-origin tag. INT imm8
  stays SWINT; unrelated exception vectors do not inherit origin by number alone.
- cpu_vmexit reads metadata with cpu_unwind_state_data before the existing
  cpu_restore_state; zero or unresolvable return addresses cannot use stale data.
  It then adds validated length to restored CS-relative EIP. 16/32-bit code
  truncates sequential offset by CS mode; long64 retains64 bits. It does not add
  CS.base or a control-transfer target to nRIP.
- The allowlist includes implemented CR/DR and instruction/MSR/IOIO intercepts;
  exceptions require origin match. Asynchronous exits, NPF, invalid entry and
  other unsupported classes zero nRIP. Every cpu_vmexit writes the field, so
  earlier intercepted-instruction nRIP cannot leak into a later async exit.
- CPUID bit is added to TCG's supported SVM mask, leaving model/feature selection
  to existing machinery. No migration-only env field is added. VMCB offsetC8
  is compile-time checked. Disabled NRIPS writes zero on exit.
- Non-intercepted INT3/INTO preserve software IDT DPL handling and following RIP;
  their exception interception now occurs before IDT delivery, as required.
  INT imm8 still checks SWINT only. New helper signatures and all callers agree.
- Software EVENTINJ consumes caller-supplied NextRIP with feature enabled;
  ordinary exception/interrupt fields retain existing branches.

## Rendered normative review

AMD APM2 publication24593 revision3.44 March2026; SHA256
3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c.
Images rendered directly from local PDF in work/qemu-nrip/review-pages and read.
All PDF indices below are zero-based; printed page numbers are distinct:
- Printed508-511/PDF569-572: §15.7.1 nRIP requirements, sequential control-transfer
  rule, §15.7.2 INT3/INTO exception-vs-INTn classification, full Fig15-1/table15-1.
- Printed512-516/PDF573-577: §15.8 and complete Table15-7 instruction intercepts,
  including continuation rows, priorities and notes.
- Printed519-520/PDF580-581: §15.12 exception saved-RIP special cases, BP/OF/BR.
- Printed531-532/PDF592-593: §15.20 software injection NRIPS dependency, BP/OF
  DPL rule, full EVENTINJ figure/table and invalid-event restrictions.
No conclusions are derived only from extracted text. The processor-specific
PPR is not needed for this architectural software-model feature.

## Remaining evidence boundaries

- Implementer is adding source-extracted compiled tests; root runs real QEMU
  fixture tests after build. Source-extracted tests alone cannot prove translator
  metadata placement, host-return-address unwinding or actual IDT/IRET behavior.
- Important dynamic cases: prefixed instructions and late immediates; nonzero
  CS.base in16/32-bit code; sequential wrap; INT3 versus INT imm3; INTO taken/not;
  BOUND versus another#BR; asynchronous/NPF exits after nonzero nRIP; feature-off
  injection; supplied NextRIP differing from current RIP.
- Preexisting backend limitation: un-intercepted natural INT3/INTO still collapse
  env.exception_is_int to boolean. handle_even_inj consequently identifies them
  as TYPE4 on interrupted IDT delivery instead of architectural TYPE3. This was
  reported to implementer as a remaining limitation, not a new patch regression.
  Do not claim complete interrupted-event fidelity from this NRIP patch.
- This is a validation backend only. No hypervisor event replay, Windows guest
  boot, hardware NRIP behavior, timing, protection compatibility or sandbox
  readiness is established by this source review.
