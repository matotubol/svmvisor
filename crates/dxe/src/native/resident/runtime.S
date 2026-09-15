/* Resident one-CPU native continuation world switch.
 * AMD APM2 rev3.44 15.5.1/15.5.2/15.7/15.13.1; BridgeContext in bridge.rs.
 * Windows x64 ABI: RCX=context; dispatched function RCX=context, AL=resume.
 * Link this complete object and dispatcher into retained private monitor code.
 * No absolute symbol/data references and no calls into the original DXE image.
 * The incoming guest auxiliary state is already in its VMCB. Host and guest
 * stacks/roots differ. No host x87/MMX/SIMD, xstate/XCR0 changes or migration;
 * extended state and DR0-3 remain live and guest-owned throughout the loop.
 * Only a successful target-owned guest INIT clears live DR0-3 in the audited
 * core helper; VMCB DR6/7 reset separately. Private INIT/#SX never resets them.
 * Entry is nonreturning. The startup profile opens STGI with IF=0 only in
 * its private INIT-to-SX acknowledgment window; no recovery after VMRUN.
 */
.text
.p2align 4
.globl svmvisor_resident_enter
svmvisor_resident_enter:
    cli
    clgi
    mov %rcx, %r10
    /* Commit point: guest original EFER/CR0 are already in its VMCB. Host
     * private NX/WP permissions require these host-only enablements before
     * using the private paging root. APM2 3.1/5.3 and 15.5/15.6. */
    mov $0xc0000080, %ecx
    rdmsr
    or $0x800, %eax
    wrmsr
    mov 8(%r10), %rax
    mov %rax, %cr3
    mov %cr0, %rax
    or $0x10000, %rax
    mov %rax, %cr0
    mov 0(%r10), %rsp
    sub $16, %rsp
    mov %r10, (%rsp) /* Host-only context, preserved by VMRUN's host RSP. */
    mov 56(%r10), %rax
    lgdt (%rax)
    pushq 88(%r10)
    lea .Lresident_cs(%rip), %rax
    push %rax
    lretq
.Lresident_cs:
    mov 80(%r10), %ax
    mov %ax, %ss
    mov %ax, %ds
    mov %ax, %es
    xor %eax, %eax
    mov %ax, %fs
    mov %ax, %gs
    lldt %ax
    mov 72(%r10), %ax
    ltr %ax
    /* Terminal gates select private IST1: its TSS must already be loaded. */
    mov 64(%r10), %rax
    lidt (%rax)
    /* A private host has no firmware/guest TLS. Guest bases are already in the
     * immutable native capture and mutable guest VMCB, before these writes. */
    xor %eax, %eax
    xor %edx, %edx
    mov $0xc0000100, %ecx
    wrmsr
    inc %ecx
    wrmsr
    inc %ecx
    wrmsr
    mov 48(%r10), %rax
    mov %rax, %rdx
    shr $32, %rdx
    mov $0xc0010117, %ecx
    wrmsr
    mov 32(%r10), %rax
    vmsave %rax
.Lresident_resume:
    mov (%rsp), %r10
    /* Host IF/GIF remain clear. Native guest IF/TPR and physical LAPIC state
     * own ordinary IRQ delivery; never overwrite the guest-live CR8 here. */
    mov 16(%r10), %rax
    /* Host-private entry provenance before guest GPRs become live. Offsets
     * are pinned by RawVmexitCapture's compile-time assertions in runtime.rs. */
.globl svmvisor_resident_entry_capture
svmvisor_resident_entry_capture:
    lea svmvisor_resident_raw_vmexit(%rip), %r11
    addq $1, 0(%r11)
    jnz .Lentry_sequence_ready
    addq $1, 0(%r11)
.Lentry_sequence_ready:
    mov %rax, 16(%r11)
    mov 0x578(%rax), %rdx
    mov %rdx, 24(%r11)
    mov 0x558(%rax), %rdx
    mov %rdx, 136(%r11)
    vmload %rax
    mov 40(%r10), %rsi
    mov 0(%rsi), %rcx
    mov 8(%rsi), %rdx
    mov 16(%rsi), %rbx
    mov 24(%rsi), %rbp
    mov 40(%rsi), %rdi
    mov 48(%rsi), %r8
    mov 56(%rsi), %r9
    mov 64(%rsi), %r10
    mov 72(%rsi), %r11
    mov 80(%rsi), %r12
    mov 88(%rsi), %r13
    mov 96(%rsi), %r14
    mov 104(%rsi), %r15
    mov 32(%rsi), %rsi
.globl svmvisor_resident_vmrun
svmvisor_resident_vmrun:
    vmrun %rax
.globl svmvisor_resident_vmexit
svmvisor_resident_vmexit:
    cli
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
    /* All 14 guest GPRs are now on the host stack. RAX is the restored
     * VMRUN physical operand, not guest RAX (which is at VMCB+5f8).
     * Capture precedes VMSAVE, host VMLOAD and every Rust/INIT mutation.
     * No calls, guest stores, FP/SIMD, or guest-stack accesses occur here. */
.globl svmvisor_resident_exit_capture
svmvisor_resident_exit_capture:
    lea svmvisor_resident_raw_vmexit(%rip), %rdi
    movq $0, 8(%rdi)
    mov %rax, 32(%rdi)
    mov 16(%r10), %r11
    mov %r11, 144(%rdi)
    mov 0x578(%rax), %r11
    mov %r11, 40(%rdi)
    mov 0x0c8(%rax), %r11
    mov %r11, 48(%rdi)
    mov 0x070(%rax), %r11
    mov %r11, 56(%rdi)
    mov 0x558(%rax), %r11
    mov %r11, 64(%rdi)
    mov 0x550(%rax), %r11
    mov %r11, 72(%rdi)
    mov 0x5f8(%rax), %r11
    mov %r11, 80(%rdi)
    mov 8(%rsp), %r11
    mov %r11, 88(%rdi)
    mov (%rsp), %r11
    mov %r11, 128(%rdi)
    mov %cr0, %r11
    mov %r11, 112(%rdi)
    movq $-1, 96(%rdi)
    movq $-1, 104(%rdi)
    movq $-1, 120(%rdi)
    movq $0, 152(%rdi)
    cmpq $0x7c, 56(%rdi)
    jne .Lraw_capture_complete
    /* The admitted platform uses an 8-bit initial APIC ID. CPUID is native
     * here; its clobbered GPRs are already saved. Optional physical MSR reads
     * are restricted to the reviewed target's actual SYS_CFG boundary. */
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    shr $24, %ebx
    mov %rbx, 120(%rdi)
    cmp $0x00b40f40, %eax
    jne .Lraw_capture_complete
    cmpl $0xc0010010, 128(%rdi)
    jne .Lraw_capture_complete
    mov $0x2ff, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 96(%rdi)
    mov $0xc0010010, %ecx
    rdmsr
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, 104(%rdi)
    movq $1, 152(%rdi)
.Lraw_capture_complete:
    mov 0(%rdi), %r11
    mov %r11, 8(%rdi) /* Commit the tuple last; no writer until next entry. */
.globl svmvisor_resident_exit_capture_end
svmvisor_resident_exit_capture_end:
    mov 16(%r10), %rax
    vmsave %rax
    mov 32(%r10), %rax
    vmload %rax
    cld
    mov %rsp, %rsi
    mov 40(%r10), %rdi
    mov $14, %ecx
    rep movsq
    add $112, %rsp
    mov (%rsp), %rcx
    sub $32, %rsp /* Required shadow space; stack remains call-aligned. */
    call *96(%rcx)
    add $32, %rsp
    test %al, %al
    jnz .Lresident_resume
.globl svmvisor_resident_terminal
svmvisor_resident_terminal:
    cli
    hlt
    jmp svmvisor_resident_terminal
/* Private redirected-INIT gate. APM2 rev3.44 15.21.8/Table15-12,15.28:
 * vector30 pushes error1 and consumes INIT without architectural CPU/APIC
 * reset. IF stays clear; CLGI closes the window before touching host state.
 * No GPR is changed. Fault-class saved RIP is resumed unchanged. IRETQ
 * restores saved host flags after removing exactly the error-code qword.
 * Neither this gate nor its caller acknowledges any maskable APIC source. */
.globl svmvisor_resident_sx
svmvisor_resident_sx:
    clgi
    cmpq $1, (%rsp)
    jne svmvisor_resident_fault_30
    lock incq svmvisor_resident_init_acks(%rip)
    add $8, %rsp
    iretq
.globl svmvisor_resident_runtime_end
svmvisor_resident_runtime_end:
