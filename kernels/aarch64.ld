/* Ferric-K aarch64 kernel linker script. */

OUTPUT_FORMAT(elf64-littleaarch64)
ENTRY(_start)

PHDRS
{
    /* PF_X = 1 << 0, PF_W = 1 << 1, PF_R = 1 << 2 */
    text   PT_LOAD FLAGS((1 << 2) | (1 << 0)); /* R + X */
    rodata PT_LOAD FLAGS((1 << 2));            /* R     */
    data   PT_LOAD FLAGS((1 << 2) | (1 << 1)); /* R + W */
}

SECTIONS
{
    . = 0xffffffff80000000;

    .text : ALIGN(0x1000)
    {
        *(.text .text.*)
    } :text

    .rodata : ALIGN(0x1000)
    {
        *(.rodata .rodata.*)
    } :rodata

    .limine_requests : ALIGN(0x1000)
    {
        KEEP(*(.limine_requests.start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests.end))
    } :rodata

    .data : ALIGN(0x1000)
    {
        *(.data .data.*)
        *(.data.rel.ro .data.rel.ro.*)
        *(.got .got.*)
    } :data

    /*
     * BSS is not zeroed by us: it is the p_memsz > p_filesz tail of the data
     * segment, which the loader zeroes per ELF semantics before handoff.
     */
    .bss : ALIGN(0x1000)
    {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ :
    {
        *(.eh_frame .eh_frame.*)
        *(.note .note.*)
        *(.comment)
    }
}
