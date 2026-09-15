# SVR, timer LVT and supplied-source scheduling — 2026-09-13

Implemented in the authoritative dirty checkout
`C:\Users\mato\.codex\worktrees\7a58\svmvisor`, preserving prior source and
evidence. Three fresh GPT-6 Astra High agents implemented the core and harness
and independently reviewed the batch; root serialized builds, regressions and
archival. The user's reference library at
`C:\Users\mato\Documents\svmvisor\docs` was checked directly, including its
processor-specific PPR. `AGENTS.md` now records that reference location.

## Supported programming surface

`LocalApic` remains the sole interrupt-state owner and now owns SVR and a
concrete timer containing LVT, initial/current count, divisor and source phase.
The old `OneShot`, `configure_timer` and `mask_timer` paths were removed and
their callers migrated. `admit_enabled()` replaces ambiguous `new`/`Default`:
it explicitly admits SVR `0x1ff`, a masked timer LVT `0x10000`, zero counts and
divider zero (divide by two). This is fixture admission, not RESET/INIT.

| Register | xAPIC offset / x2APIC MSR | Implemented behavior |
| --- | --- | --- |
| Version | 030h / 803h | RO `0x10`: one functional LVT, no extended APIC space |
| SVR | 0F0h / 80Fh | VEC, software enable and stored FCC; operand mask `0x3ff` |
| Timer LVT | 320h / 832h | Fixed delivery, vector, mask, one-shot/periodic mode |
| Initial count | 380h / 838h | Full DWORD; reload/start or zero to cancel |
| Current count | 390h / 839h | RO countdown |
| Divider | 3E0h / 83Eh | All eight encodings: 2,4,8,16,32,64,128,1 |

The version describes this explicitly admitted **timer-only** LVT inventory.
Other LVTs, extended APIC registers and capabilities remain unsupported; no
physical AMD model is claimed. Generic CPUID APIC/x2APIC bits stay clear.
The shared MSR/MMIO operations retain fixed BSP identity, TPR/PPR/IRR/ISR/EOI,
CR8 synchronization and existing instruction/mapping provenance validation.
No host APIC is mapped or accessed, and MMIO never manufactures MSR exits.

Clearing SVR.ASE retains IRR/ISR, blocks new fixed inputs and arming, and forces
the timer LVT mask. Attempts to unmask while disabled remain masked. Enabling
does not restore the previous mask. Countdown continues under the forced mask,
so masked expirations create no IRR; existing IRR remains. EOI can acknowledge
retained ISR. FCC is stored, but lowest-priority arbitration and spurious
interrupt generation are not implemented.

The supplied-source timer advances by bounded arithmetic, with no loop per
tick or expiration. Divider phase carries between calls. One-shot stops at
zero; periodic mode reloads and collapses repeated expirations into the
existing pending bit. Masked expirations are lost. The outcome reports
counting/stopped/masked/newly queued/already pending, not an expiration count.
Initial-count writes reload/cancel and reset the synthetic source phase.

Changing a running timer's vector, mode or divisor is an unchanged policy
refusal; mask-only writes and same-divisor writes are supported. Cancel first
before reprogramming. APIC_BASE Disabled refuses supplied ticks and freezes
the retained timer as an explicit fixture policy. It is distinct from ASE
disable and is not a physical clock behavior claim.

All new writes check ownership, pending events, armed state and continuation
before mutation. MSR upper-DWORD/reserved operands and RO-register writes
require the existing checked #GP(0) path. Nonzero timer delivery status,
unsupported effective vectors below 32, and active reprogramming are policy
stops. Masked low vectors can be stored; vectors 16..31 are architecturally
valid but outside this interrupt fixture's admitted range. MMIO invalid
operations remain unchanged policy stops, not invented #GP/#PF/ESR behavior.
Reads zero-extend their bus-defined results; successful writes preserve GPRs
and flags, and only successful completion advances RIP.

## Final validation

| Check | Result |
| --- | --- |
| Core tests | **207 passed**, previous 197 |
| Returning-DXE tests | **251 passed** |
| Hypervisor UEFI target check | Passed |
| Final emulator matrix | **35 profiles passed** |
| Evidence parser negative controls | **38 passed** |
| Independent review | No unresolved blocking finding in the bounded slice |

The matrix contains three strict AVX/SSE/FXSAVE profiles, four ownership,
seven synthetic fault, ten extended-state, ten relocation profiles and one
RDTSCP-disabled profile. Intentional terminal profiles stop earlier and do
not all execute the APIC suite. Shared builds ran serially.

Every completed new suite runs 16 sessions starting on each bus, 32 total.
The guest disables SVR inside a real interrupt handler with ISR51 and IRR50
retained, verifies forced masks and no mask restoration, checks masked timer
expiration, programs both timer modes and divider, disables/re-enables
APIC_BASE, and reads retained current count/LVT/SVR through MMIO before the
optional return to x2APIC. Six real interrupts are consumed, acknowledged and
returned from through IRETQ in each session.

Final per-session accounting is 62 entries/51 APIC accesses for xAPIC and
63 entries/52 accesses for x2APIC, with ten query checkpoints, six interrupt
consumptions, 38 supplied ticks and six refused ticks. Across 32 sessions this
is **2,000 entries, 1,648 APIC accesses and 192 interrupt/EOI/IRETQ deliveries**.
These are semantic execution counts, not physical timing measurements. The
complete linked guest is 3,802 bytes, within the existing 4,096-byte code page.

Seven additional MSR cases run 16 checked #GP repair/IRETQ retries each:
reserved/high SVR, reserved LVT/divider, version/current writes and high initial
count. Their **112 new retries**, added to 160 historical plus 16 ID retries,
give **288 MSR #GP retries** per completed suite. The earlier 32 intercepted
and 32 direct CR8 #GP repairs remain separate and passing.

Thirteen new actual-exit refusals cover nine MMIO and four MSR cases:
reserved operands/RO writes on MMIO, active divider/vector/mode changes,
and armed software disable. Full stopped VMCB/frame and APIC state remain
unchanged. The earlier 13 MMIO refusals are retained; their unsupported-version
read became an unsupported-APR read because version is now admitted.

Ten new unit tests cover retained ISR/IRR and disabled gating, all eight
divisors, fractional ticks, exact periodic reload/coalescence, maximum u64 ticks
with nonzero phase, maximum counts, cancellation, active reprogramming,
mask/low-vector rules, transactional writes and bus-specific errors. Six new
required markers cover the guest suites. The shared parser rejects removed,
duplicated or malformed recent markers and contradictory historical gap
claims: 38 negative controls passed.

Runtime SHA256 remains
`581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`, at
`work/qemu-cr8-fault/build-validation/runtime/bin/qemu-system-x86_64.exe`.
Strict runs use `-GuestCr8Unblock -CorrectedBackend -RequireCr8Faults`.
The final strict image SHA256 is
`fbd6c8726870e255b5ba98094dc3b5c5e690feea18d057dfeb2413b5c2a2ca43`.
The prior corrected runtime still reports its known CR8 operand gaps and
fails strict mode. Neither runtime was changed in this batch.

Final evidence is indexed by
`work/apic-svr/final-validation/regressions.json`. The earlier `validation`
directory is explicitly superseded: a final cross-bus readback addition landed
after its matrix began. The complete matrix was rerun on frozen final source;
the earlier 59/60-entry evidence was not relabeled as final proof.

The archive at
`C:\Users\mato\Documents\Codex\2026-09-10\svmvisor-bios-f7-analysis\outputs\apic-svr-timer-2026-09-13`
retains source, reference findings, independent review, runtime, tested
artifacts, logs and a SHA256 manifest. Its source delta compares against the
previous MMIO archive, preserving the pre-existing dirty implementation.

## Reference findings and remaining work

The user's AMD APM2 revision 3.44 is byte-identical to the retained manual.
Sections 16.3.4, 16.4.1/Figure16-8/Table16-3, 16.4.7 and 16.11 define the
admitted register/enable/timer behavior. Supplemental APM3 revision3.37 remains
in the worktree. Exact paths and hashes are in `work/apic-svr/reference-index.json`.

Direct review of the user's additional manuals mattered:

- `48882-3.11.pdf` is the IOMMU specification, not a BKDG or local timer-clock
  specification.
- `57896-3.00_PPR.pdf` describes Family1Ah Model44h RevisionB0. Its p43 gives
  that model's timer rate as 2xCLKIN, invariant across P/C states, with updates
  in units 1..8. Its p55 describes divided-clock counting and masked reload.
  A physical clock description exists; mapping it to this synthetic source,
  actual platform CLKIN/calibration and measured latency remain unimplemented.
- PPR p54/57 resolves FCC polarity for that model. Its MMIO timer page62
  exposes MsgType10:8, while MSR832 p180 reserves those bits. The adapters keep
  those buses distinct: nonzero MMIO message type policy-stops; MSR reserved
  bits require #GP. PPR version p56 also exposes physical capabilities absent
  from this fixture's `0x10`; it is not a value to copy into the guest.

Relevant PPR sections do not specify live-divider prescaler phase or active
vector/mode races, so those writes remain refused. Timer reset phase is an
explicit deterministic policy. No TSC-deadline mode, real-clock host timer
callback, HLT wakeup scheduler, other LVT source, ESR/arbitration/spurious
generation, RESET/INIT, SMP, IOAPIC/MSI/NMI or general OS decoder was added.

The project remains an early hypervisor foundation exercised by controlled
guests. It does not yet run Windows under a resident hypervisor or provide a
malware-analysis sandbox. Windows/Hyper-V/VBS/HVCI/protected-state compatibility,
physical timer frequency/calibration/latency/drift, device/DMA containment and
telemetry-loss characterization remain unestablished. No hardware activation,
driver installation or Windows protection change occurred. Unknown exits and
policy refusals mean incomplete execution/analysis; historical physical proof
applies only to its exact tested returning image.
