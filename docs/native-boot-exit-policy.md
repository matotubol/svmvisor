# Trusted native first-boot exit policy

The native continuation uses its own small execution policy in the existing
VMCB, MSRPM, CPUID and dispatch owners. The synthetic two-CPU/APIC model remains
an emulator fixture. This policy is intended for trusted firmware and Windows
boot preparation; it does not isolate devices, DMA, storage or networks.

The native VMCB profile intercepts CPUID, MSRs selected by the native MSRPM,
all SVM instructions and shutdown. Guest exceptions, interrupts, I/O, HLT,
CR/DR accesses, TSC instructions and XSETBV execute on the admitted hardware.
Initial setup requires no virtual interrupt controls, pending event or virtual
VMLOAD/VMSAVE extensions. Subsequent native dispatch accepts the inert retained
V_TPR nibble when V_INTR_MASKING is clear. Native APIC state stays with the executing processor.
VMEXIT disables host DR7 breakpoints; DR0–3 and extended state remain guest-live.
The complete persistent host must use no FP/SIMD, never change XCR0, and own no
debugger state. The target-owned guest INIT commit is the sole debug-register
mutation exception: after successful stopped-state preparation it clears live
DR0–3 on that same CPU and resets guest VMCB DR6/7 to FFFF0FF0h/400h. APM2
Table14-1 p482 defines those INIT values; they are not retained debug state.
The linked payload audit permits only the exact zero-only DR0–3 helper and
refuses other debug-register instructions. Ordinary exits, private INIT/#SX
notifications and duplicate SIPIs preserve the guest's debug state. XCR0,
extended state, PAT and separately retained MSRs retain their existing owners.
Native SMP ownership is a separate prerequisite: preserving
native CPUID topology does not place the other processors under the monitor.

CPUID responses are sampled on the owning processor, retain native identity,
topology and ordinary execution features, and derive OSXSAVE from stopped
guest CR4. The filter clears SVM/SKINIT and the SVM, encryption and multi-key
encryption leaves. Admission must separately reject an existing owning
hypervisor or active encryption; filtering cannot make either state supported.
The first executable fixture is intended to admit one physical emulator CPU.

The MSRPM permits the three covered native MSR ranges except EFER, TSC_RATIO
and the C0010100–C00101FF monitor/control region. Reserved map padding remains
intercepted. MSRs outside the ranges still intercept architecturally. Only
EFER has an emulation owner; any other intercepted MSR stops without mutation.
This is deliberately not a general MSR emulator.

`NativeEfer` admits the same initial long-mode EFER mask as native continuation
preparation (SCE, LME, LMA and NXE). Its logical SVME remains zero while VMCB
backing SVME remains one, as required by VMRUN. EFER reads zero-extend EDX:EAX;
writes preserve GPRs, change only admitted SCE/NXE state and request a complete
TLB flush when the logical value changes. CPL violations, MBZ writes, changing
LMA and changing LME with CR0.PG set queue #GP(0) at the original RIP. Defined
but unowned features, RAZ writes, nested SVM enable and leaving the admitted
long-mode state stop explicitly rather than receiving an invented fault.

Both CPUID and EFER completion use exact unprefixed instruction validation,
canonical RIP/next-RIP checks and existing transactional commit methods.
The runtime walks the stopped guest's current page tables and fetches each
instruction byte through a temporary supervisor read-only, non-executable
mapping. It permits retained RAM with current write-back caching and excludes
the monitor allocation and MMIO. The mapping is removed before guest entry.
This path needs no nRIP-save capability and handles page-crossing opcodes;
prefixed instructions remain an explicit unsupported case.
Pending injection, interrupted delivery or virtual interrupt state refuses
completion. Success consumes the instruction's interrupt shadow and RF;
TF is stopped because post-instruction #DB delivery has no owner here. A
queued #GP is a prepared retry, not proof that guest delivery executed.

Ten focused host tests cover logical/backing EFER separation, SCE/NXE writes,
architectural faults, unsupported-value refusals, stale state/bytes, native
MSRPM coverage, instruction intercepts, CPUID topology/OSXSAVE, RF/shadow/TF
and transactional NPT activation prerequisites.
Execution of those tests and the actual callback/EBS fixture belongs to the
integrating batch's evidence. No timing baseline, physical activation, Windows
boot, VBS/Hyper-V coexistence or malware containment is established here.
Protected Windows configurations remain unsupported or untested as recorded
by admission and the eventual exact-image run; no protection is disabled.

Primary source inspected: local AMD APM volume 2 revision 3.44,
`C:\Users\mato\Documents\svmvisor\docs\24593_3.44_APM_Vol2.pdf`, SHA256
`3D9DCB3F68222392D0EDE9970EFC95E31A047A247D54B454123D6981D278C48C`.
Applicable architectural sections: 3.1.6–3.1.7 (RFLAGS/EFER), 14.6.2 and
Table 14-5 (long-mode consistency), 15.5–15.7 (entry/exit and state),
15.11/Table 15-8 (MSRPM), 15.16 (TLB), 15.21 (interrupt masking),
15.30 (SVM MSRs), and Appendix B (VMCB encoding). These general architectural
rules do not substitute for product-specific CPU/platform admission evidence.
