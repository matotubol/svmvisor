/* Read-only, integer-only EFI x64 observation helper. No GDT dereference.
 * RCX points to an exclusive mapped writable72-byte NativeSnapshot.
 * EAX=0 captured; EAX=1 refused CPL!=0 before privileged observations.
 * Only volatile RAX is clobbered. RFLAGS arithmetic changes are ABI-volatile.
 */
.text
.p2align 4
.globl svmvisor_native_snapshot
svmvisor_native_snapshot:
    pushfq
    pop %rax
    mov %rax, 64(%rcx)
    mov %cs, %ax
    mov %ax, 32(%rcx)
    and $3, %eax
    jnz .Lsnapshot_refuse_cpl
    /* Clear the padded descriptors before writing the10-byte CPU images. */
    movq $0, 0(%rcx)
    movq $0, 8(%rcx)
    movq $0, 16(%rcx)
    movq $0, 24(%rcx)
    sgdt 0(%rcx)
    sidt 16(%rcx)
    mov %ss, %ax
    mov %ax, 34(%rcx)
    mov %ds, %ax
    mov %ax, 36(%rcx)
    mov %es, %ax
    mov %ax, 38(%rcx)
    mov %cr0, %rax
    mov %rax, 40(%rcx)
    mov %cr3, %rax
    mov %rax, 48(%rcx)
    mov %cr4, %rax
    mov %rax, 56(%rcx)
    xor %eax, %eax
    ret
.Lsnapshot_refuse_cpl:
    mov $1, %eax
    ret
