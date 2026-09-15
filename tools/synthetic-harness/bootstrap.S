/* Emulator-only backend smoke; no firmware entry point or native executable.
 * Build a flat Multiboot image consumed exclusively by QEMU's -kernel loader.
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
    mov $0x20, %eax
    mov %eax, %cr4
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax /* LME and NXE. SVME is set after capability checks. */
    wrmsr
    mov %cr0, %eax
    or $0x80010001, %eax
    mov %eax, %cr0
    ljmp $8, $entry64

.code64
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

    movl $0x01000000, vmcb+0x00c /* HLT */
    movl $1, vmcb+0x010 /* Required VMRUN intercept */
    movl $1, vmcb+0x058 /* ASID */
    movb $1, vmcb+0x05c /* Flush all ASIDs */
    movq $1, vmcb+0x090 /* NPT enabled */
    movq $nested_pml4, vmcb+0x0b0
    /* ES, CS, SS, DS: flat ring-0 segments. */
    movw $0x10, vmcb+0x400
    movw $0xc93, vmcb+0x402
    movl $-1, vmcb+0x404
    movw $0x08, vmcb+0x410
    movw $0xa9b, vmcb+0x412
    movl $-1, vmcb+0x414
    movw $0x10, vmcb+0x420
    movw $0xc93, vmcb+0x422
    movl $-1, vmcb+0x424
    movw $0x10, vmcb+0x430
    movw $0xc93, vmcb+0x432
    movl $-1, vmcb+0x434
    movl $(gdt_end-gdt-1), vmcb+0x464
    movq $gdt, vmcb+0x468
    movq $0x1500, vmcb+0x4d0
    movq $0x20, vmcb+0x548
    movq $host_pml4, vmcb+0x550
    movl $0x80010033, vmcb+0x558
    movq $0x400, vmcb+0x560 /* DR7 */
    movl $0xffff0ff0, vmcb+0x568 /* DR6 */
    movq $2, vmcb+0x570
    movq $guest_entry, vmcb+0x578
    movq $guest_stack_top, vmcb+0x5d8
    mov $msg_run, %rsi
    call print
    mov $vmcb, %rax
    vmrun %rax
    mov $msg_exit, %rsi
    call print
    mov vmcb+0x070, %rax
    call print_hex
    mov $msg_newline, %rsi
    call print
    cmpq $0x78, vmcb+0x070
    jne fail
    cmpq $guest_entry, vmcb+0x578
    jne fail
    mov $msg_pass, %rsi
    call print
    mov $0x10, %eax
    mov $0xf4, %dx
    out %eax, %dx /* Host process exits with (0x10 << 1) | 1 = 33. */
2:  cli
    hlt
    jmp 2b
fail:
    mov $msg_fail, %rsi
    call print
    mov $0x11, %eax
    mov $0xf4, %dx
    out %eax, %dx
    jmp 2b
print:
    mov $0xe9, %dx
3:  lodsb
    test %al, %al
    jz 4f
    out %al, %dx
    jmp 3b
4:  ret
print_hex:
    mov %rax, %r8
    mov $16, %ecx
    mov $0xe9, %dx
5:  rol $4, %r8
    mov %r8b, %al
    and $15, %al
    add $48, %al
    cmp $57, %al
    jbe 6f
    add $39, %al
6:  out %al, %dx
    loop 5b
    ret

guest_entry:
    hlt
    ud2

.section .rodata,"a"
.balign 8
gdt:
    .quad 0, 0x00af9b000000ffff, 0x00cf93000000ffff
gdt_end:
gdt_pointer:
    .word gdt_end-gdt-1
    .long gdt
msg_boot: .asciz "boot64\n"
msg_svm: .asciz "svm_available npt_available\n"
msg_run: .asciz "vmrun\n"
msg_exit: .asciz "vmexit="
msg_newline: .asciz "\n"
msg_pass: .asciz "PASS svm-npt-hlt\n"
msg_fail: .asciz "FAIL svm-npt-hlt\n"

.section .bss,"aw",@nobits
.balign 4096
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
host_stack: .skip 65536
host_stack_top:
