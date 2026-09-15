# Independent HWCR30 manual verification - 2026-09-16

Scope: read-only review of `.agents/answers.md` Q1 for the native first-Windows-boot profile. No implementation, build, emulator run, physical execution, or flash was performed by this reviewer. Repository instructions, current handoff, README, CONTRIBUTING, both crate guides and malware-analysis direction were read. Only this research directory was written.

## Result

A tightly bounded physical HWCR[30] allowance is justified on the already admitted Family 1Ah Model 44h stepping 0 profile when CPUID Fn80000008_EBX[1] reports IRPerfCount. HWCR30 is a per-thread read/write enable, not a cache-routing control. Preserve every other physical bit, including reserved fields, and reject any requested change to those bits. A live physical HWCR value can be the runtime authority for bit30; a separate mutable shadow is unnecessary if every guest HWCR read reaches that owner and every write is verified before instruction completion.

Two reviewer claims require qualification. The stopped snapshot does not prove the faulting operands. Also, IRPerfCount has an optional SVM hardware virtualization dependency: APM 3.44 explicitly specifies it. The current source reviewer verified that the native profile requires VMCB 0xB8 to be zero, which disables that feature. This is a necessary profile boundary, not a reason to implement PMC virtualization for first boot.

INIT retention is supported by an independently followed architectural reference chain below. RESET and INIT must not be conflated.

## Sources and visual method

Local authoritative originals under `C:/Users/mato/Documents/svmvisor/docs`:

- `57896-3.00_PPR.pdf`: AMD publication 57896 rev 3.00, August 28 2024, Family 1Ah Model 44h revision B0. SHA256 `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`, 486 PDF pages.
- `24593_3.44_APM_Vol2.pdf`: AMD publication 24593 rev 3.44, March 2026, AMD64 Architecture Programmer's Manual Volume 2. SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`, 845 PDF pages.

Pages were rendered to full-page PNGs at 1.5x scale using PyMuPDF and viewed with the image tool. Text extraction was used only to locate pages. Authoritative findings below were read from page images, including the full HWCR table, scope notation, access/reset definitions, both pages of the initialization table, and full VMCB tables B-1/B-2. Images remain in `work/hwcr-verify-2026-09-16` with zero-based PDF indices in their filenames. Additional locator images are not all authority for conclusions.

PPR printed page = PDF index + 1 for the reviewed pages. APM main-body printed page = PDF index - 61. In the map below, PDF page means one-based position; index is explicitly zero-based.

## Critical reference chains

### Product applicability, support and thread scope

| Source / section | Printed page | PDF page / index | Finding |
| --- | --- | --- | --- |
| PPR title | 1 | 1 / 0 | Family 1Ah Model 44h B0, rev3.00/date |
| PPR 1.6.1 and package definitions | 32 | 32 / 31 | Revision notation and AM5 desktop applicability |
| PPR Fn00000001_EAX | 67 | 67 / 66 | Family = base family + extended family; model = extended and base model |
| PPR Fn80000008_EBX | 99 | 99 / 98 | Bit1 InstRetCntMsr, supported on this part |
| PPR 1.4.4.5.1-5.3 | 21-22 | 21-22 / 20-21 | MSR instance parameters identify thread/core scope; RDMSR/WRMSR use current executing CPU identity |
| PPR HWCR and IRPerfCount | 203-204, 188 | same / 202-203,187 | Both include thread[1:0], therefore per-thread instances |

The source currently gates capture on signature `0x00b40f40`, decoding to Family1Ah Model44h stepping0. This verifies the implementation's bounded product scope. The host's marketing name alone or a test fixture does not independently prove measured physical CPUID; keep exact-platform admission intact.

### HWCR enable, locks, reserved fields and resets

PPR MSRC001_0015 entire table at printed pp203-204 (PDF indices202-203): bit30 IRPerfEn is Read-write, Reset0, enabling IRPerfCount. Bit27 EffFreqReadOnlyLock is Write-1-only, Reset0, Init:BIOS,1; it makes MPerfReadOnly, APerfReadOnly and IRPerfCount read-only. The IRPerfEn row has no dependency on that lock or SmmLock. HWCR reset value is `0000_0000_0100_6010h`.

PPR MSRC000_00E9 at printed p188/index187: bits47:0 are the count; bits63:48 reserved. Count resets0 and increments once per retired instruction. AccessType is `HWCR[EffFreqReadOnlyLock] ? Read,Error-on-write,Volatile : Read-write,Volatile`. Followed cross-reference back to HWCR30 and HWCR27. This does not authorize the monitor to clear the lock or synthesize writes to a locked counter.

PPR pp24-28/indices23-27 closes the notation references: field formats, conditional access expressions and complete Table8 AccessType definitions. PPR1.4.4.9 p26 says unspecified reserved fields are write-as-read. PPR1.4.4.11 p27 says unprefixed reset values apply on warm and cold RESET. PPR1.4.4.12 p28 and Table10 say Init:BIOS is an initialization recommendation with an owner, not the architectural INIT event. Definitions of cold/warm RESET are on pp31-32/indices30-31.

PPR p204 distinguishes preserved bits: bit4 INVDWBINVD is Read,Error-on-write-0; bit3 TlbCacheDis has cache/page-table and TLB-flush effects; bit0 SmmLock has different locking semantics. The requested policy must not change them. The bit0 cross-reference was followed to PPR2.1.11.1.10 p51/index50 (locking SMM registers). APM3.2.10 p71/index132 similarly describes HWCR30 and defers product-specific HWCR fields to PPR/BKDG. APM15.32 p587/index648 describes SmmLock specifically; its explicit INIT immunity must not alone be extended to other bits.

Masked full-width write safety: the reviewed complete HWCR table contains no write-one-to-clear fields. Rewriting existing1 in bit0/bit27 preserves their write-one-only lock state. Existing admission requires bit4=1, avoiding its error-on-write-0 behavior. Bit33 is error-on-write-1 outside SMM and is unconditionally cleared by RSM (PPR p203 and p51); legitimate stopped non-SMM state therefore has bit33 clear. Do not introduce an allowance that fabricates or propagates a set SMM-only lock from untrusted guest state. Fresh physical write-as-read preservation plus strict non30 drift checks is materially safer than writing a guest or stale captured full-width value.

### INIT retention, distinct from RESET

APM14.1.3 and complete Table14-1 at printed pp481-482 (PDF543-544/indices542-543): performance-monitor resources and other MSRs are not modified by INIT. The table's performance-monitor cross-reference leads to TableA-6 p731/index792, which explicitly includes IRPerfCount (C00000E9), reset0. Full TableA-6 continuation pp732-733/indices793-794 was inspected. The other-MSR reference leads to AppendixA p718/index779, TableA-1 including HWCR on p723/index784, and from that row to HWCR3.2.10 p71/index132. Full TableA-1 pp718-726/indices779-787 was inspected.

Together with the PPR register definitions, these support retaining both HWCR30 and IRPerfCount across guest INIT. Do not clear the enable merely because its RESET value is zero. The count is volatile: retention does not promise a numerically constant count while instructions execute.

APM15.21.8 and complete Table15-12 pp535-536/indices596-597 distinguish intercepted/redirected INIT from normally taken INIT. INIT intercepted with GIF1 causes VMEXIT with INIT pending; redirected INIT produces #SX; GIF0 holds INIT pending. Thus an INIT exit is not proof that physical reset state was already applied. The guest startup owner remains responsible for its supported emulated INIT semantics.

### SVM and performance-counter virtualization

APM3.2.10 p71/index132 -> 13.2.1 p411/index472 -> dedicated IRPerf paragraph p421/index482. The latter explicitly identifies C00000E9, HWCR30 enable and Fn80000008_EBX[1] support. The AppendixA IRPerf row p721/index782 also points to p421. Product-specific width/access rules remain PPR authority.

APM15.39 p626 (PDF688/index687) explicitly includes `INSTR_RETIRED_CTR - IRPerfCount MSR` among state allocated when PMC virtualization is enabled. Ordinary guests enable it via VMCB offset0xB8 bit3. Support is Fn8000000A_EDX[8]. PPR Fn8000000A_EDX p101/index100 labels this bit PerfCtrVirt and says Fixed1 on this product. It is therefore inaccurate to say this hardware has no IRPerf VMCB dependency.

TableB-1 pp737-742/indices798-803 was inspected completely. At p742/index803, 0xB8 bits0/1/2/3 are LBR / VMSAVE-VMLOAD / IBS / PMC virtualization enable, respectively; bits63:4 reserved SBZ. TableB-2 pp743-746/indices804-807 was inspected completely. Its header says state offsets are relative to a state-save area beginning at VMCB0x400. `INSTR_RETIRED_CTR` is at state offset0x1C0, therefore whole-VMCB0x5C0.

15.39's Allowed SEV Features reference was followed to 15.36.20 pp617-618/indices678-679: it is an SEV-SNP feature-mask mechanism. SEV/SEV-ES/SNP and nested SVM are outside the admitted ordinary guest policy; this note does not approve their PMC paths. The PMC section also describes AVIC/NMI virtualization requirements for virtualized PMC interrupt delivery. Those enabled-mode branches are explicitly excluded here rather than claimed verified for use.

APM15.5-15.6 pp501-507/indices562-568 describes ordinary VMRUN, VMSAVE/VMLOAD and VMEXIT state transfers. HWCR/IRPerfCount are not in the ordinary transfer lists; the separate PMC feature is conditional. Source review reports native VMCB construction/entry rejects nonzero0xB8 and native guest CPUID Fn8000000A is zero. Under that bounded profile, physical enable plus physical counter passthrough is a defensible execution policy. It is not guest-only performance accounting. Host instructions and transitions may contribute to observations; exact attribution, latency, and counts have not been measured here. No instruction-count compensation or timing invisibility claim follows.

## Recommended bounded runtime policy

1. Keep exact native platform admission, per-thread ownership and unsupported protected/nested configuration refusal. Require actual physical IRPerf support before accepting a bit30 change; avoid inventing support from fixtures or a brand name.
2. Keep HWCR trapped. Read live physical HWCR on the owning stopped CPU. Compare all non30 bits with immutable capture before allowing access/continuation; report drift instead of silently accepting new routing/lock state.
3. For WRMSR, require `(requested ^ current) & ~IRPERF_EN == 0`; reject all other changes before hardware mutation. Respect current pending-event, instruction-decoding and next-RIP checks before beginning physical effects.
4. Compute the physical target from a fresh current value with only bit30 replaced. This preserves reserved write-as-read fields and all other controls. When target equals current, a no-op completion is sufficient after admission/drift checks.
5. Issue bounded physical write and readback on the same CPU. Require exact target readback before completing the guest instruction. On a post-write mismatch, leave guest RIP/register completion unchanged and terminally stop with observed evidence; hardware may already have changed, so do not pretend refusal rolled hardware back.
6. Successful WRMSR changes no GPRs/flags and advances only the validated instruction boundary. Successful RDMSR returns the live value through architectural EDX:EAX semantics. No shadow-only success while IRPerfCount remains physical.
7. Preserve live HWCR30 across emulated guest INIT. Do not reset it to admission capture. Keep PMC virtualization disabled and do not claim counter isolation from host execution.
8. Keep this allowance in existing native cache/MSR ownership instead of adding a separate unused PMU layer. Use the shared refusal record to capture MSR index, requested/current/expected values and phase so a future physical stop can prove its operands.

## Evidence limits

No hardware operands at the historical stop were recovered in this manual review. The reviewer conclusion that this exact write directly caused that snapshot remains strong static attribution, not proof. No physical HWCR write/readback, counter delta, INIT retention measurement, exit latency or Windows boot measurement was performed. Physical results apply only to exact tested images. Existing firmware/Windows protection configuration was not changed. Hyper-V/VBS coexistence, broader protected-memory configurations, arbitrary processors and PMC virtualization remain unsupported or untested as previously documented. A future boot pass would still not establish malware containment or undetectability.

There is no unresolved critical manual dependency for the bounded ordinary-SVM, bit30-only enable policy above. Exact physical count attribution and runtime acceptance remain empirical work. Enabled PMC virtualization's SEV/AVIC/NMI branches and unrelated HWCR controls are deliberately outside this policy, and must receive their own complete reference review before implementation.
