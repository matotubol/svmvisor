/* Native EFI x64 bounded returning transition.
 * The resource/firmware/debugger contracts are in native-transition-contract.md.
 * No firmware/Rust calls and no exception recovery are provided in this object.
 * Every CALL target is a private integer-only helper in this file.
 */
.text
.p2align 4
.set NT_ORIGINAL, 192
.set NT_RESTORED, 448
.set NT_GUEST, 704
.set NT_JOURNAL, 960

.macro nt_rdmsr name, number, destination
.globl \name
\name:
    mov $\number, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, \destination(%r11)
.endm

.macro nt_write_msr name, number
.globl \name
\name:
    mov %rax, %rdx
    shr $32, %rdx
    mov $\number, %ecx
    wrmsr
.endm

.macro nt_canonical register
    mov \register, %rdx
    shl $16, %rdx
    sar $16, %rdx
    cmp \register, %rdx
    jne .Lnt_refuse
.endm

.macro nt_va offset, alignment
    mov \offset(%r10), %rax
    test %rax, %rax
    jz .Lnt_refuse
    test $\alignment-1, %rax
    jnz .Lnt_refuse
    nt_canonical %rax
.endm

.macro nt_pa offset
    mov \offset(%r10), %rax
    test %rax, %rax
    jz .Lnt_refuse
    test $4095, %rax
    jnz .Lnt_refuse
    mov 40(%r12), %cl
    shr %cl, %rax
    jnz .Lnt_refuse
.endm

.globl svmvisor_native_transition
svmvisor_native_transition:
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
    /* 136 local bytes; original flags at256, original return PC at264.
       R12 is the fixed local base except while guest registers are live.
       Context pointer at0 survives VMEXIT independently of every guest GPR. */
    sub $136, %rsp
    mov %rsp, %r12
    mov %rcx, 0(%r12)
    mov %rcx, %r10
    cld
    /* A valid mapped context and initial stack are unavoidable entry contracts.
       Null/misaligned contexts return without touching an output structure. */
    test %r10, %r10
    jz .Lnt_return
    test $63, %r10
    jnz .Lnt_return
    lea NT_ORIGINAL(%r10), %rdi
    xor %eax, %eax
    mov $112, %ecx
    rep stosq
    movq $0, 64(%r12) /* An actual original xstate capture has not completed. */
    movq $1, NT_JOURNAL+16(%r10)
    cmpq $1, 0(%r10)
    jne .Lnt_refuse
    cmpq $1088, 8(%r10)
    jne .Lnt_refuse
    cmpq $2, 184(%r10)
    ja .Lnt_refuse
    mov %cs, %ax
    and $3, %eax
    jnz .Lnt_refuse
    mov 256(%r12), %rax
    test $0x44700, %eax /* TF/IF/DF/NT/AC excluded at this high-TPL ABI. */
    jnz .Lnt_refuse

    movq $2, NT_JOURNAL+16(%r10)
    xor %eax, %eax
    cpuid
    cmp $1, %eax
    jb .Lnt_refuse
    mov %eax, 8(%r12)
    cmp $0x68747541, %ebx
    jne .Lnt_refuse
    cmp $0x69746e65, %edx
    jne .Lnt_refuse
    cmp $0x444d4163, %ecx
    jne .Lnt_refuse
    mov $1, %eax
    cpuid
    mov %ecx, 24(%r12)
    test $0x80000000, %ecx /* Reported hypervisor: production refusal. */
    jnz .Lnt_refuse
    and $0x07000020, %edx /* MSR/FXSR/SSE/SSE2. */
    cmp $0x07000020, %edx
    jne .Lnt_refuse
    mov $0x80000000, %eax
    cpuid
    cmp $0x8000000a, %eax
    jb .Lnt_refuse
    mov %eax, 16(%r12)
    movq $0, 56(%r12) /* Bit0 SEV_STATUS, bit1 DebugExtn, bit2 topology leaf. */
    mov $0x80000001, %eax
    cpuid
    test $0x20000000, %edx
    jz .Lnt_refuse
    test $4, %ecx
    jz .Lnt_refuse
    test $0x400000, %ecx
    jz .Lnt_no_topology
    cmpl $0x8000001e, 16(%r12)
    jb .Lnt_no_topology
    orq $4, 56(%r12)
.Lnt_no_topology:
    mov $0x80000008, %eax
    cpuid
    and $255, %eax
    cmp $32, %eax
    jb .Lnt_refuse
    cmp $52, %eax
    ja .Lnt_refuse
    mov %eax, 40(%r12)
    mov $0x8000000a, %eax
    cpuid
    mov %edx, 80(%r12) /* NRIPS bit3 governs optional nRIP comparisons. */
    test $1, %edx /* This one-entry guest is prepared with NPT. */
    jz .Lnt_refuse
    cmp $2, %ebx
    jb .Lnt_refuse
    mov %ebx, 72(%r12)
    cmpl $0x8000001f, 16(%r12)
    jb .Lnt_no_sev
    mov $0x8000001f, %eax
    cpuid
    test $2, %eax
    jz .Lnt_no_sev
    orq $1, 56(%r12)
.Lnt_no_sev:
    cmpl $0x80000022, 16(%r12)
    jb .Lnt_no_lbrstack
    mov $0x80000022, %eax
    cpuid
    test $2, %eax
    jz .Lnt_no_lbrstack
    orq $2, 56(%r12)
.Lnt_no_lbrstack:

    movq $3, NT_JOURNAL+16(%r10)
    mov %cr0, %rax
    test $0x6000000c, %eax /* CD/NW/EM/TS cannot be normalized. */
    jnz .Lnt_refuse
    mov %eax, %edx
    and $0x80000001, %edx
    cmp $0x80000001, %edx
    jne .Lnt_refuse
    mov %cr4, %rax
    test $0x1e01000, %eax /* LA57/SMAP/PKE/CET/PKS unqualified. */
    jnz .Lnt_refuse
    mov %eax, %edx
    and $0x220, %edx /* PAE and OSFXSR. */
    cmp $0x220, %edx
    jne .Lnt_refuse

    movq $5, NT_JOURNAL+16(%r10)
    movq $0, 32(%r12)
    movq $0, 48(%r12)
    testl $0x4000000, 24(%r12)
    jz .Lnt_no_xsave
    cmpl $0xd, 8(%r12)
    jb .Lnt_refuse
    mov $0xd, %eax
    mov $1, %ecx
    cpuid
    test $0xfffffff0, %eax /* Includes unsupported XFD/future features. */
    jnz .Lnt_refuse
    mov %eax, 32(%r12)
    test $8, %eax
    jnz .Lnt_xsave_known
    or %edx, %ecx
    jnz .Lnt_refuse
.Lnt_xsave_known:
    mov %cr4, %rax
    test $0x40000, %eax
    jz .Lnt_fx_profile
    testl $0x8000000, 24(%r12)
    jz .Lnt_refuse
    xor %ecx, %ecx
    xgetbv
    test %edx, %edx
    jnz .Lnt_refuse
    cmp 120(%r10), %rax
    jne .Lnt_refuse
    cmp $3, %eax
    je .Lnt_xcr0_known
    cmp $7, %eax
    jne .Lnt_refuse
    testl $0x10000000, 24(%r12)
    jz .Lnt_refuse
.Lnt_xcr0_known:
    mov $0xd, %eax
    xor %ecx, %ecx
    cpuid
    shl $32, %rdx
    or %rdx, %rax
    not %rax
    test 120(%r10), %rax
    jnz .Lnt_refuse
    cmp $576, %ebx
    jb .Lnt_refuse
    cmp $1024, %ebx
    ja .Lnt_refuse
    cmp %ebx, %ecx
    jb .Lnt_refuse
    mov %ebx, %eax
    cmp 128(%r10), %rax
    jne .Lnt_refuse
    cmpq $7, 120(%r10)
    jne .Lnt_profile_known
    mov $0xd, %eax
    mov $2, %ecx
    cpuid
    cmp $256, %eax
    jne .Lnt_refuse
    test $0xfffffffd, %ecx
    jnz .Lnt_refuse
    test %edx, %edx
    jnz .Lnt_refuse
    cmp $576, %ebx
    jb .Lnt_refuse
    cmp $768, %ebx
    ja .Lnt_refuse
    mov %ebx, 48(%r12)
    add %ebx, %eax
    cmp 128(%r10), %rax
    ja .Lnt_refuse
    jmp .Lnt_profile_known
.Lnt_no_xsave:
    mov %cr4, %rax
    test $0x40000, %eax
    jnz .Lnt_refuse
.Lnt_fx_profile:
    testl $0x8000000, 24(%r12)
    jnz .Lnt_refuse
    cmpq $0, 120(%r10)
    jne .Lnt_refuse
    cmpq $512, 128(%r10)
    jne .Lnt_refuse
.Lnt_profile_known:

    movq $6, NT_JOURNAL+16(%r10)
    nt_va 16, 64
    nt_va 32, 4096
    nt_va 48, 4096
    nt_va 64, 4096
    nt_va 80, 4096
    nt_va 96, 64
    nt_va 104, 64
    nt_va 112, 64
    nt_va 160, 8
    nt_va 168, 1
    nt_pa 24
    nt_pa 40
    nt_pa 56
    nt_pa 72
    nt_pa 88
    /* Pointer nonaliasing, full extents and memory attributes are adapter-owned.
       Refuse obvious page aliases, including dormant caller HSAVE below. */
    lea 24(%r10), %rsi
    mov $5, %r8d
.Lnt_pa_outer:
    mov (%rsi), %rax
    lea 16(%rsi), %rdi
    mov %r8d, %r9d
    dec %r9d
    jz .Lnt_pa_done
.Lnt_pa_inner:
    cmp (%rdi), %rax
    je .Lnt_refuse
    add $16, %rdi
    dec %r9d
    jnz .Lnt_pa_inner
    add $16, %rsi
    dec %r8d
    jmp .Lnt_pa_outer
.Lnt_pa_done:
    mov 16(%r10), %rsi
    cmpq $1, (%rsi)
    jne .Lnt_refuse
    mov 16(%rsi), %rax
    cmp 120(%r10), %rax
    jne .Lnt_refuse
    mov 24(%rsi), %rax
    cmp 128(%r10), %rax
    jne .Lnt_refuse

    /* The following debug accesses require the documented cooperating-debugger
       entry. No GD=1 trap-and-return pretends to reconstruct original DR6.BD. */
    lea NT_ORIGINAL(%r10), %r11
    pushq 256(%r12)
    popfq
    call .Lnt_capture_scalars
    movq $3, NT_JOURNAL+16(%r10)
    mov NT_ORIGINAL+48(%r10), %rax
    test $0x107000, %eax /* SVME/FFXSR/LMSLE/UAI are outside this profile. */
    jnz .Lnt_refuse
    and $0x500, %eax
    cmp $0x500, %eax
    jne .Lnt_refuse
    testq $0x10, NT_ORIGINAL+120(%r10) /* VM_CR.SVMDIS, not LOCK. */
    jnz .Lnt_refuse
    btq $33, NT_ORIGINAL+128(%r10)
    jc .Lnt_refuse /* SMM_PGCFG_LOCK makes even an EFER WRMSR raise #GP. */
    mov NT_ORIGINAL+56(%r10), %rax
    test $4095, %rax
    jnz .Lnt_refuse
    mov 40(%r12), %cl
    mov %rax, %rdx
    shr %cl, %rdx
    jnz .Lnt_refuse
    cmp 88(%r10), %rax
    je .Lnt_refuse
    cmpq $0, NT_ORIGINAL+152(%r10)
    jne .Lnt_refuse
    cmpq $0, NT_ORIGINAL+168(%r10)
    jne .Lnt_refuse
    movq $4, NT_JOURNAL+16(%r10)
    testq $0x20ff, NT_ORIGINAL+112(%r10)
    jnz .Lnt_refuse
    cmpq $0, NT_ORIGINAL+136(%r10)
    jne .Lnt_refuse
    cmpq $0, NT_ORIGINAL+144(%r10)
    jne .Lnt_refuse
    movq $7, NT_JOURNAL+16(%r10)
    mov NT_ORIGINAL+248(%r10), %eax
    cmp 152(%r10), %rax
    jne .Lnt_refuse
    mov 160(%r10), %rsi
    testq $1, (%rsi)
    jz .Lnt_refuse
.macro nt_expected field
    mov \field(%rsi), %rax
    cmp NT_ORIGINAL+\field(%r10), %rax
    jne .Lnt_refuse
.endm
    nt_expected 8
    nt_expected 24
    nt_expected 32
    nt_expected 48
    add $200, %rsi
    lea NT_ORIGINAL+200(%r10), %rdi
    mov $6, %ecx /* GDTR/IDTR/selectors, including zero reserved bytes. */
    repe cmpsq
    jne .Lnt_refuse
    mov NT_ORIGINAL+232(%r10), %rax
    movabs $0x0004000400040004, %rdx
    test %rdx, %rax
    jnz .Lnt_refuse /* Host CS/SS/DS/ES must use GDT for VMEXIT. */
    movzwl NT_ORIGINAL+200(%r10), %eax
    inc %eax
    cmp 176(%r10), %rax
    jne .Lnt_refuse
    mov %rax, %rcx
    mov NT_ORIGINAL+202(%r10), %rsi
    nt_canonical %rsi
    mov 168(%r10), %rdi
    repe cmpsb
    jne .Lnt_refuse
    /* Bound each original nonnull host descriptor. Hardware may set its A bit
       on VMEXIT; the precise post-exit repair is below, never pre-normalized.
       Other descriptor properties are checked by the retained-table adapter. */
    xor %r8d, %r8d
.Lnt_gdt_host_bounds:
    movzwl NT_ORIGINAL+232(%r10,%r8,2), %eax
    and $-8, %eax
    jz .Lnt_gdt_host_bounds_next
    lea 7(%rax), %rdx
    cmp 176(%r10), %rdx
    jae .Lnt_refuse
.Lnt_gdt_host_bounds_next:
    inc %r8d
    cmp $4, %r8d
    jne .Lnt_gdt_host_bounds
    cmpq $1, 184(%r10)
    je .Lnt_guest_prepared
    movq $8, NT_JOURNAL+16(%r10)
    mov 32(%r10), %r11
    cmpl $0xffffffff, 0(%r11) /* All CR reads/writes. */
    jne .Lnt_refuse
    cmpl $0xffffffff, 4(%r11) /* All DR reads/writes. */
    jne .Lnt_refuse
    cmpl $0xffffffff, 8(%r11) /* Every guest exception exits. */
    jne .Lnt_refuse
    mov 0xc(%r11), %eax
    and $0x1800000f, %eax
    cmp $0x1800000b, %eax /* IOIO/MSR and INTR/NMI/INIT; SMI clear. */
    jne .Lnt_refuse
    mov 0x10(%r11), %eax
    and $0x207f, %eax
    cmp $0x207f, %eax /* VMRUN..SKINIT and XSETBV. */
    jne .Lnt_refuse
    mov 0x58(%r11), %eax
    test %eax, %eax
    jz .Lnt_refuse
    cmp 72(%r12), %eax
    jae .Lnt_refuse
    cmpq $0, 0x50(%r11)
    jne .Lnt_refuse
    cmpl $1, 0x5c(%r11) /* Initial all-ASID flush; RAP/upper fields disabled. */
    jne .Lnt_refuse
    cmpq $0x1000000, 0x60(%r11) /* V_INTR_MASKING only; no injected events. */
    jne .Lnt_refuse
    cmpq $0, 0x68(%r11)
    jne .Lnt_refuse
    cmpq $1, 0x90(%r11) /* NPT only: no encrypted guest. */
    jne .Lnt_refuse
    cmpq $0, 0x98(%r11)
    jne .Lnt_refuse
    cmpq $0, 0xa0(%r11)
    jne .Lnt_refuse
    cmpq $0, 0xa8(%r11)
    jne .Lnt_refuse
    cmpq $0, 0xb8(%r11)
    jne .Lnt_refuse
    cmpq $0, 0xc0(%r11)
    jne .Lnt_refuse
    cmpq $2, 184(%r10)
    jne .Lnt_guest_prepared
    mov 0xc(%r11), %eax
    and $0x1040000, %eax /* CPUID and defensive HLT exits. */
    cmp $0x1040000, %eax
    jne .Lnt_refuse
    cmpq $0x10ff, 136(%r10)
    jne .Lnt_refuse
    cmpq $1, 144(%r10)
    jne .Lnt_refuse
.Lnt_guest_prepared:
    /* Initialize all reserved/header bytes of independent output images. */
    mov 96(%r10), %rdi
    call .Lnt_zero_xstate
    mov 104(%r10), %rdi
    call .Lnt_zero_xstate
    mov 112(%r10), %rdi
    call .Lnt_zero_xstate
    mov 96(%r10), %r11
    call .Lnt_save_xstate
    movq $1, 64(%r12)
    movq $1, NT_JOURNAL(%r10)
    movq $0, NT_JOURNAL+16(%r10)

    mov NT_ORIGINAL+48(%r10), %rax
    or $0x1000, %rax
    nt_write_msr svmvisor_native_enable_svme, 0xc0000080
    movq $2, NT_JOURNAL(%r10)
.globl svmvisor_native_clgi
svmvisor_native_clgi:
    clgi
    movq $3, NT_JOURNAL(%r10)
    mov 40(%r10), %rax
.globl svmvisor_native_save_host_extra
svmvisor_native_save_host_extra:
    vmsave %rax
    movq $4, NT_JOURNAL(%r10)
    mov 88(%r10), %rax
    nt_write_msr svmvisor_native_bind_hsave, 0xc0010117
    movq $5, NT_JOURNAL(%r10)
    mov $0xc0010117, %ecx
.globl svmvisor_native_bound_hsave_readback
svmvisor_native_bound_hsave_readback:
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    cmp 88(%r10), %rax
    jne .Lnt_bound_refusal
    mov $0xc0000080, %ecx
.globl svmvisor_native_bound_efer_readback
svmvisor_native_bound_efer_readback:
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov NT_ORIGINAL+48(%r10), %rdx
    or $0x1000, %rdx
    cmp %rdx, %rax
    jne .Lnt_bound_refusal
    cmpq $1, 184(%r10)
    jne .Lnt_enter_guest
    movq $9, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host
.Lnt_bound_refusal:
    movq $1, NT_JOURNAL+8(%r10)
    movq $9, NT_JOURNAL+16(%r10)
    jmp .Lnt_restore_host
.Lnt_enter_guest:
    mov 24(%r10), %rax
.globl svmvisor_native_load_guest_extra
svmvisor_native_load_guest_extra:
    vmload %rax
    movq $6, NT_JOURNAL(%r10)
    /* Integer guest; no guest xstate restore or x87/MMX/SIMD seed is executed. */
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
.Lnt_attempt_guest:
    push %r11
    mov 8(%rsp), %r11
    cmpq $2, 184(%r11)
    jne .Lnt_attempt_count
    cmpq $65, NT_JOURNAL+64(%r11)
    jae .Lnt_multi_cap
.Lnt_attempt_count:
    movq $7, NT_JOURNAL(%r11)
    incq NT_JOURNAL+64(%r11)
    pop %r11
.globl svmvisor_native_vmrun
svmvisor_native_vmrun:
    vmrun %rax
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
    lea 112(%rsp), %r12
    cld
    mov %rsp, %rsi
    lea NT_GUEST+64(%r10), %rdi
    mov $14, %ecx
    rep movsq
    add $112, %rsp
    orq $2, NT_GUEST+176(%r10)
    incq NT_JOURNAL+72(%r10)
    mov 32(%r10), %r11
.macro nt_guest source, destination
    mov \source(%r11), %rax
    mov %rax, NT_GUEST+\destination(%r10)
.endm
    nt_guest 0x70, 0
    nt_guest 0x78, 8
    nt_guest 0x80, 16
    nt_guest 0x88, 24
    nt_guest 0x578, 32
    nt_guest 0x5d8, 40
    nt_guest 0x570, 48
    nt_guest 0x5f8, 56
    orq $1, NT_GUEST+176(%r10)
    mov 72(%r10), %rax
.globl svmvisor_native_save_guest_extra
svmvisor_native_save_guest_extra:
    vmsave %rax
    orq $4, NT_GUEST+176(%r10)
    mov 112(%r10), %r11
    call .Lnt_save_xstate
    orq $8, NT_GUEST+176(%r10)
    movq $8, NT_JOURNAL(%r10)
    cmpq $2, 184(%r10)
    jne .Lnt_one_classify
    mov 80(%r12), %edx
    and $8, %edx
    call .Lnt_multi_dispatch
    test %eax, %eax
    jz .Lnt_restore_host
    jmp .Lnt_resume_guest
.Lnt_one_classify:
    mov NT_GUEST(%r10), %rax
    movq $7, NT_JOURNAL+8(%r10)
    cmp $0x81, %rax
    jne .Lnt_not_vmmcall
    mov NT_GUEST+32(%r10), %rax
    cmp 136(%r10), %rax
    jne .Lnt_restore_host
    mov NT_GUEST+56(%r10), %rax
    cmp 144(%r10), %rax
    jne .Lnt_restore_host
    movq $2, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host
.Lnt_not_vmmcall:
    cmp $0x60, %rax
    jne .Lnt_not_intr
    movq $10, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host
.Lnt_not_intr:
    cmp $0x61, %rax
    jne .Lnt_not_nmi
    movq $3, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host
.Lnt_not_nmi:
    cmp $0x63, %rax
    jne .Lnt_not_init
    movq $4, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host
.Lnt_not_init:
    cmp $-1, %rax
    jne .Lnt_not_invalid
    movq $6, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host
.Lnt_not_invalid:
    sub $0x40, %rax
    cmp $31, %rax
    ja .Lnt_restore_host
    movq $5, NT_JOURNAL+8(%r10)
    jmp .Lnt_restore_host

.Lnt_multi_cap:
    mov %r11, %r10
    pop %r11
    mov %rsp, %r12
    movq $7, NT_JOURNAL+8(%r10)
    movq $9, NT_GUEST+208(%r10)
    jmp .Lnt_restore_host

.Lnt_resume_guest:
    /* Restore the most recent captured hidden guest state, not its initial
       image. Integer-only dispatch has not modified guest xstate. The VMCB
       holds emulated RAX; RAX below is exclusively the physical VMRUN operand. */
    mov 72(%r10), %rax
.globl svmvisor_native_reload_guest_extra
svmvisor_native_reload_guest_extra:
    vmload %rax
    movq $6, NT_JOURNAL(%r10)
    mov 24(%r10), %rax
    mov NT_GUEST+64(%r10), %rcx
    mov NT_GUEST+72(%r10), %rdx
    mov NT_GUEST+80(%r10), %rbx
    mov NT_GUEST+88(%r10), %rbp
    mov NT_GUEST+96(%r10), %rsi
    mov NT_GUEST+104(%r10), %rdi
    mov NT_GUEST+112(%r10), %r8
    mov NT_GUEST+120(%r10), %r9
    mov NT_GUEST+144(%r10), %r12
    mov NT_GUEST+152(%r10), %r13
    mov NT_GUEST+160(%r10), %r14
    mov NT_GUEST+168(%r10), %r15
    mov NT_GUEST+136(%r10), %r11
    mov NT_GUEST+128(%r10), %r10
    jmp .Lnt_attempt_guest

.Lnt_restore_host:
    mov 40(%r10), %rax
.globl svmvisor_native_restore_host_extra
svmvisor_native_restore_host_extra:
    vmload %rax
    movq $9, NT_JOURNAL(%r10)
    mov 56(%r10), %rax
.globl svmvisor_native_observe_host_extra
svmvisor_native_observe_host_extra:
    vmsave %rax
    mov 96(%r10), %r11
    call .Lnt_restore_xstate
    mov 104(%r10), %r11
    call .Lnt_save_xstate
    call .Lnt_compare_extra
    test %eax, %eax
    jnz .Lnt_restore_mismatch
    call .Lnt_compare_xstate
    test %eax, %eax
    jnz .Lnt_restore_mismatch
    /* No private IDT was installed. LIDT deliberately rebinds the exact saved
       firmware image before STGI; no guest TR/LDTR remains active here. */
    lidt NT_ORIGINAL+216(%r10)
    mov NT_ORIGINAL+16(%r10), %rax
    mov %rax, %cr2
    mov NT_ORIGINAL+104(%r10), %rax
    mov %rax, %dr6
    mov NT_ORIGINAL+112(%r10), %rax
    mov %rax, %dr7
    mov NT_ORIGINAL+56(%r10), %rax
    nt_write_msr svmvisor_native_restore_hsave, 0xc0010117
    movq $10, NT_JOURNAL(%r10)
    /* Check all scalars while events are still held. EFER alone intentionally
       has SVME set; the comparison helper accounts for exactly that bit. */
    lea NT_RESTORED(%r10), %r11
    pushq NT_ORIGINAL+64(%r10)
    popfq
    call .Lnt_capture_scalars
    call .Lnt_compare_scalars_active
    test %eax, %eax
    jnz .Lnt_restore_mismatch
    call .Lnt_restore_gdt_accessed
    test %eax, %eax
    jnz .Lnt_restore_mismatch
    mov 96(%r10), %r11
    call .Lnt_restore_xstate
.globl svmvisor_native_stgi
svmvisor_native_stgi:
    stgi /* Pending INIT may take effect and never return. Do not dismiss it. */
    movq $1, NT_JOURNAL+80(%r10)
    movq $11, NT_JOURNAL(%r10)
    mov NT_ORIGINAL+48(%r10), %rax
    nt_write_msr svmvisor_native_restore_efer, 0xc0000080
    movq $12, NT_JOURNAL(%r10)
    lea NT_RESTORED(%r10), %r11
    pushq NT_ORIGINAL+64(%r10)
    popfq
    call .Lnt_capture_scalars
    call .Lnt_compare_scalars
    test %eax, %eax
    jnz .Lnt_restore_mismatch
    movq $13, NT_JOURNAL(%r10)
    mov 96(%r10), %r11
    call .Lnt_restore_xstate
    movq $1, NT_JOURNAL+88(%r10)
    movq $14, NT_JOURNAL(%r10)
    jmp .Lnt_return

.Lnt_refuse:
    movq $1, NT_JOURNAL+8(%r10)
    /* No SVM mutation precedes this branch. Integer operations only, except
       non-mutating reads; original transition xstate has not yet been saved. */
    jmp .Lnt_return
.Lnt_restore_mismatch:
    movq $11, NT_JOURNAL+8(%r10)
    movq $10, NT_JOURNAL+16(%r10)
.globl svmvisor_native_restoration_failed
svmvisor_native_restoration_failed:
    cli
    hlt
    jmp svmvisor_native_restoration_failed
.Lnt_return:
    add $136, %rsp
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

/* Exact finite integer dispatcher. R10=context, EDX=actual CPUID.NRIPS mask.
   Return EAX=1 only after a validated CPUID/query has committed one resume;
   otherwise EAX=0 selects the common restoration path. No external calls,
   guest memory dereferences, privileged instructions, or unbounded loops.
   R10/R12 survive; other saved guest GPRs may be private scratch here.
   Guest reserved words: CPUID count, query count, resumes, failure, nRIP,
   phase, nRIP-valid; the last two words retain zero. All phase-specific writes
   to VMCB/GPRs occur only after every rejection check has passed. */
.Lnt_multi_dispatch:
    sub $8, %rsp /* Keep every nested private CALL aligned to 16 bytes. */
    movq $7, NT_JOURNAL+8(%r10)
    movq $0, NT_GUEST+208(%r10)
    movq $0, NT_GUEST+216(%r10)
    movq $0, NT_GUEST+224(%r10)
    movq $0, NT_GUEST+232(%r10)
    mov 32(%r10), %r11
    mov NT_GUEST(%r10), %rax
    cmp $0x72, %rax
    je .Lnt_multi_protocol
    cmp $0x81, %rax
    je .Lnt_multi_protocol
    cmp $0x60, %rax
    jne .Lnt_multi_not_intr
    movq $10, NT_JOURNAL+8(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_not_intr:
    cmp $0x61, %rax
    jne .Lnt_multi_not_nmi
    movq $3, NT_JOURNAL+8(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_not_nmi:
    cmp $0x63, %rax
    jne .Lnt_multi_not_init
    movq $4, NT_JOURNAL+8(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_not_init:
    cmp $-1, %rax
    jne .Lnt_multi_not_invalid
    movq $6, NT_JOURNAL+8(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_not_invalid:
    sub $0x40, %rax
    cmp $31, %rax
    ja .Lnt_multi_exit_bad
    movq $5, NT_JOURNAL+8(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_protocol:
    test %edx, %edx
    jz .Lnt_multi_nrip_recorded
    mov 0xc8(%r11), %rax
    mov %rax, NT_GUEST+216(%r10)
    movq $1, NT_GUEST+232(%r10)
.Lnt_multi_nrip_recorded:
    /* A valid pending interrupted event would need explicit reinjection.
       This fixed guest has no event reinjection or continuation policy. */
    btq $31, NT_GUEST+24(%r10)
    jc .Lnt_multi_event_bad
    mov NT_JOURNAL+64(%r10), %rax
    test %rax, %rax
    jz .Lnt_multi_count_bad
    cmp $65, %rax
    ja .Lnt_multi_count_bad
    cmp NT_JOURNAL+72(%r10), %rax
    jne .Lnt_multi_count_bad
    dec %rax
    cmp NT_GUEST+200(%r10), %rax
    jne .Lnt_multi_count_bad
    mov NT_GUEST+184(%r10), %r8
    mov NT_GUEST+192(%r10), %r9
    cmp $32, %r8
    ja .Lnt_multi_count_bad
    cmp $32, %r9
    ja .Lnt_multi_count_bad
    lea (%r8,%r9), %rdx
    cmp %rdx, %rax
    jne .Lnt_multi_count_bad
    cmpq $0x9000, NT_GUEST+40(%r10)
    jne .Lnt_multi_stack_bad
    mov NT_GUEST+48(%r10), %rax
    test $2, %eax
    jz .Lnt_multi_stack_bad
    and $-2264, %rax /* Only bit1 and CF/PF/AF/ZF/SF/OF (0x8d7). */
    jnz .Lnt_multi_stack_bad
.macro nt_multi_sentinel offset, value
    movabs $\value, %rax
    cmp %rax, NT_GUEST+\offset(%r10)
    jne .Lnt_multi_gpr_bad
.endm
    nt_multi_sentinel 88, 0x6d75000000000304
    nt_multi_sentinel 96, 0x6d75000000000305
    nt_multi_sentinel 104, 0x6d75000000000306
    nt_multi_sentinel 112, 0x6d75000000000308
    nt_multi_sentinel 120, 0x6d75000000000309
    nt_multi_sentinel 128, 0x6d7500000000030a
    nt_multi_sentinel 136, 0x6d7500000000030b
    nt_multi_sentinel 144, 0x6d7500000000030c
    nt_multi_sentinel 152, 0x6d7500000000030d
    nt_multi_sentinel 160, 0x6d7500000000030e
    cmp %r8, %r9
    jne .Lnt_multi_expect_query
    cmp $32, %r8
    je .Lnt_multi_stop
    movq $1, NT_GUEST+224(%r10)
    cmpq $0x72, NT_GUEST(%r10)
    jne .Lnt_multi_exit_bad
    cmp NT_GUEST+168(%r10), %r8
    jne .Lnt_multi_gpr_bad
    cmp NT_GUEST+72(%r10), %r8
    jne .Lnt_multi_operand_bad
    mov %r8d, %r9d
    and $7, %r9d
    call .Lnt_multi_leaf
    movabs $0xaabbccdd00000000, %rdx
    or %rdx, %rax
    cmp NT_GUEST+56(%r10), %rax
    jne .Lnt_multi_operand_bad
    movabs $0x1122334400000000, %rax
    cmp NT_GUEST+64(%r10), %rax
    jne .Lnt_multi_operand_bad
    movabs $0x778899aa00000303, %rax
    cmp NT_GUEST+80(%r10), %rax
    jne .Lnt_multi_operand_bad
    mov $0x1086, %eax
    mov $2, %r13d
    call .Lnt_multi_site
    test %eax, %eax
    jz .Lnt_multi_done
    mov %rax, %r14
    call .Lnt_multi_cpuid
    /* Each 32-bit destination is zero-extended, including EAX in the VMCB. */
    mov %rax, 0x5f8(%r11)
    mov %rbx, NT_GUEST+80(%r10)
    mov %rcx, NT_GUEST+64(%r10)
    mov %rdx, NT_GUEST+72(%r10)
    incq NT_GUEST+184(%r10)
    jmp .Lnt_multi_commit_resume
.Lnt_multi_expect_query:
    lea 1(%r9), %rax
    cmp %r8, %rax
    jne .Lnt_multi_count_bad
    cmp $32, %r9
    jae .Lnt_multi_count_bad
    movq $2, NT_GUEST+224(%r10)
    cmpq $0x81, NT_GUEST(%r10)
    jne .Lnt_multi_exit_bad
    cmp NT_GUEST+168(%r10), %r9
    jne .Lnt_multi_gpr_bad
    cmp NT_GUEST+72(%r10), %r9
    jne .Lnt_multi_operand_bad
    cmpq $0, NT_GUEST+56(%r10) /* Full 64-bit core HYPERCALL_QUERY. */
    jne .Lnt_multi_operand_bad
    cmpq $0, NT_GUEST+64(%r10)
    jne .Lnt_multi_operand_bad
    and $7, %r9d
    xor %eax, %eax
    test %r9d, %r9d
    jz .Lnt_multi_query_vendor
    cmp $2, %r9d
    jne .Lnt_multi_query_bx
.Lnt_multi_query_vendor:
    mov $0x566d7653, %eax
.Lnt_multi_query_bx:
    cmp NT_GUEST+80(%r10), %rax
    jne .Lnt_multi_operand_bad
    mov $0x10bf, %eax
    mov $3, %r13d
    call .Lnt_multi_site
    test %eax, %eax
    jz .Lnt_multi_done
    mov %rax, %r14
    movq $1, 0x5f8(%r11) /* Core query ABI_VERSION, no other GPR changes. */
    incq NT_GUEST+192(%r10)
.Lnt_multi_commit_resume:
    mov %r14, 0x578(%r11)
    movq $0, 0xc0(%r11) /* Dirty every supported VMCB group, upper bits zero. */
    movl $1, 0x5c(%r11) /* Conservative full TLB flush for each entry. */
    incq NT_GUEST+200(%r10)
    mov $1, %eax
    add $8, %rsp
    ret
.Lnt_multi_stop:
    movq $3, NT_GUEST+224(%r10)
    cmpq $0x81, NT_GUEST(%r10)
    jne .Lnt_multi_exit_bad
    cmpq $32, NT_GUEST+168(%r10)
    jne .Lnt_multi_gpr_bad
    cmpq $1, NT_GUEST+56(%r10) /* Full 64-bit core HYPERCALL_STOP. */
    jne .Lnt_multi_operand_bad
    cmpq $32, NT_GUEST+64(%r10)
    jne .Lnt_multi_operand_bad
    cmpq $32, NT_GUEST+72(%r10)
    jne .Lnt_multi_operand_bad
    cmpq $0, NT_GUEST+80(%r10)
    jne .Lnt_multi_operand_bad
    mov $0x10ff, %eax
    mov $3, %r13d
    call .Lnt_multi_site
    test %eax, %eax
    jz .Lnt_multi_done
    /* The terminal stop is observed, never advanced or resumed. */
    movq $12, NT_JOURNAL+8(%r10)
.Lnt_multi_done:
    xor %eax, %eax
    add $8, %rsp
    ret
.Lnt_multi_count_bad:
    movq $1, NT_GUEST+208(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_operand_bad:
    movq $3, NT_GUEST+208(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_gpr_bad:
    movq $4, NT_GUEST+208(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_stack_bad:
    movq $5, NT_GUEST+208(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_event_bad:
    movq $6, NT_GUEST+208(%r10)
    jmp .Lnt_multi_done
.Lnt_multi_exit_bad:
    movq $8, NT_GUEST+208(%r10)
    jmp .Lnt_multi_done

/* Exact immutable prepared instruction sites; nRIP is read only under NRIPS.
   Return the exact next RIP for accepted CPUID/VMMCALL, or zero on failure. */
.Lnt_multi_site:
    cmp NT_GUEST+32(%r10), %rax
    jne .Lnt_multi_site_bad
    add %r13, %rax
    cmpq $0, NT_GUEST+232(%r10)
    je .Lnt_multi_site_good
    cmp NT_GUEST+216(%r10), %rax
    jne .Lnt_multi_nrip_bad
.Lnt_multi_site_good:
    ret
.Lnt_multi_site_bad:
    movq $2, NT_GUEST+208(%r10)
    xor %eax, %eax
    ret
.Lnt_multi_nrip_bad:
    movq $7, NT_GUEST+208(%r10)
    xor %eax, %eax
    ret

/* R9D is the already bounded round modulo 8. No indirect/jump-table dispatch. */
.Lnt_multi_leaf:
    xor %eax, %eax
    test %r9d, %r9d
    jz .Lnt_multi_leaf_done
    mov $1, %eax
    cmp $1, %r9d
    je .Lnt_multi_leaf_done
    mov $0x40000000, %eax
    cmp $2, %r9d
    je .Lnt_multi_leaf_done
    mov $0x40000001, %eax
    cmp $3, %r9d
    je .Lnt_multi_leaf_done
    mov $0x80000000, %eax
    cmp $4, %r9d
    je .Lnt_multi_leaf_done
    mov $0x80000001, %eax
    cmp $5, %r9d
    je .Lnt_multi_leaf_done
    mov $0xdeadbeef, %eax
    cmp $6, %r9d
    je .Lnt_multi_leaf_done
    mov $0x40000002, %eax
.Lnt_multi_leaf_done:
    ret

/* Literal parity with the project diagnostic emulation::cpuid, not host CPUID. */
.Lnt_multi_cpuid:
    xor %eax, %eax
    xor %ebx, %ebx
    xor %ecx, %ecx
    xor %edx, %edx
    cmp $6, %r9d
    jae .Lnt_multi_cpuid_done
    cmp $1, %r9d
    jne .Lnt_multi_cpuid_not_basic1
    mov $0x80000000, %ecx
    mov $0x60, %edx
    ret
.Lnt_multi_cpuid_not_basic1:
    cmp $5, %r9d
    jne .Lnt_multi_cpuid_not_extended1
    mov $0x20000000, %edx
    ret
.Lnt_multi_cpuid_not_extended1:
    mov $1, %eax
    cmp $3, %r9d
    je .Lnt_multi_cpuid_done
    cmp $4, %r9d
    jne .Lnt_multi_cpuid_vendor
    mov $0x80000001, %eax
    ret
.Lnt_multi_cpuid_vendor:
    mov $0x566d7653, %ebx
    test %r9d, %r9d
    jz .Lnt_multi_cpuid_basic_vendor
    mov $0x40000001, %eax
    mov $0x726f7369, %ecx
    mov $0x74736554, %edx
    ret
.Lnt_multi_cpuid_basic_vendor:
    mov $0x74736554, %ecx
    mov $0x726f7369, %edx
.Lnt_multi_cpuid_done:
    ret

/* r10=context, r11=output scalar state, r12=fixed local frame. */
.Lnt_capture_scalars:
    pushfq
    pop %rax
    mov %rax, 64(%r11)
    movq $0, 0(%r11)
    mov %cr0, %rax
    mov %rax, 8(%r11)
    mov %cr2, %rax
    mov %rax, 16(%r11)
    mov %cr3, %rax
    mov %rax, 24(%r11)
    mov %cr4, %rax
    mov %rax, 32(%r11)
    mov %cr8, %rax
    mov %rax, 40(%r11)
    sgdt 200(%r11)
    sidt 216(%r11)
    mov %cs, 232(%r11)
    mov %ss, 234(%r11)
    mov %ds, 236(%r11)
    mov %es, 238(%r11)
    mov %fs, 240(%r11)
    mov %gs, 242(%r11)
    sldt 244(%r11)
    str 246(%r11)
    mov $1, %eax
    cpuid
    shr $24, %ebx
    mov %ebx, 248(%r11)
    testq $4, 56(%r12)
    jz .Lnt_capture_apic_done
    mov $0x8000001e, %eax
    cpuid
    mov %eax, 248(%r11)
.Lnt_capture_apic_done:
    nt_rdmsr svmvisor_native_read_efer, 0xc0000080, 48
    orq $1, 0(%r11)
    mov %dr6, %rax
    mov %rax, 104(%r11)
    mov %dr7, %rax
    mov %rax, 112(%r11)
    mov %dr0, %rax
    mov %rax, 72(%r11)
    mov %dr1, %rax
    mov %rax, 80(%r11)
    mov %dr2, %rax
    mov %rax, 88(%r11)
    mov %dr3, %rax
    mov %rax, 96(%r11)
    orq $2, 0(%r11)
    nt_rdmsr svmvisor_native_read_vm_cr, 0xc0010114, 120
    nt_rdmsr svmvisor_native_read_hsave, 0xc0010117, 56
    orq $4, 0(%r11)
    nt_rdmsr svmvisor_native_read_hwcr, 0xc0010015, 128
    orq $8, 0(%r11)
    nt_rdmsr svmvisor_native_read_debugctl, 0x1d9, 136
    orq $16, 0(%r11)
    testq $2, 56(%r12)
    jz .Lnt_capture_no_debugext
    nt_rdmsr svmvisor_native_read_debugext, 0xc000010f, 144
    orq $32, 0(%r11)
.Lnt_capture_no_debugext:
    testq $1, 56(%r12)
    jz .Lnt_capture_no_sev
    nt_rdmsr svmvisor_native_read_sev_status, 0xc0010131, 152
    orq $64, 0(%r11)
.Lnt_capture_no_sev:
    cmpq $0, 120(%r10)
    je .Lnt_capture_no_xcr0
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 160(%r11)
    orq $128, 0(%r11)
.Lnt_capture_no_xcr0:
    testq $8, 32(%r12)
    jz .Lnt_capture_no_xss
    nt_rdmsr svmvisor_native_read_xss, 0xda0, 168
    orq $256, 0(%r11)
.Lnt_capture_no_xss:
    nt_rdmsr svmvisor_native_read_fs_base, 0xc0000100, 176
    nt_rdmsr svmvisor_native_read_gs_base, 0xc0000101, 184
    nt_rdmsr svmvisor_native_read_kernel_gs_base, 0xc0000102, 192
    orq $512, 0(%r11)
    ret

.Lnt_zero_xstate:
    xor %eax, %eax
    mov $128, %ecx
    rep stosq
    ret
.Lnt_save_xstate:
    mov 120(%r10), %eax
    test %eax, %eax
    jz .Lnt_save_fx
    xor %edx, %edx
    xsave64 (%r11)
    ret
.Lnt_save_fx:
    fxsave64 (%r11)
    ret
.Lnt_restore_xstate:
    mov 120(%r10), %eax
    test %eax, %eax
    jz .Lnt_restore_fx
    and $-2, %eax /* Never XRSTOR x87: retain resident no-ES pointer metadata. */
    xor %edx, %edx
    xrstor64 (%r11)
.Lnt_restore_fx:
    fxrstor64 (%r11)
    ret

.Lnt_compare_scalars:
    xor %r8d, %r8d
    jmp .Lnt_compare_scalars_body
.Lnt_compare_scalars_active:
    mov $0x1000, %r8d
.Lnt_compare_scalars_body:
    lea NT_ORIGINAL(%r10), %rsi
    lea NT_RESTORED(%r10), %rdi
    xor %ecx, %ecx
.Lnt_compare_scalar_word:
    mov (%rsi,%rcx), %rax
    cmp $48, %ecx
    jne .Lnt_compare_not_efer
    or %r8, %rax
.Lnt_compare_not_efer:
    xor (%rdi,%rcx), %rax
    test %rax, %rax
    jnz .Lnt_compare_bad
    add $8, %ecx
    cmp $256, %ecx
    jne .Lnt_compare_scalar_word
    xor %eax, %eax
    ret
.Lnt_compare_extra:
.macro nt_extra_range offset, count
    mov 48(%r10), %rsi
    mov 64(%r10), %rdi
    add $\offset, %rsi
    add $\offset, %rdi
    mov $\count, %ecx
    repe cmpsq
    jne .Lnt_compare_bad
.endm
    nt_extra_range 0x440, 4
    nt_extra_range 0x470, 2
    nt_extra_range 0x490, 2
    nt_extra_range 0x600, 8
    xor %eax, %eax
    ret

/* With GIF=0 and the leased APs idle, validate the ENTIRE live GDT before
   performing any repair. The only admitted differences are0->1 in bit0 of
   the access byte for original nonnull host CS/SS/DS/ES. APM4.7.2/3 makes A
   hardware-set/software-cleared; APM15.6 permits host descriptor rereads.
   The retained-table adapter has qualified the original GDT's writable mapping.
   No copying/normalization of descriptor bytes or hidden state is performed. */
.Lnt_restore_gdt_accessed:
    mov 168(%r10), %rsi
    mov NT_ORIGINAL+202(%r10), %rdi
    mov 176(%r10), %rcx
    xor %r8d, %r8d
.Lnt_gdt_validate_byte:
    movzbl (%rsi,%r8), %eax
    movzbl (%rdi,%r8), %edx
    cmp %eax, %edx
    je .Lnt_gdt_validate_next
    mov %eax, %r9d
    xor %edx, %r9d
    cmp $1, %r9d
    jne .Lnt_compare_bad
    test $1, %eax
    jnz .Lnt_compare_bad /* Never accept1->0, even at a permitted byte. */
    xor %r13d, %r13d
.Lnt_gdt_validate_selector:
    movzwl NT_ORIGINAL+232(%r10,%r13,2), %ebx
    and $-8, %ebx
    jz .Lnt_gdt_validate_selector_next
    add $5, %ebx
    cmp %r8, %rbx
    je .Lnt_gdt_validate_next
.Lnt_gdt_validate_selector_next:
    inc %r13d
    cmp $4, %r13d
    jne .Lnt_gdt_validate_selector
    jmp .Lnt_compare_bad
.Lnt_gdt_validate_next:
    inc %r8
    cmp %rcx, %r8
    jb .Lnt_gdt_validate_byte
    /* Full validation completed. Clear only original0/live1 A bits, at most
       four distinct bytes. Duplicate selectors see the already-cleared bit. */
    xor %r13d, %r13d
.Lnt_gdt_repair_selector:
    movzwl NT_ORIGINAL+232(%r10,%r13,2), %ebx
    and $-8, %ebx
    jz .Lnt_gdt_repair_next
    testb $1, 5(%rsi,%rbx)
    jnz .Lnt_gdt_repair_next
    testb $1, 5(%rdi,%rbx)
    jz .Lnt_gdt_repair_next
.globl svmvisor_native_restore_gdt_accessed
svmvisor_native_restore_gdt_accessed:
    andb $-2, 5(%rdi,%rbx)
    incq NT_JOURNAL+96(%r10)
.Lnt_gdt_repair_next:
    inc %r13d
    cmp $4, %r13d
    jne .Lnt_gdt_repair_selector
    /* Both buffers and every descriptor must now match exactly. */
    mov 176(%r10), %rcx
    repe cmpsb
    jne .Lnt_compare_bad
    xor %eax, %eax
    ret

/* Compare semantic x87, MXCSR, XMM and enabled YMM-high state. No reserved bytes
   or empty x87 slots/XSTATE_BV implementation bookkeeping are state evidence.
   Both images came from plain XSAVE/FXSAVE, not guest supplied input images. */
.Lnt_compare_xstate:
    mov 96(%r10), %r8
    mov 104(%r10), %r9
    mov %r8, %rsi
    mov %r9, %rdi
    mov $5, %ecx /* FCW, FSW and abridged tag. */
    repe cmpsb
    jne .Lnt_compare_bad
    testb $0x80, 2(%r8)
    jz .Lnt_compare_x87_slots
    lea 6(%r8), %rsi
    lea 6(%r9), %rdi
    mov $18, %ecx /* FOP, FIP and FDP only when architecturally saved. */
    repe cmpsb
    jne .Lnt_compare_bad
.Lnt_compare_x87_slots:
    movzwl 2(%r8), %eax
    shr $11, %eax
    and $7, %eax
    mov %eax, %ebx /* Physical TOP corresponding to logical ST0. */
    xor %r13d, %r13d
.Lnt_compare_x87_loop:
    mov %ebx, %ecx
    movzbl 4(%r8), %eax
    shr %cl, %eax
    test $1, %eax
    jz .Lnt_compare_x87_next
    mov %r13d, %eax
    shl $4, %eax
    lea 32(%r8,%rax), %rsi
    lea 32(%r9,%rax), %rdi
    mov $10, %ecx
    repe cmpsb
    jne .Lnt_compare_bad
.Lnt_compare_x87_next:
    inc %ebx
    and $7, %ebx
    inc %r13d
    cmp $8, %r13d
    jne .Lnt_compare_x87_loop
    mov 24(%r8), %eax
    cmp 24(%r9), %eax
    jne .Lnt_compare_bad
    lea 160(%r8), %rsi
    lea 160(%r9), %rdi
    mov $32, %ecx
    repe cmpsq
    jne .Lnt_compare_bad
    cmpq $7, 120(%r10)
    jne .Lnt_compare_good
    mov 48(%r12), %eax
    lea (%r8,%rax), %rsi
    lea (%r9,%rax), %rdi
    mov $32, %ecx
    repe cmpsq
    jne .Lnt_compare_bad
.Lnt_compare_good:
    xor %eax, %eax
    ret
.Lnt_compare_bad:
    mov $1, %eax
    ret

#ifdef SVMVISOR_NATIVE_MULTI_HOST_TEST
/* Test-only ordinary Windows x64 ABI wrapper. The production object has no
   symbol or caller for this entry. It executes the exact production dispatcher
   against owned host buffers; it never reaches a privileged instruction. */
.globl svmvisor_native_multi_dispatch_test
svmvisor_native_multi_dispatch_test:
    push %rbx
    push %rbp
    push %rsi
    push %rdi
    push %r12
    push %r13
    push %r14
    push %r15
    sub $40, %rsp
    mov %rcx, %r10
    and $8, %edx
    call .Lnt_multi_dispatch
    add $40, %rsp
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rdi
    pop %rsi
    pop %rbp
    pop %rbx
    ret
#endif
