# Native CET continuation

The 2026-09-15 physical snapshot identifies an APIC-adapter refusal with guest CR4=0xb50eb8 and CET bit23 set. It does not identify an architectural CET fault or prove that S_CET/U_CET shadow-stack operations were enabled. Windows boot remains unproven.

This batch admits the existing ordinary supervisor APIC MOV operation with CET enabled when CR0.WP is set. The decoded operation changes no shadow-stack state. The page walker retains effective permission checks: ordinary shadow-page reads are allowed; writes to read-only shadow pages remain refused, as do user-data MMIO, reserved CR4 bit24 and unsupported instructions. Native CET instructions and exception/control-transfer mechanics execute on the physical CPU.

Initial native takeover still requires CR4.CET off. The adapter now enumerates CET_SS before reading S_CET and ISST_ADDR and imports those MSRs into VMCB offsets5e0/5f0. Previously those fields remained zero even when inactive firmware MSRs contained nonzero values. Validation precedes mutation. The current SSP VMCB slot5e8 is preserved by ordinary software emulation. U_CET, PL0-3_SSP and XSS stay live on the same physical CPU; the resident host has CET disabled and does not use XSAVE-managed state. No new per-exit MSR swap or generic CET emulator was added.

The initial import is not complete arbitrary dormant-state takeover: disabled-CET RDSSP is NOP and cannot capture dormant SSP. Already-active CET firmware takeover remains refused. The reviewed AMD INIT tables explicitly retain CET MSRs and reset CR4, but do not specify non-MSR SSP's INIT value. Current SSP preservation is retained without claiming that missing rule is proven.

Three accompanying review claims were checked directly against original PDF images:

* V_INTR_MASKING comment is correct: when masking is enabled, host IF saved by VMRUN gates physical interrupts while guest IF gates virtual interrupts. The native startup path leaves physical interrupts native.
* V_INTR_VECTOR encoding at bits39:32 of the64-bit VMCB60h control field correctly places the vector in byte64h. Pending virtual IRQ preservation and V_IRQ-cleared delivery distinction are not reversed.
* INIT CS/data attributes9a/92 exactly match Table14-2;9b/93 would add Accessed. INIT CR0 retains CD/NW and sets bit4, so unconditional RESET value60000010 would be wrong when those bits were previously clear.

Validation includes independent exact-byte/transactional initialization and CET APIC tests, preservation across software INIT/SIPI, unchanged native CPUID/MSRPM exposure, existing SMP/VM_CR/PKE regression workloads and packaging checks. The current QEMU TCG does not execute CET: these results do not prove Zen5 shadow-stack entry/exit, #CP, XSAVES/XRSTORS, privilege changes, or reactivation after INIT. Physical first-boot testing remains necessary. There is no timing-fidelity, Hyper-V/VBS coexistence, migration, multiple-guest or containment claim.

Evidence and source provenance are retained under `work/native-after-apic-2026-09-15/`: `cet-native-state-audit.md`, `cet-apic-access-audit.md`, `cet-validation-plan.md`, `init-claims-review.md`, `interrupt-claims-review.md`, and independent source/linked reviews. Those reports distinguish page numbers and document hashes. Principal sources are AMD APM2 rev3.44 Tables14-1/14-2/B1/B2 and sections15.5/15.21/18, APM3 rev3.37 MOV/IRET/RDSSP/VMRUN/VMLOAD/VMSAVE algorithms, and PPR57896 rev3.00 CET MSR definitions. Critical references were read as complete rendered pages, not extracted summaries.
