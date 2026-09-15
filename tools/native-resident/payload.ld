OUTPUT_FORMAT(elf64-x86-64)
ENTRY(entry_uefi)
SECTIONS {
    . = 0x100000;
    image_start = .;
    .text : { *(.text.entry*) *(.text .text.*) }
    . = ALIGN(4096);
    text_end = .;
    .rodata : { *(.rodata .rodata.*) *(.srodata .srodata.*) }
    . = ALIGN(4096);
    data_start = .;
    .data : { *(.data .data.*) *(.sdata .sdata.*) *(.got .got.*) }
    /* A file-backed alignment byte makes the flat-image extent unambiguous. */
    .padding : { BYTE(0); . = ALIGN(4096); }
    image_load_end = .;
    .bss (NOLOAD) : { *(.bss .bss.*) *(COMMON) }
    . = ALIGN(4096);
    image_bss_end = .;
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
ASSERT(image_bss_end <= 0x1ff000, "resident image exceeds owned payload space")
