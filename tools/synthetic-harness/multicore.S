#include "guest-startup.inc"
/* Two guest CPUs multiplexed by the existing host bridge. All addresses below
 * are guest addresses, resolved as constant offsets within immutable CODE.
 * APM2rev3.44 14.6,15.27.8,16.6.1: actual real16->protected32->long64. */
.section .text.smp_guest,"ax"
.global guest_smp_start, guest_smp_end, guest_smp_bsp, guest_smp_real_witness
.global guest_smp_long_witness, guest_smp_handler
.macro smp_query
    mov $0xc0000100,%ecx
    rdmsr
    test %edx,%edx
    jnz smp_fail
    mov %r9d,%edx
    inc %edx
    shl $8,%edx
    cmp %edx,%eax
    jne smp_fail
    rdtsc
    shl $32,%rdx
    or %rdx,%rax
    mov %rax,%rbp
    xor %eax,%eax
    vmmcall
.endm
.code16
guest_smp_start:
guest_smp_real_witness:
    guest_cold_start guest_smp_start, smp_gdtr, smp_protected, smp_long, smp_idtr
    mov $1,%r9d
    mov $0xb000,%r8d
    xor %r15d,%r15d
    mov $0x1b,%ecx
    rdmsr
    test $0x400,%eax
    setnz %r15b
    call smp_identity
    xor %r11d,%r11d
    xor %r12d,%r12d
    movabs $0x73564d4350553031,%r10
guest_smp_long_witness:
    smp_query /* AP0: longmode, IF0 */
    inc %r11
    smp_query /* AP1: pending fixed coalesced while IF0 */
    lea smp_ap_ready(%rip),%r14
    sti
    nop
smp_ap_ready:
    cmp $1,%r12
    jne smp_fail
    inc %r11
    smp_query /* AP2: handler+IRETQ */
    xor %edx,%edx /* destination BSP0 */
    mov $0x50,%eax
    call smp_icr
    xor %edx,%edx
    mov $0x50,%eax
    call smp_icr
    inc %r11
    smp_query /* AP3: reverse queued */
    cmp $1,%r12
    jne smp_fail
    inc %r11
    smp_query /* AP4: extra enabled entry no duplicate */
    mov $1,%eax
    vmmcall
    ud2

guest_smp_bsp:
    cli
    mov $0x9000,%r8d
    xor %r9d,%r9d
    movabs $0x73564d4350553030,%r10
    xor %r11d,%r11d
    xor %r12d,%r12d
    call smp_identity
    mov $1,%edx
    mov $0x500,%eax
    call smp_icr
    mov $1,%edx
    mov $0x601,%eax
    call smp_icr
    mov $1,%edx
    mov $0x601,%eax
    call smp_icr /* duplicate before any AP entry */
    smp_query /* BSP0 */
    mov $1,%edx
    mov $0x50,%eax
    call smp_icr
    mov $1,%edx
    mov $0x50,%eax
    call smp_icr
    inc %r11
    smp_query /* BSP1 */
    inc %r11
    smp_query /* BSP2: pending reverse while IF0 */
    lea smp_bsp_ready(%rip),%r14
    sti
    nop
smp_bsp_ready:
    cmp $1,%r12
    jne smp_fail
    inc %r11
    smp_query /* BSP3 */
    mov $1,%edx
    mov $0x601,%eax
    call smp_icr /* duplicate after AP executed; preserve AP CPU state */
    inc %r11
    smp_query /* BSP4 */
    cmp $1,%r12
    jne smp_fail
    mov $1,%eax
    vmmcall
    ud2

smp_identity:
    test %r15,%r15
    jnz 1f
    mov $0xc020,%ebx
    mov (%rbx),%eax
    shr $24,%eax
    jmp 2f
1:  mov $0x802,%ecx
    rdmsr
2:  cmp %r9d,%eax
    jne smp_fail
    mov $0x1b,%ecx
    rdmsr
    shr $8,%eax
    and $1,%eax
    xor $1,%eax
    cmp %r9d,%eax
    jne smp_fail
    mov %r9d,%eax
    inc %eax
    shl $8,%eax
    xor %edx,%edx
    mov $0xc0000100,%ecx
    wrmsr
    mov $0x1ff,%eax
    test %r15,%r15
    jnz 3f
    mov $0xc0f0,%ebx
    mov %eax,(%rbx)
    ret
3:  mov $0x80f,%ecx
    wrmsr
    ret
smp_icr:
    test %r15,%r15
    jnz 1f
    mov %eax,%esi
    mov %edx,%eax
    shl $24,%eax
    mov $0xc310,%ebx
    mov %eax,(%rbx)
    mov %esi,%eax
    mov $0xc300,%ebx
    mov %eax,(%rbx)
    ret
1:  mov $0x830,%ecx
    wrmsr
    ret

guest_smp_handler:
    cmp %r14,(%rsp)
    jne smp_fail
    cmpq $8,8(%rsp)
    jne smp_fail
    testq $0x200,16(%rsp)
    jz smp_fail
    cmp %r8,24(%rsp)
    jne smp_fail
    cmpq $16,32(%rsp)
    jne smp_fail
    inc %r12
    cmp $1,%r12
    jne smp_fail
    xor %eax,%eax
    xor %edx,%edx
    test %r15,%r15
    jnz 1f
    mov $0xc0b0,%ebx
    mov %eax,(%rbx)
    iretq
1:  mov $0x80b,%ecx
    wrmsr
    iretq
.global guest_smp_bad_destination, guest_smp_running_init
guest_smp_bad_destination:
    mov $2,%edx
    mov $0x50,%eax
    call smp_icr
    jmp smp_fail
guest_smp_running_init:
    mov $1,%edx
    mov $0x500,%eax
    call smp_icr
    jmp smp_fail
smp_fail:
    mov $0xff,%eax
    vmmcall
    ud2
.balign 8
smp_gdtr: .word 47
    .long 0x4000
smp_idtr: .word 4095
    .quad 0x3000
guest_smp_end:
