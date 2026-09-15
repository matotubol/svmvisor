.text
.globl svmvisor_fixture_apic_fault
svmvisor_fixture_apic_fault:
    /* Win64: ECX index, EDX write flag. Both invalid reads and writes must GP. */
    movq $0, apic_contract_gp_count(%rip)
    test %edx, %edx
    jz apic_contract_read
    xor %eax, %eax
    xor %edx, %edx
apic_contract_write_fault:
    wrmsr
    jmp apic_contract_done
apic_contract_read:
apic_contract_read_fault:
    rdmsr
apic_contract_done:
    mov apic_contract_gp_count(%rip), %rax
    ret

.globl svmvisor_fixture_apic_gp
svmvisor_fixture_apic_gp:
    push %rax
    cmpq $0, 8(%rsp)
    jne apic_contract_unexpected
    lea apic_contract_write_fault(%rip), %rax
    cmp %rax, 16(%rsp)
    je apic_contract_expected
    lea apic_contract_read_fault(%rip), %rax
    cmp %rax, 16(%rsp)
    jne apic_contract_unexpected
apic_contract_expected:
    addq $2, 16(%rsp)
    incq apic_contract_gp_count(%rip)
    pop %rax
    add $8, %rsp
    iretq
apic_contract_unexpected:
    ud2
.data
.p2align 3
apic_contract_gp_count:
    .quad 0
