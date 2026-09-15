# Shared guest I/O interception boundary

Gate 1 implementation, 2026-09-13. Shared emulator VMCB initialization now
enables `InstructionIntercept::Ioio` alongside its host-only deny-all IOPM.
Installing the bitmap alone did not enforce this boundary. The AMD CPU fixture
now inherits the shared enable; its redundant local enable was removed.

## Architectural exit policy

AMD APM2 revision 3.44 sections 15.7 and 15.10 define the boundary. IOIO_PROT at
VMCB 00Ch bit 27 enables checking every byte-port bit covered by the operand.
The 12 KiB aligned, contiguous IOPM contains 64K+3 meaningful bits. Its three
overrun bits remain set; a permission span past FFFFh does not wrap to port 0.
Existing NPT ownership excludes the maps from guest backing.

Exit 7Bh occurs before instruction commitment. VMCB RIP/RAX/RSP/RFLAGS and
the assembly-owned fourteen-GPR frame are authoritative stopped state.
EXITINFO1 supplies port, width, direction, string and REP metadata. EXITINFO2
reports the following instruction's RIP; it never authorizes skipping I/O.
Privilege/TSS I/O-permission faults precede this intercept under section 15.10.2;
the fixture exercises admitted CPL0 long-mode accesses, not those faults.

The existing `svm/exit.rs` owner now selects `IoioRefused`: unknown scalar
port, string/REP, port-span overrun, or malformed defined reserved bits/width.
All ports remain terminal. Raw upper/address-size/segment fields remain
diagnostic; they never authorize memory access. There is no continuation
candidate, port handler, guest-memory read, allocation, firmware call or port
operation in the core handler. No speculative device abstraction was added.

The existing dispatcher preserves every VMCB byte and retained GPR, including
EVENTINJ, EXITINTINFO and virtual-interrupt controls. It does not consume pending
requests, repair interrupted delivery, fabricate a fault or report instruction
success. A stale snapshot remains an unchanged error. Resumption needs a
separately implemented device/instruction/event owner; this gate supplies none.

## Executed evidence

The `-IoIntercept` image reuses the memory builder, shared execution setup,
guest-state validator, xstate/clock bridge and pending-interrupt owner.
`run-io.ps1` admits only the pinned corrected QEMU executable, versioned
`pc-i440fx-10.1`, one TCG CPU, 64 MiB and no network. Its witness is a disposable
emulated UART with a null character backend. Only scratch register offset 7,
port 3FFh, is used for the endpoint control; no physical UART is accessed.

Each fresh process proves the scratch register responds to two host writes.
The guest then executes allowed byte OUT and IN with IOIO enabled and only
this bit cleared. Both reach a following R15 marker, write a second marker in
owned RAM and exit on VMMCALL. OUT changes the scratch byte; IN changes AL.
These establish endpoint liveness and guest completion witnesses.

With the unmodified shared deny-all IOPM, OUT leaves the scratch byte unchanged,
IN preserves the complete original RAX, and neither instruction reaches either
marker. RIP remains at the I/O instruction; flags, RSP and all fourteen GPRs
remain unchanged. Logs retain RIP/RAX/RFLAGS, RCX/RDX/RSI/RDI/RSP, R15,
metadata and endpoint before/after values. A real armed virtual IRQ remains
blocked by IF=0 and is observed still armed after every entry. Unit tests also
byte-compare synthetic EVENTINJ/EXITINTINFO combinations; those tests are not
evidence of actual interrupted-delivery execution.

Each positive run executes 592 cases over eight rounds: 16 allowed controls and
576 refused accesses. Cases cover byte/word/dword IN/OUT, DX and immediate
forms, unknown ports, port 0, bitmap-byte/page boundaries and the upper port
boundary. Partial-permission cases clear leading operand bits so only a later
bit forces interception, including spans into the architectural overrun region. INS/OUTS
and REP forms stop with RCX/index registers unchanged. This proves first-access
refusal, not partial-iteration recovery or string emulation.

Two negative controls prevent vacuous success. Removing the UART must produce
the exact failed-liveness witness before guest entry. The separately named
`-IoInterceptBypass` image deliberately disables IOIO for the first otherwise
trapped OUT: it completes, changes the scratch byte, reaches the guest markers
and trips the side-effect assertion. The accepted image has no such bypass.

## Validation, timing and limits

Actual final counts, source/artifact hashes and raw evidence locations are in
[final-validation/summary.json](../work/io-intercept/final-validation/summary.json):
core/DXE tests, UEFI target check, 32 positive I/O runs plus two expected negative
controls, and the AMD CPU, concurrent and legacy regression matrices.
Development runs remain separate historical evidence.

`audit-io.py` re-parses every case/metric and rejects missing, duplicate or
malformed records. It verifies stopped state, pending IRQ, side effects,
profiles and the pinned backend's exact metadata. Corrupted-record controls
exercise the auditor. The batch audit binds frozen image, source and raw
trace/record hashes and checks flat image bytes against linked ELF load bytes.
Hashes establish consistency, not producer authenticity or physical execution.

Serialized host CPUID/RDTSC/CPUID samples bracket the existing entry bridge
and, separately, terminal dispatch. Reports retain raw minimum/median/p95/max
TSC spans. Entry spans include bridge, clock/xstate restoration and checks;
service spans include sampling overhead. Telemetry emits after measured
intervals with a fixed case/field budget; the audit requires zero missing
records. These are TCG measurements. Native baseline and observation-on/off
comparisons, calibrated latency and timing invisibility remain unmeasured.

Pinned QEMU reports width/direction/port/STR/REP but omits address-size and
segment metadata even for string operations. Logs record those zeros; scalar
terminal refusal does not need those fields. String effective-address/segment
conformance remains unvalidated. The backend was not modified or substituted.

This closes the shared emulator interception gate, not a general device model,
storage/network/DMA containment, native activation, Windows boot or malware
analysis readiness. Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility was
not tested. No hardware was programmed and no malware ran. Physical evidence
remains only the older exact 65-entry returning probe. The next gate is two-CPU
UEFI ownership; full continuation, memory reservation publication and a
coherent OS platform follow it.

## References and review

- Current [AMD APM2 publication 24593 rev 3.44, March 2026](https://docs.amd.com/v/u/en-US/24593_3.44_APM_Vol2),
  sections 15.2.5,15.7–15.7.2,15.10.1–15.10.3, Appendix B and Appendix C.
  Local SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
  Applies to ordinary AMD SVM, not an implementation of SNP exit protocols.
- Corrected QEMU executable SHA256
  `c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`.
  Read-only translator and UART scratch sources are pinned in the
  [independent review](../work/io-intercept/independent-review.md).
