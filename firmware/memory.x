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

/* The slow part of flash. The AT32F403A runs code at full speed only from the first 256 KB (0x08000000-0x0803FFFF,
 * zero-wait; docs/firmware-design.md, "MCU flash"). Everything that has to be fast stays in .text and .rodata at
 * the front: the vector table, interrupt handlers, the executors and time driver, the radio, DSP, input, storage,
 * link and GPS tasks, the drivers (the display bus too), core and compiler_builtins, the fault paths.
 *
 * After them, in this order:
 *  - .slow_rodata: font and logo data (tools/mkassets.py), `slow!()` text and the info tables.
 *  - .slow_text: the UI's code, run only by the thread-mode UI task (drawing is bound by the bit-banged display
 *    bus): every function of ferrite-app and ferrite-ui, including generic instances made for them (their v0
 *    mangled names carry the crate as `11ferrite_app` / `10ferrite_ui`), and the UI task itself. Nothing on an
 *    interrupt path calls into it: tools/slowcheck.py checks that on the ELF.
 * Sections named by an INSERTed SECTIONS command are matched before the main script's `*(.text .text.*)`, so these
 * patterns win over cortex-m-rt's .text; the ASSERTs below catch it if that ever changes.
 *
 * The image stays one contiguous binary for the bootloader: no gap, no padding. */
SECTIONS
{
  .slow_rodata : ALIGN(4)
  {
    __sslow_rodata = .;
    *(.slow_rodata .slow_rodata.*);
    . = ALIGN(4);
    __eslow_rodata = .;
  } > FLASH

  .slow_text : ALIGN(4)
  {
    __sslow_text = .;
    *(.text.*11ferrite_app*);
    *(.text.*10ferrite_ui*);
    *(.text.*13ferrite_store*);
    *(.text.*7ferrite5tasks2ui*);
    /* The APRS task (run by the UI task) and the APRS library it parses and builds frames with; the modem
     * (uafsk) stays fast: the DSP task runs it. */
    *(.text.*7ferrite5tasks4aprs*);
    *(.text.*5uaprs*);
    /* The tone hint (TSRCH-12, -13): the Radio task's step for it and the heard-tone decode, at most once a
     * receive tick, never from an interrupt. */
    *(.text.*8tonehint*);
    *(.text.*4chip10heard_tone*);
    . = ALIGN(4);
    __eslow_text = .;
  } > FLASH
} INSERT AFTER .rodata;

/* The fast part (code and the other constants) must end inside the zero-wait region (0x08000000 + 256 KB). */
ASSERT(__erodata <= 0x08040000, "fast code and constants run past the 256 KB zero-wait flash");
/* The UI code really went to the slow part (the name patterns above still match the symbols). */
ASSERT(__eslow_text - __sslow_text > 32K, ".slow_text is (nearly) empty: the UI code stayed in the zero-wait flash");
