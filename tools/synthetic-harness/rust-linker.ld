OUTPUT_FORMAT(elf64-x86-64)
ENTRY(entry32)
SECTIONS {
    . = 0x100000;
    image_start = .;
    .multiboot : {
        KEEP(*(.multiboot))
        . = ALIGN(4096);
    }
    text_start = .;
    .text : {
        KEEP(*(.text.loader_fixture))
        . = ALIGN(4096);
        *(.text .text.*)
        . = ALIGN(4096);
    }
    text_end = .;
    rodata_start = .;
    .rodata : {
        *(.rodata .rodata.*) *(.srodata .srodata.*)
        . = ALIGN(4096);
    }
    rodata_end = .;
    data_start = .;
    .data : {
        *(.data .data.*) *(.sdata .sdata.*)
        *(.got .got.*)
        . = ALIGN(4096);
    }
    data_end = .;
    image_load_end = .;
    bss_start = .;
    .bss (NOLOAD) : {
        *(.bss .bss.*) *(.sbss .sbss.*) *(COMMON)
        . = ALIGN(4096);
    }
    bss_end = .;
    image_bss_end = .;
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
    ASSERT(text_start == image_start + 4096, "Multiboot header must occupy one page")
    ASSERT(image_bss_end - image_start <= 0xff000, "Rust image overlaps reserved UEFI handoff page")
}
