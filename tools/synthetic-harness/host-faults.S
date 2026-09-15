/* Emulator-only terminal host fault path. No Rust, return, or recovery.
 * Each entry leaves vector, error code, then the architectural frame on RSP.
 * AMD APM rev3.44 chapter8 defines exceptions with hardware error codes.
 */
.section .text,"ax"
.code64
.altmacro
.macro HOST_FAULT_STUB vector
host_fault_\vector:
    .if (\vector != 8) && (\vector != 10) && (\vector != 11) && (\vector != 12) && (\vector != 13) && (\vector != 14) && (\vector != 17) && (\vector != 21) && (\vector != 29) && (\vector != 30)
    pushq $0
    .endif
    pushq $\vector
    jmp host_fault_common
.endm
.set vector, 0
.rept 256
    HOST_FAULT_STUB %vector
    .set vector, vector + 1
.endr

host_fault_common:
    cli
    cld
    mov (%rsp), %rbx
    lea host_fault_message(%rip), %rsi
    mov $0xe9, %dx
1:
    lodsb
    test %al, %al
    jz 2f
    out %al, %dx
    jmp 1b
2:
    mov $60, %ecx
3:
    mov %rbx, %rax
    shr %cl, %rax
    and $15, %eax
    lea host_fault_digits(%rip), %rsi
    mov (%rsi,%rax), %al
    out %al, %dx
    sub $4, %ecx
    jns 3b
    mov $10, %al
    out %al, %dx
    cmp $14, %rbx
    jne 7f
    lea host_page_fault_error(%rip), %rsi
    call host_fault_puts
    mov 8(%rsp), %rax
    call host_fault_hex
    lea host_page_fault_address(%rip), %rsi
    call host_fault_puts
    mov %cr2, %rax
    call host_fault_hex
    mov $10, %al
    out %al, %dx
7:
    cmp $8, %rbx
    jne host_fault_failure
    /* Require the actual static IST1 backing and the architectural #DF error.
     * RSP points at vector + error + 40-byte long-mode interrupt frame.
     */
    lea host_double_fault_stack(%rip), %rax
    cmp %rax, %rsp
    jb host_fault_failure
    lea host_double_fault_stack_end-56(%rip), %rax
    cmp %rax, %rsp
    ja host_fault_failure
    cmpq $0, 8(%rsp)
    jne host_fault_failure
    lea host_double_fault_pass(%rip), %rsi
5:
    lodsb
    test %al, %al
    jz 6f
    out %al, %dx
    jmp 5b
6:
    mov $0xf4, %dx
    mov $18, %eax
    out %eax, %dx
    jmp 4f
host_fault_failure:
    mov $0xf4, %dx
    mov $17, %eax
    out %eax, %dx
4:
    hlt
    jmp 4b

host_fault_puts:
    lodsb
    test %al, %al
    jz 8f
    out %al, %dx
    jmp host_fault_puts
8:
    ret

host_fault_hex:
    mov %rax, %r8
    mov $60, %ecx
9:
    mov %r8, %rax
    shr %cl, %rax
    and $15, %eax
    lea host_fault_digits(%rip), %rsi
    mov (%rsi,%rax), %al
    out %al, %dx
    sub $4, %ecx
    jns 9b
    ret

.global host_fault_probe
.type host_fault_probe,@function
host_fault_probe:
    ud2
    jmp host_fault_probe
.size host_fault_probe, .-host_fault_probe

.global host_double_fault_trigger
.type host_double_fault_trigger,@function
host_double_fault_trigger:
    /* Selector 0x28 starts beyond the installed 40-byte GDT, causing #GP.
     * The probe cleared #GP's present bit, so its delivery generates #NP.
     * Both exceptions are contributory: the CPU raises #DF, selecting IST1.
     */
    mov $0x28, %ax
    mov %ax, %ds
    ud2 /* Unexpected successful load must never report double-fault success. */
    jmp host_double_fault_trigger
.size host_double_fault_trigger, .-host_double_fault_trigger

.global host_stack_overflow_trigger
.type host_stack_overflow_trigger,@function
host_stack_overflow_trigger:
    /* First push crosses into the unmapped low guard. Delivering the resulting
     * #PF on this exhausted stack faults again, causing #DF to select IST1.
     * Unlike the other DF probe, this leaves every IDT gate intact.
     */
    lea host_stack_guard_end(%rip), %rsp
    pushq $0
    ud2 /* A mapped guard must never report a successful double fault. */
    jmp host_stack_overflow_trigger
.size host_stack_overflow_trigger, .-host_stack_overflow_trigger

.section .rodata,"a"
host_fault_message: .asciz "HOST-FAULT vector="
host_fault_digits: .ascii "0123456789abcdef"
host_double_fault_pass: .asciz "DOUBLE-FAULT-IST-PASS\n"
host_page_fault_error: .asciz "HOST-PF error="
host_page_fault_address: .asciz " cr2="
.balign 8
.global host_fault_handlers
.type host_fault_handlers,@object
host_fault_handlers:
.macro HOST_FAULT_POINTER vector
    .quad host_fault_\vector
.endm
.set vector, 0
.rept 256
    HOST_FAULT_POINTER %vector
    .set vector, vector + 1
.endr
.size host_fault_handlers, .-host_fault_handlers

/* Host mapping installation must leave both guards unmapped. Only the middle
 * 16 KiB is the double-fault stack; its top is the first byte of the high guard.
 */
.section .bss,"aw",@nobits
.balign 4096
.global host_df_guard, host_df_guard_end
host_df_guard: .skip 4096
host_df_guard_end:
.global host_double_fault_stack, host_double_fault_stack_end
.type host_double_fault_stack,@object
host_double_fault_stack: .skip 16384
host_double_fault_stack_end:
.size host_double_fault_stack, .-host_double_fault_stack
.global host_df_high_guard, host_df_high_guard_end
host_df_high_guard: .skip 4096
host_df_high_guard_end:
.section .note.GNU-stack,"",@progbits
