/* Explicit QEMU-only OSXSAVE fixture. Encloses a complete EFI callback; no
 * Rust code can retain live values across a separate capture/restore pair.
 * See abi.md. Admission must validate CPU/capabilities before calling. */
.text
.macro fixture_push
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
.endm
.macro fixture_pop
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
.endm
.globl fixture_entry
fixture_entry:
    fixture_push
    sub $40, %rsp
    mov %rcx, 32(%rsp)
    mov %rcx, %r10
    movq $1, 56(%r10)
    movq $0, 112(%r10)
    mov 160(%rsp), %rax
    mov %rax, 96(%r10)
    mov %cr0, %rax
    mov %rax, 120(%r10)
    mov %cr4, %rax
    mov %rax, 64(%r10)
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 136(%r10)
    mov 24(%r10), %r11
    fxsave64 (%r11) /* Always preserve SSE, even when original XCR0=1. */
    cli
    mov 64(%r10), %rax
    or $0x40000, %rax
    mov %rax, %cr4
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 72(%r10)
    cmp $1, %rax
    je .Lfixture_mask_valid
    cmp $3, %rax
    je .Lfixture_mask_valid
    cmp $7, %rax
    jne .Lfixture_refuse
.Lfixture_mask_valid:
    mov 16(%r10), %rdx
    not %rdx
    test %rdx, %rax
    jnz .Lfixture_refuse /* Never reduce an existing enabled component. */
    mov 32(%r10), %r11
    xor %edx, %edx
    xsave64 (%r11) /* Original enabled state before changing XCR0. */
    xor %ecx, %ecx
    mov 16(%r10), %rax
    xor %edx, %edx
    xsetbv
    /* Caller-side nonvolatile ABI canaries around the entire Rust callback. */
    movdqa .Lfixture_vector(%rip), %xmm6
    movdqa %xmm6, %xmm7
    movdqa %xmm6, %xmm8
    movdqa %xmm6, %xmm9
    movdqa %xmm6, %xmm10
    movdqa %xmm6, %xmm11
    movdqa %xmm6, %xmm12
    movdqa %xmm6, %xmm13
    movdqa %xmm6, %xmm14
    movdqa %xmm6, %xmm15
    mov $0x667701, %ebx
    mov $0x667702, %ebp
    mov $0x667703, %esi
    mov $0x667704, %edi
    mov $0x667705, %r12d
    mov $0x667706, %r13d
    mov $0x667707, %r14d
    mov $0x667708, %r15d
    mov 0(%r10), %rax
    mov 8(%r10), %rcx
    pushq 96(%r10)
    popfq /* Callback sees incoming firmware IF/DF, no forced CLI ABI. */
    call *%rax
    mov 32(%rsp), %r10
    xor %r11d, %r11d
    cmp $0x667701, %rbx
    jne .Lfixture_gpr_bad
    cmp $0x667702, %rbp
    jne .Lfixture_gpr_bad
    cmp $0x667703, %rsi
    jne .Lfixture_gpr_bad
    cmp $0x667704, %rdi
    jne .Lfixture_gpr_bad
    cmp $0x667705, %r12
    jne .Lfixture_gpr_bad
    cmp $0x667706, %r13
    jne .Lfixture_gpr_bad
    cmp $0x667707, %r14
    jne .Lfixture_gpr_bad
    cmp $0x667708, %r15
    je .Lfixture_gpr_good
.Lfixture_gpr_bad:
    or $1, %r11d
.Lfixture_gpr_good:
.macro fixture_check_xmm register
    movdqa .Lfixture_vector(%rip), %xmm0
    pcmpeqb \register, %xmm0
    pmovmskb %xmm0, %eax
    cmp $65535, %eax
    je 1f
    or $2, %r11d
1:
.endm
    fixture_check_xmm %xmm6
    fixture_check_xmm %xmm7
    fixture_check_xmm %xmm8
    fixture_check_xmm %xmm9
    fixture_check_xmm %xmm10
    fixture_check_xmm %xmm11
    fixture_check_xmm %xmm12
    fixture_check_xmm %xmm13
    fixture_check_xmm %xmm14
    fixture_check_xmm %xmm15
    cli
    mov %cr4, %rax
    mov 64(%r10), %rdx
    or $0x40000, %rdx
    cmp %rdx, %rax
    je .Lfixture_cr4_good
    or $4, %r11d
.Lfixture_cr4_good:
    mov %rdx, %cr4 /* Establish XSAVE access even if callback broke contract. */
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    cmp 16(%r10), %rax
    je .Lfixture_xcr_good
    or $4, %r11d
.Lfixture_xcr_good:
    mov %r11, 112(%r10)
    xor %ecx, %ecx
    mov 72(%r10), %rax
    xor %edx, %edx
    xsetbv
    mov 32(%r10), %r11
    mov 72(%r10), %rax
    xor %edx, %edx
    xrstor64 (%r11)
    mov 24(%r10), %r11
    fxrstor64 (%r11) /* Restores original SSE even for original XCR0 bit1=0. */
    mov 48(%r10), %r11
    mov 72(%r10), %rax
    xor %edx, %edx
    xsave64 (%r11) /* Independent recapture, no copied success receipt. */
    mov 40(%r10), %r11
    fxsave64 (%r11)
    mov 24(%r10), %r11
    fxrstor64 (%r11) /* Final vector/FPU operation before actual ABI return. */
    movq $0, 56(%r10)
    jmp .Lfixture_restore_controls
.Lfixture_refuse:
    /* Unknown/original-outside-target: no XSETBV and no SIMD changed state.
       Retain status1 and restore the temporary CR4.OSXSAVE bit. */
    mov 40(%r10), %r11
    fxsave64 (%r11)
.Lfixture_restore_controls:
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 88(%r10)
    mov 64(%r10), %rax
    mov %rax, %cr4
    mov %cr4, %rax
    mov %rax, 80(%r10)
    mov %cr0, %rax
    mov %rax, 128(%r10)
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 144(%r10)
    pushq 96(%r10)
    popfq
    pushfq
    popq 104(%r10)
    lea 40(%rsp), %rsp /* LEA preserves final restored flags. */
    fixture_pop
    ret

.section .rdata,"dr"
.p2align 4
.Lfixture_vector:
    .quad 0x0123fedc4567ba98, 0x7654abcd321089ef
