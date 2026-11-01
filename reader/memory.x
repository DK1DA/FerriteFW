/* The reader image (decision 264): the firmware's map (firmware/memory.x), so it loads through the stock
 * bootloader as Ferrite does. The bootloader owns 0x08000000-0x08002FFF and jumps here; it stays below
 * 0x080F0000 (stock's marker sector), which it reads for the backup (`mdump`, BACKUP-12). */
MEMORY
{
  FLASH : ORIGIN = 0x08003000, LENGTH = 948K
  RAM   : ORIGIN = 0x20000000, LENGTH = 96K
}

/* Code starts after the model tags at image offsets 0x3E0 (the stock updater reads it) and 0x7E0 (the bootloader
 * compares it at every power-on: "Firmware Error!" when it differs). */
_stext = ORIGIN(FLASH) + 0x800;

SECTIONS
{
  .model_tag ORIGIN(FLASH) + 0x3E0 :
  {
    KEEP(*(.model_tag));
  } > FLASH
  .model_tag_boot ORIGIN(FLASH) + 0x7E0 :
  {
    KEEP(*(.model_tag_boot));
  } > FLASH
} INSERT AFTER .vector_table;

/* The fonts (ferrite-ui's `.slow_rodata` statics) right after the other constants: the image stays one contiguous
 * binary for the bootloader. */
SECTIONS
{
  .slow_rodata : ALIGN(4)
  {
    *(.slow_rodata .slow_rodata.*);
    . = ALIGN(4);
    __eslow_rodata = .;
  } > FLASH
} INSERT AFTER .rodata;

/* Small enough to run whole from the zero-wait flash (0x08000000 + 256 KB): no fast/slow split needed. */
ASSERT(__eslow_rodata <= 0x08040000, "the reader runs past the 256 KB zero-wait flash");
