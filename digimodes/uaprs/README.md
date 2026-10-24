# uaprs

**APRS for microcontrollers: AX.25 frames, APRS packets in and out, KISS, smart beaconing. No RAM of its own.**

uaprs reads and writes what goes inside the frames that [`uafsk`](../uafsk) (or any TNC) carries. Parsing is zero
copy: packets borrow from the frame, so the parser needs no buffer and no static RAM. It is `no_std`, allocates
nothing, and uses integers only.

Part of [udigimodes](../../README.md), digital-mode libraries for microcontrollers. Written for
[Ferrite](../../README.md), free firmware for the Radtel RT-950 Pro handheld. It covers what
that firmware needs: a heard list with symbols, positions, distance and bearing; messages with acks; beacons;
KISS over cable and Bluetooth.

Status: **prototype, 0.1**. Checked against Ham::APRS::FAP on 934 packets and reviewed for robustness and size;
not yet run on a radio.

## At a glance

| | |
| --- | --- |
| Static RAM | **none** (`kiss::Decoder` holds a 330-byte frame buffer, sized by its type) |
| Flash (Cortex-M4F, `opt-level = "z"`) | **18.1 KB** for everything; **15.5 KB** without the `nmea` and `ultimeter` features |
| Stack | about 0.9 KB for `parse`, 1.1 KB through `Frame::parse` (packet and frame are about 100 bytes each) |
| Robustness | 60,000 mutated and random packets in the tests, with overflow checks, no panics |
| Packets that agree with Ham::APRS::FAP, field by field | **926 of 934** |

## What it reads

| Packet | Details |
| --- | --- |
| Positions | Plain (with ambiguity), compressed (course and speed, range, altitude), Mic-E (message codes, altitude, radio type), raw NMEA (`$GPRMC`, `$GPGGA`, `$GPGLL`, symbol from the destination) |
| Extensions | Course/speed, PHG and PHGR, range, `/A=` altitude, `!DAO!` extra precision, timestamps (3 forms) |
| Messages | Messages with ids, acks, rejections, APRS 1.1 reply-acks, bulletins |
| Objects and items | Live and killed, plain or compressed position |
| Weather | With a position or without, and Peet Bros Ultimeter `$ULTW` |
| Other | Status (with timestamp), capabilities, telemetry (raw), third-party traffic (unwrapped by the caller) |

Units are APRS's own: millionths of a degree, knots, feet, mph, °F, hundredths of an inch, tenths of a hPa.

## Checked against Ham::APRS::FAP

[Ham::APRS::FAP](https://metacpan.org/pod/Ham::APRS::FAP) 1.21 is the Perl parser behind aprs.fi. Every packet
below is parsed by both, and every field FAP reports is compared: type, position (to 2 millionths of a degree),
symbol, ambiguity, course, speed, altitude, comment, message fields, object names, weather values.

| Set | Packets | Agree |
| --- | --- | --- |
| WA8LMF TNC Test CD: every distinct packet Dire Wolf decodes from tracks 1 and 2 (real 2005 traffic: 331 Mic-E, 224 plain, 142 NMEA, weather, objects, status, messages) | 856 | **848** |
| Ham::APRS::FAP's own test suite (literal packets) | 14 | **14** |
| Written from the APRS spec for what the recordings lack: compressed, messages and reply-acks, items, objects, timestamps, DAO, bad packets | 64 | **64** |

The 8 differences are one weather station's comment. It sends `....` with no field letter; uaprs reads that as a
missing field, while FAP keeps it in the comment. Where uaprs deliberately differs from FAP, the test says so:

- uaprs ignores a trailing CR or LF, which radios add. FAP rejects some messages that end in CR, so the vectors
  are compared without them, as APRS-IS delivers them.
- A station with the weather symbol that sends no weather keeps its comment and altitude in uaprs. FAP drops them.
- FAP 1.21 predates reply-acks and returns `MM}AA` as the message id; uaprs splits it into the id and the
  reply-ack.

## Using it

```rust
use uaprs::ax25::Frame;
use uaprs::parse::{parse, Packet};

// A frame from uafsk (FCS already checked and removed).
if let Some(f) = Frame::parse(frame) {
    match parse(f.dest.call(), f.info) {
        Ok(Packet::Position { pos, comment, .. }) => { /* pos.lat, pos.lon, pos.table, pos.code ... */ }
        Ok(Packet::Message(m)) if m.addressee == b"N0CALL-9" => { /* reply with build::ack */ }
        _ => {}
    }
}

// A beacon: the information field, then the frame.
let mut info = [0u8; 100];
let n = uaprs::build::position(&report, &mut info).unwrap();
let frame = Frame::new(me, dest, &path, &info[..n]).unwrap();
let mut out = [0u8; 330];
let len = frame.write(&mut out).unwrap();     // to uafsk::Modulator
```

For a log line or the link, `Frame::emit_tnc2` writes the TNC2 text a byte at a time. It avoids `core::fmt`,
which costs about 2 KB of flash; `Display` gives the same text on a host.

Features (both on by default): `nmea` reads raw GPS sentences (about 1.6 KB), `ultimeter` reads `$ULTW` weather
(about 0.6 KB).

**A KISS TNC** is uafsk plus [`kiss`](src/kiss.rs): `kiss::Decoder` turns the host's serial bytes into frames,
`Params::apply` handles its settings (TXDELAY, persistence, slot time, TX tail, full duplex) and `Params::flags`
turns them into uafsk's preamble and postamble; `kiss::Access` decides when a waiting frame may go
(p-persistence, with uafsk's `busy()` and the squelch as the channel state); received frames go back through
`kiss::encode`. The firmware supplies the serial or Bluetooth link and keys the transmitter.

`geo::distance_bearing` gives metres and degrees between two positions, accurate to 0.5% over a radio's range.
`beacon::Beaconer` implements smart beaconing (HamHUD's algorithm). `kiss` encodes and decodes the serial
framing for PC and phone apps.

## Testing

```sh
cargo test -p uaprs                                                   # AX.25, KISS, build-parse round trips, geo
tools/bench.sh                                                        # makes the vectors (Docker) and runs:
cargo test --release -p uaprs --test report -- --ignored --nocapture  # the FAP comparison (UAPRS_SHOW=1 lists differences)
```

## References

- APRS Protocol Reference 1.0.1 (TAPR, 2000), and the APRS 1.1 and 1.2 addenda (reply-acks, PHGR, DAO).
- AX.25 Link Access Protocol 2.2 (TAPR, 1998).
- "The KISS TNC: a simple host-to-TNC communications protocol" (Chepponis and Karn, 1987).
- Ham::APRS::FAP 1.21 as the reference parser; Dire Wolf for the recordings' frames.

## Licence

[PolyForm Noncommercial 1.0.0](../../LICENSE.md).
