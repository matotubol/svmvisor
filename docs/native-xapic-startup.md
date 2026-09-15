# Native xAPIC startup — 2026-09-14

The explicit native guest-startup profile now follows the enabled physical
xAPIC or x2APIC bus. It owns xAPIC startup through an absent LAPIC NPT page,
checked DWORD MOV emulation, and the existing ICR shadow and mailbox FIFO.
Ordinary register operations execute against the native LAPIC, including
LDR/DFR and interrupt acknowledgment. xAPIC-to-x2APIC promotion changes the
physical bus and guest-visible mode together. It does not translate logical
destinations or emulate a second interrupt controller.

## Exit and ownership contract

APM2 rev3.44 §§5.4, 7.8.5, 15.25.6, 16.3.2, 16.5, 16.9–16.13 and
APM3 rev3.37 MOV definitions govern the new path. The archived decoder at
`work/native-apic-routing/drafts/native-xapic-decoder` was adapted into the
existing `svm/xapic.rs`, `host/resident/fetch.rs` and `svm/exit.rs` owners.

- NPF must describe a missing final-data translation at the admitted fixed
  FEE00000h page. Instruction bytes and the full operand are independently
  walked through current stopped guest paging using the existing WB RAM reader.
- Supported instructions are supervisor long64 DWORD MOV 89/8B/C7 /0 with
  optional REX.W=0, ModRM/SIB, signed displacements and RIP-relative addressing.
  Other widths, prefixes, operations, legacy-mode MMIO and reads into ESP stop
  at the original RIP. Events, debug flags and continuation are checked first.
- Successful reads zero-extend the destination. Writes preserve guest GPRs.
  Completion advances RIP once and consumes RF/interrupt shadow through the
  existing VMCB owner. Unsupported cases leave architectural state uncommitted.
- Native ICR high/low share one shadow and route startup through the existing
  bounded FIFO. Source publication precedes the INIT/#SX notification. The
  destination alone resets its guest state after complete reset preflight.
- Private FD000h LAPIC, FE000h mailbox and FF000h transient RAM aliases are
  distinct. Effective UC requires the validated MTRR/PAT composition, both in
  firmware bootstrap mappings and the private host alias. Guest PAT WC refuses.
- The shared DXE NPT builder applies the LAPIC hole at every preparation,
  including final callback capture. Two additional tables split one GiB and
  one 2MiB leaf; the eight-page budget still holds. Neighboring pages retain
  identity translation and the complete monitor pool remains excluded.
- The startup profile preserves the initial enabled native bus. Physical AP
  bootstrap allows unchanged mode or verified xAPIC-to-x2 promotion. ICR writes
  are boundedly checked idle before and after delivery. Hardware disable,
  relocation and demotion remain unsupported; illegal direct x2-to-xAPIC
  prepares #GP using the existing MSR fault owner.

No persistent host allocation, firmware call or floating-point/extended-state
operation was added. All linked code receives the existing instruction audit.
The decoder is bounded to fifteen instruction bytes and four-level walks;
there is no physical latency measurement or calibrated ICR timeout claim.

## Ryzen reset and routing

[The reset contract](native-ryzen-apic-reset.md) uses exact PPR57896 rev3.00
for Family1Ah Model44h B0. It includes six standard and four extended LVTs,
extended controls and interrupt enables, with no host EOI or ICR reset send.
Actual guest INIT requires already software-disabled, masked, idle native
interrupt sources and empty interrupt state. Wake-only commands bypass reset.

PPR extended xAPIC resets its full-ID control to zero, narrowing physical
destination matching to four bits. Any observed extended xAPIC CPU therefore
requires the complete assigned topology to have unique IDs below15 before the
first physical INIT/SIPI. This gate applies even when the BSP is already x2APIC.
Runtime admission, guest control writes and guest reset also prevent unsafe
narrowing. The actual 24-thread machine is **not admitted for this physical
bootstrap when an AP is in extended xAPIC mode**. Its APIC version and initial
per-thread modes have not been physically observed in this batch. x2APIC mode
avoids that particular narrowing but does not establish overall boot readiness.

## Evidence and limits

Fresh builds, audits, test logs, emulator traces, commands and SHA256 hashes
are retained under `work/native-xapic-2026-09-14`; `summary.json` identifies
the final set. The INIT/#SX backend and firmware remain pinned to the handoff
hashes. No frozen backend, earlier evidence or one-shot finalizer was changed.

Final validation passes **399 core + 284 DXE = 683 Rust tests** and **11 emulator
scenarios**: xAPIC startup at 2/24/32 CPUs, xAPIC-source level IRQs, native
xAPIC-to-x2 promotion with level IRQs, x2APIC level-IRQ regression, held-IRQ
reset refusal, ordinary single/SMP, and one-CPU/x2APIC-off admission refusal.
The startup runs execute 118 AP restarts. The held-IRQ case disables SVR while
ISR remains set and stops before reset with native PPR=F0h; no host EOI occurs.
Diagnostic linked audit covers 12,109 instructions; production covers 9,831.
Both contain zero prohibited extended-state instructions and undefined symbols.
Production is built/audited only. These counts exclude the unchanged prior
backend's 2,059 checks, which were not rerun.

An initial candidate failed because final callback preparation rebuilt an
ordinary identity NPT after the earlier LAPIC trap. QMP snapshots showed both
CPUs still in guest loops, and diagnostic instrumentation showed no MMIO exits.
The shared builder now carries the startup policy explicitly. A host regression
tests repeated rebuilds; the executed fixture separately requires exactly four
MMIO exits for its four LDR/DFR accesses, preventing native readback alone from
being mistaken for intercepted execution. Failed runs and diagnostic candidates
remain archived. An earlier fixture RDMSR readback in xAPIC mode was corrected
to use the native ICR bus.

Normal Windows-loader handoff, owned AP bootstrap paging and nonidentity
runtime continuation remain open; see the [lifetime review](native-loader-lifetime-review-2026-09-14.md).
Self/logical/broadcast guest startup, CPU rebind, arbitrary pending-device reset,
external INIT attribution, general host nonmaskable-event recovery and guest
cache-control changes are outside this batch's established support. Hyper-V,
VBS, HVCI, PatchGuard and Secure Boot compatibility remain untested. No physical
launch, reboot, firmware programming, Windows/ESP change or protection disabling
was performed. Emulator success is neither Windows execution nor containment.
