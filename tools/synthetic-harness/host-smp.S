#ifdef CONCURRENT_SMP
/* Emulator-only INIT/SIPI trampoline. Copied to an admitted low physical page.
 * APM2 rev3.44 14.1/14.1.3 and 16.5: INIT real mode, SIPI vector entry.
 * Page-dependent fields are patched while stopped/RW-NX. Image-dependent
 * absolute pointers use the existing ELF relocation package before copying.
 */
.section .rodata.host_smp,"a"
.code16
.global host_smp_trampoline, host_smp_trampoline_end
host_smp_trampoline:
    cli
    cld
    mov %cs, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    lgdt (ap_gdt_pointer-host_smp_trampoline)
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0
    .byte 0x66, 0xea
.global host_smp_protected_target
host_smp_protected_target: .long 0
    .word 24
.code32
.global host_smp_protected
host_smp_protected:
ap_protected:
    mov $16, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    .byte 0xb8
.global host_smp_root_value
host_smp_root_value: .long 0
    mov %eax, %cr3
    mov $0x620, %eax
    mov %eax, %cr4
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax
    wrmsr
    mov %cr0, %eax
    and $0xfffffff3, %eax
    or $0x80010003, %eax
    mov %eax, %cr0
    ljmp $8, $host_smp_entry64
.balign 8
.global host_smp_boot_gdt
host_smp_boot_gdt:
    .quad 0, 0x00af9b000000ffff, 0x00cf93000000ffff, 0x00cf9b000000ffff
ap_gdt_pointer:
    .word 31
.global host_smp_gdt_base
host_smp_gdt_base: .long 0
host_smp_trampoline_end:

.section .text,"ax"
.code64
host_smp_entry64:
    mov $host_ap_stack_top, %rsp
    xor %ebp, %ebp
    fninit
    ldmxcsr ap_initial_mxcsr(%rip)
    call host_smp_ap_main
    ud2

/* Every gate owns a distinct ISR bit and EOI. Host APIC IDs are fixed 0/1.
 * Preserve all registers used, hardware frame and flags, without SIMD or calls.
 * APM2 rev3.44 16.4: ISR is set by acknowledgement; EOI clears highest priority.
 * Stacked post-HLT RIP proves HLT executed and this source resumed it; it does
 * not measure a minimum sleeping duration (the IRQ may have been pending).
 */
.macro host_source_handler name, ack, wake, bit
.global \name, \name\()_end
\name:
    push %rax
    push %rdx
    mov $0xfee00170, %eax
    cmpl $\bit, (%rax)
    jne ap_bad_id
    mov $0xfee00020, %eax
    mov (%rax), %eax
    shr $24, %eax
    cmp $1, %eax
    ja ap_bad_id
    lea \ack(%rip), %rdx
    lock incq (%rdx,%rax,8)
    lea host_smp_idle_after_hlt(%rip), %rdx
    cmp %rdx, 16(%rsp)
    jne 1f
    lea \wake(%rip), %rdx
    lock incq (%rdx,%rax,8)
1:  mov $0xfee000b0, %eax
    movl $0, (%rax)
    mov $0xfee00170, %eax
    cmpl $0, (%rax)
    jne ap_bad_id
    pop %rdx
    pop %rax
    iretq
\name\()_end:
.endm
host_source_handler host_smp_ipi_handler, host_smp_acks, host_smp_idle_ipi, 0x20000
host_source_handler host_smp_timer_handler, host_smp_timer_acks, host_smp_idle_timer, 0x10000
ap_bad_id:
    ud2

/* APM3 rev3.37 HLT p388 and STI p477. IF=0 on entry and return, GIF=1;
 * one armed F0 watchdog plus pending F1 accepted, both private integer gates.
 * Adjacent STI/HLT prevents interrupt acceptance in the empty-check/HLT gap.
 */
.global host_smp_idle_start, host_smp_idle_after_hlt, host_smp_idle_end
host_smp_idle_start:
    sti
    hlt
host_smp_idle_after_hlt:
    cli
    ret
host_smp_idle_end:

/* Permanent terminal park has no timed work. INIT-only AP is never resumed. */
.global host_smp_park
host_smp_park:
    cli
1:  hlt
    jmp 1b
.section .rodata,"a"
.balign 4
ap_initial_mxcsr: .long 0x1f80
.section .bss,"aw",@nobits
.balign 4096
.global host_ap_save
host_ap_save: .skip 4096
.global host_ap_stack_guard, host_ap_stack_high_guard, host_ap_stack_top
host_ap_stack_guard: .skip 4096
host_ap_stack: .skip 65536
host_ap_stack_top:
host_ap_stack_high_guard: .skip 4096
.global host_ap_df_guard, host_ap_df_high_guard, host_ap_df_stack_end
host_ap_df_guard: .skip 4096
host_ap_df_stack: .skip 16384
host_ap_df_stack_end:
host_ap_df_high_guard: .skip 4096
.section .note.GNU-stack,"",@progbits

#endif /* CONCURRENT_SMP: no AP backing or entry code in legacy images. */
