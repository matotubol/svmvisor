/* Emulator-only eager x87/MMX/SSE/AVX boundary. AMD APM Vol2 rev3.44 11.4/11.5.
 * Context offsets: host_save0, guest_save8, observed16, mask24, flags32,
 * host_seed40, caller_save48, clock_switch56, host_aux64, observed_aux72,
 * guest_xcr0=80, observed_guest_xcr0=88, observed_host_xcr0=96.
 * mask=0 selects FX64, otherwise XSAVE64 host mask3/7; guest XCR0 is1/3/7.
 * All areas are separately owned, initialized, 64-byte aligned and >=4096.
 * The caller validates capabilities and guest XCR0 and intercepts XSETBV.
 * Host CR4.OSXSAVE is set throughout the host boundary, independent of guest
 * CR4. All admitted components are saved/restored under the full host mask.
 */
.section .text,"ax"
.code64
/* Configure bounded fixed XCR0; caller has checked CPUID before this call. */
.global xstate_enable
.type xstate_enable,@function
xstate_enable:
    mov $0xc0000080, %ecx
    rdmsr
    and $0xffffbfff, %eax /* FFXSR must never omit XMM state. */
    wrmsr
    test %rdi, %rdi
    jz 1f
    mov %cr4, %rax
    or $0x40000, %eax
    mov %rax, %cr4
    xor %ecx, %ecx
    mov %edi, %eax
    xor %edx, %edx
    xsetbv
1:  ret
.size xstate_enable,.-xstate_enable

/* Private helper ABI r10=context, r11=area. Only RAX/RDX are clobbered. */
.global xstate_save_area
.type xstate_save_area,@function
xstate_save_area:
    mov 24(%r10), %rax
    test %rax, %rax
    jz 1f
    xor %edx, %edx
    xsave64 (%r11)
    ret
1:  fxsave64 (%r11)
    ret
.size xstate_save_area,.-xstate_save_area
.global xstate_restore_area
.type xstate_restore_area,@function
xstate_restore_area:
    mov 24(%r10), %rax
    test %rax, %rax
    jz 1f
    xor %edx, %edx
    xrstor64 (%r11)
    ret
1:  fxrstor64 (%r11)
    ret
.size xstate_restore_area,.-xstate_restore_area

/* Only integer instructions intervene between guest capture and host restore.
 * Save the Rust caller separately so the fixture seed cannot affect its ABI.
 * XCR0 limits XSAVE/XRSTOR, not legacy SSE access. Restoring the full guest
 * image before lowering XCR0 prevents a disabled component retaining host
 * data. Re-enabling the full mask before capture preserves disabled state
 * across exits and later re-enable (APM2 11.5.2,11.5.7,11.5.8).
 * Enter/leave clobber RAX/RCX/RDX/R11; guest GPRs are outside this window.
 */
.global xstate_enter
.type xstate_enter,@function
xstate_enter:
    mov 48(%r10), %r11
    call xstate_save_area
    mov 40(%r10), %r11
    call xstate_restore_area
    mov 0(%r10), %r11
    call xstate_save_area
    testq $1, 32(%r10) /* Deliberate missing-restore negative fixture. */
    jnz 1f
    mov 8(%r10), %r11
    call xstate_restore_area
1:  cmpq $0, 24(%r10)
    je 2f
    mov 80(%r10), %eax
    xor %edx, %edx
    xor %ecx, %ecx
    xsetbv
2:  ret
.size xstate_enter,.-xstate_enter
.global xstate_leave
.type xstate_leave,@function
xstate_leave:
    cmpq $0, 24(%r10)
    je 2f
    xor %ecx, %ecx
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 88(%r10)
    mov 24(%r10), %eax
    xor %edx, %edx
    xsetbv
    xgetbv
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 96(%r10)
2:
    mov 8(%r10), %r11
    call xstate_save_area
    testq $2, 32(%r10) /* Independent missing-host-restore fixture. */
    jnz 1f
    mov 0(%r10), %r11
    call xstate_restore_area
1:  mov 16(%r10), %r11
    call xstate_save_area
    mov 48(%r10), %r11
    call xstate_restore_area
    ret
.size xstate_leave,.-xstate_leave

/* Clock controls: AMD APM3.44 15.30.5; PPR57896rev3.00 C0000103/0104.
 * r10 remains the outer context; r11 is exclusively owned Clock Switch.
 * Clobbers RAX/RCX/RDX/r11 only. Guest GPRs must already be saved on leave.
 * No guest memory, external call, allocation or logging in this boundary.
 */
.macro clock_read msr, offset
    mov $\msr, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, \offset(%r11)
.endm
.macro clock_write msr, offset
    mov $\msr, %ecx
    mov \offset(%r11), %rax
    mov %rax, %rdx
    shr $32, %rdx
    wrmsr
.endm
.global clock_enter
.type clock_enter,@function
clock_enter:
    mov 56(%r10), %r11
    testq $1, (%r11)
    jz 1f
    clock_read 0xc0000103, 24
    clock_write 0xc0000103, 8
1:  testq $2, (%r11)
    jz 2f
    clock_read 0xc0000104, 32
    clock_write 0xc0000104, 16
2:  ret
.size clock_enter,.-clock_enter
.global clock_leave
.type clock_leave,@function
clock_leave:
    mov 56(%r10), %r11
    testq $1, (%r11)
    jz 1f
    clock_read 0xc0000103, 40
    clock_write 0xc0000103, 24
    clock_read 0xc0000103, 56
1:  testq $2, (%r11)
    jz 2f
    clock_read 0xc0000104, 48
    clock_write 0xc0000104, 32
    clock_read 0xc0000104, 64
2:  movq $1, 72(%r11)
    ret
.size clock_leave,.-clock_leave

