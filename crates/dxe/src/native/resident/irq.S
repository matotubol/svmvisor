/* Private returning physical IRQ gates for the pinned x2AVIC bridge.
 * APM2 rev3.44 15.13.1,15.17/Table15-10,15.21.1 and 8.9.3.
 * One resident image/data copy per physical CPU. Vectors 32..255 use IST0 and
 * remain on the owned host stack; vectors 16..31 keep terminal IST1. Every
 * gate accepts an interrupt only inside the acceptance helper's window.
 * No Rust callback, SIMD, allocation, physical EOI or guest mutation in a gate.
 * CLGI prevents another delivery before return; saved IF is cleared so IRETQ
 * returns to the helper with IF=0. All interrupted GPRs remain unchanged.
 *
 * Vectors 16..31 are exception vectors (APM2 8.2 p245, Table 8-1 p246), but a
 * guest can program them into IOAPIC/MSI sources. The window runs only NOPs,
 * so no exception other than #MC (18, asynchronous) or the INIT-redirecting
 * #SX (30, error code 1, APM2 15.28 p563) can arrive there: a window event on
 * any other vector is a physical interrupt without an error code (8.2.24
 * p261). Outside the window the gate is the terminal exception stub, with the
 * unchanged exception frame. Vector 18 has no window gate (a physical vector
 * 18 is reported as a host #MC); the #SX gate (runtime.S) enters gate 30 when
 * its error code is not 1. A captured vector below 32 stops the bridge with
 * its vector (IrqError::ReservedVector).
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

/* Returning host NMI gate (vector 2), on private IST2. APM2 rev3.44 15.13.2
 * p522, 15.21.10 p536, Table 15-10 p530. Under NMI virtualization the VMCB NMI
 * intercept turns a physical NMI in guest mode into VMEXIT_NMI; a physical NMI
 * taken here, in a host GIF window (only acknowledge_init opens one), is
 * swallowed and flagged, and the dispatcher re-presents it to the guest as
 * V_NMI (runtime.rs route_physical_nmi_to_guest) rather than stopping this CPU.
 * NMI pushes no error code (8.2.3 p249). CLGI blocks a further event before the
 * return; no GPR is changed, so the interrupted host code resumes unchanged.
 * svmvisor_resident_nmi_pending is the per-CPU flag defined in runtime.rs. */
.globl svmvisor_resident_nmi
svmvisor_resident_nmi:
    clgi
    movl $1, svmvisor_resident_nmi_pending(%rip)
    iretq

.altmacro
.macro IRQ_VECTOR number
.if \number != 18
.globl svmvisor_resident_irq_\number
svmvisor_resident_irq_\number:
    clgi
    cmpl $1, svmvisor_resident_irq_window(%rip)
    jne svmvisor_resident_fault_\number
    movl $0, svmvisor_resident_irq_window(%rip)
    movl $\number, svmvisor_resident_irq_vector(%rip)
    andq $-513, 16(%rsp)
    iretq
.endif
.endm
.set vector, 16
.rept 240
    IRQ_VECTOR %vector
    .set vector, vector + 1
.endr

/* Offsets of the gates the IDT names directly; 18 (#MC) and 30 (#SX gate)
 * have none. */
.section .rodata
.p2align 2
.globl svmvisor_resident_irq_offsets
svmvisor_resident_irq_offsets:
.macro IRQ_OFFSET number
.if \number == 18 || \number == 30
    .long 0
.else
    .long svmvisor_resident_irq_\number - svmvisor_resident_irq_offsets
.endif
.endm
.set vector, 16
.rept 240
    IRQ_OFFSET %vector
    .set vector, vector + 1
.endr

.bss
.p2align 2
svmvisor_resident_irq_vector:
    .zero 4
svmvisor_resident_irq_window:
    .zero 4
