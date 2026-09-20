/* Experimental EFI x64 ABI: RCX=resident Slot, RDX=source, R8=cookie.
 * One requested-source load exists at the exported fault RIP. All other data
 * accesses are to independently qualified slot/stack storage. No CALL exists.
 * Frame starts at Slot+8; Rust compile-time assertions pin every used offset.
 */
.text
.p2align 4
.globl svmvisor_memory_probe_load
svmvisor_memory_probe_load:
    mov %rdx, %r11
    mov %r8, %r10
    mov %rdx, 8(%rcx)
    mov %r8, 32(%rcx)
    mov %rsp, 40(%rcx)
    pushfq
    pop %rax
    mov %rax, 72(%rcx)
    test $0x64700, %eax
    jnz .Lprobe_reject
    test $2, %eax
    jz .Lprobe_reject
    mov %cr3, %rax
    cmp 16(%rcx), %rax
    jne .Lprobe_reject
    mov %cr2, %rax
    mov %rax, 48(%rcx)
    mov %cr4, %rax
    mov %rax, 56(%rcx)
    test $0x1e21000, %eax
    jnz .Lprobe_reject
    bt $5, %eax
    jnc .Lprobe_reject
    bt $9, %eax
    jnc .Lprobe_reject
    mov %cr0, %rax
    mov %rax, 64(%rcx)
    test $0xc, %eax
    jnz .Lprobe_reject
    bt $31, %eax
    jnc .Lprobe_reject
    bt $16, %eax
    jnc .Lprobe_reject
    bt $0, %eax
    jnc .Lprobe_reject
    mov %cs, %ax
    movzwq %ax, %rax
    mov %rax, 112(%rcx)
    test $3, %eax
    jnz .Lprobe_reject
    mov %ss, %ax
    movzwq %ax, %rax
    mov %rax, 120(%rcx)
    test $3, %eax
    jnz .Lprobe_reject
    pushq 72(%rcx)
    popfq
    movq $2, 0(%rcx)
.globl svmvisor_memory_probe_fault_rip
svmvisor_memory_probe_fault_rip:
    mov (%r11), %rax
    mov %rax, 104(%rcx)
    movq $0, 136(%rcx)
    jmp .Lprobe_verify

.globl svmvisor_memory_probe_failure_rip
svmvisor_memory_probe_failure_rip:
    cmpq $4, 0(%rcx)
    jne svmvisor_memory_probe_fail_stop
    movq $0, 104(%rcx)
    movq $1, 136(%rcx)
.Lprobe_verify:
    mov %cr2, %r9
    cmp 48(%rcx), %r9
    jne svmvisor_memory_probe_fail_stop
    mov %cr4, %r9
    cmp 56(%rcx), %r9
    jne svmvisor_memory_probe_fail_stop
    mov %cr3, %r9
    cmp 16(%rcx), %r9
    jne svmvisor_memory_probe_fail_stop
    mov %cr0, %r9
    cmp 64(%rcx), %r9
    jne svmvisor_memory_probe_fail_stop
    cmp 40(%rcx), %rsp
    jne svmvisor_memory_probe_fail_stop
    movq $1, 0(%rcx)
    mov 136(%rcx), %rax
    pushq 72(%rcx)
    popfq
    ret
.Lprobe_reject:
    movq $0, 104(%rcx)
    movq $2, 136(%rcx)
    mov $2, %eax
    pushq 72(%rcx)
    popfq
    ret

.p2align 4
.globl svmvisor_memory_probe_fail_stop
svmvisor_memory_probe_fail_stop:
    cli
.Lprobe_stopped:
    hlt
    jmp .Lprobe_stopped
