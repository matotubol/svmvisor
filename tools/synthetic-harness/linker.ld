OUTPUT_FORMAT(elf64-x86-64)
ENTRY(entry32)
SECTIONS {
    . = 0x100000;
    image_start = .;
    .multiboot : { KEEP(*(.multiboot)) }
    .text : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    image_load_end = .;
    .bss ALIGN(4096) (NOLOAD) : { *(.bss .bss.*) *(COMMON) }
    image_bss_end = .;
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
    ASSERT(image_bss_end < 0x200000, "smoke image exceeds reserved 1 MiB arena")
}
