/* Independent TCG launcher canaries around the loaded subsystem-11 PE entry.
 * No driver context/layout is shared. Entry must return EFI_UNSUPPORTED.
 * FX starts with OSXSAVE=0. XSAVE fixtures enclose temporary OSXSAVE/XCR0
 * configuration and restore it before Rust resumes; TCG-only admission in Rust.
 */
.text
.globl boundary_call
boundary_call:
    pushfq
    push %rax
    push %rcx
    push %rdx
    push %rbx
    push %rbp
    push %rsi
    push %rdi
    push %r8
    push %r9
    push %r10
    push %r11
    push %r12
    push %r13
    push %r14
    push %r15
    sub $56, %rsp
    mov %rcx, 32(%rsp)
    mov %rcx, %r10
    mov 0(%r10), %rax
    mov %rax, 40(%rsp)
    mov 24(%r10), %r11
    fxsave64 (%r11)
    mov %cr4, %rax
    mov %rax, 88(%r10)
    cmpq $0, 80(%r10)
    je .Lseed_fx
    cli
    or $0x40000, %rax
    mov %rax, %cr4
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 96(%r10)
    cmp $1, %rax
    je .Lold_mask_valid
    cmp $3, %rax
    je .Lold_mask_valid
    cmp $7, %rax
    jne .Lfixture_refuse
.Lold_mask_valid:
    mov 80(%r10), %rdx
    not %rdx
    test %rdx, %rax
    jnz .Lfixture_refuse
    mov 104(%r10), %r11
    xor %edx, %edx
    xsave64 (%r11)
    mov 80(%r10), %rax
    xor %ecx, %ecx
    xor %edx, %edx
    xsetbv
    mov 32(%r10), %r11
    fxrstor64 (%r11) /* Also seed SSE for the deliberate XCR0=1 refusal. */
    mov 80(%r10), %eax
    xor %edx, %edx
    xrstor64 (%r11)
    jmp .Lseed_done
.Lseed_fx:
    mov 32(%r10), %r11
    fxrstor64 (%r11)
.Lseed_done:
    pushq 176(%rsp)
    popfq
    pushfq
    popq 64(%r10)
    mov 8(%r10), %rcx
    mov 16(%r10), %rdx
    mov $0x7703, %ebx
    mov $0x7704, %ebp
    mov $0x7705, %esi
    mov $0x7706, %edi
    mov $0x7707, %r8d
    mov $0x7708, %r9d
    mov $0x770a, %r11d
    mov $0x770b, %r12d
    mov $0x770c, %r13d
    mov $0x770d, %r14d
    mov $0x770e, %r15d
    mov $0x7709, %r10d
    mov $0x7700, %eax
    call *40(%rsp)
    /* Capture returned flags before any comparison modifies them. */
    pushfq
    popq 48(%rsp)
    push %r10
    mov 40(%rsp), %r10
    mov %rax, 48(%r10)
    mov 56(%rsp), %rax
    mov %rax, 72(%r10)
    movq $0, 56(%r10)
    cmp $0x7703, %rbx
    jne .Lbad_gpr
    cmp $0x7704, %rbp
    jne .Lbad_gpr
    cmp $0x7705, %rsi
    jne .Lbad_gpr
    cmp $0x7706, %rdi
    jne .Lbad_gpr
    cmp $0x7707, %r8
    jne .Lbad_gpr
    cmp $0x7708, %r9
    jne .Lbad_gpr
    cmpq $0x7709, (%rsp)
    jne .Lbad_gpr
    cmp $0x770a, %r11
    jne .Lbad_gpr
    cmp $0x770b, %r12
    jne .Lbad_gpr
    cmp $0x770c, %r13
    jne .Lbad_gpr
    cmp $0x770d, %r14
    jne .Lbad_gpr
    cmp $0x770e, %r15
    jne .Lbad_gpr
    cmp 8(%r10), %rcx
    jne .Lbad_gpr
    cmp 16(%r10), %rdx
    je .Lcapture
.Lbad_gpr:
    movq $1, 56(%r10)
.Lcapture:
    mov 40(%r10), %r11
    cmpq $0, 80(%r10)
    je .Lcapture_fx
    mov 80(%r10), %eax
    xor %edx, %edx
    xsave64 (%r11)
    fxsave64 (%r11) /* Defined legacy state, even when XCR0 bit1 is clear. */
    cli
    mov 96(%r10), %rax
    xor %ecx, %ecx
    xor %edx, %edx
    xsetbv
    mov 104(%r10), %r11
    mov 96(%r10), %eax
    xor %edx, %edx
    xrstor64 (%r11)
    jmp .Lrestore_fx
.Lcapture_fx:
    fxsave64 (%r11)
.Lrestore_fx:
    mov 24(%r10), %r11
    fxrstor64 (%r11)
    mov 88(%r10), %rax
    mov %rax, %cr4
    add $64, %rsp
    jmp .Lfixture_return
.Lfixture_refuse:
    movq $2, 56(%r10)
    mov 88(%r10), %rax
    mov %rax, %cr4
    add $56, %rsp
.Lfixture_return:
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rdi
    pop %rsi
    pop %rbp
    pop %rbx
    pop %rdx
    pop %rcx
    pop %rax
    popfq
    ret
