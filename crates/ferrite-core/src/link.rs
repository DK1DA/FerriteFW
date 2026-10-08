//! The link protocol's wire format (manual chapter 29, LINK-1 – LINK-11; Appendix "Link protocol v1"): shared by
//! the radio, the simulators and the PC tools. Plain logic: framing, checksums, message and status numbers.
//!
//! On the wire a frame is its body COBS-encoded, then a 0x00 delimiter (0x00 never appears inside). The body is
//! `type, seq, payload..., crc16` with the CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) of type, seq and
//! payload, little-endian. The PC sends requests (types 0x01-0x7F); the radio answers each one with type | 0x80,
//! the same seq, and a payload that starts with a [`Status`] byte: every frame is acknowledged (LINK-1).

/// The protocol version in `HELLO` and its answer.
pub const PROTOCOL: u8 = 1;
/// The most payload in a frame (BLE chunks stay small: LINK-16).
pub const MAX_PAYLOAD: usize = 240;
/// The most body bytes: type, seq, payload, CRC.
pub const MAX_BODY: usize = MAX_PAYLOAD + 4;
/// The most bytes a frame takes on the wire: COBS adds one byte per 254, plus the delimiter.
pub const MAX_WIRE: usize = MAX_BODY + MAX_BODY / 254 + 2;
/// Answers carry the request's type with this bit set.
pub const ANSWER: u8 = 0x80;

/// Requests, PC to radio. The numbers are part of the published protocol: never reuse one.
pub mod msg {
    /// Start a session: protocol version, an approval token (8 bytes, zero for none), the client's name.
    pub const HELLO: u8 = 0x01;
    /// Firmware version and what the radio holds.
    pub const INFO: u8 = 0x02;
    /// After a write was approved on the radio's screen: the token to present next time (LINK-7).
    pub const TOKEN: u8 = 0x03;
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
    /// Propose a faster cable rate (LINK-11).
    pub const BAUD: u8 = 0x20;
    /// Restart into the bootloader, for a firmware update (LINK-24): the cable, or an approved Bluetooth
    /// device; refused while transmitting. The answer goes out first, then the radio saves, goes quiet and
    /// resets.
    pub const REBOOT: u8 = 0x21;
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
    /// This device may not write: the owner refused it on the radio (LINK-7).
    NotApproved = 5,
    /// The radio is asking its owner now (LINK-7): send the same request again in a second.
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

/// Cable rates `BAUD` may ask for (LINK-11); the radio takes one only if its UART reaches it within 2 %.
pub const RATES: [u32; 4] = [115_200, 230_400, 460_800, 921_600];

#[cfg(test)]
mod tests {
    use super::*;

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
}
