<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/ferrite-lockup-dark.svg">
    <img alt="Ferrite" src="assets/logo/ferrite-lockup-light.svg" width="420">
  </picture>
</p>

<p align="center"><b>Open firmware for the Radtel RT-950 Pro</b>, in Rust on Embassy.</p>

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

The chip drivers are in [`drivers/`](drivers) (BK4829, SI4732, ST7789, W25Q, keypad and NMEA; the uradio repo until Oct 13, 2026, history kept). The digital modes are a separate library, pinned by commit in `Cargo.toml`:

- [udigimodes](https://github.com/infamy/udigimodes): AFSK/APRS, POCSAG, DSC, FT8/FT4 and WSPR.

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

## Building

The toolchain is a pinned nightly (`rust-toolchain.toml`); rustup installs it on first use.

```sh
cargo test                      # logic and screens on the PC
cargo sim                       # screens to target/sim/*.png
cargo simw                      # the interactive simulator (keys are printed at start; resizing snaps to whole scales; --scale 2 for bigger)
                                # its flash is kept in target/sim/flash.bin between runs; -- --fresh starts blank (setup)
cargo simweb                    # the simulator as wasm: target/wasm32-unknown-unknown/wasm/ferrite_sim_web.wasm
cargo fw                        # the image: target/thumbv7em-none-eabihf/release/ferrite
cargo fw-clippy -- -D warnings
```

**The terminal** (manual chapter 37) runs in the simulator too: `cargo simw` opens a pseudo-terminal as the
radio's cable and prints how to connect, for example `screen /dev/ttys006 115200`; press Enter in it. Where there
is no pseudo-terminal, `cargo simw -- --term-tcp 7300`, then `stty raw -echo; nc 127.0.0.1 7300; stty sane`.
`cargo run -p ferrite-sim -- --term-headless 60` runs the radio and its cable with no window (for scripts).

To pack the image for the stock bootloader (build, slow-flash check, pack, dry run):

```sh
tools/mkbtf.sh            # target/ferrite.btf
tools/mkbtf.sh bringup    # target/ferrite-bringup.btf: the first builds on a radio
tools/mkbtf.sh tx-dry     # target/ferrite-txdry.btf: transmit with the PA drive held at 0 (the dry run)
python3 tools/flash.py PORT target/ferrite-bringup.btf --backup flash.bin
```

The bring-up build (`cargo fw-bringup`, feature `bringup`) never writes or erases the SPI flash and doesn't mount
the store; a key pressed during the boot screen opens the key test; the cable prints a plain log at 115200 (boot
report, raw key codes, the knob, the receiver, a fault's address) until a byte arrives. `flash.py` refuses a
Ferrite image without a whole-chip backup taken with the hwtest image, and refuses a backup holding a pending
update. The day-one order is `../docs/bringup.md`; debugging on the radio (cable log, the terminal's `:mem`, `:gpio`,
`:bk`, `:si`, fault addresses, `tools/bkcheck.py --diff`) is `../docs/debugging.md`.

Transmit is built in milestone 4, after receive is proven on the radio. Until then it sits behind a feature: `cargo fw --features tx`
adds it, and without the feature the build contains no transmit code at all, so bring-up builds cannot key the radio.

udigimodes is private for now. Cargo fetches it with git, so your git credentials must reach it.
CI uses a `DEPS_TOKEN` secret for this.

## Safety

- **Safe state first.** The first code after reset clears every RF path pin and holds the PA drive at 0 V,
  before RAM is even set up. Faults, stray interrupts and panics return to it (`board::fault`).
- **Factory calibration and the bootloader's update area are never written.** The SPI flash driver is given only
  Ferrite's own ranges and refuses everything else (`ferrite_core::layout`).
- **No MCU flash writes.**

## Licence

[PolyForm Noncommercial 1.0.0](LICENSE.md): free for any noncommercial use; selling Ferrite, shipping it on radios for sale or selling a service built on it needs a commercial licence. Copyright 2026 Alex Harvey. The chip drivers in `drivers/` are MIT (`drivers/LICENSE`).
