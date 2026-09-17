/* Terminal host exceptions. APM2 rev3.44 8.2/Table8-1, 8.9.3/4.
 * All IDT entries except returning #SX use private IST1. No recovery/IRET.
 * One runtime image belongs to one physical CPU. The first record is copied
 * out of the IST before calling Rust; a nested fault halts without changing
 * that copy. Broken IDT/TSS/IST/data mappings cannot be reported reliably.
 */
.text
.p2align 4
    nop /* Keep the first vector distinct from the preceding object end symbol. */
.altmacro
.macro FAULT_VECTOR number
.globl svmvisor_resident_fault_\number
svmvisor_resident_fault_\number:
.if !(\number == 8 || \number == 10 || \number == 11 || \number == 12 || \number == 13 || \number == 14 || \number == 17 || \number == 21 || \number == 29 || \number == 30)
    pushq $0
.endif
    pushq $\number
    jmp svmvisor_resident_fault_common
.endm
.set vector, 0
.rept 256
    FAULT_VECTOR %vector
    .set vector, vector + 1
.endr
.globl svmvisor_resident_fault
.set svmvisor_resident_fault, svmvisor_resident_fault_255

.globl svmvisor_resident_fault_common
svmvisor_resident_fault_common:
    cli
    clgi
    cld
    mov $1, %al
    xchg %al, svmvisor_resident_fault_latched(%rip)
    test %al, %al
    jnz svmvisor_resident_fault_stop
    /* Normalized frame: vector,error,RIP,CS,RFLAGS,RSP,SS. Capture CR2
     * before any callback can fault. No dereference of the interrupted RSP. */
    mov %cr2, %r8
    mov %cr3, %r9
    mov %rsp, %rsi
    lea svmvisor_resident_fault_record(%rip), %rdi
    mov $7, %ecx
    rep movsq
    mov %r8, (%rdi)
    mov %r9, 8(%rdi)
    lea svmvisor_resident_fault_record(%rip), %rdi
    mov %r8, %rsi
    mov %r9, %rdx
    and $-16, %rsp
    call svmvisor_resident_host_fault
.globl svmvisor_resident_fault_stop
svmvisor_resident_fault_stop:
    cli
    hlt
    jmp svmvisor_resident_fault_stop

.section .rodata
.p2align 2
.globl svmvisor_resident_fault_offsets
svmvisor_resident_fault_offsets:
.macro FAULT_OFFSET number
    .long svmvisor_resident_fault_\number - svmvisor_resident_fault_offsets
.endm
.set vector, 0
.rept 256
    FAULT_OFFSET %vector
    .set vector, vector + 1
.endr

.section .bss
.p2align 3
.globl svmvisor_resident_fault_record
svmvisor_resident_fault_record:
    .zero 72
svmvisor_resident_fault_latched:
    .zero 1
