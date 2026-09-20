/* Native opt-in EFI image entry. See docs/native-boundary-contract.md.
 * RCX=image, RDX=system table. No Rust/C/firmware call before xstate capture.
 * No control/MSR/descriptor writes or SVM instructions. RAX returns EFI status.
 */
/* Only the actual resident bridge exports bounded diagnostics through its
 * mandatory EFI x64 caller home area. No shared pointer is accepted here.
 * The EBS outer boundary and ordinary image retain their original bytes. */
#ifdef SVMVISOR_AP_DIAGNOSTICS
#define NATIVE_REFUSE(n) .Lnative_refuse_ ## n
#else
#define NATIVE_REFUSE(n) .Lnative_refuse
#endif
.text
.p2align 4
.globl efi_main
efi_main:
    pushfq
#ifdef SVMVISOR_RESIDENT_CALLBACK
    /* Preserve callback IF before closing the native capture/launch window. */
    cli
#endif
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
    mov %rsp, %r15
    /* At most1656 bytes below EFI entry RSP:128 saved +1472 +56 alignment.
       The whole frame is less than one page; no skipped guard-page probe.
       RSP is call-aligned. First64 bytes include32-byte EFI shadow space. */
    sub $1472, %rsp
    and $-64, %rsp
    lea 64(%rsp), %r14
    cld
    mov %r14, %rdi
    xor %eax, %eax
    mov $176, %ecx /*1408 bytes, including reserved XSAVE header. */
    rep stosq
    movq $1, 0(%r14)
    mov 120(%r15), %rax
    mov %rax, 160(%r14)
    /* TF is only a defensive observation: debugger cooperation is an entry
       precondition because a first-instruction #DB cannot be intercepted here. */
    test $0x100, %eax
    jnz NATIVE_REFUSE(1)
    lea 128(%r15), %rax
    mov %rax, 168(%r14)
    mov 128(%r15), %rax
    mov %rax, 176(%r14)
    lea 112(%r15), %rsi
    lea 184(%r14), %rdi
    mov $15, %ecx
.Lnative_copy_gprs:
    mov (%rsi), %rax
    mov %rax, (%rdi)
    sub $8, %rsi
    add $8, %rdi
    dec %ecx
    jnz .Lnative_copy_gprs
    mov %cs, %ax
    mov %ax, 144(%r14)
    and $3, %eax
    jnz NATIVE_REFUSE(2)
    xor %eax, %eax
    cpuid
    cmp $1, %eax
    jb NATIVE_REFUSE(3)
    mov %eax, 312(%r14)
    cmp $0x68747541, %ebx /* Auth */
    jne NATIVE_REFUSE(4)
    cmp $0x69746e65, %edx /* enti */
    jne NATIVE_REFUSE(5)
    cmp $0x444d4163, %ecx /* cAMD */
    jne NATIVE_REFUSE(6)
    mov $1, %eax
    cpuid
    mov %ebx, 336(%r14)
    mov %ecx, 316(%r14)
    mov %edx, 340(%r14)
    test $0x80000000, %ecx
    jnz NATIVE_REFUSE(7)
    /* MSR, FXSR, SSE, SSE2; the inner is compiled for baseline x86-64 SSE2. */
    and $0x07000020, %edx
    cmp $0x07000020, %edx
    jne NATIVE_REFUSE(8)
    mov $0x80000000, %eax
    cpuid
    cmp $0x80000001, %eax
    jb NATIVE_REFUSE(9)
    mov %eax, %r13d
    mov $0x80000001, %eax
    cpuid
    test $0x20000000, %edx /* Long mode => EFER exists, plus CPUID.MSR above. */
    jz NATIVE_REFUSE(10)
    cmp $0x80000008, %r13d
    jb .Lnative_controls
    mov $0x80000008, %eax
    cpuid
    mov %ebx, 308(%r14)
.Lnative_controls:
    mov %cr0, %rax
    mov %rax, 64(%r14)
    test $0xc, %eax /* EM/TS: never normalize caller controls. */
    jnz NATIVE_REFUSE(11)
    mov %cr2, %rax
    mov %rax, 72(%r14)
    mov %cr3, %rax
    mov %rax, 80(%r14)
    mov %cr4, %rax
    mov %rax, 88(%r14)
    test $0x200, %eax /* OSFXSR */
    jz NATIVE_REFUSE(12)
    mov %cr8, %rax
    mov %rax, 96(%r14)
    mov $0xc0000080, %ecx /* EFER only after AMD/MSR/LM admission. */
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 104(%r14)
    test $0x4000, %eax /* FFXSR can omit XMM at CPL0. */
    jnz NATIVE_REFUSE(13)
    sgdt 112(%r14)
    sidt 128(%r14)
    mov %ss, %ax
    mov %ax, 146(%r14)
    mov %ds, %ax
    mov %ax, 148(%r14)
    mov %es, %ax
    mov %ax, 150(%r14)
    mov %fs, %ax
    mov %ax, 152(%r14)
    mov %gs, %ax
    mov %ax, 154(%r14)
    sldt 156(%r14)
    str 158(%r14)
    /* Discover optional controls even with OSXSAVE off. Unsupported XFD and
       future leafD.1 features are rejected before any save/restore instruction. */
    testl $0x04000000, 316(%r14)
    jz .Lnative_without_xsave
    cmpl $0xd, 312(%r14)
    jb NATIVE_REFUSE(14)
    mov $0xd, %eax
    mov $1, %ecx
    cpuid
    mov %eax, 304(%r14)
    test $0xfffffff0, %eax
    jnz NATIVE_REFUSE(15)
    test $8, %eax
    jnz .Lnative_read_xss
    or %edx, %ecx /* Supervisor support without XSAVES is inconsistent. */
    jnz NATIVE_REFUSE(16)
    jmp .Lnative_xsave_enablement
.Lnative_read_xss:
    mov $0xda0, %ecx /* XSS exists iff CPUID D.1:EAX.XSAVES. */
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 40(%r14)
    orq $2, 8(%r14)
    test %rax, %rax
    jnz NATIVE_REFUSE(17)
.Lnative_xsave_enablement:
    testq $0x40000, 88(%r14)
    jz .Lnative_fx_candidate
    testl $0x08000000, 316(%r14)
    jz NATIVE_REFUSE(18)
    xor %ecx, %ecx
    xgetbv
    test %edx, %edx
    jnz NATIVE_REFUSE(19)
    cmp $3, %eax
    je .Lnative_xcr0_known
    cmp $7, %eax
    jne NATIVE_REFUSE(20)
    testl $0x10000000, 316(%r14)
    jz NATIVE_REFUSE(21)
.Lnative_xcr0_known:
    mov %rax, 16(%r14)
    mov %rax, 32(%r14)
    orq $1, 8(%r14)
    mov $0xd, %eax
    xor %ecx, %ecx
    cpuid
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 320(%r14)
    not %rax
    test 32(%r14), %rax
    jnz NATIVE_REFUSE(22)
    cmp $576, %ebx
    jb NATIVE_REFUSE(23)
    cmp $1024, %ebx
    ja NATIVE_REFUSE(24)
    cmp %ebx, %ecx /* Maximum supported image must cover enabled image. */
    jb NATIVE_REFUSE(25)
    mov %ebx, %eax
    mov %rax, 24(%r14)
    cmpq $7, 16(%r14)
    jne .Lnative_save_xsave
    mov $0xd, %eax
    mov $2, %ecx
    cpuid
    cmp $256, %eax
    jne NATIVE_REFUSE(26)
    test $0xfffffffd, %ecx /* Bit0 supervisor, bit2 XFD, future bits refused. */
    jnz NATIVE_REFUSE(27)
    test %edx, %edx
    jnz NATIVE_REFUSE(28)
    cmp $576, %ebx
    jb NATIVE_REFUSE(29)
    cmp $768, %ebx
    ja NATIVE_REFUSE(30)
    mov %ebx, 328(%r14)
    mov %eax, 332(%r14)
    add %ebx, %eax
    cmp 24(%r14), %rax
    ja NATIVE_REFUSE(31)
.Lnative_save_xsave:
    mov 16(%r14), %eax
    xor %edx, %edx
    xsave64 384(%r14)
    jmp .Lnative_call_inner
.Lnative_without_xsave:
    testq $0x40000, 88(%r14)
    jnz NATIVE_REFUSE(32)
.Lnative_fx_candidate:
    testl $0x08000000, 316(%r14)
    jnz NATIVE_REFUSE(33)
    movq $512, 24(%r14)
    fxsave64 384(%r14)
.Lnative_call_inner:
    mov 104(%r15), %rcx /* Original EFI image handle. */
    mov 96(%r15), %rdx /* Original EFI system table. */
    mov %r14, %r8
    call svmvisor_native_efi_main_inner
#ifdef SVMVISOR_AP_DIAGNOSTICS
    /* Inner return is numeric only here; the public callback still restores
     * original RAX and every other GPR. Raw captured CR0 is context, not a
     * claim that it caused every Rust refusal. Oversize codes stay lossless. */
    mov 64(%r14), %r10
    mov $0x83, %r11d
    cmp $63, %rax
    ja .Lnative_inner_code_ready
    mov %eax, %r11d
    or $0x40, %r11d
    jmp .Lnative_inner_code_store
.Lnative_inner_code_ready:
    mov %rax, %r10
.Lnative_inner_code_store:
    mov %r11d, 136(%r15)
    mov %r10, 144(%r15)
#endif
#ifdef SVMVISOR_RESIDENT_CALLBACK
    /* A refused native launch returns normally and must not execute VMMCALL.
     * An admitted launch enters only the exported guest label below, retaining
     * R14=NativeBoundary and R15=the original 128-byte register frame. */
    jmp .Lnative_restore_state
.globl svmvisor_resident_guest_resume
svmvisor_resident_guest_resume:
    mov $0x53564d41, %eax /* Bounded bootstrap ACK, not a general hypercall. */
.globl svmvisor_resident_guest_ack
svmvisor_resident_guest_ack:
    vmmcall
.globl svmvisor_resident_guest_after_ack
svmvisor_resident_guest_after_ack:
#ifdef SVMVISOR_AP_DIAGNOSTICS
    movl $0, 136(%r15)
    movq $0, 144(%r15)
#endif
    /* Only NativeBootstrapAck completion resumes here. Ordinary Rust return
     * and every pre-Rust refusal skip this publication. CPUID is forwarded
     * natively after ACK; no diagnostic leaf or general hypercall is needed.
     * Keep this bounded integer sequence inside the admitted epilogue span.
     * All scratch GPRs and xstate are restored below, including R14/R15. */
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    shr $24, %ebx
    mov svmvisor_resident_cpu_count(%rip), %rcx
    test %rcx, %rcx
    jz .Lnative_restore_state
    cmp $32, %rcx
    ja .Lnative_restore_state
    lea svmvisor_resident_cpu_ids(%rip), %rsi
    xor %edi, %edi
.Lnative_ack_find:
    cmp (%rsi,%rdi,4), %ebx
    je .Lnative_ack_publish
    inc %edi
    cmp %ecx, %edi
    jb .Lnative_ack_find
    jmp .Lnative_restore_state
.Lnative_ack_publish:
    lock bts %edi, svmvisor_resident_guest_ack_mask(%rip)
.Lnative_restore_state:
#else
    mov %rax, 112(%r15) /* Replace saved RAX with the EFI return status. */
#endif
    cmpq $0, 16(%r14)
    je .Lnative_restore_fx
    mov 16(%r14), %eax
    /* Restore SSE/AVX here, but leave x87 to conditional FXRSTOR below.
       AMD XSAVEERPTR can zero no-#MF pointers on XRSTOR(bit0); skipping bit0
       avoids introducing that loss at our own boundary. Exact old pointers
       still require no inner/firmware x87-pointer mutation; see contract. */
    and $-2, %eax
    xor %edx, %edx
    xrstor64 384(%r14)
.Lnative_restore_fx:
    fxrstor64 384(%r14)
    jmp .Lnative_return
.Lnative_refuse:
#ifndef SVMVISOR_RESIDENT_CALLBACK
    mov $0x8000000000000003, %rax /* EFI_UNSUPPORTED; no Rust entered. */
    mov %rax, 112(%r15)
#endif
.Lnative_return:
    mov %r15, %rsp
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
#ifdef SVMVISOR_AP_DIAGNOSTICS
.globl svmvisor_resident_callback_return_end
svmvisor_resident_callback_return_end:
#endif
#ifdef SVMVISOR_AP_DIAGNOSTICS
/* Distinct pre-Rust sites. MOV preserves each sampled register until copied;
 * no restore instruction runs when capture never completed. The maximum
 * two stores stay in caller home space, R15+136..151 (entry RSP+8..23). */
.Lnative_refuse_1: /* trap_flag */
    mov $1, %r11d
    movq 160(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_2: /* cpl */
    mov $2, %r11d
    movzwl 144(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_3: /* max_basic_leaf */
    mov $3, %r11d
    movl %eax, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_4: /* vendor_ebx */
    mov $4, %r11d
    movl %ebx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_5: /* vendor_edx */
    mov $5, %r11d
    movl %edx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_6: /* vendor_ecx */
    mov $6, %r11d
    movl %ecx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_7: /* hypervisor_present */
    mov $7, %r11d
    movl 316(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_8: /* baseline_features */
    mov $8, %r11d
    movl 340(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_9: /* max_extended_leaf */
    mov $9, %r11d
    movl %eax, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_10: /* long_mode */
    mov $10, %r11d
    movl %edx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_11: /* cr0_em_ts */
    mov $11, %r11d
    movq 64(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_12: /* cr4_osfxsr */
    mov $12, %r11d
    movq 88(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_13: /* efer_ffxsr */
    mov $13, %r11d
    movq 104(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_14: /* xsave_max_leaf */
    mov $14, %r11d
    movl 312(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_15: /* xsave_features */
    mov $15, %r11d
    movl 304(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_16: /* supervisor_without_xsaves */
    mov $16, %r11d
    movl %ecx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_17: /* xss_nonzero */
    mov $17, %r11d
    movq 40(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_18: /* osxsave_cpuid_missing */
    mov $18, %r11d
    movl 316(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_19: /* xcr0_high */
    mov $19, %r11d
    movl %edx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_20: /* xcr0_profile */
    mov $20, %r11d
    movl %eax, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_21: /* avx_cpuid_missing */
    mov $21, %r11d
    movl 316(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_22: /* xcr0_unsupported */
    mov $22, %r11d
    movq 320(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_23: /* xsave_size_small */
    mov $23, %r11d
    movl %ebx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_24: /* xsave_size_large */
    mov $24, %r11d
    movl %ebx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_25: /* xsave_max_size */
    mov $25, %r11d
    movl %ecx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_26: /* avx_size */
    mov $26, %r11d
    movl %eax, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_27: /* avx_flags */
    mov $27, %r11d
    movl %ecx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_28: /* avx_reserved */
    mov $28, %r11d
    movl %edx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_29: /* avx_offset_small */
    mov $29, %r11d
    movl %ebx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_30: /* avx_offset_large */
    mov $30, %r11d
    movl %ebx, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_31: /* avx_extent */
    mov $31, %r11d
    movl %eax, %r10d
    jmp .Lnative_callback_refuse
.Lnative_refuse_32: /* osxsave_without_xsave */
    mov $32, %r11d
    movq 88(%r14), %r10
    jmp .Lnative_callback_refuse
.Lnative_refuse_33: /* osxsave_cpuid_unexpected */
    mov $33, %r11d
    movl 316(%r14), %r10d
    jmp .Lnative_callback_refuse
.Lnative_callback_refuse:
    mov %r11d, 136(%r15)
    mov %r10, 144(%r15)
    jmp .Lnative_return
#endif
