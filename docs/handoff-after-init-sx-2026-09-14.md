# Continuation after INIT/#SX — 2026-09-14

The user explicitly requested a new task and fresh agents on High reasoning.
Continue implementation toward the first useful Windows boot test. Do not stop
at another roadmap. The user values simple native designs and challenged the
provisional edge-only restriction; the resulting INIT/#SX design is implemented.

## Repository and operating rules

**Authoritative working directory: C:/Users/mato/.codex/worktrees/7a58/svmvisor.**
Use it explicitly for every repository command. A newly created app worktree,
the old task's ee7e cwd, and C:/Users/mato/Documents/svmvisor are not the current
implementation. All agents must use the authoritative directory. The tree is
intentionally very dirty/uncommitted; do not reset, clean, replace, or commit it
as an incidental handoff operation. The previous root has stopped editing.

Read AGENTS.md, README.md, CONTRIBUTING.md, both crate guides and
docs/malware-analysis-direction.md. AGENTS records GPT-6 Astra with High
reasoning for complex agent work. The user expressly authorizes fresh agents;
start them with explicit `model: gpt-6-astra`, `reasoning_effort: high`, and
`fork_turns: none`, supplying complete bounded ownership. Root alone serializes
builds, tests and QEMU execution. Agents may edit separate owned files and review
independently. Do not share build/output directories or overwrite evidence.

Immediate goal: existing Windows machine under native DXE SVM. Malware analysis
features and a general emulated device platform are deferred. User asks how much
remains before reboot: xAPIC/actual-Ryzen startup, normal loader handoff, retained
memory lifetime, then candidate validation/recovery are still substantial gates.
There is no reliable time estimate yet. No physical launch, firmware programming,
reboot, Windows/ESP change, protection disabling or deployment is authorized by
this handoff. Local code, research, builds and disposable emulator tests are.

## Completed checkpoint — do not redo

Read docs/native-init-sx-wakeup.md. Frozen evidence:
`work/native-apic-routing/summary.json`, SHA256
`32bcd6d2e01da5c250ea37712bc08d7265b82877c69f55e1a4e73ed6ad8ed9e1`.
Its final-source snapshot has239 files and a hash manifest. Three extra files
in the prior242-file manifest are historical documentation/finalizer files;
they still exist unchanged. Do not rerun either one-shot finalizer.

- INIT/#SX replaces reserved F1 for host notification in the explicit
  native-resident-guest-startup profile. Ordinary physical IRQ delivery stays
  native: INTR intercept0, V_INTR_MASKING0, host IF0, CR8 preserved.
- Every target sets and verifies per-thread VM_CR.R_INIT before readiness.
  INIT intercept gives exit63h while retaining INIT. Bounded host STGI/NOP/CLGI
  consumes it through private #SX(error1). Gate CLGI first, no GPR changes,
  drop error qword, IRETQ with unchanged fault RIP. No host EOI.
- Removed F1 reservation/source refusals, forced SVR enable and SVR/TPR shadow
  ownership. ICR shadow remains because host notifications write physical ICR.
- Existing bounded shared mailbox FIFO owns commands. Notifications coalesce;
  duplicate running-target SIPI is a wake-only command. Actual guest INIT still
  requires quiescent LAPIC; reset with pending/in-service IRQ stops before commit.
- Directory ABI5; BridgeContext128bytes, offset112 now reserved0.
- Core367 + DXE282 =649 host tests pass. Ten final QEMU scenarios pass:
  2/24/32-CPU repeated startup (110 AP restarts), edge wake, RTC/IOAPIC level
  wake, reset refusal with held ISR, ordinary single and SMP, one-CPU and
  x2APIC-off admission refusal. Test logs and all failed attempts are preserved.
- IRQ fixture checks F1 pending with IF0/TPR F0, F1 held in guest ISR, native EOI,
  SVR software-disabled wake, and a live XMM0 sentinel. Level variant uses RTC
  IRQ8 and IOAPIC pin8; TMR/remote-IRR persist through wake and clear after guest
  device acknowledgment/EOI. No physical latency proof or Windows execution.
- Diagnostic linked audit9,012 instructions, production7,140 (prior7,385),
  no prohibited extended-state instructions or undefined symbols. Production
  built/audited only. Do not equate source size with equivalent scope: our core
  source was about12k lines versus Barevisor4.6k; test/evidence size is separate.

Key production owners: crates/hypervisor/src/host/resident/runtime.rs,
crates/dxe/src/native/resident/runtime.S, host/resident.rs, svm/ipi.rs,
svm/vmcb.rs. tools/native-resident/fixture/src/startup.rs and startup.S own the
guest witnesses; run.py parses exact markers and expands implied feature names.

## Corrected backend and commands

The current emulator is
`work/qemu-init-sx/build-attempt-03/runtime/bin/qemu-system-x86_64.exe`, SHA256
`677158d2f10933bfc8770e3741a3c6ebf33466d1f7f71fee87e6aec3e009b240`.
OVMF remains SHA256
`33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a`.

This is a full rebuild of a separately derived QEMU10.1.0 tree. Nine source
files add target INIT dispatch instead of generic RESET alias, VM_CR, GIF/SMM
gating, #SX error-code/contributory behavior and correct #MC-before-INIT priority.
2,059 actual-body C checks pass in work/qemu-init-sx/tests-final; IDT/CPU effects
are stubbed there, with real execution supplied by the native fixtures.
Full source/toolchain/runtime hashes and changed source bytes are archived.
The materialized QEMU tree has no .git: git67cd056 is the surrounding svmvisor
repository, NOT the QEMU revision. Do not mutate frozen backends for new work.

Use fresh output paths. Example commands from authoritative root:

    cargo test --locked -p svmvisor-hypervisor
    cargo test --locked -p svmvisor-dxe --features native-returning
    python tools/native-resident/build.py --output work/NEW/build --test-output --guest-startup
    python tools/native-resident/run.py --output work/NEW/run --driver work/NEW/build/driver.efi --features guest-irq-level,virtual-map --cpus 2 --init-sx-backend work/qemu-init-sx/build-attempt-03 --timeout 60

`--features native-preflight` alone cannot compile the current full DXE test
suite because a transition integration test needs native-returning exports.
Use the tested command above. The unused build-final-smp directory is a duplicate
single profile; actual SMP evidence uses build-final-smp-activate.

## Next work and suggested independent ownership

First inspect current gaps and pick the smallest complete native boot batch.
Useful independent agent assignments:

1. xAPIC startup/mode ownership and integration. A bounded MMIO decoder draft,
   nine unexecuted tests and applicable patch are archived under
   work/native-apic-routing/drafts/native-xapic-decoder. Its prior agent restored
   production files byte-for-byte; it has no production caller. Review/adapt
   rather than recreate or blindly apply. Native LAPIC mode following is the
   candidate design; forcing physical x2 while exposing xAPIC requires real
   LDR/DFR/lowest-priority translation, not just register-offset conversion.
2. Actual Ryzen extended-LAPIC guest reset contract using the exact PPR.
   Current apic_quiescent() requires nonextended max-LVT5/6 LVTs, incompatible
   with target extended state. Separate wake-only support from actual reset.
3. Independent review of loader handoff and retained memory lifetime while root
   integrates selected changes. Avoid turning future sandbox requirements into
   first-boot prerequisites. Do not create empty abstractions without callers.

The current explicit consumer calls the resident entry after successful EBS;
normal Windows handoff is not integrated. Low bootstrap LoaderCode pages and
original firmware page tables remain retained by the disposable fixture until
reset. Actual loader lifetime and nonidentity runtime mapping need ownership.
Self/logical/broadcast startup, CPU offline/rebind, arbitrary pending-device reset,
and external INIT attribution remain unsupported. SMM masks INIT; host nonmaskable
events are terminal. Repeated arrivals before the first #SX handler CLGI are not
exhaustively validated. Do not claim general Windows interrupt compatibility.

## References and useful prior records

Actual target: Ryzen9 9900X,12 cores/24 threads, Gigabyte B850 AORUS ELITE WIFI7.
Use C:/Users/mato/Documents/svmvisor/docs as reference library:

- 24593_3.44_APM_Vol2.pdf, March2026, SHA256
  3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c.
- 57896-3.00_PPR.pdf, Family1Ah Model44h B0, SHA256
  643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5.
- UEFI_PI_Spec_1_10.pdf, SHA256
  ed35ab171e8aa66514e2f04013faf7912098b960bdf614f973a8b9d4a5ff09ea.

Text extracts: work/qemu-corrections/amd-apm-vol2.txt and
work/native-cache-ppr.txt. PI section map: work/native-percpu/pi-1.10-review.md.
Research: work/native-apic-routing/init-sx-research.md and barevisor-review.md.
Barevisor checkout is pinned fdb4dc2fa9051c41ff8000ce662a3530fd8f3cbe under
work/native-apic-routing/reference/barevisor-checkout. It informs design, not
normative behavior; keep our queued ordering rather than its duplicate-SIPI timing
assumption. Do not copy unrelated introspection or stealth features into scope.

Other context: docs/native-guest-startup.md, native-startup-activation.md,
native-platform-evidence-review.md, native-canary-admission-review.md,
native-resident-activation.md and os-boot-readiness.md. Earlier evidence is
historical and superseded where the INIT/#SX report says so. No physical proof
transfers to a rebuilt image.
