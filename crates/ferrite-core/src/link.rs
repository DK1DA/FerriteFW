//! The link protocol's wire format (manual chapter 29, LINK-1 – LINK-11; Appendix "Link protocol v1"): shared by
//! the radio, the simulators and the PC tools. Plain logic: framing, checksums, message and status numbers.
//!
//! On the wire a frame is its body COBS-encoded, then a 0x00 delimiter (0x00 never appears inside). The body is
//! `type, seq, payload..., crc16` with the CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) of type, seq and
//! payload, little-endian. The PC sends requests (types 0x01-0x7F); the radio answers each one with type | 0x80,
//! the same seq, and a payload that starts with a [`Status`] byte: every frame is acknowledged (LINK-1).

/// The protocol version in `HELLO` and its answer. 2: no approval tokens; `AUTH` with the radio's PIN (decision 104).
pub const PROTOCOL: u8 = 2;
/// The most payload in a frame (BLE chunks stay small: LINK-16).
pub const MAX_PAYLOAD: usize = 240;
/// The most body bytes: type, seq, payload, CRC.
pub const MAX_BODY: usize = MAX_PAYLOAD + 4;
/// The most bytes a frame takes on the wire: COBS adds one byte per 254, plus the delimiter.
pub const MAX_WIRE: usize = MAX_BODY + MAX_BODY / 254 + 2;
/// The build, as `HELLO` and `INFO` give it (LINK-35): what a bench tool checks before it keys (Dry TX on holds the
/// PA drive at 0) and before it uses a bench command such as `txtest` ([`build::BENCH`], decision 151).
pub mod build {
    /// Receive only: no transmit code (no `tx` feature).
    pub const RECEIVE: u8 = 0;
    /// Transmits (`--features tx`).
    pub const TX: u8 = 1;
    /// The retired tx-dry build (the PA drive held at 0 at build time; decision 193): no image sends it now, kept
    /// so the tools still name a radio that runs an older one.
    pub const TX_DRY: u8 = 2;
    /// A transmit build with Dry TX on (TX-48): the PA drive held at 0 until it is turned off on the radio, or (the
    /// `tx` build) the radio is powered off; the tx-bench build starts with it on (TX-48.1).
    pub const TX_DRY_ON: u8 = 3;

    /// Bit 7: the bench commands are in (the `bench` cargo feature: the bring-up and tx-bench builds; TERM-38). The
    /// low bits are the transmit kind above.
    pub const BENCH: u8 = 0x80;

    /// The byte for a build with or without transmit, Dry TX on or not (TX-48: counts only with transmit), with
    /// or without the bench commands.
    pub const fn of(tx: bool, dry_on: bool, bench: bool) -> u8 {
        let kind = match (tx, dry_on) {
            (true, true) => TX_DRY_ON,
            (true, false) => TX,
            (false, _) => RECEIVE,
        };
        kind | if bench { BENCH } else { 0 }
    }

    /// Whether the PA drive is held at 0 now: Dry TX on (TX-48), or an older radio's tx-dry build. What a bench
    /// tool needs before it keys.
    pub const fn dry(b: u8) -> bool {
        matches!(b & !BENCH, TX_DRY | TX_DRY_ON)
    }

    /// Whether a build byte has the bench commands.
    pub const fn bench(b: u8) -> bool {
        b & BENCH != 0
    }

    /// Its transmit kind's name, as the tools print it (the bench bit aside).
    pub const fn name(b: u8) -> &'static str {
        match b & !BENCH {
            RECEIVE => "receive",
            TX => "tx",
            TX_DRY => "tx-dry",
            TX_DRY_ON => "tx+dry",
            _ => "unknown",
        }
    }
}

/// Answers carry the request's type with this bit set.
pub const ANSWER: u8 = 0x80;

/// Requests, PC to radio. The numbers are part of the published protocol: never reuse one.
pub mod msg {
    /// Start a session: protocol version, then the client's name. The answer's third byte says whether the session
    /// may go on (1) or must send `AUTH` first (0: the radio's PIN is set and this is Bluetooth, LINK-30).
    pub const HELLO: u8 = 0x01;
    /// Firmware version and what the radio holds.
    pub const INFO: u8 = 0x02;
    // 0x03 was TOKEN (approval tokens, dropped by decision 104): not reused.
    /// The radio's PIN as ASCII digits (4-6), on Bluetooth while one is set (LINK-30, LINK-31): Ok, `Refused`
    /// (wrong; it counts toward the radio's lockout, PIN-3), or `Busy` (locked out: wait).
    pub const AUTH: u8 = 0x04;
    /// Read part of a record: what, index, offset, length.
    pub const READ: u8 = 0x10;
    /// Write part of a record: what, index, offset, total length, bytes; applied when the last part arrives.
    pub const WRITE: u8 = 0x11;
    /// Delete a channel, zone or preset.
    pub const DELETE: u8 = 0x12;
    /// A write session of N records starts: the radio shows its progress (LINK-9).
    pub const BEGIN: u8 = 0x13;
    /// The write session ends.
    pub const END: u8 = 0x14;
    /// Which indexes hold a record (LINK-39): what; the answer is the index count (u16), then a bitmap, index i at
    /// bit `i % 8` of byte `i / 8`.
    pub const USED: u8 = 0x15;
    /// Whole records from an index on (LINK-40): what, first index (u16), count (u16). The answer is the next index
    /// to ask from (u16), then each record held, as many as fit: index (u16), length (u8), the record; empty
    /// indexes are skipped.
    pub const READ_MANY: u8 = 0x17;
    /// Several whole records in one frame (LINK-41): what, then each record as `READ_MANY` gives them (index,
    /// length, record). Applied in order, each as a `WRITE` of it would be; the answer's first byte after the
    /// status is how many were applied (a refusal is the next one's).
    pub const WRITE_MANY: u8 = 0x18;
    /// The event log, newest first (LINK-42): before (u32: records with a lower sequence number; 0xFFFFFFFF from
    /// the newest), then optionally the most to send (u8, 0: as many as fit). The answer: the next sequence number
    /// (u32), the records held (u16), how many follow (u8), then each ([`LOG_FIXED`] bytes, the position if it
    /// has one, then the name and the text, each a length and the bytes).
    pub const LOG_READ: u8 = 0x40;
    /// A slot's working fields, read or changed as the keypad would (LINK-33): the slot (0-2), then optionally a
    /// [`super::slot`] mask and its fields in bit order. The answer is the slot as it is after.
    pub const SLOT: u8 = 0x16;
    /// Propose a faster cable rate (LINK-11).
    pub const BAUD: u8 = 0x20;
    /// Restart into the bootloader, for a firmware update (LINK-24): the cable, or a Bluetooth session that may go
    /// on (LINK-30); refused while transmitting. The answer goes out first, then the radio saves, goes quiet and
    /// resets.
    pub const REBOOT: u8 = 0x21;
    /// A terminal command line (LINK-34, chapter 37): run as the terminal's command line runs it. The answer is
    /// its output: a byte, 1 if more follows (ask with [`MORE`]), then text, lines ending in `\n`; or `invalid` and
    /// the terminal's refusal. A new `CMD` ends one still running.
    pub const CMD: u8 = 0x31;
    /// The running command's next output, as `CMD`'s answer; 1 with no text: the radio is still working on it, ask
    /// again shortly. Nothing running: 0 and no text.
    pub const MORE: u8 = 0x32;
    /// A key on the radio (LINK-19, LINK-20): the key, then what it does ([`super::key`]). The keypad and side keys
    /// by stock's key code with a press type (LINK-19), the knob ([`super::key::KNOB`]), or remote PTT
    /// ([`super::Ptt`], LINK-20); anything else answers `bad argument`.
    pub const KEY: u8 = 0x30;
    /// What the screen shows (LINK-19's readback, decision 150): as `CMD` `ui`, its text in lines (the screen, the
    /// status bar, the home cards or the list's rows, the hint bar, a toast), `MORE` for the rest. An optional
    /// byte: 0 the state, 1 the key names (`ui keys`).
    pub const UI: u8 = 0x33;
    /// Events this session wants pushed (LINK-46): a [`super::event`] mask (0 ends them), then optionally the UI's
    /// least gap in 100 ms units and flags ([`super::event::RENEW`]). Answer: the mask, the Live View dictionary's
    /// version, the next UI sequence number.
    pub const SUBSCRIBE: u8 = 0x50;
    /// Radio to PC only, unasked: a frame of this type (below 0x80: the radio sends no requests, so it is never an
    /// answer) with the session's own event counter as its seq, the payload an [`super::event`] kind and its bytes
    /// (LINK-46). Sent by the PC as a request: answered `unknown type` as any other.
    pub const EVENT: u8 = 0x51;
    /// The APRS heard list (LINK-44): from an index (u8); the count, then as many stations as fit.
    pub const HEARD: u8 = 0x52;
    /// APRS messages, in and out, newest first (LINK-44): from an index (u16); the count (u16), then as many as fit.
    pub const MSGS: u8 = 0x53;
    /// Send an APRS message (LINK-45): to (length, text), the message (length, text); queued as the radio's
    /// compose queues it.
    pub const MSG_SEND: u8 = 0x54;
    /// Stop sending one of our APRS messages (LINK-49, APRS-13.1): to (length, text), the message (length, text);
    /// the newest of ours to that station with that text still being tried stops, kept as cancelled.
    pub const MSG_STOP: u8 = 0x55;
    /// The way back to stock (BACKUP-7, LINK-4, LINK-8; [`crate::restore`]): the cable only. `START` stops the radio
    /// and hands the cable to the restore until it resets; erase, program and CRC requests follow, then `FINISH`.
    pub const STOCK_RESTORE: u8 = 0x60;
}

/// `SUBSCRIBE`'s mask and `EVENT`'s kinds (LINK-46, LINK-48).
pub mod event {
    /// Mask bits.
    pub const SUB_HEARD: u8 = 1;
    pub const SUB_MSGS: u8 = 2;
    pub const SUB_UI: u8 = 4;
    /// `SUBSCRIBE`'s flags bit: keep the Live View stream as it is (a keepalive); without it a UI subscription starts
    /// over with the whole state.
    pub const RENEW: u8 = 1;
    /// Kinds: a station heard (the count, then the station as `HEARD` gives it).
    pub const HEARD: u8 = 1;
    /// A new message, in or out (the count, then the message as `MSGS` gives it).
    pub const MSG: u8 = 2;
    /// A message's state changed: acked, retrying, failed, read.
    pub const MSG_STATE: u8 = 3;
    /// Live View: the UI's compact state or what changed ([`crate::liveview`]).
    pub const UI: u8 = 4;
    /// The UI's least gap by default: Bluetooth 500 ms, the cable 200 ms; at least 100 ms.
    pub const UI_GAP_BT_MS: u32 = 500;
    pub const UI_GAP_CABLE_MS: u32 = 200;
    /// Events' bytes a second at most, and the most saved up: Bluetooth leaves room for the answers in its ~280
    /// (LINK-36).
    pub const RATE_BT: u32 = 200;
    pub const RATE_CABLE: u32 = 4000;
    pub const BURST: u32 = 300;
    /// Events stop when the session has sent nothing for this long ([`super::msg::SUBSCRIBE`] with
    /// [`RENEW`] keeps them going).
    pub const QUIET_MS: u32 = 10_000;
}

/// A `LOG_READ` record's fixed part (LINK-42): sequence number (u32), seconds ago (u32), source (u8), slot (u8),
/// flags (u8: [`LOG_UNREAD`], [`LOG_POS`]), frequency (u32, 10 Hz), id (u32), x (u16), y (u16).
pub const LOG_FIXED: usize = 23;
/// A `LOG_READ` record's flags: not yet read on the radio.
pub const LOG_UNREAD: u8 = 1;
/// The record has a position: latitude and longitude (i32 each, degrees x 1e7) follow the fixed part.
pub const LOG_POS: u8 = 2;

/// `SLOT`'s mask (LINK-33): which fields follow, in this order.
pub mod slot {
    /// The VFO's frequency, u32 in 10 Hz.
    pub const FREQ: u8 = 1;
    /// u8: 0 FM, 1 NFM, 2 AM.
    pub const MODE: u8 = 2;
    /// u8: 0 low, 1 mid, 2 high.
    pub const POWER: u8 = 4;
    /// u16, CHAN-3's tone word (the tone decoded).
    pub const RX_TONE: u8 = 8;
    /// u16 (the tone sent).
    pub const TX_TONE: u8 = 16;
    /// No bytes: to the slot's VFO (V/M), before the frequency.
    pub const VFO: u8 = 32;
    /// No bytes: make it the active slot (the SLOT key), after the rest.
    pub const ACTIVE: u8 = 64;
    /// i32 then u8, as the answer gives them: the signed shift in 10 Hz (0 simplex) and 0, or the split's TX
    /// frequency and 1 (MENU 5's paths; LINK-33).
    pub const OFFSET: u8 = 128;
    /// The answer's flags byte.
    pub const SPLIT: u8 = 1;
    pub const APRS: u8 = 2;
    pub const SCANNING: u8 = 4;
}

/// `KEY`'s payload: the key, its action, then the action's own bytes.
///
/// LINK-19: a keypad or side key is its stock key code (0-21: `keys::MAP`; PF1 0, PF2 1), then [`UP`], [`DOWN`],
/// [`SHORT`] or [`LONG`]; the knob is [`KNOB`], then the steps (i8, +1 a click clockwise, at most
/// [`KNOB_MAX`] either way). The PTT codes (24-26) are never keys here: transmit is remote PTT's alone (LINK-20).
pub mod key {
    /// A keypad key let go (as remote PTT's [`OFF`]).
    pub const UP: u8 = 0;
    /// A keypad key pressed and held, until [`UP`] (or 30 s: [`HOLD_MAX_MS`]).
    pub const DOWN: u8 = 1;
    /// Pressed and let go: a short press (KEY-3).
    pub const SHORT: u8 = 2;
    /// Held to the long press (KEY-3: 700 ms), then let go: the radio lets it go [`LONG_HOLD_MS`] after the
    /// answer, and answers `busy` to keys until then.
    pub const LONG: u8 = 3;
    /// How long a remote long press is held.
    pub const LONG_HOLD_MS: u32 = 750;
    /// A remote key held down longer than this is let go.
    pub const HOLD_MAX_MS: u32 = 30_000;
    /// The knob: then the steps, an i8.
    pub const KNOB: u8 = 0x81;
    /// The most knob steps in one `KEY`.
    pub const KNOB_MAX: i8 = 20;
    /// The last keypad code (stock's 0-21); 24 up are the PTTs, never keys here.
    pub const LAST_CODE: u8 = 21;
    /// Remote PTT (LINK-20). Actions [`ON`] (then the slot), [`ALIVE`], [`OFF`].
    pub const PTT: u8 = 0x80;
    /// Unkey: always `ok`, keyed or not, so a PC can send it whatever happened.
    pub const OFF: u8 = 0;
    /// Key up, with every transmit check a PTT press has; then a byte: the slot ([`ACTIVE`], or 0-2 for A-C).
    /// `ok` and the slot keyed, or `invalid` and the reason as text.
    pub const ON: u8 = 1;
    /// Keepalive: at least every [`KEEPALIVE_MS`] or the radio unkeys (no keepalive, no transmit). `ok` while
    /// keyed; `invalid` and why it ended once it has.
    pub const ALIVE: u8 = 2;
    /// The slot byte for "the active slot", as PTT A and the side connector key it (KEY-7).
    pub const ACTIVE: u8 = 0xFF;
    /// No keepalive for this long, and the radio unkeys (LINK-20).
    pub const KEEPALIVE_MS: u32 = 2000;
}

/// Remote PTT, as `KEY`'s payload carries it (LINK-20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ptt {
    /// Key up: the slot ([`key::ACTIVE`], or 0-2).
    On(u8),
    Alive,
    Off,
}

impl Ptt {
    /// The `KEY` payload; its length.
    pub fn encode(self, out: &mut [u8; 3]) -> usize {
        out[0] = key::PTT;
        match self {
            Ptt::On(slot) => {
                (out[1], out[2]) = (key::ON, slot);
                3
            }
            Ptt::Alive => {
                out[1] = key::ALIVE;
                2
            }
            Ptt::Off => {
                out[1] = key::OFF;
                2
            }
        }
    }

    /// A `KEY` payload: None if it isn't remote PTT, or is malformed (`ON` with no slot, or a slot past C).
    pub fn decode(p: &[u8]) -> Option<Ptt> {
        if p.first() != Some(&key::PTT) {
            return None;
        }
        match (p.get(1).copied()?, p.get(2).copied()) {
            (key::OFF, _) => Some(Ptt::Off),
            (key::ALIVE, _) => Some(Ptt::Alive),
            (key::ON, Some(s)) if s == key::ACTIVE || s < 3 => Some(Ptt::On(s)),
            _ => None,
        }
    }
}

/// What a `READ`, `WRITE` or `DELETE` is about.
pub mod what {
    /// The radio-wide settings, one record (index 0).
    pub const SETTINGS: u8 = 1;
    /// A channel by number, 0-999.
    pub const CHANNEL: u8 = 2;
    /// A zone by number.
    pub const ZONE: u8 = 3;
    /// A range preset by number.
    pub const PRESET: u8 = 4;
    /// The APRS settings, one record (index 0; LINK-43): MENU 0 5's that aren't in the settings record.
    pub const APRS: u8 = 5;
    /// An APRS contact, in callsign order (APRS-42): written by callsign whatever the index.
    pub const CONTACT: u8 = 6;
}

/// The first byte of every answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    /// The frame didn't decode (COBS, length or CRC); the type and seq echo what was readable.
    BadFrame = 1,
    UnknownType = 2,
    /// The payload is too short, or a field is out of range.
    BadArg = 3,
    /// Never allowed over the link (calibration, the update area, TX limits: LINK-3, LINK-5).
    Refused = 4,
    /// The radio's PIN is set and this Bluetooth session hasn't sent the right one: `AUTH` first (LINK-30).
    NotAuthorised = 5,
    /// Not sent since decision 104 (it was "the radio is asking its owner"); kept so the number isn't reused.
    Pending = 6,
    /// The radio's store is busy: send the same request again shortly.
    Busy = 7,
    /// Nothing at that number.
    NotFound = 8,
    /// The record isn't valid (LINK-5): the reason follows as text.
    Invalid = 9,
    /// The owner pressed BACK on the progress screen (LINK-9): the session's writes are refused until `END`.
    Cancelled = 10,
}

/// CRC-16/CCITT-FALSE.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { crc << 1 ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// COBS-encodes `src` into `dst` (no delimiter); the length written, or None if `dst` is too short.
pub fn cobs_encode(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let (mut code_at, mut o, mut code) = (0usize, 1usize, 1u8);
    for &b in src {
        if b == 0 {
            *dst.get_mut(code_at)? = code;
            (code_at, code) = (o, 1);
            o += 1;
        } else {
            *dst.get_mut(o)? = b;
            o += 1;
            code += 1;
            if code == 0xFF {
                *dst.get_mut(code_at)? = code;
                (code_at, code) = (o, 1);
                o += 1;
            }
        }
    }
    *dst.get_mut(code_at)? = code;
    Some(o)
}

/// COBS-decodes `buf` in place (no delimiter); the decoded length, or None if it is malformed.
pub fn cobs_decode(buf: &mut [u8]) -> Option<usize> {
    let (mut i, mut o) = (0usize, 0usize);
    while i < buf.len() {
        let code = buf[i] as usize;
        if code == 0 || i + code > buf.len() {
            return None;
        }
        for k in 1..code {
            buf[o] = buf[i + k];
            o += 1;
        }
        i += code;
        if code < 0xFF && i < buf.len() {
            buf[o] = 0;
            o += 1;
        }
    }
    Some(o)
}

/// A decoded body: type, seq and payload, the CRC checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub ty: u8,
    pub seq: u8,
    pub payload: &'a [u8],
}

/// Why a body didn't decode, with what could be read of it for the `BadFrame` answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameError {
    pub ty: u8,
    pub seq: u8,
}

/// The frame in `wire` (one frame's bytes, COBS-encoded, no delimiter), decoded in place.
pub fn parse(wire: &mut [u8]) -> Result<Frame<'_>, FrameError> {
    let echo = FrameError { ty: 0, seq: 0 };
    let n = cobs_decode(wire).ok_or(echo)?;
    let body = &wire[..n];
    let echo = FrameError { ty: body.first().copied().unwrap_or(0), seq: body.get(1).copied().unwrap_or(0) };
    if !(4..=MAX_BODY).contains(&n) {
        return Err(echo);
    }
    let crc = u16::from_le_bytes([body[n - 2], body[n - 1]]);
    if crc16(&body[..n - 2]) != crc {
        return Err(echo);
    }
    Ok(Frame { ty: body[0], seq: body[1], payload: &wire[2..n - 2] })
}

/// A frame on the wire into `out` (COBS and the 0x00 delimiter); the length, or None if it doesn't fit or the
/// payload is too long.
pub fn build(ty: u8, seq: u8, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let mut body = [0u8; MAX_BODY];
    body[0] = ty;
    body[1] = seq;
    body[2..2 + payload.len()].copy_from_slice(payload);
    let n = 2 + payload.len();
    let crc = crc16(&body[..n]);
    body[n..n + 2].copy_from_slice(&crc.to_le_bytes());
    let k = cobs_encode(&body[..n + 2], out)?;
    *out.get_mut(k)? = 0;
    Some(k + 1)
}

/// Bluetooth's UART rate (LINK-25, decision 152): the BT513 module's resting rate, 9600 8N1. Stock V0.27 sends its
/// KISS(BLE) frames to the module at 9600 (0x08016EEC: USART1 restarted at 0x2580 through 0x08023F68, the frame
/// written, then back to 115200); stock's own 115200 is for its CPS
/// programming (most likely the vendor's app sets the module to 115200 first: its FF31 handshake carries a rate byte).
pub const BT_BAUD: u32 = 9_600;

/// Bluetooth transmit pacing (LINK-36, decision 156): bursts of `burst` bytes to the BT513, each followed by
/// `gap_ms` quiet, counted from the burst's last byte handed to USART1. The module forwards what USART1 gives it as
/// 20-byte BLE notifications, one or so per connection event (7.5-50 ms), so 9600 baud sent back to back (960 B/s)
/// fills its buffer and it drops bytes: a 224-byte readback chunk arrived as 6 bytes (Oct 14, 2026). Stock never
/// paces (its USART1 writers poll TDBE byte after byte), but sends only short KISS frames that way. `burst` 0: no
/// pacing. The cable is never paced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BtPace {
    pub burst: u8,
    pub gap_ms: u16,
}

impl BtPace {
    /// One notification's worth, then a 50 ms gap: about 280 B/s at 9600 (a burst is on the wire 21 ms). The BT513
    /// sends what piled up only after its UART has been quiet for between 30 and 40 ms, and then only the last
    /// notification's worth: 30 ms lost every UI readback on the bench, 40 ms lost none (LINK-36).
    pub const DEFAULT: BtPace = BtPace { burst: 20, gap_ms: 50 };
    /// The longest gap `bt pace` takes.
    pub const MAX_GAP_MS: u16 = 1_000;

    /// From `bt pace B G`: B 0 (no pacing) to 255 bytes, G 0 to [`Self::MAX_GAP_MS`]; None outside.
    pub fn new(burst: u32, gap_ms: u32) -> Option<BtPace> {
        (burst <= 255 && gap_ms <= Self::MAX_GAP_MS as u32).then_some(BtPace { burst: burst as u8, gap_ms: gap_ms as u16 })
    }

    /// Packed for one atomic word (the Link task reads it each turn).
    pub const fn pack(self) -> u32 {
        (self.burst as u32) << 16 | self.gap_ms as u32
    }

    pub const fn unpack(w: u32) -> BtPace {
        BtPace { burst: (w >> 16) as u8, gap_ms: w as u16 }
    }

    /// Bytes a second it lets through at `baud` (8N1: 10 bits a byte), rounded down.
    pub fn bytes_per_s(self, baud: u32) -> u32 {
        if baud == 0 {
            return 0;
        }
        if self.burst == 0 {
            return baud / 10;
        }
        let byte_us = 10_000_000 / baud;
        let period_us = self.burst as u32 * byte_us + self.gap_ms as u32 * 1_000;
        (self.burst as u64 * 1_000_000 / period_us as u64) as u32
    }
}

impl Default for BtPace {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// LINK-36's state: what is left of the burst, and when the next one may start (the last byte's time plus the gap).
/// A burst cut short by an empty ring is topped up again only after a full gap, so short replies sent close together
/// never add up to more than one burst at the module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BtPacer {
    left: u8,
    next_ms: u64,
}

impl BtPacer {
    /// How many bytes may go at `now_ms` (`usize::MAX` without pacing).
    pub fn allow(&mut self, pace: BtPace, now_ms: u64) -> usize {
        if pace.burst == 0 {
            return usize::MAX;
        }
        if now_ms >= self.next_ms {
            self.left = pace.burst;
        }
        self.left.min(pace.burst) as usize
    }

    /// `n` bytes went at `now_ms` (n at most what [`Self::allow`] gave).
    pub fn sent(&mut self, pace: BtPace, n: usize, now_ms: u64) {
        if pace.burst == 0 || n == 0 {
            return;
        }
        self.left = self.left.saturating_sub(n.min(255) as u8);
        self.next_ms = now_ms + pace.gap_ms as u64;
    }
}

/// Cable rates `BAUD` may ask for (LINK-11); the radio takes one only if its UART reaches it within 2 %.
pub const RATES: [u32; 4] = [115_200, 230_400, 460_800, 921_600];

#[cfg(test)]
mod tests {
    use super::*;

    /// LINK-36: bursts of B bytes, then G ms of quiet from the last byte; a short burst is topped up only after a
    /// full gap; B 0 is unpaced; `bt pace`'s limits; the rate it allows at 9600.
    #[test]
    fn bt_pace_bursts_and_gaps() {
        assert_eq!((BtPace::DEFAULT.burst, BtPace::DEFAULT.gap_ms), (20, 50));
        let p = BtPace::new(20, 30).unwrap();
        let mut s = BtPacer::default();
        assert_eq!(s.allow(p, 0), 20);
        s.sent(p, 12, 0);
        assert_eq!(s.allow(p, 1), 8, "the rest of the burst");
        s.sent(p, 8, 9);
        assert_eq!(s.allow(p, 10), 0, "the gap");
        assert_eq!(s.allow(p, 38), 0, "still 1 ms short");
        assert_eq!(s.allow(p, 39), 20, "30 ms after the last byte");
        // A short reply, then another soon after: the second only tops up the same burst.
        let mut s = BtPacer::default();
        let n = s.allow(p, 100).min(6);
        s.sent(p, n, 100);
        assert_eq!(s.allow(p, 110), 14);
        assert_eq!(s.allow(p, 130), 20, "a full gap since: a fresh burst");
        // Unpaced.
        let off = BtPace::new(0, 0).unwrap();
        let mut s = BtPacer::default();
        assert_eq!(s.allow(off, 0), usize::MAX);
        s.sent(off, 500, 0);
        assert_eq!(s.allow(off, 0), usize::MAX);
        // Limits and packing.
        assert_eq!(BtPace::new(256, 30), None);
        assert_eq!(BtPace::new(20, 1_001), None);
        let q = BtPace::new(255, 1_000).unwrap();
        assert_eq!(BtPace::unpack(q.pack()), q);
        assert_eq!(BtPace::unpack(p.pack()), p);
        // Rates at 9600: 960 B/s unpaced, about 282 B/s by default, 393 with a 30 ms gap, about 650 with 10 ms.
        assert_eq!(off.bytes_per_s(BT_BAUD), 960);
        assert_eq!(BtPace::DEFAULT.bytes_per_s(BT_BAUD), 282);
        assert_eq!(p.bytes_per_s(BT_BAUD), 393);
        assert_eq!(BtPace::new(20, 10).unwrap().bytes_per_s(BT_BAUD), 648);
    }

    #[test]
    fn crc_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29B1, "CRC-16/CCITT-FALSE's check value");
    }

    #[test]
    fn cobs_round_trips() {
        let cases: [&[u8]; 6] = [&[], &[0], &[0, 0], &[1, 2, 3], &[0x11, 0, 0x22, 0, 0], &[7; 600]];
        for src in cases {
            let mut enc = [0u8; 700];
            let n = cobs_encode(src, &mut enc).unwrap();
            assert!(!enc[..n].contains(&0), "no zero inside: {src:?}");
            let k = cobs_decode(&mut enc[..n]).unwrap();
            assert_eq!(&enc[..k], src);
        }
    }

    #[test]
    fn long_runs_without_zero() {
        let src = [0x55u8; 254];
        let mut enc = [0u8; 300];
        let n = cobs_encode(&src, &mut enc).unwrap();
        assert_eq!(n, 256);
        assert_eq!(cobs_decode(&mut enc[..n]).unwrap(), 254);
    }

    #[test]
    fn frames_round_trip_and_reject_damage() {
        let mut wire = [0u8; MAX_WIRE];
        let n = build(msg::READ, 7, &[2, 11, 0, 0, 0, 64], &mut wire).unwrap();
        assert_eq!(wire[n - 1], 0, "delimited");
        let mut w = wire;
        let f = parse(&mut w[..n - 1]).unwrap();
        assert_eq!((f.ty, f.seq, f.payload), (msg::READ, 7, &[2u8, 11, 0, 0, 0, 64][..]));
        let mut bad = wire;
        bad[3] ^= 0x40;
        let e = parse(&mut bad[..n - 1]).unwrap_err();
        assert_eq!((e.ty, e.seq), (msg::READ, 7), "the damage is past the type and seq");
        assert!(build(1, 0, &[0; MAX_PAYLOAD + 1], &mut wire).is_none());
        let n = build(1, 0, &[0; MAX_PAYLOAD], &mut wire).unwrap();
        assert!(n <= MAX_WIRE);
    }

    /// LINK-20.1: remote PTT's `KEY` payloads encode and decode, framed too; malformed ones don't decode.
    #[test]
    fn remote_ptt_payloads() {
        for p in [Ptt::On(key::ACTIVE), Ptt::On(0), Ptt::On(1), Ptt::On(2), Ptt::Alive, Ptt::Off] {
            let mut b = [0u8; 3];
            let n = p.encode(&mut b);
            assert_eq!(Ptt::decode(&b[..n]), Some(p));
            let mut wire = [0u8; MAX_WIRE];
            let k = build(msg::KEY, 3, &b[..n], &mut wire).unwrap();
            let f = parse(&mut wire[..k - 1]).unwrap();
            assert_eq!((f.ty, Ptt::decode(f.payload)), (msg::KEY, Some(p)));
        }
        assert_eq!(Ptt::On(1).encode(&mut [0; 3]), 3);
        let mut b = [0u8; 3];
        Ptt::On(key::ACTIVE).encode(&mut b);
        assert_eq!(b, [0x80, 1, 0xFF], "the bytes Appendix G gives");
        for bad in [&[][..], &[0x80], &[0x80, 1], &[0x80, 1, 3], &[0x80, 7], &[0x10, 1, 0]] {
            assert_eq!(Ptt::decode(bad), None, "{bad:?}");
        }
    }
}
