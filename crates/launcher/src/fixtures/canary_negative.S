/* Deliberately corrupting TCG-only detector test, never a real transition.
 * Link this INSTEAD OF native_transition.S only for the explicit negative
 * fixture. The context and journal are untouched: outcome remains NOT_RUN.
 * Entry must come from native_transition_canary with its qualified, seeded
 * profile 0/3/7. Only local stack memory is written; no CR/MSR writes or SVM.
 * Expected canary failures: 0x7b for profiles 0/3, 0xfb for profile 7.
 */
.text
.p2align 4
.globl svmvisor_native_transition
svmvisor_native_transition:
    pushfq
    push %rax
    push %rbx
    push %rcx
    push %rdx
    /* Local bytes 0..15. Saved RDX/RCX/RBX/RAX/flags at16/24/32/40/48;
       return PC at56. No calls and no changes to any other saved GPR. */
    sub $16, %rsp
    mov %cs, %ax
    and $3, %eax
    jnz .Lntcn_refuse
    mov $0x40000000, %eax
    xor %ecx, %ecx
    cpuid
    cmp $0x54474354, %ebx
    jne .Lntcn_refuse
    cmp $0x43544743, %ecx
    jne .Lntcn_refuse
    cmp $0x47435447, %edx
    jne .Lntcn_refuse
    mov 24(%rsp), %rcx
    test %rcx, %rcx
    jz .Lntcn_refuse
    test $63, %rcx
    jnz .Lntcn_refuse
    cmpq $1, (%rcx)
    jne .Lntcn_refuse
    cmpq $1088, 8(%rcx)
    jne .Lntcn_refuse
    mov 120(%rcx), %rax
    test %rax, %rax
    jz .Lntcn_corrupt
    cmp $3, %rax
    je .Lntcn_corrupt
    cmp $7, %rax
    jne .Lntcn_refuse
.Lntcn_corrupt:
    /* Change rounding controls without changing instruction-enabling state. */
    fnstcw (%rsp)
    xorw $0x0400, (%rsp)
    fldcw (%rsp)
    stmxcsr 4(%rsp)
    xorl $0x2000, 4(%rsp)
    ldmxcsr 4(%rsp)
    /* All slots were seeded nonempty. Pop then push leaves TOP unchanged and
       replaces the original finite nonzero ST0 payload with a valid zero. */
    fstp %st(0)
    fldz
    pxor %xmm0, %xmm0
    cmpq $7, 120(%rcx)
    jne .Lntcn_corrupt_return
    /* AVX1 suffices. Do not execute this with an inactive AVX profile. */
    vxorps %ymm0, %ymm0, %ymm0
.Lntcn_corrupt_return:
    add $16, %rsp
    pop %rdx
    pop %rcx
    pop %rbx
    pop %rax
    popfq
    /* MOV/NOT do not alter flags; CMC changes exactly the incoming CF. */
    movabs $0xbad0000000000001, %rax
    not %r11
    cmc
    ret
.Lntcn_refuse:
    add $16, %rsp
    pop %rdx
    pop %rcx
    pop %rbx
    pop %rax
    popfq
    ret
