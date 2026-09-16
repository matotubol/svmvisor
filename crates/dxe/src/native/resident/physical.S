/* Physical INIT/SIPI bootstrap, AMD APM2 rev3.44 14.1/14.1.3 and 16.5.
 * Copied to one admitted low page; broadcast startup, serialized capture.
 * Each AP selects its admitted identity before stack use. No xstate mutation.
 * Page-relative fields and absolute image operands are patched before INIT.
 */
.section .rdata,"dr"
.code16
.globl svmvisor_ap_trampoline, svmvisor_ap_trampoline_end
svmvisor_ap_trampoline:
    cli
    cld
    mov %cs, %ax
    mov %ax, %ds
    lgdt (ap_gdtr-svmvisor_ap_trampoline)
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0
    .byte 0x66, 0xea
.globl svmvisor_ap_protected_target
svmvisor_ap_protected_target: .long 0
    .word 24
.code32
.globl svmvisor_ap_protected
svmvisor_ap_protected:
    mov $16, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    .byte 0xb8
.globl svmvisor_ap_root
svmvisor_ap_root: .long 0
    mov %eax, %cr3
    mov $0x620, %eax
    mov %eax, %cr4
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax
    wrmsr
    mov %cr0, %eax
    or $0x80010033, %eax
    mov %eax, %cr0
    .byte 0xea
.globl svmvisor_ap_long_target
svmvisor_ap_long_target: .long 0
    .word 8
.balign 8
.globl svmvisor_ap_gdt
svmvisor_ap_gdt:
    .quad 0, 0x00af9b000000ffff, 0x00cf93000000ffff, 0x00cf9b000000ffff
ap_gdtr:
    .word 31
.globl svmvisor_ap_gdt_base
svmvisor_ap_gdt_base: .long 0
svmvisor_ap_trampoline_end:
.text
.code64
.globl svmvisor_ap_entry64
svmvisor_ap_entry64:
    /* No stack, call, or shared scratch before matching the actual CPU.
     * Match the inventory's full AMD TOPOEXT identity when advertised; never
     * let an unexpected high identity alias an admitted eight-bit stack.
     * The immutable directory has a null BSP slot and no unassigned entries. */
    mov $0x80000000, %eax
    xor %ecx, %ecx
    cpuid
    mov %eax, %edi
    cmp $0x80000001, %eax
    jb .Lap_legacy_identity
    mov $0x80000001, %eax
    xor %ecx, %ecx
    cpuid
    test $0x400000, %ecx
    jz .Lap_legacy_identity
    cmp $0x8000001e, %edi
    jb .Lap_discovery_failed
    mov $0x8000001e, %eax
    xor %ecx, %ecx
    cpuid
    mov %eax, %ebx
    jmp .Lap_identity_ready
.Lap_legacy_identity:
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    shr $24, %ebx
.Lap_identity_ready:
    lea svmvisor_ap_boots(%rip), %r9
    mov svmvisor_ap_count(%rip), %edi
    test %edi, %edi
    jz .Lap_discovery_failed
    cmp $32, %edi
    ja .Lap_discovery_failed
    xor %esi, %esi
.Lap_discover:
    mov (%r9,%rsi,8), %r12
    test %r12, %r12
    jz .Lap_next
    cmp 72(%r12), %ebx
    je .Lap_claim
.Lap_next:
    inc %esi
    cmp %edi, %esi
    jb .Lap_discover
.Lap_discovery_failed:
    lock orl $1, svmvisor_ap_discovery_failed(%rip)
    jmp svmvisor_ap_fault
.Lap_claim:
    lock btsl $0, 104(%r12)
    jc .Lap_discovery_failed
    /* The BSP releases exactly one AP into Rust capture, after the preceding
     * AP publishes completion from its retained copied guest wait code.
     * Finite iteration bound; not a calibrated physical startup deadline. */
    mov $0x7fffffff, %edi
.Lap_release_wait:
    cmpl $1, 108(%r12)
    je .Lap_released
    pause
    dec %edi
    jnz .Lap_release_wait
    jmp .Lap_discovery_failed
.Lap_released:
    mov 0(%r12), %rsp
    lgdt 8(%r12)
    lidt 24(%r12)
    xor %eax, %eax
    mov %ax, %fs
    mov %ax, %gs
    lldt %ax
    mov $24, %ax
    ltr %ax
    xor %ebp, %ebp
    sub $32, %rsp
    /* All pre-capture bootstrap code is integer assembly. INIT preserves
     * xstate; no Rust/compiler FP instruction runs before the native capture. */
    /* APM2 16.9/Table16-5: x2APIC is the only admitted bus. Never WRMSR
     * EXTD without the capability bit. */
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    test $0x200000, %ecx
    jz svmvisor_ap_fault
    mov $0x1b, %ecx
    rdmsr
    test $0x800, %eax
    jz svmvisor_ap_fault
    /* INIT preserves APIC base and mode (APM2 16.10). Only the already
     * admitted fixed aperture is captured; an enabled xAPIC is promoted. */
    test %edx, %edx
    jnz svmvisor_ap_fault
    mov %eax, %r8d
    and $0xfffff000, %r8d
    cmp $0xfee00000, %r8d
    jne svmvisor_ap_fault
    test $0x400, %eax
    jnz .Lap_mode_ready
    or $0x400, %eax
    wrmsr
    rdmsr
    test $0x400, %eax
    jz svmvisor_ap_fault
.Lap_mode_ready:
    xor %ecx, %ecx
    mov $0x53564d52, %edx
    call svmvisor_resident_callback
    /* The EFI callee owns its 32-byte home area. Copy its bounded integer
     * result after all incoming GPRs are restored, before entering retained
     * wait code. BOOT+120..135 is private to this claimed AP until failure. */
    mov (%rsp), %eax
    mov %eax, 120(%r12)
    mov 8(%rsp), %rax
    mov %rax, 128(%r12)
    /* Enter retained copied code before publishing completion. No PE runtime
     * fixup or converted firmware pointer can alter this waiting continuation. */
    jmp *80(%r12)
.globl svmvisor_ap_fault
svmvisor_ap_fault:
    cli
1:  hlt
    jmp 1b

/* Copied verbatim to BOOT+256 before startup. All branches are local and all
 * physical pointers are numeric BOOT fields written before publication. There
 * are no COFF relocations in this span (UEFI2.11 8.4.1 reapplies PE fixups).
 * R12 is restored by the captured callback as required by the x64 EFI ABI. */
.section .rdata,"dr"
.code64
.globl svmvisor_ap_wait, svmvisor_ap_wait_end, svmvisor_ap_wait_fault
svmvisor_ap_wait:
    /* Refusal evidence outranks missing ACK, which is its consequence. */
    cmpl $0, 120(%r12)
    jne .Lap_wait_failed
    mov $1, %eax
    xor %ecx, %ecx
    cpuid
    shr $24, %ebx
    cmp 72(%r12), %ebx
    jne .Lap_wait_identity_failed
    /* A refused native callback returns too. Only its post-ACK epilogue
     * publishes this slot bit; no test-only CPUID response is admitted. */
    mov 112(%r12), %rdx
    mov 48(%r12), %eax
    test %eax, (%rdx)
    jz .Lap_wait_ack_failed
    /* Publish the actual guest root before completion. The BSP checks this
     * independently of instruction execution or any retained TLB entries. */
    mov %cr3, %rax
    mov %rax, 96(%r12)
    mov 48(%r12), %eax
    mov 56(%r12), %rdx
    lock or %eax, (%rdx)
    cmpl $0, 88(%r12)
    jne .Lap_wait_cpuid
.Lap_wait_pause:
    pause
    jmp .Lap_wait_pause
.Lap_wait_cpuid:
    xor %eax, %eax
    xor %ecx, %ecx
    cpuid
    pause
    jmp .Lap_wait_cpuid
    /* Failure records are final before the locked mask publication below.
     * The BSP reads them only after acquiring that publication. */
.Lap_wait_identity_failed:
    movl $0x81, 120(%r12)
    mov %ebx, 128(%r12)
    mov 72(%r12), %eax
    mov %eax, 132(%r12)
    jmp .Lap_wait_failed
.Lap_wait_ack_failed:
    movl $0x82, 120(%r12)
    mov %eax, 132(%r12)
    mov (%rdx), %eax
    mov %eax, 128(%r12)
.Lap_wait_failed:
    mov 48(%r12), %eax
    mov 64(%r12), %rdx
    lock or %eax, (%rdx)
svmvisor_ap_wait_fault:
    cli
.Lap_wait_halt:
    hlt
    jmp .Lap_wait_halt
svmvisor_ap_wait_end:


