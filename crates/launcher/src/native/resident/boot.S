/* UEFI2.11 7.4.6: only original EFI_SUCCESS transfers execution ownership.
 * The two-argument x64 service receives untouched RCX/RDX. Failure returns
 * directly. After success reuse the qualified full native/xstate boundary;
 * the actual resident callback is nested inside physical::start. */
.text
.p2align 4
.globl svmvisor_exit_boot_services
svmvisor_exit_boot_services:
    sub $40, %rsp
    call *svmvisor_original_exit_boot_services(%rip)
    lea 40(%rsp), %rsp
    pushfq
    test %rax, %rax
    jnz .Lebs_return
    sub $32, %rsp
    call svmvisor_boot_boundary
    lea 32(%rsp), %rsp
    cmpl $1, svmvisor_boot_completed(%rip)
    jne .Lebs_terminal
.Lebs_return:
    popfq
    ret
.Lebs_terminal:
    /* Successful EBS cannot become an EFI retry error. Preserve the bounded
     * failure record and retain all already active CPUs/resources. */
    cli
1:  hlt
    jmp 1b

#define SVMVISOR_RESIDENT_CALLBACK 1
#define efi_main svmvisor_boot_boundary
#define svmvisor_native_efi_main_inner svmvisor_boot_inner
/* These unused labels belong to this outer capture, never to the resident
 * callback's actual guest continuation. Keep that qualified object unchanged. */
#define svmvisor_resident_guest_resume svmvisor_boot_unused_resume
#define svmvisor_resident_guest_ack svmvisor_boot_unused_ack
#define svmvisor_resident_guest_after_ack svmvisor_boot_unused_after_ack
#include "../admission/boundary.S"
