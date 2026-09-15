# Ryzen extended LAPIC guest INIT reset — 2026-09-14

`svm::native_apic_reset::NativeApicReset` supplies read-only preparation and a
bounded register-write commit for actual target-owned guest INIT. Its native
runtime caller uses the current physical xAPIC/x2APIC register transport. The
existing `NativeStartupTarget` owns CPU reset and `NativeIcr` owns guest ICR
readback; the synthetic `LocalApic` is not involved. INIT/#SX notification and
running-target duplicate SIPI remain separate wake-only operations and do not
invoke this reset contract.

## Primary-source review

The supplied reference library contains AMD PPR57896 rev3.00, August28,2024,
Family1Ah Model44h B0, `57896-3.00_PPR.pdf`, SHA256
`643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5` (rechecked).
The section text was reviewed in `work/native-cache-ppr.txt`. AMD APM vol.2
rev3.44, March2026, SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`
provides the general architectural context in
`work/qemu-corrections/amd-apm-vol2.txt`. Page numbers below are printed pages.

| Requirement | Primary source and implemented consequence |
| --- | --- |
| INIT reset | PPR §2.1.11.2.1.15 p55: power-up APIC state except ID retained and pending writes complete. APM §16.10 p657 explicitly retains APIC_BASE AE/EXTD on INIT; the bus mode is preserved. |
| Extended layout | PPR APICx030 p56 / MSR803h p175: extended-space bit31 and directed-EOI bit24; APIC version10h. APICx400 p64 / MSR840h p183: feature00040007h, four extended LVTs, IER and SEOI present. |
| Quiescence | PPR APICx0F0 p57: software disable holds existing ISR/IRR and rejects new fixed/lowest-priority/ExtInt acceptance. SMI/NMI/INIT/LINT/SIPI remain possible. PPR pp58–59/62–65 defines bitmap, delivery-status, LINT remote-IRR and mask bits. |
| Extended LVT reset | PPR APIC500h–530h p65 and MSR850h–853h p185:10000h, including Mask=1. This processor-specific definition overrides generic APM Table16-2's zero. |
| Extended control / IER | PPR pp64–65/184: control resets0; IerEn gates IER writes; IER defined interrupt bits reset1. APM §16.7.2 and PPR MSR848h mark IER[15:0] reserved, so write bank0FFFF0000h and banks1–7FFFFFFFFh. |
| xAPIC destination | PPR p57: LDR0, DFR F0000000h; its reserved DFR bits are zero. The separate nonextended profile uses APM Table16-2's DFR FFFFFFFFh. x2APIC LDR is not written. |
| Error state | PPR APICx280 p60: a write latches and clears internal errors; a second write clears the visible latch. Two zero writes replace the earlier single write. |
| No acknowledgment | PPR SEOI p65/184: undefined without a pending vector. Neither SEOI nor EOI belongs in reset; device acknowledgment remains guest-owned. ICR writes generate messages, so only the existing guest readback owner resets its ICR. |

The exact extended profile requires CPUID signature00B40F40h, native APIC
version81050010h and feature00040007h. The signature follows the specified
Family1Ah/Model44h/stepping0 target and PPR p67's encoding. PPR lists six
standard LVT registers at320h–370h but leaves MaxLvtEntry's reset value `XXh`.
The implementation therefore still requires MaxLvtEntry=5 for those six
standard entries and refuses another advertised count, including6. This is an
explicit compatibility limit, not a measurement of the physical9900X APIC
version. A differing observed count requires resolving its register mapping
before extending admission. Nonextended six-LVT emulator support is retained.

## Stopped-state and exit contract

A queued guest INIT reaches the destination only after the existing INIT
intercept63h/private #SX notification drain. Saved VMCB/register state is
authoritative. `NativeStartupTarget::validate` and APIC preparation must both
succeed before CPU/APIC reset commit; a refusal leaves guest CPU/APIC state and
the queued command uncommitted and terminates the stopped runtime. No target
RIP is advanced to bypass a refusal. Successful CPU INIT retains the existing
reset semantics, then awaits SIPI; the source ICR instruction completes through
its separately validated MSR or MMIO exit owner.

Preparation requires the guest's SVR already software-disabled; IF/GIF clear
alone does not prevent incoming interrupts from accumulating in IRR. All eight
banks of each IRR/ISR/TMR must be empty; TPR/PPR/ESR and timer initial/current
counts must be zero. All six standard and, for Ryzen, four extended LVTs must
be masked with delivery status clear; LINT remote-IRR must also be clear. The
native xAPIC ICR must not be busy. APR must be zero where its read is admitted.
Preparation performs no writes and does not acknowledge a pending source to
manufacture quiescence.

Commit keeps SVR disabled, resets the standard and extended writable state,
enables IER writes temporarily while preserving the other extended controls,
resets the defined IER bits, then resets extended control to0. APIC identity,
APIC_BASE and physical ICR are preserved. Resetting ExtApicIdEn to0 is the
PPR-defined reset and changes its documented xAPIC physical-ID matching; the
native topology/notification owner must admit the resulting identity behavior.
No read of an unadvertised extended register is attempted.

This establishes local vectored quiescence under the existing ownership
exclusion for external nonvectored events and concurrent APIC writers. It does
not stop SMI/NMI/INIT hardware sources, drain asserted devices, reset an IOAPIC,
or provide general CPU/device reset. Those unsupported conditions remain
terminal. Guest software must disable SVR before an actual reset request;
ordinary enabled-SVR reset is deliberately refused. Wake-only notification
retains the broader pending/in-service/level-interrupt contract.

## Validation and fidelity limits

Eight focused host tests cover both bus plans, complete extended-state reset,
IER gating/order and reserved bits, absence of EOI/SEOI/ICR writes, every
bitmap bank/bit refusal, each standard/extended LVT, priority/timer/error/busy
ICR refusal, unavailable reads and incompatible layout refusal. The parent
integration task owns execution of tests, native builds, linked audits and
fresh emulator fixtures; their results are recorded separately. This document
does not claim those tests have run merely because their source exists.

Preparation and commit use fixed register loops and integer state, with no
allocation, firmware calls, FP, logging or unbounded waits. Physical reset and
notification latency, native-vs-virtualized timing distributions and Windows
boot/protection compatibility remain unmeasured. Hyper-V/VBS/HVCI, PatchGuard
and Secure Boot compatibility are untested by this batch. No physical launch,
reboot, firmware programming, Windows/ESP change, protection change or malware
execution occurred. This is bounded first-boot infrastructure, not containment
or general interrupt-platform compatibility evidence.
