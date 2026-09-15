.text
.globl svmvisor_fixture_vmcr_mbz
svmvisor_fixture_vmcr_mbz:
    movq $0, vmcr_gp_count(%rip)
    mov $0xc0010114, %ecx
    mov $0x20, %eax
    xor %edx, %edx
vmcr_fault_instruction:
    wrmsr
    mov vmcr_gp_count(%rip), %rax
    ret

.globl svmvisor_fixture_vmcr_gp
svmvisor_fixture_vmcr_gp:
    push %rax
    cmpq $0, 8(%rsp)
    jne vmcr_unexpected_fault
    lea vmcr_fault_instruction(%rip), %rax
    cmp %rax, 16(%rsp)
    jne vmcr_unexpected_fault
    addq $2, 16(%rsp)
    incq vmcr_gp_count(%rip)
    pop %rax
    add $8, %rsp
    iretq
vmcr_unexpected_fault:
    ud2
.data
.p2align 3
vmcr_gp_count:
    .quad 0
