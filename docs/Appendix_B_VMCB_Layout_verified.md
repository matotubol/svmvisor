# Appendix B - VMCB Layout

**Source:** AMD64 Architecture Programmer’s Manual, Volume 2: System Programming, Publication 24593, Revision 3.44, March 2026.

**Coverage:** physical PDF pages **799-814**, corresponding to printed pages **737-752**. This includes Tables B-1 through B-4 and stops before Section B.1, “Guest MSR Intercepts,” on printed page 753.

## Transcription and verification notes

- Every one of the 16 source pages was rendered and visually reviewed.
- Table cells were extracted with pdfplumber and independently checked against PyMuPDF text. **1,194 of 1,195 non-empty extracted cells matched automatically** after whitespace and typography normalization. The one remaining cell was the vertically merged Guest Intercept Control notes block on physical page 814; it was resolved directly from the rendered page.
- Merged offset/register cells are expanded by repeating their effective value, so each Markdown row is self-contained.
- In Table B-2, the source heading **Contents** spans two visual columns. They are represented below as **Content** and **Subfield**; this is only a Markdown representation of the source’s merged heading.
- Values are source-faithful. Apparent source-document inconsistencies are **not silently corrected**; they are listed after the tables.

## Table B-1. VMCB Layout, Control Area

Physical PDF pages 799-804; printed pages 737-742. The control area begins at offset 0 of the VMCB and is padded to 1024 bytes.

| Byte Offset | Bit(s) | Function |
| --- | --- | --- |
| <code>000h (vector 0)</code> | <code>15:0</code> | Intercept reads of CR0–15, respectively |
| <code>000h (vector 0)</code> | <code>31:16</code> | Intercept writes of CR0–15, respectively |
| <code>004h (vector 1)</code> | <code>15:0</code> | Intercept reads of DR0–15, respectively |
| <code>004h (vector 1)</code> | <code>31:16</code> | Intercept writes of DR0–15, respectively. |
| <code>008h (vector 2)</code> | <code>31:0</code> | Intercept exception vectors 0–31, respectively |
| <code>00Ch (vector 3)</code> | <code>0</code> | Intercept INTR (physical maskable interrupt) |
| <code>00Ch (vector 3)</code> | <code>1</code> | Intercept NMI |
| <code>00Ch (vector 3)</code> | <code>2</code> | Intercept SMI |
| <code>00Ch (vector 3)</code> | <code>3</code> | Intercept INIT |
| <code>00Ch (vector 3)</code> | <code>4</code> | Intercept VINTR (virtual maskable interrupt) |
| <code>00Ch (vector 3)</code> | <code>5</code> | Intercept CR0 writes that change bits other than CR0.TS or<br>CR0.MP |
| <code>00Ch (vector 3)</code> | <code>6</code> | Intercept reads of IDTR |
| <code>00Ch (vector 3)</code> | <code>7</code> | Intercept reads of GDTR |
| <code>00Ch (vector 3)</code> | <code>8</code> | Intercept reads of LDTR |
| <code>00Ch (vector 3)</code> | <code>9</code> | Intercept reads of TR |
| <code>00Ch (vector 3)</code> | <code>10</code> | Intercept writes of IDTR |
| <code>00Ch (vector 3)</code> | <code>11</code> | Intercept writes of GDTR |
| <code>00Ch (vector 3)</code> | <code>12</code> | Intercept writes of LDTR |
| <code>00Ch (vector 3)</code> | <code>13</code> | Intercept writes of TR |
| <code>00Ch (vector 3)</code> | <code>14</code> | Intercept RDTSC instruction |
| <code>00Ch (vector 3)</code> | <code>15</code> | Intercept RDPMC instruction |
| <code>00Ch (vector 3)</code> | <code>16</code> | Intercept PUSHF instruction |
| <code>00Ch (vector 3)</code> | <code>17</code> | Intercept POPF instruction |
| <code>00Ch (vector 3)</code> | <code>18</code> | Intercept CPUID instruction |
| <code>00Ch (vector 3)</code> | <code>19</code> | Intercept RSM instruction |
| <code>00Ch (vector 3)</code> | <code>20</code> | Intercept IRET instruction |
| <code>00Ch (vector 3)</code> | <code>21</code> | Intercept INTn instruction |
| <code>00Ch (vector 3)</code> | <code>22</code> | Intercept INVD instruction |
| <code>00Ch (vector 3)</code> | <code>23</code> | Intercept PAUSE instruction |
| <code>00Ch (vector 3)</code> | <code>24</code> | Intercept HLT instruction |
| <code>00Ch (vector 3)</code> | <code>25</code> | Intercept INVLPG instruction |
| <code>00Ch (vector 3)</code> | <code>26</code> | Intercept INVLPGA instruction |
| <code>00Ch (vector 3)</code> | <code>27</code> | IOIO_PROT—Intercept IN/OUT accesses to selected ports |
| <code>00Ch (vector 3)</code> | <code>28</code> | MSR_PROT—intercept RDMSR or WRMSR accesses to<br>selected MSRs |
| <code>00Ch (vector 3)</code> | <code>29</code> | Intercept task switches |
| <code>00Ch (vector 3)</code> | <code>30</code> | FERR_FREEZE: intercept processor “freezing” during<br>legacy FERR handling |
| <code>00Ch (vector 3)</code> | <code>31</code> | Intercept shutdown events |
| <code>010h (vector 4)</code> | <code>0</code> | Intercept VMRUN instruction |
| <code>010h (vector 4)</code> | <code>1</code> | Intercept VMMCALL instruction |
| <code>010h (vector 4)</code> | <code>2</code> | Intercept VMLOAD instruction |
| <code>010h (vector 4)</code> | <code>3</code> | Intercept VMSAVE instruction |
| <code>010h (vector 4)</code> | <code>4</code> | Intercept STGI instruction |
| <code>010h (vector 4)</code> | <code>5</code> | Intercept CLGI instruction |
| <code>010h (vector 4)</code> | <code>6</code> | Intercept SKINIT instruction |
| <code>010h (vector 4)</code> | <code>7</code> | Intercept RDTSCP instruction |
| <code>010h (vector 4)</code> | <code>8</code> | Intercept ICEBP instruction |
| <code>010h (vector 4)</code> | <code>9</code> | Intercept WBINVD and WBNOINVD instructions |
| <code>010h (vector 4)</code> | <code>10</code> | Intercept MONITOR/MONITORX instruction |
| <code>010h (vector 4)</code> | <code>11</code> | Intercept MWAIT/MWAITX instruction unconditionally |
| <code>010h (vector 4)</code> | <code>12</code> | Intercept MWAIT/MWAITX instruction if monitor hardware<br>is armed |
| <code>010h (vector 4)</code> | <code>13</code> | Intercept XSETBV instruction |
| <code>010h (vector 4)</code> | <code>14</code> | Intercept RDPRU instruction |
| <code>010h (vector 4)</code> | <code>15</code> | Intercept writes of EFER (occurs after guest instruction<br>finishes) |
| <code>010h (vector 4)</code> | <code>31:16</code> | Intercept writes of CR0-15 (occurs after guest instruction<br>finishes) |
| <code>014h (vector 5)</code> | <code>0</code> | Intercept all INVLPGB instructions |
| <code>014h (vector 5)</code> | <code>1</code> | Intercept only illegally specified INVLPGB instructions |
| <code>014h (vector 5)</code> | <code>2</code> | Intercept INVPCID instruction |
| <code>014h (vector 5)</code> | <code>3</code> | Intercept MCOMMIT instruction |
| <code>014h (vector 5)</code> | <code>4</code> | Intercept TLBSYNC instruction. Presence of this bit is<br>indicated by CPUID Fn8000_000A, EDX[24] = 1. |
| <code>014h (vector 5)</code> | <code>5</code> | Intercept bus lock operations when Bus Lock Threshold<br>Counter is 0 (occurs before guest instruction executes). |
| <code>014h (vector 5)</code> | <code>6</code> | Intercept HLT instruction if a virtual interrupt is not pending. |
| <code>014h (vector 5)</code> | <code>31:7</code> | RESERVED, SBZ |
| <code>018h–03Bh</code> | <code>RESERVED, SBZ</code> |  |
| <code>03Ch</code> | <code>15:0</code> | PAUSE Filter Threshold |
| <code>03Eh</code> | <code>15:0</code> | PAUSE Filter Count |
| <code>040h</code> | <code>63:0</code> | IOPM_BASE_PA—Physical base address of IOPM (bits 11:0<br>are ignored) |
| <code>048h</code> | <code>63:0</code> | MSRPM_BASE_PA—Physical base address of MSRPM<br>(bits 11:0 are ignored) |
| <code>050h</code> | <code>63:0</code> | TSC_OFFSET—To be added in RDTSC and RDTSCP |
| <code>058h</code> | <code>31:0</code> | Guest ASID |
| <code>058h</code> | <code>39:32</code> | TLB_CONTROL<br>00h—Do nothing<br>01h—Flush entire TLB (all entries, all ASIDs) on VMRUN<br>Should only be used by legacy hypervisors<br>03h—Flush this guest’s TLB entries<br>07h—Flush this guest’s non-global TLB entries<br>NOTE: All other encodings are reserved. |
| <code>058h</code> | <code>40</code> | ALLOW_LARGER_RAP<br>0—RAP size for the guest is 32<br>1—RAP size for the guest is equal to CPUID<br>Fn8000_0021_EBX[RapSize] |
| <code>058h</code> | <code>41</code> | CLEAR_RAP<br>1—Clear RAP on VMRUN |
| <code>058h</code> | <code>63:42</code> | RESERVED, SBZ |
| <code>060h</code> | <code>7:0</code> | V_TPR—The virtual TPR for the guest. Bits 3:0 are used for<br>a 4-bit virtual TPR value; bits 7:4 are SBZ.<br>NOTE: This value is written back to the VMCB at #VMEXIT. |
| <code>060h</code> | <code>8</code> | V_IRQ—If nonzero, virtual INTR is pending<br>NOTE: This value is written back to the VMCB at #VMEXIT.<br>This field is ignored on VMRUN when AVIC is enabled. |
| <code>060h</code> | <code>9</code> | VGIF value (0 – Virtual interrupts are masked, 1 – Virtual<br>Interrupts are unmasked) |
| <code>060h</code> | <code>11</code> | V_NMI - If nonzero, virtual NMI is pending |
| <code>060h</code> | <code>12</code> | V_NMI_MASK - if nonzero, virtual NMI is masked |
| <code>060h</code> | <code>15:13</code> | RESERVED, SBZ |
| <code>060h</code> | <code>19:16</code> | V_INTR_PRIO—Priority for virtual interrupt<br>NOTE: This field is ignored on VMRUN when AVIC is enabled.. |
| <code>060h</code> | <code>20</code> | V_IGN_TPR—If nonzero, the current virtual interrupt<br>ignores the (virtual) TPR<br>NOTE: This field is ignored on VMRUN when AVIC is enabled.. |
| <code>060h</code> | <code>23:21</code> | RESERVED, SBZ |
| <code>060h</code> | <code>24</code> | V_INTR_MASKING—Virtualize masking of INTR<br>interrupts ( “Virtualizing APIC.TPR” on page533) |
| <code>060h</code> | <code>25</code> | AMD Virtual GIF enabled for this guest (0 -Disabled,<br>1 -Enabled) |
| <code>060h</code> | <code>26</code> | V_NMI_ENABLE - NMI Virtualization Enable (see “NMI<br>Virtualization” on page536) |
| <code>060h</code> | <code>29:27</code> | Reserved, SBZ |
| <code>060h</code> | <code>30</code> | x2AVIC Enable (see “x2AVIC” on page582) |
| <code>060h</code> | <code>31</code> | AVIC Enable |
| <code>060h</code> | <code>39:32</code> | V_INTR_VECTOR—Vector to use for this interrupt<br>NOTE: This field is ignored on VMRUN when AVIC is enabled.. |
| <code>060h</code> | <code>63:40</code> | RESERVED, SBZ |
| <code>068h</code> | <code>0</code> | INTERRUPT_SHADOW - Guest is in an interrupt shadow |
| <code>068h</code> | <code>1</code> | GUEST_INTERRUPT_MASK - Value of the RFLAGS.IF<br>bit for SEV-ES guest<br>Note: This value is written back to the VMCB on #VMEXIT. |
| <code>068h</code> | <code>63:2</code> | RESERVED, SBZ |
| <code>070h</code> | <code>63:0</code> | EXITCODE |
| <code>078h</code> | <code>63:0</code> | EXITINFO1 |
| <code>080h</code> | <code>63:0</code> | EXITINFO2 |
| <code>088h</code> | <code>63:0</code> | EXITINTINFO |
| <code>090h</code> | <code>0</code> | NP_ENABLE—Enable nested paging. |
| <code>090h</code> | <code>1</code> | Enable Secure Encrypted Virtualization |
| <code>090h</code> | <code>2</code> | Enable Encrypted State for Secure Encrypted Virtualization |
| <code>090h</code> | <code>3</code> | Guest Mode Execute Trap |
| <code>090h</code> | <code>4</code> | SSSCheckEn - Enable supervisor shadow stack restrictions in<br>nested page tables. Support for this feature is indicated by<br>CPUID Fn8000_000A_EDX[19] (SSSCheck) |
| <code>090h</code> | <code>5</code> | Virtual Transparent Encryption. |
| <code>090h</code> | <code>6</code> | Enable Read Only Guest Page Tables. See “Nested Table<br>Walk” on page550 |
| <code>090h</code> | <code>7</code> | Enable INVLPGB/TLBSYNC.<br>0 - INVLPGB and TLBSYNC will result in #UD.<br>1 - INVLPGB and TLBSYNC can be executed in guest.<br>Presence of this bit is indicated by CPUID bit 8000_000A,<br>EDX[24] = 1. When in SEV-ES guest or this bit is not<br>present, INVLPGB/TLBSYNC is always enabled in guest if<br>supported by processor. |
| <code>090h</code> | <code>63:8</code> | RESERVED, SBZ |
| <code>098h</code> | <code>63:52</code> | RESERVED, SBZ |
| <code>098h</code> | <code>51:0</code> | AVIC APIC_BAR |
| <code>0A0h</code> | <code>63:0</code> | Guest physical address of GHCB |
| <code>0A8h</code> | <code>63:0</code> | EVENTINJ—Event injection ( “Event Injection” on<br>page531 for details) |
| <code>0B0h</code> | <code>63:0</code> | N_CR3—Nested page table CR3 to use for nested<br>paging |
| <code>0B8h</code> | <code>0</code> | LBR Virtualization Enable |
| <code>0B8h</code> | <code>1</code> | VMSAVE/VMLOAD Virtualization Enable |
| <code>0B8h</code> | <code>2</code> | IBS Virtualization Enable |
| <code>0B8h</code> | <code>3</code> | PMC Virtualization Enable |
| <code>0B8h</code> | <code>63:4</code> | RESERVED, SBZ |
| <code>0C0h</code> | <code>31:0</code> | VMCB Clean Bits. |
| <code>0C0h</code> | <code>63:32</code> | RESERVED, SBZ |
| <code>0C8h</code> | <code>63:0</code> | nRIP—Next sequential instruction pointer |
| <code>0D0h</code> | <code>7:0</code> | Number of bytes fetched |
| <code>0D0h</code> | <code>127:8</code> | Guest instruction bytes |
| <code>0E0h</code> | <code>63:52</code> | RESERVED, SBZ |
| <code>0E0h</code> | <code>51:0</code> | AVIC APIC_BACKING_PAGE Pointer |
| <code>0E8h–0EFh</code> | <code>RESERVED, SBZ</code> |  |
| <code>0F0h</code> | <code>63:52</code> | RESERVED, SBZ |
| <code>0F0h</code> | <code>51:12</code> | AVIC LOGICAL_TABLE Pointer |
| <code>0F0h</code> | <code>11:0</code> | Reserved, SBZ |
| <code>0F8h</code> | <code>63:52</code> | RESERVED, SBZ |
| <code>0F8h</code> | <code>51:12</code> | AVIC PHYSICAL_TABLE Pointer[51:12] |
| <code>0F8h</code> | <code>11:0</code> | AVIC_PHYSICAL_MAX_INDEX |
| <code>100h – 107h</code> | <code>RESERVED, SBZ</code> |  |
| <code>108h</code> | <code>63:52</code> | RESERVED, SBZ |
| <code>108h</code> | <code>51:12</code> | VMSA Pointer[51:12] |
| <code>108h</code> | <code>11:0</code> | RESERVED, SBZ |
| <code>110h</code> | <code>63:0</code> | VMGEXIT_RAX |
| <code>118h</code> | <code>7:0</code> | VMGEXIT_CPL |
| <code>120h</code> | <code>15:0</code> | Bus Lock Threshold Counter |
| <code>128h – 133h</code> | <code>RESERVED, SBZ</code> |  |
| <code>134h</code> | <code>0</code> | UPDATE_IRR |
| <code>138h</code> | <code>63</code> | ALLOWED_SEV_FEATURES_EN |
| <code>138h</code> | <code>62</code> | RESERVED, SBZ |
| <code>138h</code> | <code>61:0</code> | ALLOWED_SEV_FEATURES_MASK |
| <code>140h</code> | <code>61:0</code> | GUEST_SEV_FEATURES |
| <code>148h</code> | <code>RESERVED, SBZ</code> |  |
| <code>150h</code> | <code>255:0</code> | REQUESTED_IRR |
| <code>170h – 3DFh</code> | <code>RESERVED, SBZ</code> |  |
| <code>3E0h – 3FFh</code> | <code>Reserved for Host usage</code> |  |

## Table B-2. VMCB Layout, State Save Area

Physical PDF pages 805-808; printed pages 743-746. These offsets are **relative to the state-save area**, which starts at offset 400h in the VMCB when SEV-ES is not enabled.

| Offset | Size | Content | Subfield | Notes |
| --- | --- | --- | --- | --- |
| <code>000h</code> | word | <code>ES</code> | <code>selector</code> |  |
| <code>002h</code> | word | <code>ES</code> | <code>attrib</code> |  |
| <code>004h</code> | dword | <code>ES</code> | <code>limit</code> |  |
| <code>008h</code> | qword | <code>ES</code> | <code>base</code> | Only lower 32 bits are implemented |
| <code>010h</code> | word | <code>CS</code> | <code>selector</code> |  |
| <code>012h</code> | word | <code>CS</code> | <code>attrib</code> |  |
| <code>014h</code> | dword | <code>CS</code> | <code>limit</code> |  |
| <code>018h</code> | qword | <code>CS</code> | <code>base</code> | Only lower 32 bits are implemented |
| <code>020h</code> | word | <code>SS</code> | <code>selector</code> |  |
| <code>022h</code> | word | <code>SS</code> | <code>attrib</code> |  |
| <code>024h</code> | dword | <code>SS</code> | <code>limit</code> |  |
| <code>028h</code> | qword | <code>SS</code> | <code>base</code> | Only lower 32 bits are implemented |
| <code>030h</code> | word | <code>DS</code> | <code>selector</code> |  |
| <code>032h</code> | word | <code>DS</code> | <code>attrib</code> |  |
| <code>034h</code> | dword | <code>DS</code> | <code>limit</code> |  |
| <code>038h</code> | qword | <code>DS</code> | <code>base</code> | Only lower 32 bits are implemented |
| <code>040h</code> | word | <code>FS</code> | <code>selector</code> |  |
| <code>042h</code> | word | <code>FS</code> | <code>attrib</code> |  |
| <code>044h</code> | dword | <code>FS</code> | <code>limit</code> |  |
| <code>048h</code> | qword | <code>FS</code> | <code>base</code> |  |
| <code>050h</code> | word | <code>GS</code> | <code>selector</code> |  |
| <code>052h</code> | word | <code>GS</code> | <code>attrib</code> |  |
| <code>054h</code> | dword | <code>GS</code> | <code>limit</code> |  |
| <code>058h</code> | qword | <code>GS</code> | <code>base</code> |  |
| <code>060h</code> | word | <code>GDTR</code> | <code>selector</code> | RESERVED |
| <code>062h</code> | word | <code>GDTR</code> | <code>attrib</code> | RESERVED |
| <code>064h</code> | dword | <code>GDTR</code> | <code>limit</code> | Only lower 16 bits are implemented |
| <code>068h</code> | qword | <code>GDTR</code> | <code>base</code> |  |
| <code>070h</code> | word | <code>LDTR</code> | <code>selector</code> |  |
| <code>072h</code> | word | <code>LDTR</code> | <code>attrib</code> |  |
| <code>074h</code> | dword | <code>LDTR</code> | <code>limit</code> |  |
| <code>078h</code> | qword | <code>LDTR</code> | <code>base</code> |  |
| <code>080h</code> | word | <code>IDTR</code> | <code>selector</code> | RESERVED |
| <code>082h</code> | word | <code>IDTR</code> | <code>attrib</code> | RESERVED |
| <code>084h</code> | dword | <code>IDTR</code> | <code>limit</code> | Only lower 16 bits are implemented |
| <code>088h</code> | qword | <code>IDTR</code> | <code>base</code> |  |
| <code>090h</code> | word | <code>TR</code> | <code>selector</code> |  |
| <code>092h</code> | word | <code>TR</code> | <code>attrib</code> |  |
| <code>094h</code> | dword | <code>TR</code> | <code>limit</code> |  |
| <code>098h</code> | qword | <code>TR</code> | <code>base</code> |  |
| <code>0A0h–0CAh</code> |  | <code>RESERVED</code> |  |  |
| <code>0CBh</code> | byte | <code>CPL</code> |  | If the guest is real-mode then the CPL is forced<br>to 0; if the guest is virtual-mode then the CPL is<br>forced to 3 |
| <code>0CCh</code> | dword | <code>RESERVED</code> |  |  |
| <code>0D0h</code> | qword | <code>EFER</code> |  |  |
| <code>0D8h–0DFh</code> |  | <code>RESERVED</code> |  |  |
| <code>0E0h</code> | qword | <code>PERF_CTL0</code> |  |  |
| <code>0E8h</code> | qword | <code>PERF_CTR0</code> |  |  |
| <code>0F0h</code> | qword | <code>PERF_CTL1</code> |  |  |
| <code>0F8h</code> | qword | <code>PERF_CTR1</code> |  |  |
| <code>100h</code> | qword | <code>PERF_CTL2</code> |  |  |
| <code>108h</code> | qword | <code>PERF_CTR2</code> |  |  |
| <code>110h</code> | qword | <code>PERF_CTL3</code> |  |  |
| <code>118h</code> | qword | <code>PERF_CTR3</code> |  |  |
| <code>120h</code> | qword | <code>PERF_CTL4</code> |  |  |
| <code>128h</code> | qword | <code>PERF_CTR4</code> |  |  |
| <code>130h</code> | qword | <code>PERF_CTL5</code> |  |  |
| <code>138h</code> | qword | <code>PERF_CTR5</code> |  |  |
| <code>148h</code> | qword | <code>CR4</code> |  |  |
| <code>150h</code> | qword | <code>CR3</code> |  |  |
| <code>158h</code> | qword | <code>CR0</code> |  |  |
| <code>160h</code> | qword | <code>DR7</code> |  |  |
| <code>168h</code> | qword | <code>DR6</code> |  |  |
| <code>170h</code> | qword | <code>RFLAGS</code> |  |  |
| <code>178h</code> | qword | <code>RIP</code> |  |  |
| <code>180h–1BFh</code> |  | <code>RESERVED</code> |  |  |
| <code>1C0h</code> | qword | <code>INSTR_RETIRED_CTR</code> |  |  |
| <code>1C8h</code> | qword | <code>PERF_CTR_GLOBAL_STS</code> |  |  |
| <code>1D0h</code> | qword | <code>PERF_CTR_GLOBAL_CTL</code> |  |  |
| <code>1D4h–1D7h</code> |  | <code>RESERVED</code> |  |  |
| <code>1D8h</code> | qword | <code>RSP</code> |  |  |
| <code>1E0h</code> | qword | <code>S_CET</code> |  |  |
| <code>1E8h</code> | qword | <code>SSP</code> |  |  |
| <code>1F0h</code> | qword | <code>ISST_ADDR</code> |  |  |
| <code>1F8h</code> | qword | <code>RAX</code> |  |  |
| <code>200h</code> | qword | <code>STAR</code> |  |  |
| <code>208h</code> | qword | <code>LSTAR</code> |  |  |
| <code>210h</code> | qword | <code>CSTAR</code> |  |  |
| <code>218h</code> | qword | <code>SFMASK</code> |  |  |
| <code>220h</code> | qword | <code>KernelGsBase</code> |  |  |
| <code>228h</code> | qword | <code>SYSENTER_CS</code> |  |  |
| <code>230h</code> | qword | <code>SYSENTER_ESP</code> |  |  |
| <code>238h</code> | qword | <code>SYSENTER_EIP</code> |  |  |
| <code>240h</code> | qword | <code>CR2</code> |  |  |
| <code>248h–267h</code> |  | <code>RESERVED</code> |  |  |
| <code>268h</code> | qword | <code>G_PAT</code> |  | Guest PAT—only used if nested paging enabled |
| <code>270h</code> | qword | <code>DBGCTL</code> |  | Guest DebugCtl MSR—only used if hardware<br>acceleration of LBR virtualization is supported<br>and enabled by setting the<br>LBR_VIRTUALIZATION_ENABLE bit of the<br>VMCB control area. |
| <code>278h</code> | qword | <code>BR_FROM</code> |  | Guest LastBranchFromIP MSR—only used if<br>hardware acceleration of LBR virtualization is<br>supported and enabled. |
| <code>280h</code> | qword | <code>BR_TO</code> |  | Guest LastBranchToIP MSR—only used if<br>hardware acceleration of LBR virtualization is<br>supported and enabled. |
| <code>288h</code> | qword | <code>LASTEXCPFROM</code> |  | Guest LastIntFromIP MSR—Only used if<br>hardware acceleration of LBR virtualization is<br>supported and enabled. |
| <code>290h</code> | qword | <code>LASTEXCPTO</code> |  | Guest LastIntToIP MSR—Only used if<br>hardware acceleration of LBR virtualization is<br>supported and enabled. |
| <code>298h</code> | qword | <code>DBGEXTNCTL</code> |  | Guest DebugExtnCtl MSR—only used if<br>hardware acceleration of LBR Stack<br>virtualization is supported and enabled by<br>setting the<br>LBR_VIRTUALIZATION_ENABLE bit of the<br>VMCB control area. |
| <code>2A0h–2DFh</code> | 72 bytes | <code>RESERVED</code> |  |  |
| <code>2E0h</code> | qword | <code>SPEC_CTRL</code> |  |  |
| <code>2E8h–66Fh</code> | 904<br>bytes | <code>RESERVED</code> |  |  |
| <code>670h–76Fh</code> | 256<br>bytes | <code>LBR_STACK_FROM<br>LBR_STACK_TO</code> |  | Guest LastBranchStackFromIp and<br>LastBranchStackToIp MSRs in MSR address<br>order — only used if hardware acceleration of<br>LBR Stack virtualization is supported and<br>enabled by setting the<br>LBR_VIRTUALIZATION_ENABLE bit of the<br>VMCB control area. |
| <code>770h</code> | qword | <code>LBR_SELECT</code> |  | Guest LastBranchStackSelect MSR - only used<br>if hardware acceleration of LBR Stack<br>virtualization is supported and enabled by<br>setting the<br>LBR_VIRTUALIZATION_ENABLE bit of the<br>VMCB control area. |
| <code>778h</code> | qword | <code>IBS_FETCH_CTL</code> |  | IBS Virtualization state (swap type C). |
| <code>780h</code> | qword | <code>IBS_FETCH_<br>LINADDR</code> |  | IBS Virtualization state (swap type C). |
| <code>788h</code> | qword | <code>IBS_OP_CTL</code> |  | IBS Virtualization state (swap type C). |
| <code>790h</code> | qword | <code>IBS_OP_RIP</code> |  | IBS Virtualization state (swap type C). |
| <code>798h</code> | qword | <code>IBS_OP_DATA</code> |  | IBS Virtualization state (swap type C). |
| <code>7A0h</code> | qword | <code>IBS_OP_DATA2</code> |  | IBS Virtualization state (swap type C). |
| <code>7A8h</code> | qword | <code>IBS_OP_DATA3</code> |  | IBS Virtualization state (swap type C). |
| <code>7B0h</code> | qword | <code>IBS_DC_LINADDR</code> |  | IBS Virtualization state (swap type C). |
| <code>7B8h</code> | qword | <code>BP_IBSTGT_RIP</code> |  | IBS Virtualization state (swap type C). |
| <code>7C0h</code> | qword | <code>IC_IBS_EXTD_CTL</code> |  | IBS Virtualization state (swap type C). |
| <code>7C8h to ends of VMCB</code> |  | <code>RESERVED</code> |  |  |

## Table B-3. Swap Types

Physical PDF page 809; printed page 747.

| Swap Type | Behavior in VMRUN | Behavior in AE VMEXIT |
| --- | --- | --- |
| <code>A</code> | Host state saved to host save area<br>Guest state loaded from VMSA | Guest state saved to VMSA<br>Host state loaded from host save area |
| <code>B</code> | Guest state loaded from VMSA<br>(Host state not saved to host save area) | Guest state saved to VMSA<br>Host state loaded from host save area |
| <code>C</code> | Guest state loaded from VMSA<br>(Host state not saved to host save area) | Guest state saved to VMSA<br>Host state initialized to default (reset) values |

## Table B-4. VMSA Layout, State Save Area for SEV-ES

Physical PDF pages 809-814; printed pages 747-752. The VMSA begins at offset 0 in the page indicated by the VMSA Pointer. The source states that the host save area has the same format but begins at offset 400h in the host save page.

| Offset | Size | Content | Swap Type | Notes |
| --- | --- | --- | --- | --- |
| <code>000h</code> | 16 bytes | <code>ES</code> | A |  |
| <code>010h</code> | 16 bytes | <code>CS</code> | A |  |
| <code>020h</code> | 16 bytes | <code>SS</code> | A |  |
| <code>030h</code> | 16 bytes | <code>DS</code> | A |  |
| <code>040h</code> | 16 bytes | <code>FS</code> | B |  |
| <code>050h</code> | 16 bytes | <code>GS</code> | B |  |
| <code>060h</code> | 16 bytes | <code>GDTR</code> | A |  |
| <code>070h</code> | 16 bytes | <code>LDTR</code> | B |  |
| <code>080h</code> | 16 bytes | <code>IDTR</code> | A |  |
| <code>090h</code> | 16 bytes | <code>TR</code> | B |  |
| <code>0A0h</code> | qword | <code>PL0_SSP</code> | B |  |
| <code>0A8h</code> | qword | <code>PL1_SSP</code> | B |  |
| <code>0B0h</code> | qword | <code>PL2_SSP</code> | B |  |
| <code>0B8h</code> | qword | <code>PL3_SSP</code> | B |  |
| <code>0C0h</code> | qword | <code>U_CET</code> | B |  |
| <code>0C8h</code> | dword | <code>RESERVED</code> | – |  |
| <code>0CAh</code> | byte | <code>VMPL</code> | – | Swapped for guest. Not used in<br>host mode. |
| <code>0CBh</code> | byte | <code>CPL</code> | A |  |
| <code>0CCh</code> | dword | <code>RESERVED</code> | – |  |
| <code>0D0h</code> | qword | <code>EFER</code> | A |  |
| <code>0D8h-0DFh</code> | 8 bytes | <code>RESERVED</code> | – |  |
| <code>0E0h</code> | qword | <code>PERF_CTL0</code> | C |  |
| <code>0E8h</code> | qword | <code>PERF_CTR0</code> | C |  |
| <code>0F0h</code> | qword | <code>PERF_CTL1</code> | C |  |
| <code>0F8h</code> | qword | <code>PERF_CTR1</code> | C |  |
| <code>100h</code> | qword | <code>PERF_CTL2</code> | C |  |
| <code>108h</code> | qword | <code>PERF_CTR2</code> | C |  |
| <code>110h</code> | qword | <code>PERF_CTL3</code> | C |  |
| <code>118h</code> | qword | <code>PERF_CTR3</code> | C |  |
| <code>120h</code> | qword | <code>PERF_CTL4</code> | C |  |
| <code>128h</code> | qword | <code>PERF_CTR4</code> | C |  |
| <code>130h</code> | qword | <code>PERF_CTL5</code> | C |  |
| <code>138h</code> | qword | <code>PERF_CTR5</code> | C |  |
| <code>140h</code> | qword | <code>XSS</code> | B |  |
| <code>148h</code> | qword | <code>CR4</code> | A |  |
| <code>150h</code> | qword | <code>CR3</code> | A |  |
| <code>158h</code> | qword | <code>CR0</code> | A |  |
| <code>160h</code> | qword | <code>DR7</code> | C |  |
| <code>168h</code> | qword | <code>DR6</code> | C |  |
| <code>170h</code> | qword | <code>RFLAGS</code> | A |  |
| <code>178h</code> | qword | <code>RIP</code> | A |  |
| <code>180h</code> | qword | <code>DR0</code> | B |  |
| <code>188h</code> | qword | <code>DR1</code> | B |  |
| <code>190h</code> | qword | <code>DR2</code> | B |  |
| <code>198h</code> | qword | <code>DR3</code> | B |  |
| <code>1A0h</code> | qword | <code>DR0_ADDR_MASK</code> | B |  |
| <code>1A8h</code> | qword | <code>DR1_ADDR_MASK</code> | B |  |
| <code>1B0h</code> | qword | <code>DR2_ADDR_MASK</code> | B |  |
| <code>1B8h</code> | qword | <code>DR3_ADDR_MASK</code> | B |  |
| <code>1C0h</code> | qword | <code>INSTR_RETIRED_CTR</code> | A |  |
| <code>1C8h</code> | qword | <code>PERF_CTR_GLOBAL_STS</code> | A |  |
| <code>1D0h</code> | dword | <code>PERF_CTR_GLOBAL_CTL</code> | C |  |
| <code>1D4h-1D7h</code> | 4 bytes | <code>RESERVED</code> | – |  |
| <code>1D8h</code> | qword | <code>RSP</code> | A |  |
| <code>1E0h</code> | qword | <code>S_CET</code> | A |  |
| <code>1E8h</code> | qword | <code>SSP</code> | A |  |
| <code>1F0h</code> | qword | <code>ISST_ADDR</code> | A |  |
| <code>1F8h</code> | qword | <code>RAX</code> | A |  |
| <code>200h</code> | qword | <code>STAR</code> | B |  |
| <code>208h</code> | qword | <code>LSTAR</code> | B |  |
| <code>210h</code> | qword | <code>CSTAR</code> | B |  |
| <code>218h</code> | qword | <code>SFMASK</code> | B |  |
| <code>220h</code> | qword | <code>KernelGsBase</code> | B |  |
| <code>228h</code> | qword | <code>SYSENTER_CS</code> | B |  |
| <code>230h</code> | qword | <code>SYSENTER_ESP</code> | B |  |
| <code>238h</code> | qword | <code>SYSENTER_EIP</code> | B |  |
| <code>240h</code> | qword | <code>CR2</code> | C |  |
| <code>248h-267h</code> | 32 bytes | <code>RESERVED</code> | – |  |
| <code>268h</code> | qword | <code>G_PAT</code> | – | Swapped for guest, not used in<br>host mode. |
| <code>270h</code> | qword | <code>DBGCTL</code> | A |  |
| <code>278h</code> | qword | <code>BR_FROM</code> | A |  |
| <code>280h</code> | qword | <code>BR_TO</code> | A |  |
| <code>288h</code> | qword | <code>LASTEXCPFROM</code> | A |  |
| <code>290h</code> | qword | <code>LASTEXCPTO</code> | A |  |
| <code>298h</code> | qword | <code>DBGEXTNCFG</code> | A |  |
| <code>2A0-2DFh</code> | 72 bytes | <code>RESERVED</code> | – |  |
| <code>2E0h</code> | qword | <code>SPEC_CTRL</code> | A |  |
| <code>2E8h</code> | dword | <code>PKRU</code> | B |  |
| <code>2ECh</code> | dword | <code>TSC_AUX</code> | B |  |
| <code>2F0h</code> | qword | <code>GUEST_TSC_SCALE</code> | – |  |
| <code>2F8h</code> | qword | <code>GUEST_TSC_OFFSET</code> | – |  |
| <code>300h</code> | qword | <code>REG_PROT_NONCE</code> | – |  |
| <code>308h</code> | qword | <code>RCX</code> | B |  |
| <code>310h</code> | qword | <code>RDX</code> | B |  |
| <code>318h</code> | qword | <code>RBX</code> | B |  |
| <code>320h</code> | qword | <code>SECURE_AVIC_CTL</code> | – |  |
| <code>328h</code> | qword | <code>RBP</code> | B |  |
| <code>330h</code> | qword | <code>RSI</code> | B |  |
| <code>338h</code> | qword | <code>RDI</code> | B |  |
| <code>340h</code> | qword | <code>R8</code> | B |  |
| <code>348h</code> | qword | <code>R9</code> | B |  |
| <code>350h</code> | qword | <code>R10</code> | B |  |
| <code>358h</code> | qword | <code>R11</code> | B |  |
| <code>360h</code> | qword | <code>R12</code> | B |  |
| <code>368h</code> | qword | <code>R13</code> | B |  |
| <code>370h</code> | qword | <code>R14</code> | B |  |
| <code>378h</code> | qword | <code>R15</code> | B |  |
| <code>380h</code> | 16 bytes | <code>RESERVED</code> | – |  |
| <code>390h</code> | qword | <code>GUEST_EXITINFO1</code> | – | EXITINFO1 for AE exits |
| <code>398h</code> | qword | <code>GUEST_EXITINFO2</code> | – | EXITINFO2 for AE exits |
| <code>3A0h</code> | qword | <code>GUEST_EXITINTINFO</code> | – | EXITINTINFO for AE exits |
| <code>3A8h</code> | qword | <code>GUEST_NRIP</code> | – | Next sequential instruction<br>pointer for AE exits |
| <code>3B0h</code> | qword | <code>SEV_FEATURES</code> | – | Guest-controlled SEV feature<br>selection<br>• Bit 0: SNPActive<br>• Bit 1: vTOM<br>• Bit 2: ReflectVC<br>• Bit 3: RestrictedInjection<br>• Bit 4: AlternateInjection<br>• Bit 5: DebugVirtualization<br>• Bit 6: PreventHostIBS<br>• Bit 7: BTBIsolation<br>• Bit 8: VmplSSS<br>• Bit 9: SecureTSC<br>• Bit 10: VmgexitParameter<br>• Bit 11: PmcVirtualization *<br>• Bit 12: IbsVirtualization<br>• Bit 13: GuestInterceptCtl *<br>• Bit 14: VmsaRegProt<br>• Bit 15: SmtProtection<br>• Bit 16: SecureAvic *<br>• Bit 20:17: Reserved, SBZ<br>• Bit 21: IbpbOnEntry<br>• Bits 63:22: Reserved, SBZ<br>* - This feature may only be<br>used if Allowed SEV Features<br>is enabled and the Allowed<br>SEV Features Mask permits the<br>use of the feature. |
| <code>3B8h</code> | qword | <code>VINTR_CTRL</code> | – | • Bits 7:0: V_TPR<br>• Bit 8: V_IRQ<br>• Bit 9: VGIF<br>• Bit 10: INT_SHADOW<br>• Bit 11: V_NMI<br>• Bit 12: V_NMI_MASK<br>• Bits 15:13: Reserved, SBZ<br>• Bits 19:16: V_INTR_PRIO<br>• Bit 20: V_IGN_TPR<br>• Bits 25:21: Reserved, SBZ<br>• Bit 26: V_NMI_ENABLE<br>• Bits 31:27: Reserved, SBZ<br>• Bits 39:32:<br>V_INTR_VECTOR<br>• Bits 62:40: Reserved, SBZ<br>• Bit 63: BUSY |
| <code>3C0h</code> | qword | <code>GUEST_EXITCODE</code> | – | EXITCODE for AE exits |
| <code>3C8h</code> | qword | <code>VIRTUAL_TOM</code> | – | Swapped for guest, not used in<br>host mode. Only bits 51:21 are<br>observed. |
| <code>3D0h</code> | qword | <code>TLB_ID</code> | – |  |
| <code>3D8h</code> | qword | <code>PCPU_ID</code> | – |  |
| <code>3E0h</code> | qword | <code>EVENTINJ</code> | – | Same as the EVENTINJ field<br>in the VMCB (Table B-1) at<br>offset 0A8h. |
| <code>3E8h</code> | qword | <code>XCR0</code> | B |  |
| <code>3F0h-3FFh</code> | 16 bytes | <code>Reserved</code> | – |  |
| <code>400h</code> | qword | <code>X87_DP</code> | C | FP x87 data pointer |
| <code>408h</code> | dword | <code>MXCSR</code> | C | FP MXCSR |
| <code>40Ch</code> | word | <code>X87_FTW</code> | C | FP x87 tag word |
| <code>40Eh</code> | word | <code>X87_FSW</code> | C | FP x87 status word |
| <code>410h</code> | word | <code>X87_FCW</code> | C | FP control word |
| <code>412h</code> | word | <code>X87_FOP</code> | C | FP x87 opcode |
| <code>414h</code> | word | <code>X87_DS</code> | C | FP x87 DS |
| <code>416h</code> | word | <code>X87_CS</code> | C | FP x87 CS |
| <code>418h</code> | qword | <code>X87_RIP</code> | C | FP x87 RIP |
| <code>420h-46Fh</code> | 80 bytes | <code>FPREG_X87</code> | C | X87 register state (stack order) |
| <code>470h-56Fh</code> | 256 bytes | <code>FPREG_XMM</code> | C | XMM register state |
| <code>570h-66Fh</code> | 256 bytes | <code>FPREG_YMM</code> | C | YMM_HI register state |
| <code>670h-76Fh</code> | 256 bytes | <code>LBR_STACK_FROM<br>LBR_STACK_TO</code> | C | LBR Stack state |
| <code>770h</code> | qword | <code>LBR_SELECT</code> | C | LastBranchStackSelect state |
| <code>778h</code> | qword | <code>IBS_FETCH_CTL</code> | C | IBS Virtualization state |
| <code>780h</code> | qword | <code>IBS_FETCH_LINADDR</code> | C | IBS Virtualization state |
| <code>788h</code> | qword | <code>IBS_OP_CTL</code> | C | IBS Virtualization state |
| <code>790h</code> | qword | <code>IBS_OP_RIP</code> | C | IBS Virtualization state |
| <code>798h</code> | qword | <code>IBS_OP_DATA</code> | C | IBS Virtualization state |
| <code>7A0h</code> | qword | <code>IBS_OP_DATA2</code> | C | IBS Virtualization state |
| <code>7A8h</code> | qword | <code>IBS_OP_DATA3</code> | C | IBS Virtualization state |
| <code>7B0h</code> | qword | <code>IBS_DC_LINADDR</code> | C | IBS Virtualization state |
| <code>7B8h</code> | qword | <code>BP_IBSTGT_RIP</code> | C | IBS Virtualization state |
| <code>7C0h</code> | qword | <code>IC_IBS_EXTD_CTL</code> | C | IBS Virtualization state |
| <code>7C8h-8FFh</code> | 312 bytes | <code>RESERVED</code> | – |  |
| <code>900h</code> | dword | <code>INTERCEPT_VEC_0</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>904h</code> | dword | <code>INTERCEPT_VEC_1</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>908h</code> | dword | <code>INTERCEPT_VEC_2</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>90Ch</code> | dword | <code>INTERCEPT_VEC_3</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>910h</code> | dword | <code>INTERCEPT_VEC_4</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>914h</code> | dword | <code>INTERCEPT_VEC_5</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>918h</code> | dword | <code>INTERCEPT_VEC_6</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>91Ch</code> | dword | <code>INTERCEPT_VEC_7</code> | – | Guest Intercept Control<br>See Section 15.36.23, “Guest Intercept Control,” on page 623. |
| <code>920h</code> | qword | <code>INTERCEPT_MSR_VEC_0</code> | – | See Table B-5. |
| <code>928h</code> | qword | <code>INTERCEPT_MSR_VEC_1</code> | – | See Table B-6. |
| <code>930h</code> | qword | <code>INTERCEPT_MSR_VEC_2</code> | – | See Table B-7. |
| <code>938h</code> | qword | <code>INTERCEPT_MSR_VEC_3</code> | – | See Table B-8. |
| <code>940h-97Fh</code> | 64 bytes | <code>RESERVED</code> | – |  |
| <code>980h-9BFh</code> | 64 bytes | <code>FPREG_K</code> | C | AVX512 K0-K7 opmask<br>registers |
| <code>9C0h-BBFh</code> | 512 bytes | <code>FPREG_ZMMHI</code> | C | ZMM0-ZMM15 register state,<br>upper half |
| <code>BC0h-FBFh</code> | 1024 bytes | <code>FPREG_HIZMM</code> | C | ZMM16-ZMM31 register state |

## Source-document consistency findings

These findings concern the AMD source as printed; they are not transcription corrections:

1. **Table B-1, offset 060h:** bit 10 is not listed. The sequence goes from bit 9 to bit 11.
2. **Table B-2:** the source jumps from offset 138h to 148h, leaving 140h-147h unlisted.
3. **Tables B-2 and B-4:** the range 2A0h-2DFh contains 64 bytes, while the source declares 72 bytes.
4. **Table B-4:** the row at 0C8h declares a dword RESERVED field, which nominally covers 0C8h-0CBh, while 0CAh and 0CBh are separately defined as VMPL and CPL.

These anomalies were preserved exactly in the tables above so the transcription remains faithful to Revision 3.44.

## Audit metadata

- Source PDF SHA-256: `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`
- Table B-1 data rows: **147**
- Table B-2 data rows: **107**
- Table B-3 data rows: **3**
- Table B-4 data rows: **152**
- Total table data rows: **409**
