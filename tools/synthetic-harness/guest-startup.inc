/* Shared fixed-emulator cold AP trampoline. APM2 rev3.44 15.27.8.
 * Caller installs the existing GDT32 entry and immutable startup EFER map. */
.macro guest_cold_start base, gdtr, protected, long, idtr
.code16
    cpuid
    cli
    cld
    xor %ax,%ax
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%ss
    lgdtl (0x1000 + \gdtr - \base)
    mov %cr0,%eax
    and $0x9fffffff,%eax
    or $1,%eax
    mov %eax,%cr0
    ljmpl $40,$(0x1000 + \protected - \base)
.code32
\protected:
    mov $16,%ax
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%ss
    mov $0xb000,%esp
    mov $0x620,%eax
    mov %eax,%cr4
    mov $0x10000,%eax
    mov %eax,%cr3
    mov $0xc0000080,%ecx
    mov $0x1900,%eax
    xor %edx,%edx
    wrmsr
    mov $0x80010033,%eax
    mov %eax,%cr0
    ljmp $8,$(0x1000 + \long - \base)
.code64
\long:
    lidt \idtr(%rip)
.endm
