# QEMU CR8 reserved-operand fault correction

This batch corrects the local QEMU test backend's missing reserved-operand check
for MOV-to-CR8. Invalid bits must cause #GP(0) before an SVM write intercept and
before any priority change. The previous backend and its evidence remain intact.
This is a backend correction, not additional hypervisor stealth or Windows support.

## Implementation and ownership

A full-width CR8 operand check runs after normal decoding/CPL validation and
before the SVM intercept. The translator uses its existing decoded source loader,
so the check follows the operand width selected by the decoder. Only a CR8 write
destination selects this check; CR8 reads and other control registers are unchanged.
The helper raises #GP with error zero and the existing fault-PC restoration path.
The same check runs when interception is disabled and outside an SVM guest.
The existing CR8 write helper and earlier interrupt/timing corrections remain intact.

The implementation lives in a separate source/build/runtime tree under
`work/qemu-cr8-fault`. It reuses the pinned portable toolchain without installing
system tools. The old `work/qemu-corrections` runtime is retained for negative
controls. Source revision, incremental/cumulative patches, executable and runtime
hashes, build logs and packaging details accompany the evidence archive.
The new executable SHA256 is
`581a847b6ab3e414ba47a6978882571079fc3defa787b12a57c29ec35e18a763`.
The old corrected executable remains
`a67f4cd5b5c5e61954905ff77eb7010b8c726aeddf4ef52c2097361096b62d21`.

Normative basis: AMD APM volume 2 revision 3.44, Table 15-7 and section 16.6.4;
volume 3 revision 3.37 MOV(CRn). Normal CR8 exceptions precede the intercept and
reserved operand bits must fault. The retained AMD document hashes are listed in
the preceding [CR8 synchronization report](cr8-synchronization.md) and its review.

## Guest checks and honest coverage

The existing 32 intercepted invalid-operand cases must first exit for #GP with
error zero and unchanged fault RIP, source operand, saved GPRs, RSP and flags.
The guest handler repairs RAX, then IRETQ retries the same instruction. The next
exit must be the valid CR8 write; the core emulates it through the existing TPR
owner. The final guest stop verifies exactly one handler invocation.

A separate backend fixture disables the CR8 write intercept. Sixteen valid
writes cover priorities 0 through 15; thirty-two invalid writes must fault,
run the same repair/IRETQ handler and complete the repaired hardware write.
It checks V_TPR before the fault and after repair, guest stack and handler count.
No LocalApic object is created in that direct-write fixture, so hardware priority
updates cannot silently stale a controller owner. Every entry checks host CR8
restoration through the existing entry wrapper.

The prior backend reports two distinct gaps: early interception and truncation
without a fault. Its strict-fault run must fail. The new RequireCr8Faults runner
option requires both 32-case fault-success results, valid fixture accounting and
no GAP markers; missing or contradictory markers cannot satisfy it. Older
backend diagnostic runs can still retain incomplete coverage explicitly.

## Review, validation and limits

A fresh GPT-6 Astra/High agent implemented and built the backend, then reviewed
the root's guest tests. Root independently reviewed the three-file patch and
build/runtime provenance. The app agent-thread limit prevented a second fresh
reviewer. This cross-review led to the explicit pre-reflection register/flag
checks. All 35 regression profiles passed, including strict AVX/SSE/FX runs with both
32-case fault suites and no GAP markers. The 186 core tests, 251 returning-DXE
tests and UEFI target check also passed. Ten marker controls and the old-backend
strict-gate rejection passed. Exact results are retained in state.json and logs.

The bounded fixtures exercise 64-bit guest code on TCG. Legacy-mode CR8 encoding,
non-SVM execution and physical CPU behavior are not separately execution-tested;
source placement covers those paths but does not constitute run evidence.
No physical image was activated. This does not establish physical latency/drift,
protected Windows compatibility, undetectability or malware containment.

Next: remaining APIC register/capability/MMIO admission, keeping architectural
faults distinct from unsupported-policy stops. Physical routing, RESET/INIT,
real timer scheduling/HLT wakeup, IOAPIC/MSI, NMI and SMP remain separate work.
