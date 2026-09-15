/* QEMU-only EFI x64 returning probe. See ../abi.md. No firmware calls here. */
.text
.p2align 4
/* Private helpers: r10=context, r11=aligned area. Clobber rax/rdx only. */
.Lsave:
    mov 56(%r10), %rax
    test %rax, %rax
    jz .Lsave_fx
    xor %edx, %edx
    xsave64 (%r11)
    ret
.Lsave_fx:
    fxsave64 (%r11)
    ret
.Lrestore:
    mov 56(%r10), %rax
    test %rax, %rax
    jz .Lrestore_fx
    xor %edx, %edx
    xrstor64 (%r11)
    ret
.Lrestore_fx:
    fxrstor64 (%r11)
    ret

.macro save_all
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
.macro restore_all
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

/* EFI ABI outer observation boundary. Argument rcx=context; void return. */
.globl probe_entry
probe_entry:
    save_all
    sub $40, %rsp /* 32-byte shadow area + retained context; call alignment16. */
    mov %rcx, 32(%rsp)
    mov %rcx, %r10
    movq $0, 216(%r10)
    mov 160(%rsp), %rax /* original RFLAGS: 40 locals +120 saved GPR bytes. */
    mov %rax, 128(%r10)
    mov 256(%r10), %r11
    call .Lsave /* Preserve actual firmware vectors/FPU before setting canaries. */
    cmpq $7, 56(%r10)
    jne .Lseed_legacy
    vmovdqa .Lymm_canary(%rip), %ymm0
    vmovdqa %ymm0, %ymm1
    vmovdqa %ymm0, %ymm2
    vmovdqa %ymm0, %ymm3
    vmovdqa %ymm0, %ymm4
    vmovdqa %ymm0, %ymm5
    vmovdqa %ymm0, %ymm6
    vmovdqa %ymm0, %ymm7
    vmovdqa %ymm0, %ymm8
    vmovdqa %ymm0, %ymm9
    vmovdqa %ymm0, %ymm10
    vmovdqa %ymm0, %ymm11
    vmovdqa %ymm0, %ymm12
    vmovdqa %ymm0, %ymm13
    vmovdqa %ymm0, %ymm14
    vmovdqa %ymm0, %ymm15
    jmp .Lseed_done
.Lseed_legacy:
    movdqa .Lxmm_canary(%rip), %xmm6
    movdqa %xmm6, %xmm7
    movdqa %xmm6, %xmm8
    movdqa %xmm6, %xmm9
    movdqa %xmm6, %xmm10
    movdqa %xmm6, %xmm11
    movdqa %xmm6, %xmm12
    movdqa %xmm6, %xmm13
    movdqa %xmm6, %xmm14
    movdqa %xmm6, %xmm15
.Lseed_done:
    mov $0x112201, %ebx
    mov $0x112202, %ebp
    mov $0x112203, %esi
    mov $0x112204, %edi
    mov $0x112205, %r12d
    mov $0x112206, %r13d
    mov $0x112207, %r14d
    mov $0x112208, %r15d
    mov 32(%rsp), %rcx
    pushq 128(%rcx)
    popfq /* CALL/MOV preserve flags so inner observes original caller flags. */
    call probe_inner
    pushfq /* Observe actual returned flags before XOR/CMP can change them. */
    pop %rax
    mov 32(%rsp), %r10
    mov %rax, 192(%r10)
    xor %r11d, %r11d
    cmp 128(%r10), %rax
    je .Lflags_good
    or $4, %r11d
.Lflags_good:
    cmp $0x112201, %rbx
    jne .Lgpr_bad
    cmp $0x112202, %rbp
    jne .Lgpr_bad
    cmp $0x112203, %rsi
    jne .Lgpr_bad
    cmp $0x112204, %rdi
    jne .Lgpr_bad
    cmp $0x112205, %r12
    jne .Lgpr_bad
    cmp $0x112206, %r13
    jne .Lgpr_bad
    cmp $0x112207, %r14
    jne .Lgpr_bad
    cmp $0x112208, %r15
    je .Lgpr_done
.Lgpr_bad:
    or $1, %r11d
.Lgpr_done:
.macro check_xmm register
    movdqa .Lxmm_canary(%rip), %xmm0
    pcmpeqb \register, %xmm0
    pmovmskb %xmm0, %eax
    cmp $65535, %eax
    je 1f
    or $2, %r11d
1:
.endm
    check_xmm %xmm6
    check_xmm %xmm7
    check_xmm %xmm8
    check_xmm %xmm9
    check_xmm %xmm10
    check_xmm %xmm11
    check_xmm %xmm12
    check_xmm %xmm13
    check_xmm %xmm14
    check_xmm %xmm15
    cmpq $7, 56(%r10)
    jne .Lymm_checked
.macro check_ymm register
    vextractf128 $1, \register, %xmm0
    pcmpeqb .Lymm_canary+16(%rip), %xmm0
    pmovmskb %xmm0, %eax
    cmp $65535, %eax
    je 1f
    or $8, %r11d
1:
.endm
    /* CheckYMM0 first because the scratchXMM0 clobbers its upper half. */
    check_ymm %ymm0
    check_ymm %ymm1
    check_ymm %ymm2
    check_ymm %ymm3
    check_ymm %ymm4
    check_ymm %ymm5
    check_ymm %ymm6
    check_ymm %ymm7
    check_ymm %ymm8
    check_ymm %ymm9
    check_ymm %ymm10
    check_ymm %ymm11
    check_ymm %ymm12
    check_ymm %ymm13
    check_ymm %ymm14
    check_ymm %ymm15
.Lymm_checked:
    mov %r11, 216(%r10)
    mov 256(%r10), %r11
    call .Lrestore /* Last vector/FPU operation before firmware ABI return. */
    add $40, %rsp
    restore_all
    /* Return no status in RAX: the original volatile GPR is preserved as well.
       Result is solely in the context so flags also remain bit-exact. */
    ret

/* Inner boundary has no Rust/C/firmware calls. All caller state is explicit. */
.globl probe_inner
probe_inner:
    save_all
    push %rcx /* context remains recoverable after all guest GPRs changed. */
    mov %rcx, %r10
    mov %cr0, %rax
    mov %rax, 72(%r10)
    mov %cr2, %rax
    mov %rax, 80(%r10)
    mov %cr3, %rax
    mov %rax, 88(%r10)
    mov %cr4, %rax
    mov %rax, 96(%r10)
    mov %dr7, %rax
    mov %rax, 120(%r10)
    mov %dr6, %rax
    mov %rax, 272(%r10)
    mov 128(%rsp), %rax /* context8 +15GPR120 -> saved RFLAGS */
    mov %rax, 128(%r10)
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 104(%r10)
    mov $0xc0010117, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 112(%r10)
    movq $0, 224(%r10)
    movq $0, 232(%r10)
    movq $0, 240(%r10) /* No XSS read: fixture rejects CPUs enumerating XSAVES. */
    movq $0, 248(%r10)
    cmpq $0, 56(%r10)
    je .Lcaptured_xcr0
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 224(%r10)
.Lcaptured_xcr0:
    mov 32(%r10), %r11
    call .Lsave
    movq $0, 408(%r10)
    movq $0, 416(%r10)
    movq $0, 424(%r10)
    movq $0, 432(%r10)
    sidt 408(%r10)
    movq $0, 448(%r10)
    movq $0, 456(%r10)
    movq $0, 464(%r10)
    movq $0, 472(%r10)
    movq $0, 480(%r10)
    movq $0, 488(%r10)
    movq $0, 496(%r10)
    movq $0, 504(%r10)
    movq $0, 512(%r10)
    movq $0, 520(%r10)
    movq $0, 544(%r10)
    movq $0, 552(%r10)
    cmpq $0, 440(%r10)
    je .Lbegin_svm
    cmpq $15, 440(%r10)
    ja host_fault_unexpected_stub
    cmpq $3, 440(%r10)
    ja .Lbegin_svm
    cli
    /* Private IDT is live ONLY around this pre-SVM synchronous fault window. */
    sub $16, %rsp
    movw $4095, 0(%rsp)
    mov 400(%r10), %rax
    mov %rax, 2(%rsp)
    movq $1, 480(%r10)
    lidt 0(%rsp)
    add $16, %rsp
    cmpq $2, 440(%r10)
    je .Linject_gp
.globl host_fault_ud_pc
host_fault_ud_pc:
    ud2 /* Kind1 expectsUD; kind3 deliberately expectsGP at this same UD. */
    jmp host_fault_unexpected_stub
.Linject_gp:
    movabs $0x0000800000000000, %rax
.globl host_fault_gp_pc
host_fault_gp_pc:
    mov (%rax), %rax /* Noncanonical48 DS reference: #GP(0), no control write. */
    jmp host_fault_unexpected_stub
.globl host_fault_recovered_pc
host_fault_recovered_pc:
    lidt 408(%r10)
    sidt 424(%r10)
    movq $3, 480(%r10)
    cmpq $0, 512(%r10)
    je .Lhost_restore_vectors
    cmpq $7, 512(%r10)
    je .Lhost_loaded_restore
    cmpq $5, 512(%r10)
    je .Lhost_loaded_restore
    cmpq $3, 512(%r10)
    jne .Lhost_binding_observe
.Lhost_loaded_restore:
    mov 8(%r10), %rax
    vmload %rax /* Undo the completed host-derived guest extra-state load. */
    jmp .Lhost_extra_observe
.Lhost_binding_observe:
    cmpq $1, 512(%r10)
    jne host_fault_unexpected_stub
    /* Post-binding checkpoint precedes VMLOADguest: extra state never changed.
       VMSAVE independently observes that fact; no unnecessary VMLOAD is used. */
.Lhost_extra_observe:
    mov 16(%r10), %rax
    vmsave %rax
.Lhost_restore_vectors:
    mov 32(%r10), %r11
    call .Lrestore
    mov 40(%r10), %r11
    call .Lsave
    mov 32(%r10), %r11
    call .Lrestore
    /* QEMU's noncanonical #GP path can update CR2. Restore observed caller
       debug/fault state explicitly instead of assuming the injector is inert. */
    mov 80(%r10), %rax
    mov %rax, %cr2
    mov 272(%r10), %rax
    mov %rax, %dr6
    mov 120(%r10), %rax
    mov %rax, %dr7
    cmpq $0, 512(%r10)
    je .Lobserve_controls
    /* Only the completed post-binding checkpoint owns this rollback sequence.
       IF remains clear; restore HSAVE before restoring GIF while SVME is set. */
    mov 112(%r10), %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0010117, %ecx
    wrmsr
    stgi
    mov 104(%r10), %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0000080, %ecx
    wrmsr
    cmpq $7, 512(%r10)
    jne .Lcheck_xstate_cleanup
    movq $8, 512(%r10)
    jmp .Lobserve_controls
.Lcheck_xstate_cleanup:
    cmpq $5, 512(%r10)
    jne .Lcheck_loaded_cleanup
    movq $6, 512(%r10)
    jmp .Lobserve_controls
.Lcheck_loaded_cleanup:
    cmpq $3, 512(%r10)
    jne .Lbinding_cleanup_done
    movq $4, 512(%r10)
    jmp .Lobserve_controls
.Lbinding_cleanup_done:
    movq $2, 512(%r10)
    jmp .Lobserve_controls
.Lbegin_svm:
    cli
    mov 104(%r10), %rax
    or $0x1000, %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0000080, %ecx
    wrmsr
    clgi
    mov 8(%r10), %rax
    vmsave %rax /* Host FS/GS/TR/LDTR and syscall/SYSENTER state. */
    mov 24(%r10), %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0010117, %ecx
    wrmsr
    cmpq $7, 440(%r10)
    jae .Lprepare_loaded_guest
    cmpq $4, 440(%r10)
    jb .Lload_guest
    /* Actual readbacks establish the checkpoint; no synthetic mutation flags. */
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 496(%r10)
    mov $0xc0010117, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 504(%r10)
    pushfq
    popq 520(%r10)
    movq $1, 512(%r10)
    /* Private IDT covers only the exact, fully established checkpoint.
       Partial setup/cleanup exceptions remain outside this recovery scope. */
    sub $16, %rsp
    movw $4095, 0(%rsp)
    mov 400(%r10), %rax
    mov %rax, 2(%rsp)
    movq $1, 480(%r10)
    lidt 0(%rsp)
    add $16, %rsp
    cmpq $5, 440(%r10)
    je .Linject_armed_gp
.globl host_armed_ud_pc
host_armed_ud_pc:
    ud2
    jmp host_fault_unexpected_stub
.Linject_armed_gp:
    movabs $0x0000800000000000, %rax
.globl host_armed_gp_pc
host_armed_gp_pc:
    mov (%rax), %rax
    jmp host_fault_unexpected_stub
.Lprepare_loaded_guest:
    /* Preserve actual host descriptor state in this bounded fixture. Copy only
       VMSAVE-defined extra fields; leave the guest VMCB control area intact. */
    mov 8(%r10), %rdx
    mov 0(%r10), %rax
.macro copy_extra offset
    mov \offset(%rdx), %rcx
    mov %rcx, \offset(%rax)
.endm
    copy_extra 0x440
    copy_extra 0x448
    copy_extra 0x450
    copy_extra 0x458
    /* The post-exit fixture must actually pass VMRUN guest consistency.
       Keep its validated guest TR/LDTR; firmware TR may be unusable as guest. */
    cmpq $13, 440(%r10)
    jae .Lcopy_guest_syscalls
    copy_extra 0x470
    copy_extra 0x478
    copy_extra 0x490
    copy_extra 0x498
.Lcopy_guest_syscalls:
    copy_extra 0x600
    copy_extra 0x608
    copy_extra 0x610
    copy_extra 0x618
    copy_extra 0x620
    copy_extra 0x628
    copy_extra 0x630
    copy_extra 0x638
    movq $0x12345000, 0x608(%rax) /* LSTAR: no SYSCALL executes. */
    movq $0x23456000, 0x610(%rax) /* CSTAR: canonical, distinct from host. */
.Lload_guest:
    mov 0(%r10), %rax
    vmload %rax
    cmpq $13, 440(%r10)
    jae .Lrestore_guest_vectors /* No private IDT across VMRUN/VMEXIT. */
    cmpq $7, 440(%r10)
    jb .Lrestore_guest_vectors
    mov 528(%r10), %rax
    vmsave %rax /* Independent actual loaded-state checkpoint capture. */
    cmpq $10, 440(%r10)
    jb .Lloaded_checkpoint_controls
    mov 48(%r10), %r11
    call .Lrestore /* Completed guest xstate load, still outside private IDT. */
    mov 536(%r10), %r11
    call .Lsave /* Independent live capture; never alias the source image. */
.Lloaded_checkpoint_controls:
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 496(%r10)
    mov $0xc0010117, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 504(%r10)
    pushfq
    popq 520(%r10)
    movq $3, 512(%r10) /* Guest extra load and observation both completed. */
    cmpq $10, 440(%r10)
    jb .Lloaded_arm_idt
    movq $5, 512(%r10) /* Guest xstate restore and live capture completed. */
.Lloaded_arm_idt:
    sub $16, %rsp
    movw $4095, 0(%rsp)
    mov 400(%r10), %rax
    mov %rax, 2(%rsp)
    movq $1, 480(%r10)
    lidt 0(%rsp)
    add $16, %rsp
    cmpq $10, 440(%r10)
    jae .Linject_xstate
    cmpq $8, 440(%r10)
    je .Linject_loaded_gp
.globl host_loaded_ud_pc
host_loaded_ud_pc:
    ud2
    jmp host_fault_unexpected_stub
.Linject_loaded_gp:
    movabs $0x0000800000000000, %rax
.globl host_loaded_gp_pc
host_loaded_gp_pc:
    mov (%rax), %rax
    jmp host_fault_unexpected_stub
.Linject_xstate:
    cmpq $11, 440(%r10)
    je .Linject_xstate_gp
.globl host_xstate_ud_pc
host_xstate_ud_pc:
    ud2 /* Kind12 expectsGP at this UD site and must terminate. */
    jmp host_fault_unexpected_stub
.Linject_xstate_gp:
    movabs $0x0000800000000000, %rax
.globl host_xstate_gp_pc
host_xstate_gp_pc:
    mov (%rax), %rax
    jmp host_fault_unexpected_stub
.Lrestore_guest_vectors:
    mov 48(%r10), %r11
    call .Lrestore
    mov 0(%r10), %rax
    mov $0x201, %ecx
    mov $0x202, %edx
    mov $0x203, %ebx
    mov $0x204, %ebp
    mov $0x205, %esi
    mov $0x206, %edi
    mov $0x208, %r8d
    mov $0x209, %r9d
    mov $0x20a, %r10d
    mov $0x20b, %r11d
    mov $0x20c, %r12d
    mov $0x20d, %r13d
    mov $0x20e, %r14d
    mov $0x20f, %r15d
    push %r11
    mov 8(%rsp), %r11
    incq 488(%r11) /* Count the actual entry attempt, with guestGPRs preserved. */
    pop %r11
    vmrun %rax
    /* Do not clobber any guest GPR until its complete frame is captured. */
    push %r15
    push %r14
    push %r13
    push %r12
    push %r11
    push %r10
    push %r9
    push %r8
    push %rdi
    push %rsi
    push %rbp
    push %rbx
    push %rdx
    push %rcx
    mov 112(%rsp), %r10
    cld
    mov %rsp, %rsi
    lea 288(%r10), %rdi
    mov $14, %ecx
    rep movsq
    add $112, %rsp
    mov 0(%r10), %rax
    vmsave %rax
    mov 0x70(%rax), %rdx
    mov %rdx, 200(%r10)
    mov 0x5f8(%rax), %rdx
    mov %rdx, 208(%r10)
    mov 0x578(%rax), %rdx
    mov %rdx, 544(%r10)
    cmpq $13, 440(%r10)
    jae .Lpost_exit_capture
    mov 48(%r10), %r11
    call .Lsave /* Capture guest vectors before loading any host vectors. */
    jmp .Lnormal_restore_host
.Lpost_exit_capture:
    /* R10 was recovered from the known stack AFTER all guest GPRs were saved.
       Only integer operations/VMSAVE intervene before the independent xsave. */
    mov 528(%r10), %rax
    vmsave %rax
    mov 536(%r10), %r11
    call .Lsave /* Retain the initial guest image at48; actual exit state at536. */
    movq $1, 552(%r10)
    cmpq $1, 488(%r10)
    jne host_fault_unexpected_stub
    cmpq $0x81, 200(%r10) /* Reject INVALID or intercepted guest exceptions. */
    jne host_fault_unexpected_stub
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 496(%r10)
    mov $0xc0010117, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 504(%r10)
    pushfq
    popq 520(%r10)
    movq $7, 512(%r10)
    /* Exact same-CPL, IST0 post-exit window only. Host CS/SS have reloaded;
       guest TR/LDTR remain active until cleanup VMLOAD. No task switch occurs. */
    sub $16, %rsp
    movw $4095, 0(%rsp)
    mov 400(%r10), %rax
    mov %rax, 2(%rsp)
    movq $1, 480(%r10)
    lidt 0(%rsp)
    add $16, %rsp
    cmpq $14, 440(%r10)
    je .Linject_post_exit_gp
.globl host_post_exit_ud_pc
host_post_exit_ud_pc:
    ud2 /* Kind15 expectsGP and must terminate on this actualUD. */
    jmp host_fault_unexpected_stub
.Linject_post_exit_gp:
    movabs $0x0000800000000000, %rax
.globl host_post_exit_gp_pc
host_post_exit_gp_pc:
    mov (%rax), %rax
    jmp host_fault_unexpected_stub
.Lnormal_restore_host:
    mov 8(%r10), %rax
    vmload %rax
    mov 16(%r10), %rax
    vmsave %rax /* Independent hardware recapture of restored extra state. */
    testq $1, 64(%r10)
    jnz .Lomit_restore
    mov 32(%r10), %r11
    call .Lrestore
.Lomit_restore:
    mov 40(%r10), %r11
    call .Lsave /* Independent observed state catches deliberate omission. */
    mov 32(%r10), %r11
    call .Lrestore /* Repair even negative fixtures before STGI/firmware return. */
    mov 80(%r10), %rax
    mov %rax, %cr2
    mov 272(%r10), %rax
    mov %rax, %dr6
    mov 120(%r10), %rax
    mov %rax, %dr7
    mov 112(%r10), %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0010117, %ecx
    wrmsr
    /* Host descriptors/extra state/vectors/debug/HSAVE are now restored.
       Keep IF0 while restoring GIF, then clear SVME while STGI is legal. */
    stgi
    mov 104(%r10), %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0000080, %ecx
    wrmsr
.Lobserve_controls:
    mov %cr0, %rax
    mov %rax, 136(%r10)
    mov %cr2, %rax
    mov %rax, 144(%r10)
    mov %cr3, %rax
    mov %rax, 152(%r10)
    mov %cr4, %rax
    mov %rax, 160(%r10)
    mov %dr7, %rax
    mov %rax, 184(%r10)
    mov %dr6, %rax
    mov %rax, 280(%r10)
    mov $0xc0000080, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 168(%r10)
    mov $0xc0010117, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 176(%r10)
    cmpq $0, 56(%r10)
    je .Lobserved_xcr0
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 232(%r10)
.Lobserved_xcr0:
    sidt 424(%r10)
    cmpq $0, 440(%r10)
    je .Lhost_stage_done
    cmpq $3, 480(%r10)
    jne host_fault_unexpected_stub
    movq $4, 480(%r10)
.Lhost_stage_done:
    testq $2, 64(%r10)
    jz .Lfinal_flags
    movq $0, 96(%rsp) /* Deliberately corrupt inner saved RBX; outer repairs. */
.Lfinal_flags:
    pushq 128(%r10)
    popfq
    pushfq
    popq 192(%r10)
    pop %rcx /* discard context without changing flags */
    restore_all
    ret

/* AMD64 exception frames always include SS:RSP and restore them withIRETQ.
 * Normalize vector/error at stacktop. No compiler/helper is called. */
.globl host_fault_ud_stub
host_fault_ud_stub:
    pushq $0
    pushq $6
    jmp .Lhost_fault_common
.globl host_fault_gp_stub
host_fault_gp_stub:
    pushq $13 /* CPU already pushed the GP error code. */
.Lhost_fault_common:
    push %rax
    push %rdx
    /* Normalized offsets after two savedGPRs: vector16,error24,RIP32. */
    mov 16(%rsp), %rax
    mov %rax, 448(%r10)
    mov 32(%rsp), %rdx
    mov %rdx, 456(%r10)
    mov 24(%rsp), %rdx
    mov %rdx, 464(%r10)
    cmpq $1, 480(%r10)
    jne host_fault_unexpected_stub
    cmpq $13, 440(%r10)
    jae .Lhost_post_exit_count_guard
    cmpq $0, 488(%r10)
    jne host_fault_unexpected_stub
    jmp .Lhost_recovery_count_guard
.Lhost_post_exit_count_guard:
    cmpq $1, 488(%r10)
    jne host_fault_unexpected_stub
    cmpq $1, 552(%r10)
    jne host_fault_unexpected_stub
    cmpq $0x81, 200(%r10)
    jne host_fault_unexpected_stub
.Lhost_recovery_count_guard:
    cmpq $0, 472(%r10)
    jne host_fault_unexpected_stub
    cmpq $3, 440(%r10)
    ja .Lhost_post_binding_guard
    cmpq $0, 512(%r10)
    jne host_fault_unexpected_stub
    cmpq $1, 440(%r10)
    je .Lhost_expect_ud
    cmpq $2, 440(%r10)
    je .Lhost_expect_gp
    cmpq $3, 440(%r10)
    jne host_fault_unexpected_stub
.Lhost_expect_gp:
    cmp $13, %rax
    jne host_fault_unexpected_stub
    lea host_fault_gp_pc(%rip), %rdx
    cmpq $3, 440(%r10)
    jne .Lhost_check_pc
    lea host_fault_ud_pc(%rip), %rdx /* Mismatch changes only expectedvector. */
    jmp .Lhost_check_pc
.Lhost_expect_ud:
    cmp $6, %rax
    jne host_fault_unexpected_stub
    lea host_fault_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_post_binding_guard:
    cmpq $7, 440(%r10)
    jae .Lhost_loaded_stage_guard
    cmpq $1, 512(%r10)
    jne host_fault_unexpected_stub
    jmp .Lhost_mutated_controls_guard
.Lhost_loaded_stage_guard:
    cmpq $10, 440(%r10)
    jae .Lhost_xstate_stage_guard
    cmpq $3, 512(%r10)
    jne host_fault_unexpected_stub
    jmp .Lhost_mutated_controls_guard
.Lhost_xstate_stage_guard:
    cmpq $13, 440(%r10)
    jae .Lhost_post_exit_stage_guard
    cmpq $5, 512(%r10)
    jne host_fault_unexpected_stub
    jmp .Lhost_mutated_controls_guard
.Lhost_post_exit_stage_guard:
    cmpq $7, 512(%r10)
    jne host_fault_unexpected_stub
.Lhost_mutated_controls_guard:
    mov 104(%r10), %rdx
    or $0x1000, %rdx
    cmp 496(%r10), %rdx
    jne host_fault_unexpected_stub
    mov 24(%r10), %rdx
    cmp 504(%r10), %rdx
    jne host_fault_unexpected_stub
    testq $0x200, 520(%r10)
    jnz host_fault_unexpected_stub
    cmpq $7, 440(%r10)
    jae .Lhost_loaded_site_guard
    cmpq $4, 440(%r10)
    je .Lhost_expect_armed_ud
    cmpq $5, 440(%r10)
    je .Lhost_expect_armed_gp
    cmpq $6, 440(%r10)
    jne host_fault_unexpected_stub
.Lhost_expect_armed_gp:
    cmp $13, %rax
    jne host_fault_unexpected_stub
    lea host_armed_gp_pc(%rip), %rdx
    cmpq $6, 440(%r10)
    jne .Lhost_check_pc
    lea host_armed_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_expect_armed_ud:
    cmp $6, %rax
    jne host_fault_unexpected_stub
    lea host_armed_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_loaded_site_guard:
    cmpq $10, 440(%r10)
    jae .Lhost_xstate_site_guard
    cmpq $7, 440(%r10)
    je .Lhost_expect_loaded_ud
    cmpq $8, 440(%r10)
    je .Lhost_expect_loaded_gp
    cmpq $9, 440(%r10)
    jne host_fault_unexpected_stub
.Lhost_expect_loaded_gp:
    cmp $13, %rax
    jne host_fault_unexpected_stub
    lea host_loaded_gp_pc(%rip), %rdx
    cmpq $9, 440(%r10)
    jne .Lhost_check_pc
    lea host_loaded_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_expect_loaded_ud:
    cmp $6, %rax
    jne host_fault_unexpected_stub
    lea host_loaded_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_xstate_site_guard:
    cmpq $13, 440(%r10)
    jae .Lhost_post_exit_site_guard
    cmpq $10, 440(%r10)
    je .Lhost_expect_xstate_ud
    cmpq $11, 440(%r10)
    je .Lhost_expect_xstate_gp
    cmpq $12, 440(%r10)
    jne host_fault_unexpected_stub
.Lhost_expect_xstate_gp:
    cmp $13, %rax
    jne host_fault_unexpected_stub
    lea host_xstate_gp_pc(%rip), %rdx
    cmpq $12, 440(%r10)
    jne .Lhost_check_pc
    lea host_xstate_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_expect_xstate_ud:
    cmp $6, %rax
    jne host_fault_unexpected_stub
    lea host_xstate_ud_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_post_exit_site_guard:
    cmpq $13, 440(%r10)
    je .Lhost_expect_post_exit_ud
    cmpq $14, 440(%r10)
    je .Lhost_expect_post_exit_gp
    cmpq $15, 440(%r10)
    jne host_fault_unexpected_stub
    /* Only the precise post-exit mismatched-vector experiment earns its own
       terminal marker; arbitrary setup/entry faults cannot masquerade as it. */
    cmp $6, %rax
    jne host_fault_unexpected_stub
    lea host_post_exit_ud_pc(%rip), %rdx
    cmp 32(%rsp), %rdx
    jne host_fault_unexpected_stub
    cmpq $0, 24(%rsp)
    jne host_fault_unexpected_stub
    jmp host_post_exit_mismatch_stub
.Lhost_expect_post_exit_gp:
    cmp $13, %rax
    jne host_fault_unexpected_stub
    lea host_post_exit_gp_pc(%rip), %rdx
    jmp .Lhost_check_pc
.Lhost_expect_post_exit_ud:
    cmp $6, %rax
    jne host_fault_unexpected_stub
    lea host_post_exit_ud_pc(%rip), %rdx
.Lhost_check_pc:
    cmp 32(%rsp), %rdx
    jne host_fault_unexpected_stub
    cmpq $0, 24(%rsp)
    jne host_fault_unexpected_stub
    movq $1, 472(%r10)
    movq $2, 480(%r10)
    lea host_fault_recovered_pc(%rip), %rax
    mov %rax, 32(%rsp)
    pop %rdx
    pop %rax
    add $16, %rsp /* Drop normalizedvector+error, retain hardwareIRETQframe. */
    iretq
.globl host_post_exit_mismatch_stub
host_post_exit_mismatch_stub:
    cli
    cld
    lea .Lpost_exit_mismatch_message(%rip), %rsi
    jmp .Lterminal_fault_print
.globl host_fault_unexpected_stub
host_fault_unexpected_stub:
    cli
    cld
    lea .Lunexpected_fault_message(%rip), %rsi
.Lterminal_fault_print:
    mov $0xe9, %edx
.Lunexpected_fault_print:
    lodsb
    test %al, %al
    jz .Lunexpected_fault_exit
    out %al, %dx
    jmp .Lunexpected_fault_print
.Lunexpected_fault_exit:
    mov $17, %eax
    mov $0xf4, %edx
    out %eax, %dx
.Lunexpected_fault_halt:
    hlt
    jmp .Lunexpected_fault_halt

/* Position-independent guest blob copied to its owned GPA1000 page. */
.p2align 4
.globl guest_code_start
.globl guest_vmmcall
.globl guest_code_end
guest_code_start:
    fninit
    fld1
    pxor %xmm0, %xmm0
    pxor %xmm15, %xmm15
    mov %cr4, %rax
    test $0x40000, %eax
    jz .Lguest_legacy
    xor %ecx, %ecx
    xgetbv
    test $4, %eax
    jz .Lguest_legacy
    vxorps %ymm15, %ymm15, %ymm15 /* AVX1: fixture does not require AVX2. */
.Lguest_legacy:
    movabs $0x51554d5552455455, %rax
    mov %rax, -16(%rsp)
guest_vmmcall:
    vmmcall
    ud2
guest_code_end:
.globl guest_ud_start
.globl guest_ud_fault
.globl guest_ud_end
guest_ud_start:
    movabs $0x51554d5552455455, %rax
    mov %rax, -24(%rsp)
guest_ud_fault:
    ud2
    movabs $0xdead0000bad00000, %rax
    mov %rax, -16(%rsp)
    hlt
guest_ud_end:
.globl guest_pf_start
.globl guest_pf_fault
.globl guest_pf_end
guest_pf_start:
    movabs $0x51554d5552455455, %rax
    mov %rax, -24(%rsp)
    mov $0x200000, %eax
guest_pf_fault:
    mov (%rax), %rax
    movabs $0xdead0000bad00000, %rax
    mov %rax, -16(%rsp)
    hlt
guest_pf_end:


.section .rdata,"dr"
.p2align 4
.Lxmm_canary:
    .quad 0x89abcdef01234567, 0x76543210fedcba98

.p2align 5
.Lymm_canary:
    .quad 0x89abcdef01234567, 0x76543210fedcba98
    .quad 0x55aa12344321aa55, 0xfedc098776543210

.Lunexpected_fault_message:
    .asciz "FAIL host-fault-unexpected\n"

.Lpost_exit_mismatch_message:
    .asciz "FAIL post-exit-fault-mismatch\n"
