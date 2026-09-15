/* Integer-only named-MSR capture for AMD Family1Ah Model44h B0 (PPR57896).
 * RCX = exclusive mapped writable352-byte CacheSnapshot, aligned8; EAX=status.
 * No firmware calls, WRMSR, control writes, SVM or arbitrary MSR-index input.
 * This object is unlinked groundwork; no physical execution in this batch.
 */
.text
.p2align 4
.macro nc_read name, number, destination
.globl \name
\name:
    mov $\number, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, \destination(%r10)
    incq 24(%r10)
.endm

.macro nc_variable index, base_number, mask_number
    cmpq $\index, %r8
    jbe .Lnc_complete
    nc_read svmvisor_native_cache_mtrr_base\index, \base_number, (224+16*\index)
    nc_read svmvisor_native_cache_mtrr_mask\index, \mask_number, (232+16*\index)
.endm

.globl svmvisor_native_cache_read
svmvisor_native_cache_read:
    pushfq
    push %rbx
    push %rdi
    mov %rcx, %r10
    test %r10, %r10
    jz .Lnc_output_refuse
    test $7, %r10
    jnz .Lnc_output_refuse
    cld
    mov %r10, %rdi
    xor %eax, %eax
    mov $44, %ecx
    rep stosq
    movq $1, (%r10)
    movq $2, 16(%r10)
    mov 16(%rsp), %rax
    mov %rax, 80(%r10)
    test $0x44700, %eax /* TF/IF/DF/NT/AC must already be clear. */
    jnz .Lnc_refuse
    mov %cs, %ax
    and $3, %eax
    jnz .Lnc_refuse
    movq $3, 16(%r10)
    xor %eax, %eax
    cpuid
    mov %eax, 36(%r10)
    cmp $1, %eax
    jb .Lnc_refuse
    cmp $0x68747541, %ebx
    jne .Lnc_refuse
    cmp $0x69746e65, %edx
    jne .Lnc_refuse
    cmp $0x444d4163, %ecx
    jne .Lnc_refuse
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    mov %eax, 32(%r10)
    mov %ecx, 44(%r10)
    mov %edx, 48(%r10)
    shr $24, %ebx
    mov %ebx, 72(%r10)
    cmp $0x00b40f40, %eax /* ExtFam0Bh+0Fh, ExtModel4h:BaseModel4h, step0. */
    jne .Lnc_refuse
    test $0x80000000, %ecx /* Ordinary firmware, no reported hypervisor. */
    jnz .Lnc_refuse
    movq $4, 16(%r10)
    and $0x00011020, %edx /* MSR, MTRR and PAT. */
    cmp $0x00011020, %edx
    jne .Lnc_refuse
    mov $0x80000000, %eax
    cpuid
    mov %eax, 40(%r10)
    cmp $0x80000023, %eax
    jb .Lnc_refuse
    mov $0x80000001, %eax
    cpuid
    test $0x20000000, %edx /* LM supplies the named EFER register. */
    jz .Lnc_refuse
    mov $0x80000008, %eax
    cpuid
    and $255, %eax
    mov %eax, 52(%r10)
    cmp $48, %eax /* PPR target physical-address width. */
    jne .Lnc_refuse
    mov $0x8000001f, %eax
    xor %ecx, %ecx
    cpuid
    mov %eax, 56(%r10)
    mov %ebx, 60(%r10)
    test $1, %eax /* SME capability expected, enablement read separately. */
    jz .Lnc_refuse
    test $0xbe000000, %eax /* PPR reserved bits31/29..25. */
    jnz .Lnc_refuse
    test $0xffff0000, %ebx
    jnz .Lnc_refuse
    mov %ebx, %eax
    and $63, %eax
    cmp $51, %eax /* This target reports a separate page attribute bit51. */
    jne .Lnc_refuse
    shr $6, %ebx
    and $63, %ebx
    cmp $6, %ebx
    ja .Lnc_refuse
    mov $0x80000023, %eax
    xor %ecx, %ecx
    cpuid
    mov %eax, 64(%r10)
    mov %ebx, 68(%r10)
    test $0xfffffffe, %eax
    jnz .Lnc_refuse
    test $0xffff0000, %ebx
    jnz .Lnc_refuse
    or %edx, %ecx
    jnz .Lnc_refuse
    orq $1, 8(%r10)
    mov %cr0, %rax
    mov %rax, 88(%r10)
    mov %cr4, %rax
    mov %rax, 96(%r10)
    nc_read svmvisor_native_cache_efer, 0xc0000080, 104
    orq $2, 8(%r10)
    nc_read svmvisor_native_cache_sys_cfg, 0xc0010010, 112
    testl $2, 56(%r10)
    jz .Lnc_encryption_check
    /* APM15.34.10: SEV_STATUS exists only when actual CPUID advertises SEV. */
    nc_read svmvisor_native_cache_sev_status, 0xc0010131, 120
    orq $32, 8(%r10)
.Lnc_encryption_check:
    movq $5, 16(%r10)
    testq $0x07800000, 112(%r10) /* SMEE, SNP, VMPL, HMKEE all refused. */
    jnz .Lnc_refuse
    cmpq $0, 120(%r10)
    jne .Lnc_refuse
    orq $4, 8(%r10)
    nc_read svmvisor_native_cache_top_mem, 0xc001001a, 152
    nc_read svmvisor_native_cache_iorr_base0, 0xc0010016, 192
    nc_read svmvisor_native_cache_iorr_mask0, 0xc0010017, 200
    nc_read svmvisor_native_cache_iorr_base1, 0xc0010018, 208
    nc_read svmvisor_native_cache_iorr_mask1, 0xc0010019, 216
    nc_read svmvisor_native_cache_smm_address, 0xc0010112, 160
    nc_read svmvisor_native_cache_smm_mask, 0xc0010113, 168
    nc_read svmvisor_native_cache_apic_base, 0x1b, 176
    nc_read svmvisor_native_cache_mmio_config, 0xc0010058, 184
    orq $8, 8(%r10)
    nc_read svmvisor_native_cache_pat, 0x277, 128
    nc_read svmvisor_native_cache_mtrr_cap, 0xfe, 136
    mov %rax, %r8
    and $255, %r8
    movq $6, 16(%r10)
    cmp $8, %r8
    ja .Lnc_refuse
    nc_read svmvisor_native_cache_mtrr_default, 0x2ff, 144
    /* Literal guarded sites only: no RDMSR with a caller-supplied index. */
    nc_variable 0, 0x200, 0x201
    nc_variable 1, 0x202, 0x203
    nc_variable 2, 0x204, 0x205
    nc_variable 3, 0x206, 0x207
    nc_variable 4, 0x208, 0x209
    nc_variable 5, 0x20a, 0x20b
    nc_variable 6, 0x20c, 0x20d
    nc_variable 7, 0x20e, 0x20f
.Lnc_complete:
    orq $16, 8(%r10)
    movq $0, 16(%r10)
.Lnc_refuse:
    mov 16(%r10), %eax
    jmp .Lnc_return
.Lnc_output_refuse:
    mov $1, %eax
.Lnc_return:
    pop %rdi
    pop %rbx
    popfq
    ret
