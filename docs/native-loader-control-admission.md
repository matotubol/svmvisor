# Initial loader control admission

The resident activation adapter and initial native continuation now accept
captured CR4.FSGSBASE and CR4.PCIDE. They share the existing continuation owner's
numeric CR4 predicate. No control bit is enabled, disabled or normalized to
obtain admission. This closes an unconditional rejection of otherwise supported
captured state; Windows use of these bits at ExitBootServices is still unobserved.

FSGSBASE enables instructions operating on the existing FS/GS hidden bases.
The captured VMSAVE auxiliary image, initial VMCB serialization and every
VMLOAD/VMSAVE transition already retain those bases and KernelGsBase. Enabling
this instruction set requires no additional XSAVE component or host TLS access.

With PCIDE set, the current paging adapter passes the entire captured CR3 and
`pcid=true` to the existing four-level page walker. The walker separates the
12-bit PCID from the page-table address. Initial VMCB preparation likewise
preserves those bits instead of interpreting them as PWT/PCD. With PCIDE clear,
the native adapter still refuses nonzero low CR3 bits because its paging-
structure reads require the admitted WB cache policy.

The existing private host root is page aligned and therefore uses PCID zero.
Its MOV-to-CR3 operand has bit 63 clear, so the hardware invalidates nonglobal
translations for the destination PCID. The no-flush operand hint is never a
captured CR3 bit and remains rejected by the continuation address validator.
VMRUN and VMEXIT preserve their respective CR3/CR4 state; the host runs under
ASID zero and the guest under its existing separately owned ASID. No new
VM-exit interception or emulation is introduced by this admission change.
Existing global-mapping and trusted native identity requirements still apply.

SMEP/SMAP remain refused at initial capture: the native paging-structure reader
dereferences identity aliases under the loader root, and its current RAM/cache
checks do not prove supervisor access permission for every such alias. Merely
checking the final retained leaf mappings would not justify admitting those
controls. PKE, CET and LA57 also remain refused. The original XSS=0, XCR0=3/7
and at-most-1,024-byte initial save-image restriction remains; extending that
image requires coordinated boundary stack ABI, component validation, restore
and returning-profile work. None of these restrictions establish which state
the real Windows loader will present.

New host tests check PCID values zero, one, cache-bit-shaped 0x18 and 0xfff;
the page walker resolves the same physical root for all four. Continuation
tests check exact CR3/CR4, hidden FS/GS and KernelGsBase preservation, and refuse
the no-flush operand hint. Negative adapter cases retain unsupported protection
controls, missing PAE/OSFXSR, caching-disabled CR0 and non-PCID low CR3 refusal.
The final host run passed these cases as part of 414 core and 290 native DXE
checks. The exact checkpoint is recorded in
`work/native-firstboot-2026-09-14/validation-v3.json`.

The actual FSGSBASE fixture also passed on the frozen INIT/#SX QEMU backend:
it enabled FSGSBASE before EBS, wrote distinct FS/GS bases with WRFSBASE/WRGSBASE,
executed a real intercepted CPUID, checked both bases, and restored them before
returning to Rust. Both failed and successful EBS paths preserve captured state.
The complete matrix passed 11 scenarios and 118 AP restarts, including 24- and
32-CPU runs. These are emulator observations, not measurements on the Ryzen.

Actual PCID execution remains unmeasured: the same backend's TCG reports PCID
unsupported even when requested, and the fixture correctly refused before EBS.
`run-controls-attempt-01` retains that result. The backend and admission checks
were not changed to manufacture a pass.

No physical timing, Windows boot, protection change, Hyper-V/VBS/HVCI,
PatchGuard or Secure Boot compatibility measurement was performed for this
change. It supplies neither nested SVM nor malware containment evidence.

Normative source consulted in the user's local reference library:
AMD APM volume 2, document 24593 revision 3.44, sections 3.1.3, 5.5.1,
15.5.1, 15.5.2 and 15.16.1. The supplied PDF SHA256 is
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
These architectural controls are admitted only as actual CPU state; no
processor-specific capability or Windows-loader enablement is inferred.
