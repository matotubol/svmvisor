<!-- PDF source page: 48 | printed page: xlviii -->

<a id="preface"></a>

# Preface

<a id="about-this-book"></a>

## About This Book

This book is part of a multi-volume work entitled the *AMD64 Architecture Programmer’s Manual*. This table lists each volume and its order number.

**Title Order No.**

*Volume 1: Application Programming* 24592

*Volume 2: System Programming* 24593

*Volume 3: General-Purpose and System Instructions* 24594

*Volume 4: 128-Bit and 256-Bit Media Instructions* 26568

*Volume 5: 64-Bit Media and x87 Floating-Point Instructions* 26569

<a id="audience"></a>

## Audience

This volume (Volume 2) is intended for programmers writing operating systems, loaders, linkers, device drivers, or system utilities. It assumes an understanding of AMD64 architecture application-level programming as described in Volume 1.

This volume describes the AMD64 architecture’s resources and functions that are managed by system software, including operating-mode control, memory management, interrupts and exceptions, task and state-change management, system-management mode (including power management), multi-processor support, debugging, and processor initialization.

Application-programming topics are described in Volume 1. Details about each instruction are described in Volumes 3, 4, and 5.

<a id="organization"></a>

## Organization

This volume begins with an overview of system programming and differences between the x86 and AMD64 architectures. This is followed by chapters that describe the following details of system programming:

- *System Resources*—The system registers and processor ID (CPUID) functions.
- *Segmented Virtual Memory*—The segmented-memory models supported by the architecture and their associated data structures and protection checks.
- *Page Translation and Protection*—The page-translation functions supported by the architecture and their associated data structures and protection checks.


<!-- PDF source page: 49 | printed page: xlix -->

- *System Instructions*—The instructions used to manage system functions.
- *Memory System*—The memory-system hierarchy and its resources and protocols, including memory-characterization, caching, and buffering functions.
- *Exceptions and Interrupts*—Details about the types and causes of exceptions and interrupts, and the methods of transferring control during these events.
- *Machine-Check Mechanism*—The resources and functions that support detection and handling of machine-check errors.
- *System-Management Mode*—The resources and functions that support system-management mode (SMM), including power-management functions.
- *SSE, MMX, and x87 Programming*—The resources and functions that support use (by application software) and state-saving (by the operation system) of the 256-bit media, 128-bit media, 64-bit media, and x87 floating-point instructions.
- *Multiple-Processor Management*—The features of the instruction set and the system resources and functions that support multiprocessing environments.
- *Debug and Performance Resources*—The system resources and functions that support software debugging and performance monitoring.
- *Legacy Task Management*—Support for the legacy hardware multitasking functions, including register resources and data structures.
- *Processor Initialization and Long-Mode Activation*—The methods by which system software initializes and changes operating modes.
- *Mixing Code Across Operating Modes*—Things to remember when running programs in different operating modes.
- *Secure Virtual Machine*—The system resources that support machine virtualization.
- *Advanced Programmable Interrupt Controller (APIC) operation*.

There are appendices describing details of model-specific registers (MSRs) and machine-check implementations. Definitions assumed throughout this volume are listed below. The index at the end of this volume cross-references topics within the volume. For other topics relating to the AMD64 architecture, see the tables of contents and indexes of the other volumes.

<a id="conventions-and-definitions"></a>

## Conventions and Definitions

The section which follows, **Notational Conventions,** describes notational conventions used in this volume. The next section, **Definitions,** lists a number of terms used in this volume along with their technical definitions. Some of these definitions assume knowledge of the legacy x86 architecture. See “Related Documents” on page lx for further information about the legacy x86 architecture. Finally, the **Registers** section lists the registers which are a part of the system programming model.


<!-- PDF source page: 50 | printed page: l -->

<a id="notational-conventions"></a>

### Notational Conventions

#GP(0) An instruction exception—in this example, a general-protection exception with error code of 0.

1011b A binary value—in this example, a 4-bit value.

F0EA_0B02h A hexadecimal value. Underscore characters may be inserted to improve readability.

128 Numbers without an alpha suffix are decimal unless the context indicates otherwise.

7:4 A bit range, from bit 7 to 4, inclusive. The high-order bit is shown first. Commas may be inserted to indicate gaps.

CPUID Fn*XXXX_XXXX_RRR*[*FieldName*] Support for optional features or the value of an implementation-specific parameter of a processor can be discovered by executing the CPUID instruction on that processor. To obtain this value, software must execute the CPUID instruction with the function code *XXXX_XXXX*h in EAX and then examine the field *FieldName* returned in register *RRR*. If the “*_RRR*” notation is followed by “_x*YYY*”, register ECX must be set to the value *YYY*h before executing CPUID. When *FieldName* is not given, the entire contents of register *RRR* contains the desired value. When determining optional feature support, if the bit identified by *FieldName* is set to a one, the feature is supported on that processor.

CR0–CR4 A register range, from register CR0 through CR4, inclusive, with the low-order register first.

CR0[PE], CR0.PE Notation for referring to a field within a register—in this case, the PE field of the CR0 register.

CR0[PE] = 1, CR0.PE = 1 The PE field of the CR0 register is set (contains the value 1).

EFER[LME] = 0, EFER.LME = 0 The LME field of the EFER register is cleared (contains a value of 0).

DS:SI A far pointer or logical address. The real address or segment descriptor specified by the segment register (DS in this example) is combined with the offset contained in the second register (SI in this example) to form a real or virtual address.


<!-- PDF source page: 51 | printed page: li -->

RFLAGS[13:12] A field within a register identified by its bit range. In this example, corresponding to the IOPL field.

<a id="definitions"></a>

### Definitions

16-bit mode Legacy mode or compatibility mode in which a 16-bit address size is active. See *legacy mode* and *compatibility mode*.

32-bit mode Legacy mode or compatibility mode in which a 32-bit address size is active. See *legacy mode* and *compatibility mode*.

64-bit mode A submode of *long mode*. In 64-bit mode, the default address size is 64 bits and new features, such as register extensions, are supported for system and application software.

absolute Said of a displacement that references the base of a code segment rather than an instruction pointer. Contrast with *relative*.

ASID Address space identifier.

byte Eight bits.

clear To write a bit value of 0. Compare *set*.

compatibility mode A submode of *long mode*. In compatibility mode, the default address size is 32 bits, and legacy 16-bit and 32-bit applications run without modification.

commit To irreversibly write, in program order, an instruction’s result to software-visible storage, such as a register (including flags), the data cache, an internal write buffer, or memory.

CPL Current privilege level.

direct Referencing a memory location whose address is included in the instruction’s syntax as an immediate operand. The address may be an absolute or relative address. Compare *indirect*.


<!-- PDF source page: 52 | printed page: lii -->

dirty data Data held in the processor’s caches or internal buffers that is more recent than the copy held in main memory.

displacement A signed value that is added to the base of a segment (absolute addressing) or an instruction pointer (relative addressing). Same as *offset*.

doubleword Two words, or four bytes, or 32 bits.

double quadword Eight words, or 16 bytes, or 128 bits. Also called *octword*.

effective address size The address size for the current instruction after accounting for the default address size and any address-size override prefix.

effective operand size The operand size for the current instruction after accounting for the default operand size and any operand-size override prefix.

exception An abnormal condition that occurs as the result of executing an instruction. The processor’s response to an exception depends on the type of the exception. For all exceptions except 128-bit media SIMD floating-point exceptions and x87 floating-point exceptions, control is transferred to the handler (or service routine) for that exception, as defined by the exception’s vector. For floating-point exceptions defined by the IEEE 754 standard, there are both masked and unmasked responses. When unmasked, the exception handler is called, and when masked, a default response is provided instead of calling the handler.

flush An often ambiguous term meaning (1) writeback, if modified, and invalidate, as in “flush the cache line,” or (2) invalidate, as in “flush the pipeline,” or (3) change a value, as in “flush to zero.”

GDT Global descriptor table.

GIF Global interrupt flag.

GPA Guest physical address. In a virtualized environment, the page tables maintained by the guest operating system provide the translation from the linear (virtual) address to the guest physical


<!-- PDF source page: 53 | printed page: liii -->

address. Nested page tables define the translation of the GPA to the host physical address (HPA). See *SPA* and *HPA*.

HPA Host physical address. The address space owned by the virtual machine monitor. In a virtualized environment, nested page translation tables controlled by the VMM provide the translation from the guest physical address to the host physical address. See *GPA*.

IDT Interrupt descriptor table.

IGN Ignored. Value written is ignored by hardware. Value returned on a read is indeterminate. See *reserved*.

indirect Referencing a memory location whose address is in a register or other memory location. The address may be an absolute or relative address. Compare *direct*.

IRB The virtual-8086 mode interrupt-redirection bitmap.

IST The long-mode interrupt-stack table.

IVT The real-address mode interrupt vector table.

LDT Local descriptor table.

legacy x86 The legacy x86 architecture. See “Related Documents” on page lx for descriptions of the legacy x86 architecture.

legacy mode An operating mode of the AMD64 architecture in which existing 16-bit and 32-bit applications and operating systems run without modification. A processor implementation of the AMD64 architecture can run in either *long mode* or *legacy mode*. Legacy mode has three submodes, *real mode*, *protected mode*, and *virtual-8086 mode*.

LIP Linear Instruction Pointer. LIP = (CS.base + rIP).


<!-- PDF source page: 54 | printed page: liv -->

long mode An operating mode unique to the AMD64 architecture. A processor implementation of the AMD64 architecture can run in either *long mode* or *legacy mode*. Long mode has two submodes, *64-bit mode* and *compatibility mode*.

lsb Least-significant bit.

LSB Least-significant byte.

main memory Physical memory, such as RAM and ROM (but not cache memory) that is installed in a particular computer system.

mask (1) A control bit that prevents the occurrence of a floating-point exception from invoking an exception-handling routine. (2) A field of bits used for a control purpose.

MBZ Must be zero. If software attempts to set an MBZ bit to 1 in a system register, a general-protection exception (#GP) occurs; if in a translation table entry, a reserved-bit page fault exception (#PF) will occur if the hardware attempts to use the entry for address translation. See *reserved*.

memory Unless otherwise specified, *main memory*.

ModRM A byte following an instruction opcode that specifies address calculation based on mode (Mod), register (R), and memory (M) variables.

moffset A 16, 32, or 64-bit offset that specifies a memory operand directly, without using a ModRM or SIB byte.

msb Most-significant bit.

MSB Most-significant byte.

octword Same as *double quadword.*

offset Same as *displacement*.


<!-- PDF source page: 55 | printed page: lv -->

overflow The condition in which a floating-point number is larger in magnitude than the largest, finite, positive or negative number that can be represented in the data-type format being used.

PAE Physical-address extensions.

physical memory Actual memory, consisting of *main memory* and cache.

probe A check for an address in a processor’s caches or internal buffers. *External probes* originate outside the processor, and *internal probes* originate within the processor.

procedure stack A portion of a stack segment in memory that is used to link procedures. Also known as a *program stack.*

program stack See *procedure stack.*

protected mode A submode of *legacy mode*.

quadword Four words, or eight bytes, or 64 bits.

RAZ Value returned on a read is always zero (0) regardless of what was previously written. See *reserved*.

RA1 Value returned on a read is always one (1) regardless of what was previously written. See *reserved*.

real-address mode See *real mode*.

real mode A short name for *real-address mode,* a submode of *legacy mode*.

relative Referencing with a displacement (also called offset) from an instruction pointer rather than the base of a code segment. Contrast with *absolute*.


<!-- PDF source page: 56 | printed page: lvi -->

reserved Fields marked as reserved may be used at some future time. To preserve compatibility with future processors, reserved fields require special handling when read or written by software. Software must not depend on the state of a reserved field (unless qualified as RAZ), nor upon the ability of such fields to return a previously written state. If a field is marked reserved without qualification, software must not change the state of that field; it must reload that field with the same value returned from a prior read. Reserved fields may be qualified as IGN, MBZ, RAZ, or SBZ (see definitions).

REX An instruction prefix that specifies a 64-bit operand size and provides access to additional registers.

RIP-relative addressing Addressing relative to the 64-bit RIP instruction pointer.

SBZ Should be zero. An attempt by software to set an SBZ bit to 1 results in undefined behavior. See *reserved*.

shadow stack A shadow stack is a separate, protected stack that is conceptually parallel to the procedure stack and used only by the shadow stack feature.

set To write a bit value of 1. Compare *clear*.

SIB A byte following an instruction opcode that specifies address calculation based on scale (S), index (I), and base (B).

SPA System physical address. The address directly used to address system memory. Under SVM, also known as the host physical address. See *HPA*.

sticky bit A bit that is set or cleared by hardware and that remains in that state until explicitly changed by software.

SVM Secure virtual machine. AMD’s virtualization architecture. SVM is defined in Chapter 15 on page 498.


<!-- PDF source page: 57 | printed page: lvii -->

System software Privileged software that owns and manages the hardware resources of a system after initialization by *system firmware* and controls access to these resources. In a non-virtualized environment, system software is provided by the operating system. In a virtualized environment, system software is largely equivalent to the virtual machine monitor (VMM), also commonly known as the *hypervisor*.

TOP The x87 top-of-stack pointer.

TSS Task-state segment.

underflow The condition in which a floating-point number is smaller in magnitude than the smallest nonzero, positive or negative number that can be represented in the data-type format being used.

vector (1) A set of integer or floating-point values, called *elements*, that are packed into a single data object. Most of the SSE and 64-bit media instructions use vectors as operands. (2) An index into an interrupt descriptor table (IDT), used to access exception handlers. Compare *exception*.

virtual-8086 mode A submode of *legacy mode*.

VMCB Virtual machine control block.

VMM Virtual machine monitor.

word Two bytes, or 16 bits.

x86 See *legacy x86*.

<a id="registers"></a>

### Registers

In the following list of registers, the names are used to refer either to a given register or to the contents of that register:

AH–DH The high 8-bit AH, BH, CH, and DH registers. Compare *AL–DL.*


<!-- PDF source page: 58 | printed page: lviii -->

AL–DL The low 8-bit AL, BL, CL, and DL registers. Compare *AH–DH.*

AL–r15B The low 8-bit AL, BL, CL, DL, SIL, DIL, BPL, SPL, and R8B–R15B registers, available in 64-bit mode.

BP Base pointer register.

CR*n* Control register number *n*.

CS Code segment register.

eAX–eSP The 16-bit AX, BX, CX, DX, DI, SI, BP, and SP registers or the 32-bit EAX, EBX, ECX, EDX, EDI, ESI, EBP, and ESP registers. Compare *rAX–rSP.*

EFER Extended features enable register.

eFLAGS 16-bit or 32-bit flags register. Compare *rFLAGS*.

EFLAGS 32-bit (extended) flags register.

eIP 16-bit or 32-bit instruction-pointer register. Compare *rIP*.

EIP 32-bit (extended) instruction-pointer register.

FLAGS 16-bit flags register.

GDTR Global descriptor table register.

GPRs General-purpose registers. For the 16-bit data size, these are AX, BX, CX, DX, DI, SI, BP, and SP. For the 32-bit data size, these are EAX, EBX, ECX, EDX, EDI, ESI, EBP, and ESP. For the 64-bit data size, these include RAX, RBX, RCX, RDX, RDI, RSI, RBP, RSP, and R8–R15.


<!-- PDF source page: 59 | printed page: lix -->

IDTR Interrupt descriptor table register.

IP 16-bit instruction-pointer register.

LDTR Local descriptor table register.

MSR Model-specific register.

r8–r15 The 8-bit R8B–R15B registers, or the 16-bit R8W–R15W registers, or the 32-bit R8D–R15D registers, or the 64-bit R8–R15 registers.

rAX–rSP The 16-bit AX, BX, CX, DX, DI, SI, BP, and SP registers, or the 32-bit EAX, EBX, ECX, EDX, EDI, ESI, EBP, and ESP registers, or the 64-bit RAX, RBX, RCX, RDX, RDI, RSI, RBP, and RSP registers. Replace the placeholder *r* with nothing for 16-bit size, “E” for 32-bit size, or “R” for 64-bit size.

RAX 64-bit version of the EAX register.

RBP 64-bit version of the EBP register.

RBX 64-bit version of the EBX register.

RCX 64-bit version of the ECX register.

RDI 64-bit version of the EDI register.

RDX 64-bit version of the EDX register.

rFLAGS 16-bit, 32-bit, or 64-bit flags register. Compare *RFLAGS*.

RFLAGS 64-bit flags register. Compare *rFLAGS*.


<!-- PDF source page: 60 | printed page: lx -->

rIP 16-bit, 32-bit, or 64-bit instruction-pointer register. Compare *RIP*.

RIP 64-bit instruction-pointer register.

RSI 64-bit version of the ESI register.

RSP 64-bit version of the ESP register.

SP Stack pointer register.

SS Stack segment register.

SSP Shadow-stack pointer register.

TPR Task priority register (CR8), a new register introduced in the AMD64 architecture to speed interrupt management.

TR Task register.

ZMM/YMM/XMM Set of 8, 16, or 32 registers, 128, 256, or 512 bits wide, depending on features enabled and mode of the CPU.

<a id="endian-order"></a>

### Endian Order

The x86 and AMD64 architectures address memory using little-endian byte-ordering. Multibyte values are stored with their least-significant byte at the lowest byte address, and they are illustrated with their least significant byte at the right side. Strings are illustrated in reverse order, because the addresses of their bytes increase from right to left.

<a id="related-documents"></a>

## Related Documents

- Peter Abel, *IBM PC Assembly Language and Programming*, Prentice-Hall, Englewood Cliffs, NJ,
- Rakesh Agarwal, *80x86 Architecture & Programming: Volume II*, Prentice-Hall, Englewood Cliffs, NJ, 1991.


<!-- PDF source page: 61 | printed page: lxi -->

- AMD, *BIOS and Kernel Developer’s Guide* (BKDG) for particular hardware implementations of older families of the AMD64 architecture.
- AMD, *Processor Programming Reference (PPR)* for particular hardware implementations of newer families of the AMD64 architecture.
- AMD, *AMD I/O Virtualization Technology (IOMMU) Specification*, Revision 2.2 or later; order number 48882.
- AMD, *Software Optimization Guide for AMD Family 15h Processors,* order number 47414.
- Don Anderson and Tom Shanley, *Pentium Processor System Architecture*, Addison-Wesley, New York, 1995.
- Nabajyoti Barkakati and Randall Hyde, *Microsoft Macro Assembler Bible*, Sams, Carmel, Indiana,
- Barry B. Brey, *8086/8088, 80286, 80386, and 80486 Assembly Language Programming*, Macmillan Publishing Co., New York, 1994.
- Barry B. Brey, *Programming the 80286, 80386, 80486, and Pentium Based Personal Computer*, Prentice-Hall, Englewood Cliffs, NJ, 1995.
- Ralf Brown and Jim Kyle, *PC Interrupts,* Addison-Wesley, New York, 1994.
- Penn Brumm and Don Brumm, *80386/80486 Assembly Language Programming*, Windcrest McGraw-Hill, 1993.
- Geoff Chappell, *DOS Internals,* Addison-Wesley, New York, 1994.
- Chips and Technologies, Inc. *Super386 DX Programmer’s Reference Manual*, Chips and Technologies, Inc., San Jose, 1992.
- John Crawford and Patrick Gelsinger, *Programming the 80386*, Sybex, San Francisco, 1987.
- Cyrix Corporation, *5x86 Processor BIOS Writer's Guide*, Cyrix Corporation, Richardson, TX,
- Cyrix Corporation, *M1 Processor Data Book*, Cyrix Corporation, Richardson, TX, 1996.
- Cyrix Corporation, *MX Processor MMX Extension Opcode Table*, Cyrix Corporation, Richardson, TX, 1996.
- Cyrix Corporation, *MX Processor Data Book*, Cyrix Corporation, Richardson, TX, 1997.
- Ray Duncan, *Extending DOS: A Programmer's Guide to Protected-Mode DOS*, Addison Wesley, NY, 1991.
- William B. Giles, *Assembly Language Programming for the Intel 80xxx Family*, Macmillan, New York, 1991.
- Frank van Gilluwe, *The Undocumented PC,* Addison-Wesley, New York, 1994.
- John L. Hennessy and David A. Patterson, *Computer Architecture*, Morgan Kaufmann Publishers, San Mateo, CA, 1996.
- Thom Hogan, *The Programmer’s PC Sourcebook*, Microsoft Press, Redmond, WA, 1991.
- Hal Katircioglu, *Inside the 486, Pentium, and Pentium Pro*, Peer-to-Peer Communications, Menlo Park, CA, 1997.


<!-- PDF source page: 62 | printed page: lxii -->

- IBM Corporation, *486SLC Microprocessor Data Sheet*, IBM Corporation, Essex Junction, VT,
- IBM Corporation, *486SLC2 Microprocessor Data Sheet*, IBM Corporation, Essex Junction, VT,
- IBM Corporation, *80486DX2 Processor Floating Point Instructions*, IBM Corporation, Essex Junction, VT, 1995.
- IBM Corporation, *80486DX2 Processor BIOS Writer's Guide*, IBM Corporation, Essex Junction, VT, 1995.
- IBM Corporation, *Blue Lightning 486DX2 Data Book*, IBM Corporation, Essex Junction, VT,
- Institute of Electrical and Electronics Engineers, *IEEE Standard for Binary Floating-Point Arithmetic*, ANSI/IEEE Std 754-1985.
- Institute of Electrical and Electronics Engineers, *IEEE Standard for Radix-Independent Floating- Point Arithmetic*, ANSI/IEEE Std 854-1987.
- Muhammad Ali Mazidi and Janice Gillispie Mazidi, *80X86 IBM PC and Compatible Computers*, Prentice-Hall, Englewood Cliffs, NJ, 1997.
- Hans-Peter Messmer, *The Indispensable Pentium Book,* Addison-Wesley, New York, 1995.
- Karen Miller, *An Assembly Language Introduction to Computer Architecture: Using the Intel Pentium*, Oxford University Press, New York, 1999.
- Stephen Morse, Eric Isaacson, and Douglas Albert, *The 80386/387 Architecture*, John Wiley & Sons, New York, 1987.
- NexGen Inc.*, Nx586TM Processor Data Book*, NexGen Inc., Milpitas, CA, 1993.
- NexGen Inc.*, Nx686TM Processor Data Book*, NexGen Inc., Milpitas, CA, 1994.
- Bipin Patwardhan, *Introduction to the Streaming SIMD Extensions in the Pentium® III*, www.x86.org/articles/sse_pt1/ simd1.htm, June, 2000.
- Peter Norton, Peter Aitken, and Richard Wilton, *PC Programmer’s Bible,* Microsoft Press, Redmond, WA, 1993.
- *PharLap 386|ASM Reference Manual*, Pharlap, Cambridge MA, 1993.
- *PharLap TNT DOS-Extender Reference Manual*, Pharlap, Cambridge MA, 1995.
- Sen-Cuo Ro and Sheau-Chuen Her, *i386/i486 Advanced Programming*, Van Nostrand Reinhold, New York, 1993.
- Jeffrey P. Royer, *Introduction to Protected Mode Programming*, course materials for an onsite class, 1992.
- Tom Shanley, *Protected Mode System Architecture*, Addison Wesley, NY, 1996.
- SGS-Thomson Corporation, *80486DX Processor SMM Programming Manual*, SGS-Thomson Corporation, 1995.
- Walter A. Triebel, *The 80386DX Microprocessor*, Prentice-Hall, Englewood Cliffs, NJ, 1992.
- John Wharton, *The Complete x86*, MicroDesign Resources, Sebastopol, California, 1994.
