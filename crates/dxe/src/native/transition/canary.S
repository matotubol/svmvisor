/* Immediate canaries around the exact native transition symbol.
 * EFI ABI: RCX=context, RDX=separate aligned3328-byte buffer; RAX=failure mask.
 * Default admission is TCG; native-returning selects exact-target admission.
 * Reads EFER; no CR/MSR/XCR0 writes, firmware calls, or external data relocations.
 * This is instrumentation, not an exception/invalid-return-stack recovery path.
 */
.text
.p2align 4
.set NTC_ORIGINAL, 256
.set NTC_SEEDED, 1280
.set NTC_OBSERVED, 2304

.macro ntc_push_gprs
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

.macro ntc_pop_gprs
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
.endm

.globl svmvisor_native_transition_canary
svmvisor_native_transition_canary:
    pushfq
    ntc_push_gprs
    /* B=RSP after184 bytes. Original flags B+176, RAX B+168, returnPC B+184.
       B is16-byte call aligned; B+0..31 is EFI shadow space.
       The nested post-return capture uses128 more bytes: total312, plus CALL.
       The transition's own stack use is additional and belongs to its ABI. */
    sub $56, %rsp
    mov %rcx, 32(%rsp)
    mov %rdx, 40(%rsp)
    mov %rdx, %r10
    test %r10, %r10
    jz .Lntc_bad_buffer
    test $63, %r10
    jnz .Lntc_bad_buffer
    cld
    mov %r10, %rdi
    xor %eax, %eax
    mov $416, %ecx
    rep stosq
    movq $1, 0(%r10)
    movq $3328, 8(%r10)
    btsq $63, 16(%r10) /* Remains SETUP until original xstate is captured. */
    mov 32(%rsp), %r11
    mov %r11, 184(%r10)
    test %r11, %r11
    jz .Lntc_refuse
    test $63, %r11
    jnz .Lntc_refuse
    cmpq $1, 0(%r11)
    jne .Lntc_refuse
    cmpq $1088, 8(%r11)
    jne .Lntc_refuse
    mov 176(%rsp), %rax
    mov %rax, 32(%r10)
    test $0x44700, %eax /* TF/IF/DF/NT/AC must already be clear. */
    jnz .Lntc_refuse
    mov %rsp, 48(%r10)
    mov %cs, %ax
    and $3, %eax
    jnz .Lntc_refuse
#ifdef SVMVISOR_NATIVE_RETURNING
    /* Match the native cache reader's exact CPU identity before control/MSR
       reads. This does not replace the caller's live resource admission. */
    xor %eax, %eax
    xor %ecx, %ecx
    cpuid
    cmp $1, %eax
    jb .Lntc_refuse
    cmp $0x68747541, %ebx
    jne .Lntc_refuse
    cmp $0x69746e65, %edx
    jne .Lntc_refuse
    cmp $0x444d4163, %ecx
    jne .Lntc_refuse
#else
    /* Keep the explicit fixture guard even if a caller forgets its own check. */
    mov $0x40000000, %eax
    xor %ecx, %ecx
    cpuid
    cmp $0x54474354, %ebx
    jne .Lntc_refuse
    cmp $0x43544743, %ecx
    jne .Lntc_refuse
    cmp $0x47435447, %edx
    jne .Lntc_refuse
#endif
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
#ifdef SVMVISOR_NATIVE_RETURNING
    cmp $0x00b40f40, %eax /* Family1Ah Model44h B0; pinned native target. */
    jne .Lntc_refuse
    test $0x80000000, %ecx /* Require no reported hypervisor. */
    jnz .Lntc_refuse
#endif
    mov %ecx, %r8d
    and $0x07000020, %edx /* MSR/FXSR/SSE/SSE2. */
    cmp $0x07000020, %edx
    jne .Lntc_refuse
    mov %cr0, %rax
    mov %rax, 192(%r10)
    test $12, %eax
    jnz .Lntc_refuse
    mov %cr4, %rax
    mov %rax, 200(%r10)
    test $0x200, %eax
    jz .Lntc_refuse
    mov $0xc0000080, %ecx
    rdmsr
    test $0x4000, %eax /* FFXSR would omit CPL0 XMM state. */
    jnz .Lntc_refuse
    mov 16(%r11), %r9 /* Immutable qualified NativeBoundary. */
    test %r9, %r9
    jz .Lntc_refuse
    test $63, %r9
    jnz .Lntc_refuse
    cmpq $1, 0(%r9)
    jne .Lntc_refuse
    mov 120(%r11), %rax
    mov %rax, 24(%r10)
    cmp 16(%r9), %rax
    jne .Lntc_refuse
    mov 128(%r11), %rdx
    cmp 24(%r9), %rdx
    jne .Lntc_refuse
    test %rax, %rax
    jz .Lntc_fx_setup
    cmp $3, %eax
    je .Lntc_xsave_setup
    cmp $7, %eax
    jne .Lntc_refuse
    test $0x10000000, %r8d
    jz .Lntc_refuse
    cmpl $576, 328(%r9)
    jne .Lntc_refuse
    cmpl $256, 332(%r9)
    jne .Lntc_refuse
.Lntc_xsave_setup:
    cmp $576, %rdx
    jb .Lntc_refuse
    cmp $1024, %rdx
    ja .Lntc_refuse
    testq $0x40000, 200(%r10)
    jz .Lntc_refuse
    mov %r8d, %eax
    and $0x0c000000, %eax
    cmp $0x0c000000, %eax
    jne .Lntc_refuse
    xor %ecx, %ecx
    xgetbv
    test %edx, %edx
    jnz .Lntc_refuse
    cmp 24(%r10), %rax
    jne .Lntc_refuse
    mov %rax, 208(%r10)
    /* The supplied boundary has already excluded XSS/XFD/future state. */
    cmpq $0, 40(%r9)
    jne .Lntc_refuse
    testl $0xfffffff0, 304(%r9)
    jnz .Lntc_refuse
    mov $0xd, %eax
    xor %ecx, %ecx
    cpuid
    mov %ebx, %eax
    cmp 128(%r11), %rax
    jne .Lntc_refuse
    cmpq $7, 24(%r10)
    jne .Lntc_save_original_xsave
    mov $0xd, %eax
    mov $2, %ecx
    cpuid
    cmp $256, %eax
    jne .Lntc_refuse
    cmp $576, %ebx
    jne .Lntc_refuse
    cmpq $832, 128(%r11)
    jb .Lntc_refuse
.Lntc_save_original_xsave:
    mov 24(%r10), %eax
    xor %edx, %edx
    xsave64 NTC_ORIGINAL(%r10)
    jmp .Lntc_save_original_fx
.Lntc_fx_setup:
    cmp $512, %rdx
    jne .Lntc_refuse
    testq $0x40000, 200(%r10)
    jnz .Lntc_refuse
    test $0x08000000, %r8d
    jnz .Lntc_refuse
.Lntc_save_original_fx:
    /* Defined legacy bytes even when XSAVE's in-use optimization omits a
       component. This writes only the first512 bytes, retaining XSAVE header. */
    fxsave64 NTC_ORIGINAL(%r10)
    movq $0, 16(%r10)
    /* All8 x87 slots are valid finite values; TOP=0, no exceptions pending. */
    movw $0x037f, NTC_SEEDED(%r10)
    movb $0xff, NTC_SEEDED+4(%r10)
    movl $0x1f80, NTC_SEEDED+24(%r10)
    lea NTC_SEEDED+32(%r10), %rdi
    movabs $0x9000000000000010, %rax
    mov $8, %ecx
.Lntc_seed_x87:
    mov %rax, (%rdi)
    movw $0x4001, 8(%rdi)
    inc %rax
    add $16, %rdi
    dec %ecx
    jnz .Lntc_seed_x87
    /* Distinct64-bit halves across all16 XMM registers. */
    lea NTC_SEEDED+160(%r10), %rdi
    movabs $0x1122334455667700, %rax
    mov $32, %ecx
.Lntc_seed_xmm:
    mov %rax, (%rdi)
    inc %rax
    add $8, %rdi
    dec %ecx
    jnz .Lntc_seed_xmm
    cmpq $7, 24(%r10)
    jne .Lntc_seed_profile
    lea NTC_SEEDED+576(%r10), %rdi
    movabs $0xa1b2c3d4e5f60700, %rax
    mov $32, %ecx
.Lntc_seed_ymm:
    mov %rax, (%rdi)
    inc %rax
    add $8, %rdi
    dec %ecx
    jnz .Lntc_seed_ymm
.Lntc_seed_profile:
    mov 24(%r10), %eax
    test %eax, %eax
    jz .Lntc_seed_fx
    mov %rax, NTC_SEEDED+512(%r10)
    and $-2, %eax
    xor %edx, %edx
    xrstor64 NTC_SEEDED(%r10)
.Lntc_seed_fx:
    fxrstor64 NTC_SEEDED(%r10)
    movq $1, 248(%r10)
    pushq 32(%r10)
    popfq
    mov 32(%rsp), %rcx
    movabs $0xcafe010000000000, %rax
    movabs $0xcafe010000000002, %rdx
    movabs $0xcafe010000000003, %rbx
    movabs $0xcafe010000000004, %rbp
    movabs $0xcafe010000000005, %rsi
    movabs $0xcafe010000000006, %rdi
    movabs $0xcafe010000000007, %r8
    movabs $0xcafe010000000008, %r9
    movabs $0xcafe010000000009, %r10
    movabs $0xcafe01000000000a, %r11
    movabs $0xcafe01000000000b, %r12
    movabs $0xcafe01000000000c, %r13
    movabs $0xcafe01000000000d, %r14
    movabs $0xcafe01000000000e, %r15
.globl svmvisor_native_transition_canary_call
svmvisor_native_transition_canary_call:
    call svmvisor_native_transition
.globl svmvisor_native_transition_canary_capture
svmvisor_native_transition_canary_capture:
    /* First returned instructions capture flags and every GPR without scratch.
       The pointer is recovered from our stack, never a returned GPR. */
    pushfq
    ntc_push_gprs
    mov 168(%rsp), %r10
    mov 120(%rsp), %rax
    mov %rax, 40(%r10)
    lea 128(%rsp), %rax
    mov %rax, 56(%r10)
    lea 112(%rsp), %rsi
    lea 64(%r10), %rdi
    mov $15, %ecx
.Lntc_capture_gprs:
    mov (%rsi), %rax
    mov %rax, (%rdi)
    sub $8, %rsi
    add $8, %rdi
    dec %ecx
    jnz .Lntc_capture_gprs
    /* A corrupt IF/DF is retained in the observation. Reinstate the actual
       incoming flags before string operations; no such repair counts as PASS. */
    pushq 32(%r10)
    popfq
    cmpq $0, 24(%r10)
    je .Lntc_capture_fx
    mov 24(%r10), %eax
    xor %edx, %edx
    xsave64 NTC_OBSERVED(%r10)
.Lntc_capture_fx:
    fxsave64 NTC_OBSERVED(%r10)
    movq $1, 240(%r10)
    /* These are observations only. A control corruption disabling the save
       instructions is outside this non-recovering shim's return contract. */
    mov %cr0, %rax
    mov %rax, 216(%r10)
    cmp 192(%r10), %rax
    jne .Lntc_bad_controls
    mov %cr4, %rax
    mov %rax, 224(%r10)
    cmp 200(%r10), %rax
    jne .Lntc_bad_controls
    cmpq $0, 24(%r10)
    je .Lntc_compare
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 232(%r10)
    cmp 208(%r10), %rax
    je .Lntc_compare
.Lntc_bad_controls:
    orq $256, 16(%r10)
.Lntc_compare:
    mov 64+8(%r10), %rax
    cmp 184(%r10), %rax
    jne .Lntc_bad_gpr
    lea 64(%r10), %rsi
    movabs $0xcafe010000000000, %rax
    xor %ecx, %ecx
.Lntc_compare_gprs:
    cmp $1, %ecx /* RCX is the actual context pointer, checked above. */
    je .Lntc_next_gpr
    cmp (%rsi), %rax
    jne .Lntc_bad_gpr
.Lntc_next_gpr:
    inc %rax
    add $8, %rsi
    inc %ecx
    cmp $15, %ecx
    jne .Lntc_compare_gprs
    jmp .Lntc_compare_flags
.Lntc_bad_gpr:
    orq $1, 16(%r10)
.Lntc_compare_flags:
    mov 32(%r10), %rax
    cmp 40(%r10), %rax
    je .Lntc_compare_rsp
    orq $2, 16(%r10)
.Lntc_compare_rsp:
    mov 48(%r10), %rax
    cmp 56(%r10), %rax
    je .Lntc_compare_x87_environment
    orq $4, 16(%r10)
.Lntc_compare_x87_environment:
    lea NTC_SEEDED(%r10), %rsi
    lea NTC_OBSERVED(%r10), %rdi
    mov $5, %ecx /* FCW, FSW and abridged FTW; reserved byte5 excluded. */
    repe cmpsb
    je .Lntc_compare_x87_payload
    orq $8, 16(%r10)
.Lntc_compare_x87_payload:
    lea NTC_SEEDED+32(%r10), %rsi
    lea NTC_OBSERVED+32(%r10), %rdi
    mov $8, %r8d
.Lntc_compare_x87_slot:
    mov $10, %ecx
    repe cmpsb
    jne .Lntc_bad_x87_payload
    add $6, %rsi
    add $6, %rdi
    dec %r8d
    jnz .Lntc_compare_x87_slot
    jmp .Lntc_compare_mxcsr
.Lntc_bad_x87_payload:
    orq $16, 16(%r10)
.Lntc_compare_mxcsr:
    mov NTC_SEEDED+24(%r10), %eax
    cmp NTC_OBSERVED+24(%r10), %eax
    je .Lntc_compare_xmm
    orq $32, 16(%r10)
.Lntc_compare_xmm:
    lea NTC_SEEDED+160(%r10), %rsi
    lea NTC_OBSERVED+160(%r10), %rdi
    mov $32, %ecx
    repe cmpsq
    je .Lntc_compare_ymm
    orq $64, 16(%r10)
.Lntc_compare_ymm:
    cmpq $7, 24(%r10)
    jne .Lntc_restore
    lea NTC_SEEDED+576(%r10), %rsi
    lea NTC_OBSERVED+576(%r10), %rdi
    mov $32, %ecx
    repe cmpsq
    je .Lntc_restore
    orq $128, 16(%r10)
.Lntc_restore:
    mov 24(%r10), %eax
    test %eax, %eax
    jz .Lntc_restore_fx
    /* Avoid AMD XSAVEERPTR's no-#MF pointer loss from XRSTOR(bit0). The
       subsequent FXRSTOR has the documented conditional-pointer limitation. */
    and $-2, %eax
    xor %edx, %edx
    xrstor64 NTC_ORIGINAL(%r10)
.Lntc_restore_fx:
    fxrstor64 NTC_ORIGINAL(%r10)
    add $128, %rsp
.Lntc_refuse:
    mov 16(%r10), %rax
    mov %rax, 168(%rsp) /* Replace the shim caller's saved RAX only. */
    jmp .Lntc_return
.Lntc_bad_buffer:
    movabs $0x8000000000000000, %rax
    mov %rax, 168(%rsp)
.Lntc_return:
    add $56, %rsp
    ntc_pop_gprs
    popfq
    ret
