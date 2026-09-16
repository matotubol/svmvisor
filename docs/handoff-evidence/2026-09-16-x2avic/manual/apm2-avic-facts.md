# AMD APM Vol.2 section 15.29 (AVIC / x2AVIC) fact sheet

Reviewer: read-only manual review, 2026-09-16. Method: every rule below was read from rendered page
images (PyMuPDF renders in `work/x2avic-manual-2026-09-16/apm-avic/renders/`), not from extracted text.
Text search was used only to find pages. Repo summaries were not used as evidence.

Citation key: `p.N` = printed page number read from the image footer; `PDF N` = one-based PDF page;
`idx N` = zero-based page index (PDF - 1). Unless a line says V3 or IOMMU, the citation is Vol.2.
"Inference" marks a conclusion that the manual does not state in so many words.

## 0. Sources

| Key | Document | Revision | File | SHA256 | Pages | Applicability |
|---|---|---|---|---|---|---|
| V2 | AMD64 Architecture Programmer's Manual Vol.2 (pub. 24593) | 3.44, March 2026 | docs/24593_3.44_APM_Vol2.pdf | 3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c (matches the expected hash) | 845 | Primary source. 15.29 spans p.563-583 = PDF 625-645 = idx 624-644 (checked from the footers). Table B-1 (VMCB control area) is on p.736-742 (the AVIC rows are p.740-742 = PDF 802-804); Table C-1 is p.755-758 (the 401h/402h rows are p.758 = PDF 820). On every body page read, printed page = PDF page - 62. Front-matter PDF 36 = p.xxxvi. |
| V3 | AMD64 Architecture Programmer's Manual Vol.3 (pub. 24594) | 3.37, July 2025 | docs/24594_3.37_APM_Vol3.pdf | c77a21e75e49b645f9588df36af122f2c80039b7b0e35d27a5d642cd10a571d4 | 712 | Used only for the CPUID Fn8000_000A bits (E.4.9: p.653 = PDF 687, p.654 = PDF 688) and for WRMSR (p.511 = PDF 546). The PDF-to-printed offset is 34 or 35 depending on the page. |
| IOMMU | AMD I/O Virtualization Technology (IOMMU) Specification (pub. 48882) | 3.11, Apr 2026 | docs/48882-3.11.pdf | f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22 | 313 | Used only for the device-interrupt/doorbell chain that starts at V2 15.29.6.2. Pages p.93, 94, 186-188 (printed page = PDF page). |

Revision note: the V2 revision history (p.xxxvi, PDF 36, idx 35) says 3.44 changed only Figure 15-32
(Secure AVIC). 3.43 (June 2025) added "Support for up to 4096 vCPUs in x2AVIC mode in Section 15.29".
The 15.29 text reviewed here is therefore the 3.43 text.

Profile under review (from the coordinator): x2AVIC, NPT enabled, V_INTR_MASKING=1, non-SEV, each vCPU
pinned 1:1 to its core, guest APIC ID = host x2APIC ID.

---

## 1. Answers

### Q1. VMCB fields, required values, VMRUN failures

**Rules**

Sources: Table B-1 (p.740/PDF 802/idx 801; p.741/PDF 803/idx 802; p.742/PDF 804/idx 803) and 15.29.4.1-15.29.4.3 (p.570-571/PDF 632-633/idx 631-632).

**Field layout**
- **060h bit 31 = AVIC Enable** (p.741). AVIC "may be enabled on a per virtual processor basis" (p.570).
- **060h bit 30 = x2AVIC Enable** ("x2AVIC Enable (see 'x2AVIC' on page 582)", p.740).
  - p.570: "When this bit is set to 1 on a VMRUN, AVIC Enable also has to be set to 1. If not the VMRUN fails with a VMEXIT_INVALID error code."
  - Bit 30 = 0 selects xAVIC ("used for MMIO local APIC register interface"); bit 30 = 1 selects x2AVIC ("used for MSR local APIC register interface") (p.570).
- **060h bit 24 = V_INTR_MASKING** (p.740).
- **060h bits 7:0 = V_TPR.** "Bits 3:0 are used for a 4-bit virtual TPR value; bits 7:4 are SBZ." The value is written back to the VMCB at #VMEXIT (p.740).
- **060h reserved bits (SBZ):** 15:13, 23:21, 29:27 and 63:40. Table B-1 has no row for bit 10.
- **098h = AVIC APIC_BAR.** Bits 51:0 hold the value; bits 63:52 are SBZ (p.741).
  - 15.29.4.2 (p.570): this is the guest APIC base as a guest physical address.
  - p.583: "V_APIC_BAR and Logical Destination Table are not used in x2AVIC mode."
- **0E0h = AVIC APIC_BACKING_PAGE Pointer.** Bits 51:0 hold the value; bits 63:52 are SBZ (p.742). p.570: "52-bit HPA pointer to the vAPIC backing page for this virtual processor."
- **0E8h-0EFh:** "RESERVED, SBZ" (p.742).
- **0F0h = AVIC LOGICAL_TABLE Pointer.** Bits 51:12 hold the pointer; bits 11:0 and 63:52 are SBZ (p.742). Hardware does not use it in x2AVIC mode (p.574, p.583).
- **0F8h = AVIC PHYSICAL_TABLE Pointer[51:12]**, plus **AVIC_PHYSICAL_MAX_INDEX in bits 11:0** (a 12-bit field). Bits 63:52 are SBZ (p.742). p.570: MAX_INDEX is the "index of the last guest physical core ID for this guest".
- **15.29.4.3 (p.571), rules for all the pointers:**
  - "All of the physical addresses ... must point to legal, implementation-supported physical address ranges. These pointers are evaluated on VMRUN and cause a #VMEXIT if they are outside of the legal range."
  - "These memory ranges must be mapped as write-back cacheable memory type."
  - "All the addresses point to 4-Kbyte aligned data structures. Bits 11:0 are reserved (except for offset 0F8h) and should be set to zero."

**What offsets 090h and 0B8h contain**
- **090h (p.741):**
  - bit 0 = NP_ENABLE; bit 1 = SEV; bit 2 = SEV-ES; bit 3 = Guest Mode Execute Trap; bit 4 = SSSCheckEn; bit 5 = Virtual Transparent Encryption; bit 6 = Read Only Guest Page Tables.
  - bit 7 = INVLPGB/TLBSYNC enable: "0 - INVLPGB and TLBSYNC will result in #UD. 1 - INVLPGB and TLBSYNC can be executed in guest. Presence of this bit is indicated by CPUID bit 8000_000A, EDX[24] = 1. When in SEV-ES guest or this bit is not present, INVLPGB/TLBSYNC is always enabled in guest if supported by processor."
  - bits 63:8 are SBZ.
- **0B8h (p.742):** bit 0 = LBR virtualization; bit 1 = VMSAVE/VMLOAD virtualization; bit 2 = IBS virtualization; bit 3 = PMC virtualization; bits 63:4 are SBZ.
  - IBS and PMC virtualization each "requires the use of AVIC ... or NMI virtualization" (p.625-626). The dependency runs from IBS/PMC to AVIC. AVIC does not need any 0B8h bit.

**V_* fields ignored under AVIC**
- p.570: "Enabling AVIC implicitly disables the V_IRQ, V_INTR_PRIO, V_IGN_TPR, and V_INTR_VECTOR fields".
- p.579: "the V_IRQ, V_INTR_PRIO, V_INTR_VECTOR, and V_IGN_TPR fields in the VMCB are ignored."
- Table B-1 marks bit 8 (V_IRQ), bits 19:16 (V_INTR_PRIO), bit 20 (V_IGN_TPR) and bits 39:32 (V_INTR_VECTOR) as "ignored on VMRUN when AVIC is enabled" (p.740-741).
- V_TPR and V_INTR_MASKING are **not** on the ignored list. V_TPR is synchronized by the hardware (see Q8).

**Nested paging**
- p.570: "Any guest configured to use AVIC must also enable nested paging."
- No VMRUN consequence is stated for breaking this rule.

**VMRUN failures that the manual states**
- (a) x2AVIC Enable = 1 with AVIC Enable = 0 → VMEXIT_INVALID (p.570).
- (b) MAX_INDEX > 255 in xAVIC mode → VMEXIT_INVALID (p.571).
- (c) MAX_INDEX > 511 in x2AVIC mode when CPUID Fn8000_000A_ECX[x2AVIC_EXT] (bit 6) = 0 → VMEXIT_INVALID (p.571).
- (d) Any AVIC pointer outside the legal address range → "#VMEXIT" (the exit code is not named) (p.571).
- (e) When SEV-SNP is globally enabled, an AVIC backing page that is not hypervisor-owned → VMEXIT_INVALID (Table 15-39, p.612/PDF 674).
- (f) An SNP-active guest that uses Restricted or Alternate Injection with AVIC → VMEXIT_INVALID (p.614). Not applicable to this profile.
- The generic consistency-check list (p.504-505/PDF 566-567) contains no AVIC item and no NPT-required item.

**VMCB clean bits** (Figure 15-4, p.527/PDF 589/idx 588)
- Bit 11 (AVIC) covers "AVIC APIC_BAR; AVIC APIC_BACKING_PAGE, AVIC PHYSICAL_TABLE and AVIC LOGICAL_TABLE Pointers".
- Bit 3 (TPR) covers "V_TPR, V_IRQ, V_INTR_PRIO, V_IGN_TPR, V_INTR_MASKING, V_INTR_VECTOR (Offset 60h-67h)".
- The whole clean field must be 0 on the guest's first run, when the guest runs on a different core, or after the VMCB has moved (p.526).

**Implementation implication**
- **060h bits 31:30 = 11b and bit 24 = 1:** correct.
  - Keep bits 15:13, 23:21, 29:27 and 63:40 at zero.
  - Keep bit 10 at zero too, since the manual does not document it.
- **`0x090 == 1`:** meets the "must enable nested paging" rule, and leaving every other bit at zero is legal for a non-SEV guest.
  - Side effect: on a CPU with CPUID 8000_000A EDX[24] = 1, bit 7 = 0 makes guest INVLPGB/TLBSYNC raise #UD (p.741). This is not an AVIC issue, but flag it if the guest (the host OS) uses INVLPGB.
- **`0x0B8 == 0`:** legal. All four bits are optional features that AVIC does not require.
- **`0x0E8 == 0`:** required (reserved, SBZ).
- **`0x098 == 0` and `0x0F0 == 0`:** consistent with the manual.
  - The manual says both fields are unused in x2AVIC mode, and 0 meets the 4K-alignment and zero-low-bits rules.
  - However, the manual never says 0 is acceptable (U3).
- **0E0h:** must be a 4K-aligned host physical address with bits 11:0 and 63:52 at zero.
- **0F8h:** holds HPA[51:12] | MAX_INDEX. MAX_INDEX must be ≤ 511 unless x2AVIC_EXT = 1.
- **Clean bits:** clear bit 11 whenever 098h, 0E0h, 0F0h or 0F8h changes (MAX_INDEX included). Clear bit 3 whenever 060h-067h changes. Whether bit 3 also covers bits 30 and 31 is not stated (U22).
- **NPT:** enforce "AVIC implies NPT" in software. Do not rely on VMRUN failing (U1).

### Q2. Physical APIC ID table

**Rules**

**Entry format** (Figure 15-17 / Table 15-23, p.572/PDF 634/idx 633)
- **Bit 63, V (Valid):** "When set, indicates that this entry contains a valid vAPIC backing page pointer. If cleared, this table entry contains no information."
- **Bit 62, IR (IsRunning):** "IsRunning. This bit indicates that the corresponding guest virtual processor is currently scheduled by the VMM to run on a physical core."
- **Bits 61:52:** "Reserved, SBZ. Should always be set to zero."
- **Bits 51:12, Backing Page Pointer:** "4-Kbyte aligned HPA of the vAPIC backing page for this virtual processor."
- **Bits 11:0, Host Physical APIC ID:** "Physical APIC ID of the physical core allocated by the VMM to host the guest virtual processor. This field is not valid unless the IsRunning bit is set."
- **The host physical APIC ID field is 12 bits wide.**
- p.572 on IR: "Note that the IR bit, when set, indicates that the VMM has assigned a physical core to host this virtual processor. The bit does not differentiate between a physical processor running in guest mode (actively executing guest software) or in host mode (having suspended the execution of guest software)."

**Table rules** (15.29.5.2, p.571/PDF 633/idx 632)
- "The physical APIC ID table is set up and maintained by the VMM ... One physical APIC ID table must be provided per virtual machine."
- "The guest physical APIC ID is used as an index into this table."
- "If CPUID Fn8000_000A_ECX[x2AVIC_EXT] (bit 6) = 0, the length of this table is fixed at 4 Kbytes allowing a maximum of 512 virtual processors per virtual machine."
- "If CPUID Fn8000_000A_ECX[x2AVIC_EXT] (bit 6) = 1, the length of this table is up to eight consecutive 4-Kbyte pages, where the number of pages is equal to AVIC_PHYSICAL_MAX_INDEX[11:9] plus 1."
- "The physical ID table can be populated in a sparse manner using the valid bit to indicate assigned IDs. The index of the last valid entry is stored in the VMCB AVIC_PHYSICAL_MAX_INDEX field."
- p.572: the table pointer "is the same for every virtual processor within the virtual machine."

**Maximum guest index**
- **xAVIC:** the index must be ≤ 255 (checked by VMRUN, p.571).
  - "Since a destination of FFh is used to specify a broadcast, physical APIC ID FFh is reserved. The upper 2048 bytes of the table are reserved and should be set to zero." (p.573/PDF 635)
  - The table occupies "the lower half of a single 4-Kbyte memory page" (Figure 15-18: entries 0-254, entry 255 reserved at byte 2040).
- **x2AVIC with x2AVIC_EXT = 0:** the index must be ≤ 511 (checked by VMRUN, p.571).
- **x2AVIC with x2AVIC_EXT = 1:** V3 E.4.9 (p.653/PDF 687/idx 686) says bit 6 "x2AVIC_EXT: 4096 vCPUs supported in x2AVIC mode".
  - The 12-bit field therefore implies a maximum index of 4095 (inference).
  - No VMRUN limit check is stated for this case.

**Table memory**
- The table must be at a 4K-aligned host physical address, inside the legal range (checked at VMRUN), and mapped WB (p.571).
- Under SNP, VMRUN does not check the AVIC tables, because the hardware only reads them (p.612).

**Implementation implication**
- **Host ID limit:** because the table index = guest APIC ID = host x2APIC ID, every host x2APIC ID must be ≤ 511.
  - Alternatively, x2AVIC_EXT = 1 allows IDs up to 4095 with a table of MAX_INDEX[11:9]+1 consecutive pages.
- **MAX_INDEX:** must be ≥ the highest ID in use.
- **Entry bits:** host IDs up to 4095 fit in bits 11:0. Bits 61:52 must be zero.
- **Sharing:** one shared table page for all vCPUs matches the manual.
- **IR set once and never cleared:** this matches the literal definition (a core is assigned, and IR does not distinguish guest mode from host mode). Consequences:
  - Hardware never reports ID 1 ("target not running") for these vCPUs.
  - Hardware always sets IRR and sends a doorbell to the target core, even while that core is in host mode. What a doorbell does in host mode is unspecified (U5).
  - IRR is still evaluated at the next VMRUN (p.579), so an IPI that arrives while the target is in host mode is not lost.
- **Entry 255:** the manual does not say whether entry 255 is reserved or special in x2AVIC (U7). Treat a host x2APIC ID of 255 as a risk.

### Q3. Table 15-22 (complete) for x2AVIC

**Definitions** (15.29.3.1, p.566/PDF 628/idx 627)
- **Allow:** "Writes update the backing page value, while reads return the current value. In certain cases, a write results in specific hardware-based acceleration actions."
- **Fault:** "The processor performs an SVM intercept before the access. Causes a #VMEXIT."
- **Trap:** "The processor performs an SVM intercept immediately after the access completes. Causes a #VMEXIT."
- The table has a single behavior column, headed "xAVIC and x2AVIC Register Access Behavior", so both modes share it.
- General trap rule (15.7.1, p.508/PDF 570): "a trap intercept takes place after the execution of the instruction that triggered the trap in the first place. The saved guest state thus includes the effects of executing that instruction."

**Table 15-22** (p.566-568, PDF 628-630, idx 627-629). Columns 1-3 are from the manual; the Read and Write columns give the access behavior.

| MMIO off | x2APIC MSR | Register | Read | Write |
|---|---|---|---|---|
| 20h | 802h | APIC ID | Allowed | trap |
| 30h | 803h | Version | Allowed | fault |
| 80h | 808h | TPR | Allowed | Accelerated |
| 90h | 809h | APR | fault | fault |
| A0h | 80Ah | PPR | Allowed | fault |
| B0h | 80Bh | EOI | Allowed | "Accelerated by AVIC for edge-triggered interrupts or #VMEXIT (trap) for level triggered interrupts" |
| C0h | - | Remote Read | Allowed | trap |
| D0h | 80Dh | LDR | Allowed | trap |
| E0h | - | DFR | Allowed | trap |
| F0h | 80Fh | SVR | Allowed | trap |
| 100h-170h | 810h-817h | ISR | Allowed | fault |
| 180h-1F0h | 818h-81Fh | TMR | Allowed | fault |
| 200h-270h | 820h-827h | IRR | Allowed | fault |
| 280h | 828h | ESR | Allowed | trap |
| 300h | 830h | ICRL | Allowed | "Accelerated by AVIC or #VMEXIT (trap) for advanced functions." |
| 310h | - | ICRH | "Allowed (xAVIC)" | "Allowed (xAVIC)" |
| 320h | 832h | Timer LVT | Allowed | trap |
| 330h | 833h | Thermal LVT | Allowed | trap |
| 340h | 834h | Perf Counter LVT | Allowed | trap |
| 350h | 835h | LINT0 LVT | Allowed | trap |
| 360h | 836h | LINT1 LVT | Allowed | trap |
| 370h | 837h | Error LVT | Allowed | trap |
| 380h | 838h | Timer Initial Count | Allowed | trap |
| 390h | 839h | Timer Current Count | fault | fault |
| 3E0h | 83Eh | Timer Divide Config | Allowed | trap |
| - | 83Fh | Self IPI (x2APIC only) | (no entry) | "Allowed (x2AVIC)" |
| 400h | 840h | Ext APIC Feature | fault | fault |
| 410h | 841h | Ext APIC Control | fault | fault |
| 420h | 842h | SEOI | fault | fault |
| 480h-4F0h | 848h-84Fh | IER | fault | fault |
| 500h-530h | 850h-853h | Ext Interrupt [3:0] LVT | EDX[27]=1: Allowed; EDX[27]=0: fault | EDX[27]=1: trap; EDX[27]=0: fault |
| 540h-FFFh | - | Reserved | fault | fault |

**Notes to the table**
- EDX[27] is CPUID Fn8000_000A_EDX bit 27, ExtLvtAvicAccessChg (V3 p.654/PDF 688).
- p.568: "Accesses to any other register locations not explicitly defined in this table are allowed to read and write the backing page."
- p.568: "All vAPIC registers are 32-bits wide and are located at 16-byte aligned offsets. The results of an attempted read or write of any bytes in the range [register_offset + 4:register_offset + 15] are undefined."
- **15.29.10 (p.583/PDF 645/idx 644):**
  - "In x2AVIC mode ICR MSR bits 31:0 and 63:32 are mapped to ICRL (offset 300h) and ICRH (offset 310h) in the backing page."
  - "SELF_IPI MSR (83Fh) acceleration is handled the same way as ICR MSR acceleration."
  - "x2APIC MSR intercept checks and access checks have higher priority than AVIC access permission checks."
- **x2APIC MSR numbering (16.11.1, p.657/PDF 719):**
  - x2APIC MSR address = 800h + (MMIO offset >> 4).
  - Exceptions: ICR is merged at 830h; Self IPI is at 83Fh; DFR and RRR do not exist, so "MSR addresses 80Eh and 80Ch are not used and are reserved".
  - p.659: "A #GP(0) exception is generated if a WRMSR or an RDMSR instruction attempts to access an unimplemented MSR in the x2APIC address range."

**RIP handling (inference, based on 15.7.1)**
- **Trap:** the write has already completed and the backing page is updated. Saved RIP is past the WRMSR, so the VMM must not advance RIP.
- **Fault:** nothing was accessed. Saved RIP is at the RDMSR/WRMSR, so the VMM emulates the access and then advances RIP.
- 15.29 itself never states either RIP rule.
- **nRIP (p.509/PDF 571):** nRIP is "saved ... on all #VMEXITs that are due to instruction intercepts, as defined in Section 15.9, as well as MSR and IOIO intercepts and exceptions caused by the INT3, INTO, and BOUND instructions. For all other intercepts, nRIP is reset to zero."
  - AVIC_NOACCEL and AVIC_INCOMPLETE_IPI are not in that list, so read literally nRIP is 0 on those exits (U10).

**Register-by-register (x2AVIC)**
- **APIC ID (802h):** the read is served from the backing page; the write traps. However, x2APIC rules say "Attempting to write MSR 802h ... causes a #GP(0)" (p.660). Which rule wins is unresolved (U12).
- **Version (803h):** the read is served from the backing page; the write faults.
- **TPR (808h):** the read is served from the backing page; the write is accelerated (see Q8).
- **PPR (80Ah):** the read is served from the backing page, and "AVIC hardware maintains the PPR value" (p.569). The write faults: writes "cause a #VMEXIT without updating the value in the backing page" (p.569).
- **APR (809h):** the read faults and the write faults.
- **EOI (80Bh):** the write is accelerated for edge-triggered vectors and traps for level-triggered ones. The table allows reads, but x2APIC defines EOI as write-only (U12).
- **LDR (80Dh):** the read is served from the backing page; the write traps. x2APIC defines LDR as read-only (Table 16-6), so this conflicts (U12).
- **SVR (80Fh):** the read is served from the backing page; the write traps.
- **ISR/TMR/IRR (810h-827h):** reads are served from the backing page; writes fault.
- **ESR (828h):** the read is served from the backing page; the write traps. x2APIC says a non-zero write raises #GP(0) (p.658-659) (U12).
- **ICR (830h):** the read is served from the backing page; the write is accelerated (see Q4), or traps for "advanced functions".
- **LVT timer, thermal, perf, LINT0, LINT1, error (832h-837h):** reads are served from the backing page; writes trap.
- **Initial count (838h):** the read is served from the backing page; the write traps.
- **Current count (839h):** the read faults and the write faults.
- **Divide config (83Eh):** the read is served from the backing page; the write traps.
- **SELF IPI (83Fh):** the write is accelerated like ICR. The table has no read entry, but x2APIC says a read raises #GP(0) (p.662) (U12).
- **840h-842h and 848h-84Fh:** both directions fault.
- **850h-853h:** behavior depends on EDX[27].
- **x2APIC MSRs not in the table** (800h, 801h, 804h-807h, 80Ch, 80Eh, 831h, 83Ah-83Dh, 843h-847h, 854h-8FFh): unresolved (U12). Two statements conflict:
  - Table 15-22: "any other register locations ... allowed to read and write the backing page".
  - x2APIC: unimplemented MSRs raise #GP(0), and 15.29.10 says access checks run before AVIC permission checks.

**Implementation implication**
- **The MSRPM passes 800h-83Fh through**, so the AVIC_NOACCEL handler must cover the following cases:
  - Fault reads: 809h.
  - Fault writes: 803h, 809h, 80Ah, 810h-827h, 839h.
  - Trap writes: 80Fh, 828h, 832h-838h and 83Eh, plus 802h and 80Dh if hardware delivers them rather than raising #GP (U12).
  - The level-triggered EOI exit.
- **A read of 839h** arrives as VMEXIT_MSR, because the MSRPM intercept takes priority. nRIP is valid on that exit.
- **840h-8FFh** also arrive as VMEXIT_MSR. The VMM emulates 840h-853h and injects #GP for the unimplemented MSRs in that range.
- **Trap handlers** read the new value from the backing page and leave RIP alone.
- **Fault handlers** emulate the access and advance RIP. Do not trust nRIP until it is checked on hardware.
- **APIC timer:** the manual describes no hardware timer virtualization (LVT and initial-count writes trap; current count faults). The VMM must emulate the timer.

### Q4. IPI acceleration

**Rules**
- **p.569 (PDF 631/idx 630):**
  - "Writes to the ICRL register have the side-effect of initiating the generation of an interprocessor interrupt (IPI) based on the values written to the fields in both the ICRL and ICRH registers."
  - "AVIC hardware handles the generation of IPIs when the specified Message Type is Fixed (also known as fixed delivery mode) and the Trigger Mode is edge-triggered."
  - "The hardware also supports self and broadcast delivery modes specified via the Destination Shorthand (DSH) field of the ICRL. Logical and physical APIC ID formats are supported. All other IPI types cause a #VMEXIT."
- **p.573 (PDF 635):** "AVIC hardware supports the fixed interrupt message type targeting one or more logical destinations. The hardware also supports self and broadcast delivery modes specified via the Destination Shorthand (DSH) field of the ICRL. Any other message types must be supported through emulation by the VMM."
- **p.578 (15.29.8.1):** "Writes to the ICRL register with simple functional side effects such as the generation of a directed IPI or a self-IPI request are handled directly. Values written to the ICRL defined to initiate more complex behavior cause a #VMEXIT to allow the VMM to emulate the function."
- **p.583:** in x2AVIC mode the ICR MSR (830h) is split across 300h and 310h, and 83Fh is accelerated the same way as ICR.
- **Delivery algorithm (15.29.6.1, p.576-577, PDF 638-639, idx 637-638):**
  1. "If the destination-shorthand coded in the command is 01b (i.e. self), update the IRR in the backing page, signal doorbell to self and skip remaining steps."
  2. "If destination-shorthand is non-zero, or if the destination field is FFh (i.e. broadcast), jump to step 4."
  3. For a logical destination:
     - Look up the guest physical APIC ID in the Logical APIC ID table. "If the entry is not valid (V bit is cleared), cause a #VMEXIT."
     - "If the entry is valid, but the Guest Physical APIC ID is greater than 255, cause a #VMEXIT (xAVIC)."
     - "If the entry is valid, but contains an invalid backing page pointer, cause a #VMEXIT."
  4. "Lookup the vAPIC backing page address in the Physical APIC table using the guest physical APIC ID as an index into the table. For directed interrupts, if the selected table entry is not valid, cause a #VMEXIT. For broadcast IPIs, invalid entries are ignored."
  5. "For every valid destination:
     - Atomically set the appropriate IRR bit in each of the destinations' vAPIC backing page.
     - Check the IsRunning status of each destination.
     - If the destination IsRunning bit is set, send a doorbell message using the host physical core number from the Physical APIC ID table."
  6. "If any destinations are identified as not currently scheduled on a physical core (i.e., the IsRunning bit for that virtual processor is not set), cause a #VMEXIT."
- **x2AVIC logical IDs (p.574/PDF 636/idx 635):** "The Logical APIC ID Table is not used by AVIC hardware in x2AVIC mode. Instead, the destination logical ID is derived from the target x2APIC ID as follows: Logical x2APICID = (X2APICID[19:4] << 16) | (1 << x2APICID[3:0])."
  - This matches 16.14 (p.662/PDF 724): "logical_id[15:0] = 1 << x2APIC_ID[3:0] and cluster_id[15:0] = x2APIC_ID[19:4]".
  - An x2APIC accepts a logical destination when bits 31:16 equal its cluster ID and any bit in 15:0 matches.
  - "The legacy 'flat logical' addressing is not supported in x2APIC mode."
- **x2APIC ICR (Chapter 16):**
  - DEST is 32 bits wide. "A DEST value of FFFF_FFFFh is used to broadcast IPIs to all local APICs" (p.660, p.662).
  - Message-type encodings 1 (lowest priority), 3 (remote read) and 7 (ExtINT) "are eliminated and the encodings are reserved".
  - The DS bit (12) and RRS bits (17:16) must be zero (p.661/PDF 723).
- **Exit reasons that limit acceleration:** Table 15-27 ID 0 (level trigger or unsupported destination type) and ID 4 (vector < 16) (p.581).

**What the hardware accelerates in x2AVIC mode, per the manual**
- **Message type:** Fixed only. Lowest priority, SMI, NMI, INIT, STARTUP and ExtINT cause an exit and need VMM emulation.
- **Trigger mode:** edge only. Level-triggered IPIs cause an exit (ID 0).
- **Destination mode:** physical and logical are both accelerated. x2AVIC logical delivery uses the formula above, not the logical table.
- **Shorthands:** self (01b) is accelerated. Broadcast DSH modes are accelerated: step 2 sends any non-zero DSH to the broadcast path, which covers 10b and 11b. The manual does not describe how "excluding self" is applied for 11b.
- **Broadcast destination value:** 15.29 mentions only "destination field is FFh". It never mentions the x2APIC broadcast value FFFF_FFFFh (U7).
- **Vector:** a vector below 16 causes an exit (ID 4).
- **Are IRR bits written before the exit?**
  - **Not-running case:** yes. Step 5 sets IRR in every valid destination (running or not) and doorbells the running ones before step 6 exits.
  - **Directed IPI with an invalid entry (steps 3 and 4):** the exit happens before step 5, so no IRR bit is written (inference from the step order).

**Implementation implication**
- **INIT, SIPI, NMI, SMI and level-triggered IPIs** always need software. This matches the mailbox design.
- **Software fan-out must depend on the exit reason:**
  - **ID 1:** hardware has already set IRR in every valid target and doorbelled the running ones. Setting IRR again could re-deliver a vector that a target has already accepted and EOI'd, causing a duplicate interrupt.
  - **ID 0, ID 2, ID 4, and ID 3 for directed IPIs:** nothing was delivered.
- **LDR in the backing page:** hardware ignores the guest LDR in x2AVIC mode, and the VMM initializes the backing page (p.566). So the VMM should set backing-page LDR (offset 0D0h) to the formula value, so that guest reads of 80Dh are consistent.
- **Consistency with native x2APIC:** because guest ID = host x2APIC ID, the hardware-derived logical IDs equal the ones native x2APIC would compute.

### Q5. AVIC_INCOMPLETE_IPI (exit code 401h)

**Rules** (15.29.9.1, p.580-581, PDF 642-643, idx 641-642)
- **Cause:** "An IPI could not be delivered to all targeted guest virtual processors because at least one guest virtual processor was not allocated to a physical core at the time. This results in a #VMEXIT with an exit code of AVIC_INCOMPLETE_IPI."
- **EXITINFO1** (Figure 15-23 / Table 15-25):
  - Bits 63:32 = ICRH: "Value written to the vAPIC ICRH register."
  - Bits 31:0 = ICRL: "Value written to the vAPIC ICRL register."
- **EXITINFO2** (Figure 15-24 / Table 15-26):
  - Bits 63:32 = ID (the specific reason, per Table 15-27).
  - Bits 31:12 are reserved.
  - Bits 11:0 = Index: "For ID = 1 - 3, this field provides the index of a logical or physical table entry. Reserved for all other ID values."

**Table 15-27** (p.581)

| ID | Cause | Description | Index |
|---|---|---|---|
| 0 | Invalid Interrupt type | "The trigger mode for the specified IPI was set to level or the destination type is unsupported." | Reserved |
| 1 | IPI Target Not Running | "IsRunning bit of the target for a Singlecast/Broadcast/Multicast IPI is not set in the physical APIC ID table." | "Index of the physical or logical APIC ID table entry for the target virtual processor that was not scheduled on a physical core." |
| 2 | Invalid IPI Target | "Target ID invalid. Target is not covered by the physical or logical ID table." | "Index of the physical or logical table entry for the invalid target." |
| 3 | Invalid Backing Page Pointer | "The vAPIC Backing Page Pointer field of the Physical APIC ID Table contained an invalid physical address." | "For shorthand or broadcast delivery modes, index of the physical APIC ID Table containing the invalid address. For directed IPIs, index of the logical or physical APIC ID table depending on the destination mode." |
| 4 | Invalid IPI Vector | "The vector for the specified IPI was set to an illegal value (VEC < 16)." | Reserved |
| 5 | Un-accelerated IPI | "Destination Shorthand is not set to Self (Secure AVIC)." | Reserved |
| >5 | Reserved | | |

**State of the ICR write**
- Table 15-22 lists the ICRL write as "#VMEXIT (trap)" (p.567), so the write has completed.
- EXITINFO1 is described as the "Value written" to ICRL and ICRH.
- RIP is past the WRMSR by the 15.7.1 trap rule, but 15.29 does not say so.
- nRIP is 0 under the general rule (U10).
- The manual gives no per-ID trap-or-fault statement.

**What the hardware has already done, by reason** (from the 15.29.6.1 step order)
- **ID 1 (target not running):**
  - Done: IRR set in every valid target (running and not running), a doorbell sent to every target with IR = 1, then the exit.
  - VMM: get the non-running target(s) scheduled. VMRUN evaluates IRR (p.579). Do not inject again.
  - Index: the index of the non-running target. In x2AVIC logical mode there is no logical table, and the manual does not say which index is reported. If several targets are not running, it does not say which one is reported (U9).
- **ID 2 (invalid target):**
  - Done: for a directed IPI, the exit happens at step 3 or 4, before any IRR write. Broadcast IPIs skip invalid entries, so ID 2 comes from directed IPIs.
  - VMM: emulate the IPI (a missing destination is simply not delivered, as on real APICs).
- **ID 3 (invalid backing-page pointer):**
  - Done: for a directed IPI, the exit happens before any IRR write. For a broadcast IPI, the manual does not say whether other targets' IRR bits were already written (U9).
- **ID 0 and ID 4:**
  - Done: the algorithm does not write IRR before these checks (inference; the check point is not stated).
  - VMM: emulate the whole ICR write. For an illegal vector the manual leaves the handling to the VMM (APIC error semantics).
- **ID 5:** Secure AVIC only; not applicable.

**Implementation implication**
- **Decode:** EXITINFO1 bits 31:0 are the ICR low half; bits 63:32 are the 32-bit x2APIC destination.
- **RIP:** do not advance it (trap). Confirm this on hardware (U10).
- **ID 1 should not occur here:** IR is permanently 1 for every valid vCPU. If ID 1 does appear, an entry has IR = 0 (for example, before arming), and the handler must still avoid double delivery.
- **INIT/SIPI mailbox:** the manual does not say which ID hardware reports for non-Fixed message types (U8). Route INIT/SIPI by decoding the ICR fields, not by relying on a particular ID.

### Q6. AVIC_NOACCEL (exit code 402h)

**Rules** (15.29.9.2, p.581-582, PDF 643-644, idx 642-643)
- **Cause:** "A guest access to an APIC register that is not accelerated by AVIC results in a #VMEXIT with the exit code of AVIC_NOACCEL. This fault is also generated if an EOI is attempted when the highest priority in-service interrupt is set for level-triggered mode."
- **EXITINFO1** (Figure 15-25 / Table 15-28):
  - Bits 63:33 are reserved.
  - Bit 32 = R/W: "If set, write was attempted. If clear, read was attempted."
  - Bits 31:12 are reserved.
  - Bits 11:4 = APIC_Offset[11:4]: "Offset within virtual vAPIC backing page at which read or write was attempted. APIC_Offset[3:0] = 0, since all registers are aligned on 16-byte boundaries."
  - Bits 3:0 are reserved.
- **EXITINFO2** (Figure 15-26 / Table 15-29):
  - "If the EXITINFO1 fields indicate a write to the vAPIC EOI register (offset = B0h), bits 7:0 of this value contain the number of the highest in-service vector found in the virtual APIC ISR."
  - Bits 63:8 are reserved.
  - Bits 7:0 = Vector: "Vector for attempted EOI; otherwise undefined."
- **Trap or fault per register:** see Table 15-22 in Q3.
- **EOI behavior (p.569):**
  - "When the guest writes to the EOI register address, AVIC hardware clears the highest priority in-service interrupt (ISR) bit in the backing page and re-evaluates the interrupt state to determine if another pending interrupt should be delivered."
  - "If the highest priority in-service interrupt is set to level mode (in the corresponding TMR bit), the EOI write causes a #VMEXIT to allow the VMM to emulate the level-triggered behavior."
  - Table 15-22 says the same write is "#VMEXIT (trap) for level triggered interrupts" and accelerated for edge-triggered ones.
- **TMR on real APICs (16.6.3, p.648/PDF 710):** "When the interrupt is accepted by the local APIC and the IRR bit is set, the associated TMR bit is set for level-sensitive interrupts or reset for edge-triggered interrupts." An EOI message goes to the I/O APIC only when the TMR bit is set.

**Answers**
- **When an EOI exits:** only when the highest-priority in-service vector has its TMR bit set to 1.
  - If that bit is 0, hardware handles the EOI completely and there is no exit.
  - EXITINFO2[7:0] contains the vector.
- **Is the ISR bit already cleared at the exit?** Not settled. Table 15-22 calls the exit a trap (after the access), but 15.29.9.2 calls it "This fault" (U11).
- **RIP:** after the instruction for trap registers, before it for fault registers (inference from 15.7.1). nRIP is 0 under the general rule (U10).

**Implementation implication**
- **Decoding the MSR:** in x2AVIC mode the MSR number is presumably 800h + (APIC_Offset >> 4).
  - This is an inference from the 16.11.1 formula and the p.583 ICR mapping (offset 300h ↔ MSR 830h). 15.29 never states it for EXITINFO1 (U18).
- **TMR for level-triggered device vectors:** setting it gives the VMM an EOI exit with the vector, which it needs in order to forward the EOI to the (physical or virtual) I/O APIC.
- **After a level EOI exit:** check the backing-page ISR bit for the EXITINFO2 vector.
  - If the bit is still set, clear it (the next VMRUN re-evaluates pending interrupts).
  - If it is already clear, hardware has done it.
- **Edge vectors (TMR = 0):** never produce an EOI notification.
- **No EOI-exit bitmap:** the manual describes none. The only EOI signal is the TMR-driven exit.

### Q7. Doorbell

**Rules** (15.29.8.2, p.578-579, PDF 640-641, idx 639-640)
- **Mechanism:** "Each core provides a doorbell mechanism that is used by other cores (for IPIs) and the IOMMU (for device interrupts) to signal to the VMM of the target physical core that a virtual interrupt requires processing. The exact mechanism is implementation-specific, but must be protected from access from non-privileged software running on other cores and from direct access by an external device."
- **Receipt in guest mode:** "When the doorbell is received in guest mode, hardware on the receiving core evaluates the vAPIC state in the vAPIC backing page for the currently running virtual processor and injects the interrupt into the guest as appropriate."
- **Doorbell MSR:**
  - "Sending a doorbell signal to a another core is initiated by writing the physical APIC ID corresponding to that core to the Doorbell Register (MSR C001_011Bh)."
  - Figure 15-22: bits 63:8 are "Reserved, MBZ"; bits 7:0 are "Physical APIC ID".
  - "Writing to this register causes a doorbell signal to be sent to the specified physical core. The serializing semantics of WRMSR are relaxed when writing to the Doorbell Register. Any attempt to read from this register results in a #GP."
- **Processing:** "A doorbell signal delivered to a running guest is recognized by the hardware regardless of whether it can be immediately injected into the guest as a virtual interrupt. On the next VMRUN, the virtual interrupt delivery mechanism evaluates the state of the IRR register of the guest's vAPIC backing page to find the highest priority pending interrupt and injects it if interrupt masking and priority allow."
- **During VMRUN (15.29.8.3, p.579):** "Any doorbell signals received during VMRUN processing are recognized immediately after entering the guest".
- **Appendix A:**
  - Table A-1 (p.724/PDF 786/idx 785): "C001_011Bh | Doorbell Register | SVM | 'Doorbell Register' on page 579".
  - Table A-7 (p.733/PDF 795/idx 794): "Sends a doorbell signal to the specified physical APIC."
- **V3 WRMSR (p.511/PDF 546/idx 545):** requires CPL 0, otherwise #GP(0). "some x2APIC and AVIC MSRs may have relaxed serialization semantics".
- **IOMMU spec:**
  - p.94: after the atomic IRR set, "If IRTE[IsRun]=1b, then the IOMMU sends a guest APIC doorbell signal using the Destination field". If IsRun = 0 and GALogIntr = 1, the IOMMU writes a GA log entry.
  - p.187: "If the target virtual processor is running at the time of the interrupt request, the IOMMU completes the delivery of the interrupt by sending a doorbell interrupt to the physical processor that is hosting the virtual processor."
  - p.188: "The virtual interrupt remains pending until system software makes the targeted virtual processor active. When the virtual processor is again active it acts on the interrupt request that the IOMMU has already set in the IRR of the vAPIC backing page."

**Answers**
- **MSR index:** yes, C001_011Bh. It is write-only.
- **Value format:** an 8-bit physical APIC ID in bits 7:0, with bits 63:8 must-be-zero. The manual documents no wider format for x2APIC host IDs, even though the table entry field is 12 bits (U4).
- **Required conditions:** a WRMSR at CPL 0. Nothing else is stated.
- **Target core not in guest mode:** not stated. The manual does not say whether the doorbell is ignored, kept pending, or delivered to the host as an interrupt (and if so, with which vector) (U5). The only guarantees are:
  - a doorbell that arrives during VMRUN is recognized after guest entry;
  - the next VMRUN evaluates IRR.
- **Software IRR write for a target with IsRunning = 1:**
  - The manual lists only these evaluation points: VMRUN, a doorbell, and guest EOI/TPR/PPR changes.
  - So a target that is executing guest code needs a doorbell to notice a new IRR bit promptly (inference). The hardware IPI path and the IOMMU both send doorbells to IR = 1 targets.
  - A target that is in host mode picks the bit up at its next VMRUN.
- **Software IRR write for a target with IsRunning = 0:** no doorbell is needed.
  - Hardware reports ID 1 instead, and the IOMMU logs to the GA log.
  - Delivery happens at the target's next VMRUN, so the VMM must schedule the target.

**Implementation implication**
- **Planned remote wake:** WRMSR C001_011Bh with the target host APIC ID is documented only for host APIC IDs ≤ 255. For larger IDs no method is documented.
- **Host-mode doorbells:** because the manual does not define what a doorbell does in host mode, host wait paths must not rely on it.
  - The current design (handler runs with IF=0, then returns to VMRUN) picks up IRR on the next VMRUN, which covers this case.
- **Local publication:** publishing a device IRQ into the local core's backing page while that core is in host mode needs no doorbell.

### Q8. CR8, TPR, V_TPR, V_INTR_MASKING, host IF and INTR interception

**Rules**
- **CR8 under AVIC (p.570):** "Enabling AVIC also affects CR8 behavior independent of V_INTR_MASKING enable (bit 24): writes to CR8 affect the V_TPR and update the backing page and reads from CR8 return V_TPR."
- **TPR synchronization (p.568-569):**
  - On a guest TPR write, "the value is updated in the backing page and the upper 4 bits of the value are automatically copied by the hardware to the V_TPR value in the VMCB. All reads from the TPR location return the value from the vAPIC backing page. Also, any TPR accesses using the MOV CR8 semantics update the backing page and V_TPR values."
  - "Only the Task Priority bits of are maintained in the lower 4 bits of CR8 and V_TPR. The Task Priority Sub-class value is not stored. Writes to the memory-mapped TPR register update bits 3:0 of CR8 and V_TPR and writes to CR8 update the TPR backing page value bits 7:4 while bits 3:0 are set to zero." (Figure 15-16)
- **PPR (p.569):** "AVIC hardware updates the PPR value in the backing page when either the TPR value or the highest in-service interrupt changes. This value is used to control the delivery of virtual interrupts to the guest."
- **V_TPR in the VMCB:** loaded by VMRUN and written back by #VMEXIT. Bits 3:0 are used; bits 7:4 are SBZ (p.533, p.740).
- **15.21.1 (p.533/PDF 595/idx 594):**
  - With V_INTR_MASKING = 0: "EFLAGS.IF controls both virtual and physical interrupts."
  - With V_INTR_MASKING = 1: "The host EFLAGS.IF at the time of the VMRUN is saved and controls physical interrupts while the guest is running. The guest value of EFLAGS.IF controls virtual interrupts only."
- **15.21.2 (p.533):**
  - "The APIC's TPR always controls the task priority for physical interrupts, and the V_TPR always controls virtual interrupts."
  - With V_INTR_MASKING = 1: "Writes to CR8 affect only the V_TPR register." and "Reads from CR8 return V_TPR."
- **#VMEXIT (p.507/PDF 569):** clears GIF; writes V_IRQ, V_TPR and INTERRUPT_SHADOW back to the VMCB; and "Clears the V_IRQ and V_INTR_MASKING bits inside the processor."
- **VMRUN (p.503/PDF 565):** "the processor reenables interrupts by setting GIF to 1. It is assumed that VMM software cleared GIF some time before executing the VMRUN instruction".
- **INTR intercept (15.13, p.522/PDF 584):** "External interrupts, when intercepted, cause a #VMEXIT; the interrupt is held pending so that the interrupt can eventually be taken in the VMM." 15.13.1: "This intercept affects physical, as opposed to virtual, maskable interrupts."
- **Priority (p.534):** "Physical interrupts take priority over virtual interrupts, whether they are taken directly or through a #VMEXIT."
- **GIF (Table 15-10, p.530):** INTR and vINTR are "Held pending until GIF==1".
- **EFLAGS isolation (p.506):** "Host values of EFLAGS have no effect on the guest and guest values of EFLAGS have no effect on the host."

**Answers**
- **Is V_TPR synchronized with the backing-page TPR?** Yes, while the guest runs: hardware syncs the two on guest TPR-MSR writes and on CR8 writes.
  - V_TPR itself is loaded from the VMCB at VMRUN and saved at #VMEXIT.
  - The manual does not say that VMRUN reconciles V_TPR with backing-page TPR in either direction (U14).
- **In this profile (V_INTR_MASKING = 1):**
  - Guest IF gates only virtual interrupts; the host's IF value at VMRUN gates physical interrupts while the guest runs.
  - An intercepted physical INTR exits and stays pending. After #VMEXIT, GIF = 0 holds it until the host sets GIF = 1.

**Implementation implication**
- **Host IF at VMRUN matters.** If the host executes VMRUN with RFLAGS.IF = 0, p.533 says that saved IF "controls physical interrupts while the guest is running".
  - That would mask physical INTR for the whole guest time slice, so the INTR intercept would not be a dependable exit source. Whether the intercept still fires in that case is not stated (U15).
  - The pattern implied by p.503 and p.533 is: CLGI, then IF = 1, then VMRUN.
  - Check the IF value at the VMRUN instruction itself. Running the exit handler with IF = 0 is fine if VMRUN sees IF = 1.
- **VMM rewrites of guest TPR** (for example, INIT reset): write both backing page 80h and VMCB V_TPR (bits 3:0 = TPR >> 4), and clear clean bit 3.
- **No V_IRQ injection path:** V_IRQ and V_INTR_VECTOR are ignored under AVIC, so VMM-originated fixed interrupts must go through IRR. There is no V_IGN_TPR path for ExtINT-style injection.

### Q9. 15.29.10: MSR interception precedence and access checks

**Rules**
- **p.583:** "When x2AVIC mode is enabled, x2APIC MSR accesses are virtualized in a similar manner to MMIO accesses in xAVIC mode. x2APIC MSR intercept checks and access checks have higher priority than AVIC access permission checks. See Section 15.29.3.1 for x2APIC register access behavior when x2AVIC is enabled."
- **15.11 (p.518/PDF 580/idx 579):** "RDMSR and WRMSR instructions check for exceptions and intercepts in the following order:
  - Exceptions common to all MSRs (e.g., #GP if not at CPL 0)
  - Check SVM intercepts in the MSR permission map, if the MSR_PROT intercept is requested.
  - Exceptions specific to a given MSR (including password protection, unimplemented MSRs, reserved bits, etc.)"
- **MSRPM layout (p.518):**
  - "The lsb of the two bits controls read access to the MSR and the msb controls write access. A value of 1 indicates that the operation is intercepted."
  - Bytes 000h-7FFh cover MSRs 0000_0000h-0000_1FFFh, so 800h-8FFh are covered.
- **MSR intercept exit (p.519):** EXITINFO1 = 0 for RDMSR, 1 for WRMSR. nRIP is saved for MSR intercepts (p.509).
- **x2APIC access rules (Chapter 16):** each of these raises #GP(0):
  - using the MSR interface when not in x2APIC mode (p.657);
  - accessing an unimplemented MSR in 800h-8FFh (p.657, p.659);
  - writing 1 to a reserved bit (p.659);
  - writing 1 to bits 63:32 of any legacy register except ICR (p.659);
  - writing a non-zero value to ESR (p.658-659);
  - writing a non-zero value to EOI (Table 16-6, p.658);
  - writing 802h (p.660);
  - reading 83Fh (p.662).
  - Also, "The RDMSR instruction returns a zero for any reserved bit." (p.659)

**Answers**
- **Hardware order:**
  1. The CPL check (#GP).
  2. The MSRPM intercept (VMEXIT_MSR, 7Ch).
  3. The "access checks" (#GP delivered to the guest).
  4. AVIC permission handling: allow, trap or fault (the last two produce AVIC_NOACCEL, 402h), or acceleration.
- **Which Chapter 16 checks does x2AVIC hardware apply itself?** The manual does not list them. Only the CPL check is explicit (U12).
- **x2APIC MSRs that AVIC does not virtualize:**
  - MSRs marked fault or trap in Table 15-22 → AVIC_NOACCEL.
  - MSRs with no row in Table 15-22 → unresolved (U12).

**Implementation implication**
- **840h-8FFh:** because the MSRPM intercepts them, they never reach AVIC. The VMM owns #GP for the unimplemented ones and must emulate 840h-853h.
- **839h reads:** the MSRPM intercept gives VMEXIT_MSR, which carries a valid nRIP (p.509), so RIP can be advanced reliably.
- **800h-83Fh (passed through):**
  - Until U12 is resolved, the same register may produce a hardware #GP (never seen by the VMM) or an AVIC_NOACCEL exit.
  - The AVIC_NOACCEL handler should inject #GP wherever x2APIC semantics require it: writes to 802h, non-zero ESR or EOI writes, and writes to read-only registers.

### Q10. Backing-page and table requirements; concurrent IRR updates

**Rules**
- **Lifetime:**
  - p.564: the backing page "remains pinned in system memory as long as the virtual machine persists, even when the specific virtual processor associated with the backing page is not running."
  - p.566: "The vAPIC backing page must be present in system physical memory for the life of the guest VM because some fields are updated even when the guest is not running."
- **Initialization (p.566):** "The VMM initializes the backing page with appropriate default APIC register values including items such as APIC version number."
- **Memory (p.571):**
  - 4K-aligned; low 12 bits zero (0F8h excepted); inside the legal address range (checked at VMRUN); "must be mapped as write-back cacheable memory type".
  - The three structures are "defined to fit exactly in one 4-Kbyte page. Future implementations may expand the size."
  - "Each virtual processor in the system is assigned a virtual APIC backing page".
- **xAVIC only (p.565):** the NPT must grant read/write access at the APIC GPA. Hardware checks those permissions but uses the AVIC_BACKING_PAGE pointer instead of the leaf entry's system physical address.
- **Table entries (p.572):** entries hold 4K-aligned host physical addresses.
- **xAVIC multiprocessor VMs (p.571):** when a core runs a different vCPU of the same VM than it ran last time, flush with TLB_CONTROL = 3h.
- **SNP only (p.612):** the backing page must be hypervisor-owned, and it is marked in-use while the guest runs.
- **Concurrent updates:**
  - The hardware IPI path must "Atomically set the appropriate IRR bit" (p.577).
  - For device interrupts, the IOMMU "atomically sets the bit in the IRR" (p.577).
  - IOMMU spec p.94: "Atomically set one bit using the calculated bit index within the calculated target byte." Its hardware note adds that "the atomic-OR operation may use any byte width that is a power of 2 between 1 and 32 bytes, inclusive".
  - **The manual gives no rule for VMM software writes to IRR, TMR or ISR.**

**Implementation implication**
- **Each per-vCPU 4K backing page must:**
  - be a 4K-aligned host physical address, mapped WB, inside the legal range;
  - never be freed or moved while the VM exists, even when its vCPU is not running;
  - be initialized by the VMM: Version, LDR, SVR and masked LVTs, using the Table 16-2 reset values (p.631/PDF 693).
- **Physical table page:** 4K-aligned and mapped WB; one per VM.
- **Software IRR publication must be an atomic OR at a granularity that cannot drop concurrent updates** — for example, a lock-prefixed bit set or a lock-prefixed OR on a 32- or 64-bit word.
  - A non-atomic read-modify-write is unsafe: the IOMMU may OR up to 32 bytes at once, and the guest-side hardware clears IRR bits when it accepts interrupts.
- **TMR vs IRR ordering:** not specified (U17). Set TMR before IRR so that acceptance sees the correct trigger mode.
- **TLB-flush rule:** the xAVIC flush rule does not apply to 1:1-pinned x2AVIC (inference).

### Q11. How hardware decides x2APIC mode; APIC_BASE; guest MMIO access

**Rules**
- **p.570:** x2AVIC Enable bit = 0 selects xAVIC "(used for MMIO local APIC register interface)"; = 1 selects x2AVIC "(used for MSR local APIC register interface)".
- **p.583:**
  - "x2AVIC mode is enabled by setting AVIC Enable (bit 31) and x2AVIC Mode Enable (bit 30) in VMCB offset 60h to 1."
  - "V_APIC_BAR and Logical Destination Table are not used in x2AVIC mode."
- **APIC_BASE interception in xAVIC (p.566 and p.570):**
  - "If the guest attempts to relocate the vAPIC base address by writing to the APIC Base Address Register (MSR 0000_001Bh), the VMM should intercept the write to update the V_APIC_BAR field of the VMCB and the GPA part of translation in the host's nested page tables."
  - p.570: "Guest writes to the APIC_BASE register are intercepted by the VMM".
- **Chapter 16 rules for APIC_BASE:**
  - AE is bit 11 and EXTD is bit 10. AE = 0 with EXTD = 1 is invalid and raises #GP(0).
  - "Once the local APIC has been placed into x2APIC mode, the only valid transition (other than reset) is to 'APIC Disabled' mode"; any other transition raises #GP(0) (p.655/PDF 717).
  - RESET clears AE and EXTD. "An INIT does not modify the APIC Base Address Register AE and EXTD bits" (p.657).
  - "In x2APIC mode, the legacy MMIO access to the APIC register set is disabled. Attempts to use the legacy MMIO access mechanism may result in an unintended memory access or a memory-related exception such as #GP or #PF." (p.657)
- **No VMCB field holds the guest APIC_BASE MSR or its EXTD bit** (Table B-1, p.736-742).

**Answers**
- **Mode selection:** hardware takes the mode from VMCB 060h bit 30 only. The manual never says it reads a guest APIC_BASE or EXTD value.
- **Must the VMM intercept APIC_BASE?**
  - The manual's explicit interception rule is about relocation (V_APIC_BAR), and x2AVIC does not use V_APIC_BAR.
  - There is no x2AVIC-specific APIC_BASE rule. The need to intercept 1Bh to keep bit 30 and the virtual APIC state in step with the guest's mode transitions is an inference.
- **Guest MMIO access to the APIC while x2AVIC is enabled:** not stated (U13). Because V_APIC_BAR is unused, it is presumably an ordinary memory access governed by the NPT.

**Implementation implication**
- **Intercept 1Bh.** This is needed to emulate the AE/EXTD transition rules (#GP, p.655) and to switch or refuse AVIC modes.
- **Guest disables its APIC** (AE = EXTD = 0): bit 30 stays set unless the VMM clears it, and hardware would keep virtualizing x2APIC MSR accesses (no hardware check is described). The VMM must handle this case.
- **NPT and GPA FEE0_0000h:** the NPT must not map it to the physical local APIC page.
  - With an identity map, guest MMIO would reach the host's APIC page.
  - The host APIC is in x2APIC mode, where MMIO is "disabled" with undefined results (p.657).

### Q12. HLT/idle, interrupt windows, evaluation points, NMI/SMI/INIT

**Rules**
- **When pending virtual interrupts are evaluated:**
  - At VMRUN: "the interrupt state is evaluated and the highest priority pending interrupt indicated in the IRR is delivered if interrupt masking and priority allow" (p.579).
  - When a doorbell is received in guest mode (p.579).
  - For doorbells received during VMRUN processing: "recognized immediately after entering the guest" (p.579).
  - On a guest EOI, hardware "re-evaluates the interrupt state" (p.569).
  - TPR or ISR changes update PPR, which "is used to control the delivery" (p.569).
  - **A software IRR write is not on this list.**
- **HLT:**
  - HLT exits as VMEXIT_HLT (78h).
  - The Idle HLT intercept is VMCB 014h bit 6: "Intercept HLT instruction if a virtual interrupt is not pending" (p.739/PDF 801).
  - Table 15-7 (p.515/PDF 577): "This intercept occurs only if a virtual interrupt is not pending (V_INTR or V_NMI)." and "When both HLT and Idle HLT intercepts are active at the same time, the HLT intercept takes priority."
  - Table C-1 lists A6h VMEXIT_IDLE_HLT (p.757). Support is CPUID Fn8000_000A_EDX[30] (V3 p.654).
  - 15.29 says nothing about HLT.
- **Interrupt windows:**
  - The VINTR intercept (15.21.6, p.535) lets the VMM "gain control at the moment interrupts become enabled in the guest (i.e., just before the guest takes a virtual interrupt)".
  - The manual says nothing about VINTR under AVIC. V_IRQ is ignored under AVIC (p.570, p.579).
- **EVENTINJ (p.531):** the event is injected "unconditionally before executing the first guest instruction".
- **Virtual NMI (p.536-537, PDF 598-599):**
  - V_NMI_ENABLE (bit 26) requires the NMI intercept; otherwise VMRUN fails with VMEXIT_INVALID. V_NMI is bit 11 and V_NMI_MASK is bit 12.
  - "The processor takes a virtual NMI if: virtual NMIs are not masked, interrupts are enabled with GIF, virtual interrupts are enabled with VGIF, and the processor is not in an interrupt shadow."
  - No interaction with AVIC is stated.
- **Guest-originated NMI, SMI, INIT and SIPI IPIs:** not accelerated; they cause a #VMEXIT (p.569, p.573).
- **Device interrupts:**
  - p.564: "Acceleration of the delivery of virtual interrupts from I/O devices to virtual processors is not addressed directly by AVIC hardware. This acceleration would be provided by an IOMMU."
  - In guest-APIC mode the IOMMU handles only "upstream fixed and arbitrated interrupts" (IOMMU p.93).
- **Physical INIT and NMI:**
  - Table 15-12 (p.536): with GIF = 0 the INIT is held pending. With GIF = 1 and the INIT intercept set: "#VMEXIT(INIT), INIT is still pending."
  - p.523: an intercepted INIT "remains pending until the VMM sets GIF ..., at which point it either takes effect or is redirected".
  - Table 15-13 (p.536): an intercepted NMI gives "#VMEXIT(NMI), NMI is still pending."
- **INIT state on a real APIC (16.5, p.643):** "In the INIT state, the target APIC is responsive only to the STARTUP IPI. All other interrupts (including SMI and NMI) are held pending until the STARTUP IPI has been accepted."
- **INIT and APIC registers (16.10, p.657):** INIT leaves AE and EXTD unchanged. "All other APIC registers are initialized to their values as described in 'Reset in x2APIC mode' above."
  - No section with that name exists. Table 16-2 (p.631) gives the register values "after reset and INIT".

**Implementation implication**
- **Guest HLT without a HLT or Idle-HLT intercept:** waking depends on a doorbell or IRR evaluation. The manual does not describe whether a doorbell wakes a halted vCPU (U16).
- **Guest HLT with the HLT intercept:** before blocking, the VMM must itself compare backing-page IRR against PPR and the guest IF.
- **Planned guest-INIT backing-page reset:** the manual defines no hardware reset of the backing page. The VMM must:
  - reset the backing-page registers to the Table 16-2 values while keeping x2APIC mode (AE/EXTD unchanged);
  - keep V_TPR consistent with the backing page;
  - hold every other interrupt until SIPI is accepted.
- **Intercepted physical INIT:** it stays pending. The host must deal with it before setting GIF = 1, or it takes effect on the host.

### Q13. Appendix C, Table C-1

- **401h AVIC_INCOMPLETE_IPI:** "AVIC—Virtual IPI delivery not completed. See 'AVIC IPI Delivery Not Completed' on page 580 for EXITINFO1–2 definitions." (p.758/PDF 820/idx 819)
- **402h AVIC_NOACCEL:** "AVIC—Attempted access by guest to vAPIC register not handled by AVIC hardware. See 'AVIC Access to Un-accelerated vAPIC register' on page 581 for EXITINFO1–2 definitions." (p.758)
- **Related codes without "AVIC" in their names:**
  - p.756/PDF 818: 60h VMEXIT_INTR, 61h VMEXIT_NMI, 62h VMEXIT_SMI, 63h VMEXIT_INIT, 64h VMEXIT_VINTR, 78h VMEXIT_HLT.
  - p.757/PDF 819: 7Ch VMEXIT_MSR, A6h VMEXIT_IDLE_HLT.
  - p.758: 400h VMEXIT_NPF, 403h VMEXIT_VMGEXIT, -1 VMEXIT_INVALID, -2 VMEXIT_BUSY, -3 VMEXIT_IDLE_REQUIRED, -4 VMEXIT_INVALID_PMC.
- **Table C-1 contains no other AVIC exit codes.**
- 15.29.9 (p.579): "two new AVIC-related #VMEXIT events ... Assigned EXITCODE values are given in Table C-1 on page 756."
- **SEV-ES only:** Table 15-33, AE Exitcodes (p.596/PDF 658), does not list 401h or 402h, so under SEV-ES both would be non-automatic exits. That table's "HW Advances RIP" column applies only to SEV-ES; the manual gives no such column for non-SEV guests.

---

## 2. Cross-reference chains followed

1. 15.29.4.1 (p.570) → Table B-1 060h bits 30/31 (p.740-741) → "x2AVIC on page 582" → 15.29.10 (p.582-583) → "Section 15.29.3.1" = Table 15-22 (p.566-568). 15.29.10 also cites "Section 15.29.4.1, Section 15.29.4.3 and Table 15-28"; those are p.570, p.571 and p.582.
2. 15.29.4.1 V_INTR_MASKING "(bit 24)" → Table B-1 060h bit 24 (p.740) → "Virtualizing APIC.TPR on page 533" → 15.21.1/15.21.2 (p.533) → 15.21.4 (p.534) → 15.13.1 INTR intercept (p.522) → "Virtual Interrupt Intercept on page 535" (p.535).
3. 15.29.6.1 (p.576-577) → "Section 15.29.9.1 on page 580" → Tables 15-25, 15-26, 15-27 (p.580-581) → Table C-1 (p.756-758).
4. 15.29.9 (p.579) → "Table C-1 on page 756" (p.756-758). Table C-1 401h/402h → back to p.580/p.581.
5. 15.29.3.1 ICRL (p.569) → "Inter-processor Interrupts on page 564" (p.564) and "Chapter 16" → Figure 16-18 and Table 16-4 (p.642-644) → 16.6 (p.645-648) → 16.8-16.15 (p.654-663).
6. 15.29.5.2 x2AVIC_EXT (p.571) → V3 E.4.9 ECX (p.653). 15.29.7 (p.578) → "Section 3.3 on page 71" (V2 p.71/PDF 133, a generic pointer) → V3 E.4.9 EDX (p.654), including bit 27 ExtLvtAvicAccessChg → "Virtual APIC Register Accesses" = the last rows of Table 15-22 (p.568).
7. 15.29.8.2 Figure 15-22 (p.579) → Appendix A Tables A-1 and A-7 (p.724, p.733) → V3 WRMSR (p.511).
8. 15.29.6.2 device interrupts (p.577) and 15.29.1 IOMMU mention (p.564) → IOMMU spec 2.2.5.x (p.93-94) and 2.7 (p.186-188) → back to "Chapter 15 of ... #24593".
9. VMRUN "clean" fields (p.503) → 15.15 (p.526-527), Figure 15-4 bits 11 and 3.
10. Trap/fault definitions (p.566) → 15.7 and 15.7.1 (p.508-509) for trap state and nRIP.
11. 15.29.10 precedence (p.583) → 15.11 (p.518-519) → 16.11 (p.657-659), 16.12 (p.660), 16.15 (p.662).
12. VMRUN checks: 15.5.1 (p.503) → "Canonicalization and Consistency Checks on page 504" (p.504-505); 15.25.3 (p.550/PDF 612) NPT enable; 15.36.12 (p.611-612) Table 15-39; 15.36.16 (p.614-615).
13. INIT/NMI/GIF: 15.13.4 (p.523) → 15.17 Table 15-10 (p.530) → 15.21.8 Table 15-12 and 15.21.9 Table 15-13 (p.535-536) → 15.21.10 vNMI (p.536-537).
14. 0B8h users: Table B-1 0B8h (p.742) → 15.38 IBS (p.625) and 15.39 PMC (p.626).
15. Idle HLT: Table B-1 014h bit 6 (p.739) → Table 15-7 (p.515) → Table C-1 A6h (p.757) → V3 EDX[30] (p.654).
16. Backing-page reset values: 16.10 (p.657) → "section 16.3.2 APIC Registers" = Table 16-2 (p.631). Also "Reset in x2APIC mode above", which does not resolve.
17. SEV-ES exit classification: Table 15-33 (p.596), used only to confirm 401h/402h are absent.

## 3. UNRESOLVED (the manual does not settle these; do not extrapolate)

- **U1.** VMRUN behavior when AVIC = 1 and NP_ENABLE = 0. The manual only says the guest "must also enable nested paging" (p.570).
- **U2.** The exit code for "pointers ... outside of the legal range" (p.571 says only "#VMEXIT").
- **U3.** Whether 0 is accepted for AVIC_LOGICAL_TABLE (0F0h) and V_APIC_BAR (098h) in x2AVIC mode. The manual only says both are "not used".
- **U4.** How to doorbell a host with physical APIC ID > 255. The C001_011Bh value field is 8 bits with bits 63:8 must-be-zero, while the table entry's host-ID field is 12 bits. What happens if bits above 7 are set is not stated.
- **U5.** What a doorbell does when the target core is in host mode (IR = 1 but inside the #VMEXIT handler): ignored, kept pending, or delivered as a host interrupt, and with which vector.
- **U6.** Whether a running guest notices a software IRR write without a doorbell. The manual lists only VMRUN, doorbell, and EOI/TPR/PPR changes as evaluation points.
- **U7.** x2AVIC broadcast handling:
  - whether hardware treats destination FFFF_FFFFh as broadcast;
  - whether FFh is still special;
  - whether physical-table entry 255 is reserved in x2AVIC mode;
  - how DSH = 11b excludes the sender;
  - whether the "target x2APIC ID" in the logical-ID formula is the physical-table index.
- **U8.** The exit code and ID for non-Fixed message types (INIT, SIPI, NMI, SMI, lowest priority, ExtINT). Table 15-27 ID 0's text mentions only level trigger and "destination type".
- **U9.** Which index is reported for ID 1 when several targets are not running, or in x2AVIC logical mode. For ID 3 on a broadcast, whether other targets' IRR bits were already set.
- **U10.** The nRIP value on 401h/402h (the general rule on p.509 gives 0; there is no AVIC-specific statement). The saved RIP for trap and fault exits (inferred from 15.7.1; 15.29 never states it). Verify on hardware.
- **U11.** Level-triggered EOI exit: Table 15-22 says "trap" while 15.29.9.2 says "fault". Whether the ISR bit is already cleared at the exit.
- **U12.** Which "access checks" x2AVIC hardware performs before the AVIC permission checks (15.29.10). Specific conflicts:
  - a write to 802h (table: trap; x2APIC: #GP);
  - a write to 80Dh (table: trap; x2APIC: read-only);
  - writes to the read-only registers 803h, 809h, 80Ah, 810h-827h and 839h;
  - reading EOI (x2APIC: write-only) and reading 83Fh (x2APIC: #GP; no table entry);
  - reserved-bit #GP, and non-zero EOI or ESR writes;
  - unimplemented MSRs in 800h-83Fh (800h, 801h, 804h-807h, 80Ch, 80Eh, 831h, 83Ah-83Dh) versus "any other register locations ... allowed to read and write the backing page".
- **U13.** Whether hardware consults any guest APIC mode (AE/EXTD) under x2AVIC. What happens if the guest disables its APIC while bit 30 stays set. How guest MMIO access to the APIC GPA behaves under x2AVIC.
- **U14.** Whether VMRUN reconciles VMCB V_TPR with the backing-page TPR (and which wins if they differ). Whether AVIC CR8 writes also update the physical TPR when V_INTR_MASKING = 0.
- **U15.** Whether the INTR intercept still triggers when the host runs VMRUN with RFLAGS.IF = 0 and V_INTR_MASKING = 1. p.533 says the saved host IF "controls physical interrupts while the guest is running".
- **U16.** Whether Idle-HLT's "virtual interrupt ... pending (V_INTR or V_NMI)" includes AVIC IRR state. Whether a doorbell wakes a halted guest. VINTR intercept behavior under AVIC. Re-evaluation when the guest's IF goes from 0 to 1.
- **U17.** Any rule for VMM software writes to IRR, TMR, ISR or PPR (atomicity width, ordering of TMR and IRR). Only hardware and IOMMU atomicity is described (p.577; IOMMU p.94).
- **U18.** How EXITINFO1 APIC_Offset maps to an x2APIC MSR for 402h. Inferred as (MSR - 800h) << 4.
- **U19.** Guest INIT under AVIC. No hardware backing-page reset is described. The Chapter 16 INIT-reset reference is dangling ("Reset in x2APIC mode").
- **U20.** Whether the "up to eight consecutive 4-Kbyte pages" with x2AVIC_EXT = 1 must be physically contiguous (the manual says "consecutive" without qualification).
- **U21.** The maximum index and VMRUN check when x2AVIC_EXT = 1 (field width implies 4095; no check is stated).
- **U22.** Whether clean bit 3 ("Offset 60h-67h") covers the AVIC/x2AVIC enable bits (060h bits 31:30). They are not named.
- **U23.** VMCB 060h bit 10 is not documented in Table B-1.

## 4. Manual inconsistencies and stale references noticed (all read from the page images)

- 15.29.9.2 (p.581) calls the level-EOI exit "This fault", while Table 15-22 (p.566) calls it a trap.
- 15.29.10 (p.583) says new x2AVIC error conditions are in "Table 15-28", but Table 15-28 is the AVIC_NOACCEL EXITINFO1 layout.
- 15.29.3.1 (p.568) contains the garbled sentence "Only the Task Priority bits of are maintained ...".
- 15.29.10 writes "Fn8000000A_EDX[X2AVIC] (bit 18)" (non-standard spelling).
- 16.13 (p.661) cites "Fig 16-18 on page 582", but Figure 16-18 is on p.642. Figure 16-34 has a "55:20 Reserved" row, which evidently should be 31:20.
- 16.10 (p.657) refers to "Reset in x2APIC mode above", and no such section exists. 16.11.1 (p.659) cites "Table 16-2" for the x2APIC MSR list, but that list is Table 16-6.
- 16.9.1 (p.656) puts the 8-bit APIC_ID at "MMIO offset 30h", while Table 16-2 has it at 20h.
- Figure 15-4 text (p.527) says "Bits 31:12 are reserved" even though bit 12 (CET) is defined.
- V3 E.4.9 (p.653): the ECX subsection intro says "The value returned in EDX".
- Table B-1 060h has no row for bit 10.

---

## Appendix T. Page-by-page transcription notes

Text in double quotes is verbatim from the rendered image. Everything else is paraphrase. Every page listed
here was viewed as an image at least twice; the second pass checked these notes against the image.

### V2 PDF 625 / idx 624 / p.563
- End of 15.28: "INIT is gated by the Global Interrupt Flag (GIF), and so will be held pending if asserted while GIF is 0."
- Start of 15.29 and 15.29.1 Introduction (overview only).

### V2 PDF 626 / idx 625 / p.564
- "Inter-processor Interrupts." overview paragraph. This is the target of the cross-reference on p.569.
- "Device Interrupts. Acceleration of the delivery of virtual interrupts from I/O devices to virtual processors is not addressed directly by AVIC hardware. This acceleration would be provided by an IOMMU."
- 15.29.2: "This image is backed by a page in the system physical address (SPA) space called a vAPIC backing page. The backing page remains pinned in system memory as long as the virtual machine persists, even when the specific virtual processor associated with the backing page is not running."
- The VMM reads the backing page and writes status to it. Guest reads are mostly allowed directly; most writes are intercepted.

### V2 PDF 627 / idx 626 / p.565
- 15.29.3 AVIC Backing Page. Figure 15-15 shows allow, trap and fault paths through a "Register-level Permissions Filter". Footnote: "*Writes to specific registers can initiate AVIC hardware actions".
- "System software is responsible for setting up a translation in the nested page table granting guest read and write permissions for accesses to the vAPIC Backing Page in SPA space. AVIC hardware walks the nested page table to check permissions, but does not use the SPA address specified in the leaf page table entry. Instead, AVIC hardware finds this address in the AVIC_BACKING_PAGE pointer field of the VMCB."

### V2 PDF 628 / idx 627 / p.566
- "The VMM initializes the backing page with appropriate default APIC register values including items such as APIC version number."
- V_APIC_BAR relocation: the VMM "should intercept the write to update the V_APIC_BAR field of the VMCB and the GPA part of translation in the host's nested page tables."
- "The vAPIC backing page must be present in system physical memory for the life of the guest VM because some fields are updated even when the guest is not running."
- 15.29.3.1 definitions of Allow, Fault and Trap (quoted in Q3). Table 15-22, part 1 (rows 20h-B0h).

### V2 PDF 629 / idx 628 / p.567
- Table 15-22, continued: rows C0h-3E0h and 83Fh (see Q3).

### V2 PDF 630 / idx 629 / p.568
- Table 15-22, continued: rows 400h-FFFh.
- "Accesses to any other register locations not explicitly defined in this table are allowed to read and write the backing page."
- The 32-bit width / 16-byte alignment note, the ICRH note, the PPR note, and the start of the TPR text (quoted in Q3 and Q8).

### V2 PDF 631 / idx 630 / p.569
- Remainder of the TPR text and Figure 15-16.
- PPR, EOI and ICRL paragraphs (quoted in Q3, Q4, Q6 and Q8).
- 15.29.4 heading.

### V2 PDF 632 / idx 631 / p.570
- 15.29.4.1 AVIC Enable (bit 31) and x2AVIC Mode Enable (bit 30). 15.29.4.2 V_APIC_BAR (098h), backing-page pointer (0E0h, "52-bit HPA"), logical table (0F0h), physical table (0F8h), and MAX_INDEX.

### V2 PDF 633 / idx 632 / p.571
- 15.29.4.3 pointer restrictions and the MAX_INDEX VMRUN checks.
- The xAVIC multiprocessor TLB_CONTROL=3h requirement.
- 15.29.5 structures "defined to fit exactly in one 4-Kbyte page. Future implementations may expand the size."
- 15.29.5.1: "Each virtual processor in the system is assigned a virtual APIC backing page (vAPIC backing page)."
- 15.29.5.2 physical table (quoted in Q2).

### V2 PDF 634 / idx 633 / p.572
- The table pointer is the same for every vCPU in the VM.
- Figure 15-17 / Table 15-23 entry format, and the IR note (quoted in Q2).

### V2 PDF 635 / idx 634 / p.573
- xAVIC layout (Figure 15-18): entries 0-254 at bytes 0-2032; entry 255 reserved at byte 2040; bytes 2048-4095 reserved.
- "Since a destination of FFh is used to specify a broadcast, physical APIC ID FFh is reserved. The upper 2048 bytes of the table are reserved and should be set to zero."
- 15.29.5.3: "In addition to the Physical APIC ID Table, each guest VM is assigned a Logical APIC ID Table. This table is used to lookup the guest physical APIC ID for logically addressed interrupt requests. ... Note that this implies that the logical ID of each vAPIC must be unique."
- "If the guest attempts to change the logical ID of its APIC, the VMM must reflect this change in the Logical APIC ID Table." Then the fixed/self/broadcast support sentence (quoted in Q4).

### V2 PDF 636 / idx 635 / p.574
- Figure 15-19 / Table 15-24 logical entry format: bit 31 V; bits 30:8 reserved SBZ; bits 7:0 guest physical APIC ID.
- Flat mode uses the first 8 entries, at offset 4*log2(l_apic_id).
- The x2AVIC logical-ID formula (quoted in Q4).

### V2 PDF 637 / idx 636 / p.575
- Figure 15-20 (flat mode).
- Cluster mode: bits 7:4 are the cluster (Fh reserved); bits 3:0 are a bit-encoded index. Offset = (16*c) + 4*log2(apic_ix).

### V2 PDF 638-639 / idx 637-638 / p.576-577
- Figure 15-21. 15.29.6 and 15.29.6.1, steps 1-6 (quoted in Q4).
- 15.29.6.2 device interrupts (IOMMU):
  - an invalid entry "aborts the virtual interrupt delivery and logs an error";
  - the IOMMU "atomically sets the bit in the IRR in the vAPIC backing page";
  - for a non-running target, "the virtual interrupt will be presented when the virtual processor is made active again."

### V2 PDF 640 / idx 639 / p.578
- Device-interrupt doorbell text.
- 15.29.7 CPUID: EDX bit 13 is AVIC; EDX bit 18 is x2AVIC.
- 15.29.8.1 filter description (quoted in Q4).
- Start of 15.29.8.2.

### V2 PDF 641 / idx 640 / p.579
- Doorbell text, Figure 15-22, "Processing of Doorbell Signals", 15.29.8.3 and 15.29.9 (quoted in Q7 and Q12).

### V2 PDF 642-643 / idx 641-642 / p.580-581
- 15.29.9.1: Figures 15-23 and 15-24, Tables 15-25, 15-26 and 15-27.
- 15.29.9.2 opening text (quoted in Q5 and Q6).

### V2 PDF 644 / idx 643 / p.582
- Figures 15-25 and 15-26, Tables 15-28 and 15-29.
- 15.29.10: "x2AVIC support is reported by Fn8000000A_EDX[X2AVIC] (bit 18) = 1."

### V2 PDF 645 / idx 644 / p.583
- 15.29.10 body (quoted in Q3, Q9 and Q11).
- "New x2AVIC mode error conditions are documented in Section 15.29.4.1, Section 15.29.4.3 and Table 15-28."
- 15.30 starts.

### Appendix B, Table B-1 (V2 PDF 801-804 / idx 800-803 / p.739-742)
- **p.739:**
  - 014h bit 6: "Intercept HLT instruction if a virtual interrupt is not pending."
  - 040h IOPM_BASE_PA; 048h MSRPM_BASE_PA.
- **p.740:**
  - 058h: bits 31:0 guest ASID; bits 39:32 TLB_CONTROL (00h, 01h, 03h, 07h); bit 40 ALLOW_LARGER_RAP; bit 41 CLEAR_RAP.
  - 060h: V_TPR, V_IRQ, VGIF value, V_NMI, V_NMI_MASK, V_INTR_PRIO, V_IGN_TPR, V_INTR_MASKING, VGIF enable (bit 25), V_NMI_ENABLE (bit 26), x2AVIC Enable (bit 30). No row for bit 10.
- **p.741:**
  - 060h (continued): bit 31 AVIC Enable; bits 39:32 V_INTR_VECTOR; bits 63:40 SBZ.
  - 068h interrupt shadow; 070h-088h exit fields.
  - 090h bits 0-7, as listed in Q1.
  - 098h AVIC APIC_BAR.
- **p.742:**
  - 0B8h bits 0-3; 0C0h clean bits; 0C8h nRIP; 0E0h backing-page pointer; 0E8h-0EFh SBZ; 0F0h logical table; 0F8h physical table + MAX_INDEX.
  - Also 108h VMSA, 120h, 134h UPDATE_IRR, 138h, 140h and 150h REQUESTED_IRR (SEV-SNP / Secure AVIC fields, not used here).

### Appendix C, Table C-1 (V2 PDF 818-820 / idx 817-819 / p.756-758)
- See Q13.
- p.756: "Intercept exit codes are equal to the bit position of the corresponding flag in the VMCB's intercept vector."

### Appendix A (V2 PDF 786 / idx 785 / p.724; PDF 795 / idx 794 / p.733)
- See Q7. Table A-1 also lists C001_0137h IDLE_WAKEUP_ICR and C001_0138h SECURE_AVIC_CTRL.

### SVM chapter (V2)
- **p.503 (PDF 565):** VMRUN loads V_TPR and V_IRQ, then sets GIF = 1. V_INTR_MASKING is listed as a control bit.
- **p.504-505 (PDF 566-567):** the generic VMEXIT_INVALID list (no AVIC item). "the final merged hardware state is used for consistency checks."
- **p.506 (PDF 568):** host and guest EFLAGS are isolated from each other.
- **p.507 (PDF 569):** list of #VMEXIT actions.
- **p.508-509 (PDF 570-571):** 15.7 ("When an external or virtual interrupt is intercepted, the interrupt is left pending."), trap semantics, and the nRIP rule.
- **p.515 (PDF 577):** the Idle HLT row of Table 15-7.
- **p.518-519 (PDF 580-581):** 15.11 MSRPM, check order, and EXITINFO1.
- **p.522-523 (PDF 584-585):** 15.13 and 15.13.1 INTR; 15.13.4 INIT intercept.
- **p.526-527 (PDF 588-589):** 15.15 clean bits (see Q1).
- **p.530 (PDF 592):** Table 15-10 GIF (doorbell is not listed).
- **p.531-532 (PDF 593-594):** event injection.
- **p.533-537 (PDF 595-599):** 15.21.1-15.21.10 (quoted in Q8 and Q12).
- **p.550 (PDF 612):** 15.25.3: "If VMRUN is executed with hCR0.PG cleared to zero and NP_ENABLE set to 1, VMRUN terminates with #VMEXIT(VMEXIT_INVALID)." Not AVIC-specific.
- **p.596 (PDF 658):** Table 15-33 AE exit codes (SEV-ES); 401h and 402h are absent.
- **p.611-615 (PDF 673-677):** SNP VMRUN checks and injection modes (SNP only).
- **p.625-626 (PDF 687-688):** IBS and PMC virtualization and their AVIC dependency.

### APIC chapter (V2)
- **p.630 (PDF 692):** APIC_BASE Figure 16-2. "The reset value of the APIC base address is 0_0000_FEE0_0000h. This address is not affected by INIT."
- **p.631 (PDF 693):** Table 16-2 reset values "after reset and INIT":
  - 20h ??000000h; 30h 80??0010h; F0h 000000FFh; E0h FFFFFFFFh;
  - 320h-370h 00010000h (masked); 400h 00040007h; IER FFFFFFFFh;
  - all other listed registers 0.
- **p.642-644 (PDF 704-706):** ICR Figure 16-18, message-type and DSH definitions, Table 16-4.
- **p.645-648 (PDF 707-710):** 16.6 destination matching; 16.6.3 IRR/ISR/TMR semantics.
- **p.654-663 (PDF 716-725):** x2APIC sections 16.8-16.15 (quoted in Q3, Q4, Q9, Q11 and Q12).

### V3 and IOMMU
- **V3 p.653 (PDF 687):** Fn8000_000A_ECX bit 6 x2AVIC_EXT ("4096 vCPUs supported in x2AVIC mode").
- **V3 p.654 (PDF 688):** Fn8000_000A_EDX bits:
  - 5 VmcbClean, 6 FlushByAsid, 8 PmcVirt, 13 AVIC, 16 VGIF, 18 x2AVIC,
  - 24 TlbiCtl, 25 VNMI, 26 IbsVirt, 27 ExtLvtAvicAccessChg, 30 IdleHltIntercept.
- **V3 p.511 (PDF 546):** WRMSR (see Q7).
- **IOMMU p.93-94 and p.186-188:** see Q7 and Q10.
