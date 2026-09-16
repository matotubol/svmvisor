/* Disposable guest-authored INIT/SIPI continuation. Copied to retained guest
 * RAM below 640KiB; immediate fields patched before EBS. APM2 14.1/14.6/15.27.8.
 * The guest writes independent state witnesses, then spins without VM exits. */
.section .rdata,"dr"
.code16
.globl guest_start, guest_end, guest_protected_target, guest_protected
.globl guest_root, guest_long_target, guest_long, guest_gdt_base
#if SVMVISOR_GUEST_CPUID_NRIP
.globl guest_cpuid_base
#endif
.globl guest_irq_handler
guest_start:
    mov %eax, %cs:0x810
    mov %ebx, %cs:0x814
    mov %ecx, %cs:0x818
    mov %edx, %cs:0x81c
    mov %cr0, %eax
    mov %eax, %cs:0x820
    mov %cr3, %eax
    mov %eax, %cs:0x824
    mov %cr4, %eax
    mov %eax, %cs:0x828
    mov %cs, %ax
    mov %ax, %ds
    mov %ax, %ss
    mov $0xf00, %sp
    pushfl
    pop %eax
    mov %eax, 0x82c
    incl 0x800
    mov $1, %eax
    cpuid
    cli
    cld
    lgdt (guest_gdtr-guest_start)
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0
    .byte 0x66, 0xea
guest_protected_target: .long 0
    .word 24
.code32
guest_protected:
    mov $16, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    .byte 0xb8
guest_root: .long 0
    mov %eax, %cr3
    mov $0x620, %eax
    mov %eax, %cr4
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax
    wrmsr
    mov %cr0, %eax
    or $0x80010033, %eax
    mov %eax, %cr0
#if SVMVISOR_GUEST_CPUID_NRIP
    /* LMA=1 with CS.L=0 is compatibility32. Execute a real prefixed CPUID
     * before the far jump selects 64-bit code; retain independent witnesses. */
    .byte 0xbe
guest_cpuid_base: .long 0
    mov $1, %eax
    xor %ecx, %ecx
    .byte 0x66, 0x67, 0x0f, 0xa2
    mov %eax, 0x974(%esi)
    mov %cs, %ax
    movzwl %ax, %eax
    mov %eax, 0x970(%esi)
#endif
    .byte 0xea
guest_long_target: .long 0
    .word 8
.code64
guest_long:
    lea guest_start(%rip), %rsi
    lea 0xf00(%rsi), %rsp
#if SVMVISOR_GUEST_CPUID_NRIP
    mov $1, %eax
    xor %ecx, %ecx
    .byte 0x66, 0x67, 0x48, 0x0f, 0xa2
    mov %eax, 0x978(%rsi)
    mov %cs, %ax
    movzwl %ax, %eax
    mov %eax, 0x97c(%rsi)
#endif
    /* Independent guest readback after INIT/SIPI. DR0-3 are live across
     * ordinary VMEXIT; DR6/7 come from the guest VMCB (APM2 Table14-1). */
    mov %dr0, %rax
    mov %rax, 0x8b0(%rsi)
    mov %dr1, %rax
    mov %rax, 0x8b8(%rsi)
    mov %dr2, %rax
    mov %rax, 0x8c0(%rsi)
    mov %dr3, %rax
    mov %rax, 0x8c8(%rsi)
    mov %dr6, %rax
    mov %rax, 0x8d0(%rsi)
    mov %dr7, %rax
    mov %rax, 0x8d8(%rsi)
    mov $0xc0000080, %ecx
    rdmsr
    mov %eax, 0x830(%rsi)
#if SVMVISOR_GUEST_VMCR
    mov $0xc0010114, %ecx
    rdmsr
    mov %eax, 0x880(%rsi)
    mov %edx, 0x884(%rsi)
#endif
    mov $0x1b, %ecx
    rdmsr
    test $0x400, %eax
    jz guest_xapic_snapshot
    mov $0x80f, %ecx
    rdmsr
    mov %eax, 0x834(%rsi)
    mov $0x830, %ecx
    rdmsr
    mov %eax, 0x838(%rsi)
    mov %edx, 0x83c(%rsi)
    mov $0x808, %ecx
    rdmsr
    mov %eax, 0x840(%rsi)
    mov $0x838, %ecx
    rdmsr
    mov %eax, 0x960(%rsi)
    jmp guest_snapshot_done
guest_xapic_snapshot:
    mov $0xfee00000, %ebx
    mov 0xf0(%rbx), %eax
    mov %eax, 0x834(%rsi)
    mov 0x300(%rbx), %eax
    mov %eax, 0x838(%rsi)
    mov 0x310(%rbx), %eax
    mov %eax, 0x83c(%rsi)
    mov 0x80(%rbx), %eax
    mov %eax, 0x840(%rsi)
    mov 0x380(%rbx), %eax
    mov %eax, 0x960(%rsi)
guest_snapshot_done:
    mov $0x4fff0000, %eax
    cpuid
    mov %eax, 0x844(%rsi)
    /* Seed distinct canonical addresses and status before the next INIT.
     * All DR7 breakpoint enables and GD remain zero, so no #DB is armed. */
    mov $0x11111000, %eax
    mov %rax, %dr0
    mov $0x22222000, %eax
    mov %rax, %dr1
    mov $0x33333000, %eax
    mov %rax, %dr2
    mov $0x44444000, %eax
    mov %rax, %dr3
    mov $0xffff0ff1, %eax
    mov %rax, %dr6
    mov $0x10400, %eax
    mov %rax, %dr7
    mov %dr0, %rax
    mov %rax, 0x900(%rsi)
    mov %dr1, %rax
    mov %rax, 0x908(%rsi)
    mov %dr2, %rax
    mov %rax, 0x910(%rsi)
    mov %dr3, %rax
    mov %rax, 0x918(%rsi)
    mov %dr6, %rax
    mov %rax, 0x920(%rsi)
    mov %dr7, %rax
    mov %rax, 0x928(%rsi)
    /* Seed resettable LAPIC configuration for the next INIT. Software enable
     * stays clear, every LVT stays masked, and no EOI or interrupt is issued. */
    mov $0x1b, %ecx
    rdmsr
    test $0x400, %eax
    jz guest_xapic_reset_seed
    xor %edx, %edx
    mov $0x808, %ecx
    mov $0xf0, %eax
    wrmsr
    rdmsr
    mov %eax, 0x964(%rsi)
    mov $0x838, %ecx
    mov $0x7fffffff, %eax
    wrmsr
    rdmsr
    mov %eax, 0x968(%rsi)
    jmp guest_reset_seed_done
guest_xapic_reset_seed:
    mov $0xfee00000, %ebx
    movl $0xf0, 0x80(%rbx)
    mov 0x80(%rbx), %eax
    mov %eax, 0x964(%rsi)
    movl $0x7fffffff, 0x380(%rbx)
    mov 0x380(%rbx), %eax
    mov %eax, 0x968(%rsi)
guest_reset_seed_done:
    incl 0x804(%rsi)
1:  mov %dr0, %rax
    mov %rax, 0x930(%rsi)
    mov %dr1, %rax
    mov %rax, 0x938(%rsi)
    mov %dr2, %rax
    mov %rax, 0x940(%rsi)
    mov %dr3, %rax
    mov %rax, 0x948(%rsi)
    mov %dr6, %rax
    mov %rax, 0x950(%rsi)
    mov %dr7, %rax
    mov %rax, 0x958(%rsi)
    incq 0x848(%rsi)
#if SVMVISOR_GUEST_CACHE
    cmpl $7, 0x850(%rsi)
    je guest_cache_entry
    cmpl $8, 0x850(%rsi)
    je guest_cache_init_source
#endif
    cmpl $1, 0x850(%rsi)
    je guest_irq_setup
    pause
    jmp 1b
#if SVMVISOR_GUEST_CACHE
guest_cache_entry:
    mov 0x980(%rsi), %rax
    mov 0x988(%rsi), %rsp
    and $-16, %rsp
    sub $40, %rsp
    jmp *%rax
guest_cache_init_source:
    mov $0x1b, %ecx
    rdmsr
    or $0x400, %eax
    wrmsr
    mov $0x830, %ecx
    mov $0x4500, %eax
    xor %edx, %edx
    wrmsr
9:  pause
    jmp 9b
#endif
guest_irq_setup:
    /* Upgrade after the reset witness; the IRQ workload uses native x2APIC. */
    mov $0x1b, %ecx
    rdmsr
    or $0x400, %eax
    wrmsr
    movabs $0x123456789abcdef0, %rax
    movq %rax, %xmm0
    lea 0x1000(%rsi), %rax
    movw $4095, 0x8a0(%rsi)
    mov %rax, 0x8a2(%rsi)
    lidt 0x8a0(%rsi)
    mov $0x80f, %ecx
    mov $0x1ff, %eax
    xor %edx, %edx
    wrmsr
    mov $0x808, %ecx
    mov $0xf0, %eax
    wrmsr
    movl $1, 0x854(%rsi)
2:  mov $0x808, %ecx
    rdmsr
    mov %eax, 0x860(%rsi)
    mov $0x827, %ecx
    rdmsr
    mov %eax, 0x864(%rsi)
    mov $0x81f, %ecx
    rdmsr
    mov %eax, 0x878(%rsi)
    movq %xmm0, %rax
    mov %rax, 0x870(%rsi)
    incq 0x848(%rsi)
    cmpl $2, 0x850(%rsi)
    jne 2b
    mov $0x808, %ecx
    xor %eax, %eax
    xor %edx, %edx
    wrmsr
    sti
3:  pause
    cmpl $3, 0x850(%rsi)
    jne 3b
    cli
    mov $0x817, %ecx
    rdmsr
    mov %eax, 0x868(%rsi)
    mov $0x827, %ecx
    rdmsr
    mov %eax, 0x864(%rsi)
    movl $3, 0x854(%rsi)
4:  pause
    cmpl $4, 0x850(%rsi)
    jne 4b
    mov $0x80f, %ecx
    mov $0xff, %eax
    xor %edx, %edx
    wrmsr
    movl $4, 0x854(%rsi)
5:  mov $0x80f, %ecx
    rdmsr
    mov %eax, 0x86c(%rsi)
    movq %xmm0, %rax
    mov %rax, 0x870(%rsi)
    incq 0x848(%rsi)
    pause
    jmp 5b
guest_irq_handler:
    push %rax
    push %rcx
    push %rdx
    incl 0x858(%rsi)
    mov $0x817, %ecx
    rdmsr
    mov %eax, 0x868(%rsi)
    movl $1, 0x85c(%rsi)
6:  incq 0x848(%rsi)
    movq %xmm0, %rax
    mov %rax, 0x870(%rsi)
    pause
    cmpl $5, 0x850(%rsi)
    je guest_held_disable
    cmpl $3, 0x850(%rsi)
    jne 6b
    mov $0x80b, %ecx
    xor %eax, %eax
    xor %edx, %edx
    wrmsr
    pop %rdx
    pop %rcx
    pop %rax
    iretq
guest_held_disable:
    mov $0x80f, %ecx
    mov $0xff, %eax
    xor %edx, %edx
    wrmsr
    rdmsr
    mov %eax, 0x86c(%rsi)
    mov $0x817, %ecx
    rdmsr
    mov %eax, 0x868(%rsi)
    movl $5, 0x85c(%rsi)
7:  pause
    jmp 7b
.balign 8
guest_gdt:
    .quad 0, 0x00af9b000000ffff, 0x00cf93000000ffff, 0x00cf9b000000ffff
guest_gdtr:
    .word 31
guest_gdt_base: .long 0
guest_end:
