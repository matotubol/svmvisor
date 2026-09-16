/* Private returning physical IRQ gates for the pinned x2AVIC bridge.
 * APM2 rev3.44 15.13.1,15.17/Table15-10,15.21.1 and 8.9.3.
 * One resident image/data copy per physical CPU. Vectors32..255 use IST0,
 * remain on the owned host stack, and may run only in the acceptance helper.
 * No Rust callback, SIMD, allocation, physical EOI or guest mutation in a gate.
 * CLGI prevents another delivery before return; saved IF is cleared so IRETQ
 * returns to the helper with IF=0. All interrupted GPRs remain unchanged.
 */
.text
.p2align 4
.globl svmvisor_resident_accept_irq
svmvisor_resident_accept_irq:
    cli
    clgi
    movl $-1, svmvisor_resident_irq_vector(%rip)
    movl $1, svmvisor_resident_irq_window(%rip)
    stgi
    sti
    nop
    nop
    cli
    clgi
    movl $0, svmvisor_resident_irq_window(%rip)
    mov svmvisor_resident_irq_vector(%rip), %eax
    ret

.altmacro
.macro IRQ_VECTOR number
.globl svmvisor_resident_irq_\number
svmvisor_resident_irq_\number:
    clgi
    cmpl $1, svmvisor_resident_irq_window(%rip)
    jne svmvisor_resident_fault_\number
    movl $0, svmvisor_resident_irq_window(%rip)
    movl $\number, svmvisor_resident_irq_vector(%rip)
    andq $-513, 16(%rsp)
    iretq
.endm
.set vector, 32
.rept 224
    IRQ_VECTOR %vector
    .set vector, vector + 1
.endr

.section .rodata
.p2align 2
.globl svmvisor_resident_irq_offsets
svmvisor_resident_irq_offsets:
.macro IRQ_OFFSET number
    .long svmvisor_resident_irq_\number - svmvisor_resident_irq_offsets
.endm
.set vector, 32
.rept 224
    IRQ_OFFSET %vector
    .set vector, vector + 1
.endr

.bss
.p2align 2
svmvisor_resident_irq_vector:
    .zero 4
svmvisor_resident_irq_window:
    .zero 4
