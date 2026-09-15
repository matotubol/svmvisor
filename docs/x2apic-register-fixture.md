# Partial guest x2APIC register fixture

Historical checkpoint: the subsequent [MSR fault and APIC mode batch](apic-modes-msr-faults.md)
implements checked #GP retries and bounded transitions. The results below describe
the earlier fixed-mode image.

Real guest RDMSR/WRMSR accesses now exercise the existing local controller's
TPR, PPR, EOI, IRR and ISR. The profile explicitly admits one BSP already in
x2APIC mode at FEE00000h. It is a partial diagnostic surface: default CPUID
continues to withhold APIC/x2APIC capability, mode transitions are unsupported,
and this does not establish a complete Windows-compatible APIC.

## Register and exit contract

| MSR | Implemented behavior |
| --- | --- |
| 01Bh | Read fixed admitted APIC_BASE FEE00D00h; writes stop as unsupported policy. |
| 808h | Read/write full 8-bit TPR; successful writes atomically synchronize V_TPR class. |
| 80Ah | Read PPR computed from TPR and highest ISR; writes require #GP. |
| 80Bh | Zero write performs non-specific EOI; reads/nonzero writes require #GP. |
| 810h–817h | Read eight 32-bit ISR windows; writes require #GP. |
| 820h–827h | Read eight 32-bit IRR windows; writes require #GP. |

TPR writes with reserved bits set require #GP. A required fault is returned as
`GeneralProtectionRequired` and stops this fixture with no state mutation;
guest #GP injection for emulated MSR faults is not implemented. Other registers
and all APIC_BASE writes stop as unsupported, including architecturally valid
mode transitions. They are not falsely converted into architectural faults.

The adapter handles only actual MSR exits 7Ch with EXITINFO1 exactly zero/read
or one/write and exact unprefixed instruction bytes from the stopped RIP.
It checks canonical continuation, classic SVM controls, pending injection,
interrupted delivery, CPL and TPR ownership before committing any changes.
Successful RDMSR zero-extends EDX:EAX; WRMSR uses their low halves and preserves
the full saved GPRs. ECX's low 32 bits select the MSR; flags remain unchanged.
No host APIC MSR is accessed. Generic MSR dispatch remains unsupported; the
existing clock-MSR refusal probes still pass.

`svm::x2apic` is a register adapter over `LocalApic`, not a second interrupt
owner. Shared instruction-continuation validation is reused, with a separate
scoped MSR entry point. Existing harness initialization, entry bridge and IDT
setup are shared rather than duplicated. Writes are refused while delivery is
armed; reads can observe retained state after the owner accounts for the exit.

Virtual interrupt masking is enabled before the first guest entry. CR8 reads
therefore see the synchronized virtual class. CR8 writes are intercepted and
stopped: even a write of the same class must clear the TPR subclass, which cannot
be detected by comparing class values afterward. General CR8 emulation remains
separate work.

## Executed checks

Each of 16 guest sessions executes 13 MSR accesses. It verifies fixed mode
readback, withheld generic feature bits, full TPR subclass, CR8 class reads,
pending IRR, delivered ISR, PPR before/after real WRMSR EOI, IRETQ restoration
and exactly one handler invocation. The monitor supplies the interrupt; no
physical APIC or timer source is used. The handler stack is bounded to 64 bytes.

Five actual negative probes stop on EOI read, reserved TPR write, nonzero EOI,
an unsupported APIC_BASE mode change, and a same-class CR8 write. Rejected MSR
accesses preserve VMCB/GPR state; CR8 stops before execution at exit 18h.
Both new completion/refusal markers are mandatory in shared runner accounting.
Removing either fails the completed-fixture check.

169 core tests, 251 returning-DXE tests and the UEFI target check pass. Five
new unit tests cover operand widths, register permissions, bitmap mapping,
EOI, transactional TPR, pending state, invalid continuation, privilege and
unsupported virtualization controls. These synthesize inert VMCB fields and
are distinguished from the actual guest runs.

Three corrected AVX/SSE/FXSAVE profiles plus 32 broader ownership, fault,
xstate, relocation and RDTSCP-disabled profiles pass on the unchanged patched
QEMU: 35 profiles total. Stock normal execution retains its older backend gaps,
and its separate guest-CR8 diagnostic still reproduces the expected assertion.
See [QEMU correction provenance](qemu-svm-corrections.md).

A fresh Astra/High agent performed normative and independent implementation
review. Exact source, binaries, results and review are retained externally in
`outputs/x2apic-register-fixture`. No physical image was activated.

## References and next work

AMD APM volume 2 revision 3.44: sections 15.11, 16.9, 16.11/Table 16-6,
16.6.4 and Appendix B. Local PDF SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
AMD volume 3 revision 3.37: RDMSR p438, WRMSR pp511–512 and Table B-1
pp599/606 confirm operand widths, zero extension and GPR preservation.
PDF SHA256 `c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`.
The AMD download endpoint failed; the AMD-authored PDF was retrieved from an
archival mirror. Exact provenance and retrieval limitations accompany review.

Next: checked guest fault delivery for rejected MSR operations, mode/register
coverage and CR8 synchronization. Timer registers, real scheduling/HLT wakeup,
periodic/deadline timers, frequency/divider ownership, IOAPIC/MSI, NMI and SMP
remain unsupported. Physical timing/drift and protected Windows compatibility
are unmeasured. This fixture does not prove Windows guest boot or sandbox readiness.
