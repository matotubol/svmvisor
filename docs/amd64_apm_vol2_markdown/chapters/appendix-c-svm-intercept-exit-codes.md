<!-- PDF source page: 818 | printed page: 756 -->

<a id="appendix-c-svm-intercept-exit-codes"></a>

# Appendix C SVM Intercept Exit Codes

On a #VMEXIT, a reason for exit (an exit code) is stored in the EXITCODE field in the VMCB. The exit codes are defined in Table C-1. Intercept exit codes are equal to the bit position of the corre-sponding flag in the VMCB’s intercept vector.

**Table C-1. SVM Intercept Codes**

| Code | Name | Cause |
| --- | --- | --- |
| 0h–Fh | VMEXIT CR[0–15] READ<br>_ _ | Read of CR 0 through 15, respectively. |
| 10h–1Fh | VMEXIT CR[0–15] WRITE<br>_ _ | Write of CR 0 through 15, respectively. |
| 20h–2Fh | VMEXIT DR[0–15] READ<br>_ _ | Read of DR 0 through 15, respectively. |
| 30h–3Fh | VMEXIT DR[0–15] WRITE<br>_ _ | Write of DR 0 through 15, respectively. |
| 40h–5Fh | VMEXIT EXCP[0–31] | Exception vector 0–31, respectively. |
| 60h | VMEXIT INTR | Physical INTR (maskable interrupt). |
| 61h | VMEXIT NMI | Physical NMI. |
| 62h | VMEXIT SMI | Physical SMI (the EXITINFO1 field provides more<br>information). |
| 63h | VMEXIT INIT | Physical INIT. |
| 64h | VMEXIT VINTR | Virtual INTR. |
| 65h | VMEXIT CR0 SEL WRITE<br>_ _ _ | Write of CR0 changed bits other than CR0.TS or CR0.MP. |
| 66h | VMEXIT IDTR READ<br>_ _ | Read of IDTR. |
| 67h | VMEXIT GDTR READ<br>_ _ | Read of GDTR. |
| 68h | VMEXIT LDTR READ<br>_ _ | Read of LDTR. |
| 69h | VMEXIT TR READ<br>_ _ | Read of TR. |
| 6Ah | VMEXIT IDTR WRITE<br>_ _ | Write of IDTR. |
| 6Bh | VMEXIT GDTR WRITE<br>_ _ | Write of GDTR. |
| 6Ch | VMEXIT LDTR WRITE<br>_ _ | Write of LDTR. |
| 6Dh | VMEXIT TR WRITE<br>_ _ | Write of TR. |
| 6Eh | VMEXIT RDTSC | RDTSC instruction. |
| 6Fh | VMEXIT RDPMC | RDPMC instruction. |
| 70h | VMEXIT PUSHF | PUSHF instruction. |
| 71h | VMEXIT POPF | POPF instruction. |
| 72h | VMEXIT CPUID | CPUID instruction. |
| 73h | VMEXIT RSM | RSM instruction. |
| 74h | VMEXIT IRET | IRET instruction. |
| 75h | VMEXIT SWINT | Software interrupt (INTn instructions). |
| 76h | VMEXIT INVD | INVD instruction. |
| 77h | VMEXIT PAUSE | PAUSE instruction. |
| 78h | VMEXIT HLT | HLT instruction. |

<details>
<summary>Rendered source page 818 (figures/tables)</summary>

![Rendered source PDF page 818](../assets/pages/pdf-page-0818.webp)

</details>


<!-- PDF source page: 819 | printed page: 757 -->

**Table C-1. SVM Intercept Codes (continued)**

| Code | Name | Cause |
| --- | --- | --- |
| 79h | VMEXIT INVLPG | INVLPG instructions. |
| 7Ah | VMEXIT INVLPGA | INVLPGA instruction. |
| 7Bh | VMEXIT IOIO | IN or OUT accessing protected port (the EXITINFO1 field<br>provides more information). |
| 7Ch | VMEXIT MSR | RDMSR or WRMSR access to protected MSR. |
| 7Dh | VMEXIT TASK SWITCH<br>_ _ | Task switch. |
| 7Eh | VMEXIT FERR FREEZE<br>_ _ | FP legacy handling enabled, and processor is frozen in an<br>x87/mmx instruction waiting for an interrupt. |
| 7Fh | VMEXIT SHUTDOWN | Shutdown |
| 80h | VMEXIT VMRUN | VMRUN instruction. |
| 81h | VMEXIT VMMCALL | VMMCALL instruction. |
| 82h | VMEXIT VMLOAD | VMLOAD instruction. |
| 83h | VMEXIT VMSAVE | VMSAVE instruction. |
| 84h | VMEXIT STGI | STGI instruction. |
| 85h | VMEXIT CLGI | CLGI instruction. |
| 86h | VMEXIT SKINIT | SKINIT instruction. |
| 87h | VMEXIT RDTSCP | RDTSCP instruction. |
| 88h | VMEXIT ICEBP | ICEBP instruction. |
| 89h | VMEXIT WBINVD | WBINVD or WBNOINVD instruction. |
| 8Ah | VMEXIT MONITOR | MONITOR or MONITORX instruction. |
| 8Bh | VMEXIT MWAIT | MWAIT or MWAITX instruction. |
| 8Ch | VMEXIT MWAIT CONDITIONAL<br>_ _ | MWAIT or MWAITX instruction, if monitor hardware is<br>armed. |
| 8Eh | VMEXIT RDPRU | RDPRU instruction. |
| 8Dh | VMEXIT XSETBV | XSETBV instruction. |
| 8Fh | VMEXIT EFER WRITE TRAP<br>_ _ _ | Write of EFER MSR (occurs after guest instruction<br>finishes). |
| 90h-9Fh | VMEXIT CR[0-15] WRITE TRAP<br>_ _ _ | Write of CR0-15, respectively (occurs after guest instruction<br>finishes). |
| A0h | VMEXIT INVLPGB | INVLPGB instruction. |
| A1h | VMEXIT INVLPGB ILLEGAL<br>_ _ | Illegal INVLPGB instruction. |
| A2h | VMEXIT INVPCID | INVPCID instruction. |
| A3h | VMEXIT MCOMMIT | MCOMMIT instruction. |
| A4h | VMEXIT TLBSYNC | TLBSYNC instruction. |
| A5h | VMEXIT BUSLOCK | Bus lock while Bus Lock Threshold Counter value is 0. |
| A6h | VMEXIT IDLE HLT<br>_ _ | HLT instruction if a virtual interrupt is not pending. |

<details>
<summary>Rendered source page 819 (figures/tables)</summary>

![Rendered source PDF page 819](../assets/pages/pdf-page-0819.webp)

</details>


<!-- PDF source page: 820 | printed page: 758 -->

**Table C-1. SVM Intercept Codes (continued)**

| Code | Name | Cause |
| --- | --- | --- |
| 400h | VMEXIT NPF | Nested paging: host-level page fault occurred (EXITINFO1<br>contains fault error code; EXITINFO2 contains the guest<br>physical address causing the fault). |
| 401h | AVIC INCOMPLETE IPI<br>_ _ | AVIC—Virtual IPI delivery not completed. See "AVIC IPI<br>Delivery Not Completed" on page 580 for EXITINFO1–2<br>definitions. |
| 402h | AVIC NOACCEL | AVIC—Attempted access by guest to vAPIC register not<br>handled by AVIC hardware. See "AVIC Access to Un-<br>accelerated vAPIC register" on page 581 for EXITINFO1–2<br>definitions. |
| 403h | VMEXIT VMGEXIT | VMGEXIT instruction. |
| F000 000h | Unused | Reserved for Host. |
| –1 | VMEXIT INVALID | Invalid guest state in VMCB. |
| –2 | VMEXIT BUSY | BUSY bit was set in the VMSA (see "Interrupt Injection<br>Restrictions" on page 614). |
| –3 | VMEXIT IDLE REQUIRED<br>_ _ | The sibling thread is not in an idle state (see "Side-Channel<br>Protection" on page 615). |
| -4 | VMEXIT INVALID PMC<br>_ _ | Invalid PMC state (see “Performance Monitoring Counter<br>Virtualization” on page N). |

<details>
<summary>Rendered source page 820 (figures/tables)</summary>

![Rendered source PDF page 820](../assets/pages/pdf-page-0820.webp)

</details>
