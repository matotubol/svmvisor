/* Emulator-only Rust integration bootstrap; not a PE/COFF firmware driver.
 * Flat payload for QEMU Multiboot or the explicit OVMF handoff entry below.
 * AMD APM 3.44 chapter 15 and Appendix B define the SVM/VMCB fields below.
 */
.section .multiboot,"a"
.balign 4
mb_header:
    .long 0x1badb002, 0x00010003, -(0x1badb002 + 0x00010003)
    .long mb_header, image_start, image_load_end, image_bss_end, entry32
    .long 0, 0, 0, 0

.section .text,"ax"
.code32
.global entry32
entry32:
    cli
    cld
    mov $host_stack_top, %esp
    lgdt gdt_pointer
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    /* Identity-map the first 1 GiB for this isolated emulator profile. */
    mov $host_pdpt, %eax
    or $3, %eax
    mov %eax, host_pml4
    mov $host_pd, %eax
    or $3, %eax
    mov %eax, host_pdpt
    mov $nested_pdpt, %eax
    or $7, %eax
    mov %eax, nested_pml4
    mov $nested_pd, %eax
    or $7, %eax
    mov %eax, nested_pdpt
    xor %ecx, %ecx
1:
    mov %ecx, %eax
    shl $21, %eax
    or $0x83, %eax
    mov %eax, host_pd(,%ecx,8)
    or $4, %eax
    mov %eax, nested_pd(,%ecx,8)
    inc %ecx
    cmp $512, %ecx
    jne 1b
    mov $host_pml4, %eax
    mov %eax, %cr3
    mov $0x620, %eax
    mov %eax, %cr4
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax /* LME and NXE. SVME is set after capability checks. */
    wrmsr
    mov %cr0, %eax
    and $0xfffffff3, %eax
    or $0x80010003, %eax
    mov %eax, %cr0
    ljmp $8, $entry64

.code64
.global entry_uefi
.type entry_uefi,@function
/* Emulator-only post-ExitBootServices entry. The loader must have allocated
 * a 1 MiB identity-mapped arena, relocated the linked image and zeroed BSS.
 * RDI points to the six-qword handoff in its final page. No firmware calls follow.
 * This entry never returns, and does not preserve the firmware call ABI.
 */
entry_uefi:
    cli
    cld
    mov $host_stack_top, %rsp
    mov $msg_uefi, %rsi
    call print
    /* Derive every expected address from the executing image, never the header.
     * The bootstrap identity map covers the first GiB; the private image map
     * uses one 2 MiB window. Retain 1 MiB for the original Multiboot fixture.
     */
    lea image_start(%rip), %r8
    cmp $0x100000, %r8
    jb fail
    test $0xfff, %r8
    jnz fail
    cmp $0x3ff00000, %r8
    ja fail
    mov %r8, %rax
    and $0x1fffff, %eax
    cmp $0x100000, %eax /* The complete arena must fit one 2 MiB window. */
    ja fail
    lea 0xff000(%r8), %r9
    cmp %r9, %rdi
    jne fail
    movabs $0x32494645554d5653, %rax /* SVMUEFI2, little endian. */
    cmp %rax, 0(%rdi)
    jne fail
    cmp %r8, 8(%rdi)
    jne fail
    cmpq $0x100000, 16(%rdi)
    jne fail
    lea image_load_end(%rip), %rax
    sub %r8, %rax
    cmp %rax, 24(%rdi)
    jne fail
    lea image_bss_end(%rip), %rax
    cmp %r9, %rax
    ja fail
    sub %r8, %rax
    cmp %rax, 32(%rdi)
    jne fail
    lea entry_uefi(%rip), %rax
    sub %r8, %rax
    cmp %rax, 40(%rdi)
    jne fail
    mov %rdi, resident_handoff(%rip)
    mov $0x80000000, %eax
    cpuid
    cmp $0x8000000a, %eax
    jb fail
    mov $0x80000001, %eax
    cpuid
    test $4, %ecx /* SVM */
    jz fail
    test $0x100000, %edx /* NX */
    jz fail
    mov $0x8000000a, %eax
    cpuid
    test $1, %edx /* NPT */
    jz fail
    test %ebx, %ebx
    jz fail
    /* Five-level paging cannot be changed while long mode remains active. */
    mov %cr4, %rax
    test $0x1000, %eax
    jnz fail
    /* Construct the same identity bootstrap mapping as Multiboot, using only
     * image BSS. The loader has already cleared the untouched table entries.
     */
    mov $host_pdpt, %rax
    or $3, %rax
    mov %rax, host_pml4
    mov $host_pd, %rax
    or $3, %rax
    mov %rax, host_pdpt
    mov $nested_pdpt, %rax
    or $7, %rax
    mov %rax, nested_pml4
    mov $nested_pd, %rax
    or $7, %rax
    mov %rax, nested_pdpt
    xor %ecx, %ecx
7:
    mov %ecx, %eax
    shl $21, %eax
    or $0x83, %eax
    mov %rax, host_pd(,%rcx,8)
    or $4, %eax
    mov %rax, nested_pd(,%rcx,8)
    inc %ecx
    cmp $512, %ecx
    jne 7b
    mov %cr0, %rax
    and $-13, %rax /* EM and TS clear. */
    or $0x10003, %rax /* WP, MP, PE; retain active PG. */
    mov %rax, %cr0
    mov $0x620, %eax
    mov %rax, %cr4
    mov $0xc0000080, %ecx
    rdmsr
    or $0x800, %eax /* NXE; LME/LMA are already active. */
    wrmsr
    lgdt gdt_pointer64
    mov $host_pml4, %rax
    mov %rax, %cr3
    /* Reload CS from our GDT using a far return, then the flat data segments. */
    pushq $8
    lea uefi_segments(%rip), %rax
    push %rax
    lretq
uefi_segments:
    mov $16, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    jmp entry64
.size entry_uefi, .-entry_uefi

entry64:
    mov $host_stack_top, %rsp
    mov $msg_boot, %rsi
    call print
    mov $0x80000000, %eax
    cpuid
    cmp $0x8000000a, %eax
    jb fail
    mov $0x80000001, %eax
    cpuid
    test $4, %ecx
    jz fail
    mov $0x8000000a, %eax
    cpuid
    test $1, %edx /* Nested paging */
    jz fail
    test %ebx, %ebx
    jz fail
    mov $msg_svm, %rsi
    call print
    mov $0xc0000080, %ecx
    rdmsr
    or $0x1000, %eax
    wrmsr
    mov $0xc0010117, %ecx /* VM_HSAVE_PA */
    mov $host_save, %eax
    xor %edx, %edx
    wrmsr

    fninit
    ldmxcsr initial_mxcsr
    stgi /* Establish owned GIF=1 with IF=0 for every returning bridge. */
    /* 16-byte aligned before CALL; harness_main uses the System V ABI. */
    call harness_main
    jmp fail

/* SysV guest_run(VMCB *rdi, GuestRegisters *rsi, XstateContext *rdx).
 * Frame layout is crates/hypervisor/src/registers.rs: 14 qwords, no RAX/RSP.
 * Preconditions: identity-mapped aligned VMCB, valid HSAVE and guest state,
 * host physical interrupts disabled, bounded XSTATE profile, owned frame/areas.
 * Guest IF requires V_INTR_MASKING. INTR-intercept bit0 opts into the owned
 * emulator timer: CLGI closes the host delivery window before STI, then
 * VMRUN samples host IF=1 independently of guest IF. VMEXIT clears GIF;
 * CLI is the first returning instruction. STGI restores GIF with IF=0 only
 * after all host context has been restored (APM15.13.1,15.21.1).
 * Eager x87/MMX/SSE/optional AVX switching; not a native OS world switch.
 * VMRUN saves/restores host RAX/RSP/RIP. All remaining guest GPRs must be
 * captured before scratch-register use after VMEXIT. No host stack address
 * is placed in guest memory; the permissive smoke NPT is not isolation.
 */
.global guest_run
.type guest_run,@function
guest_run:
    clgi
    push %rbx
    push %rbp
    push %r12
    push %r13
    push %r14
    push %r15
    push %rdx /* Host-only XSTATE context: offset16 at VMRUN. */
    push %rsi /* Frame pointer: host stack offset 8 at VMRUN. */
    push %rdi /* VMCB pointer: host stack offset 0 at VMRUN. */
    mov %rdx, %r10
    call xstate_enter
    call clock_enter
    /* Opt-in auxiliary ownership shares this bridge. Save original host fields
     * before loading this same guest VMCB. APM2 15.5.2/15.7.2. */
    mov 16(%rsp), %r10
    mov 64(%r10), %rax
    test %rax,%rax
    jz 91f
    vmsave %rax
    mov 0(%rsp), %rax
    vmload %rax
91:
    mov 0(%rsp), %rax
    testb $1, 0x0c(%rax)
    jz 90f
    sti
90:
    mov 8(%rsp), %rsi
    mov 0x00(%rsi), %rcx
    mov 0x08(%rsi), %rdx
    mov 0x10(%rsi), %rbx
    mov 0x18(%rsi), %rbp
    mov 0x28(%rsi), %rdi
    mov 0x30(%rsi), %r8
    mov 0x38(%rsi), %r9
    mov 0x40(%rsi), %r10
    mov 0x48(%rsi), %r11
    mov 0x50(%rsi), %r12
    mov 0x58(%rsi), %r13
    mov 0x60(%rsi), %r14
    mov 0x68(%rsi), %r15
    mov 0x20(%rsi), %rsi
.global guest_run_vmrun, guest_run_vmexit
guest_run_vmrun:
    vmrun %rax
guest_run_vmexit:
    cli
    push %r15
    push %r14
    push %r13
    push %r12
    push %r11
    push %r10
    push %r9
    push %r8
    push %rdi
    push %rsi
    push %rbp
    push %rbx
    push %rdx
    push %rcx
    mov 128(%rsp), %r10 /* Capture XSTATE before host code can use SIMD. */
    mov 64(%r10), %rax
    test %rax,%rax
    jz 92f
    mov 112(%rsp), %rax /* Same guest VMCB; all guest GPRs already captured. */
    vmsave %rax
    mov 64(%r10), %rax
    vmload %rax
    mov 72(%r10), %rax
    vmsave %rax
92:
    call clock_leave
    call xstate_leave
    cld
    mov %rsp, %rsi
    mov 120(%rsp), %rdi /* 112 saved bytes + 8 to original frame pointer. */
    mov $14, %ecx
    rep movsq
    add $136, %rsp /* Discard frame and three host-only pointers. */
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbp
    pop %rbx
    stgi
    ret
.size guest_run, .-guest_run

fail:
    mov $msg_fail, %rsi
    call print
    mov $0x11, %eax
    mov $0xf4, %dx
    out %eax, %dx
2:  cli
    hlt
    jmp 2b
print:
    mov $0xe9, %dx
3:  lodsb
    test %al, %al
    jz 4f
    out %al, %dx
    jmp 3b
4:  ret

/* Each normal exit increments MM7 bytes and XORs XMM15/YMM15 with 01. */
.macro mutate_guest_xstate
    mov %cr4, %rax
    test $0x40000, %eax
    jz 80f
    xor %ecx, %ecx
    xgetbv
    test $4, %eax
    jz 80f
    vxorps guest_xstate_increment(%rip), %ymm15, %ymm15
    jmp 81f
80: xorps guest_xstate_increment(%rip), %xmm15
81: paddb guest_xstate_increment(%rip), %mm7
.endm
.global guest_entry, guest_fail, guest_stop
guest_entry:
    movabs $0x53564d534553534e, %rax
    mov %rax, 0x8ff0
    mutate_guest_xstate
    xor %eax, %eax
    xor %ecx, %ecx
    cpuid
    cmp $1, %eax
    jne guest_fail
    cmp $0x566d7653, %ebx /* SvmV */
    jne guest_fail
    cmp $0x726f7369, %edx /* isor */
    jne guest_fail
    cmp $0x74736554, %ecx /* Test */
    jne guest_fail
    mutate_guest_xstate
    xor %eax, %eax
    vmmcall
    cmp $1, %rax
    jne guest_fail
    mutate_guest_xstate
    mov $1, %eax
guest_stop:
    vmmcall
    ud2
guest_fail:
    hlt
    ud2
.global guest_npf, guest_nx, guest_guard, guest_end
guest_npf:
    mov 0x6000, %eax
    hlt
guest_nx:
    mov $0x8000, %eax
    jmp *%rax
guest_guard:
    mov 0x7000, %eax
    hlt
.global guest_x87
guest_x87:
    fninit
    fld1
    fld1
    faddp
    mov $2, %eax
    vmmcall
    hlt
.global guest_xsetbv
guest_xsetbv:
    xor %ecx, %ecx
    mov $3, %eax
    xor %edx, %edx
    xsetbv
    hlt
.global guest_exception_entry, guest_ud_handler, guest_gp_handler, guest_pf_handler
guest_exception_entry:
    xor %r12d, %r12d
    xor %r13d, %r13d
    xor %r14d, %r14d
exception_ud:
    ud2
exception_after_ud:
    cmp $1, %r12
    jne guest_fail
    mov $0x38, %eax
exception_gp:
    mov %ax, %ds
exception_after_gp:
    cmp $1, %r13
    jne guest_fail
exception_pf:
    mov 0x7000, %eax
exception_after_pf:
    cmp $1, %r14
    jne guest_fail
    mov $1, %eax
    vmmcall
    jmp guest_fail
guest_ud_handler:
    cmpq $(0x1000+exception_ud-guest_entry), (%rsp)
    jne guest_fail
    cmpq $8, 8(%rsp)
    jne guest_fail
    cmpq $0x9000, 24(%rsp)
    jne guest_fail
    inc %r12
    addq $(exception_after_ud-exception_ud), (%rsp)
    iretq
guest_gp_handler:
    cmpq $(0x1000+exception_gp-guest_entry), 8(%rsp)
    jne guest_fail
    cmpq $8, 16(%rsp)
    jne guest_fail
    cmpq $0x9000, 32(%rsp)
    jne guest_fail
    cmpq $0x38, (%rsp)
    jne guest_fail
    inc %r13
    add $8, %rsp
    addq $(exception_after_gp-exception_gp), (%rsp)
    iretq
guest_pf_handler:
    cmpq $(0x1000+exception_pf-guest_entry), 8(%rsp)
    jne guest_fail
    cmpq $8, 16(%rsp)
    jne guest_fail
    cmpq $0x9000, 32(%rsp)
    jne guest_fail
    cmpq $0, (%rsp)
    jne guest_fail
    mov %cr2, %rax
    cmp $0x7000, %rax
    jne guest_fail
    inc %r14
    add $8, %rsp
    addq $(exception_after_pf-exception_pf), (%rsp)
    iretq
.balign 32
guest_xstate_increment: .fill 32,1,1
# Benign timestamp ordering across intercepted CPUID. These intervals include
# the monitor exit/resume path; they are not instruction latency measurements.
.macro timestamp_fixture name, instruction
.global \name, \name\()_read, \name\()_after_read
\name:
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    test $16, %edx
    jz guest_fail
\name\()_read:
    \instruction
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, %r8
    mov %ecx, %r10d
    mov $0x80000001, %eax
    xor %ecx, %ecx
\name\()_after_read:
    cpuid
.ifc \instruction,rdtscp
    test $0x08000000, %edx
    jz guest_fail
.endif
    \instruction
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, %r9
    mov %ecx, %r11d
    xor %eax, %eax
    xor %ecx, %ecx
    cpuid
    mov $1, %eax
    vmmcall
    ud2
.endm
timestamp_fixture guest_rdtsc, rdtsc
timestamp_fixture guest_rdtscp, rdtscp
.global guest_clock_read, guest_clock_read_pc, guest_clock_write, guest_clock_write_pc
guest_clock_read:
    mov %edi, %ecx
guest_clock_read_pc:
    rdmsr
    ud2
guest_clock_write:
    mov %edi, %ecx
    xor %eax, %eax
    xor %edx, %edx
guest_clock_write_pc:
    wrmsr
    ud2
/* One pending virtual INTR. The host supplies expected interrupted RIP in
 * R14; only the guest handler increments R12. R13 witnesses STI's shadow.
 * APM vol.2 rev.3.44 sections8.9 and15.21.4-6. No APIC/EOI emulation. */
.global guest_irq_if, guest_irq_if_ready, guest_irq_tpr, guest_irq_tpr_ready
.global guest_irq_tpr_stopped, guest_irq_tpr_stopped_ready
.global guest_irq_handler, guest_irq_probe, guest_irq_probe_ready
guest_irq_if:
    xor %eax, %eax
    vmmcall
    cmp $1, %rax
    jne guest_fail
    sti
    inc %r13
guest_irq_if_ready:
    jmp guest_irq_continued
/* Default priority fixture: V_TPR is lowered by the stopped host after the
 * query, then hardware recognizes the same pending IRQ on the next VMRUN. */
guest_irq_tpr_stopped:
    sti
    mov %cr8, %r15
    cmp $6, %r15
    jne guest_fail
    xor %eax, %eax
    vmmcall
guest_irq_tpr_stopped_ready:
    cmp $1, %rax
    jne guest_fail
    mov %cr8, %r15
    cmp $4, %r15
    jne guest_fail
    jmp guest_irq_continued
/* Retained separate guest-cr8-unblock diagnostic. QEMU10.1 currently aborts
 * in its unlocked cpu_interrupt call when this guest write unmasks VIRQ. */
guest_irq_tpr:
    sti
    mov %cr8, %r15
    cmp $6, %r15
    jne guest_fail
    xor %eax, %eax
    vmmcall
    cmp $1, %rax
    jne guest_fail
    mov $4, %eax
    mov %rax, %cr8
guest_irq_tpr_ready:
    mov %cr8, %r15
    cmp $4, %r15
    jne guest_fail
guest_irq_continued:
    cmp $1, %r12
    jne guest_fail
    xor %eax, %eax
    vmmcall
    cmp $1, %rax
    jne guest_fail
    cmp $1, %r12
    jne guest_fail
    mov $1, %eax
    vmmcall
    jmp guest_fail
guest_irq_handler:
    cmp %r14, (%rsp)
    jne guest_fail
    cmpq $8, 8(%rsp)
    jne guest_fail
    testq $0x200, 16(%rsp)
    jz guest_fail
    cmpq $0x9000, 24(%rsp)
    jne guest_fail
    cmpq $16, 32(%rsp)
    jne guest_fail
    push %rax
    pushfq
    pop %rax
    test $0x200, %eax
    jnz guest_fail
    pop %rax
    inc %r12
    iretq
guest_irq_probe:
    sti
    nop
guest_irq_probe_ready:
    xor %eax, %eax
    vmmcall
    jmp guest_fail
/* Synthetic EOI adapter, not an APIC MMIO/MSR interface. */
.global guest_apic_loop, guest_apic_ready, guest_apic_handler, guest_apic_eoi
 guest_apic_loop:
    sti
    nop
 guest_apic_ready:
    xor %eax, %eax
    vmmcall
    jmp guest_apic_ready
 guest_apic_handler:
    cmp %r14, (%rsp)
    jne guest_fail
    cmpq $8, 8(%rsp)
    jne guest_fail
    testq $0x200, 16(%rsp)
    jz guest_fail
    cmpq $0x9000, 24(%rsp)
    jne guest_fail
    cmpq $16, 32(%rsp)
    jne guest_fail
    push %rax
    inc %r12
    mov $0xe01, %r11
    xor %eax, %eax
 guest_apic_eoi:
    vmmcall
    cmp $1, %rax
    jne guest_fail
    pop %rax
    iretq
/* Fixed admitted x2APIC MSR subset; all accesses are intercepted. */
.global guest_x2apic, guest_x2apic_ready, guest_x2apic_handler
.global guest_x2apic_write_bad
 guest_x2apic:
    mov $1, %eax
    cpuid
    test $(1<<21), %ecx
    jnz guest_fail
    test $(1<<9), %edx
    jnz guest_fail
    mov $0x1b, %ecx
    mov $-1, %rdx
    rdmsr
    cmp $0xfee00d00, %eax
    jne guest_fail
    test %rdx, %rdx
    jnz guest_fail
    mov $0x808, %ecx
    mov $0x5b, %eax
    xor %edx, %edx
    wrmsr
    mov %cr8, %r15
    cmp $5, %r15
    jne guest_fail
    rdmsr
    cmp $0x5b, %eax
    jne guest_fail
    mov $5, %r8d
    mov %r8, %cr8
    rdmsr
    cmp $0x50, %eax
    jne guest_fail
    mov $0x80a, %ecx
    rdmsr
    cmp $0x50, %eax
    jne guest_fail
    xor %r11, %r11
    xor %eax, %eax
    vmmcall
    mov $0x822, %ecx
    rdmsr
    cmp $0x10000, %eax
    jne guest_fail
    mov $2, %eax
    mov %rax, %cr8
    mov %cr8, %r15
    cmp $2, %r15
    jne guest_fail
    sti
    nop
 guest_x2apic_ready:
    cmp $1, %r12
    jne guest_fail
    mov $1, %r11
    xor %eax, %eax
    vmmcall
    cmp $1, %r12
    jne guest_fail
    mov $1, %eax
    vmmcall
    jmp guest_fail
 guest_x2apic_handler:
    cmp %r14, (%rsp)
    jne guest_fail
    cmpq $0x9000, 24(%rsp)
    jne guest_fail
    push %rax
    push %rcx
    push %rdx
    inc %r12
    mov $0x808, %ecx
    rdmsr
    cmp $0x20, %eax
    jne guest_fail
    mov $0x80a, %ecx
    rdmsr
    cmp $0x50, %eax
    jne guest_fail
    mov $0x812, %ecx
    rdmsr
    cmp $0x10000, %eax
    jne guest_fail
    mov $0x822, %ecx
    rdmsr
    test %eax, %eax
    jnz guest_fail
    mov $0x80b, %ecx
    xor %eax, %eax
    xor %edx, %edx
    wrmsr
    mov $0x80a, %ecx
    rdmsr
    cmp $0x20, %eax
    jne guest_fail
    mov $0x812, %ecx
    rdmsr
    test %eax, %eax
    jnz guest_fail
    pop %rdx
    pop %rcx
    pop %rax
    iretq
 guest_x2apic_write_bad:
    wrmsr
    ud2
.global guest_msr_gp_read, guest_msr_gp_write, guest_msr_gp_handler, guest_apic_modes
 guest_msr_gp_read:
    rdmsr
    mov $1, %eax
    vmmcall
    jmp guest_fail
 guest_msr_gp_write:
    wrmsr
    mov $1, %eax
    vmmcall
    jmp guest_fail
 guest_msr_gp_handler:
    cmpq $0, (%rsp)
    jne guest_fail
    cmp %rbx, 8(%rsp)
    jne guest_fail
    cmpq $8, 16(%rsp)
    jne guest_fail
    cmpq $0x9000, 32(%rsp)
    jne guest_fail
    cmpq $16, 40(%rsp)
    jne guest_fail
    inc %r12
    mov %r13, %rax
    mov %r14, %rcx
    mov %r15, %rdx
    add $8, %rsp
    iretq
 guest_apic_modes:
    mov $0x1b, %ecx
    rdmsr
    cmp $0xfee00100, %eax
    jne guest_fail
    mov $0xfee00900, %eax
    xor %edx, %edx
    wrmsr
    rdmsr
    cmp $0xfee00900, %eax
    jne guest_fail
    mov $0xfee00d00, %eax
    wrmsr
    mov $0x808, %ecx
    rdmsr
    test %eax, %eax
    jnz guest_fail
    mov $0x1b, %ecx
    mov $0xfee00d00, %eax
    wrmsr
    mov $0xfee00100, %eax
    wrmsr
    rdmsr
    cmp $0xfee00100, %eax
    jne guest_fail
    mov $0xfee00900, %eax
    wrmsr
    mov $0xfee00d00, %eax
    wrmsr
    mov $0x808, %ecx
    rdmsr
    test %eax, %eax
    jnz guest_fail
    mov $1, %eax
    vmmcall
    jmp guest_fail

/* Bounded MOV-to-CR8 sources. No stack access before restoring RSP. */
.global guest_cr8_source_0
 guest_cr8_source_0:
    .byte 0x44, 0x0f, 0x22, 0xc0
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_1
 guest_cr8_source_1:
    .byte 0x44, 0x0f, 0x22, 0xc1
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_2
 guest_cr8_source_2:
    .byte 0x44, 0x0f, 0x22, 0xc2
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_3
 guest_cr8_source_3:
    .byte 0x44, 0x0f, 0x22, 0xc3
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_4
 guest_cr8_source_4:
    .byte 0x44, 0x0f, 0x22, 0xc4
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_5
 guest_cr8_source_5:
    .byte 0x44, 0x0f, 0x22, 0xc5
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_6
 guest_cr8_source_6:
    .byte 0x44, 0x0f, 0x22, 0xc6
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_7
 guest_cr8_source_7:
    .byte 0x44, 0x0f, 0x22, 0xc7
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_8
 guest_cr8_source_8:
    .byte 0x45, 0x0f, 0x22, 0xc0
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_9
 guest_cr8_source_9:
    .byte 0x45, 0x0f, 0x22, 0xc1
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_10
 guest_cr8_source_10:
    .byte 0x45, 0x0f, 0x22, 0xc2
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_11
 guest_cr8_source_11:
    .byte 0x45, 0x0f, 0x22, 0xc3
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_12
 guest_cr8_source_12:
    .byte 0x45, 0x0f, 0x22, 0xc4
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_13
 guest_cr8_source_13:
    .byte 0x45, 0x0f, 0x22, 0xc5
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_14
 guest_cr8_source_14:
    .byte 0x45, 0x0f, 0x22, 0xc6
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
.global guest_cr8_source_15
 guest_cr8_source_15:
    .byte 0x45, 0x0f, 0x22, 0xc7
    mov $0x9000, %rsp
    mov $1, %eax
    vmmcall
    jmp guest_fail
/* Fixed trapped xAPIC alias. 0x7000 remains the unmapped #PF guard. */
.macro apic_read offset, expected
    mov $(0xc000 + \offset), %ebx
    mov $-1, %rax
    .byte 0x8b, 0x03
    cmp $\expected, %rax
    jne guest_fail
.endm
.macro apic_write offset, value
    mov $(0xc000 + \offset), %ebx
    mov $\value, %eax
    .byte 0x89, 0x03
.endm
.global guest_xapic, guest_xapic_ready, guest_xapic_handler
.global guest_xapic_read, guest_xapic_write, guest_xapic_word, guest_xapic_qword
 guest_xapic:
    mov $1, %eax
    cpuid
    test $(1<<21), %ecx
    jnz guest_fail
    test $(1<<9), %edx
    jnz guest_fail
    shr $24, %ebx
    jnz guest_fail
    apic_read 0x20, 0
    apic_write 0x80, 0x5b
    mov %cr8, %r15
    cmp $5, %r15
    jne guest_fail
    apic_read 0x80, 0x5b
    mov $5, %r8d
    mov %r8, %cr8
    apic_read 0xa0, 0x50
    xor %eax, %eax
    vmmcall
    apic_read 0x220, 0x10000
    mov $0x1b, %ecx
    mov $0xfee00d00, %eax
    xor %edx, %edx
    wrmsr
    mov $0x802, %ecx
    rdmsr
    test %rax, %rax
    jnz guest_fail
    test %rdx, %rdx
    jnz guest_fail
    mov $0x808, %ecx
    rdmsr
    cmp $0x50, %eax
    jne guest_fail
    mov $0x822, %ecx
    rdmsr
    cmp $0x10000, %eax
    jne guest_fail
    mov $0x1b, %ecx
    mov $0xfee00100, %eax
    wrmsr
    mov $0xfee00900, %eax
    wrmsr
    apic_read 0x20, 0
    apic_read 0x80, 0x50
    apic_read 0x220, 0x10000
    apic_write 0x80, 0x2b
    mov %cr8, %r15
    cmp $2, %r15
    jne guest_fail
    sti
    nop
 guest_xapic_ready:
    cmp $1, %r12
    jne guest_fail
    xor %eax, %eax
    vmmcall
    cmp $1, %r12
    jne guest_fail
    mov $1, %eax
    vmmcall
    jmp guest_fail
 guest_xapic_handler:
    cmp %r14, (%rsp)
    jne guest_fail
    cmpq $0x9000, 24(%rsp)
    jne guest_fail
    push %rax
    push %rbx
    inc %r12
    apic_read 0x80, 0x2b
    apic_read 0xa0, 0x50
    apic_read 0x120, 0x10000
    apic_read 0x220, 0
    apic_write 0xb0, 0
    apic_read 0xa0, 0x2b
    apic_read 0x120, 0
    pop %rbx
    pop %rax
    iretq
 guest_xapic_read:
    .byte 0x8b, 0x03
    jmp guest_fail
 guest_xapic_write:
    .byte 0x89, 0x03
    jmp guest_fail
 guest_xapic_word:
    .byte 0x66, 0x8b, 0x03
    jmp guest_fail
 guest_xapic_qword:
    .byte 0x48, 0x8b, 0x03
    jmp guest_fail

/* One program runs through both guest buses; all offsets are guest-owned.
 * Calls use at most eight stack bytes, including inside the IRQ frame.
 * The owner supplies bounded source ticks only at named VMMCALL checkpoints. */
.macro svr_read offset, expected
    mov $\offset, %ecx
    mov $\expected, %esi
    call guest_svr_read
.endm
.macro svr_write offset, value
    mov $\offset, %ecx
    mov $\value, %eax
    call guest_svr_write
.endm
.macro svr_query stage
    mov $\stage, %r13d
    xor %eax, %eax
    vmmcall
.endm
.global guest_svr, guest_svr_handler
 guest_svr:
    svr_read 0x30, 0x10
    svr_read 0xf0, 0x1ff
    svr_read 0x320, 0x10000
    svr_write 0x80, 0x70
    svr_write 0x320, 0x70
    svr_write 0x380, 4
    svr_query 0
    svr_write 0xf0, 0xff
    svr_read 0xf0, 0xff
    svr_read 0x320, 0x10070
    svr_write 0x320, 0x70
    svr_read 0x320, 0x10070
    svr_query 1
    svr_read 0x390, 0
    svr_read 0x220, 0x30000
    svr_write 0xf0, 0x1ff
    svr_read 0x320, 0x10070
    svr_write 0x80, 0
    sti
    nop
    cmp $2, %r12d
    jne guest_fail
    svr_write 0x320, 0x50
    svr_write 0x3e0, 0
    svr_write 0x380, 3
    svr_read 0x380, 3
    svr_read 0x3e0, 0
    svr_query 3
    svr_read 0x390, 3
    svr_query 4
    svr_read 0x390, 1
    svr_query 5
    svr_read 0x390, 0
    cmp $3, %r12d
    jne guest_fail
    svr_write 0x320, 0x20050
    svr_write 0x380, 3
    svr_query 6
    svr_read 0x390, 2
    cmp $4, %r12d
    jne guest_fail
    svr_query 7
    svr_read 0x390, 3
    cmp $5, %r12d
    jne guest_fail
    svr_write 0x380, 0
    svr_read 0x390, 0
    svr_write 0x320, 0x50
    svr_write 0x380, 3
    mov $0x1b, %ecx
    mov $0xfee00100, %eax
    xor %edx, %edx
    wrmsr
    svr_query 8
    mov $0x1b, %ecx
    mov $0xfee00900, %eax
    xor %edx, %edx
    wrmsr
    /* Re-enabled xAPIC reads retain timer/SVR programmed through either bus. */
    apic_read 0x390, 3
    apic_read 0x320, 0x50
    apic_read 0xf0, 0x1ff
    test %r15d, %r15d
    jz 1f
    mov $0xfee00d00, %eax
    wrmsr
1:  svr_read 0x390, 3
    svr_query 9
    svr_read 0x390, 0
    cmp $6, %r12d
    jne guest_fail
    mov $1, %eax
    vmmcall
    jmp guest_fail
 guest_svr_handler:
    inc %r12d
    cmp $1, %r12d
    jne 1f
    svr_write 0xf0, 0xff
    svr_read 0x120, 0x20000
    svr_read 0x220, 0x10000
    svr_read 0x320, 0x10070
    svr_query 2
    svr_write 0xf0, 0x1ff
    svr_read 0x320, 0x10070
1:  svr_write 0xb0, 0
    iretq
 guest_svr_read:
    test %r15d, %r15d
    jz 1f
    shr $4, %ecx
    or $0x800, %ecx
    rdmsr
    test %edx, %edx
    jnz guest_fail
    jmp 2f
1:  lea 0xc000(%rcx), %rbx
    mov $-1, %rax
    .byte 0x8b, 0x03
2:  cmp %rsi, %rax
    jne guest_fail
    ret
 guest_svr_write:
    test %r15d, %r15d
    jz 1f
    shr $4, %ecx
    or $0x800, %ecx
    xor %edx, %edx
    wrmsr
    ret
1:  lea 0xc000(%rcx), %rbx
    .byte 0x89, 0x03
    ret
 guest_end:

/* Pending timer/fault overlap: r9=fault, r10=TPR blocker, r15=APIC bus.
 * Every handler validates its own hardware frame; only the guest skips the
 * deliberate fault. No guest code/stack borrowing survives VMRUN. */
.global guest_overlap_start, guest_overlap_end
.global guest_overlap_ud, guest_overlap_gp, guest_overlap_pf, guest_overlap_irq
guest_overlap_start:
    mov $0x3e0, %ecx
    mov $0xb, %eax
    call overlap_write
    mov $0x320, %ecx
    mov $0x50, %eax
    call overlap_write
    test %r10d, %r10d
    jz 1f
    mov $0x80, %ecx
    mov $0x60, %eax
    call overlap_write
    sti
    nop
1:  mov $0x380, %ecx
    mov $4096, %eax
    call overlap_write
    xor %r11d, %r11d
    xor %eax, %eax
    vmmcall /* host waits for this programmed one-shot while stopped */
    cmp $1, %r9d
    je overlap_gp_fault
    cmp $2, %r9d
    je overlap_pf_fault
overlap_ud_fault:
    ud2
    jmp overlap_after_fault
overlap_gp_fault:
    mov $0x38, %eax
overlap_gp_instruction:
    mov %ax, %ds
    jmp overlap_after_fault
overlap_pf_fault:
    mov 0x7000, %eax
overlap_after_fault:
    cmp $1, %r13d
    jne overlap_fail
    test %r12d, %r12d
    jnz overlap_fail
    mov $2, %r11d
    xor %eax, %eax
    vmmcall /* proves exception IRETQ while the timer remains blocked */
    cli
    mov $0x80, %ecx
    xor %eax, %eax
    call overlap_write
    sti
    inc %r14 /* the STI shadow must protect this instruction */
overlap_irq_return:
    nop
    cli
    cmp $1, %r12d
    jne overlap_fail
    cmp $1, %r14d
    jne overlap_fail
    sti
    nop
    mov $3, %r11d
    xor %eax, %eax
    vmmcall /* one additional entry proves no duplicate interrupt */
    cmp $1, %r12d
    jne overlap_fail
    mov $1, %eax
    vmmcall
overlap_fail:
    mov $2, %eax
    vmmcall
    ud2
guest_overlap_ud:
    lea overlap_ud_fault(%rip), %r8
    jmp overlap_exception_frame
guest_overlap_gp:
    cmpq $0x38, (%rsp)
    jne overlap_fail
    add $8, %rsp
    lea overlap_gp_instruction(%rip), %r8
    jmp overlap_exception_frame
guest_overlap_pf:
    cmpq $0, (%rsp)
    jne overlap_fail
    mov %cr2, %rax
    cmp $0x7000, %rax
    jne overlap_fail
    add $8, %rsp
    lea overlap_pf_fault(%rip), %r8
overlap_exception_frame:
    cmp %r8, (%rsp)
    jne overlap_fail
    cmpq $8, 8(%rsp)
    jne overlap_fail
    cmpq $0x9000, 24(%rsp)
    jne overlap_fail
    test %r12d, %r12d
    jnz overlap_fail
    inc %r13
    lea overlap_after_fault(%rip), %rax
    mov %rax, (%rsp)
    mov $1, %r11d
    xor %eax, %eax
    vmmcall /* pending IRQ must not enter the interrupt-gate handler */
    iretq
guest_overlap_irq:
    lea overlap_irq_return(%rip), %rax
    cmp %rax, (%rsp)
    jne overlap_fail
    cmpq $8, 8(%rsp)
    jne overlap_fail
    cmpq $0x9000, 24(%rsp)
    jne overlap_fail
    cmp $1, %r13d
    jne overlap_fail
    cmp $1, %r14d
    jne overlap_fail
    inc %r12
    mov $0xb0, %ecx
    xor %eax, %eax
    call overlap_write
    iretq
overlap_write:
    test %r15d, %r15d
    jz 2f
    shr $4, %ecx
    or $0x800, %ecx
    xor %edx, %edx
    wrmsr
    ret
2:  lea 0xc000(%rcx), %rbx
    .byte 0x89, 0x03
    ret
guest_overlap_end:

/* Fault after first IRQ dispatch, with a second timer expiry in IRR. */
.global guest_irq_fault_start, guest_irq_fault_end
.global guest_irq_fault_ud, guest_irq_fault_gp, guest_irq_fault_pf, guest_irq_fault_handler
guest_irq_fault_start:
    mov $0x3e0, %ecx
    mov $0xb, %eax
    call irq_fault_write
    mov $0x320, %ecx
    mov $0x50, %eax
    call irq_fault_write
    mov $0x380, %ecx
    mov $4096, %eax
    call irq_fault_write
    xor %r11d, %r11d
    xor %eax, %eax
    vmmcall
    sti
    inc %r14
irq_fault_return:
    nop
    cmp $2, %r12d
    jne irq_fault_fail
    mov $4, %r11d
    xor %eax, %eax
    vmmcall
    cmp $2, %r12d
    jne irq_fault_fail
    mov $1, %eax
    vmmcall
irq_fault_fail:
    mov $2, %eax
    vmmcall
    ud2
guest_irq_fault_handler:
    lea irq_fault_return(%rip), %rax
    cmp %rax, (%rsp)
    jne irq_fault_fail
    cmpq $8, 8(%rsp)
    jne irq_fault_fail
    testq $0x200, 16(%rsp)
    jz irq_fault_fail
    cmpq $0x9000, 24(%rsp)
    jne irq_fault_fail
    cmp $1, %r14d
    jne irq_fault_fail
    inc %r12
    cmp $2, %r12d
    je irq_fault_eoi
    cmp $1, %r12d
    jne irq_fault_fail
    mov $0x380, %ecx
    mov $4096, %eax
    call irq_fault_write
    mov $1, %r11d
    xor %eax, %eax
    vmmcall
    cmp $1, %r9d
    je irq_fault_gp
    cmp $2, %r9d
    je irq_fault_pf
irq_fault_ud:
    ud2
    jmp irq_fault_after
irq_fault_gp:
    mov $0x38, %eax
irq_fault_gp_instruction:
    mov %ax, %ds
    jmp irq_fault_after
irq_fault_pf:
    mov 0x7000, %eax
irq_fault_after:
    cmp $1, %r13d
    jne irq_fault_fail
    cmp $1, %r12d
    jne irq_fault_fail
    mov $3, %r11d
    xor %eax, %eax
    vmmcall
irq_fault_eoi:
    mov $0xb0, %ecx
    xor %eax, %eax
    call irq_fault_write
    iretq
guest_irq_fault_ud:
    lea irq_fault_ud(%rip), %r8
    jmp irq_fault_frame
guest_irq_fault_gp:
    cmpq $0x38, (%rsp)
    jne irq_fault_fail
    add $8, %rsp
    lea irq_fault_gp_instruction(%rip), %r8
    jmp irq_fault_frame
guest_irq_fault_pf:
    cmpq $0, (%rsp)
    jne irq_fault_fail
    mov %cr2, %rax
    cmp $0x7000, %rax
    jne irq_fault_fail
    add $8, %rsp
    lea irq_fault_pf(%rip), %r8
irq_fault_frame:
    cmp %r8, (%rsp)
    jne irq_fault_fail
    cmpq $8, 8(%rsp)
    jne irq_fault_fail
    testq $0x200, 16(%rsp)
    jnz irq_fault_fail
    cmpq $0x8fd8, 24(%rsp)
    jne irq_fault_fail
    cmp $1, %r12d
    jne irq_fault_fail
    test %r13d, %r13d
    jnz irq_fault_fail
    inc %r13
    lea irq_fault_after(%rip), %rax
    mov %rax, (%rsp)
    mov $2, %r11d
    xor %eax, %eax
    vmmcall
    iretq
irq_fault_write:
    test %r15d, %r15d
    jz 2f
    shr $4, %ecx
    or $0x800, %ecx
    xor %edx, %edx
    wrmsr
    ret
2:  lea 0xc000(%rcx), %rbx
    .byte 0x89, 0x03
    ret
guest_irq_fault_end:

/* Bad exception gates exercise intercepted IDT delivery. NP resumes only an
 * explicitly repaired #UD continuation. DF checks error/frame then stops;
 * its undefined saved RIP is never inspected or returned through. */
.global guest_delivery_start, guest_delivery_end, guest_delivery_np, guest_delivery_pf, guest_delivery_df
guest_delivery_start:
    test %r9d, %r9d
    jz delivery_ud
    cmp $2, %r9d
    je delivery_pf
    cmp $6, %r9d
    je delivery_pf
    mov $0x38, %eax
delivery_gp_instruction:
    mov %ax, %ds
    jmp delivery_fail
delivery_pf:
    mov 0x7000, %eax
    jmp delivery_fail
delivery_ud:
    ud2
delivery_return:
    cmp $1, %r12d
    jne delivery_fail
    mov $1, %eax
    vmmcall
delivery_fail:
    mov $2, %eax
    vmmcall
    ud2
guest_delivery_np:
    mov (%rsp), %r10
    cmpq $0x32, (%rsp)
    jne delivery_fail
    add $8, %rsp
    lea delivery_ud(%rip), %rax
    cmp %rax, (%rsp)
    jne delivery_fail
    cmpq $8, 8(%rsp)
    jne delivery_fail
    cmpq $0x9000, 24(%rsp)
    jne delivery_fail
    testq $0x200, 16(%rsp)
    jnz delivery_fail
    inc %r12
    lea delivery_return(%rip), %rax
    mov %rax, (%rsp)
    mov $1, %r11d
    xor %eax, %eax
    vmmcall
    iretq
guest_delivery_pf:
    cmpq $0, (%rsp)
    jne delivery_fail
    mov %cr2, %rax
    cmp $0x2ff0, %rax
    jne delivery_fail
    add $8, %rsp
    lea delivery_gp_instruction(%rip), %rax
    cmp %rax, (%rsp)
    jne delivery_fail
    cmpq $8, 8(%rsp)
    jne delivery_fail
    cmpq $0x9000, 24(%rsp)
    jne delivery_fail
    testq $0x200, 16(%rsp)
    jnz delivery_fail
    inc %r12
    lea delivery_return(%rip), %rax
    mov %rax, (%rsp)
    mov $14, %r11d
    xor %eax, %eax
    vmmcall
    iretq
guest_delivery_df:
    cmpq $0, (%rsp)
    jne delivery_fail
    cmpq $8, 16(%rsp)
    jne delivery_fail
    cmpq $0x9000, 32(%rsp)
    jne delivery_fail
    inc %r12
    mov $8, %r11d
    mov $1, %eax
    vmmcall
    ud2
guest_delivery_end:

/* Separate immutable source blob, installed in the same owned code page only
 * after every earlier suite has stopped. All branches stay within this blob.
 * r10=case, r15=bus, r12=handler count, r13=post-HLT continuation count. */
.global guest_scheduled_start, guest_scheduled_end, guest_scheduled_handler
.global guest_scheduled_hlt, guest_scheduled_second_hlt
guest_scheduled_start:
    rdtsc
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, %r8
    mov $0x3e0, %ecx
    mov $0xb, %eax /* divide by one */
    call scheduled_write
    mov $0x320, %ecx
    mov $0x50, %eax
    cmp $1, %r10d
    jne 1f
    or $0x20000, %eax
1:  cmp $2, %r10d
    jne 2f
    or $0x10000, %eax
2:  call scheduled_write
    cmp $4, %r10d
    jne 3f
    mov $0x80, %ecx
    mov $0x50, %eax
    call scheduled_write
3:  mov $0x380, %ecx
    mov $4096, %eax
    call scheduled_write
    cmp $5, %r10d
    jne 4f
    mov $0xf0, %ecx
    mov $0xff, %eax
    call scheduled_write
4:  cmp $6, %r10d
    jne 5f
    mov $0x1b, %ecx
    mov $0xfee00100, %eax
    xor %edx, %edx
    wrmsr
5:  cmp $7, %r10d
    jne 6f
    mov $0x380, %ecx
    xor %eax, %eax
    call scheduled_write
6:  lea scheduled_after_hlt(%rip), %r14
    cmp $3, %r10d
    je guest_scheduled_hlt /* IF remains clear from admission */
    sti
guest_scheduled_hlt:
    hlt
scheduled_after_hlt:
    cli
    inc %r13
    cmp $1, %r12d
    jne scheduled_fail
    cmp $1, %r13d
    jne scheduled_fail
    cmp $1, %r10d
    jne 7f
    lea scheduled_after_second_hlt(%rip), %r14
    sti
guest_scheduled_second_hlt:
    hlt
scheduled_after_second_hlt:
    cli
    inc %r13
    cmp $2, %r12d
    jne scheduled_fail
    cmp $2, %r13d
    jne scheduled_fail
7:  mov $0x380, %ecx
    xor %eax, %eax
    call scheduled_write
    rdtsc
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, %r9
    cmp %r8, %r9
    jb scheduled_fail
    mov $1, %eax
    vmmcall
scheduled_fail:
    mov $2, %eax
    vmmcall
    ud2
guest_scheduled_handler:
    cmp %r14, (%rsp) /* hardware stacked the exact completed-HLT RIP */
    jne scheduled_fail
    inc %r12
    mov $0xb0, %ecx
    xor %eax, %eax
    call scheduled_write
    iretq
scheduled_write:
    test %r15d, %r15d
    jz 8f
    shr $4, %ecx
    or $0x800, %ecx
    xor %edx, %edx
    wrmsr
    ret
8:  lea 0xc000(%rcx), %rbx
    .byte 0x89, 0x03
    ret
guest_scheduled_end:

/* Actual CPU-only running loop. No intercepted instruction in [spin,end).
 * r10=case, r15=bus, r13=progress, r12=handler count. Expected interrupted RIP
 * is supplied in owned guest data at 8000h only when a real INTR arms delivery. */
.global guest_preempt_start, guest_preempt_end, guest_preempt_handler
.global guest_preempt_spin, guest_preempt_spin_end
guest_preempt_start:
    mov $0x3e0, %ecx
    mov $0xb, %eax
    call preempt_write
    mov $0x320, %ecx
    mov $0x50, %eax
    cmp $1, %r10d
    jne 1f
    or $0x20000, %eax
1:  cmp $4, %r10d
    jne 2f
    or $0x10000, %eax
2:  call preempt_write
    cmp $3, %r10d
    jne 3f
    mov $0x80, %ecx
    mov $0x50, %eax
    call preempt_write
3:  mov $0x380, %ecx
    mov $4096, %eax
    call preempt_write
    cmp $5, %r10d
    jne 4f
    mov $0xf0, %ecx
    mov $0xff, %eax
    call preempt_write
4:  cmp $6, %r10d
    jne 5f
    mov $0x380, %ecx
    xor %eax, %eax
    call preempt_write
5:  cmp $7, %r10d
    jne 6f
    mov $0x1b, %ecx
    mov $0xfee00100, %eax
    xor %edx, %edx
    wrmsr
6:  cmp $2, %r10d
    je guest_preempt_spin
    sti
guest_preempt_spin:
    inc %r13
    test %r12d, %r12d
    jz guest_preempt_spin
    cmp $1, %r10d
    jne guest_preempt_spin_end
    cmp $2, %r12d
    jb guest_preempt_spin
guest_preempt_spin_end:
    cli
    mov $0x380, %ecx
    xor %eax, %eax
    call preempt_write
    mov $1, %eax
    vmmcall
preempt_fail:
    mov $2, %eax
    vmmcall
    ud2
guest_preempt_handler:
    push %rax
    mov 0x8000, %rax
    cmp %rax, 8(%rsp)
    pop %rax
    jne preempt_fail
    inc %r12
    mov $0xb0, %ecx
    xor %eax, %eax
    call preempt_write
    iretq
preempt_write:
    test %r15d, %r15d
    jz 7f
    shr $4, %ecx
    or $0x800, %ecx
    xor %edx, %edx
    wrmsr
    ret
7:  lea 0xc000(%rcx), %rbx
    .byte 0x89, 0x03
    ret
guest_preempt_end:

/* Host-only F0 owner. Preserve every GPR/flags through IRET; no Rust/SIMD. */
.global host_preempt_handler
host_preempt_handler:
    push %rax
    incq host_preempt_acks(%rip)
    mov $0xfee000b0, %eax
    movl $0, (%rax)
    pop %rax
    iretq

.section .rodata,"a"
.balign 8
.global gdt, gdt_end
gdt:
    .quad 0, 0x00af9b000000ffff, 0x00cf93000000ffff
gdt_end:
gdt_pointer:
    .word gdt_end-gdt-1
    .long gdt
gdt_pointer64:
    .word gdt_end-gdt-1
    .quad gdt
.balign 4
initial_mxcsr: .long 0x1f80
msg_boot: .asciz "boot64-rust\n"
msg_uefi: .asciz "uefi-private-entry\n"
msg_svm: .asciz "svm_available npt_available\n"
msg_fail: .asciz "FAIL rust-bootstrap\n"

.section .bss,"aw",@nobits
.balign 8
.global host_preempt_acks
host_preempt_acks: .skip 8
.balign 8
.global resident_handoff
resident_handoff: .skip 8
.balign 4096
.global host_pml4, nested_pml4, vmcb, guest_stack_top
host_pml4: .skip 4096
host_pdpt: .skip 4096
host_pd: .skip 4096
nested_pml4: .skip 4096
nested_pdpt: .skip 4096
nested_pd: .skip 4096
vmcb: .skip 4096
host_save: .skip 4096
guest_stack: .skip 16384
guest_stack_top:
.balign 4096
.global host_stack_guard, host_stack_guard_end, host_stack
.global host_stack_high_guard, host_stack_high_guard_end
host_stack_guard: .skip 4096
host_stack_guard_end:
host_stack: .skip 65536
.global host_stack_top
host_stack_top:
host_stack_high_guard: .skip 4096
host_stack_high_guard_end:




