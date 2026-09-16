# PPR fact sheet: LAPIC, x2APIC, AVIC and topology on Family 1Ah Model 44h B0 (Ryzen 9 9900X, CPUID 0x00B40F40)

I read this manually from rendered page images. Page images are in `pages/` (144 dpi full pages, plus top and bottom halves at 230 dpi). Critical pages were rendered again at 200 or 240 dpi into `pages/hires/`. I used `find_pages.py` only to find pages; none of its text output is used as evidence.

**Citation format:** `pN/iM` means PDF page N (one-based) with zero-based index M = N-1. The printed page number is also N; the page-numbering section below explains how I checked this.

## 0. Source

| Item | Value |
|---|---|
| Document | AMD #57896, "Processor Programming Reference (PPR) for AMD Family 1Ah Model 44h, Revision B0 Processors" (cover, p1/i0) |
| Revision / date | Rev 3.00, Aug 28, 2024 (header on every page viewed) |
| File | `docs/57896-3.00_PPR.pdf`, 486 PDF pages |
| SHA256 | `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`. The render script printed this on every run, and it matches the expected value. |
| Applicability to 0x00B40F40 | **Confirmed for family and model; stepping is not confirmed by the PPR.** Decoding 0x00B40F40 gives ExtFamily 0Bh, ExtModel 4h, BaseFamily Fh, BaseModel 4h, Stepping 0h, so Family = Fh+0Bh = 1Ah and Model = 44h. The PPR's `CPUID_Fn00000001_EAX` (FamModStep) resets agree: ExtFamily 0Bh, ExtModel 4h, BaseFamily Fh. BaseModel and Stepping are only "Xh" (p67/i66). The cover and every page header say "Family 1Ah Model 44h B0". Section 1.6.1 (p32/i31) says the document "uses a revision letter instead of specific model numbers" and points to "the revision guide in 1.2", but Table 1 in 1.2 (p12/i11) lists no revision guide. **The PPR therefore never states that B0 = stepping 0.** Table 12 (p32) lists package AM5 ("Desktop, single socket", PkgType 00h), which fits a desktop 9900X. The PPR does not name retail products. |
| Page numbering | The printed footer number equals the PDF page number on every page I viewed: 1-6, 12, 15-16, 19-28, 32-37, 43-44, 50-70, 73-75, 86-88, 98-101, 109-111, 119-121, 173-185, 202-204, 215-217, 240-244, 458, 486. |

## 1. Reading conventions that change the meaning of the tables

- **Access types** (Table 8, p27/i26):
  - Read-only: "Readable; writes are ignored."
  - Write-only: "Writable. Reads are undefined."
  - Write-0-only: "Writing a 0 clears to a 0; Writing a 1 has no effect."
  - Error-on-read / Error-on-write: "Error occurs on read/write". The kind of error is not stated.
  - Error-on-write-1: "Error occurs on bitwise write of 1."
  - Volatile: hardware may modify the field. Reads must not be cached and writes must not be elided.
  - A Reserved field with no explicit type is "write-as-read" (1.4.4.9, p26/i25).
- **Reset values.** A Reset value is "the value ... at the time that hardware exits reset, before firmware initialization" and applies at warm and cold reset (1.4.4.11, p27/i26). A **"Fixed"** reset is "The read value that applies at all times" (Table 9, p28/i27). So "Reset: 0" without "Fixed" does **not** mean the field always reads 0.
- **"X" digits** (for example XXh) are not defined in 1.4.1 or 1.4.2 (p15-16). The manual uses them for values that vary ("Model numbers vary with product").
- **X2APICEN** = `APIC_BAR[ApicEn] && APIC_BAR[x2ApicEn]` (Table 13, p37/i36; glossary p486/i485). Every x2APIC MSR access type has the form `X2APICEN ? <type> : Error-on-read,Error-on-write`.
- **Instance suffixes** (1.4.4.5.3, p22/i21; 2.1.9, p43/i42):
  - `_ccd[1:0]_lthree0_core[7:0]_thread[1:0]` means one register per logical thread. This applies to every LAPIC register and x2APIC MSR, APIC_BAR, VM_CR, HWCR, AvicDoorbell, CPUID_Features, and every CPUID table cited here.
  - SYS_CFG is `_core[7:0]` only, so it is per core and shared by SMT siblings.

## 2. Answers

### Q1. Local APIC register definitions (2.1.11.2.2, p56-65; register table in section 3)

**MMIO location.** The MMIO range is 4 KB at `{APIC_BAR[ApicBar[47:12]],000h}`, UC memory type, offsets APICx020 through APICx530. It is "treated as normal memory space when APIC is disabled" (2.1.11.2.1.2, p52/i51).

**APIC ID and version**
- **APIC20 ApicId** (p56/i55): Read-only. Bits 31:24 hold ApicId, "Reset: XXh. The reset value varies based on core number". Bits 23:0 are Reserved.
- **APIC30 ApicVersion** (p56/i55, confirmed at 240 dpi): Read-only.
  - Bit 31 ExtApicSpace: Reset 1, "presence of extended APIC register space starting at ExtendedApicFeature".
  - Bits 30:25: Reserved.
  - **Bit 24 DirectedEoiSupport: "Read-only. Reset: Fixed,1. 0=Directed EOI capability not supported."** The x2APIC copy on p175 gives "Reset: 1" with the legend "1=Directed EOI capability supported".
  - Bits 23:16 MaxLvtEntry: "Reset: XXh. Specifies the number of entries in the local vector table minus one."
  - Bits 15:8: Reserved.
  - Bits 7:0 Version: Reset 10h.
  - The implied physical value is `0x81nn_0010`, but the PPR never states nn.
- The PPR has no "EOI-broadcast suppression" field. Bit 24 (DirectedEoiSupport) is the only related bit.

**Priority and EOI registers**
- **APIC80 TaskPriority** (p56): Read-write, Reset 0. Bits 7:0 Priority; bits 31:8 Reserved.
- **APIC90 ArbitrationPriority** and **APICA0 ProcessorPriority** (p56): Read-only,Volatile, Reset 0, bits 7:0.
  - PPR is "the higher value of the task priority value and the current highest in-service interrupt".
  - The arbitration algorithm is in 2.1.11.2.1.11 (p54/i53).
- **APICB0 EndOfInterrupt** (p57/i56): "Write-only." with no reset value. Bits 31:0 are Reserved.
- **APICC0**: "Remote Read is not supported." Read-only, Reset 0.

**Destination registers**
- **APICD0 LocalDestination** (p57): Read-write,Volatile, Reset 0. Bits 31:24 Destination.
- **APICE0 DestinationFormat** (p57): Read-write, Reset F000_0000h, **"Only supported in xAPIC mode."** Bits 31:28 Format: 0h = cluster, Fh = flat, Eh-1h Reserved.
  - Flat mode addresses up to 8 APICs. Cluster mode addresses "15 clusters of 4" (2.1.11.2.1.5, p52).
  - Physical mode allows "up to 255 APICs" (2.1.11.2.1.4, p52).

**APICF0 SpuriousInterruptVector** (p57): "Reset: 0000_00FFh" with no register-level access type.
- **Bits 31:10: Reserved. Bit 12 is not defined.**
- Bit 9 FocusDisable: Read-write, Reset 0.
- Bit 8 APICSWEn: Read-write,Volatile, Reset 0. When 0: "SMI, NMI, INIT, LINT[1:0], and Startup interrupts may be accepted; pending interrupts in ISR and IRR are held, but further fixed, lowest-priority, and ExtInt interrupts are not accepted. All LVT entry mask bits are set and cannot be cleared."
- Bits 7:0 Vector: Read-write,Volatile, Reset FFh.

**ISR, TMR and IRR**
- **APIC100-170 InService** (p58/i57): Read-only,Volatile, Reset 0. "The first 16 InServiceBits of the first InService register are Reserved."
- **APIC180-1F0 TriggerMode** (p58-59): Read-only,Volatile, Reset 0. 1 = level, 0 = edge. The bit is updated when an interrupt is accepted, and the first 16 bits are Reserved.
- **APIC200-270 InterruptRequest** (p59-60): "Read-only. Reset: 0000_0000h." The MMIO copy has no Volatile flag. The first 16 bits are Reserved.
- 2.1.11.2.1.6 (p53/i52) adds: "Vectors[15:0] are Reserved."

**APIC280 ErrorStatus** (p60/i59): no register-level row.
- Behaviour: "The value written by software is arbitrary. Each write causes the internal error state to be loaded into this register, clearing the internal error state."
- Fields, each Read-write with Reset 0:
  - Bit 7 IllegalRegAddr ("Can only be set in xAPIC mode").
  - Bit 6 RcvdIllegalVector and bit 5 SentIllegalVector: "00h to 0Fh for fixed and lowest priority interrupts".
  - Bit 3 RcvAcceptError and bit 2 SendAcceptError.
- Bits 31:8, 4 and 1:0 are Reserved.

**APIC300 InterruptCommandLow** (p61/i60): "Reset: 0000_0000h."
- Bits 19:18 DestShrthnd (Read-write): 0h = destination field, 1h = Self, 2h = All including self, 3h = All excluding self. With 2h or 3h, "destination mode is ignored and physical is automatically used".
- Bits 17:16 RemoteRdStat: Read-only.
- Bit 15 TM: Read-write, 0 = edge.
- Bit 14 Level: Read-write.
- Bit 13: Reserved.
- **Bit 12 DS: Read-only.** "In xAPIC mode this bit is set to indicate that the interrupt has not yet been accepted ... Software may repeatedly write InterruptCommandLow without polling the DS bit; all requested IPIs are delivered."
- Bit 11 DM: Read-write, 0 = physical.
- Bits 10:8 MsgType (Read-write): 0h Fixed, 1h Lowest Priority, 2h SMI, **3h Reserved**, 4h NMI, 5h INIT, 6h Startup, 7h External interrupt.
- Bits 7:0 Vector: Read-write.
- Table 17 (p55/i54) lists the only valid ICR combinations: Fixed with edge; Fixed with level and assert; LowPri, SMI, NMI or INIT with edge, or with level and assert, and a destination or all-excluding-self shorthand; Startup with a destination or all-excluding-self shorthand.

**APIC310 InterruptCommandHigh** (p62/i61): Read-write, Reset 0. Bits 31:24 DestinationField.

**LVT registers.** Every LVT resets to 0001_0000h, which is the masked state. Common fields:
- Bit 16 Mask: Read-write, Reset 1.
- Bit 12 DS: Read-only,Volatile.
- Bit 11: Reserved.
- Bits 10:8 MsgType: Read-write, "See 2.1.11.2.1.14".
- Bits 7:0 Vector: Read-write.

Per-register differences:
- **APIC320 TimerLvtEntry** (p62): bits 31:18 Reserved; **bit 17 Mode** (0 = one-shot, 1 = periodic); bits 15:13 Reserved; the MMIO copy has MsgType at bits 10:8. The PPR defines no TSC-deadline mode.
- **APIC330 ThermalLvtEntry** (p62): common layout, bits 31:17 Reserved.
- **APIC340 PerformanceCounterLvtEntry** (p62): same layout. Sources are overflows of PERF_LEGACY_CTL0-3 and PERF_CTL0-5.
- **APIC350/360 LVTLINT[0]/[1]** (p63/i62): bit 15 TM (Read-write); bit 14 RmtIRR (Read-only,Volatile; set when a level interrupt begins service and cleared at EOI); **bit 13 Reserved, so there is no polarity bit.**
- **APIC370 ErrorLvtEntry** (p63): common layout.

**Generalized LVT message types** (2.1.11.2.1.14, p55): "All LVTs (ThermalLvtEntry to LVTLINT, and ExtendedInterruptLvtEntries) support a generalized message type: **000b=Fixed, 010b=SMI, 100b=NMI, 111b=ExtINT; all other message types are Reserved.**"

**Timer registers**
- **APIC380 TimerInitialCount** (p63): Read-write,Volatile, Reset 0, bits 31:0.
- **APIC390 TimerCurrentCount** (p63): Read-only,Volatile, Reset 0.
- **APIC3E0 TimerDivideConfiguration** (p64/i63): Read-write, Reset 0. Bits 3:0 Div, and "Div[2] is unused."
  - Valid values: 0h /2, 1h /4, 2h /8, 3h /16, 8h /32, 9h /64, Ah /128, Bh /1.
  - Reserved values: 7h-4h and Fh-Ch.

**Extended APIC registers**
- **APIC400 ExtendedApicFeature** (p64): "Read-only. Reset: 0004_0007h." Bits 23:16 ExtLvtCount = 04h; bit 2 ExtApicIdCap = 1; bit 1 SeoiCap = 1; bit 0 IerCap = 1.
- **APIC410 ExtendedApicControl** (p64): Read-write, Reset 0. **Bits 31:3 are Reserved, so there is no directed-EOI enable.**
  - Bit 2 ExtApicIdEn: enables an 8-bit APIC ID. Physical broadcast then requires IntDest[7:0] = FFh, and matching uses [7:0] instead of [3:0].
  - Bit 1 SeoiEn: "Enable SEOI generation when a Write to SpecificEndOfInterrupt is received".
  - Bit 0 IerEn: "Enable writes to the interrupt enable registers".
- **APIC420 SpecificEndOfInterrupt** (p65/i64): Read-write, Reset 0. Bits 7:0 EoiVec. "The behavior is undefined if no interrupt is pending for the specified interrupt vector."
- **APIC480-4F0 InterruptEnable** (p65): "Read-write. Reset: FFFF_FFFFh." These masks apply only when IerEn=1. A masked interrupt keeps its IRR bit set (2.1.11.2.1.8, p53).
- **APIC500-530 ExtendedInterruptLvtEntries** (p65): Reset 0001_0000h. Bit 16 Mask, bit 12 DS (Read-only,Volatile), bits 10:8 MsgType, bits 7:0 Vector.
  - APIC500 is IBS, APIC510 is the MCA threshold (`McaIntrCfg[ThresholdLvtOffset]`), and APIC520 is deferred errors (`MCi_CONFIG[DeferredIntType]`). APIC530 has no assignment.

**EOI and spurious-interrupt behaviour** (2.1.11.2.1.7, p53)
- An EOI write clears the highest ISR bit.
- "If the corresponding bit in TriggerMode is set, a Write to EndOfInterrupt is performed on all APICs to complete service of the interrupt at the source." This is the EOI broadcast, and the PPR describes no way to suppress it.
- Spurious interrupts (2.1.11.2.1.9) leave ISR unchanged and cause no EOI.

**Implications (my inferences, not PPR text)**
- **Which guest LVT values are legal:**
  - Vector must be 16-255.
  - MsgType must be one of 000b, 010b, 100b or 111b. 001b, 011b, 101b and 110b are Reserved for LVTs.
  - Timer LVT: only bit 17 is a mode bit, so TSC-deadline (bit 18) is illegal. Bits 11:8 must be 0 in x2APIC form (see D1 in section 5).
  - LINT LVTs: bit 13 is Reserved.
  - DS and RmtIRR are read-only.
- **What to mirror onto the physical LAPIC:** only vector, mask, mode and trigger mode with MsgType 000b. Never mirror SMI (010b); per p204, with HWCR[SmmLock] "SMI interrupts are not intercepted in SVM". NMI and ExtINT on the physical LINT pins belong to the platform (wiring unknown; see Q8).
- **SVR:** only bits 9:0 are defined. Never set bit 12 on the physical LAPIC. Treat guest bit 12 as reserved because the guest version bit 24 is 0.
- **Directed EOI / EOI-broadcast suppression:** bit 24 says "supported", but the PPR defines no enable bit, so treat it as unavailable. Specific EOI is available: SeoiCap=1, but SeoiEn must be set first (reset 0).
- **Version value:** 0x0005_0010 keeps Version 10h, which matches the PPR.
  - MaxLvtEntry 05h matches the six standard LVTs the PPR defines (320-370), but the PPR only gives XXh. Read the host's live APIC version register to confirm.
  - Clearing bit 31 (no extended space) and bit 24 (no directed EOI) are deliberate reductions from the physical 0x81nn0010. The bit-24 change is the only self-consistent choice given the missing enable bit.

### Q2. x2APIC MSR interface

**Defined: yes.**
- 2.1.11.2.1.2 (p52/i51) says the "MSR X2APIC space" is "the local APIC register space in x2APIC mode ... from MSR0000_0802 to MSR0000_08[53:50] (APIC_ID through ExtendedInterruptLvtEntries)".
- It states: **"If (Core::X86::Msr::APIC_BAR[x2ApicEn] == 0) then GP-read-write."** and "RDMSR/WRMSR will occur in program order."
- Full per-MSR tables are on p174-185 (i173-i184). All are per-thread. Access below applies when X2APICEN=1; otherwise every MSR is Error-on-read,Error-on-write. Register-level resets are 0 unless noted.

**ID and version MSRs**
- **0802 APIC_ID** (p174): 63:32 Reserved. **31:0 ApicId[31:0], "Local x2APIC ID register", Reset XXXX_XXXXh**. Access Read-only,Error-on-write. No register-level row.
- **0803 ApicVersion** (p175): the same fields as APIC30, with bit 24 "Reset: 1". Access Read-only,Error-on-write.

**Priority and EOI MSRs**
- **0808 TPR:** Read-write,Volatile.
- **0809 APR** and **080A PPR:** Read-only,Error-on-write,Volatile.
- **080B EOI:** bits 63:0, "A Write zero ... indicates the end of interrupt". Access **Write-0-only,Error-on-read,Error-on-write-1**.

**Destination and SVR MSRs** (p176)
- **080D LDR:** 31:16 ClusterDestination and 15:0 LogicalDestination ("one of up to sixteen x2APICs within the cluster"; bit n = x2APIC n). Both fields are **Read-only,Error-on-write**.
- **080F SVR:** **63:10 Reserved.** Bits 9, 8 and 7:0 are Read-write, with Vector reset FFh. No register-level row.

**ISR, TMR, IRR and ESR**
- **0810-0817 ISR**, **0818-081F TMR** and **0820-0827 IRR** (p177): 31:0, Read-only,Error-on-write,Volatile.
- **0828 ESR** (p178): same bits as MMIO. Each field is **Read,Write-0-only,Error-on-write-1,Volatile**.

**0830 InterruptCommand** (p179): a single 64-bit register, Read-write.
- 63:32 DestinationField.
- 19:18 DestShrthnd.
- **17:16 Reserved.**
- 15 TM and 14 Level.
- **13:12 Reserved, so there is no DS bit.**
- 11 DM, 10:8 MsgType (same valid values), 7:0 Vector.
- No MSR 0831 is defined.

**LVT MSRs.** Registers are Read-write, DS is Read-only,Volatile, and all reset to 0000_0000_0001_0000h.
- **0832 LVT Timer** (p180, confirmed at 240 dpi): 63:18 Reserved, 17 Mode, 16 Mask, 15:13 Reserved, 12 DS, **11:8 Reserved**, 7:0 Vector.
- **0833 Thermal** (p180), **0834 PerfMon** (p181), **0835/0836 LINT0/LINT1** (p181; RmtIRR Read-only,Volatile, bit 13 Reserved) and **0837 Error** (p182): same layout as MMIO, including MsgType at 10:8.

**Timer MSRs** (p182-183)
- **0838 TimerInitialCount:** 31:0, Read-write.
- **0839 TimerCurrentCount:** Read,Error-on-write,Volatile.
- **083E TimerDivide:** 3:0, same valid values as MMIO.

**083F SelfIPI** (p183): 7:0 Vector, **Write-only,Error-on-read**. "Semantically identical to an IPI sent via the ICR, with a Destination Shorthand of Self, Trigger Mode equal to Edge, and a Delivery Mode equal to Fixed."

**AMD extended MSRs**
- **0840 ExtendedApicFeature** (p183): Reset 0000_0000_0004_0007h. Access Read-only,Error-on-write.
- **0841 ExtendedApicControl** (p184): 63:3 Reserved; bit 2 ExtApicIdEn, bit 1 SeoiEn, bit 0 IerEn. Read-write.
- **0842 SpecificEndOfInterrupt** (p184): 7:0 EoiVec, Read-write.
- **0848 InterruptEnable0** (p184): Reset 0000_0000_FFFF_FFFFh. **Bits 31:16 only**; bits 15:0 Reserved.
- **0849-084F InterruptEnable7..1** (p184): 31:0, Reset FFFF_FFFFh.
- **0850-0853 ExtendedInterruptLvtEntries** (p185): Reset 0001_0000h. **DS is Read-write,Volatile.**

**MSRs not defined** in 0800-08FF: 0800-0801, 0804-0807, 080C, **080E** (DFR; APICE0 is "Only supported in xAPIC mode"), 0829-082F, **0831**, 083A-083D, 0843-0847 and 0854-08FF. The PPR says nothing about accessing them. The only general "not listed" statement (p458) is about SB-RMI sideband access, not RDMSR.

**Enabling x2APIC mode**
- 2.1.11.2.1.1 (p52): x2APIC presence is detected via `FeatureIdEcx[X2APIC]` and enabled via `APIC_BAR[x2ApicEn]`. "Reset forces the APIC and X2APIC disabled."
- MSR0000_001B bit 10 x2ApicEn (p121/i120): Read-write, Reset 0. "1=Extended Local APIC is enabled in x2APIC mode. **Clearing this bit after it has been set requires ApicEn to be cleared as well.**"

**ID width**
- x2APIC: 32 bits (MSR 0802 [31:0]; ICR destination 63:32).
- MMIO: 8 bits (APIC20 [31:24]).
- ExtApicIdEn chooses 8-bit versus 4-bit physical matching (p64, p184).

**Implications**
- Emulate these x2APIC-specific rules:
  - EOI accepts only 0.
  - ESR accepts writes of 0 only.
  - LDR is read-only, and MSR 080E does not exist.
  - The ICR has no DS bit and no remote-read field.
  - SelfIPI is write-only.
  - The x2APIC timer LVT has no MsgType.
- MSRs 0840-0853 are AMD extensions. Hide them when the guest version bit 31 = 0.
- The PPR names #GP only for the case x2ApicEn=0. The kind of error for reads or writes while X2APICEN=1 is not stated (U6).

### Q3. CPUID Fn0000_0001 ECX[21], EDX[9], EBX[31:24] and the CPUID_Features override

**`CPUID_Fn00000001_ECX` (FeatureIdEcx)** (p68/i67): "Read-only." "These values can be over-written by Core::X86::Msr::CPUID_Features."
- **Bit 21 X2APIC: "Read-only. Reset: X. x2APIC capability."** It has no Fixed value.
- Bit 24: Reserved. This is the SDM's TSC-deadline position.
- Bit 17 PCID: Fixed,0.
- Bit 3 Monitor: Reset `!HWCR[MonMwaitDis]`.
- Bits 27 OSXSAVE, 25 AES and 1 PCLMULQDQ: Reset X.

**`MSRC001_1004` CPUID_Features** (p240/i239 to p241/i240): "Read-write." with no register-level reset. Per-thread.
- "[63:32] provides control over values read from FeatureIdEcx; [31:0] provides control over values read from FeatureIdEdx."
- **Bit 53 X2APIC: "Read-write. Reset: Core::X86::Cpuid::FeatureIdEcx[X2APIC]. See Core::X86::Cpuid::FeatureIdEcx[X2APIC]."** This override has **no condition**.
- Other bits do have conditions:
  - Bit 59 OSXSAVE applies only if CR4[OSXSAVE].
  - Bits 57 AES and 33 PCLMULQDQ apply only if the reset value is 1.
  - Bit 35 Monitor applies only if `~HWCR[MonMwaitDis]`.
  - **Bit 9 APIC: "Modifies FeatureIdEdx[APIC] only if APIC_BAR[ApicEn]."**
- Bit 56 is Reserved.

**`CPUID_Fn00000001_EDX` bit 9 APIC** (p69/i68): "APIC exists and is enabled. Read-only. Reset: X. Core::X86::Msr::APIC_BAR[ApicEn]."
- `CPUID_Fn80000001_EDX` bit 9 matches it: "Reset is APIC_BAR[ApicEn]" (p88/i87).
- Its override is `CPUID_ExtFeatures` (MSRC001_1005) bit 9, which has no condition (p244/i243).

**`CPUID_Fn00000001_EBX` (FeatureIdEbx)** (p67/i66): "Read-only."
- **Bits 31:24 LocalApicId: "Read-only. Reset: XXh. Initial local APIC physical ID."**
- Bits 23:16 LogicalProcessorCount: Fixed,XXh = `0FFh & (SizeId[NC]+1)`.
- Bits 15:8 CLFlush: Fixed,08h.

**Implications**
- The host's X2APIC bit can be changed by firmware through MSRC001_1004[53], and the PPR gives no numeric default. Use the live CPUID value.
- An x2APIC guest must see ECX[21]=1 and ECX[24]=0.
- Guest EDX[9] in leaves 1 and 8000_0001 should follow the guest's `APIC_BAR[ApicEn]`, so it changes at runtime.
- The PPR describes EBX[31:24] only as an 8-bit initial ID. How it relates to a 32-bit ID above 255 is unresolved (U12).

### Q4. CPUID Fn8000_000A

- **EAX** (p100/i99): "Read-only. Reset: Fixed,0000_0001h." Bits 7:0 SvmRev = Fixed,01h.
- **EBX** (p100): "Read-only,Volatile. Reset: 0000_8000h." Bits 31:0 NASID.
- **ECX: the PPR has no table.** p100 ends with EBX and p101 starts with EDX; a search found no "SvmRevFeatIdEcx".
- **EDX (SvmRevFeatIdEdx)** (p101/i100, 240 dpi): "Read-only." and "This provides SVM feature information." There is no register-level reset. "F1" below means "Read-only. Reset: Fixed,1".
  - 31 EnhancedShutdownIntercept F1
  - 30 IdleHltIntercept F1
  - 29 GuestBusLockThreshold F1
  - 28 VmcbAddrChkChg F1
  - **27 ExtLvtOffsetFaultChg F1:** "Read/Write fault behavior for the extended LVT offsets (APIC addresses 0x500-0x530) changed to Read Allowed, Write #MVEXIT (trap)" [sic]
  - 26 IbsVirt F1
  - 25 NmiVirt F1
  - 24 Reserved
  - 23 HOST_MCE_OVERRIDE F1
  - 22 Reserved
  - 21 AllowNonWriteAbleGPT F1
  - 20 GuestSpecCtrl F1
  - 19 SupervisorShadowStack F1
  - **18 X2AVIC: "virtualized X2APIC. Read-only. Reset: 0. 1=Virtualized X2APIC is supported."**
  - 17 GMET F1
  - 16 vGIF F1
  - 15 V_VMSAVE_VMLOAD F1
  - 14 Reserved
  - **13 AVIC: "AMD virtual interrupt controller. Read-only. Reset: 0. 1=Support indicated for SVM mode virtualized interrupt controller; Indicates support for Core::X86::Msr::AvicDoorbell."**
  - 12 PauseFilterThreshold F1
  - 11 Reserved
  - 10 PauseFilter F1
  - 9 Reserved
  - 8 PerfCtrVirt F1
  - 7 DecodeAssists F1
  - 6 FlushByAsid F1
  - 5 VmcbClean F1
  - 4 TscRateMsr F1
  - **3 NRIPS: "Read-only. Reset: Fixed,1. NRIP Save."**
  - 2 SVML F1
  - 1 LbrVirt F1
  - **0 NP: "Read-only. Reset: Fixed,1. Nested Paging."**

**Comparison with the live CPU** (EDX=0xFEBFBDFF, ECX=0)
- The PPR Fixed,1 mask is **0xFEBB95FF**, and every bit in it is set live.
- Live bits beyond that mask are **11** (PPR: Reserved), **13** (AVIC, PPR "Reset: 0") and **18** (X2AVIC, PPR "Reset: 0").
- Reserved bits 9, 14, 22 and 24 read 0 live.
- "Reset: 0" without Fixed is only the value when hardware leaves reset, so bits 13 and 18 reading 1 does not contradict the PPR. The PPR does not say what sets them.
- The live bit 11 is undocumented in the PPR.
- ECX=0 cannot be checked against the PPR because no ECX table exists.

**Implications**
- Base AVIC/x2AVIC decisions on the live CPUID (both bits are 1 live), not on the PPR reset column. Keep NP (bit 0) and NRIPS (bit 3) as hard requirements; the PPR guarantees them.
- By bit 27, guest writes to APIC offsets 500-530 cause a #VMEXIT trap and reads are allowed.
- Any x2AVIC size or limit data from Fn8000_000A_ECX must come from the APM, not the PPR.

### Q5. AVIC doorbell MSR

**Present.** `MSRC001_011B` [AVIC Doorbell] (Core::X86::Msr::AvicDoorbell), p216/i215, read at 240 dpi:
- **"Write-only,Error-on-read. Reset: 0000_0000_0000_0000h."**
- Description: **"The ApicId is a physical APIC Id; not valid for logical APIC ID. Enable: (Core::X86::Cpuid::SvmRevFeatIdEdx[AVIC] == 1)."**
- Instance: `_ccd[1:0]_lthree0_core[7:0]_thread[1:0]` (per-thread).
- Bits 63:32: Reserved.
- **Bits 31:0 ApicId: "APIC ID [31:0]. Write-only,Error-on-read. Reset: 0000_0000h. The value written must be a valid physical APID_ID."** [sic]

**Other AVIC items in the PPR.** A search over the whole PDF for "AVIC" found only these pages:
- Fn8000_000A_EDX bits 13, 18 and 27 (p101).
- `MSRC001_0138` [Secure AVIC Control] (p217/i216): "Read-write. Reset: 0", per-thread. Bits 63:12 GuestApicBackingPagePtr ("4K aligned GPA"), bit 1 AllowedNmi, bit 0 SecureAvicEn. This is for SEV-SNP guests.

**Not in the PPR:** what a doorbell write does, what happens with an invalid ID, the AVIC VMCB fields, and the backing-page or physical/logical tables.

**Implication.** To wake another CPU's running guest, issue `WRMSR(0xC001_011B, target host physical (x2)APIC ID)` with the ID as a 32-bit value and the upper half 0.
- Do this only when the live CPUID AVIC bit is 1.
- Never RDMSR this MSR.
- Never pass a logical ID.
- Guest APIC ID equals host x2APIC ID here, so the target's own ID is the value to write. The doorbell's semantics come from the APM.

### Q6. Topology leaves and x2APIC ID reporting

**Fn0000_0001_EBX[31:24]:** the 8-bit initial APIC ID (p67).

**Fn0000_000B** (p73-75/i72-i74). Intro text: "specifies the hierarchy of logical cores from the SMT level through the processor socket level. Software determines the presence of CPUID Fn0000_000B if (EBX_x0[31:0] != 0) ... reads ... for ascending values of ECX until (EBX[LogProcAtThisLevel] == 0)."

| Subleaf | EAX CoreMaskWidth [4:0] | EBX LogProcAtThisLevel [15:0] | ECX |
|---|---|---|---|
| x00 | Reset `SMT ? 01h : 00h`. "Number of bits to shift ExtendedApicId right to get unique topology ID of the next instance of the current level type." | Reset `SMT ? 2 : 0001h`. "Number of threads in a core." | Fixed,0000_0100h: LevelType 01h Thread; valid values 00h Invalid, 01h Thread, 02h Processor, FFh-03h Reserved |
| x01 | Reset XXXXXb. "ExtendedApicId right shift value." | Reset XXXXh. "Number of logical cores in processor socket." | Fixed,0000_0201h: LevelType 02h Processor |
| x02 | Fixed,0000_0000h. "Zero indicates no more levels." | Reset 0000h | Fixed,0000_0002h: LevelType 00h |

- **EDX** (no subleaf suffix): **bits 31:0 ExtendedLocalApicId, "Read-only. Reset: XXXX_XXXXh. Extended APIC_ID."**

**Fn8000_001E** (p110/i109). "If `FeatureExtIdEcx[TopologyExtensions]` == 0 then CPUID Fn8000001E_E[D,C,B,A]X are reserved. If `(APIC_BAR[ApicEn] == 0)` then `ExtApicId[ExtendedApicId]` is Reserved." TopologyExtensions is bit 22 of Fn8000_0001_ECX, Fixed,1 (p87).
- **EAX 31:0 ExtendedApicId:** "Read-only. Reserved." [sic]
  - **Reset:** `(ApicEn && x2ApicEn) ? Msr::APIC_ID[ApicId[31:0]] : ApicEn ? {00_0000h, Apic::ApicId[ApicId]} : 0000_0000h`
- **EBX:** bits 15:8 ThreadsPerCore (the thread count is ThreadsPerCore+1); bits 7:0 CoreId ("unique per-socket logical core unit ID").
- **ECX:** bits 10:8 NodesPerProcessor (0h = 1 node, 7h-1h Reserved); bits 7:0 NodeId, Fixed,XXh.
- **EDX: not defined.** p111 starts with Fn8000_001F.

**Fn8000_0026**, subleaves x00-x03 (p119-121/i118-i120). Intro text: "...from the SMT level through the processor socket level ... read ... until (EBX[LogProcAtThisLevel] == 0). Note: While CPUID Fn8000_0026 is a preferred superset to CPUID_Fn0000000B, CPUID_Fn0000000B information is valid for software for the supported levels on AMD."
- **EAX:**
  - Bit 31 AsymmetricCores: Fixed,X.
  - Bit 30 HeterogeneousCoreTopology: Fixed,0.
  - Bit 29 EfficiencyRankingAvailable: Fixed,0.
  - Bits 4:0 CoreMaskWidth: Fixed,XXh. This is the shift applied to `ExtCpuTopologyEdx[ExtendedLocalApicId]`.
- **EBX:**
  - Bits 31:28 CoreType: 0h Performance, 1h Efficiency.
  - Bits 27:24 NativeModelId: 0h = Zen5.
  - Bits 23:16 ProcessorPowerEfficiencyRanking.
  - Bits 15:0 LogProcThisLevel.
- **ECX:**
  - Bits 15:8 LevelType: 01h Core, 02h Complex, 04h Socket. Values 00h, 03h and 05h-FFh are Reserved.
  - Bits 7:0 EcxVal.
- **EDX** (no subleaf suffix): **bits 31:0 ExtendedLocalApicId, "Read-only. Reset: Fixed,XXXX_XXXXh."**

**Fn8000_0008_ECX (SizeId)** (p100):
- Bits 15:12 ApicIdSize, Reset Xh: "number of bits in the initial ApicId value that indicate thread ID within a package".
- Bits 11:0 NC: the package has NC+1 threads.

**2.1.11.2.1.3** (p52): "do not require contiguous ApicId assignments". The OS uses ApicIdSize for per-core masks. ApicIdSize gives the theoretical maximum core count; NC gives the actual count.

**Where the 32-bit x2APIC ID appears:** MSR 0802 (p174), Fn0000_000B_EDX, Fn8000_0026_EDX, and Fn8000_001E_EAX (only while the APIC is in x2APIC mode).

**Implication.** Guest APIC IDs equal host x2APIC IDs, so for each vCPU the following must all match the guest's MSR 0802 value:
- `0B.EDX`
- `8000_0026.EDX`
- `8000_001E.EAX`, computed from the **guest's** APIC_BAR state: the 32-bit ID in x2APIC mode, the 8-bit ID in xAPIC mode, 0 when the APIC is disabled.
- `0000_0001.EBX[31:24]`

Pass through the per-thread level data. Note that the level-type codes differ between leaf 0B (Thread, Processor) and leaf 8000_0026 (Core, Complex, Socket).

### Q7. LAPIC timer implementation

- **2.1.10 Timers** (p43/i42): "Each core includes the following timers. **These timers do not vary in frequency regardless of the current P-state or C-state.** ... The APIC timer (TimerInitialCount and TimerCurrentCount), which **increments at the rate of 2xCLKIN; the APIC timer may increment in units of between 1 and 8.**" CLKIN appears nowhere else in the PDF.
- **2.1.11.2.1.13 APIC Timer Operation** (p55/i54):
  - "The local APIC contains a 32-bit timer, controlled by TimerLvtEntry, TimerInitialCount, and TimerDivideConfiguration."
  - "The **processor bus clock** is divided by Div[3:0] to obtain a time base for the timer."
  - A write to the initial count copies it to the current count, which "is decremented at the rate of the divided clock".
  - At 0, an interrupt is raised with TimerLvtEntry[Vector]. In periodic mode the count reloads.
  - "If TimerLvtEntry[Mask] is set, timer interrupts are not generated."
- **Divider:** see the table under Q1 (p64 and p183). "Div[2] is unused."
- **ARAT:** `CPUID_Fn00000006_EAX` = Fixed,0000_0004h. Bit 2 ARAT is "Read-only. Reset: Fixed,1. 1=Indicates support for APIC timer always running feature." (p70/i69).
- **No TSC-deadline support:** Fn0000_0001_ECX[24] is Reserved (p68), timer LVT bits 31:18 and 63:18 are Reserved (p62, p180), and the PPR has no MSR 06E0.
- **Legacy timer-tick note:** 2.1.11.2.1.10 (p53-54) is about a *legacy INTR/PIC* timer tick that is latched as ExtInt. It gives "a 50 percent probability of spurious interrupts" and does not concern the LAPIC timer.
- **Wording quirks:** 2.1.10 says the timer "increments" while 2.1.11.2.1.13 says it is "decremented". The count may step in units of 1 to 8.

**Implication.** Mirror the divide value and counts 1:1. The count rate does not depend on P-state or C-state, but its absolute frequency is not given (CLKIN and the processor bus clock are undefined), so calibrate at runtime, for example against the TSC. Current-count reads may jump by up to 8. Do not offer TSC-deadline mode.

### Q8. INIT/RESET effects, APIC_BAR, LINT wiring

**`MSR0000_001B` APIC_BAR** (p121/i120): per-thread, with **no register-level access or reset row**.
- 63:48: Reserved.
- **47:12 ApicBar:** Read-write, Reset 0_000F_EE00h. "physical address [47:12], for the APICXX register set in xAPIC mode".
- **11 ApicEn:** Read-write, Reset 0. "1=Local APIC is enabled in xAPIC mode".
- **10 x2ApicEn:** Read-write, Reset 0. See Q2 for its full text.
- 9: Reserved.
- **8 BSC:** Read-write,Volatile, Reset X. 1 means this is the boot core of the BSP.
- 7:0: Reserved.
- The computed reset value is 0000_0000_FEE0_0000h, plus 0100h on the BSC.

**Reset and INIT**
- "Reset forces the APIC and X2APIC disabled." (p52)
- **2.1.11.2.1.15 State at Reset** (p55): "At power-up or reset, the APIC is hardware disabled (APIC_BAR[ApicEn] == 0) so only SMI, NMI, INIT, and ExtInt interrupts may be accepted. The APIC can be software disabled through SVR[APICSWEn]. The software disable has no effect when the APIC is hardware disabled. **When a processor accepts an INIT interrupt, the APIC is reset as at power-up, with the exception that: ApicId is unaffected. Pending APIC register writes complete.**"
- Register reset values are listed in section 3. SMM entry masks INTR, NMI, SMI and INIT (p44, p50).

**LINT wiring**
- The only statements are "Legacy local interrupts from the IO hub (INTR and NMI)" in the list of interrupt sources (2.1.11.2.1, p51/i50) and "LINT: Local interrupt" (Table 13, p36).
- **The PPR does not map INTR or NMI to LINT0 or LINT1.** Both LINT LVTs reset to masked Fixed (0001_0000h).

**Implications**
- On guest INIT, reset the vLAPIC to the reset values in this sheet but keep the APIC ID.
- The PPR does not settle whether INIT clears APIC_BAR's ApicEn or x2ApicEn (U9).
- Reject x2ApicEn 1→0 unless ApicEn is cleared in the same write. The PPR does not name the fault.
- Get LINT wiring from the platform (ACPI MADT), not from the PPR.

### Q9. VM_CR, SYS_CFG, HWCR (interrupt-relevant parts only)

**`MSRC001_0114` VM_CR** (p215/i214): "Reset: 0000_0000_0000_0000h." Per-thread.
- 63:5: Reserved.
- 4 SvmeDisable: "Configurable". Reset 0. "Attempting to set this field when EFER[SVME] == 1 causes a #GP fault, regardless of the state of Lock."
- 3 Lock: Read-only,Volatile. Reset 0.
- **2: Reserved.**
- **1 InterceptInit** (this is the APM's R_INIT; the PPR does not use that name): **"Read-write,Volatile. Reset: 0. 0=INIT delivered normally. 1=INIT translated into a SX interrupt. This bit controls how INIT is delivered in host mode. This bit is set by hardware when the SKINIT instruction is executed."**
- **0: Reserved.**
- Enabling SVM (2.1.2.1.1, p37) requires SvmeDisable=0, Lock=1 and SvmLockKey=0.

**`MSRC001_0010` SYS_CFG** (p202/i201): Reset 0, **per core**. "If SecureNestedPagingEn is set, writes to this register are ignored."
- Fields: bit 26 HMKEE, bit 25 VmplEn, bit 24 SecureNestedPagingEn, bit 23 SMEE, and the MTRR/TOM2 controls in bits 22:18.
- Bits 17:0 are Reserved.
- **No interrupt or APIC fields.**

**`MSRC001_0015` HWCR** (p203-204/i202-i203): "Reset: 0000_0000_0100_6010h." Per-thread.
- The only interrupt-relevant statement: **bit 0 SmmLock (Read,Write-1-only, Reset 0, Init BIOS,1): "1=SMM code in the ASeg and TSeg range and the SMM registers are Read-only and SMI interrupts are not intercepted in SVM."**
- Bit 35 CpuidFltEn: when 1, CPUID at CPL > 0 outside SMM raises #GP.
- No APIC fields.

**Implications**
- Setting VM_CR bit 1 on a logical CPU turns host-mode INIT into #SX. Hardware sets it on SKINIT. VM_CR is per-thread, so program it on every logical CPU.
- SMIs cannot be intercepted once SmmLock is set, which is another reason never to let guest-controlled SMI messages reach hardware.

## 3. Register table (MMIO and x2APIC)

MMIO is the 2.1.11.2.2 table (p56-65). x2APIC is the MSR table (p174-185); its access column applies when X2APICEN=1, and otherwise every MSR is Error-on-read,Error-on-write. "RO" = Read-only, "RW" = Read-write, "Vol" = Volatile, "EoW" = Error-on-write, "EoR" = Error-on-read. A dash means the register or access type is not defined.

| Offset | Name (Apic::) | x2APIC MSR | MMIO access; reset | x2APIC access; reset | Key fields | Pages (MMIO / MSR) |
|---|---|---|---|---|---|---|
| 020 | ApicId | 0802 APIC_ID | RO; [31:24]=XXh | RO,EoW; XXXX_XXXXh | 8-bit ID in MMIO; 32-bit ID in x2APIC | 56 / 174 |
| 030 | ApicVersion | 0803 | RO; [31]=1, [24]=Fixed,1, [23:16]=XXh, [7:0]=10h | RO,EoW; same values ([24] shown as "Reset: 1") | 31 ExtApicSpace; 24 DirectedEoiSupport; 23:16 MaxLvtEntry; 7:0 Version | 56 / 175 |
| 080 | TaskPriority | 0808 TPR | RW; 0 | RW,Vol; 0 | 7:0 Priority | 56 / 175 |
| 090 | ArbitrationPriority | 0809 | RO,Vol; 0 | RO,EoW,Vol; 0 | 7:0 | 56 / 175 |
| 0A0 | ProcessorPriority | 080A | RO,Vol; 0 | RO,EoW,Vol; 0 | 7:0 | 56 / 175 |
| 0B0 | EndOfInterrupt | 080B EOI | Write-only; no reset; 31:0 Reserved | Write-0-only,EoR,Error-on-write-1; 0 | write 0 only (x2APIC) | 57 / 175 |
| 0C0 | RemoteRead | - | RO; 0 ("not supported") | - | - | 57 / - |
| 0D0 | LocalDestination | 080D LDR | RW,Vol; 0 | RO,EoW; 0 | MMIO 31:24; x2APIC 31:16 cluster + 15:0 logical | 57 / 176 |
| 0E0 | DestinationFormat | none (080E undefined) | RW; F000_0000h; "Only supported in xAPIC mode" | - | 31:28 Format (0h cluster, Fh flat) | 57 / - |
| 0F0 | SpuriousInterruptVector | 080F SVR | per field; 0000_00FFh | RW; field resets | 9 FocusDisable, 8 APICSWEn, 7:0 Vector (FFh); **31:10 / 63:10 Reserved (no bit 12)** | 57 / 176 |
| 100-170 | InService | 0810-0817 ISR | RO,Vol; 0 | RO,EoW,Vol; 0 | first 16 bits Reserved | 58 / 177 |
| 180-1F0 | TriggerMode | 0818-081F TMR | RO,Vol; 0 | RO,EoW,Vol; 0 | 1 = level | 58-59 / 177 |
| 200-270 | InterruptRequest | 0820-0827 IRR | RO; 0 | RO,EoW,Vol; 0 | first 16 bits Reserved | 59-60 / 177 |
| 280 | ErrorStatus | 0828 ESR | fields RW, 0; a write loads error state | Read,Write-0-only,Error-on-write-1,Vol; 0 | 7, 6, 5, 3, 2 | 60 / 178 |
| 300 / 310 | InterruptCommandLow / High | 0830 (64-bit) | RW except RemoteRdStat and DS (RO); 0 | RW; 0; bits 17:16 and 13:12 Reserved | 19:18 shorthand, 15 TM, 14 Level, 12 DS (MMIO only), 11 DM, 10:8 MsgType (3h Reserved), 7:0 Vector; destination is 31:24 (MMIO high) or 63:32 (x2APIC) | 61-62 / 179 |
| 320 | TimerLvtEntry | 0832 | RW (DS RO,Vol); 0001_0000h | RW (DS RO,Vol); 0001_0000h | 17 Mode, 16 Mask, 12 DS, **10:8 MsgType (MMIO) vs 11:8 Reserved (x2APIC)**, 7:0 | 62 / 180 |
| 330 | ThermalLvtEntry | 0833 | same; 0001_0000h | same | 16, 12, 10:8, 7:0 | 62 / 180 |
| 340 | PerformanceCounterLvtEntry | 0834 | same; 0001_0000h | same | 16, 12, 10:8, 7:0 | 62 / 181 |
| 350 / 360 | LVTLINT[0] / [1] | 0835 / 0836 | same; 0001_0000h | same | 16 Mask, 15 TM, 14 RmtIRR (RO,Vol), **13 Reserved**, 12 DS, 10:8, 7:0 | 63 / 181 |
| 370 | ErrorLvtEntry | 0837 | same; 0001_0000h | same | 16, 12, 10:8, 7:0 | 63 / 182 |
| 380 | TimerInitialCount | 0838 | RW,Vol; 0 | RW; 0 | 31:0 | 63 / 182 |
| 390 | TimerCurrentCount | 0839 | RO,Vol; 0 | Read,EoW,Vol; 0 | 31:0 | 63 / 182 |
| 3E0 | TimerDivideConfiguration | 083E | RW; 0 | RW; 0 | 3:0 Div (valid: 0-3, 8-B) | 64 / 183 |
| - | - | 083F SelfIPI | - | Write-only,EoR; 0 | 7:0 Vector | - / 183 |
| 400 | ExtendedApicFeature | 0840 | RO; 0004_0007h | RO,EoW; 0004_0007h | ExtLvtCount=4, ExtApicIdCap, SeoiCap, IerCap | 64 / 183 |
| 410 | ExtendedApicControl | 0841 | RW; 0 | RW; 0 | 2 ExtApicIdEn, 1 SeoiEn, 0 IerEn; **no directed-EOI enable** | 64 / 184 |
| 420 | SpecificEndOfInterrupt | 0842 | RW; 0 | RW; 0 | 7:0 EoiVec | 65 / 184 |
| 480-4F0 | InterruptEnable | 0848, 0849-084F | RW; FFFF_FFFFh | RW; FFFF_FFFFh (0848 implements only 31:16) | enable bits, used when IerEn=1 | 65 / 184 |
| 500-530 | ExtendedInterruptLvtEntries | 0850-0853 | RW (DS RO,Vol); 0001_0000h | RW (**DS RW,Vol**); 0001_0000h | 16, 12, 10:8, 7:0 (500 IBS, 510 threshold, 520 deferred) | 65 / 185 |
| - | APIC_BAR | MSR 001B | - | per field (no register-level row) | 47:12 = 0_000F_EE00h, 11 ApicEn=0, 10 x2ApicEn=0, 8 BSC=X | - / 121 |

## 4. Cross-reference chains followed

1. **X2APIC capability.** `FeatureIdEcx[X2APIC]` (p68, "Reset: X"; overridable by CPUID_Features) → `MSRC001_1004[53]` (p240, "Reset: FeatureIdEcx[X2APIC]") → back to p68. **The chain is circular, so no concrete value is ever given.**
2. **APIC present/enabled bit.** `FeatureIdEdx[APIC]` (p69) resets to `APIC_BAR[ApicEn]` (p121). `CPUID_Features[9]` changes it only if ApicEn (p241). `FeatureExtIdEdx[APIC]` (p88) is overridden by `CPUID_ExtFeatures[9]` (p244).
3. **AVIC doorbell.** `SvmRevFeatIdEdx[AVIC]` (p101) says "Indicates support for AvicDoorbell" → `MSRC001_011B` (p216), whose enable condition is `SvmRevFeatIdEdx[AVIC] == 1`.
4. **x2APIC enable and access.** 2.1.11.2.1.1 (p52) → `FeatureIdEcx[X2APIC]` (p68) and `APIC_BAR[ApicEn, x2ApicEn]` (p121) → X2APICEN definition (p37, p486) → every x2APIC MSR access type (p174-185). 2.1.11.2.1.2 (p52) adds "GP-read-write" when x2ApicEn=0.
5. **Extended APIC space.** `ApicVersion[ExtApicSpace]` (p56/p175) → `ExtendedApicFeature` (p64/p183) → ExtLvtCount → `ExtendedInterruptLvtEntries` (p65/p185). The matching CPUID bit is `FeatureExtIdEcx[ExtApicSpace]` (p87, Fixed,1), overridden by `CPUID_ExtFeatures[35]` (p243).
6. **SEOI and IER.**
   - SEOI: SeoiCap → `ExtendedApicControl[SeoiEn]` (p64/p184) → `SpecificEndOfInterrupt` (p65/p184).
   - IER: IerCap → IerEn → `InterruptEnable` (p65/p184) → interrupt masking (2.1.11.2.1.8, p53) and lowest-priority arbitration (2.1.11.2.1.11, p54).
7. **Timer.** 2.1.10 (p43) → counts (p63/p182). 2.1.11.2.1.13 (p55) → `TimerLvtEntry` (p62/p180) and `TimerDivideConfiguration` (p64/p183). ARAT is on p70.
8. **LVT message types.** Every LVT MsgType row says "See 2.1.11.2.1.14" (p62-65, p180-185) → p55, which allows 000b, 010b, 100b and 111b.
9. **APIC ID topology.**
   - 2.1.11.2.1.3 (p52) → SizeId ApicIdSize and NC (p100), with FeatureIdEbx[LocalApicId] on p67.
   - TopologyExtensions (p87) → ExtApicId (p110), which depends on APIC_BAR (p121), APIC_ID (p174) and Apic::ApicId (p56).
   - CoreMaskWidth in leaves 0B (p73-75) and 8000_0026 (p119-121) → ExtendedLocalApicId.
10. **Revision guide.** 1.6.1 (p32) → "revision guide in 1.2" → Table 1 (p12), which lists no revision guide. **This is a dead reference.** MSRC001_0140 and 0141 (p217) also say "See the Revision Guide".
11. **SVM enable.** VM_CR SvmeDisable and Lock (p215) → SvmLockKey (p216) → 2.1.2.1 (p37).
12. **SMM lock.** `HWCR[SmmLock]` (p204) → 2.1.11.1.10 Locking SMM (p51).

## 5. Internal inconsistencies in the PPR

- **D1. Timer LVT message type.** MMIO APICx320 has MsgType at bits 10:8 (p62), but MSR 0832 marks bits 11:8 Reserved (p180, 240 dpi). The generalized-LVT sentence covers "ThermalLvtEntry to LVTLINT", a range that does not include the timer.
- **D2. DirectedEoiSupport.** The MMIO copy says "Reset: Fixed,1" but prints only the "0=not supported" legend (p56, 240 dpi). The x2APIC copy says "Reset: 1" with both legends (p175). No enable bit exists anywhere: SVR bits 31:10 (MMIO) and 63:10 (x2APIC) are Reserved, and ExtApicControl bits 31:3 and 63:3 are Reserved.
- **D3. Extended LVT DS.** Read-only,Volatile in MMIO (p65) but Read-write,Volatile in x2APIC (p185).
- **D4. IRR volatility.** MMIO IRR is "Read-only" without Volatile (p59), while ISR, TMR and the x2APIC IRR are Volatile.
- **D5. ESR write semantics.** MMIO says a write of any value loads and clears the error state (p60). x2APIC says Write-0-only with Error-on-write-1 (p178).
- **D6. Fn8000_001E_EAX field text.** The field reads "Reserved." yet has a full reset expression (p110).
- **D7. Fn8000_000A_EDX bit 11.** Reserved in the PPR (p101) but set on the live CPU.
- **D8. Timer counting direction.** 2.1.10 says "increments" (p43); 2.1.11.2.1.13 says "decremented" (p55).
- **D9. Revision guide.** 1.6.1 refers to a revision guide that Table 1 does not list.
- **D10. Access and volatility mismatches between MMIO and x2APIC copies.**
  - TPR: Read-write in MMIO, Read-write,Volatile in x2APIC.
  - TimerInitialCount: Read-write,Volatile in MMIO, Read-write in x2APIC.
  - LDR: Read-write,Volatile in MMIO, Read-only in x2APIC.
  - SVR APICSWEn: Read-write,Volatile in MMIO, Read-write in x2APIC.

## 6. UNRESOLVED: things the PPR does not define

- **U1. Numeric values that vary.** MaxLvtEntry (APIC30[23:16] = "XXh"), the actual APIC and x2APIC IDs, ApicIdSize, NC, the subleaf-x01 CoreMaskWidth and LogProcAtThisLevel values, and the Fn8000_0026 values. Read them from the live system.
- **U2. Enabling directed EOI / EOI-broadcast suppression.** The PPR advertises support but never says how to enable it, or whether the physical LAPIC honours SVR[12]. SVR[12] is Reserved in the PPR.
- **U3. CPUID Fn8000_000A_ECX.** There is no table at all, so AVIC/x2AVIC extension fields and limits are undocumented here.
- **U4. What sets AVIC and X2AVIC to 1.** Fn8000_000A_EDX bits 13 and 18 are "Reset: 0" but read 1 live. The PPR doesn't say what sets them, and doesn't explain live bit 11.
- **U5. AVIC doorbell behaviour.** The PPR does not say what a write does, what happens with an invalid APIC ID, or which error a read causes (only "Error-on-read"). It also omits the AVIC VMCB fields and the backing-page or physical/logical tables.
- **U6. x2APIC-mode access rules.**
  - The error kind for Error-on-read/Error-on-write while X2APICEN=1 (only the x2ApicEn=0 case is "GP-read-write").
  - What happens when writing reserved bits of x2APIC MSRs, which the conventions only call "write-as-read".
  - How undefined MSRs such as 080E, 0831, 0829-082F and 0854-08FF behave.
  - How the MMIO window behaves while x2APIC mode is on (only "normal memory when APIC disabled" is stated).
- **U7. Timer frequency and behaviour.** The absolute frequency is unknown: "2xCLKIN" and the "processor bus clock" are undefined. Also undocumented: the exact 1-8 step granularity, what writing an initial count of 0 does, and what changing mode or divide while running does.
- **U8. LINT wiring.** The PPR does not say which of LINT0/LINT1 carries INTR and which carries NMI.
- **U9. INIT and APIC_BAR.** Whether INIT changes APIC_BAR[ApicEn] or [x2ApicEn] is unclear ("APIC is reset as at power-up" is ambiguous). Beyond "clearing x2ApicEn requires clearing ApicEn", the PPR doesn't say which APIC_BAR transitions are illegal or what fault they raise.
- **U10. Stepping and product names.** The PPR never maps B0 to CPUID stepping 0 (the revision guide is absent), and it never names the Ryzen 9 9900X.
- **U11. x2APIC logical addressing.** How the read-only x2APIC LDR is derived from the x2APIC ID. The x2APIC broadcast ID (FFFF_FFFFh) and x2APIC logical-mode matching rules are not stated; the 2.1.11.2.1.4/.5 text is written for 8-bit xAPIC.
- **U12. Leaf 1 APIC ID above 255.** How Fn0000_0001_EBX[31:24] (8-bit) relates to a 32-bit x2APIC ID greater than 255.
- **U13. Fn8000_0026 SMT level.** Its LevelType list has no Thread/SMT code (only Core, Complex and Socket), although the intro says the hierarchy starts "from the SMT level".
