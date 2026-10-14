/* Same map as fw/board/at32f403a_app.ld: the stock bootloader owns 0x08000000-0x08002FFF and jumps here;
 * 0x080F0000 up (stock marker sector) is left alone. */
MEMORY
{
  FLASH : ORIGIN = 0x08003000, LENGTH = 948K
  RAM   : ORIGIN = 0x20000000, LENGTH = 96K
}

/* Code starts after the model tags, which must sit at 0x080033E0 (image offset 0x3E0, read by the stock updater)
 * and 0x080037E0 (0x7E0: the bootloader compares it at every power-on and shows "Firmware Error!" instead of
 * starting the app when it differs; 0x08000654, found on the radio Oct 11, 2026). */
_stext = ORIGIN(FLASH) + 0x800;

SECTIONS
{
  /* The room around the tags (0x1C0-0x3E0 after the vector table, 0x3F0-0x7E0 between the tags) holds a few
   * fast functions instead of padding: ones whose size moves only with the pinned toolchain, so a change of ours
   * never pushes them past a tag. If one ever outgrows its gap, the ASSERTs below stop the link: drop its line
   * (it then goes back into .text). Each is as fast here as in .text (all zero-wait flash). */
  .fast_gap1 : ALIGN(4)
  {
    *(.text.*17compiler_builtins3int19specialized_div_rem11u64_div_rem);
  } > FLASH
  __efast_gap1 = .;
  .model_tag ORIGIN(FLASH) + 0x3E0 :
  {
    KEEP(*(.model_tag));
  } > FLASH
  .fast_gap2 : ALIGN(4)
  {
    *(.text.*17compiler_builtins3mem7memmove);
    *(.text.*17compiler_builtins3mem6memcpy);
  } > FLASH
  __efast_gap2 = .;
  .model_tag_boot ORIGIN(FLASH) + 0x7E0 :
  {
    KEEP(*(.model_tag_boot));
  } > FLASH
} INSERT AFTER .vector_table;

ASSERT(__efast_gap1 <= ORIGIN(FLASH) + 0x3E0, ".fast_gap1 runs into the model tag at 0x3E0: drop a line from it");
ASSERT(__efast_gap2 <= ORIGIN(FLASH) + 0x7E0, ".fast_gap2 runs into the model tag at 0x7E0: drop a line from it");

/* The slow part of flash. The AT32F403A runs code at full speed only from the first 256 KB (0x08000000-0x0803FFFF,
 * zero-wait; docs/firmware-design.md, "MCU flash"). Everything that has to be fast stays in .text and .rodata at
 * the front: the vector table, interrupt handlers, the executors and time driver, the radio, DSP, input, storage,
 * link and GPS tasks, the drivers (the display bus too), core and compiler_builtins, the fault paths, and the
 * named `static` tables they read.
 *
 * After them, in this order:
 *  - .slow_rodata: every unnamed constant (all literal strings, `const` tables and match tables: the rule since
 *    Oct 13, 2026), the UI's jump tables, font and logo data (tools/mkassets.py) and the other explicit
 *    `.slow_rodata` statics. Fast code may read from here: a wait state or two per read, nothing more (the firmware
 *    never writes the MCU flash, and no DMA reads it). Text read once per message is fine; a table read per
 *    sample, per interrupt or per pass of the Radio task must be a named `static` instead, which stays fast.
 *    tools/slowcheck.py fails on a table read from interrupt context unless its allow-list says why.
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
  /* The drawing primitives stay fast, ahead of the slow part: text, glyphs, masks, fills and the panel's pixel
   * runs run once per pixel, and from slow flash they made large redraws crawl on the radio (Oct 12, 2026). Matched
   * here first, so .slow_text's ferrite_ui pattern doesn't take them. */
  .fast_ui : ALIGN(4)
  {
    __sfast_ui = .;
    *(.text.*10ferrite_ui3gfx*);
    *(.text.*10ferrite_ui5font4*);
    *(.text.*10ferrite_ui5panel*);
    /* the off-screen canvas's band (DISP-25): drawing into it is per-pixel work too */
    *(.text.*10ferrite_ui6canvas4Band*3gfx3Gfx*);
    *(.text.*10ferrite_ui6canvasNtB*4Band*);
    . = ALIGN(4);
    __efast_ui = .;
  } > FLASH

  .slow_rodata : ALIGN(4)
  {
    __sslow_rodata = .;
    *(.slow_rodata .slow_rodata.*);
    /* Every unnamed constant: with LTO and one codegen unit, each `const`, literal string and table the compiler
     * doesn't name is `.rodata..Lanon.<hash>.<n>`, with no crate in the name to sort by, so they all come here.
     * Named `static`s keep their own `.rodata.<symbol>` sections and stay fast: a table read per sample, per
     * interrupt or per pass of the Radio task must be a `static` (tools/slowcheck.py fails on a table read from
     * interrupt context in this section). */
    *(.rodata..Lanon.*);
    /* The UI's match jump tables (the code that reads them is slow anyway). */
    *(.rodata..Lswitch.table.*11ferrite_app*);
    *(.rodata..Lswitch.table.*10ferrite_ui*);
    *(.rodata..Lswitch.table.*7ferrite5tasks2ui*);
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
     * (uafsk) stays fast: the DSP task runs it. `5uaprs?*`, not `5uaprs*`: a core generic made for uaprs ends in
     * `5uaprs` (its crate suffix) and LLVM's function merging can hand that one copy to fast callers (the Radio and
     * DSP tasks' `RangeInclusive::contains`), so those stay fast. */
    *(.text.*7ferrite5tasks4aprs*);
    *(.text.*5uaprs?*);
    /* The tone hint (TSRCH-12, -13): the Radio task's step for it and the heard-tone decode, at most once a
     * receive tick, never from an interrupt. */
    *(.text.*8tonehint*);
    /* The GPS power manager (GPS-26 to GPS-33): the GPS task's decision and its sends, a few times a second. */
    *(.text.*8gpspower*);
    *(.text.*7ferrite5tasks3gps6manage*);
    /* TERM-30: the GGA copy for the terminal's `gps` (once a second, the GPS task) and its answer (the UI task). */
    *(.text.*7ferrite5tasks3gps8keep_gga*);
    *(.text.*7ferrite5debug3gga*);
    *(.text.*4chip10heard_tone*);
    /* FM broadcast's report (at most ten times a second) and its watch mask (BCR-2, BCR-23): the Radio task's,
     * never from an interrupt. */
    *(.text.*5Radio7fm_poll*);
    *(.text.*5Radio9bcast_off*);
    /* The band scope's and home strip's step (SCOPE-4, MODE-5): once a point, at least 2 ms apart, the Radio
     * task's; the BK4829 writes it calls stay fast. */
    *(.text.*5Radio10scope_tick*);
    /* The backlight's PWM (DISP-23): set by the UI task only, once a pass. */
    *(.text.*3hal9backlight*);
    . = ALIGN(4);
    __eslow_text = .;
  } > FLASH
} INSERT AFTER .rodata;

/* The fast-flash budget: everything from the image start to the end of .fast_ui (vectors, the gaps by the tags,
 * .text, .rodata, .fast_ui). Set Oct 13, 2026 at the tx build's use (141.5 KB; the receive-only build 127.6 KB)
 * plus a small margin, of the 244 KB zero-wait room: growth past it is a decision (move something to the slow
 * part, or raise this), not an accident. tools/slowcheck.py prints the use against it. */
__fast_budget = 148K;
ASSERT(__efast_ui - ORIGIN(FLASH) <= __fast_budget, "fast flash over its budget (__fast_budget in memory.x): move code to the slow part or raise the budget");

/* The fast part (code and the other constants) must end inside the zero-wait region (0x08000000 + 256 KB). */
ASSERT(__erodata <= 0x08040000, "fast code and constants run past the 256 KB zero-wait flash");
ASSERT(__efast_ui <= 0x08040000, "the fast drawing code runs past the 256 KB zero-wait flash");
/* The UI code really went to the slow part (the name patterns above still match the symbols). */
ASSERT(__eslow_text - __sslow_text > 32K, ".slow_text is (nearly) empty: the UI code stayed in the zero-wait flash");
