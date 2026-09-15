.intel_syntax noprefix
.text
.globl svmvisor_fixture_signal_witness
# Win64: RCX=SignalEvent firmware pointer, RDX=real event. Preserve the
# launcher's nonvolatile state while checking all nonvolatile GPRs and two
# full nonvolatile XMM registers across firmware's actual event dispatch.
# UEFI2.11 2.3.4.2 x64 ABI; SSE2 is the native firmware baseline.
svmvisor_fixture_signal_witness:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 88
    movdqu [rsp+32], xmm6
    movdqu [rsp+48], xmm15
    mov [rsp+64], rcx
    mov [rsp+72], rdx
    mov ebx, 0x123401
    mov ebp, 0x123402
    mov esi, 0x123403
    mov edi, 0x123404
    mov r12d, 0x123405
    mov r13d, 0x123406
    mov r14d, 0x123407
    mov r15d, 0x123408
    pcmpeqd xmm6, xmm6
    pxor xmm15, xmm15
    mov rcx, [rsp+72]
    call qword ptr [rsp+64]
    test rax, rax
    jne .Lfail
    cmp rbx, 0x123401
    jne .Lfail
    cmp rbp, 0x123402
    jne .Lfail
    cmp rsi, 0x123403
    jne .Lfail
    cmp rdi, 0x123404
    jne .Lfail
    cmp r12, 0x123405
    jne .Lfail
    cmp r13, 0x123406
    jne .Lfail
    cmp r14, 0x123407
    jne .Lfail
    cmp r15, 0x123408
    jne .Lfail
    pcmpeqd xmm0, xmm0
    pcmpeqb xmm0, xmm6
    pmovmskb eax, xmm0
    cmp eax, 0xffff
    jne .Lfail
    pxor xmm0, xmm0
    pcmpeqb xmm0, xmm15
    pmovmskb eax, xmm0
    cmp eax, 0xffff
    jne .Lfail
    xor eax, eax
    jmp .Lrestore
.Lfail:
    mov eax, 1
.Lrestore:
    movdqu xmm6, [rsp+32]
    movdqu xmm15, [rsp+48]
    add rsp, 88
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

.globl svmvisor_fixture_fsgsbase_witness
# Disposable post-EBS witness only. Win64 RCX/RDX are distinct canonical bases.
# APM2 3.1.3/15.5.2: no firmware/Rust call while substituted bases are live.
# CPUID forces an actual exit; VMLOAD/VMSAVE must preserve both hidden bases.
svmvisor_fixture_fsgsbase_witness:
    push rbx
    pushfq
    cli
    rdfsbase r8
    rdgsbase r9
    mov r10, rcx
    mov r11, rdx
    wrfsbase r10
    wrgsbase r11
    mov eax, 0x4fff0000
    xor ecx, ecx
    cpuid
    cmp eax, 0x53564d52
    jne .Lfsgs_fail
    rdfsbase rax
    cmp rax, r10
    jne .Lfsgs_fail
    rdgsbase rax
    cmp rax, r11
    jne .Lfsgs_fail
    xor eax, eax
    jmp .Lfsgs_restore
.Lfsgs_fail:
    mov eax, 1
.Lfsgs_restore:
    wrfsbase r8
    wrgsbase r9
    popfq
    pop rbx
    ret
