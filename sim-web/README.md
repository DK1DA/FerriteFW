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
| `keymap() -> ptr` | The desktop simulator's keyboard map, as text. |
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
| 8 | A write from a new Bluetooth device (F11) |
| 9 | Crossband repeater (X) |
| 10 | Next home layout (L) |
| 11 | A beacon, an automatic transmit (B) |

Events 0, 1 and 6–11 are ignored while booting or off.
