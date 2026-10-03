/* Same map as fw/board/at32f403a_app.ld: the stock bootloader owns 0x08000000-0x08002FFF and jumps here;
 * 0x080F0000 up (stock marker sector) is left alone. */
MEMORY
{
  FLASH : ORIGIN = 0x08003000, LENGTH = 948K
  RAM   : ORIGIN = 0x20000000, LENGTH = 96K
}

/* Code starts after the model tag, which must sit at 0x080033E0 (image offset 0x3E0). */
_stext = ORIGIN(FLASH) + 0x400;

SECTIONS
{
  .model_tag ORIGIN(FLASH) + 0x3E0 :
  {
    KEEP(*(.model_tag));
  } > FLASH
} INSERT AFTER .vector_table;
