# Native bootstrap APIC admission

The current physical candidate reached preparation stage13, then refused with
EFI_UNSUPPORTED before low-page allocation. Runtime pool allocation and every
slot's package initialization completed. See the preserved resource-fix batch.
A new Windows user-mode CPUID observation reports CPUID.1:ECX=0x7ed8320b:
x2APIC is not advertised and the hypervisor-present bit is clear. This is not
an observation of boot-time MSRs or proof that silicon lacks x2APIC; the exact
PPR permits firmware override of CPUID feature bits.

Four unconditional capability checks remained from the former fixed-x2APIC
bootstrap: preparation, AP firmware observation, AP assembly entry, and resident
runtime arming. The native guest-startup profile already owns an xAPIC MMIO path.
It now requires x2APIC capability only when the selected mode is x2APIC. The
ordinary fixed-x2APIC profile retains its requirement. Post-EBS BSP validation
also checks the selected mode before an ICR MSR can be used. AP assembly requires
the capability before EXTD promotion and still refuses an x2APIC downgrade.

Complete processor ownership admission, count bounds, VM_CR.R_INIT-clear,
enabled LAPIC/base/reserved-bit checks, UC mapping, full identity checks and
extended-xAPIC routing admission remain. Firmware-owned AP mode is not changed.
Stages25–28 export count, CPUID.1:ECX, VM_CR and APIC_BASE at early refusal; the
snapshot decoder labels these as observed register values.

The guest APIC_BASE writer also receives the local advertised capability.
Without it, attempted x2APIC selection stops with UnsupportedMode before any
physical WRMSR, routing commit or guest RIP/state change. Same-mode xAPIC access
remains supported. No new #GP is invented for a potentially firmware-masked
feature. The established illegal-transition #GP behavior remains unchanged.

Normative review: local APM2 rev3.44 §16.9/Table16-5 (x2APIC interface and mode
transition rules), §15.30.1 (VM_CR.R_INIT redirects INIT to #SX), and exact PPR
57896 rev3.00 printed68/240 (CPUID feature override and MSRC001_1004 bit53).
Detailed page images/hashes and findings are retained under
`work/native-bootstrap-gates-2026-09-14/architecture-gate-review.md`.

This batch changes preparation admission and an unsupported guest mode request
boundary. It does not establish Windows boot, post-loader telemetry, timing
fidelity, Hyper-V/VBS support or malware containment. Existing historical
x2APIC-off refusal fixtures remain evidence of their old exact builds; current
native xAPIC success must be measured with capability disabled.
