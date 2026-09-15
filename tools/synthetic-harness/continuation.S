/* Emulator-only, single-CPU integer continuation experiment.
 * AMD APM vol.2 rev.3.44 section 15.7: VMCB owns guest RAX/RIP/RSP/RFLAGS;
 * remaining GPRs use GuestRegisters. This fixture captures no system/SIMD state.
 * SysV ABI: rdi=144-byte output, rsi=owned stack top, rdx=input record.
 * Caller guarantees writable output, 4KiB dedicated stack, mapped input/code,
 * IF=DF=0, no asynchronous entry, and no concurrent/reentrant invocation.
 * Monitor saves are intentionally outside the shared fixture page and guest NPT.
 */
.section .text,"ax"
.code64
.global native_loader_capture
native_loader_capture:
    mov %rsp, capture_host_rsp(%rip)
    mov %rbx, capture_host_rbx(%rip)
    mov %rbp, capture_host_rbp(%rip)
    mov %r12, capture_host_r12(%rip)
    mov %r13, capture_host_r13(%rip)
    mov %r14, capture_host_r14(%rip)
    mov %r15, capture_host_r15(%rip)
    pushfq
    popq capture_host_flags(%rip)
    mov %rdi, capture_output(%rip)
    mov %rsi, %rsp
    mov %rdx, %rdi
    jmp loader_native_entry

capture_boundary:
    /* Preserve flags and RAX before touching either. CALL placed only a
     * shared-code return address on the dedicated stack. */
    pushfq
    popq capture_guest_flags(%rip)
    mov %rax, capture_guest_rax(%rip)
    mov capture_output(%rip), %rax
    mov %rcx, 0(%rax)
    mov %rdx, 8(%rax)
    mov %rbx, 16(%rax)
    mov %rbp, 24(%rax)
    mov %rsi, 32(%rax)
    mov %rdi, 40(%rax)
    mov %r8, 48(%rax)
    mov %r9, 56(%rax)
    mov %r10, 64(%rax)
    mov %r11, 72(%rax)
    mov %r12, 80(%rax)
    mov %r13, 88(%rax)
    mov %r14, 96(%rax)
    mov %r15, 104(%rax)
    mov capture_guest_rax(%rip), %rcx
    mov %rcx, 112(%rax)
    mov (%rsp), %rcx
    mov %rcx, 120(%rax)
    lea 8(%rsp), %rcx
    mov %rcx, 128(%rax)
    mov capture_guest_flags(%rip), %rcx
    mov %rcx, 136(%rax)
    mov capture_host_rsp(%rip), %rsp
    mov capture_host_rbx(%rip), %rbx
    mov capture_host_rbp(%rip), %rbp
    mov capture_host_r12(%rip), %r12
    mov capture_host_r13(%rip), %r13
    mov capture_host_r14(%rip), %r14
    mov capture_host_r15(%rip), %r15
    pushq capture_host_flags(%rip)
    popfq
    ret

.section .text.loader_fixture,"ax"
.balign 4096
.global loader_fixture_start, loader_fixture_end, loader_native_entry, loader_resume
loader_fixture_start:
loader_native_entry:
    mov $0x101, %ecx
    mov $0x102, %edx
    mov $0x103, %ebx
    mov $0x104, %ebp
    mov $0x105, %esi
    mov $0x108, %r8d
    mov $0x109, %r9d
    mov $0x10a, %r10d
    mov $0x10b, %r11d
    mov $0x10c, %r12d
    mov $0x10d, %r13d
    mov $0x10e, %r14d
    mov $0x10f, %r15d
    movabs $0x535441434b303032, %rax
    push %rax
    movabs $0x535441434b303031, %rax
    push %rax
    movabs $0x1122334455667788, %rax
    stc
    call capture_boundary
loader_resume:
    jnc loader_fail
    pushfq
    push %rax
    mov 8(%rsp), %rax
    cmp 104(%rdi), %rax
    jne loader_fail
    pop %rax
    popfq
    cmp $0x101, %rcx
    jne loader_fail
    cmp $0x102, %rdx
    jne loader_fail
    cmp $0x103, %rbx
    jne loader_fail
    cmp $0x104, %rbp
    jne loader_fail
    cmp $0x105, %rsi
    jne loader_fail
    cmp $0x108, %r8
    jne loader_fail
    cmp $0x109, %r9
    jne loader_fail
    cmp $0x10a, %r10
    jne loader_fail
    cmp $0x10b, %r11
    jne loader_fail
    cmp $0x10c, %r12
    jne loader_fail
    cmp $0x10d, %r13
    jne loader_fail
    cmp $0x10e, %r14
    jne loader_fail
    cmp $0x10f, %r15
    jne loader_fail
    movabs $0x1122334455667788, %rcx
    cmp %rcx, %rax
    jne loader_fail
    movabs $0x535441434b303031, %rcx
    cmp %rcx, (%rsp)
    jne loader_fail
    movabs $0x535441434b303032, %rcx
    cmp %rcx, 8(%rsp)
    jne loader_fail
    movabs $0x31495041444c5653, %rcx
    cmp %rcx, (%rdi)
    jne loader_fail
    cmpq $3, 8(%rdi)
    jne loader_fail
    lea 16(%rdi), %rsi
    mov $3, %ecx
loader_scan:
    cmpq $7, 16(%rsi)
    je loader_allocate
    add $24, %rsi
    loop loader_scan
    jmp loader_allocation_refused
loader_allocate:
    mov 96(%rdi), %rax
    cmp $0x20000, %rax
    jne loader_fail
    cmp %rax, (%rsi)
    jne loader_fail
    cmpq $1, 8(%rsi)
    jne loader_fail
    mov 88(%rdi), %rdx
    movabs $0x434f4e54494e5545, %rax
    mov %rax, (%rdx)
    mov $1, %eax
    vmmcall
.global loader_allocation_refused
loader_allocation_refused:
    hlt
    jmp loader_allocation_refused
.global loader_fail
loader_fail:
    hlt
    jmp loader_fail
    .balign 4096, 0xcc
loader_fixture_end:

.section .bss,"aw",@nobits
.balign 8
capture_host_rsp: .skip 8
capture_host_rbx: .skip 8
capture_host_rbp: .skip 8
capture_host_r12: .skip 8
capture_host_r13: .skip 8
capture_host_r14: .skip 8
capture_host_r15: .skip 8
capture_host_flags: .skip 8
capture_output: .skip 8
capture_guest_rax: .skip 8
capture_guest_flags: .skip 8
