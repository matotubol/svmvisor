#include "guest-startup.inc"
/* Bounded actual parallel guest workload. Shared aligned qwords at D000h are
 * normal WB RAM: x86 TSO release stores/acquire loads, locked publication fences.
 * AMD APM2 rev3.44 ch7,15.21.4,16.6.1. No host-written guest progress. */
.section .text.concurrent_guest,"ax"
.code64
.global guest_concurrent_start, guest_concurrent_end, guest_concurrent_handler
.global guest_concurrent_wait_start, guest_concurrent_wait_end
.global guest_concurrent_bsp, guest_concurrent_long, guest_concurrent_hlt
.global guest_concurrent_timer_handler, guest_concurrent_timer_hlt
.global guest_concurrent_spin, guest_concurrent_spin_end
.global guest_concurrent_handler_end
.macro wait_peer offset
    mov $0x10000000,%edi
1:  cmp %r13,\offset(%r14)
    jae 2f
    pause
    dec %edi
    jnz 1b
    jmp concurrent_fail
2:
.endm
.macro publish offset
    mov %r13,\offset(%rsi)
    lock orq $0,\offset(%rsi)
.endm
.macro query phase
    mov $\phase,%r11d
    mov $0xc0000100,%ecx
    rdmsr
    test %edx,%edx
    jnz concurrent_fail
    mov %r9d,%edx
    inc %edx
    shl $8,%edx
    cmp %edx,%eax
    jne concurrent_fail
    rdtsc
    shl $32,%rdx
    or %rdx,%rax
    mov %rax,%rbp
    xor %eax,%eax
    vmmcall
.endm
guest_concurrent_start:
    guest_cold_start guest_concurrent_start, concurrent_gdtr, concurrent_protected, concurrent_long, concurrent_idtr
    mov $1,%r9d
guest_concurrent_long:
    xor %eax,%eax
    vmmcall /* executed long-mode startup; before AP publishes any guest progress */
    jmp concurrent_common
guest_concurrent_bsp:
    cli
    xor %r15d,%r15d
    mov $1,%edx
    mov $0xc310,%ebx
    mov $0x01000000,%eax
    mov %eax,(%rbx)
    mov $0xc300,%ebx
    mov $0x500,%eax
    mov %eax,(%rbx)
    mov $0x601,%eax
    mov %eax,(%rbx)
    mov %eax,(%rbx) /* accepted duplicate, never restarts the AP */
concurrent_common:
    cli
    movabs $0x83764d4350553030,%r8
    add %r9,%r8
    cld
    movabs $0x73564d4350553030,%r10
    add %r9,%r10
    mov $0xd000,%esi
    lea (%rsi,%r9,8),%rsi
    mov $1,%r14d
    sub %r9,%r14
    lea 0xd000(,%r14,8),%r14
    mov %r9d,%eax
    inc %eax
    shl $8,%eax
    xor %edx,%edx
    mov $0xc0000100,%ecx
    wrmsr
    mov $1,%r13d
    xor %r15d,%r15d
concurrent_generation:
    cli
    xor %r12d,%r12d
    cmp $5,%r13
    jne 1f
    mov $0x1b,%ecx
    rdmsr
    or $0x400,%eax
    wrmsr
    mov $1,%r15d
1:  mov $0x1ff,%eax
    xor %edx,%edx
    test %r15,%r15
    jnz 2f
    mov $0xc0f0,%ebx
    mov %eax,(%rbx)
    jmp 3f
2:  mov $0x80f,%ecx
    wrmsr
3:  publish 0
    wait_peer 0
    call concurrent_send
    call concurrent_send
    publish 16
    wait_peer 16
    query 0 /* both sends committed, IF0: exactly one pending IRR bit */
    sti
    nop
    mov $1,%eax
    call concurrent_wait_irq
    test %r9,%r9
    jz 6f
    cmp $1,%r13
    jne 6f
    publish 48
6:
    query 1 /* real EOI + IRETQ completion */

    /* Each target publishes readiness and spins without voluntary exits. */
    test %r9,%r9
    jz concurrent_forward
    publish 48
    mov $2,%eax
    call concurrent_wait_irq
    wait_peer 48
    call concurrent_send
    jmp concurrent_directed_done
concurrent_forward:
    wait_peer 48
    call concurrent_send
    publish 48
    mov $2,%eax
    call concurrent_wait_irq
concurrent_directed_done:
    query 2
    publish 80
    wait_peer 80
    call concurrent_send
    mov $3,%eax
    call concurrent_wait_irq
    query 3

    /* Directed HLT wakeups: guest publishes readiness; sender host waits for
     * an exact target-owned drain/park boundary before accepting the guest ICR. */
    test %r9,%r9
    jz 10f
    call concurrent_idle
    jmp 11f
10: wait_peer 112
    call concurrent_send
11: query 4
    test %r9,%r9
    jnz 12f
    call concurrent_idle
    jmp 13f
12: wait_peer 112
    call concurrent_send
13: query 5

    /* Independent local timers on both host/guest CPUs, through each APIC bus.
     * The running loop contains integer instructions only; F0 must preempt it.
     * Timer expiry never changes guest progress cells from the host. */
    mov $6,%r11d
    xor %ebp,%ebp
    call concurrent_timer_program
    mov $0x10000000,%edi
guest_concurrent_spin:
    inc %rbp
    cmp $5,%r12
    je guest_concurrent_spin_end
    dec %edi
    jnz guest_concurrent_spin
    jmp concurrent_fail
guest_concurrent_spin_end:
    query 6
    mov $7,%r11d
    call concurrent_timer_program
    sti
guest_concurrent_timer_hlt:
    hlt
    cmp $6,%r12
    jne concurrent_fail
    query 7

    /* Strict cross-CPU timestamp ordering has an explicit release/acquire
     * handshake, never inferred from host logging order. */
    test %r9,%r9
    jnz 4f
    mov %rbp,64(%rsi)
    publish 96
    jmp 5f
4:  wait_peer 96
    mfence
    rdtsc /* retain sender sample separately */
    shl $32,%rdx
    or %rdx,%rax
    cmp 64(%r14),%rax
    jb concurrent_fail
    mov %rax,64(%rsi)
5:  publish 32
    wait_peer 32
    inc %r13
    cmp $9,%r13
    jb concurrent_generation
    mov $1,%eax
    vmmcall
    ud2

concurrent_idle:
    cli
    publish 112
    sti
guest_concurrent_hlt:
    hlt
    cmp $4,%r12
    jne concurrent_fail
    ret

concurrent_timer_program:
    mov $0xb,%eax /* divide by one */
    xor %edx,%edx
    test %r15,%r15
    jnz 1f
    mov $0xc3e0,%ebx
    mov %eax,(%rbx)
    mov $0x51,%eax /* one-shot, enabled */
    mov $0xc320,%ebx
    mov %eax,(%rbx)
    mov $16384,%eax
    mov $0xc380,%ebx
    mov %eax,(%rbx)
    ret
1:  mov $0x83e,%ecx
    wrmsr
    mov $0x51,%eax
    mov $0x832,%ecx
    wrmsr
    mov $16384,%eax
    mov $0x838,%ecx
    wrmsr
    ret

concurrent_send:
    mov $1,%edx
    sub %r9d,%edx
    mov $0x50,%eax
    test %r15,%r15
    jnz 1f
    shl $24,%edx
    push %rax
    mov %edx,%eax
    mov $0xc310,%ebx
    mov %eax,(%rbx)
    pop %rax
    mov $0xc300,%ebx
    mov %eax,(%rbx)
    ret
1:  mov $0x830,%ecx
    wrmsr
    ret
concurrent_wait_irq:
    mov $0x10000000,%edi
guest_concurrent_wait_start:
    cmp %rax,%r12
    jae 1f
    pause
    dec %edi
    jnz guest_concurrent_wait_start
    jmp concurrent_fail
1:  cmp %rax,%r12
    jne concurrent_fail
guest_concurrent_wait_end:
    ret

guest_concurrent_handler:
    cmp $4,%r12
    jae concurrent_fail
    jmp concurrent_handler_common
guest_concurrent_timer_handler:
    cmp $6,%r11
    jne 1f
    cmp $4,%r12
    jne concurrent_fail
    cmpq $(0x1000+guest_concurrent_spin-guest_concurrent_start),(%rsp)
    jb concurrent_fail
    cmpq $(0x1000+guest_concurrent_spin_end-guest_concurrent_start),(%rsp)
    jae concurrent_fail
    jmp concurrent_handler_common
1:  cmp $7,%r11
    jne concurrent_fail
    cmp $5,%r12
    jne concurrent_fail
    cmpq $(0x1001+guest_concurrent_timer_hlt-guest_concurrent_start),(%rsp)
    jne concurrent_fail
concurrent_handler_common:
    push %rax
    mov 32(%rsp),%rax /* hardware-saved preinterrupt RSP */
    test %r9,%r9
    jnz 7f
    cmp $0x8ff0,%rax
    jb concurrent_fail
    cmp $0x9000,%rax
    ja concurrent_fail
    jmp 8f
7:  cmp $0xaff0,%rax
    jb concurrent_fail
    cmp $0xb000,%rax
    ja concurrent_fail
8:  pop %rax
    cmpq $8,8(%rsp)
    jne concurrent_fail
    testq $0x200,16(%rsp)
    jz concurrent_fail
    cmpq $16,32(%rsp)
    jne concurrent_fail
    cmpq $0x1000,(%rsp)
    jb concurrent_fail
    cmpq $(0x1000+guest_concurrent_end-guest_concurrent_start),(%rsp)
    jae concurrent_fail
    push %rax
    push %rbx
    push %rcx
    push %rdx
    inc %r12
    cmp $6,%r12
    ja concurrent_fail
    xor %eax,%eax
    xor %edx,%edx
    test %r15,%r15
    jnz 1f
    mov $0xc0b0,%ebx
    mov %eax,(%rbx)
    jmp 2f
1:  mov $0x80b,%ecx
    wrmsr
2:  pop %rdx
    pop %rcx
    pop %rbx
    pop %rax
    iretq
guest_concurrent_handler_end:
concurrent_fail:
    mov $0xff,%eax
    vmmcall
    ud2
.balign 8
concurrent_gdtr: .word 47
    .long 0x4000
concurrent_idtr: .word 4095
    .quad 0x3000
guest_concurrent_end:
