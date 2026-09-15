/* Disposable UEFI execution fixture. No return, physical devices or guest OS. */
.text
.globl efi_main
efi_main:
    cli
    mov $0xc0000080, %ecx
    rdmsr
    or $0x1000, %eax
    wrmsr
    clgi
    lea host_stack_top(%rip), %rsp
    /* Available long-mode TSS descriptor, limit103, base from retained image. */
    lea tss(%rip), %rax
    mov %ax, gdt+26(%rip)
    shr $16, %rax
    mov %al, gdt+28(%rip)
    shr $8, %rax
    mov %al, gdt+31(%rip)
    shr $8, %rax
    mov %eax, gdt+32(%rip)
    lea fault_stack_top(%rip), %rax
    mov %rax, tss+36(%rip)
    movw $104, tss+102(%rip)
    lea gdt(%rip), %rax
    mov %rax, gdtr+2(%rip)
    lea idt(%rip), %rax
    mov %rax, idtr+2(%rip)
    lea svmvisor_resident_fault_offsets(%rip), %r8
    lea idt(%rip), %rdi
    xor %ecx, %ecx
1:
    movslq (%r8,%rcx,4), %rax
    add %r8, %rax
    mov %ax, (%rdi)
    movw $8, 2(%rdi)
    movw $0x8e01, 4(%rdi)
    shr $16, %rax
    mov %ax, 6(%rdi)
    shr $16, %rax
    mov %eax, 8(%rdi)
    add $16, %rdi
    inc %ecx
    cmp $256, %ecx
    jne 1b
    lgdt gdtr(%rip)
    pushq $8
    lea private_cs(%rip), %rax
    push %rax
    lretq
private_cs:
    mov $16, %eax
    mov %ax, %ss
    mov %ax, %ds
    mov %ax, %es
    mov $24, %eax
    ltr %ax
    lidt idtr(%rip)
    mov %cr3, %rax
    mov %rax, expected_cr3(%rip)
    lea trigger(%rip), %rax
    mov %rax, expected_rip(%rip)
#if FAULT_KIND == 3
    /* Canonical upper-half address absent in the pinned OVMF page tables. */
    movabs $0xffff800000000000, %rax
#elif FAULT_KIND == 2 || FAULT_KIND == 4
    movabs $0x0000800000000000, %rax
#endif
#if FAULT_KIND == 4
    mov %rax, %rsp
#endif
    mov %rsp, expected_rsp(%rip)
trigger:
#if FAULT_KIND == 1 || FAULT_KIND == 4
    ud2
#else
    mov (%rax), %rax
#endif
    jmp failed

/* Real production handler calls this with copied frame and CR2/CR3. */
.globl svmvisor_resident_host_fault
svmvisor_resident_host_fault:
    lea svmvisor_resident_fault_record(%rip), %rax
    cmp %rax, %rdi
    jne failed
#if FAULT_KIND == 2
    cmpq $13, (%rdi)
#elif FAULT_KIND == 3
    cmpq $14, (%rdi)
#else
    cmpq $6, (%rdi)
#endif
    jne failed
    cmpq $0, 8(%rdi)
    jne failed
    mov expected_rip(%rip), %rax
    cmp %rax, 16(%rdi)
    jne failed
    mov expected_rsp(%rip), %rax
    cmp %rax, 40(%rdi)
    jne failed
    mov expected_cr3(%rip), %rax
    cmp %rax, %rdx
    jne failed
    cmp %rdx, 64(%rdi)
    jne failed
    cmp %rsi, 56(%rdi)
    jne failed
#if FAULT_KIND == 3
    movabs $0xffff800000000000, %rax
    cmp %rax, %rsi
    jne failed
#endif
    testq $2, 32(%rdi)
    jz failed
    lea pass_message(%rip), %rsi
    mov $0xe9, %edx
2:  lodsb
    test %al, %al
    jz 3f
    out %al, %dx
    jmp 2b
3:  mov $0xf4, %edx
    mov $16, %eax
    out %eax, %dx
    jmp stop
failed:
    mov $0xe9, %edx
    mov $0x46, %eax
    out %al, %dx
    mov $0xf4, %edx
    mov $17, %eax
    out %eax, %dx
stop:
    cli
    hlt
    jmp stop
.section .rdata
pass_message: .asciz "PASS actual-host-fault-frame\n"
.data
.p2align 4
gdt:
    .quad 0, 0x00af9b000000ffff, 0x00cf93000000ffff
    .quad 0x0000890000000067, 0
gdtr: .word 39
    .quad 0
idtr: .word 4095
    .quad 0
.bss
.p2align 4
tss: .zero 104
expected_cr3: .zero 8
expected_rip: .zero 8
expected_rsp: .zero 8
.p2align 12
idt: .zero 4096
host_stack: .zero 16384
host_stack_top:
fault_stack: .zero 16384
fault_stack_top:
