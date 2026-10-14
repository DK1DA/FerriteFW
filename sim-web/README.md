# sim-web

The simulator in the browser: [`SimCore`](../sim/src/simcore.rs) (the firmware's own UI, the fake radio and the
front panel, the same code as `cargo simw`) compiled to wasm behind a plain C ABI. No wasm-bindgen: the page
instantiates the module with no imports and reads frames and strings out of `memory`.

```sh
cargo simweb   # -> target/wasm32-unknown-unknown/wasm/ferrite_sim_web.wasm (about 400 KB)
```

## Use

```js
const { instance: { exports: x } } = await WebAssembly.instantiate(bytes, {});
const str = p => new TextDecoder().decode(new Uint8Array(x.memory.buffer, p, x.text_len()));
x.init(2, 0);                               // 2x, no owner card
x.set_clock(Math.floor(Date.now() / 1000));
function tick(ms) {                         // ms since start, from requestAnimationFrame
  const p = x.frame(ms);                    // re-read memory.buffer after every call: it can grow
  const img = new ImageData(new Uint8ClampedArray(x.memory.buffer, p, x.width() * x.height() * 4), x.width(), x.height());
  ctx.putImageData(img, 0, 0);
}
```

Call `frame` about every 16 ms (it steps the app: long presses, timers and the fake radio run on its clock).
Mouse and key calls take effect at the time of the last `frame`.

## Exports

| Export | Does |
| --- | --- |
| `init(scale: u32, owner_card: u32)` | Switches the radio on (boot screen); whole scale 1–6; `owner_card` 1 boots with the sample owner card. Again: starts over. |
| `set_clock(utc_secs: u32)` | The status bar clock (UTC seconds since 1970). |
| `set_scale(scale: u32)`, `scale() -> u32` | Whole scale 1–6; read `width`/`height` again after. |
| `width() -> u32`, `height() -> u32` | The panel in pixels: 315 × 506 at 1x (630 × 1012 at 2x). |
| `screen_x() -> u32`, `screen_y() -> u32` | The 240 × 320 screen's top-left in the panel at this scale (for a mock-up overlay drawn by the page). |
| `frame(now_ms: u32) -> ptr` | Steps to `now_ms` and draws; RGBA8, `width*height*4` bytes, valid until the next call. |
| `set_flash(on: u32)` | Outline each region as it is redrawn (the desktop's F2). |
| `mouse(x: i32, y: i32, down: u32)` | The pointer at panel pixel (x, y), button down 1 / up 0. Send every change. Keys hold while down (long press = 0.7 s); the knob steps (left half −1, right half +1); VOL is the power switch. |
| `key(code: u32, down: u32)` | A key down 1 / up 0 (codes below); lights its part on the panel. |
| `knob(d: i32)` | Channel knob, −1 or +1. |
| `event(id: u32)` | A simulator event (ids below). |
| `dest_count() -> u32` | Screens `jump` offers (71: Home, then the manual's inventory). |
| `dest_name(i: u32) -> ptr` | "Home", "A1 Direct entry", "B4 Shortcut map", … |
| `jump(i: u32)` | Opens screen `i` from home (BACK returns home). Ignored while booting or off. |
| `screen_name() -> ptr` | The screen on show; "Boot" while booting, "Off" while off. |
| `mockup_name() -> ptr` | Its look-review mock-up, a file in `docs/img/screens/` (480 × 640). |
| `status_text() -> ptr` | One line: screen · last key · TX · active slot and frequency · last redraw px · locked. |
| `init_blank(scale: u32)` | Switches the radio on with a blank flash, as after installing: setup runs. |
| `flash_stage(len: u32) -> ptr` | Room for a saved flash image before `init`: copy the image there, then call `init`, which boots from it (no image staged: the sample data). |
| `flash_changed() -> u32` | 1 when the flash changed since the last call: time to save the image. |
| `flash_image() -> ptr`, `flash_len() -> u32` | The flash image to keep (settings, slots, channels, zones, range presets): 94,208 bytes, 0x02C000–0x042FFF of the chip; an image from another layout (the earlier 303,104-byte one) boots as a blank chip, into setup. Valid until the next call. |
| `flush()` | Saves settings and slots now if they changed (call on `pagehide`); otherwise they save 2 s after the last change. |
| `keymap() -> ptr` | The desktop simulator's keyboard map, as text. |
| `term_in(len: u32) -> ptr` | Room for `len` typed bytes for the terminal; write them there, then call `term_feed(len)`. |
| `term_feed(len: u32)` | Those bytes go to the terminal as typed on the cable (Enter starts it: TERM-2). Ignored while booting or off. |
| `term_read() -> ptr` | What the terminal drew since the last call, taken: VT100 bytes (ASCII), length `text_len()`. Call it after `frame` and `term_feed` and write it to the pane. |
| `term_size(cols: u32, rows: u32)` | The pane's size in characters (80 x 24 at least, up to 200 x 64). The terminal also asks with ESC [ 6 n; a pane that answers (xterm.js does) sets it the same way. |
| `dump() -> ptr` | Everything as the terminal's `:dump` writes it (manual ch 37, TERM-13): text, one command per line, ending with `end`; `text_len()` bytes. |
| `load_in(len: u32) -> ptr` | Room for a dump of `len` bytes the page read from a file: write it there, then call `load(len)`. |
| `load(len: u32) -> ptr` | Reads those bytes back as `:load` does, line by line up to `end`; the report (each refused line and why, then the count) as a string, `text_len()` bytes. |
| `text_len() -> u32` | Length in bytes (UTF-8) of the string the last string getter returned. |

## Key codes

| Code | Key | Code | Key |
| --- | --- | --- | --- |
| 0–9 | Digits | 18 | ◀ |
| 10 | \* | 19 | ▶ |
| 11 | # | 20 | SIDE1 (monitor; + knob: squelch) |
| 12 | MENU / OK | 21 | SIDE2 (scan) |
| 13 | SLOT (A·B·C) | 22 | PTT A |
| 14 | V/M | 23 | PTT B |
| 15 | BACK | 24 | PTT C |
| 16 | ▲ | 25 | Side-connector PTT (active slot) |
| 17 | ▼ | | |

The desktop map, for the page's keyboard: arrows, Enter = MENU, Esc = BACK, 0–9, `,` = \*, `.` = #, S = SLOT,
V = V/M, Q = SIDE1, W = SIDE2, Space = PTT A, `[` `]` = knob.

**Settings reset at power on:** hold MENU (key 12, or the mouse on OK) through the boot screen.

## Event ids

| Id | Event (desktop key) |
| --- | --- |
| 0 | An APRS message arrives (M) |
| 1 | A SAME weather alert, as if heard on slot C (N) |
| 2 | GPS: off → searching → fix (F5) |
| 3 | Bluetooth on/off (F6) |
| 4 | Battery: normal → low → cut-off (F7) |
| 5 | The power switch, same as clicking VOL (F8): off shows "Saving…", then dark; on boots again |
| 6 | Empty battery (F9) |
| 7 | An unknown SSB patch at boot (F10) |
| 8 | A write from the web app: the progress screen (F11) |
| 9 | Crossband repeater (X) |
| 10 | Next home layout (L) |
| 11 | A beacon, an automatic transmit (B) |
| 12 | LED demo: quiet bands and the screen off; again brings the stations back (D) |
| 13 | A station on the active slot with another tone: the tone hint; again clears it (T) |
| 14 | Broadcast FM: the active slot's VFO to 100.5 MHz (full RDS radio text); again 99.1 (station name only) (R) |
| 15 | A speaker mic plugged into the side connector, or taken out: the Status page's Speaker mic row (K) |

Events 0, 1 and 6–14 are ignored while booting or off.

## The terminal

The radio's terminal (manual chapter 37) over a pretend cable, for a terminal pane such as xterm.js:

```js
const enc = new TextEncoder();
// Call term_in first, then read memory.buffer: the call can grow memory and detach an older view.
term.onData(d => { const b = enc.encode(d); const p = x.term_in(b.length); new Uint8Array(x.memory.buffer, p, b.length).set(b); x.term_feed(b.length); });
// after each frame():
const out = str(x.term_read()); if (out) term.write(out);
x.term_size(term.cols, term.rows);
```

Press Enter in the pane to start it. Power-cycling the radio (event 5) closes it; Enter opens it again.

## Dump and load as files

The terminal's `:dump` and `:load` work in the pane as on a cable. For a page that keeps or offers the dump
itself, `dump()` returns the same text and `load_in`/`load` read one back (an artifact page cannot download a
file through a link: show the text for copying, or keep it in the page's own storage; a file input can read one
in). Region and callsign are set on the radio during setup and only noted in a dump, so load into a radio that
has been set up: before setup, settings that depend on the region (SAME) are refused.

```js
const d = str(x.dump());                                   // the dump as text
const b = new TextEncoder().encode(fileText);
const p = x.load_in(b.length); new Uint8Array(x.memory.buffer, p, b.length).set(b);
console.log(str(x.load(b.length)));                        // "54 lines read: 54 applied, 0 refused."
```

## Keeping the flash

The radio's settings, slots, channels, zones and range presets live in a simulated SPI flash through the
firmware's own store (`ferrite-store`), so they survive the power switch (VOL). To keep them across page loads,
save the image when `flash_changed()` says so and stage it before `init`:

```js
// load
const saved = await idbGet('flash');               // Uint8Array or undefined
if (saved) new Uint8Array(x.memory.buffer, x.flash_stage(saved.length), saved.length).set(saved);
x.init(scale, 0);                                  // no image staged: the sample data
// every second or so
if (x.flash_changed()) { const p = x.flash_image(); idbPut('flash', new Uint8Array(x.memory.buffer, p, x.flash_len()).slice()); }
addEventListener('pagehide', () => { x.flush(); /* then save the image as above */ });
```

`init_blank(scale)` starts from an erased flash (setup); `init` with nothing staged writes the look review's
sample data to a blank flash.
