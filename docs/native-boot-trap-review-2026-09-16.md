# Native boot exit review - 2026-09-16

This records the first review batch. The subsequent
[compatibility review](native-boot-compatibility-review-2026-09-16.md) supersedes
its PCI I/O limitation and adds CPUID, HWCR35 and broadcast-startup changes.
Interrupted-event recovery remains unsupported in both batches.

The external review identified two concrete correctness gaps and several real
compatibility limits, but did not establish six inevitable Windows boot failures.
The existing HWCR30 fix remains justified. No new physical boot or flash occurred
in this batch. The card still holds candidate
`bfb3f06466d24a24a53e121a594ec7a3`; the changes below are built locally only.

## Findings

| Claim | Result |
| --- | --- |
| 254 MSR indices intercepted without general completion | Confirmed compatibility limit, not 254 implemented Windows-required registers. The range includes reserved indices and real monitor/SMM/SVM controls. Neither blanket physical passthrough nor blanket #GP is justified. |
| Cache owner rejects all changed MTRRs | Incorrect: fixed/variable replay and default E0/E1 transitions exist. New final maps and some RW SYS_CFG controls remain unsupported because they can invalidate resident memory typing/routing. |
| Kernel ECAM writes require NPF.US=0 | Incorrect for this unencrypted NPT profile. APM15.25.5 explicitly makes the final guest-page access user at the nested level, regardless of guest CPL. Keep the existing check. |
| 40-bit guest aperture | Confirmed limit, but current Windows lists 36 device memory resources, all below1TiB. Above4GiB does not imply above1TiB. Later reassignment or unreported high apertures remain unsupported. |
| Interrupted event delivery | Confirmed missing recovery. Some paths already refused; ECAM retry could resume without recovering the interrupted event. Added a common refusal preserving the raw event, before dispatch can mutate or resume guest state. Reinjection is still unsupported. |
| Silent terminal returns | Confirmed. Added explicit reasons and an outer fallback so a refused dispatch cannot silently bypass stop publication. Barrier participation still requires armed/ready state and an available endpoint. This is not proof of the historical incomplete barrier's cause. |
| CR0/SVM/SHUTDOWN/PCI I/O | CR0 guard intentionally bounds cache replay. Eight SVM-related instruction intercepts are real (seven misc2 bits plus INVLPGA); nested SVM remains unsupported. SHUTDOWN must terminate. CF8 DWORD spans CF8-CFB, and CFC-CFF byte lanes are supported; arbitrary widths and reset-port access are not. No current boot trace establishes these unsupported operations are needed. |

## Implemented stopped-state behavior

The native runtime now records these reasons through its existing stop owner:

- `F10A`: unexpected bridge-context address (never dereferenced).
- `F10B`: dispatcher reached before arm.
- `F10C`: prior injected fault could not be retired; raw EXITINTINFO retained.
- `F10D`: missing bootstrap acknowledgement owner.
- `F10E`: terminal callback result without a recorded reason.
- `F10F`: interrupted hardware event without native recovery; raw EXITINTINFO retained.
- `F110`: SHUTDOWN or invalid entry; no saved RIP/event interpretation.

The outer fallback runs after route guards unwind and preserves existing reasons.
Peers responding to an already published terminal request do not invent a new
failure. The event guard runs after physical INIT acknowledgement and before
bootstrap/startup or exit handlers. Rejected event preparation preserves every
VMCB byte and the outstanding fault owner. No guest exception is fabricated and
RIP is not advanced. SHUTDOWN and invalid entry terminate without interpreting
their saved event/RIP as valid continuation evidence.

The full reason is retained locally and in diagnostic event3 when publication is
available. The legacy compact terminal format does not separately name these new
reasons. ECAM/configuration writes can permanently revoke card publication;
missing later card evidence is not proof that execution stopped.

## Verification and limits

- Full default hypervisor suite passed.
- Four new release `resident-runtime` tests passed: fault refusal ownership and
  state preservation, hardware-event retry refusal, poisoned shutdown/invalid
  state, and unexplained refusal without overwriting an existing reason.
- Existing focused NPT/admission and cache/map tests passed (15 and16).
- Two-CPU `guest-cache-hwcr` emulator fixture reached expected F400/16 refusal,
  complete terminal barrier and no continuation. This uses modeled HWCR. The
  expected terminal state times out the emulator; the harness result is PASS.
- Final production build audited23,670 instructions, with no FP/SIMD/xstate and
  no undefined symbols. Build evidence and source hashes are retained separately.

The new event tests use synthetic stopped snapshots. They do not prove a physical
interrupted event was delivered, lost or recovered. In particular, the INIT/event
combination test is a guard test, not a demonstrated hardware overlap. General
EVENTINJ recovery requires a separate owner for pending injections, exception
combination, software-interrupt RIP, NMI and external acknowledgement semantics.
The existing bounded exception-combination fixture cannot simply be enabled in
the native profile, whose exception intercept bitmap is zero.

No native timing, Windows boot, Hyper-V/VBS compatibility or malware containment
was measured. The previous physical cache-write stop still lacks runtime MSR
operands; the HWCR attribution remains strong static evidence.

## Evidence

Detailed primary-manual image reviews record hashes, revisions, applicability,
printed pages, PDF indices and cross-reference limits in
[the retained review evidence](handoff-evidence/2026-09-16-boot-review/).
Local build/emulator artifacts remain under `work/boot-review-2026-09-16/`.
