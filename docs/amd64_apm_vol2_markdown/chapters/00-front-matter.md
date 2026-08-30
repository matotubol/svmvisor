# Front Matter


<!-- PDF source page: 1 -->

**AMD64 Technology**

<a id="amd64-architecture-programmers-manual-volume-2-system-programming"></a>

# AMD64 Architecture Programmer’s Manual Volume 2: System Programming

**Volume 2: System Programming**

**Publication No. Revision Date**

24593 3.44 March 2026


<!-- PDF source page: 2 -->

The information contained herein is for informational purposes only, and is subject to change without notice. While every precaution has been taken in the preparation of this document, it may contain technical inaccuracies, omissions and typographical errors, and AMD is under no obligation to update or otherwise correct this information. Advanced Micro Devices, Inc. makes no representations or warranties with respect to the accuracy or completeness of the contents of this document, and assumes no liability of any kind, including the implied warranties of noninfringement, merchantability or fitness for particular purposes, with respect to the operation or use of AMD hardware, software or other products described herein. No license, including implied or arising by estoppel, to any intellectual property rights is granted by this document. Terms and limitations applicable to the purchase or use of AMD products are as set forth in a signed agreement between the parties or in AMD Standard Terms and Conditions of Sale. Any unauthorized copying, alteration, distribution, transmission, performance, display or other use of this material is prohibited.

**Trademarks**

AMD, the AMD arrow logo, and combinations thereof, AMD Virtualization and 3DNow! are trademarks of Advanced Micro Devices, Inc. Other product names used in this publication are for identification purposes only and may be trademarks of their respective companies.

MMX is a trademark and Pentium is a registered trademark of Intel Corporation.

HyperTransport is a licensed trademark of the HyperTransport Technology Consortium.


<!-- PDF source page: 36 | printed page: xxxvi -->

<a id="revision-history"></a>

## Revision History

| Date | Revision | Description |
| --- | --- | --- |
| March 2026 | 3.44 | Corrected Figure 15-32 in Section 15.36.21.3, ”Secure AVIC Control MSR,” on<br>page 619. |
| June 2025 | 3.43 | Added new features:<br>• AVX512 in Section 11, ”SSE, MMX, and x87 Programming,” on page 342 and<br>throughout the document.<br>• Segmented RMP in Section 15.36.22 on page 621.<br>• Guest Intercept Control in Section 15.36.23 on page 623.<br>• Support for up to 4096 vCPUs in x2AVIC mode in Section 15.29, ”Advanced<br>Virtual Interrupt Controller,” on page 563.<br>• Selective Branch Prediction Barrier (SBPB) in Section 3.2.9, ”Speculation<br>Control Registers,” on page 66.<br>• Enhanced Return Address Predictor Security (ERAPS) in Section 3.2.9,<br>”Speculation Control Registers,” on page 66 and Section 15.37.2, ”ERAPS<br>Virtualization,” on page 624.<br>• PREC RET in PerfEvtSeln MSR in Section 13.2.1, ”Performance Counter<br>MSRs,” on page 411.<br>• 4 Kbyte coalesced IBS Fetch L1 TLB Page Size in Section 13.3.2, ”IBS Fetch<br>Sampling Registers,” on page 425.<br>Corrected:<br>• #CP Error Codes in Section 8.4.3, ”Control-Protection Error Code,” on<br>page 263.<br>• CET U and CET S component identification in Section 18.13,<br>_ _<br>”XSAVE/XRSTOR,” on page 691. |
| March 2024 | 3.42 | Added new features:<br>• Replaced MOESI with MOESDIF protocol in Section 7.3, ”Memory Coherency<br>and Protocol,” on page 192.<br>• Indirect Branch Prediction Barrier on Entry under in Section 15.36.17, ”Side-<br>Channel Protection,” on page 615.<br>• Idle HLT intercept in Section 15.9, ”Instruction Intercepts,” on page 513.<br>• Allowed SEV Features in Section 15.36.20 on page 617.<br>• Secure AVIC in Section 15.36.21 on page 618.<br>• Performance Monitoring Counter Virtualization in Section 15.39 on<br>page 626. |

<details>
<summary>Rendered source page 36 (figures/tables)</summary>

![Rendered source PDF page 36](../assets/pages/pdf-page-0036.webp)

</details>


<!-- PDF source page: 37 | printed page: xxxvii -->

| Date | Revision | Description |
| --- | --- | --- |
| June 2023 | 3.41 | Added the following corrections/clarifications:<br>Section 2.5.2, Added that an exception exists when Upper Address Ignore<br>(UAI) is enabled.<br>Section 15.30.4: Removed statement that "software must not attempt to<br>read or write the host save-state area directly."<br>Added AE Exitcode A5h, VMEXIT BUSLOCK, to Table 15-35<br>Section 15.36.13: Removed sentence: "Note that the DR6 and DR7 registers<br>are always swapped as type ‘A’ state for any SEV guest."<br>Added new MSRs to Table A-1.<br>Added new features:<br>Bus Lock: Section 7.3.3, section 8.2.2, Figure 13.2, Figure 13.4, section<br>13.1.3.6, section 15.14.5, Tables B-1 and C-1.<br>Collaborative Processor Performance Control (CPPC): Section 17.6 and<br>Section A.14.<br>Platform Quality of Service: Chapter 19 and Section A.13. |
| January<br>2023 | 3.40 | Added the following corrections/clarifications:<br>Replaced figure 5-17 with correct drawing<br>Added paragraph at end of section 7.3.2<br>Corrected description for the “Valid” field in the LastBranchStackToIp<br>MSR documentation in section 13.1.1.9<br>Corrected heading text for first and second columns of table 13-3<br>Corrected heading level of section 13.3.5, IBS Filtering<br>Removed a duplicate paragraph from section 15.34.10 |

<details>
<summary>Rendered source page 37 (figures/tables)</summary>

![Rendered source PDF page 37](../assets/pages/pdf-page-0037.webp)

</details>


<!-- PDF source page: 38 | printed page: xxxviii -->

| Date | Revision | Description |
| --- | --- | --- |
| November<br>2022 | 3.39 | Added corrections/clarifications to:<br>Write combining memory type in section 7.4<br>Added new features:<br>5-level paging in section 5.3<br>Automatic IBRS in section 3.1.7<br>Section 7.10.9 Secure Multi-Key Memory Encryption<br>CPUID disable for non-privileged software in section 3.2.10<br>Section 7.6.6 L3 Cache Range Reservation<br>Last Branch Record Stack in section 13.1.1.8<br>Core Performance Global Control Register in section 13.2.1<br>Core Performance Counter Status Registers in section 13.2.1<br>Section 13.3.4.1 IBS Filtering<br>Section 15.21.10 NMI Virtualization<br>Read Only Guest Page Tables in section 15.25.5<br>Section 15.29.10 x2AVIC<br>VMGEXIT Parameter in section 15.35.6<br>VMPL Supervisor Shadow Stack in section 15.36.7<br>Virtual TOM MSR in section 15.36.8<br>SMT Protection for SEV-SNP VMs in section 15.36.17<br>Section 15.36.19 SEV-SNP Instruction Virtualization<br>Section 15.38 Instruction-Based Sampling Virtualization |
| November<br>2021 | 3.38 | Added corrections/clarifications to:<br>Address generation in Section 4.5.3.<br>PCID behavior in Section 5.5.<br>Machine Check Exception in Section 8.2.18.<br>SEV Status MSR in Section 15.34.10.<br>SEV SNP in Sections 15.36.7, .10, .17.<br>VMCB and VMSA contents in Appendix B.<br>SVM Intercepts in Appendix C.<br>Added new features:<br>Chapter 5: Upper Address Ignore.<br>Chapter 9, Section 3.2.6 and Appendix A: Machine Check Architecture<br>Extension.<br>Section 15.36: VMSA Register Protection, SEV-SNP VMSA Register Protection<br>and Secure TSC. |

<details>
<summary>Rendered source page 38 (figures/tables)</summary>

![Rendered source PDF page 38](../assets/pages/pdf-page-0038.webp)

</details>


<!-- PDF source page: 39 | printed page: xxxix -->

| Date | Revision | Description |
| --- | --- | --- |
| March 2021 | 3.37 | Added SPEC CTRL and PRED CMD support.<br>_ _<br>Added x2APIC support.<br>Added SMM Page Configuration Lock.<br>Corrected the AMD64 Architecture SMM State-Save Area table.<br>Add new section: APERF Read-only (AperfReadOnly).<br>Updated the MSRs of the AMD64 Architecture table.<br>Updated the Machine-Check MSR Cross-Reference table.<br>Updated the Performance-Monitoring MSR Cross-Reference table.<br>Updated the Secure Virtual Machine MSR Cross-Reference table.<br>Added new section: Speculation Control MSRs. |
| August 2020 | 3.36 | Added Shadow Stack support.<br>Chapter 3: Section 3.1: Added content. Section 3.1.3: Added bit 23 and new<br>content. Section 3.2.7: Added Shadow Stack Registers as new section 3.2.7.<br>Chapter 5: Section 5.6: Updated content.<br>Chapter 6: Added content to Table 6-1. Added section 6.7.<br>Chapter 7: Table 7-3. Updated.<br>Chapter 8: Table 8-1 and Table 8-2: Added content. Section 8.4: Updated<br>content. Section 8.4.2: Added content. Added #CP as new section 8.2.20.<br>Table 8-8: Added content. Added new Shadow Stack section 8.7.6. Section 8.9:<br>Added bullet. Added new 8.9.4.1 section.<br>Chapter 10: Updated Table 10-1. Section 10.4: Added bullet.<br>Chapter 11: Section 11.5.2 and Figure 11-8: Updated table and figure. Section<br>11.5.8: Updated Table 11-3.<br>Chapter 12: Section 12.2.2: Updated. Section 12.2.4: Updated Figure 12-6.<br>Section 12.3.2: Updated content and added bullets.<br>Chapter 15: Section 15.5.1: Updates. Section 15.15.3: Updated Figure 15-4.<br>Section 15.25.6: Added bullet. Section 15.29.1. Updates. Section 15.29.4.1:<br>Added content. Section 15.29.8.2: Added content.<br>Added Chapter 18.<br>Appendix A: Table A-1: Added content. Added new section A.10. Appendix B:<br>Table B-1, Table B-2, and Table 4: Added content. |
| May 2020 | 3.35 | Sections 5.6.1 and 5.6.6: Minor updates.<br>Figure 5-16: Updated figure. |

<details>
<summary>Rendered source page 39 (figures/tables)</summary>

![Rendered source PDF page 39](../assets/pages/pdf-page-0039.webp)

</details>


<!-- PDF source page: 40 | printed page: xl -->

| Date | Revision | Description |
| --- | --- | --- |
| April 2020 | 3.34 | Section 3.1.3: Updated register information. Added PCIDE and PKE registers.<br>Updated (TCE) content.<br>Section 5.3.2: Added Process Context Identifier register information and<br>register figure.<br>Section 5.3.3: Updated figure.<br>Section 5.3.4: Updated figure.<br>Section 5.3.5: Updated figure.<br>Section 5.4.1: Added (MPK) register information.<br>Section 5.5.1: Inserted Process Context Identifiers as Section 5.5.1.<br>Section 5.5.3: Added bullets to Implicit Invalidations list.<br>Section 5.6: Updated content.<br>Section 8.2.15: Added bullet.<br>Section 8.4.2: Updated register figure and added PK register information.<br>Section 11.5.2: Updated register figure and table.<br>Section 14.1.3: Updated table.<br>Section 15.9: Updated table.<br>Appendix B: Updated table.<br>Appendix C: Updated table. |

<details>
<summary>Rendered source page 40 (figures/tables)</summary>

![Rendered source PDF page 40](../assets/pages/pdf-page-0040.webp)

</details>


<!-- PDF source page: 41 | printed page: xli -->

| Date | Revision | Description |
| --- | --- | --- |
| April 2020 | 3.33 | Section 1.1.2: Clarification on address size support.<br>Section 3.2.1: New feature enable bits in SYSCFG MSR.<br>Section 7.6.5: Updated terminology.<br>Section 7.10.6: Clarification to encrypted memory operation.<br>Section 8.1.4: Clarification to IRET and NMI behavior.<br>Tables 8-1 and 8-2: Added #HV exception.<br>Inserted new 8.2.20 section for #HV exception.<br>Section 8.4.2: Changes for SEV-SNP extension.<br>Table 8-8: Added SEV-related exceptions.<br>Figure 10-6: Updated I/O Restart DWORD.<br>Section 15 and 15.1: General updates.<br>Section 15.5.2: Relocated VMLOAD/VMSAVE documentation.<br>Section 15.2.4 and 15.2.6: Updated content.<br>Section 15.6: Added content.<br>Table 15-7: Added content.<br>Section 15.25.6: Clarification.<br>Section 15.25.13: General clarifications.<br>Section 15.33.1 and 15.33.2: General clarifications.<br>Section 15.34.3, 15.34.7, and 15.34.10: Clarifications, and additions for SEV-<br>SNP.<br>Table 15-35: Added content.<br>Section 15.35.8: Corrected terminology.<br>Section 15.36: Added SEV-SNP extension documentation.<br>Table A-1 and Table A-7: Added SEV-SNP related MSRs.<br>Appendix B: Updates for SEV-SNP extension.<br>Table C-1: Added exit code for SEV-SNP extension. |
| October<br>2019 | 3.32 | Added UMIP, XSS, GMET, VTE, MCOMMIT, and RDPRU. |
| July 2019 | 3.31 | Added CLWB and WBNOINVD details.<br>Clarified FP error pointer save/restore behavior.<br>Corrected description of APIC Software Enable functionality.<br>Clarified canonical address checking behavior.<br>Clarified fault generation. March 2026n for instructions that cross page or<br>segment boundaries. |

<details>
<summary>Rendered source page 41 (figures/tables)</summary>

![Rendered source PDF page 41](../assets/pages/pdf-page-0041.webp)

</details>


<!-- PDF source page: 42 | printed page: xlii -->

| Date | Revision | Description |
| --- | --- | --- |
| September<br>2018 | 3.30 | Modified Section 7.4<br>Modified Section 7.6.4<br>Modified Section 8.5.2<br>Modified Section 9.2<br>Corrected Figure 9-4<br>Corrected Table 9-1<br>Modified Section 9.3.2<br>Corrected Figure 9-6<br>Corrected Table 9-4<br>Modified Section 14.2.3<br>Modified Section 14.4<br>Modified Section 15.6<br>Modified Section 15.7<br>Modified Section 15.34.9<br>Modified Section 15.34.10<br>Modified Section 15.35.2<br>Corrected Table B-4 in Appendix B |
| December<br>2017 | 3.29 | Modified Sections 7.10.1 and 7.10.4.<br>Modified Sections 15.34.1, 15.34.7.<br>Added new Section 15.34.10.<br>Modified Section 15.35.10.<br>Modified Appendix A, Table A-7. |
| March 2017 | 3.28 | Modified CR4 Register, Section 3.1.3.<br>Removed UD2 in Table 6-1.<br>Added new bullet in Section 7.1.1.<br>Modified Note in Table 7-1.<br>Added new Section 7.4.1.<br>Clarified Self Modifying Code in Section 7.6.1.<br>Added UD0 and UD1 instructions in Section 8.2.7.<br>Added Instructions Retired Performance counter in Section 13.1.1.<br>Modified Table in Section 15.34.9. |

<details>
<summary>Rendered source page 42 (figures/tables)</summary>

![Rendered source PDF page 42](../assets/pages/pdf-page-0042.webp)

</details>


<!-- PDF source page: 43 | printed page: xliii -->

| Date | Revision | Description |
| --- | --- | --- |
| December<br>2016 | 3.27 | Added Resume Flag (RF) Bit in Section 3.1.6, ”RFLAGS Register,” on page 51.<br>Added Tom2ForceMemTypeWB in Section 3.2.1, ”System Configuration<br>Register (SYSCFG),” on page 60.<br>Clarified SYSCALL and SYSRET in Section 6.1.1, ”SYSCALL and SYSRET,”<br>on page 174.<br>Added Section 7.3.2, ”Access Atomicity,” on page 195.<br>Updated Note b in Table 7-12 on page 232.<br>Modified Table 8-1, “Interrupt Vector Source and Cause,” on page 246.<br>Modified Table 8-2, “Interrupt Vector Classification,” on page 247.<br>Added Section 8.2.22, ”#VC—VMM Communication Exception (Vector 29),” on<br>page 261.<br>Added a Note in Chapter 10, "System-Management Mode," on page 324.<br>Added Section 10.5, ”Multiprocessor Considerations,” on page 341.<br>Updated CPUID 8000 001F[EAX] and added CPUID 8000 001F[EDX]<br>_ _<br>in Section 15.34.1, ”Determining Support for SEV,” on page 589.<br>Added new Section 15.35, ”Encrypted State (SEV-ES),” on page 595.<br>Clarified TSC Ratio MSR in Section 15.30.5 ”TSC Ratio MSR (C000 0104h)” on<br>page 585.<br>Modified Appendix B, ”VMCB Layout” on page 737.<br>Added Table B-3, “Swap Types,” on page 747.<br>Added Codes 8Fh, 90h-9Fh, and 403h in Table C-1, “SVM Intercept Codes,” on<br>page 756. |
| April 2016 | 3.26 | Clarification on loading a null selector into FS or GS added in Section 4.5.3,<br>”Segment Registers in 64-Bit Mode,” on page 80<br>Translation table diagrams corrected for definition of bit 8 in Section 5.5,<br>”Translation-Lookaside Buffer (TLB),” on page 157<br>CR0.CD implementation-dependent behavior noted in Section 7.6.2, ”Cache<br>Control Mechanisms,” on page 207<br>Added clarification on IST usage in Section 8.9.4, ”Interrupt-Stack Table,” on<br>page 285.<br>Added new Section 7.10, ”Secure Memory Encryption,” on page 238.<br>Added guideline for secure AP startup in Section 15.27.8, ”Secure<br>Multiprocessor Initialization,” on page 561<br>Added TLB maintenance requirement for multiprocessor VM's in Section<br>15.29.4, ”VMCB Changes for AVIC,” on page 569.<br>Added new Section 15.34, ”Secure Encrypted Virtualization,” on page 588 |
| June 2015 | 3.25 | Added new section 15.33 Nested Virtualization for coverage of VMSAVE and<br>VMLOAD Virtualization and Virtual GIF.<br>Various minor edits. |

<details>
<summary>Rendered source page 43 (figures/tables)</summary>

![Rendered source PDF page 43](../assets/pages/pdf-page-0043.webp)

</details>


<!-- PDF source page: 44 | printed page: xliv -->

| Date | Revision | Description |
| --- | --- | --- |
| October<br>2013 | 3.24 | Added description of Supervisor-Mode Execution Prevention. See Section 5.6.5<br>”Supervisor-Mode Execution Prevention (CR4.SMEP) Bit” on page 164.<br>Indicated the deprecation of the Processor Feedback Interface. See Section<br>17.4, ”Processor Feedback Interface,” on page 669.<br>Added Section 17.5, ”Processor Core Power Reporting,” on page 669. |
| May 2013 | 3.23 | Clarified guidelines for implementing cross-modifying code in the sub-section<br>”Cross-Modifying Code” on page 206.<br>Added AVIC description. See Section 15.29, ”Advanced Virtual Interrupt<br>Controller,” on page 563.<br>Added L2I PMC architecture definition. See Section 13.2, ”Performance<br>Monitoring Counters,” on page 411. |
| September<br>2012 | 3.22 | Clarified processor behavior on write of EFER[LMA] bit in Section 3.1.7<br>”Extended Feature Enable Register (EFER)” on page 55.<br>Clarified difference between cold reset and warm reset in Section 9.3,<br>”Machine Check Architecture MSRs,” on page 301.<br>Added information on FFXSR feature bit to Table 11-5 on page 343.<br>Clarified SMM code responsibility to manage VMCB clean bits. See Section<br>15.15.2, ”Guidelines for Clearing VMCB Clean Bits,” on page 526.<br>Added a note to Table 15-9 on page 529 to indicate that all encodings of<br>TLB CONTROL not defined are reserved.<br>Corrected information concerning the assignment of logical APIC IDs in Section<br>16.6.1, ”Receiving System and IPI Interrupts,” on page 645. |
| March 2012 | 3.21 | Added definition of processor feedback interface—frequency sensitivity monitor<br>(See Section 17.4, ”Processor Feedback Interface,” on page 669)<br>Added Instruction-Based Sampling in a new section of Chapter 13 (See Section<br>13.3, ”Instruction-Based Sampling,” on page 424.)<br>Reworked Introduction and first section of Chapter 9, "Machine Check<br>Architecture," on page 296 and added deferred error handling.<br>Added description of CR4[FSGSBASE] bit. (See Section 3.1.3, ”CR4 Register,”<br>on page 46.)<br>Added references to the RDFSBASE, RDGSBASE, WRFSBASE, and WRGSBASE<br>instructions in discussion of FS and GS segment descriptors. (See ”FS and GS<br>Registers in 64-Bit Mode” on page 80)<br>Added Section 6.3.2, ”Accessing Segment Register Hidden State,” on page 179. |
| December<br>2011 | 3.20 | Clarified description of the Cache Disable (CD) memory type in Section 7.4<br>”Memory Types” on page 198.<br>Added caveat: an overflow of either APERF or MPERF can invalidate the<br>effective frequency calculation. See Section 17.3, ”Determining Processor<br>Effective Frequency,” on page 667.<br>Other minor editorial changes. |

<details>
<summary>Rendered source page 44 (figures/tables)</summary>

![Rendered source PDF page 44](../assets/pages/pdf-page-0044.webp)

</details>


<!-- PDF source page: 45 | printed page: xlv -->

| Date | Revision | Description |
| --- | --- | --- |
| September<br>2011 | 3.19 | Added XSAVEOPT to discussions on XSAVE.<br>Corrections to discussion on multiprocessor memory access ordering in<br>Chapter 7.<br>Added discussion of extended core and northbridge performance counters and<br>feature indicators to Chapter 13.<br>Added Lightweight Profiling (LWP) to Chapter 13.<br>Added Global Timestamp Counter, Continuous Mode to LWP description<br>Clarification: Function of pin A20M# is only defined in real mode. Statement<br>added to Section 1.2.4, ”Real Addressing,” on page 10.<br>Eliminated hardware P-state references |
| May 2011 | 3.18 | Added information for OSXSAVE and XSAVE features.<br>Added Cache Topology, Pause Filter Threshold, and XSETBV information.<br>Updated TSC ratio information.<br>Corrected description of FXSAVE/FXRSTOR exception behavior when<br>CR0.EM=1 |
| June 2010 | 3.17 | Replaced missing figures in Chapter 8, "Exceptions and Interrupts," on<br>page 242. |
| June 2010 | 3.16 | Updated information on performance monitoring counters in ”Performance-<br>Monitoring Counter Enable (PCE)” on page 49 and 6.2.5, ”Accessing Model-<br>Specific Registers” on page 178.<br>Revised Table 4-1, ”Segment Registers” on page 79.<br>Add flush by ASID information to section 15.16, ”TLB Control” on page 528.<br>Added information on VMCB clean field to Chapter15, ”Secure Virtual Machine”<br>on page 498 and Appendix B, ”VMCB Layout” on page 737.<br>Added section 15.10, ”IOIO Intercepts” on page 516.<br>Added section 15.30.5, ”TSC Ratio MSR (C000 0104h)” on page 585.<br>Added section 17.2, ”Core Performance Boost” on page 666. |

<details>
<summary>Rendered source page 45 (figures/tables)</summary>

![Rendered source PDF page 45](../assets/pages/pdf-page-0045.webp)

</details>


<!-- PDF source page: 46 | printed page: xlvi -->

| Date | Revision | Description |
| --- | --- | --- |
| November<br>2009 | 3.15 | Added section 7.5, ”Buffering and Combining Memory Writes” on page 202<br>Added MFENCE to list of ”Serializing Instructions” on page 211.<br>Updated section 7.6.1, ”Cache Organization and Operation” on page 204.<br>Updated Table 7-4, “Memory Access Ordering Rules,” on page 201 and notes.<br>Updated 7.4, ”Memory Types” on page 198.<br>Clarified 5.5.3, ”TLB Management” on page 159.<br>Added ”Invalidation of Table Entry Upgrades.” on page 160.<br>Updated ”Speculative Caching of Address Translations” on page 160.<br>Update ”Handling of D-Bit Updates” on page 161.<br>Revised and updated section 7.2, ”Multiprocessor Memory Access Ordering” on<br>page 189 ff.<br>Added information on long mode segment-limit checks in ”Extended Feature<br>Enable Register (EFER)” on page 56table on page 56 and ”Long Mode Segment<br>Limit Enable (LMSLE) Bit” on page 57 on page 57.<br>Added discussion of ”Data Limit Checks in 64-bit Mode” on page 123on page<br>123.<br>Updated Table 6-1, “System Management Instructions,” on page 170.<br>Updated ”Canonicalization and Consistency Checks” on page 504on page 504.<br>Added information about the next sequential instruction pointer (nRIP) in<br>15.7.1, ”State Saved on Exit” on page 508.<br>Updated priority definition of PAUSE instruction intercept in Table 15-7,<br>“Instruction Intercepts,” on page 513.<br>Added nRIP field to Table B-1, “VMCB Layout, Control Area,” on page 737.<br>Clarified information on ICEBP event injection, on page 531.<br>Deleted erroneous statement concerning the operation of the General Local<br>Vector Table register Mask bit in section 16.4.<br>Clarified the description of the Interrupt Command Register Delivery Status bit<br>in section ”Interprocessor Interrupts (IPI)” on page 641on page 641. |
| September<br>2007 | 3.14 | Added information on ”Speculative Caching of Address Translations,” ”Caching<br>of Upper Level Translation Table Entries,” ”Use of Cached Entries When<br>Reporting a Page Fault Exception,” ”Use of Cached Entries When Reporting a<br>Page Fault Exception,” ”Handling of D-Bit Updates,” ”Invalidation of Cached<br>Upper-level Entries by INVLPG” on page 161 and ”Handling of PDPT Entries in<br>PAE Mode” on page 161to section 5.5.3, ”TLB Management” on page 159.<br>Added 15.21.7, ”Interrupt Masking in Local APIC” on page 535.<br>Added 16.3.6, ”Extended APIC Control Register” on page 633; clarified the use<br>of the ICR DS bit in 16.5, ”Interprocessor Interrupts (IPI)” on page 641.<br>Added minor clarifications and corrected typographical and formatting errors. |

<details>
<summary>Rendered source page 46 (figures/tables)</summary>

![Rendered source PDF page 46](../assets/pages/pdf-page-0046.webp)

</details>


<!-- PDF source page: 47 | printed page: xlvii -->

| Date | Revision | Description |
| --- | --- | --- |
| July 2007 | 3.13 | Added 5.3.5, ”1-Gbyte Page Translation” on page 149.<br>Added 7.2, ”Multiprocessor Memory Access Ordering” on page 189<br>Added divide-by-zero exception to Table 8-9, “Simultaneous Interrupt<br>Priorities,” on page 264.<br>Added information on ”CPU Watchdog Timer Register” on page 304and<br>”Machine-Check Miscellaneous-Error Information Register 0 (MCi MISC0)” on<br>page 311to Chapter 9.<br>Added SSE4A support to Chapter 11, ”SSE, MMX, and x87 Programming” on<br>page 342.<br>Added Monitor and MWAIT intercept information to section 15.9, ”Instruction<br>Intercepts” on page 513 and reorganized intercept information; clarified<br>15.16.1, ”TLB Flush” on page 529.<br>Added Monitor and MWAIT intercepts to tables B-1, ”VMCB Layout, Control<br>Area” on page 737 and C-1, ”SVM Intercept Codes” on page 756.<br>Added Chapter 16, ”Advanced Programmable Interrupt Controller (APIC)” on<br>page 627, Chapter 17, ”OS-Visible Workaround Information” on page 515,<br>Chapter 17, ”Hardware Performance Monitoring and Control” on page 664.<br>Added Table A-7, “Secure Virtual Machine MSR Cross-Reference,” on page 733.<br>Added minor clarifications and corrected typographical and formatting errors. |
| September<br>2006 | 3.12 | Added numerous minor clarifications. |
| December<br>2005 | 3.11 | Added Chapter 15, Secure Virtual Machine. Incorporated numerous factual<br>corrections and updates. |
| February<br>2005 | 3.10 | Corrected Table 8-6, “General-Protection Exception Conditions,” on page 255.<br>Added SSE3 information. Clarified and corrected information on the CPUID<br>instruction and feature identification. Added information on the RDTSCP<br>instruction. Clarified information about MTRRs and PATs in multiprocessing<br>systems. |
| September<br>2003 | 3.09 | Corrected numerous minor typographical errors. |
| April 2003 | 3.08 | Clarified terms in section on FXSAVE/FXSTOR. Corrected several minor errors<br>of omission. Documentation of CR0.NW bit has been corrected. Several<br>register diagrams and figure labels have been corrected. Description of shared<br>cache lines has been clarified in 7.3, ”Memory Coherency and Protocol” on<br>page 192. |
| September<br>2002 | 3.07 | Made numerous small grammatical changes and factual clarifications. Added<br>Revision History. |

<details>
<summary>Rendered source page 47 (figures/tables)</summary>

![Rendered source PDF page 47](../assets/pages/pdf-page-0047.webp)

</details>
