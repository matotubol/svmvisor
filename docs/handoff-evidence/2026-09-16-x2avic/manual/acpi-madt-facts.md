# ACPI MADT fact sheet: x2APIC guest (Windows) and captured machine tables

Manual review date: 2026-09-16. This was a read-only review: no source or docs were edited, no git
commands were run, and no firmware-table or hardware interface was called. The specification was
read only from rendered page images (`pages/ACPI_Spec_6.6.pNNNN*.png`). Text extraction was used
only to find pages. Quotes are verbatim from the images, except that curly quotes and apostrophes
are written as ASCII.

Artifacts in this folder:
- `pages/`: rendered PNGs (144 dpi, plus `.top`/`.bottom` halves at 230 dpi).
- `decode_madt.py`: read-only decoder. It verifies the signature, the length (header length must
  equal the file size), the checksum, the 6.6 fixed structure lengths, and that the structure walk
  ends exactly at the end of the table.
- `madt-decoded.json` and `madt-decoded.txt`: the decoder output.

Decoder command, run from this folder:
`python decode_madt.py ../../x2avic-2026-09-16/acpi/APIC.bin --fadt ../../x2avic-2026-09-16/acpi/FACP.bin --manifest ../../x2avic-2026-09-16/acpi/manifest.json --ivrs-admission ../../x2avic-2026-09-16/acpi/ivrs-admission.json --json madt-decoded.json`
(the actual run used absolute paths). Self-test, done in the session scratchpad only:
- synthetic type 9, 0xA, 3 and 5 entries decoded correctly;
- a bad checksum and an out-of-bounds structure length were rejected;
- a type 4 NMI entry combined with an ID >= 255 was flagged.

## Source table

| Item | Value |
| --- | --- |
| Document | Advanced Configuration and Power Interface (ACPI) Specification |
| Release | 6.6 (the page header on every rendered page reads "... Specification, Release 6.6") |
| File | `C:\Users\mato\Documents\svmvisor\docs\ACPI_Spec_6.6.pdf`, 1202 PDF pages |
| SHA256 (printed by render_pages.py on every run) | `8c7542dd4de974ae47bba71bb0336637fe1e3838daad7692370ab4cf218efd35`, which matches the expected hash |
| Page numbering | Zero-based index = PDF page - 1. On every body page cited here, printed footer = PDF page - 71 (for example PDF 204 = printed 133). Every footer was checked on the image. Front matter uses roman numerals (PDF 5 = printed "iv"). |
| Applicability | Normative for ACPI table layouts and for what OSPM expects from firmware tables. It does not define local APIC register or LVT bit encodings, x2APIC MSR numbers, IOAPIC register behavior, or IOMMU rules. The machine's MADT reports revision 6 (6.6 lists 7) and its FADT reports version 6.5, so the platform does not claim ACPI 6.6 conformance. The structures actually present (types 0, 1, 2, 4) have exactly the fixed lengths that 6.6 gives. |

### Rule citation index

Every row cites ACPI 6.6, SHA256 8c7542dd...efd35. "PDF/idx/printed" means one-based PDF page /
zero-based index / printed footer page.

| Rule | Location (section / table) | PDF / idx / printed |
| --- | --- | --- |
| R1 | 5.2.12 intro: interrupt models, no mixing, GSI mapping, physical addresses, Device()/ACPI0007 | 204 / 203 / 133 |
| R2 | Table 5.19 MADT format | 204-205 / 203-204 / 133-134 |
| R3 | Table 5.20 Multiple APIC Flags (PCAT_COMPAT) | 205 / 204 / 134 |
| R4 | Structure type/length prefix sentence | 205 / 204 / 134 |
| R5 | Table 5.21 Interrupt Controller Structure Types, with notes (a) and (b) | 205-206 / 204-205 / 134-135 |
| R6 | 5.2.12.1 entry order | 206-207 / 205-206 / 135-136 |
| R7 | 5.2.12.2 and Tables 5.22/5.23: Local APIC and its flags | 207 / 206 / 136 |
| R8 | 5.2.12.3 and Table 5.24: I/O APIC | 208 / 207 / 137 |
| R9 | 5.2.12.4 APIC plus dual 8259 | 208 / 207 / 137 |
| R10 | 5.2.12.5 ISO text and Table 5.25 | 208-209 / 207-208 / 137-138 |
| R11 | Table 5.26 MPS INTI flags, plus the MPS 1.4 note and the SCI_INT bullet | 209 / 208 / 138 |
| R12 | 5.2.12.6 and Table 5.27: NMI Source | 210 / 209 / 139 |
| R13 | 5.2.12.7 and Table 5.28: Local APIC NMI | 210 / 209 / 139 |
| R14 | 5.2.12.8 text and Table 5.29: LAPIC Address Override | 210-211 / 209-210 / 139-140 |
| R15 | 5.2.12.12 x2APIC text and compatibility note | 213 / 212 / 142 |
| R16 | Table 5.34 Processor Local x2APIC | 214 / 213 / 143 |
| R17 | 5.2.12.13 x2APIC NMI rules and Table 5.35 | 214 / 213 / 143 |
| R18 | Remainder of the MADT section (GIC, MP Wakeup 0x10, LoongArch, RISC-V), each page viewed | 215-231 / 214-230 / 144-160 |
| R19 | 5.2.13 Global System Interrupts, Fig. 5.3 and Fig. 5.4 | 231-233 / 230-232 / 160-162 |
| R20 | Table 5.9: FADT header, version, SCI_INT | 182-183 / 181-182 / 111-112 |
| R21 | Table 5.9: IAPC_BOOT_ARCH @109, Flags @112 | 187 / 186 / 116 |
| R22 | Table 5.10: FADT flags (title; bits 0-4; bits 18, 19, 20, 21) | 190-191, 193-194 / 189-190, 192-193 / 119-120, 122-123 |
| R23 | 6.2.7 _GSB (I/O APIC device, _MAT interplay) | 454 / 453 / 383 |
| R24 | 6.2.11 _MAT | 470-471 / 469-470 / 399-400 |
| R25 | 8.4 Declaring Processors | 594 / 593 / 523 |
| R26 | Table 5.220 \_PR note | 364 / 363 / 293 |
| R27 | 19.2 grammar: ProcessorTerm | 982 / 981 / 911 |
| R28 | 19.6.108-19.6.110: no "Processor (Declare Processor)" section | 1096-1097 / 1095-1096 / 1025-1026 |
| R29 | Table 5.5 "APIC" -> 5.2.12; Table 5.6 "DMAR", "IVRS" | 177-179 / 176-178 / 106-108 |

## 1. MADT header, Flags (PCAT_COMPAT), and the structure type list

**Header (R2, Table 5.19).** Each row is offset / length: field.
- 0/4: Signature, "'APIC' Signature for the Multiple APIC Description Table."
- 4/4: Length, "Length, in bytes, of the entire MADT."
- 8/1: Revision, "7".
- 9/1: Checksum, "Entire table must sum to zero."
- 10/6: OEMID.
- 16/8: OEM Table ID.
- 24/4: OEM Revision.
- 28/4: Creator ID.
- 32/4: Creator Revision.
- 36/4: Local Interrupt Controller Address, "The 32-bit physical address at which each processor can
  access its local interrupt controller."
- 40/4: Flags, "Multiple APIC flags. See Multiple APIC Flags for a description of this field."
- 44/variable: Interrupt Controller Structure[n], "A list of interrupt controller structures for
  this implementation. This list will contain all of the structures from Interrupt Controller
  Structure Types needed to support this platform."

**Flags (R3, Table 5.20).**
- PCAT_COMPAT, bit 0 (1 bit): "A one indicates that the system also has a PC-AT-compatible
  dual-8259 setup. The 8259 vectors must be disabled (that is, masked) when enabling the ACPI APIC
  operation."
- Reserved, bits 1-31 (31 bits): "This value is zero."

**Structure framing (R4).** "The first byte of each structure declares the type of that structure
and the second byte declares the length of that structure."

**Type list (R5, Table 5.21).** "MAT-CPU" and "MAT-IOAPIC" are the table's "_MAT for Processor
object (a)" and "_MAT for an I/O APIC object (b)" columns.

| Value | Description | MAT-CPU | MAT-IOAPIC | Reference |
| --- | --- | --- | --- | --- |
| 0 | Processor Local APIC | yes | no | 5.2.12.2 |
| 1 | I/O APIC | no | yes | 5.2.12.3 |
| 2 | Interrupt Source Override | no | yes | 5.2.12.5 |
| 3 | Non-maskable Interrupt (NMI) Source | no | yes | 5.2.12.6 |
| 4 | Local APIC NMI | yes | no | 5.2.12.7 |
| 5 | Local APIC Address Override | no | no | 5.2.12.8 |
| 6 | I/O SAPIC | no | yes | 5.2.12.9 |
| 7 | Local SAPIC | yes | no | 5.2.12.10 |
| 8 | Platform Interrupt Sources | no | yes | 5.2.12.11 |
| 9 | Processor Local x2APIC | yes | no | 5.2.12.12 |
| 0xA | Local x2APIC NMI | yes | no | 5.2.12.13 |
| 0xB | GIC CPU Interface (GICC) | yes | no | 5.2.12.14 |
| 0xC | GIC Distributor (GICD) | no | no | 5.2.12.15 |
| 0xD | GIC MSI Frame | no | no | 5.2.12.16 |
| 0xE | GIC Redistributor (GICR) | no | no | 5.2.12.17 |
| 0xF | GIC Interrupt Translation Service (ITS) | no | no | 5.2.12.18 |
| 0x10 | Multiprocessor Wakeup | no | no | 5.2.12.19 |
| 0x11 | Core Programmable Interrupt Controller (CORE PIC) | no | no | 5.2.12.20 |
| 0x12 | Legacy I/O Programmable Interrupt Controller (LIO PIC) | no | no | 5.2.12.21 |
| 0x13 | HyperTransport Programmable Interrupt Controller (HT PIC) | no | no | 5.2.12.22 |
| 0x14 | Extend I/O Programmable Interrupt Controller (EIO PIC) | no | no | 5.2.12.23 |
| 0x15 | MSI Programmable Interrupt Controller (MSI PIC) | no | no | 5.2.12.24 |
| 0x16 | Bridge I/O Programmable Interrupt Controller (BIO PIC) | no | no | 5.2.12.25 |
| 0x17 | Low Pin Count Programmable Interrupt Controller (LPC PIC) | no | no | 5.2.12.26 |
| 0x18 | RISC-V Hart Local Interrupt Controller (RINTC) | yes | no | 5.2.12.27 |
| 0x19 | RISC-V Incoming MSI Controller (IMSIC) | no | no | 5.2.12.28 |
| 0x1A | RISC-V Advanced Platform Level Interrupt Controller (APLIC) | no | no | 5.2.12.29 |
| 0x1B | RISC-V Platform Level Interrupt Controller (PLIC) | no | no | 5.2.12.30 |
| 0x1C-0x7F | "Reserved for OEM use" (as printed; the table has no 0x80-0xFF row) | no | no | - |

Notes to Table 5.21:
- (a) "When _MAT (see Section 6.2.11 ) appears under a Processor Device object (see Section 8.4 ),
  OSPM processes the Interrupt Controller Structures returned by _MAT with the types labeled "yes"
  and ignores other types."
- (b) "When _MAT appears under an I/O APIC Device, OSPM processes the Interrupt Controller
  Structures returned by _MAT with the types labeled "yes" and ignores other types."

**Model rules (R1).**
- "The interrupt model cannot be dynamically changed by system firmware; OSPM will choose which
  model to use and install support for that model at the time of installation. If a platform
  supports multiple models, an OS will install support for only one of the models and will not mix
  models."
- "All addresses in the MADT are processor-relative physical addresses."
- "From ACPI Specification 6.3 onward, all processor objects for all architectures except Itanium
  must now use Device() objects with an _HID of ACPI0007, and use only integer _UID values."

**Order rules (R6, 5.2.12.1).**
- "OSPM should initialize processors in the order that they appear in the MADT."
- "platform firmware should list the boot processor as the first processor entry in the MADT."
- "platform firmware should list the first logical processor of each of the individual
  multi-threaded processors in the MADT before listing any of the second logical processors. This
  approach should be used for all successive logical processors."

**Implementation implication.**
- The machine sets PCAT_COMPAT=1, so the platform has dual 8259s. ACPI requires them to be masked
  when APIC operation is enabled.
- A virtual x2APIC trap handler should therefore not expect 8259 traffic once Windows is in APIC
  mode. ACPI gives no LINT0 wiring for that traffic (see Q3 and UNRESOLVED #4).
- Any hypervisor code that walks the MADT must use the type/length framing, skip types it does not
  model, and keep table order as the processor initialization order.

## 2. Processor Local APIC (type 0) vs Processor Local x2APIC (type 9)

**Type 0 (R7, Table 5.22).** Each row is offset / length: field.
- 0/1: Type = 0.
- 1/1: Length = 8.
- 2/1: ACPI Processor UID. "The OS associates this Local APIC Structure with a processor object in
  the namespace when the _UID child object of the processor's device object (or the ProcessorId
  listed in the Processor declaration operator) evaluates to a numeric value that matches the
  numeric value in this field. Note that the use of the Processor declaration operator is
  deprecated."
- 3/1: APIC ID, "The processor's local APIC ID."
- 4/4: Flags, "Local APIC flags. See the following table (Table 5.23)".

**Type 9 (R16, Table 5.34).**
- 0/1: Type = 9.
- 1/1: Length = 16.
- 2/2: Reserved, "Reserved - Must be zero".
- 4/4: X2APIC ID, "The processor's local x2APIC ID."
- 8/4: Flags, "Same as Local APIC flags. See Local APIC Flags for a description of this field."
- 12/4: ACPI Processor UID. "OSPM associates the X2APIC Structure with a processor object declared
  in the namespace using the Device statement, when the _UID child object of the processor device
  evaluates to a numeric value, by matching the numeric value with this field."

**Flags (R7, Table 5.23; type 9 uses the same flags).**
- Enabled, bit 0: "If this bit is set the processor is ready for use. If this bit is clear and the
  Online Capable bit is set, system hardware supports enabling this processor during OS runtime. If
  this bit is clear and the Online Capable bit is also clear, this processor is unusable, and OSPM
  shall ignore the contents of the Processor Local APIC Structure."
- Online Capable, bit 1: "The information conveyed by this bit depends on the value of the Enabled
  bit. If the Enabled bit is set, this bit is reserved and must be zero. Otherwise, if this this bit
  is set, system hardware supports enabling this processor during OS runtime." ("this this" is as
  printed.)
- Reserved, bits 2-31 (30 bits): "Must be zero."

**Rules, exact wording.**
- R7 (type 0): "When using the APIC interrupt model, each processor in the system is required to
  have a Processor Local APIC record in the MADT, and a processor device object in the DSDT. OSPM
  does not expect the information provided in this table to be updated if the processor
  information changes during the lifespan of an OS boot. While in the sleeping state, processors
  are not allowed to be added, removed, nor can their APIC ID or Flags change. When a processor is
  not present, the Processor Local APIC information is either not reported or flagged as
  disabled."
- R15 (type 9): "The Processor X2APIC structure is very similar to the processor local APIC
  structure. When using the X2APIC interrupt model, logical processors are required to have a
  processor device object in the DSDT and must convey the processor's APIC information to OSPM
  using the Processor Local X2APIC structure."
- R15 (compatibility note): "[Compatibility note] On some legacy OSes, Logical processors with APIC
  ID values less than 255 (whether in XAPIC or X2APIC mode) must use the Processor Local APIC
  structure to convey their APIC information to OSPM, and those processors must be declared in the
  DSDT using the Processor() keyword. Logical processors with APIC ID values 255 and greater must
  use the Processor Local x2APIC structure and be declared using the Device() keyword."
- R15: "OSPM does not expect the information provided in this table to be updated if the processor
  information changes during the lifespan of an OS boot. While in the sleeping state, logical
  processors must not be added or removed, nor can their X2APIC ID or x2APIC Flags change. When a
  logical processor is not present, the processor local X2APIC information is either not reported
  or flagged as disabled."
- R25 (8.4): "When the platform uses the APIC interrupt model, UID object values under a processor
  device are used to associate processor devices with entries in the MADT."
- R26 (\_PR): "Platforms may maintain the \_PR namespace for compatibility with ACPI 1.0 operating
  systems, but it is otherwise deprecated. see the compatibility note in Processor Local x2APIC
  Structure."
- R27: the grammar gives `Processor (ProcessorName, ProcessorID // ByteConstExpr, ...)`, so a
  Processor() ID is one byte.

**Reading.**
- The only explicit numeric trigger in the spec is the APIC ID value. IDs of 255 and above must use
  type 9 (and Device()).
- The note allows type 0 for IDs below 255 "whether in XAPIC or X2APIC mode".
- The MADT section does not define "X2APIC interrupt model" separately; the R1 model list names
  only APIC among the Intel-style models.
- No sentence in the MADT section says that x2APIC mode (APIC_BASE EXTD) by itself obliges firmware
  to emit type 9 entries for IDs below 255.
- Whether a particular Windows build insists on type 9 in x2APIC mode is OS behavior, not ACPI
  (UNRESOLVED #1).

**Implementation implication.**
- For this machine (enabled APIC IDs 0-11 and 16-27, all below 255), the firmware's type 0 entries
  are sanctioned by the spec even when Windows runs the x2APIC interface. Nothing in ACPI requires
  the hypervisor to synthesize type 9 entries. Grep of `crates/` found no MADT rewriting code, so
  Windows consumes the firmware MADT as-is.
- The guest-visible x2APIC ID of each vCPU must equal the MADT APIC ID of the enabled UID it
  represents. The x2APIC ID register itself is defined outside ACPI.
- Do not assume UID == APIC ID. Here UIDs 12-23 map to APIC IDs 16-27.
- Filter on the flags before checking for duplicate IDs. The 8 entries with UID 24-31, APIC ID 0
  and flags 0 are "unusable" and OSPM "shall ignore" them.
- A design that exposes guest APIC IDs of 255 or more must present type 9 entries (Device()/_UID
  association). It must then also move all LINT NMI descriptions to type 0xA (Q3).

## 3. Local APIC NMI (type 4) and Local x2APIC NMI (type 0xA)

**Type 4 (R13, Table 5.28).**
- 0/1: Type = 4.
- 1/1: Length = 6.
- 2/1: ACPI Processor UID. "Value corresponding to the _UID listed in the processor's device
  object, or the Processor ID corresponding to the ID listed in the processor object. A value of
  0xFF signifies that this applies to all processors in the machine."
- 3/2: Flags, "MPS INTI flags. See Table 5.26 for a description of this field."
- 5/1: Local APIC LINT#, "Local APIC interrupt input LINTn to which NMI is connected."

**Type 0xA (R17, Table 5.35).**
- 0/1: Type = 0A.
- 1/1: Length = 12.
- 2/2: Flags, "Same as MPS INTI flags. See MPS INTI Flags For a description of this field."
- 4/4: ACPI Processor UID. "UID corresponding to the ID listed in the processor Device object. A
  value of 0xFFFFFFFF signifies that this applies to all processors in the machine."
- 8/1: Local x2APIC LINT#, "Local x2APIC interrupt input LINTn to which NMI is connected."
- 9/3: Reserved, "Reserved - Must be zero."

**Flags (R11, Table 5.26).** "The MPS INTI flags listed in Table 5.26 are identical to the flags
used in the MPS version 1.4 specification, Table 4-10. The Polarity flags are the PO bits and the
Trigger Mode flags are the EL bits."
- Polarity, bits 1:0: "Polarity of the APIC I/O input signals":
  - 00 Conforms to the specifications of the bus (for example, EISA is active-low for
    level-triggered interrupts)
  - 01 Active high
  - 10 Reserved
  - 11 Active low
- Trigger Mode, bits 3:2: "Trigger mode of the APIC I/O Input signals":
  - 00 Conforms to specifications of the bus (For example, ISA is edge-triggered)
  - 01 Edge-triggered
  - 10 Reserved
  - 11 Level-triggered
- Reserved, bits 15:4 (12 bits): "Must be zero."

**Rules, exact wording.**
- R13: "This structure describes the Local APIC interrupt input (LINTn) that NMI is connected to for
  each of the processors in the system where such a connection exists. This information is needed
  by OSPM to enable the appropriate local APIC entry."
- R13: "Each Local APIC NMI connection requires a separate Local APIC NMI structure. For example, if
  the platform has 4 processors with ID 0-3 and NMI is connected LINT1 for processor 3 and 2, two
  Local APIC NMI entries would be needed in the MADT."
- R17: "The Local APIC NMI and Local x2APIC NMI structures describe the interrupt input (LINTn) that
  NMI is connected to for each of the logical processors in the system where such a connection
  exists. Each NMI connection to a processor requires a separate NMI structure. This information is
  needed by OSPM to enable the appropriate APIC entry."
- R17: "NMI connection to a logical processor with local x2APIC ID 255 and greater requires an
  X2APIC NMI structure. NMI connection to a logical processor with an x2APIC ID less than 255
  requires a Local APIC NMI structure. For example, if the platform contains 8 logical processors
  with x2APIC IDs 0-3 and 256-259 and NMI is connected LINT1 for processor 3, 2, 256 and 257 then
  two Local APIC NMI entries and two X2APIC NMI entries must be provided in the MADT."
- R17: "The Local APIC NMI structure is used to specify global LINTx for all processors if all
  logical processors have x2APIC ID less than 255. If the platform contains any logical processors
  with an x2APIC ID of 255 or greater then the Local X2APIC NMI structure must be used to specify
  global LINTx for ALL logical processors."

**What OSPM is expected to program.** ACPI says only that OSPM uses this information "to enable the
appropriate (local) APIC entry" on the named LINT input, with the given polarity and trigger. The
LVT register layout (delivery-mode encoding, polarity and trigger bits, mask) and the x2APIC MSR
numbers for LINT0/LINT1 are not in ACPI.

**Implementation implication (key).**
- The machine has exactly one NMI structure: type 4, UID 0xFF (all processors), LINT1, flags 0x0005
  (active-high, edge). No type 0xA and no LINT0 NMI entry exist.
- Windows is therefore expected to program LVT LINT1 as an NMI entry on every processor, the BSP
  and all APs, with active-high polarity and edge trigger.
- The virtual x2APIC trap handler should accept that write as the platform-conformant
  configuration: an unmasked LINT1 with NMI delivery. It should keep the value for readback and not
  rewrite polarity or trigger.
- The handler should treat that write as the guest arming NMI delivery from the platform's LINT1
  NMI wire. How host NMIs on LINT1 are forwarded is a hypervisor policy outside ACPI.
- Type 4 with 0xFF is the correct and sufficient form here because every ID is below 255 (R17).
- If vCPU IDs of 255 or more are ever exposed, the global LINT description must move entirely to
  type 0xA with UID 0xFFFFFFFF.
- LINT0 carries no ACPI NMI description. Its LVT programming (masked or ExtINT) has no
  ACPI-described platform meaning on this machine; see UNRESOLVED #3 and #4 for the register and
  wiring sources.

## 4. I/O APIC (1), Interrupt Source Override (2), NMI Source (3), Local APIC Address Override (5)

**I/O APIC, type 1 (R8, Table 5.24).**
- 0/1: Type = 1.
- 1/1: Length = 12.
- 2/1: I/O APIC ID, "The I/O APIC's ID."
- 3/1: Reserved, 0.
- 4/4: I/O APIC Address, "The 32-bit physical address to access this I/O APIC. Each I/O APIC
  resides at a unique address."
- 8/4: Global System Interrupt Base, "The Global System Interrupt number where this I/O APIC's
  interrupt inputs start. The number of interrupt inputs is determined by the I/O APIC's Max Redir
  Entry register."
- Text: "There is one I/O APIC structure for each I/O APIC in the system."

**GSI mapping (R19, 5.2.13).** "OSPM determines the mapping of the Global System Interrupts by
determining how many interrupt inputs each I/O APIC supports and by determining the Global System
Interrupt base for each I/O APIC as specified by the I/O APIC Structure. OSPM determines the number
of interrupt inputs by reading the Max Redirection register from the I/O APIC. The Global System
Interrupts mapped to that I/O APIC begin at the Global System Interrupt base and extending through
the number of interrupts specified in the Max Redirection register." Fig. 5.3 shows an example:
bases 0, 24 and 40 for IOAPICs with 24, 16 and 24 inputs.

**APIC plus 8259 (R9).**
- "Systems that support both APIC and dual 8259 interrupt models must map Global System Interrupts
  0-15 to the 8259 IRQs 0-15, except where Interrupt Source Overrides are provided ... I/O APIC
  interrupt inputs 0-15 must be mapped to Global System Interrupts 0-15 and have identical sources
  as the 8259 IRQs 0-15 unless overrides are used."
- "If OSPM implements APIC support, it will enable the APIC as described by the APIC specification
  and will use all reported Global System Interrupts that fall within the limits of the interrupt
  inputs defined by the I/O APIC structures."

**Interrupt Source Override, type 2 (R10, Table 5.25).**
- 0/1: Type = 2.
- 1/1: Length = 10.
- 2/1: Bus, "0 Constant, meaning ISA".
- 3/1: Source, "Bus-relative interrupt source (IRQ)".
- 4/4: Global System Interrupt, "The Global System Interrupt that this bus-relative interrupt
  source will signal."
- 8/2: Flags, MPS INTI (Table 5.26).
- Text: "It is assumed that the ISA interrupts will be identity-mapped into the first I/O APIC
  sources. ... Only those that are not identity-mapped onto the APIC interrupt inputs need be
  described."
- Text: "This specification only supports overriding ISA interrupt sources."
- R11: "Interrupt Source Overrides are also necessary when an identity mapped interrupt input has a
  non-standard polarity."
- R11: "You must have an interrupt source override entry for the IRQ mapped to the SCI interrupt if
  this IRQ is not identity mapped. This entry will override the value in SCI_INT in FADT."
- R20, FADT SCI_INT at 46/2: "System vector the SCI interrupt is wired to in 8259 mode. On systems
  that do not contain the 8259, this field contains the Global System Interrupt number of the SCI
  interrupt. OSPM is required to treat the ACPI SCI interrupt as a shareable, level, active low
  interrupt."

**NMI Source, type 3 (R12, Table 5.27).**
- 0/1: Type = 3.
- 1/1: Length = 8.
- 2/2: Flags, "Same as MPS INTI flags".
- 4/4: Global System Interrupt, "The Global System Interrupt that this NMI will signal."
- Text: "This structure allows a platform designer to specify which I/O (S)APIC interrupt inputs
  should be enabled as non-maskable. Any source that is non-maskable will not be available for use
  by devices."

**Local APIC Address Override, type 5 (R14, Table 5.29).**
- 0/1: Type = 5.
- 1/1: Length = 12.
- 2/2: Reserved, "must be set to zero".
- 4/8: Local APIC Address, "Physical address of Local APIC."
- Text: "If defined, OSPM must use the address specified in this structure for all local APICs (and
  local SAPICs), rather than the address contained in the MADT's table header. Only one Local APIC
  Address Override Structure may be defined."

**Related (R23, R24).**
- _GSB: "Any I/O APIC device that either supports hot-plug or is not described in the MADT must
  contain a _GSB object."
- _MAT: "Specific types of MADT entries are meaningful to (in other words, processed by) OSPM when
  returned via the evaluation of this object as described in Table 5.21. Other entry types returned
  by the evaluation of _MAT are ignored by OSPM."

**Implementation implication.**
- A route validator maps GSI g to IOAPIC 0x20 pin g when g is in [0, pin count of 0x20), and to
  IOAPIC 0x21 pin g-24 when g >= 24. The pin counts are not in the MADT (UNRESOLVED #6).
- Expect PIT IRQ0 on GSI 2 with bus-conformant (ISA) flags.
- Expect the SCI on GSI 9 as active-low, level. The ISO and FADT agree: SCI_INT=9 and ISO flags
  0x000F.
- There are no type 3 entries, so no IOAPIC pin is reserved as NMI.
- There is no type 5 entry, so the local APIC address is the header value 0xFEE00000. ACPI does not
  say whether that MMIO page matters in x2APIC mode; that is an APM question.

## 5. x2APIC/IOAPIC destination limits, interrupt remapping, IVRS/DMAR

**Finding.** Every page of the MADT section was viewed (PDF 204-231, printed 133-160). It says
nothing about x2APIC or IOAPIC destination-field widths, interrupt remapping, IOMMU, IVRS or DMAR.
Location-only text search of PDF 204-236 also found no hits for IVRS, DMAR, remap or IOMMU. The
only x2APIC thresholds in the section are the ID >= 255 rules (R15, R17).

**Outside the MADT section, recorded for the chain only.**
- R29: DMAR ("DMA Remapping Table") and IVRS ("I/O Virtualization Reporting Structure") appear only
  as reserved signatures that point to "Links to ACPI-Related Documents".
- R22, FADT flag bit 18, FORCE_APIC_CLUSTER_MODEL: "A one indicates that all local APICs must be
  configured for the cluster destination model when delivering interrupts in logical mode. ... This
  bit is intended for xAPIC based machines that require the cluster destination model even when 8
  or fewer local APICs are present in the machine."
- R22, FADT flag bit 19, FORCE_APIC_PHYSICAL_DESTINATION_MODE: "A one indicates that all local
  xAPICs must be configured for physical destination mode. If this bit is set, interrupt delivery
  operation in logical destination mode is undefined. On machines that contain fewer than 8 local
  xAPICs or that do not use the xAPIC architecture, this bit is ignored."
- The machine has both bits clear.

**Implementation implication.**
- ACPI gives no basis for choosing 8-bit or 32-bit destinations, or for any remapping dependency.
  Those rules must come from the IOAPIC, IOMMU and APM sources; the project's IOMMU review records
  IVRS XTSup=0 for this capture.
- All enabled IDs are at most 27, which fits in 8 bits. That is a numeric observation, not an ACPI
  rule.
- The FADT does not force cluster or physical destination mode, so the virtual x2APIC must support
  whichever logical or physical destination addressing Windows selects.

## Decoded machine tables (captured, read-only)

### Provenance

| Item | Value |
| --- | --- |
| MADT file | `C:\Users\mato\Documents\svmvisor\work\x2avic-2026-09-16\acpi\APIC.bin`, 350 bytes, SHA256 `98f50a329875e2f16d91b3b50884137105623180f08404973b491dce720df6d5` (equals the manifest value) |
| FADT file | `...\acpi\FACP.bin`, 276 bytes, SHA256 `93e831afbc828851b4cbdb32c59640693ebf8b3c67ced7e8e0acf9b9b6b187fe` (equals the manifest value) |
| Manifest | `...\acpi\manifest.json`: captured_utc `2026-09-16T02:23:06.1230651Z`; source "Windows GetSystemFirmwareTable API; read-only"; board Gigabyte Technology Co., Ltd. B850 AORUS ELITE WIFI7 (Version "x.x"). File mtime 04:23:06 +0200. |
| Capture script | `...\x2avic-2026-09-16\read-acpi.ps1` (read, not run): EnumSystemFirmwareTables/GetSystemFirmwareTable with provider 'ACPI'; saves only IVRS, APIC, MCFG and FACP |
| CPU and BIOS | The manifest records neither the CPU nor the BIOS version. `live-cpuid.json` (02:10:35Z) names "AMD Ryzen 9 9900X 12-Core Processor", unpinned current CPU, CPUID.1 ECX=7ED8320B and EBX=01180800. BIOS F7 comes from the task statement and is not recorded in the capture files. |

**Timeline limitation.**
- 02:10:35Z: the CPUID probe shows x2APIC (bit 21) clear. `docs/x2avic-hardware-review-2026-09-16.md`
  line 14 says so.
- 02:23:06Z: the tables are captured.
- 04:32 +0200 (02:32Z): `docs/handoff-2026-09-16.md` says "The user has not checked the BIOS x2APIC
  setting."
- 04:56 +0200 (02:56Z): `docs/x2avic-iommu-implementation-2026-09-16.md` says "The user reports
  enabling BIOS x2APIC for the next reboot; no new CPU/IVRS/live-register observation has been
  made."

This MADT therefore predates the BIOS x2APIC change. Firmware after the change may emit different
structure types (for example type 9 or 0xA), IDs or flags. No later capture exists.

### Header

| Field | Value |
| --- | --- |
| Signature / Length | "APIC" / 350; the structure walk ends exactly at 350 |
| Revision | 6 (ACPI 6.6 Table 5.19 lists 7) |
| Checksum | byte 0xEA; the table sums to 0 (valid) |
| OEMID / OEM Table ID / OEM Revision | "ALASKA" / "A M I " / 0x01072009 |
| Creator ID / Revision | "AMI " / 0x00010013 |
| Local Interrupt Controller Address | 0xFEE00000 |
| Flags | 0x00000001: PCAT_COMPAT=1, reserved bits 0 |

### Entries (37 structures, all at the 6.6 fixed lengths, no decoder problems)

**Type 0 entries 0-11 (offsets 44-132), all Enabled.** Primary threads, one per core, BSP-candidate
APIC ID 0 first. Each entry is UID -> APIC ID, flags 0x00000001:
- UID 0 -> 0x00
- UID 2 -> 0x02
- UID 4 -> 0x04
- UID 6 -> 0x06
- UID 8 -> 0x08
- UID 10 -> 0x0A
- UID 12 -> 0x10
- UID 14 -> 0x12
- UID 16 -> 0x14
- UID 18 -> 0x16
- UID 20 -> 0x18
- UID 22 -> 0x1A

**Type 0 entries 12-23 (offsets 140-228), all Enabled.** Second threads, same flags:
- UID 1 -> 0x01
- UID 3 -> 0x03
- UID 5 -> 0x05
- UID 7 -> 0x07
- UID 9 -> 0x09
- UID 11 -> 0x0B
- UID 13 -> 0x11
- UID 15 -> 0x13
- UID 17 -> 0x15
- UID 19 -> 0x17
- UID 21 -> 0x19
- UID 23 -> 0x1B

**Type 0 entries 24-31 (offsets 236-292).** UIDs 24-31, each with APIC ID 0x00 and flags
0x00000000: Enabled=0, Online Capable=0, so they are "unusable" and OSPM "shall ignore" them.

**Remaining structures.**

| # | Offset | Type | Decoded |
| --- | --- | --- | --- |
| 32 | 300 | 4 Local APIC NMI | UID 0xFF (all processors), LINT1, flags 0x0005: polarity 01 active-high, trigger 01 edge |
| 33 | 306 | 1 I/O APIC | ID 0x20, address 0xFEC00000, GSI base 0 |
| 34 | 318 | 1 I/O APIC | ID 0x21, address 0xFEC01000, GSI base 24 |
| 35 | 330 | 2 ISO | bus 0 (ISA), IRQ 0 -> GSI 2, flags 0x0000 (bus-conformant polarity and trigger) |
| 36 | 340 | 2 ISO | bus 0, IRQ 9 -> GSI 9, flags 0x000F: active-low, level (identity-mapped, non-standard polarity) |

**Summary.**
- CPU entry types present: type 0 only. There are no type 9 and no type 0xA entries.
- 24 CPUs are enabled, 0 are online-capable, and 8 entries are unusable.
- Enabled APIC IDs: {0-11, 16-27}; the maximum is 27. No ID is >= 255, and there are no duplicate
  enabled IDs or UIDs.
- There are no type 3, type 5, or other types.
- APIC ID 0 is the first processor entry. The MADT alone cannot prove which processor is the BSP.
- The order (all primary threads, then all second threads) matches the 5.2.12.1 guideline.
- In the unpinned CPUID probe, CPUID.1 EBX=01180800. Its byte EBX[31:24] = 0x01 is by CPUID
  convention the initial local APIC ID; that reading is not an ACPI rule and is not cited here.
  The value 0x01 is in the enabled set.

### FADT fields followed from the MADT cross-reference (FACP.bin)

| Field | Value |
| --- | --- |
| Checksum / length | valid / 276 |
| Version (major at offset 8 . minor at offset 131) | 6.5 |
| SCI_INT (46) | 9, which agrees with ISO IRQ9 -> GSI9, active-low, level |
| IAPC_BOOT_ARCH (109) | 0x0000 (bits not interpreted here) |
| Flags (112) | 0x0003C5A5: bit18 FORCE_APIC_CLUSTER_MODEL=0, bit19 FORCE_APIC_PHYSICAL_DESTINATION_MODE=0, bit20 HW_REDUCED_ACPI=0, bit21 LOW_POWER_S0_IDLE_CAPABLE=0 |
| DSDT (40) | 0x8DC50000. The DSDT was not captured. X_DSDT was not decoded because its offset was not rendered. |

### IVRS cross-check

This uses the existing `ivrs-admission.json`, whose source SHA256 equals the manifest's IVRS
value.
- IVRS special-device IOAPIC handles are 0x20 (requester 0x00A0) and 0x21 (requester 0x0001).
- These equal the MADT IOAPIC IDs {0x20, 0x21}.
- The meaning of an IVRS handle comes from the project's IOMMU review
  (`docs/x2avic-iommu-owner-2026-09-16.md`), not from ACPI.

### Files and places searched for MADT captures

- `work\x2avic-2026-09-16\`:
  - `acpi\APIC.bin`, `acpi\FACP.bin`, `acpi\IVRS.bin`, `acpi\MCFG.bin`
  - `acpi\manifest.json`, `acpi\ivrs-admission.json`
  - `read-acpi.ps1`, `live-cpuid.json`, `live-cpuid-80000008.json`, `dxe-check.log`
- Signature scan: every `*.bin`, `*.dat`, `*.aml` and `*.raw` under 2 MiB in the repo, excluding
  `docs/vendor`, `target` and `.git`. Only `work\x2avic-2026-09-16\acpi\APIC.bin` starts with
  "APIC".
- Name scan for `*apic*.bin`, `*madt*`, `*.dsl`, `*acpidump*`, `APIC*`, `DSDT*` and `SSDT*`: only
  Rust or assembly source files matched, and no table dumps.
- `work\x2avic-batch-2026-09-16\` appeared during this review. It holds build output only.
- Notes read:
  - `docs\handoff-2026-09-16.md`
  - `docs\x2avic-iommu-owner-2026-09-16.md`
  - `docs\x2avic-iommu-implementation-2026-09-16.md`
  - `docs\x2avic-rewrite-design-2026-09-16.md`
  - `docs\x2avic-interrupt-delivery-2026-09-16.md`, lines 225-260. It states that the archive's
    `native-platform-inventory.json` and os-inventory files contain no MADT. Those files were not
    found under `C:\Users\mato\Documents` (depth 6).
  - `docs\x2avic-hardware-review-2026-09-16.md`, grep only
  - `crates\dxe\README.md`, lines 135-160
- `crates\` grep for MADT handling: none. The only ACPI discovery code is IVRS/XSDT in
  `crates\dxe\src\native\resident\iommu.rs`.

## Cross-reference chains

1. Global System Interrupts:
   - 5.2.12 intro "See Global System Interrupts" (R1, PDF 204) -> 5.2.13 (R19, PDF 231-232).
   - That chain ends at the "Max Redirection register", which is IOAPIC hardware outside ACPI.
   - Separately, 5.2.12.3 -> 5.2.13, and 6.2.7 _GSB (PDF 454) -> 5.2.13.
2. Table 5.19 Flags -> "Multiple APIC Flags", Table 5.20 (PDF 205).
3. Structure list and _MAT:
   - Table 5.19 Structure[n] -> Table 5.21 (PDF 205-206) -> sections 5.2.12.2-5.2.12.30 (PDF
     207-231, all viewed).
   - Note (a) -> 6.2.11 _MAT (R24, PDF 470), which points back to Table 5.21.
   - Note (a) -> 8.4 Declaring Processors (R25, PDF 594) -> "Device (Declare Device Package)" (not
     followed).
   - Note (b) -> I/O APIC Device _MAT/_GSB (R23, PDF 454).
4. Interrupt Source Override (5.2.12.5):
   - Table 5.25 Flags -> Table 5.26 (PDF 209) -> "MPS version 1.4 specification, Table 4-10"
     (external, not available locally).
   - The SCI bullet -> FADT SCI_INT, Table 5.9 (R20, PDF 183).
5. 5.2.12.4 -> "Section 6" (resource configuration; not followed).
6. Type 4 UID and the Processor declaration:
   - Table 5.28 UID -> "compatibility note in Processor Local x2APIC Structure" (R15, PDF 213).
   - Table 5.28 UID -> "Processor (Declare Processor)". No such section exists in 6.6: 19.6.109
     Printf is followed directly by 19.6.110 QWordIO (R28, PDF 1096-1097). Only the grammar
     `ProcessorTerm` exists (R27, PDF 982).
   - The \_PR note (R26, PDF 364) points back to the same compatibility note.
   - Table 5.31 (PDF 212) uses the same references.
7. Table 5.22 Flags -> Table 5.23 (PDF 207). Table 5.34 Flags -> "Local APIC Flags", Table 5.23.
8. Table 5.35 Flags -> "MPS INTI Flags", Table 5.26 (PDF 209).
9. FADT flags:
   - Table 5.9 Flags @112 (R21, PDF 187) -> Table 5.10 (R22, PDF 190-194), bits 18 and 19.
   - Table 5.9 IAPC_BOOT_ARCH -> 5.2.9.3 (not followed).
10. Signatures:
    - Table 5.5 "APIC" -> 5.2.12 and "FACP" -> 5.2.9 (R29, PDF 177).
    - Table 5.6 "DMAR" and "IVRS" -> "Links to ACPI-Related Documents" (external, not followed).

## UNRESOLVED

1. **Type 0 vs type 9 on Windows.** ACPI allows type 0 for APIC IDs below 255 "whether in XAPIC or
   X2APIC mode". Whether the target Windows build accepts type 0 entries, or requires type 9, while
   running the x2APIC interface is OS behavior outside the spec. It needs OS-specific evidence or a
   boot test.
2. **MADT after the BIOS change.** The capture (02:23:06Z) predates the user's BIOS x2APIC change.
   The types, IDs, flags and NMI structure type the firmware emits now are unknown until a fresh
   read-only capture is taken after the reboot, together with fresh CPUID.
3. **LVT and MSR encodings are not in ACPI.** This covers the LVT LINT0/LINT1 bit encodings
   (delivery mode, polarity, trigger, mask), the x2APIC MSR numbers, and how NMI delivery mode
   interacts with the trigger bit. They must come from the AMD APM and PPR (sibling sheets
   `apm-x2apic`, `ppr-lapic`).
4. **LINT0 wiring.** The physical wiring of LINT0 on this board (for example, the 8259 INTR used as
   ExtINT) is not described by the MADT, which has no LINT0 entry. It cannot be determined from the
   captured tables.
5. **MPS 1.4 Table 4-10.** Table 5.26 refers to it, but it is not available locally and was not
   read.
6. **IOAPIC pin counts.** The Max Redirection Entry values for IOAPIC 0x20 and 0x21 are not in the
   MADT. So the exact GSI ranges are unknown: 0x20 is bounded only if its range ends before GSI 24,
   and 0x21's upper bound is unknown. This needs live IOAPIC register evidence.
7. **DSDT/SSDT not captured.** The processor Device (ACPI0007) or Processor() declarations and
   their _UID values (which must match MADT UIDs 0-31) could not be checked. Neither could any _MAT
   objects or any IOAPIC devices with _GSB.
8. **Dangling cross-reference.** "Processor (Declare Processor)" has no target section in ACPI 6.6.
9. **Table 5.21 reserved rows.** The table prints "0x1C-0x7F Reserved for OEM use" and has no
   0x80-0xFF row. The 6.6 intent for types 0x80-0xFF is not stated in the rendered table. The
   machine uses no such types.
10. **Revision gap.** The machine MADT is revision 6 and its FADT is version 6.5. ACPI 6.5 was not
    reviewed, so revision 6 vs 7 differences are not assessed. The present structures do match the
    6.6 fixed lengths.
11. **BSP identity.** MADT order is a "should" guideline, so entry 0 (APIC ID 0) is not proof of
    the BSP.
12. **Archive inventory files.** The files referenced by
    `docs/x2avic-interrupt-delivery-2026-09-16.md` were not located under
    `C:\Users\mato\Documents` (depth 6). That doc states they contain no MADT.
13. **IVRS handle meaning.** "IVRS IOAPIC handle = IOAPIC ID" comes from the project IOMMU review,
    not from ACPI. This sheet confirms only that the values 0x20 and 0x21 are numerically equal.
