<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/ferrite-lockup-dark.svg">
    <img alt="Ferrite" src="assets/logo/ferrite-lockup-light.svg" width="420">
  </picture>
</p>

<p align="center"><b>Free firmware for the Radtel RT-950 Pro</b>, source public, in Rust on Embassy.</p>
<p align="center"><a href="https://www.radtels.com/products/radtel-rt-950-pro-27mhz-tx-rx-10w-walkie-talkie-am-fm-usb-lsb-cw-receive-gps-aprs-wireless-bluetooth-phone-programming-noaa-two-way-radio?ref=FERRITEFW&variant=44669996794064">Get the RT-950 Pro from Radtel</a> (affiliate link: buying through it supports Ferrite, at no extra cost to you)</p>

<p align="center">Help and questions: <a href="https://discord.gg/fYygBVD2Kx">the Ferrite Discord</a>.</p>

Ferrite replaces the RT-950 Pro's stock firmware with one that is open, RF-aware and transmits and receives on every band the radio is built for. Planned:

- Daily use as good as stock: FM voice transmit on every band and power level, repeaters and tones, three watch slots, scanning, APRS, GPS, the broadcast receiver.
- Decoders on the radio itself: POCSAG, SAME, DSC, AIS, FT8 and WSPR.
- A documented link protocol for its own web app.
- Real battery life.
- Transmit with its safety checks in the firmware itself: calibrated band limits and power, timeout, battery cut-off.

> **Status: scaffold.** The start-up, the six tasks and the chip drivers are wired together, and the image builds,
> but it has not run on a radio yet. Milestone 1 (bring-up, receive only) starts here.

## Layout

| Path | What |
| --- | --- |
| [`firmware/`](firmware) | The image: start-up and safe state, the HAL (`hal/`), the board's pins, and the six tasks (`tasks/`) |
| [`crates/ferrite-core`](crates/ferrite-core) | Plain logic, tested on the PC: task messages, frequencies, keys, the SPI flash map and its protected ranges, the decoder slot |
| [`crates/ferrite-ui`](crates/ferrite-ui) | The 4-bit drawing layer and the widgets the screens are made of; draws to the panel or, on the PC, to a frame buffer |
| [`crates/ferrite-app`](crates/ferrite-app) | The UI state machine, run by the firmware's UI task and the simulator: keys, knob, time and the radio's snapshot in; radio commands and redraws (only what changed) out. One module per screen in `src/screens/` (how to add one: `screens/mod.rs`; how each is reached: `screens/nav.rs`; the work list: `docs/screens.md`). Built: home, edit in place, quick settings, advanced settings, Status page; every other screen opens as a placeholder |
| [`crates/ferrite-store`](crates/ferrite-store) | The SPI-flash store: a fixed memory map (settings and slots in A/B copies, channel, zone and preset records at fixed addresses, one scratch sector for rewrites), safe against a power cut at any byte (tested on a simulated flash with thousands of cuts); the UI's channel store on the radio and in both simulators |
| [`sim/`](sim) | The screens on the PC with the firmware's own code: PNGs (`cargo sim`), or an interactive window with the radio's clickable front panel and a fake three-slot radio (`cargo simw`) |
| [`sim-web/`](sim-web) | The same interactive simulator as wasm for a web page (`cargo simweb`; API in its README) |
| [`tools/`](tools) | Font and logo generators, `btf.py` (packing for the stock bootloader), `flash.py`, `slowcheck.py` (fast/slow flash split check) |
| [`assets/logo/`](assets/logo) | The logo: the toroid mark, lockups for dark and light backgrounds, the app icon (`ferrite-icon.svg`, a simplified one for 16-48 px, PNGs 16-1024 and `favicon.ico`), the social preview |
| [`fonts/`](fonts) | Atkinson Hyperlegible and Barlow Semi Condensed (OFL; licences beside them) |

The chip drivers are in [`drivers/`](drivers) (BK4829, SI4732, ST7789, W25Q, keypad and NMEA; the uradio repo until Oct 13, 2026, history kept). The digital modes are in [`digimodes/`](digimodes): AFSK and FX.25, APRS, POCSAG, DSC and the DSP they share (from the udigimodes library until Oct 23, 2026).

## Tasks

Each piece of hardware has exactly one owning task. Other tasks send the owner commands on its channel. Owners
announce what happened on one event bus (`firmware/src/chan.rs`, types in `ferrite_core::msg`).

| Task | Executor | Owns |
| --- | --- | --- |
| Radio | interrupt, highest | Both BK4829s and their bus, the SI4732, RF path pins, PA drive, speaker enable |
| DSP | interrupt, below Radio | The decoder slot, audio in (ADC1) and out (DAC1); the input sampler runs here too |
| UI | thread | Display, backlight, what the keys and knob mean |
| Storage | thread | SPI flash (writes only inside `layout::WRITABLE`) |
| Link | thread | Programming cable: the terminal (chapter 37) for now; the link protocol and KISS later |
| GPS | thread | USART3, position and time |

## Getting started (developers)

You need git, [rustup](https://rustup.rs) and Python 3. The compiler is a pinned nightly (`rust-toolchain.toml`): rustup installs it, with its components and the radio's target, the first time you build. Everything else comes with the repo, the digital modes included (`digimodes/`).

```sh
git clone <this repo> ferrite && cd ferrite
cargo test                      # the logic, the screens and the decoders on the PC (the first run installs the toolchain)
cargo simw                      # the radio in a window: keys are printed at start; -- --fresh starts at setup
cargo sim                       # every screen to target/sim/*.png
```

Build an image for the radio:

```sh
python3 -m venv .venv && .venv/bin/pip install -r tools/requirements.txt
tools/mkbtf.sh tx               # target/ferrite-tx.btf: the user build (transmit, no bench commands)
tools/mkbtf.sh tx-bench         # target/ferrite-txbench.btf: plus the bench commands, Dry TX on at power-on
tools/mkbtf.sh reader           # target/ferrite-reader.btf: the read-only backup reader Ferrite Console loads
```

`mkbtf.sh` builds, checks what must sit in fast flash, packs the image for the stock bootloader and dry-runs it.

Put it on a radio:

- **Ferrite Console** (Chrome or Edge): the Install page backs the radio up first, then installs; Advanced › Ferrite image takes your own `.btf`. On a radio already on Ferrite, Firmware Update does the same.
- **Or the cable tool:** `.venv/bin/python tools/flash.py PORT target/ferrite-tx.btf --backup backup.bin`. It refuses without a whole-chip backup file (take one with Console).

Before a commit:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fw-clippy -- -D warnings          # the radio's build
cargo fw-reader-clippy -- -D warnings   # the reader
cargo test -p uradio-bus -p bk4829 -p si4732 -p st7789 -p w25q -p keypad -p nmea --all-features
```

**The terminal** (manual chapter 37) runs in the simulator too: `cargo simw` opens a pseudo-terminal as the radio's cable and prints how to connect, for example `screen /dev/ttys006 115200`; press Enter in it. Without a pseudo-terminal: `cargo simw -- --term-tcp 7300`, then `stty raw -echo; nc 127.0.0.1 7300; stty sane`. `cargo run -p ferrite-sim -- --term-headless 60` runs the radio and its cable with no window, for scripts.

More: the spec (the manual) and the decisions are in the docs repo; debugging on the radio (the cable log, the terminal's bench commands, fault addresses) is its `debugging.md`.

## Safety

- **Safe state first.** The first code after reset clears every RF path pin and holds the PA drive at 0 V,
  before RAM is even set up. Faults, stray interrupts and panics return to it (`board::fault`).
- **Factory calibration and the bootloader's update area are never written.** The SPI flash driver is given only
  Ferrite's own ranges and refuses everything else (`ferrite_core::layout`).
- **No MCU flash writes.**

## Licence

[PolyForm Noncommercial 1.0.0](LICENSE.md): free for any noncommercial use; selling Ferrite, shipping it on radios for sale or selling a service built on it needs a commercial licence. Copyright 2026 Alex Harvey; the chip drivers in `drivers/` too.
