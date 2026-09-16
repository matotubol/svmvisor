# APM Vol.2 Chapter 16 (Local APIC / x2APIC): fact sheet

Reviewer: read-only manual review, 2026-09-16. Method: every rule below was read from rendered page
images (PyMuPDF, 144 dpi pages plus 230 dpi top/bottom halves for dense tables). Text extraction was
used only to locate pages. Repo summaries were used only to locate pages and are **not** evidence.
No source, doc or git state was changed.

Images:
- `work/x2avic-manual-2026-09-16/apm-x2apic/pages/` (V2)
- `.../pages-vol3/` (V3)
- `.../pages-ppr/` (PPR)

## 0. Sources

| Key | Document | Revision | SHA256 (printed by render script) | Applicability | Pages read (PDF, one-based) |
|---|---|---|---|---|---|
| **V2** | AMD64 Architecture Programmer's Manual Vol. 2, pub. 24593 (cover PDF 1) | Rev 3.44, March 2026 | `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c` (matches expected), 845 pages | Architectural; the **primary authority** for every answer | 1; 17–18 (TOC); 689–726 |
| **V3** | AMD64 APM Vol. 3, pub. 24594 | Rev 3.37, July 2025 | `c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4`, 712 pages | Architectural. V2 §16.11 delegates to it: "See APM volume 3 for more information on the WRMSR and RDMSR instructions" | 1, 473, 546, 547, 658, 659 |
| **PPR** | Processor Programming Reference for AMD Family 1Ah Model 44h, Revision B0, pub. 57896 | Rev 3.00, Aug 28 2024 | `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`, 486 pages | **Product-specific** (Fam 1Ah Model 44h B0 only). V2 §16.4.1/§16.4.4 delegate timer clock and thermal details to "the BKDG or PPR applicable to your product". This review did **not** verify that this is the execution host's exact product. Used only as supplementary or conflicting evidence, never to override V2. | 1, 6, 26–28, 36, 37, 43, 51–65, 68, 121, 174–185, 485, 486 |

Chapter 16 spans V2 PDF 689–725 (index 688–724), printed pp. 627–663. PDF 726 is Chapter 17 (printed 664).

Page-number mapping, taken from the footer of every page image:
- **V2, chapter 16:** printed = PDF − 62; index = PDF − 1.
- **PPR:** printed = PDF on every page read.
- **V3:** RDMSR printed 438 = PDF 473; WRMSR printed 511–512 = PDF 546–547; CPUID Fn1 printed 624–625 = PDF 658–659.

Citation form: `V2 p630/PDF692/i691` = printed page / one-based PDF page / zero-based index.

### Chapter 16 page map (all read)

| Printed | PDF | Index | Content |
|---|---|---|---|
| 627 | 689 | 688 | Ch.16 intro, Fig 16-1 |
| 628 | 690 | 689 | 16.1, **Table 16-1** (interrupt sources and message types), 16.2 |
| 629 | 691 | 690 | 16.2 cont., 16.3.1 (AE, software disable rules) |
| 630 | 692 | 691 | **Fig 16-2** APIC_BASE, 16.3.2 |
| 631 | 693 | 692 | **Table 16-2** (register values after reset and INIT). The whole table is on this page. |
| 632 | 694 | 693 | 16.3.3 ID (Fig 16-3), 16.3.4 Version (Fig 16-4) |
| 633 | 695 | 694 | Version text, 16.3.5 ExtFeature (Fig 16-5), 16.3.6 |
| 634 | 696 | 695 | Fig 16-6 ExtControl, 16.4 local interrupts |
| 635 | 697 | 696 | **Fig 16-7** general LVT format and field text |
| 636 | 698 | 697 | TGM/M/TMM text, **16.4.1 timer**, Fig 16-8 |
| 637 | 699 | 698 | Figs 16-9 CCR, 16-10 ICR(count), 16-11 DCR |
| 638 | 700 | 699 | **Table 16-3**, 16.4.2 LINT (Fig 16-12), 16.4.3 perf (Fig 16-13) |
| 639 | 701 | 700 | 16.4.4 thermal (Fig 16-14), 16.4.5 extended, 16.4.6 error (Fig 16-15) |
| 640 | 702 | 701 | ESR (Fig 16-16), 16.4.7 spurious |
| 641 | 703 | 702 | **Fig 16-17 SVR**, 16.5 IPI |
| 642 | 704 | 703 | **Fig 16-18 ICR**, MT list |
| 643 | 705 | 704 | MT list cont., DM/DS/L/TGM/RRS/DSH |
| 644 | 706 | 705 | DSH cont., DES, Fig 16-19 RRR, **Table 16-4** |
| 645 | 707 | 706 | 16.6.1 destination, Fig 16-20 LDR |
| 646 | 708 | 707 | Fig 16-21 DFR, flat/cluster, 16.6.2 |
| 647 | 709 | 708 | Fig 16-22 APR, focus, **16.6.3 acceptance** |
| 648 | 710 | 709 | IRR/ISR/TMR text, Fig 16-23 |
| 649 | 711 | 710 | Figs 16-24 ISR, 16-25 TMR |
| 650 | 712 | 711 | TMR map, 16.6.4 priorities, Fig 16-26 TPR |
| 651 | 713 | 712 | TPR/PPR text, Fig 16-27 |
| 652 | 714 | 713 | EOI (Fig 16-28), 16.7, 16.7.1 SEOI |
| 653 | 715 | 714 | Fig 16-29, 16.7.2 IER (Fig 16-30) |
| 654 | 716 | 715 | IER reset, **16.8 x2APIC**, 16.9 |
| 655 | 717 | 716 | **Fig 16-31**, **Table 16-5**, transition #GP rules |
| 656 | 718 | 717 | **Fig 16-32** transitions, 16.9.1 |
| 657 | 719 | 718 | **16.10 init**, **16.11 access**, 16.11.1 |
| 658 | 720 | 719 | **Table 16-6** |
| 659 | 721 | 720 | unimplemented MSR #GP, 16.11.2, **16.11.3 reserved bits**, 16.12 |
| 660 | 722 | 721 | Fig 16-33 (802h), 16.13 |
| 661 | 723 | 722 | x2APIC ICR exceptions, **Fig 16-34**, 16.14 |
| 662 | 724 | 723 | Fig 16-35 LDR, logical matching, derivation, 16.15 |
| 663 | 725 | 724 | **Fig 16-36 Self_IPI**, self-IPI semantics |

### Check of the repo's location claims (for locating only)

- **Correct:** Fig 16-2 on p630; timer on pp636–638; TPR/PPR on pp650–651; EOI/SEOI/IER on pp652–654.
- **Table 16-2:** the repo says pp631–634, but it is entirely on **p631**. Pages 632–634 hold §16.3.3–§16.4.
- **ICR:** Fig 16-18 is on **p642**. Table 16-4 is on p644.
- **Acceptance:** pp647–649 is right, but the TMR map continues onto p650.
- **x2APIC:** the material runs to **p663**, not p662 (§16.15 and Fig 16-36), so the PDF range is 692–**725**, not 692–724.

---

## 1. Answers

### Q1. x2APIC MSR space 0x800–0x8FF (Table 16-6), access types and #GP conditions

**Rules:**

1. **The MSR interface exists only in x2APIC mode.**
   - "All APIC registers except the APIC Base Address Register are mapped into the architecturally dedicated MSR range 800h to 8FFh".
   - "A #GP(0) exception is generated if an unimplemented APIC register is specified in ECX."
   - "When not in x2APIC mode, attempts to access the APIC register set using the MSR interface results in the WRMSR or RDMSR instruction generating a #GP(0) exception."
   - "In x2APIC mode, the legacy MMIO access to the APIC register set is disabled."
   - [V2 p657/PDF719/i718 §16.11]
   - PPR agrees: "If (…APIC_BAR[x2ApicEn] == 0) then GP-read-write." and "RDMSR/WRMSR will occur in program order" [PPR p52/PDF52/i51].
2. **Address formula** [V2 p657 §16.11.1]:
   - "x2APIC MSR address = 800h + ((APIC MMIO offset) >> 4)".
   - Exceptions: ICR 300h/310h are "merged into a single 64-bit x2APIC register at MSR address 830h"; Self_IPI "is added at MSR address 83Fh".
   - DFR (E0h) and RRR (C0h) "are not supported in x2APIC mode. Accordingly, MSR addresses 80Eh and 80Ch are not used and are reserved."
3. **Unimplemented addresses:** "MSR addresses in the range 800h through 8FFh that are not listed in Table 16-2 are unimplemented and reserved. A #GP(0) exception is generated if a WRMSR or an RDMSR instruction attempts to access an unimplemented MSR in the x2APIC address range." [V2 p659/PDF721/i720]
   - The text cites Table 16-2, the MMIO table. Mapping Table 16-2 through the formula, then applying the three exceptions in rule 2, yields exactly the Table 16-6 set.
4. **Reserved bits** [V2 p659 §16.11.3]:
   - "Attempting to write a '1' to a reserved bit causes a #GP(0) exception."
   - Legacy registers: "Reserved bit checks … are the same as described for each register in non-x2APIC mode … Except for the Interrupt Command Register, attempting to write a '1' into bits 63:32 of the legacy APIC registers causes a #GP(0) exception."
   - ESR: "A WRMSR of a non-zero value causes a #GP(0) exception."
   - ICR: see 16.13. SELF IPI: see 16.15.
   - "The RDMSR instruction returns a zero for any reserved bit."
5. **Explicit per-register #GP rules:**
   - 802h: "Attempting to write MSR 802h or attempting to read this MSR when not in x2APIC mode causes a #GP(0)" [V2 p660/PDF722/i721].
   - 83Fh: "This register is write-only and attempts to read it cause a #GP(0) exception" [V2 p662/PDF724/i723].
   - Table 16-6 notes: EOI and ESR "#GP(0) if non-zero value is written" [V2 p658/PDF720/i719].
6. **Delegated instruction rules** [V3 p438/PDF473/i472; p511–512/PDF546–547/i545–546]:
   - RDMSR #GP: "CPL was not 0" or "a reserved or unimplemented MSR address".
   - WRMSR #GP adds "Writing 1 to any bit that must be zero (MBZ) in the MSR" and a non-canonical value.
   - WRMSR note: "some x2APIC and AVIC MSRs may have relaxed serialization semantics" (V2 §16.11.2).
7. **Serialization** [V2 p659 §16.11.2]: an x2APIC WRMSR "may complete before older store operations are complete". "WRMSR and RDMSR instructions targeting the x2APIC MSRs are always executed in program order with respect to each other."
8. **AMD extended registers (840h–853h)** are listed in Table 16-6 as x2APIC MSRs, so they exist in x2APIC mode.
   - Presence is advertised by Version bit 31 and ExtFeature bits: "The IER and SEOI registers are located in the APIC Extended Space area. The presence … is indicated by bit 31 of the APIC Version Register" [V2 p653/PDF715/i714].
   - PPR defines the x2APIC forms of all of them [PPR p183–185].

**Row-by-row table.** The APM columns are authoritative. "Reserved bits" means a write of 1 gives #GP(0) (16.11.3). The PPR column is product-specific.

| MSR | Register (Table 16-6) | APM access | APM #GP / value rules (explicit) | APM reserved bits, 64-bit view | PPR x2APIC access (Fam1Ah M44h) | Notes |
|---|---|---|---|---|---|---|
| any 800–8FF | (all) | — | #GP(0) on RDMSR **and** WRMSR when not in x2APIC mode (p657) | — | "GP-read-write" if x2ApicEn==0 (p52) | |
| 800h, 801h | not listed | — | unimplemented → #GP(0) R/W (p659) | — | — | |
| 802h | x2APIC ID [31:0], MMIO 20h | RO, "Expanded to 32-bits" | **write → #GP(0)** (p660) | 63:32 (reads 0) | 31:0 Read-only, Error-on-write; 63:32 Reserved (p174) | Fig 16-33: 31:0 RO |
| 803h | APIC Version, 30h | RO | write rule **not stated** (only "RO") | 30:24, 15:8 (Fig 16-4), 63:32 | Read-only, Error-on-write; 63:32 Reserved; **bit 24 DirectedEoiSupport** (p175) | see Q7 |
| 804h–807h | not listed | — | #GP(0) R/W | — | — | |
| 808h | TPR, 80h | R/W | — | 31:8 (Fig 16-26) + 63:32 | 7:0 Read-write,Volatile; 63:8 Reserved (p175) | |
| 809h | APR, 90h | RO | write rule not stated | 31:8, 63:32 | Read-only, Error-on-write, Volatile (p175) | |
| 80Ah | PPR, A0h | RO | write rule not stated | 31:8, 63:32 | Read-only, Error-on-write, Volatile (p175) | |
| 80Bh | EOI, B0h | **WO** | **#GP(0) if non-zero value is written** (Table 16-6); read rule **not stated** | — | 63:0 **Write-0-only, Error-on-read, Error-on-write-1** (p175) | |
| 80Ch | (Remote Read, C0h) | "Eliminated in x2APIC mode" | "not used and are reserved" (p657) → #GP(0) R/W | — | — | |
| 80Dh | LDR, D0h | RO, "Expanded to 32-bits" | write rule not stated (16.14: "read-only register") | 63:32 | 31:16 and 15:0 Read-only, Error-on-write (p176) | see Q12 |
| 80Eh | (DFR, E0h) | "Eliminated in x2APIC mode" | reserved → #GP(0) R/W | — | — | |
| 80Fh | SVR, F0h | R/W | — | **31:10** (Fig 16-17) + 63:32 | 9, 8, 7:0 Read-write; 63:10 Reserved (p176) | see Q6 |
| 810h–817h | ISR, 100–170h | RO | write rule not stated | 810h bits 15:0 (Fig 16-24) | Read-only, Error-on-write, Volatile; 63:32 Reserved (p177) | |
| 818h–81Fh | TMR, 180–1F0h | RO | write rule not stated | 818h bits 15:0 (Fig 16-25) | same (p177) | |
| 820h–827h | IRR, 200–270h | RO | write rule not stated | 820h bits 15:0 (Fig 16-23) | same (p177) | |
| 828h | ESR, 280h | R/W | **#GP(0) if non-zero value is written** (Table 16-6; p659) | (any 1 → #GP) | bits 7, 6, 5, 3, 2 "Read, Write-0-only, Error-on-write-1, Volatile"; 63:8 Reserved; bit 7 "Can only be set in xAPIC mode" (p178) | see Q11 |
| 829h–82Fh | not listed | — | #GP(0) R/W | — | — | |
| 830h | ICR (bits 63:0), 300h | R/W | MT encodings 1, 3, 7 "eliminated … reserved"; bits 17:16 and 12 "must be zero" (p661) | **31:20, 17:16, 13:12** (Fig 16-34); 63:32 = DEST (exempt) | 31:20, 17:16, 13:12 Reserved; MsgType 3h Reserved but **1h and 7h listed valid** (p179) | see Q8 |
| 831h | (ICR high, 310h) | merged into 830h | not a register → #GP(0) R/W | — | — | |
| 832h | Timer LVT, 320h | R/W | — | **31:18, 15:13, 11:8** (Fig 16-8) + 63:32 | 17, 16, 7:0 RW; 12 DS "Read-only,Volatile"; 63:18, 15:13, **11:8** Reserved (p180) | Fig 16-7 (generic) conflicts; see Q4 |
| 833h | Thermal LVT, 330h | R/W | — | 31:17, 15:13, 11 (Fig 16-14) + 63:32 | 16, 10:8, 7:0 RW; 12 DS RO,Volatile; 63:17, 15:13, 11 Reserved (p180) | |
| 834h | Perf LVT, 340h | R/W | — | 31:17, 15:13, 11 (Fig 16-13) | same pattern (p181) | |
| 835h, 836h | LINT0/LINT1 LVT, 350/360h | R/W | — | 31:17, 13, 11 (Fig 16-12) | 16, 15 TM, 10:8, 7:0 RW; 14 RmtIRR and 12 DS RO,Volatile; 63:17, 13, 11 Reserved (p181) | |
| 837h | Error LVT, 370h | R/W | — | 31:17, 15:13, 11 (Fig 16-15) | as thermal (p182) | |
| 838h | Timer Initial Count, 380h | R/W | — | 63:32 | 31:0 RW; 63:32 Reserved (p182) | |
| 839h | Timer Current Count, 390h | RO | write rule not stated | 63:32 | 31:0 **Read, Error-on-write, Volatile** (p182) | |
| 83Ah–83Dh | not listed | — | #GP(0) R/W | — | — | |
| 83Eh | Timer Divide Config, 3E0h | R/W | — | **31:4 and bit 2** (Fig 16-11) + 63:32 | 3:0 Div RW ("Div[2] is unused"; values 4h–7h and Ch–Fh "Reserved"); 63:4 Reserved (p183) | see Q5 |
| 83Fh | Self IPI (x2APIC only) | **WO** | **read → #GP(0)** (p662) | 31:8 (Fig 16-36); 63:32 not stated by APM | 7:0 **Write-only, Error-on-read**; 63:8 Reserved (p183) | Table 16-6's "See Figure 16-6" is wrong: Fig 16-6 is ExtControl; the right figure is 16-36 |
| 840h | Ext APIC Feature, 400h | RO | write rule not stated | 31:24, 15:3 (Fig 16-5), 63:32 | Read-only, Error-on-write; reset 0000_0000_0004_0007h (p183) | |
| 841h | Ext APIC Control, 410h | R/W | — | 31:3 (Fig 16-6), 63:32 | 2, 1, 0 RW; 63:3 Reserved (p184) | |
| 842h | SEOI, 420h | R/W | — | 31:8 (Fig 16-29), 63:32 | 7:0 RW; 63:8 Reserved; "behavior is undefined if no interrupt is pending for the specified interrupt vector" (p184) | |
| 843h–847h | not listed | — | #GP(0) R/W | — | — | |
| 848h | IER0, 480h | R/W | — | 848h bits **15:0** (Fig 16-30), 63:32 | 31:16 RW; 15:0 Reserved; 63:32 Reserved (p184) | reset conflict, see Q3 |
| 849h–84Fh | IER1–7, 490–4F0h | R/W | — | 63:32 | 31:0 RW; 63:32 Reserved (p184) | |
| 850h–853h | Ext Interrupt [3:0] LVT, 500–530h | R/W | — | 63:32; the APM has **no bit layout** for these | 16, 10:8, 7:0 RW; **12 DS "Read-write,Volatile"**; 63:17, 15:13, 11 Reserved (p185) | the MMIO form lists DS as RO (p65) |
| 854h–8FFh | not listed | — | #GP(0) R/W | — | — | |

**Implementation implications:**
- The MSR intercept must give #GP(0) for:
  - all addresses not in Table 16-6, including 80Ch, 80Eh and 831h, in both directions;
  - any write to 802h;
  - any read of 83Fh;
  - a non-zero write to 80Bh or 828h;
  - any write that sets a reserved bit listed above.
- Reserved bits must read back as 0.
- The whole 800h–8FFh range must #GP when the guest's EXTD=0. With a permanently x2APIC shadow, that path is never reached.
- The APM **does not state** #GP for writes to the other RO registers (803h, 809h, 80Ah, 80Dh, 810h–827h, 839h, 840h) or for reads of 80Bh. The PPR marks them Error-on-write and Error-on-read. See UNRESOLVED U1 and U2.
- The #GP must come from the WRMSR itself, i.e. before the value takes effect (V3 WRMSR table; §16.11.3 "the WRMSR instruction checks for reserved bits"). Whether x2AVIC delivers these writes as fault-style or trap-style intercepts is a Chapter 15 question outside this sheet.

### Q2. APIC_BASE (MSR 1Bh): fields, transitions, INIT/RESET

**Fields** (Fig 16-2 [V2 p630/PDF692/i691] and Fig 16-31 [V2 p655/PDF717/i716]):

| Bits | Name | Access |
|---|---|---|
| 63:52 | Reserved | MBZ |
| 51:12 | ABA | R/W |
| 11 | AE ("APIC Enable (xAPIC mode)") | R/W |
| 10 | EXTD ("X2APIC Mode Enable") | R/W |
| 9 | Reserved | MBZ |
| 8 | BSC | **RO** |
| [7:0] | Reserved | MBZ |

- ABA: "extended by 12 bits at the least-significant end to form the 52-bit physical base address. The reset value … is 0_0000_FEE0_0000h. This address is not affected by INIT."
- "a given processor may implement a physical address less than 52 bits in length."
- Bit 10 was "previously reserved" [V2 p654/PDF716/i715].
- With AE=0, "the local APIC is disabled, including all local vector table interrupts" [V2 p629/PDF691/i690].
- MBZ writes fall under V3 WRMSR: "Writing 1 to any bit that must be zero (MBZ)" gives #GP.
- PPR [p121/PDF121/i120]: 63:48 Reserved; ApicBar 47:12 (reset 0_000F_EE00h); ApicEn; x2ApicEn ("Clearing this bit after it has been set requires ApicEn to be cleared as well"); 9 and 7:0 Reserved; **BSC "Read-write,Volatile. Reset: X"**, which conflicts with the APM's RO.

**Modes (Table 16-5, p655):**

| AE, EXTD | Mode |
|---|---|
| 0,0 | disabled |
| **0,1** | **Invalid** |
| 1,0 | xAPIC |
| 1,1 | x2APIC |

"Attempting to set the combination of AE=0 and EXTD=1 is invalid and causes the WRMSR instruction to generate a #GP(0) exception."

**Transitions** (Fig 16-32 [V2 p656/PDF718/i717], text p655–656):
- Valid arrows: RESET→Disabled; Disabled→xAPIC (AE=1, EXTD=0); xAPIC→Disabled (0,0); xAPIC→x2APIC (1,1); x2APIC→Disabled (0,0).
- "Once the local APIC has been placed into x2APIC mode, the only valid transition (other than reset) is to 'APIC Disabled' mode by simultaneously clearing AE and EXTD to zero. Executing a WRMSR instruction to attempt a transition other than those specified in Figure 16-32 results in a #GP(0) exception."
- So **Disabled→x2APIC directly gives #GP(0)** ("two-step process", §16.9.1) and **x2APIC→xAPIC gives #GP(0)**.
- The sentence before that cites "Figure 16-2" for the transitions; that is an erratum for 16-32.
- §16.9 [p654]: "Before entering x2APIC mode, the local APIC must first be enabled (AE=1, EXTD=0)." Support is CPUID Fn0000_0001_ECX[21] [V2 p654; V3 p625/PDF659/i658; PPR p68].

**What changes on each transition:**
- **xAPIC→x2APIC** [p656]: "Most APIC registers previously written by software while in APIC_Enabled mode are not affected". The exceptions:
  - "The Logical Destination Register (LDR) is not preserved."
  - "The upper half of the Interrupt Command Register (ICR High) is not preserved."
  - The 8-bit APIC_ID value "is converted by hardware … and reflected into the 32-bit x2APIC_ID register (MSR 802h)". The page says "MMIO offset 30h", an erratum for 20h.
  - "System hardware initializes LDR with the 32-bit logical x2APIC_ID whenever x2APIC mode is enabled" [p661].
  - MMIO access is disabled [p657].
- **x2APIC→Disabled, and Disabled→xAPIC:** register effects are not stated (U6).
- **AE=0 in general:** "no local vector table interrupts are supported" [p630]. PPR: when hardware-disabled "only SMI, NMI, INIT, and ExtInt interrupts may be accepted" [PPR p55]; MMIO is "Treated as normal memory space when APIC is disabled" [PPR p52].

**RESET** [V2 p657/PDF719/i718 §16.10]: "A RESET clears the APIC Base Address Register AE bit and EXTD bit, disabling the local APIC. All local APIC registers are initialized to their reset values as described in section 16.3.2". ABA resets to FEE0_0000h [p630]. PPR: "Reset forces the APIC and X2APIC disabled" [p52]; BSC reset "X" [p121].

**INIT** [p657]: "An INIT does not modify the APIC Base Address Register AE and EXTD bits, thus the local APIC mode is not changed." ABA is "not affected by INIT" [p630]. What INIT does to BSC is not stated.

**Implementation implications:**
- The APIC_BASE intercept must give #GP(0) for:
  - bits 63:52, 9 or 7:0 set;
  - AE=0 with EXTD=1;
  - (1,1)→(1,0);
  - (0,0)→(1,1).
- BSC is RO.
- **(1,1)→(0,0) is an APM-valid transition.** A design that keeps the shadow permanently x2APIC-enabled and refuses that write is a **deviation** from the APM and must be recorded as such, not claimed as conformant. Once disabled, the only APM path back is Disabled→xAPIC→x2APIC, which needs xAPIC (MMIO) mode.
- INIT must not touch AE, EXTD or ABA.

### Q3. State after RESET and INIT (Table 16-2 and §16.10)

- Table 16-2 [V2 p631/PDF693/i692]: "The table includes the value of each register after reset and INIT."
- §16.10 [p657]: INIT leaves AE and EXTD alone. "All other APIC registers are initialized to their values as described in 'Reset in x2APIC mode' above." **No such heading exists** (dangling reference). The only value list in the chapter is Table 16-2.
- PPR [p55/PDF55/i54 §2.1.11.2.1.15]: "When a processor accepts an INIT interrupt, the APIC is reset as at power-up, with the exception that: Core::X86::Apic::ApicId is unaffected. Pending APIC register writes complete."
- INIT state [V2 p643/PDF705/i704]: "In the INIT state, the target APIC is responsive only to the STARTUP IPI. All other interrupts (including SMI and NMI) are held pending until the STARTUP IPI has been accepted."

| Register (x2APIC MSR) | V2 Table 16-2 value (MMIO offset) | Other V2 statements (x2APIC) | PPR reset (x2APIC form) | Status after INIT in x2APIC mode |
|---|---|---|---|---|
| APIC_BASE (1Bh) | — | RESET: AE=EXTD=0, ABA=FEE0_0000h (p630, p657) | ApicEn 0, x2ApicEn 0, ApicBar 0_000F_EE00h, BSC X (p121) | **AE, EXTD, ABA preserved** (p657, p630); BSC not stated |
| x2APIC ID (802h) | 20h: `??000000h` (xAPIC layout) | "assigned by hardware at reset time" (p659); RO | XXXX_XXXXh (p174) | V2 silent; **PPR: ApicId unaffected** by INIT |
| Version (803h) | 30h: `80??0010h` | — | EAS 1, bit 24 = 1, MLE XXh, Ver 10h (p175) | constant (RO) |
| TPR (808h) | 0 | — | 0 | 0 |
| APR (809h) | 0 | — | 0 | 0 |
| PPR (80Ah) | 0 | — | 0 | 0 |
| EOI (80Bh) | "–" | — | 0 | n/a (WO) |
| Remote Read | C0h: 0 | eliminated in x2APIC | — | n/a |
| LDR (80Dh) | D0h: **0** | "System hardware initializes LDR with the 32-bit logical x2APIC_ID **whenever x2APIC mode is enabled**" (p661) | 0 (p176) | **UNRESOLVED** (U5): 0 per Table 16-2, or the derived logical ID per §16.14 |
| DFR | E0h: `FFFFFFFF` | eliminated in x2APIC | F000_0000h (MMIO, p57) | n/a in x2APIC |
| SVR (80Fh) | F0h: `000000FFh` | — | 0000_00FFh | 0xFF: **ASE=0** (software-disabled, so LVT masks are forced; see Q4), FCC=0, vector FFh |
| ISR/TMR/IRR (810h–827h) | 0 | — | 0 | 0 |
| ESR (828h) | 0 | — | 0 | 0 |
| ICR (830h) | 300h: 0, 310h: 0 | — | 0 | 0 |
| Timer LVT (832h) | 320h: `00010000h` | — | 0000_0000_0001_0000h | 0x10000: masked, one-shot, vector 0 |
| Thermal LVT (833h) | `00010000h` | — | same | 0x10000 |
| Perf LVT (834h) | `00010000h` | — | same | 0x10000 |
| LINT0 LVT (835h) | `00010000h` | — | same | 0x10000: masked, edge, fixed |
| LINT1 LVT (836h) | `00010000h` | — | same | 0x10000 |
| Error LVT (837h) | `00010000h` | — | same | 0x10000 |
| Timer initial count (838h) | 0 | — | 0 | 0, so the timer is stopped (§16.4.1: a zero count "stops decrementing") |
| Timer current count (839h) | 0 | — | 0 | 0 |
| Divide config (83Eh) | 0 | — | 0 | 0, i.e. divide by 2 |
| Self IPI (83Fh) | (none) | — | 0 | n/a (WO) |
| Ext Feature (840h) | 400h: `00040007h` | — | 0000_0000_0004_0007h | XLC=4, XAIDC=SNIC=INC=1 |
| Ext Control (841h) | 0 | — | 0 | 0 |
| SEOI (842h) | "–" | — | 0 | — |
| IER (848h–84Fh) | 480–4F0h: `FFFFFFFFh`; "The reset value of IER is all ones." (p654) | 848h bits 15:0 are MBZ (Fig 16-30); "RDMSR … returns a zero for any reserved bit" (p659) | 848h: 0000_0000_FFFF_FFFFh at register level, but field 15:0 Reserved (p184); 849h–84Fh: FFFF_FFFFh | all enabled. **848h[15:0] read-back UNRESOLVED** (U8) |
| Ext LVT (850h–853h) | 500–530h: **`00000000h`** (unmasked) | — | **0000_0000_0001_0000h** (masked) (p185; MMIO p65) | **CONFLICT** (U9) |

**What INIT preserves:**
- APM: AE, EXTD (and therefore x2APIC mode) and ABA.
- PPR (product): ApicId. PPR also says "pending APIC register writes complete".
- Everything else takes its Table 16-2 value.

**Implementation implications:**
- The guest INIT path should keep the APIC_BASE shadow and the x2APIC ID. It should load the Table 16-2 values shown above: SVR=0xFF, all six LVTs=0x10000, counts, divide and ESR=0, IER all ones, and so on.
- It must stop the virtual timer, and therefore the mirrored physical timer.
- It must enter the INIT state, where only SIPI is serviced and other interrupts are held pending.
- The LDR value and the extended-LVT reset value need a documented decision (U5, U9).

### Q4. LVT formats and delivery modes; mask rule while software-disabled

**General format** (Fig 16-7 [V2 p635/PDF697/i696]):

| Bits | Name | Access |
|---|---|---|
| 31:18 | Reserved | MBZ |
| 17 | TMM | R/W |
| 16 | M | R/W |
| 15 | TGM | R/W |
| 14 | RIR (Remote IRR) | RO |
| 13 | Reserved | MBZ |
| 12 | DS | RO |
| 11 | Reserved | MBZ |
| 10:8 | MT | R/W |
| 7:0 | VEC | R/W |

Field semantics:
- **VEC:** "sent for this interrupt source when the message type is fixed. It is ignored when the message type is NMI and is set to 00h when the message type is SMI. Valid values for the vector field are from 16 to 255. A value of 0 to 15 when the message type is fixed results in an illegal vector APIC error."
- **MT** "legal values": 000b Fixed; 010b SMI ("the vector field should be set to 00h"); 100b NMI ("vector field being ignored"); 111b External interrupt.
- **DS:** "set to 1 when the interrupt is pending at the CPU core interrupt handler. After a successful delivery of the interrupt, the associated bit in the IRR is set and this bit is cleared to zero … cleared to 0 when the interrupt is idle."
- **RIR:** "set to 1 when the local APIC accepts an LINT0 or LINT1 interrupt with the trigger mode=1 (level sensitive). The bit is cleared to 0 when the interrupt completes, as indicated when an EOI is received."
- **TGM** [p636]: 1 = level-sensitive, 0 = edge. "When the message type is SMI or NMI, the trigger mode is edge triggered."
- **M:** 1 = "reception of the interrupt is disabled".
- **TMM:** 1 = periodic, 0 = one-shot.

**Per-LVT layouts:**

| LVT | Figure | Reserved bits | Fields present |
|---|---|---|---|
| Timer | 16-8 [p636] | 31:18, 15:13, **11:8** | 17 TMM, 16 M, 12 DS, 7:0 VEC; **no MT field** |
| LINT0/1 | 16-12 [p638] | 31:17, 13, 11 | 16 M, 15 TGM, 14 RIR, 12 DS, 10:8 MT, 7:0 VEC |
| Perf | 16-13 [p638] | 31:17, 15:13, 11 | 16 M, 12 DS, 10:8 MT, 7:0 VEC |
| Thermal | 16-14 [p639] | 31:17, 15:13 ("Res"), 11 | same as Perf; "may not be supported in all implementations" |
| Error | 16-15 [p639] | 31:17, 15:13, 11 | same as Perf |

- LINT text [p638]:
  - "Trigger Mode - indicates whether the interrupt pin is edge triggered or level sensitive when the message type is fixed."
  - "Remote IRR - When the trigger mode indicates level, this flag is set when the local APIC accepts the interrupt, and is reset when the local APIC receives an EOI. When the flag is set, no additional local interrupt requests are sent to the local APIC, and they remain pending."
  - §16.4 also lists "input pin polarity" for LINT0/1 [p634], but no polarity bit appears in Fig 16-7 or 16-12; bit 13 is MBZ (U26).
- Extended LVTs 500h–530h (§16.4.5, p639): "four additional LVT registers … located at APIC offsets 500h–530h". The source is "specified by the control register associated with the source". The count is in ExtFeature XLC. **The APM gives no bit layout.** PPR [p65, p185] gives 16 Mask, 12 DS, 10:8 MsgType, 7:0 Vector, with 31:17, 15:13 and 11 reserved.

**Allowed delivery modes** (Table 16-1 [V2 p628/PDF690/i689]):

| Source | Allowed message types |
|---|---|
| APIC Timer | **Fixed** |
| Performance Monitor Counter | **Fixed, SMI, or NMI** |
| Thermal Sensor | **Fixed, SMI, or NMI** |
| APIC Internal Error | **Fixed, SMI, or NMI** |
| Extended Interrupt[3:0] | **Fixed, SMI, NMI, or External interrupt** |

- LINT0/LINT1 have no row of their own. They appear as message types under "I/O interrupts", and the generic legal MT list (Fig 16-7) applies: Fixed/SMI/NMI/ExtINT.
- §16.2 [p629]: "The message type may be Fixed, SMI, NMI, or External interrupt."
- PPR [p55 §2.1.11.2.1.14]: "All LVTs (…ThermalLvtEntry to …LVTLINT, and …ExtendedInterruptLvtEntries) support … 000b=Fixed, 010b=SMI, 100b=NMI, 111b=ExtINT. All other messages types are Reserved."
- PPR x2APIC timer LVT 832h has bits 11:8 Reserved (no MsgType) [p180], matching Fig 16-8.

**Mask bits while software-disabled (SVR bit 8 = 0):**
- §16.3.1 [V2 p629/PDF691/i690]: "SMI, NMI, INIT, Startup, and Remote Read interrupts may be accepted. Pending interrupts in the ISR and IRR are held. Further fixed, lowest-priority, and ExtInt interrupts are not accepted. **All LVT entry mask bits are set and cannot be cleared.**"
- The same wording is repeated under ASE [p641/PDF703/i702].
- PPR repeats "All LVT entry mask bits are set and cannot be cleared" [p57, p176]. PPR's MMIO text lists "LINT[1:0]" among the accepted types.
- **So the masks are forced set, and writes cannot clear them.**
- Not stated:
  - whether other LVT fields stay writable while disabled;
  - whether masks clear automatically when ASE is set back to 1 (U13).

**Delivery status in x2APIC mode:** DS is RO (Fig 16-7). PPR x2APIC LVTs mark DS "Read-only,Volatile" [p180–182], but extended LVTs (850h–853h) are "Read-write,Volatile" [p185].
- The APM does not say whether writing 1 to an RO (non-reserved) bit #GPs.
- PPR "Read-only: Readable; writes are ignored" [PPR p27 Table 8] (U14).

**Implementation implications:**
- The LVT trap validation should give #GP(0) for reserved bits:
  - timer 63:18, 15:13, 11:8;
  - thermal, perf and error 63:17, 15:13, 11;
  - LINT0/1 63:17, 13, 11.
- It should treat DS (and RIR on LINT) as hardware-owned.
- It should enforce that, while the virtual SVR[8]=0, bit 16 of every LVT stays 1 in both the virtual register and the physical mirror.
- It must not raise #GP for vector 0–15. The APM defines that case as an "illegal vector APIC error", not a fault.
- The consequence of a non-listed MT value is not stated (U3). Timer MT bits are reserved in x2APIC.
- Before mirroring to a host LAPIC in x2APIC mode, reserved bits must never be passed through, because the host WRMSR would #GP.

### Q5. Timer (§16.4.1)

- **Modes** [V2 p636/PDF698/i697]: "The timer can operate in two modes, periodic and one-shot, under the control of bit 17 (Timer Mode)". TMM=1 is periodic, 0 is one-shot.
- **TSC-deadline:**
  - Chapter 16 never mentions it.
  - Timer LVT bit 18 is reserved: Fig 16-7 "31:18 Reserved MBZ", Fig 16-8, and PPR 832h "63:18 Reserved" [p180]. In x2APIC mode, writing bit 18 therefore gives #GP(0) (§16.11.3).
  - CPUID Fn0000_0001_ECX bit 24 is "Reserved" [V3 p625/PDF659/i658; PPR p68/PDF68/i67].
  - A text locator found no "deadline" in V2, V3 or the PPR.
  - **Conclusion: AMD defines no TSC-deadline timer mode in these documents.**
- **Initial-count semantics** [p636]:
  - "When the Initial Count Register is written to a non-zero value, the APIC timer is initialized to the value just written and starts decrementing."
  - "When the Initial Count Register is written to zero, the APIC timer is initialized to zero and stops decrementing."
  - "In one-shot mode, the APIC timer stops counting when the timer reaches zero. In periodic mode, the APIC timer is initialized again when it reaches zero, and it starts to decrement again."
  - "Whenever the timer value is decremented to zero, an APIC timer interrupt is generated under the control of bit 16 (Mask)".
  - CCR bullet: "Whenever the ICR [initial count] is written to a non-zero value, or when the CCR reaches zero while in periodic mode, it is initialized to a start count loaded from the ICR and then decrements."
  - So writing a non-zero value **restarts** the count from the new value, and writing zero **stops** it with CCR=0.
  - "To avoid race conditions, software should initialize the Divide Configuration Register and the Timer Local Vector Table Register prior to writing the Initial Count Register to start the timer."
- **Registers** [p637/PDF699/i698]:
  - Fig 16-9 CCR (390h): 31:0 RO, "contains the current value of the APIC timer".
  - Fig 16-10 initial count (380h): 31:0 R/W, "loaded into the APIC Timer Current Count Register when the APIC timer is initialized".
  - Fig 16-11 DCR (3E0h): **31:4 Reserved MBZ; 3 DV[2] R/W; 2 Reserved MBZ; 1:0 DV[1:0] R/W**.
  - The page text says "See Table 16-10" for the initial count; that is an erratum for Figure 16-10.
- **Table 16-3** [V2 p638/PDF700/i699], "Bits 3, 1:0" encodings:

  | Encoding | Divide by | Register value |
  |---|---|---|
  | 000b | 2 | 0h |
  | 001b | 4 | 1h |
  | 010b | 8 | 2h |
  | 011b | 16 | 3h |
  | 100b | 32 | 8h |
  | 101b | 64 | 9h |
  | 110b | 128 | Ah |
  | 111b | 1 | Bh |

  - All eight 3-bit encodings are defined, and bit 2 is MBZ.
  - PPR [p64, p183] gives the same mapping and says "Div[2] is unused". It labels values 7h–4h and Fh–Ch "Reserved".
- **Clock:** "dividing the CPU core clock by a programmable amount … see the BKDG or PPR applicable to your product" [p636]. That leads to PPR §2.1.10 [p43]: the APIC timer "increments at the rate of 2xCLKIN; the APIC timer may increment in units of between 1 and 8". These timers "do not vary in frequency regardless of the current P-state or C-state". PPR §2.1.11.2.1.13 [p55]: "The processor bus clock is divided by … Div[3:0]"; "If … TimerLvtEntry[Mask] is set, timer interrupts are not generated."
- **Current count read:** RO, returns the live value. In x2APIC mode the APM gives only "RO"; PPR says "Read, Error-on-write, Volatile" [p182].
- **Changing the LVT mode, mask or DCR while counting:** not stated. The APM says only that each zero-crossing interrupt is "under the control of bit 16 (Mask)" (U12).
- **Implementation implications:**
  - The mirror must restart the physical timer on every non-zero initial-count write and stop it on a zero write.
  - Periodic mode reloads from the initial count.
  - The divide mapping must use bits 3,1,0. Bit 2 set, or bits 63:4 set, gives #GP(0).
  - Timer LVT bit 18 set gives #GP(0).
  - Mask suppresses the interrupt. The APM does not say it stops the count.

### Q6. Spurious Interrupt Vector Register

- **Fig 16-17** [V2 p641/PDF703/i702]:

  | Bits | Name | Access |
  |---|---|---|
  | 31:10 | Reserved | **MBZ** |
  | 9 | FCC (Focus CPU Core Checking) | R/W |
  | 8 | ASE (APIC Software Enable) | R/W |
  | 7:0 | VEC | R/W |

- VEC: "the vector that is sent to the CPU core in the event of a spurious interrupt."
- ASE: disabled when 0, with the Q4 rules. "Setting the ASE bit to 1, enables the local APIC."
- **FCC:**
  - The p641 text is self-contradictory: "set to 1 disables focus CPU core checking … Clearing the FCC bit to 0 disables focus CPU core checking".
  - p647 [PDF709/i708] resolves it: "If focus CPU core checking is enabled (Spurious Interrupt Register bit 9=0)" and "…disabled (Spurious Interrupt Register bit 9=1)".
  - PPR FocusDisable agrees: "1=Disable focus core checking" [p57, p176].
  - **So bit 9 = 1 disables focus checking.**
- **EOI-broadcast suppression:**
  - **There is no such bit in the APM SVR** (bits 31:10 MBZ). The APM version register has no support bit either (bits 30:24 MBZ, see Q7).
  - PPR SVR also defines 31:10 (MMIO) and 63:10 (x2APIC) as Reserved [p57, p176], even though PPR version bit 24 is "DirectedEoiSupport … Fixed,1" [p56, p175] (U22).
- Reserved bits in x2APIC: 63:10.
- Reset: 0xFF [Table 16-2].
- The APM states no validity restriction on the SVR vector value.
- **Implementation implications:**
  - An SVR write with any of bits 63:10 set, including bit 12, gives #GP(0).
  - Bits 9, 8 and 7:0 are stored.
  - A transition of bit 8 to 0 must force all six LVT masks (and the physical mirror) to set.

### Q7. Version register and the presented value 0x0005_0010

- **Fig 16-4** [V2 p632/PDF694/i693] and text [p633/PDF695/i694]:

  | Bits | Name | Access |
  |---|---|---|
  | 31 | EAS ("Extended APIC Register Space Present") | RO |
  | 30:24 | Reserved | **MBZ** |
  | 23:16 | MLE ("number of entries in the local vector table minus one") | RO |
  | 15:8 | Reserved | MBZ |
  | 7:0 | VER | RO |

  - VER: "The local APIC implementation is identified with a value=1Xh (20h-FFh are reserved)."
  - EAS: "when set to 1 indicates the presence of an extended APIC register space, starting at offset 400h."
- Table 16-2 reset: `80??0010h`.
- **The APM defines no EOI-broadcast-suppression or directed-EOI bit.**
- PPR [p56, p175] (product): 31 ExtApicSpace reset 1; 30:25 Reserved; **24 DirectedEoiSupport "Reset: Fixed,1"**; 23:16 MaxLvtEntry XXh; 7:0 Version 10h.
- **Decoding 0x0005_0010:** EAS=0, bits 30:24=0, MLE=05h (six LVT entries), VER=10h.

  | Field | Value | Against the manuals |
  |---|---|---|
  | VER | 10h | Fits the APM's "1Xh" and the low byte of the Table 16-2 template |
  | Bit 24 | 0 | The only APM-conformant value (reserved) |
  | MLE | 05h | The APM leaves it "??". Six standard LVTs exist at 832h–837h. Whether MLE counts extended LVTs is not stated (XLC is separate) (U11). |
  | EAS | 0 | **Differs from Table 16-2's `80??0010h` and from PPR reset 1.** It is a legal RO value meaning "no extended register space", but the APM gives no rule for 840h–853h when EAS=0 (U10). |

- **Implementation implications:**
  - 0x0005_0010 is consistent with the APM field definitions, but not with the Table 16-2 reset template (EAS).
  - The 840h–853h handling must be decided consistently with EAS=0: either implement them anyway or treat them as absent. The manual does not choose.
  - Bits 63:32 read 0. Writes are covered under Q1.

### Q8. ICR in x2APIC mode (MSR 830h)

- **§16.13** [V2 p660/PDF722/i721]:
  - The two ICRs are combined "into a single 64-bit Interrupt Command Register located at MSR address 830h. Thus in x2APIC mode sending an IPI requires only a single WRMSR".
  - "The upper half … (bits 63:32) contains the Destination ID (DEST) field, which is expanded to 32 bits in x2APIC mode. A DEST value of FFFF_FFFFh is used to broadcast IPIs to all local APICs."
- **Low half** [p661/PDF723/i722]: it "is identical to the APIC Interrupt Command Register Low[31:0] (see Fig 16-18 on page 582 [sic; Fig 16-18 is on p642])", with these exceptions:
  - "The Remote Read Status field (bits 17:16) is eliminated and must be zero."
  - "Message Type field (bits 10:8). Encodings 1, 3 and 7 are eliminated and the encodings are reserved."
  - "The Delivery Status field (bit 12) is eliminated and must be zero."
- **Fig 16-34:**

  | Bits | Name | Access |
  |---|---|---|
  | 63:32 | DEST | R/W |
  | **31:20** | Reserved | MBZ |
  | 19:18 | DSH | R/W |
  | 17:16 | Reserved | MBZ |
  | 15 | TMG/TGM | R/W |
  | 14 | L | R/W |
  | 13:12 | Reserved | MBZ |
  | 11 | DM | R/W |
  | 10:8 | MT | R/W |
  | 7:0 | VEC | R/W |

  - The figure's table prints "55:20" for the reserved range (a stale copy from Fig 16-18). Its bit diagram shows 31:20. Its top bar is mislabelled "63…0 DEST".
- **Legal delivery modes in x2APIC:** 000b Fixed, 010b SMI, 100b NMI, 101b INIT, 110b STARTUP. Lowest Priority (001b), Remote read (011b) and External (111b) are reserved.
  - PPR 830h [p179] still lists 1h Lowest Priority and 7h External interrupt as valid (3h Reserved), which conflicts with the APM (U24).
- **Field semantics** (Fig 16-18 text [p642–644/PDF704–706]):
  - VEC is used for fixed and lowest-priority.
  - SMI: "trigger mode is edge-triggered and the Vector field must = 00h".
  - NMI: "Vector field is ignored".
  - INIT: "trigger mode is edge-triggered, and the Vector field must =00h".
  - STARTUP: "boot-strap routine whose address is specified by the Vector field".
  - DM: 1 = logical, 0 = physical.
  - L: 1 = assert, 0 = deassert.
  - TGM: 1 = level, 0 = edge.
  - The xAPIC DS text: "Code may repeatedly write ICRL without polling the DS bit; all requested IPIs will be delivered." In x2APIC mode DS is absent and MBZ.
  - **DSH:**

    | Value | Meaning |
    |---|---|
    | 00b | Destination field required |
    | 01b | Self: "The issuing APIC is the only destination" |
    | 10b | All including self |
    | 11b | All excluding self |

    - "…if the lowest priority is used, the message could end up being reflected back to this local APIC. If DS=1xb [sic: DSH], the destination mode is ignored and physical is automatically used." PPR 830h agrees: "If all including self or all excluding self is used, then destination mode is ignored and physical is automatically used" [p179].
    - DEST is "used when the Destination Shorthand field=00b".
- **Table 16-4** [V2 p644/PDF706/i705], "Only the combinations indicated in Table 16-4 are valid". "x" means don't care.

  | Message Type | Trigger | Level | Destination Shorthand |
  |---|---|---|---|
  | Fixed | Edge | x | x |
  | Fixed | Level | Assert | x |
  | Lowest Priority, SMI, NMI, INIT | Edge | x | Destination or all excluding self |
  | Lowest Priority, SMI, NMI, INIT | Level | Assert | Destination or all excluding self |
  | Startup | x | x | Destination or all excluding self |

  - PPR Table 17 [p55] is identical.
  - Not listed, so invalid: Level trigger with Level=deassert; SMI/NMI/INIT/Startup with DSH Self or All-including-self; Remote read; ExtINT.
  - The consequence of an invalid combination is **not stated** (U4).
- 16.11.3 exempts the ICR from the "bits 63:32 give #GP" rule.
- **Implementation implications** for the software IPI path:
  - Give #GP(0) if bits 31:20, 17:16 or 13:12 are set.
  - Treat MT 1, 3 and 7 as reserved. The consequence is not stated (U3).
  - Decode DSH before DEST, and force physical mode for DSH 10b/11b.
  - Use 32-bit DEST with FFFF_FFFFh as broadcast.
  - There is no delivery-status emulation.

### Q9. SELF IPI (MSR 83Fh)

- **Fig 16-36** [V2 p663/PDF725/i724]: 31:8 Reserved **MBZ**; 7:0 VEC **WO**.
- "This register is write-only and attempts to read it cause a #GP(0) exception" [p662/PDF724/i723].
- A write is "equivalent to a to-self IPI generated by writing the Interrupt Command Register (ICR, MSR 830h)" with "Destination shorthand = self, Trigger Mode = edge-triggered, Message Type = fixed, Vector = … Self_IPI". It is "architecturally identical to one sent via the ICR", including IRR, ISR and TMR operation (see p647).
- "Completion of the WRMSR to the Self_IPI register ensures that the resulting IPI has been entered into the IRR, and that the associated TMR bit is cleared (as expected for edge-triggered interrupts)."
- §16.11.3 calls it "the 32-bit SELF IPI register". **The APM does not address bits 63:32.** PPR: 63:8 Reserved, "Write-only, Error-on-read" [p183].
- No vector restriction or #GP is stated for vectors 0–15. The illegal-vector handling follows the ICR, and is itself unspecified (U16).
- **#GP:** any RDMSR; any WRMSR with bits 31:8 set. Bits 63:32 give #GP only by the PPR's reserved layout plus §16.11.3's general rule.
- **Implementation implication:** a self-IPI must set IRR and clear TMR **before the WRMSR completes**, i.e. before guest execution continues.

### Q10. EOI (MSR 80Bh)

- Table 16-6 [V2 p658]: "WO", "**#GP(0) if non-zero value is written**".
- p652 [PDF714/i713]: "software writes a value of zero to the End-of-Interrupt Register (EOI) in the local APIC, which causes the local APIC to reset the associated ISR bit. The EOI register is a write-only register." Fig 16-28: 31:0 WO.
- PPR 80Bh [p175]: 63:0 "Write-0-only, Error-on-read, Error-on-write-1"; "A Write zero to this field indicates the end of interrupt processing the currently in service interrupt".
- **The required write value is 0; non-zero gives #GP(0).**
- A read gives #GP per the PPR only; the APM says just "WO" (U2).
- The effect of EOI with an empty ISR is not stated (U18).
- **Implementation implication:** accept only EDX:EAX = 0.

### Q11. ESR (MSR 828h)

- **x2APIC rule:** "A WRMSR of a non-zero value causes a #GP(0) exception" [V2 p659]. Table 16-6 says "R/W", "#GP(0) if non-zero value is written".
- **Legacy semantics** [V2 p640/PDF702/i701]: "a read-write register. Writes to the register cause the internal error state to be recorded in the register, clearing the original error."
- **Fig 16-16:**

  | Bits | Name | Access |
  |---|---|---|
  | 31:8 | Reserved | MBZ |
  | 7 | IRA | R/W |
  | 6 | RIV | R/W |
  | 5 | SIV | R/W |
  | 4 | Reserved | MBZ |
  | 3 | RAE | R/W |
  | 2 | SAE | R/W |
  | 1:0 | Reserved | MBZ |

- **PPR MMIO** [p60]: "Writes to this register trigger an update of the register state. The value written by software is arbitrary. Each write causes the internal error state to be loaded into this register, clearing the internal error state. Consequently, a second write prior to the occurrence of another error causes the register to be overwritten with cleared data."
- **PPR x2APIC** [p178]: "Read, Write-0-only, Error-on-write-1, Volatile". PPR Table 8 defines "Write-0-only: Writing a 0 clears to a 0". IllegalRegAddr "Can only be set in xAPIC mode".
- **So in x2APIC mode the write must be zero.** The latch-on-write semantics are stated only for the legacy register and are not restated in §16.11–§16.15. The PPR's x2APIC "Write-0-only" wording differs (U15).
- **Implementation implication:** WRMSR(828h, 0) should perform the latch/clear. Anything else gives #GP(0). In x2APIC mode ESR bit 7 is never set; unimplemented MSRs give #GP instead (PPR).

### Q12. LDR and DFR in x2APIC mode

- **LDR 80Dh** [V2 p661–662/PDF723–724]:
  - "expanded to 32 bits and contains the 'logical x2APIC_ID'. System hardware initializes LDR with the 32-bit logical x2APIC_ID whenever x2APIC mode is enabled. The LDR is a read-only register located at MSR address 080Dh."
  - Fig 16-35: 31:0 x2Logical_ID RO.
  - "LDR[31:16] cluster_id … LDR[15:0] logical_id. A bit vector uniquely identifying this processor within the cluster."
- **Derivation (exact):** "logical_id[15:0] = 1 << x2APIC_ID[3:0] and cluster_id[15:0] = x2APIC_ID[19:4]".
  - So **LDR = (x2APIC_ID[19:4] << 16) | (1 << x2APIC_ID[3:0])**.
  - The text also says "derived from the remaining bits of the x2APIC_ID", but the formula uses only bits 19:4. How x2APIC_ID bits 31:20 are represented is not stated (U19).
  - "possible 65,535 (2^16−1) clusters, with each cluster having up to 16 logical processors."
  - "The legacy 'flat logical' addressing is not supported in x2APIC mode."
  - Writing 80Dh: the APM says only "read-only". PPR says Error-on-write [p176] (U1).
- **DFR:** "no longer needed and is not supported" [V2 p654]; "Eliminated in x2APIC mode" [Table 16-6]; MSR 80Eh "not used and are reserved" [p657]. **So access to 80Eh gives #GP(0)** [p659].
- **Implementation implication:** compute the LDR from the (pinned) x2APIC ID when x2APIC mode is enabled. Give #GP(0) on 80Eh. The LDR value after INIT is U5.

### Q13. Acceptance, IRR/ISR/TMR, EOI, spurious interrupts, PPR

- **Routing** (§16.6.3 [V2 p647/PDF709/i708]):
  - SMI, NMI, INIT, STARTUP and ExtINT go "directly to the CPU core".
  - Fixed and lowest-priority interrupts go "into an open slot in either the IRR or ISR registers. If there is no free slot, the interrupt is rejected and sent back to the sender with a retry request."
  - "Bits 255:16 correspond to interrupt vectors 255:16 with 255 being the highest priority; bits 15:0 are reserved."
- **IRR** [p647–648]:
  - "When a system interrupt is accepted, the associated bit … is set in the IRR."
  - "When the CPU core requests a new interrupt, the local APIC selects the highest priority IRR interrupt and sends it to the CPU core. The local APIC then sets the corresponding bit in the ISR and resets the associated IRR bit."
- **ISR** [p648/PDF710/i709]:
  - EOI: "the associated ISR bit is reset and a new interrupt is selected from the IRR register."
  - "If a second interrupt with the same interrupt vector number is received by the local APIC while the ISR bit is set, the local APIC sets the IRR bit. No more than two interrupts can be pending for the same interrupt vector number. Subsequent interrupt requests to the same interrupt vector number will be rejected."
  - Nesting: p648 says the higher-priority interrupt is sent directly "and the associated IRR bit is set"; p652 says "associated ISR bit is set". The two pages disagree (U25).
- **TMR** [p648]:
  - It "indicates the trigger mode of the interrupt and determines whether an EOI message is sent to the I/O APIC for level-sensitive interrupts. When the interrupt is accepted … and the IRR bit is set, the associated TMR bit is set for level-sensitive interrupts or reset for edge-triggered interrupts."
  - **"when the EOI is received at the local APIC, an EOI message is sent to the I/O APIC if the associated TMR bit is set for a system interrupt."**
  - PPR [p53]: an EOI write clears "the highest bit in … InService"; "If the corresponding bit in … TriggerMode … is set, a Write to … EndOfInterrupt is performed on all APICs to complete service of the interrupt at the source". Same-vector requests arriving while IRR is set "are collapsed into one".
- **Priorities** [p650–651/PDF712–713]:
  - "Of the 15 priority levels, 15 is the highest and 1 is the lowest." Priority = vector/16 rounded down, "with vectors 0Fh–00h reserved".
  - TP "varies from 0 (all interrupts are allowed) to 15 (all interrupts with fixed delivery mode are inhibited)". TPS "is written with zero when TPR is written using the architectural CR8 register".
  - **PPR computation:** "either the interrupt priority level of the highest priority ISR bit set or the value in the TPR, whichever is higher. The PPR is equal to the TPR when the CPU core is not servicing a higher priority interrupt." PPS "is set to the … TPS … if the PP field is equal to the Task Priority field". PPS otherwise is U20.
  - PPR (product): "The processor priority is the higher of the two main priorities" (bits 7:4) [p53].
  - "Pending interrupts must have a higher priority level than the value in the PPR to be selected … No pending interrupts are selected by the local APIC when the TPR=15."
- **IER** [p654]: "Only vectors that are enabled in IER participate in APIC's computation of the highest-priority pending interrupt." PPR: masking applies only when IerEn=1, and a masked interrupt's IRR bit "remains set" [p53].
- **Spurious** (§16.4.7 [p640]): if TPR is at or above the interrupt's level "while the interrupt is being acknowledged, the local APIC delivers a spurious interrupt to the CPU core instead, with the vector number specified by the Vector field of the Spurious Interrupt Register. The ISR is unaffected by the spurious interrupt, so the interrupt handler completes without sending an EOI".
- **Software-disabled:** ISR/IRR are held, and further fixed, lowest-priority and ExtInt interrupts are not accepted (Q4).
- **APR** [p646–647]: equals the highest of TPR, highest ISR, highest IRR; APS = TPS "if the APR is equal to the TPR, and zero otherwise".
- **Implementation implications:**
  - A guest EOI clears the highest ISR bit.
  - The guest-level source must be completed only when the TMR bit is set (level-triggered). There is no directed-EOI mechanism in the APM.
  - Edge interrupts, self-IPIs and edge IPIs clear TMR.
  - A spurious interrupt does not touch the ISR.
  - Reject a third instance of the same vector.

### Q14. Destination matching in x2APIC mode

- **Physical** (§16.6.1 [p645/PDF707/i706]): "the value of the interrupt message destination field is compared with the unique APIC ID value of each local APIC … If the destination field … is set to FFh, the interrupt is broadcasted … In physical destination mode, the lowest priority message type is not supported."
  - §16.8 [p654]: "Logical and physical destinations are extended to 32 bits".
  - §16.13 [p660]: "A DEST value of FFFF_FFFFh is used to broadcast IPIs to all local APICs."
  - The x2APIC chapter does not say whether DEST=FFh remains a broadcast in x2APIC physical mode (U19).
- **Logical** [p662/PDF724/i723]: "each x2APIC compares bits 31:16 of the message destination with LDR[31:16] (cluster_id). If there is a match, then bits 15:0 of the destination and LDR[15:0] are tested for matching ones. If bits[31:16] (cluster_id) match and any bit in 15:0 (logical_id) match, this x2APIC is a valid destination."
  - "A DEST value of FFFF_FFFFh in the ICR is used to broadcast IPIs to all local APICs."
  - Flat logical is not supported.
- **Shorthand:** DSH 10b/11b force physical mode and ignore DM (Q8); DSH 01b targets only the sender.
- **Lowest priority:**
  - In x2APIC the ICR MT 001b is "eliminated and the encodings are reserved" [p661].
  - Legacy rules: not supported in physical mode [p645] or cluster-logical mode [p646].
  - Arbitration uses the lowest APR; the focus core wins when FCC=0; "If there is a tie for lowest priority, the local APIC with the highest APIC ID is selected" [p647].
  - With all-excluding-self, the message "could end up being reflected back to this local APIC" [p644].
- **Implementation implication** (software fan-out):
  - DSH first.
  - For DSH=00b: DEST==FFFF_FFFFh means all.
  - DM=0 means a 32-bit compare with the x2APIC ID. DM=1 means an exact cluster compare plus a logical-bit AND.
  - Do not implement flat logical mode or lowest priority.

### Q15. Error conditions and ESR bits

- **LVT:** "A value of 0 to 15 when the message type is fixed results in an illegal vector APIC error" [V2 p635].
- **ESR bits** [p640]:
  - SAE bit 2: "a message sent by the local APIC was not accepted by any other APIC".
  - RAE bit 3: "a message received by the local APIC was not accepted by this or any other APIC".
  - SIV bit 5: "attempted to send a message with an illegal vector value".
  - RIV bit 6: "has received a message with an illegal vector value".
  - IRA bit 7: "access to an unimplemented register location within the local APIC register range (APIC Base Address + 4 Kbytes)".
- PPR defines "illegal vector" as "00h to 0Fh for fixed and lowest priority interrupts" [p60, p178]. IRA "Can only be set in xAPIC mode" [p178].
- §16.4.6 [p639]: "Errors that are detected while handling interrupts cause an APIC error interrupt to be generated under the control of bit 16 (Mask) in the APIC Error Local Vector Table Register." The error LVT allows Fixed, SMI or NMI (Table 16-1).
- **Not stated (U16):**
  - which ESR bit a local-LVT illegal vector sets, and whether that happens at write time or at delivery;
  - whether an illegal-vector IPI or self-IPI is still delivered;
  - whether errors are recorded while software-disabled;
  - the consequence of violating the SMI/INIT "Vector field must = 00h" rule.
- **Implementation implication:** vector < 16 is an APIC error (ESR plus the error LVT), **never #GP**. Unimplemented x2APIC MSRs give #GP(0), not ESR[7].

### Q16. SEOI, IER and extended feature/control in x2APIC mode

- §16.7 [p652]: "The SVM hypervisor uses the Extended APIC Feature Register, Extended APIC Control Register, Specific End of Interrupt Register (SEOI), and Interrupt Enable Register (IER) to control virtualized interrupts … the VMM can mask pending interrupts in the local APIC, so they do not participate in the prioritization of other interrupts."
- **SEOI** (§16.7.1, Fig 16-29 [p652–653]):
  - "writing the vector number of the interrupt to the SEOI register".
  - Offset 420h; 31:8 MBZ; 7:0 VECTOR R/W.
  - Gated by ExtControl SN (bit 1): "enables Specific End of Interrupt (SEOI) generation when a write to the specific end of interrupt register is received" [p634].
  - PPR: "The behavior is undefined if no interrupt is pending for the specified interrupt vector" [p65, p184].
- **IER** (§16.7.2, Fig 16-30 [p653]):
  - Eight registers at 480h–4F0h; "bit i … is located at bit position (i mod 32) in … IER[i / 32]".
  - 255:16 IE R/W; 15:0 MBZ; reset all ones.
  - Gated by ExtControl IERN (bit 0): "enables writes to the interrupt enable registers" [p634].
- **Presence** [p653]: "identified by bits 0 and 1, respectively, of the APIC Extended Feature Register … IER and SEOI are enabled by setting bits 0 and 1, respectively, of the APIC Extended Control Register". The Extended Space itself is advertised by version bit 31.
- **ExtFeature Fig 16-5** [p633]:

  | Bits | Name | Access |
  |---|---|---|
  | 31:24 | Reserved | MBZ |
  | 23:16 | XLC | RO |
  | 15:3 | Reserved | MBZ |
  | 2 | XAIDC ("capable of supporting an 8-bit APIC ID") | RO |
  | 1 | SNIC | RO |
  | 0 | INC | RO |

  Reset 00040007h.
- **ExtControl Fig 16-6** [p634]:

  | Bits | Name | Access |
  |---|---|---|
  | 31:3 | Reserved | MBZ |
  | 2 | XAIDN | R/W |
  | 1 | SN | R/W |
  | 0 | IERN | R/W |

  - XAIDN "enables the upper four bits of the APIC ID field".
  - Reset 0.
- **x2APIC availability:** Table 16-6 lists 840h RO, 841h R/W, 842h R/W, 848h–84Fh R/W and 850h–853h R/W, so **they are available in x2APIC mode**. PPR gives x2APIC definitions for each [p183–185].
- **Not stated (U18, U21):**
  - what XAIDN means in x2APIC mode;
  - what happens to IER writes when IERN=0, or SEOI writes when SN=0.
- **Implementation implication:** if extended space is presented (EAS=1), implement 840h/841h/842h/848h–853h with the MBZ checks above, and gate IER and SEOI on IERN and SN. With the presented EAS=0, the manual gives no rule (U10).

---

## 2. Cross-reference chains followed

1. **V2 §16.11 (p657) → V3 RDMSR/WRMSR.**
   - RDMSR: V3 p438/PDF473/i472. WRMSR: p511–512/PDF546–547/i545–546. These give the #GP causes.
   - WRMSR text: "some x2APIC and AVIC MSRs may have relaxed serialization semantics. See the APIC and AVIC sections in APM Volume 2", which returns to V2 §16.11.2 (p659).
2. **V2 §16.11.3 "Legacy APIC registers … sections 16.3 through 16.6" → per-register figures.**
   - Figs 16-3/4 (p632), 16-5 (p633), 16-6 (p634), 16-7 (p635), 16-8 (p636), 16-9/10/11 (p637), 16-12/13 (p638), 16-14/15 (p639), 16-16 (p640), 16-17 (p641), 16-18 (p642), 16-20 (p645), 16-21 (p646), 16-22 (p647), 16-23 (p648), 16-24/25 (p649), 16-26 (p650), 16-27 (p651), 16-28 (p652).
   - Figs 16-29 and 16-30 are in §16.7, outside the cited range. Their MBZ bits fall under §16.11.3's first sentence.
3. **V2 §16.11.3 ICR → §16.13 (p660–661) → Fig 16-18 (p642) and Table 16-4 (p644).** §16.13 cites "page 582" for Fig 16-18, which is wrong.
4. **V2 §16.11.3 SELF IPI → §16.15 (p662–663) → "Accepting System and IPI Interrupts" p647 (§16.6.3).**
5. **V2 §16.10 INIT → "Reset in x2APIC mode" above: DANGLING** (no such heading in ch.16, TOC PDF 17–18 checked).
   - Fallbacks: Table 16-2 (p631, "after reset and INIT") and §16.3.2 (the RESET sentence).
   - Supplemented by PPR §2.1.11.2.1.15 (p55).
6. **V2 p659 "not listed in Table 16-2" → Table 16-2 (p631, MMIO).** Table 16-6 (p658) is the x2APIC list. Both give the same implemented set after the p657 exceptions.
7. **V2 p655 "illustrated in Figure 16-2" → Fig 16-2 is the APIC_BASE layout (p630).** The diagram is Fig 16-32 (p656), which the next sentence cites.
8. **V2 Table 16-6 SELF IPI "See Figure 16-6" → Fig 16-6 is ExtControl (p634).** The correct figure is Fig 16-36 (p663).
9. **V2 §16.4.1 (p636) and §16.4.4 (p639) → "BKDG or PPR applicable to your product".**
   - PPR §2.1.10 (p43), §2.1.11.2.1.13–15 (p55), thermal LVT p62/p180.
   - Applicability to the host is not verified.
10. **V2 §16.9 CPUID Fn0000_0001_ECX[21] → V3 p624–625 (PDF658–659) and PPR p68.** Bit 24 is reserved in both.
11. **V2 §16.4.1 "See Table 16-10" → Figure 16-10 (p637).** Erratum.
12. **V2 §16.9.1 "8-bit APIC_ID register (MMIO offset 30h)" → Table 16-2 gives APIC ID at 20h.** 30h is Version. Erratum.
13. **PPR access-type terms → PPR Table 8 (p27), §1.4.4.9 (p26), Table 9 (p28), p36 (#GP notation), p485–486 (index definitions).**
    - "Error" is not typed in Table 8. PPR MSR 2Ah (p121) ties its "Error-on-write" field to "a GP fault with error code 0".
14. **Not followed (outside these questions):**
    - §16.12 → "Cache and Processor Topology" p212 and V3 Appendix E (x2APIC_ID sub-field layout).
    - §16.4.5 → MCi_MISC0 p311 (extended-LVT sources).

## 3. Document errata and conflicts noticed

- **V2 internal:**
  - p641 FCC sentence says both values disable. p647 and the PPR resolve it: bit 9=1 disables.
  - p644 "If DS=1xb" should be DSH.
  - p648 and p652 disagree (IRR vs ISR) on nesting.
  - p655 "Figure 16-2" should be 16-32.
  - p656 "offset 30h" should be 20h.
  - p657 "Reset in x2APIC mode" is dangling.
  - p659 "Table 16-2" should be 16-6.
  - p661 "page 582" should be p642.
  - Fig 16-34: "55:20" should be 31:20, and the DEST bar says 63..0.
  - p658 "Figure 16-6" should be 16-36.
  - p636 "Table 16-10" should be Fig 16-10.
  - p634 mentions LINT "input pin polarity", but no such bit is defined.
  - Fig 16-7 (generic, MT on all LVTs) conflicts with Fig 16-8 (timer 11:8 reserved).
- **V2 vs PPR:**
  - Ext LVT reset: 00000000h vs 00010000h.
  - DFR reset: FFFFFFFF vs F000_0000h.
  - BSC: RO vs RW,Volatile.
  - APIC ID: R/W ("model dependent") vs RO.
  - Version bits 30:24: MBZ vs bit 24 DirectedEoiSupport=1.
  - x2APIC ICR MT 1h/7h: reserved vs valid.
  - Remote Read: xAPIC feature vs "not supported" [PPR p57].
  - DCR: bit 2 MBZ vs "Div[2] is unused" with reserved values.
  - Software-disable accepted interrupts: APM lists Remote Read; PPR lists LINT[1:0].
- **PPR internal:**
  - Ext-LVT DS is RO in MMIO form (p65) but RW,Volatile in x2APIC form (p185).
  - MSR 848h register reset is FFFF_FFFFh while field 15:0 is Reserved (p184).

## 4. Implementation checklist (derived only from the rules above)

- **SVR/LVT trap:**
  - Reserved-bit #GP(0) using the per-register masks in Q1 and Q4.
  - The SVR[8]=0 rule forces and holds every LVT mask, virtually and physically.
  - DS/RIR are hardware-owned.
  - Vector < 16 is an APIC error, not #GP.
  - Timer: fixed delivery only; bits 11:8 and 18 reserved.
  - Thermal/perf/error: Fixed, SMI or NMI.
  - LINT: Fixed, SMI, NMI or ExtINT, plus TGM.
  - Never mirror reserved bits to the host LAPIC.
- **MSR intercept:**
  - #GP(0) for unlisted addresses (including 80Ch, 80Eh, 831h), WRMSR 802h, RDMSR 83Fh, non-zero EOI/ESR writes, and reserved bits.
  - 839h is a live read; write handling is U1.
  - APIC_BASE follows Q2. Refusing (1,1)→(0,0) is a documented deviation.
- **Guest INIT:** keep AE, EXTD, ABA and the x2APIC ID; load the Table 16-2 values; stop the timer; only SIPI is serviced until accepted. LDR and Ext-LVT values are U5/U9.
- **IPI fan-out:**
  - Reserved bits 31:20, 17:16, 13:12 give #GP(0).
  - DSH decode first; DSH 1xb forces physical mode.
  - 32-bit DEST; FFFF_FFFFh is broadcast.
  - Logical matching: cluster [31:16] equal AND any common bit in [15:0].
  - Only MT 0, 2, 4, 5, 6.
  - Table 16-4 combinations.
  - A self-IPI sets IRR and clears TMR before the WRMSR completes.

## 5. UNRESOLVED (the manuals do not settle these; no extrapolation)

- **U1. Writes to RO x2APIC MSRs other than 802h** (803h, 809h, 80Ah, 80Dh, 810h–827h, 839h, 840h). The APM says only "RO". The PPR (product-specific) marks them Error-on-write. PPR Table 8 does not name the exception; only its MSR 2Ah entry ties Error-on-write to "GP fault with error code 0".
- **U2. RDMSR of EOI (80Bh).** The APM says only "WO". PPR: Error-on-read.
- **U3. Reserved encodings (as opposed to reserved bits).** The APM gives no fault/ignore rule for:
  - x2APIC ICR MT 001b, 011b or 111b;
  - LVT MT values other than 000/010/100/111.
  - (DCR bit 2 is MBZ, so that case is settled as #GP.)
- **U4. ICR combinations not in Table 16-4.** They are "not valid", but the consequence (#GP, ignore, ESR) is not stated.
- **U5. Register values after INIT in x2APIC mode.** The §16.10 reference is dangling. Open points:
  - LDR: 0 (Table 16-2) or the derived logical ID (§16.14 "whenever x2APIC mode is enabled")?
  - x2APIC ID preservation: the APM is silent; the PPR says unaffected.
  - The effect on BSC.
- **U6. Register state on x2APIC→Disabled, and on the later Disabled→xAPIC re-enable.** Not stated.
- **U7. APIC_BASE writes that do not change mode** (Fig 16-32 has no self-loops); ABA changes while in x2APIC mode; writes to BSC; ABA bits above the implemented physical-address width.
- **U8. MSR 848h bits 15:0.** Reset is "FFFFFFFFh" and "all ones", yet those bits are MBZ and RDMSR returns 0 for reserved bits.
- **U9. Extended LVTs.** Reset value 0 (APM) vs 0x10000 (PPR). The APM gives no bit layout. The PPR x2APIC DS access type conflicts with its MMIO form.
- **U10. Behavior of 840h–853h when EAS=0** is presented (as with version 0x0005_0010).
- **U11. Whether MLE counts extended LVTs** (the APM value is "??").
- **U12. Timer changes while counting:** changing TMM, the mask, or the DCR mid-count; whether counting continues while masked or software-disabled.
- **U13. LVTs while ASE=0 and after re-enable:** whether non-mask fields stay writable while ASE=0; whether masks remain set after ASE returns to 1.
- **U14. Writing 1 to RO non-reserved bits (DS, RIR) in x2APIC mode.** The APM is silent. PPR "Read-only" means writes are ignored.
- **U15. Exact x2APIC ESR semantics.** Legacy "write latches internal error state" vs PPR "Write-0-only (writing 0 clears)".
- **U16. Illegal-vector handling:** which ESR bit a local LVT sets and when; whether illegal-vector IPIs and self-IPIs are delivered; error recording while software-disabled; the consequence of a non-zero vector with SMI or INIT.
- **U17. SELF IPI bits 63:32.** The APM calls it a 32-bit register; the PPR marks 63:8 Reserved.
- **U18. EOI and SEOI edge cases:** EOI with an empty ISR; SEOI for a vector not in service (the PPR says "undefined"); IER writes with IERN=0; SEOI writes with SN=0.
- **U19. Destination edge cases:** whether DEST=FFh is still a broadcast in x2APIC physical mode; how x2APIC_ID bits 31:20 are represented in the LDR.
- **U20. PPS when PP ≠ TP.**
- **U21. Meaning of ExtControl XAIDN (bit 2) in x2APIC mode.**
- **U22. Directed EOI / EOI-broadcast suppression.** The APM defines neither a support bit nor a control bit. The PPR declares DirectedEoiSupport=1 but defines no SVR enable bit.
- **U23. Timer LVT MT field.** The generic Fig 16-7 (MT R/W on all LVTs) conflicts with Fig 16-8 and PPR 832h (11:8 reserved). The per-register reading was used above.
- **U24. Lowest-priority and ExtINT in the x2APIC ICR.** The APM says reserved; PPR 830h lists them as valid.
- **U25. Nested-interrupt wording:** p648 says IRR, p652 says ISR.
- **U26. LINT0/1 "input pin polarity".** Listed in §16.4 but no bit is defined, and bit 13 is MBZ.
