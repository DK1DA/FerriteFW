<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/ferrite-lockup-dark.svg">
    <img alt="Ferrite" src="assets/logo/ferrite-lockup-light.svg" width="420">
  </picture>
</p>

<p align="center"><b>Open firmware for the Radtel RT-950 Pro</b>, in Rust on Embassy.</p>

Ferrite replaces the RT-950 Pro's stock firmware with one that is open, receive-first and RF-aware. Planned:

- Daily use as good as stock: three watch slots, scanning, APRS, GPS, the broadcast receiver.
- Decoders on the radio itself: POCSAG, SAME, DSC, AIS, FT8 and WSPR.
- A documented link protocol for its own web app.
- Real battery life.
- Transmit fenced in by the firmware itself.

> **Status: scaffold.** The start-up, the six tasks and the chip drivers are wired together, and the image builds,
> but it has not run on a radio yet. Milestone 1 (bring-up, receive only) starts here.

## Layout

| Path | What |
| --- | --- |
| [`firmware/`](firmware) | The image: start-up and safe state, the HAL (`hal/`), the board's pins, and the six tasks (`tasks/`) |
| [`crates/ferrite-core`](crates/ferrite-core) | Plain logic, tested on the PC: task messages, frequencies, keys, the SPI flash map and its protected ranges, the decoder slot |
| [`crates/ferrite-ui`](crates/ferrite-ui) | Screens and the 4-bit drawing layer; draws to the panel or, on the PC, to a frame buffer |
| [`sim/`](sim) | Renders the screens on the PC with the firmware's own drawing code |
| [`tools/`](tools) | Font and logo generators, `btf.py` (packing for the stock bootloader), `flash.py` |
| [`assets/logo/`](assets/logo) | The logo: the toroid mark, lockups for dark and light backgrounds, the app icon (`ferrite-icon.svg`, a simplified one for 16-48 px, PNGs 16-1024 and `favicon.ico`), the social preview |
| [`fonts/`](fonts) | Atkinson Hyperlegible and Barlow Semi Condensed (OFL; licences beside them) |

The chip drivers and the digital modes are separate libraries, pinned by commit in `Cargo.toml`:

- [uradio](https://github.com/infamy/uradio): BK4829, SI4732, ST7789, W25Q, keypad and NMEA drivers.
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
| Link | thread | Programming cable (and later Bluetooth) |
| GPS | thread | USART3, position and time |

## Building

The toolchain is a pinned nightly (`rust-toolchain.toml`); rustup installs it on first use.

```sh
cargo test                      # logic and screens on the PC
cargo sim                       # screens to target/sim/*.png
cargo fw                        # the image: target/thumbv7em-none-eabihf/release/ferrite
cargo fw-clippy -- -D warnings
```

To pack the image for the stock bootloader:

```sh
BIN=$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin
$BIN/llvm-objcopy -O binary target/thumbv7em-none-eabihf/release/ferrite target/ferrite.bin
python3 tools/btf.py pack target/ferrite.bin target/ferrite.btf
```

`cargo fw --features tx` adds transmit. Without that feature, the build contains no transmit code at all. It
stays off until milestone 4.

uradio and udigimodes are private for now. Cargo fetches them with git, so your git credentials must reach them.
CI uses a `DEPS_TOKEN` secret for this.

## Safety

- **Safe state first.** The first code after reset clears every RF path pin and holds the PA drive at 0 V,
  before RAM is even set up. Faults, stray interrupts and panics return to it (`board::fault`).
- **Factory calibration and the bootloader's update area are never written.** The SPI flash driver is given only
  Ferrite's own ranges and refuses everything else (`ferrite_core::layout`).
- **No MCU flash writes.**

## Licence

To be decided before the first public release.
