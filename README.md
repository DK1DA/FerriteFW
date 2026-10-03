# uradio

**Drivers for the chips in a handheld radio**: written for [Ferrite](https://github.com/infamy/ferrite), an open
firmware for the Radtel RT-950 Pro, and usable on anything with the same parts.

| Crate | Chip | Status |
| --- | --- | --- |
| [`uradio-bus`](crates/bus) | The minimal bus traits the drivers use, and recording mocks for host tests | 0.1 |
| [`w25q`](crates/w25q) | W25Q SPI NOR flash: read, program, non-blocking erase, write protection by address range | 0.1 |
| `st7789` | ST7789 display over an 8-bit parallel bus: windows, fills, hardware scroll | planned |
| `keypad` | Key matrix: scan, debounce, long press | planned |
| `nmea` | GPS sentences: position, time, fix | planned |
| `si4732` | SI4732 broadcast receiver: FM, AM, SW, SSB (patch from external flash), RDS | planned |
| `bk4829` | BK4829 transceiver: tuning, modes, squelch, tones, DTMF, FSK modem; transmit behind the `tx` feature | planned |

Every crate follows the same rules:

- `no_std`, no allocation.
- **Our own minimal bus traits** ([`uradio-bus`](crates/bus)), not embedded-hal: only what each chip needs.
  embedded-hal's transaction-list traits measured 210 bytes more flash for the same operations.
- **Nothing blocks for long.** A slow operation, such as a flash erase or a tuning settle, is a start call and a
  poll, so an async firmware waits on a timer.
- **Tested on the PC** against recorded or simulated bus traffic: every byte a driver sends is checked.

## Building and testing

```sh
cargo test --workspace --all-features                                   # host tests with the mock buses
cargo build --release --target thumbv7em-none-eabihf --workspace        # the radio's Cortex-M4F
```

Commits are AI-assisted.

## Licence

MIT ([LICENSE](LICENSE)).
