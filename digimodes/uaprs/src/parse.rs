//! APRS packets from a frame's information field, after the APRS Protocol Reference 1.0.1 and its 1.1 and 1.2
//! addenda. Nothing is copied: text fields borrow from the information field.
//!
//! Units are APRS's own: positions in millionths of a degree, speed in knots, altitude in feet, weather in the
//! units it is sent in (mph, °F, hundredths of an inch, tenths of a hectopascal).

use crate::ax25::Tnc2;

/// Why an information field is not a valid APRS packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than its type needs.
    Short,
    /// A position with bad digits, hemisphere or range.
    Position,
    /// A symbol table or code out of range.
    Symbol,
    /// Mic-E: the destination address or the information field does not decode.
    MicE,
    /// A message without its 9-character addressee and second colon.
    Message,
    /// An object or item with a bad name or live/killed flag.
    Object,
    /// An NMEA sentence without a fix, or with a bad checksum or field.
    Nmea,
    /// A timestamp that is not one of the three APRS forms.
    Timestamp,
}

/// How a position was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Uncompressed,
    Compressed,
    MicE,
    Nmea,
}

/// A position and what travels with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// Latitude, millionths of a degree, north positive.
    pub lat: i32,
    /// Longitude, millionths of a degree, east positive.
    pub lon: i32,
    /// Digits left blank for privacy, 0 to 4 (4: to the nearest degree).
    pub ambiguity: u8,
    /// Symbol table (`/` primary, `\` alternate, or an overlay character on the alternate table) and code.
    pub table: u8,
    pub code: u8,
    /// Course, degrees 1 to 360 (none when unknown or not sent).
    pub course: Option<u16>,
    /// Speed, knots.
    pub speed: Option<u16>,
    /// Altitude, feet.
    pub altitude: Option<i32>,
    /// Power, height, gain and directivity (`PHGphgd`), as the four characters sent.
    pub phg: Option<[u8; 4]>,
    /// Radio range, miles (`RNGrrrr`, or compressed range).
    pub range: Option<u16>,
    pub format: Format,
}

impl Position {
    pub(crate) fn new(lat: i32, lon: i32, table: u8, code: u8, format: Format) -> Self {
        Position {
            lat,
            lon,
            ambiguity: 0,
            table,
            code,
            course: None,
            speed: None,
            altitude: None,
            phg: None,
            range: None,
            format,
        }
    }
}

/// When a packet says it was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timestamp {
    /// `DDHHMMz` (UTC) or `DDHHMM/` (the sender's local time).
    DayHourMin { day: u8, hour: u8, min: u8, utc: bool },
    /// `HHMMSSh`, UTC.
    HourMinSec { hour: u8, min: u8, sec: u8 },
    /// `MMDDHHMM`, UTC: positionless weather reports.
    MonthDayHourMin { month: u8, day: u8, hour: u8, min: u8 },
}

/// Mic-E message code: one of seven standard messages (Off Duty, En Route, ... Emergency) or seven custom ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MicEStatus {
    /// The three message bits A, B, C as sent (`0b111` = M0 "Off Duty", `0b000` = Emergency).
    pub bits: u8,
    /// The bits are custom (C0 to C6) rather than standard.
    pub custom: bool,
}

/// A comment: the text after a position, with what was read out of it (an `/A=` altitude, a `!DAO!` precision
/// extension, a Mic-E altitude) cut out, without copying. Read it with [`Comment::bytes`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Comment<'a> {
    text: &'a [u8],
    /// Ranges of `text` left out; empty ranges are unused.
    cuts: [(u16, u16); 2],
}

impl<'a> Comment<'a> {
    fn plain(text: &'a [u8]) -> Self {
        Comment { text, cuts: [(0, 0); 2] }
    }

    fn kept(&self, i: usize) -> bool {
        self.cuts.iter().all(|&(a, b)| i < a as usize || i >= b as usize)
    }

    /// The comment's bytes.
    pub fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.text.iter().enumerate().filter(|(i, _)| self.kept(*i)).map(|(_, &c)| c)
    }

    pub fn len(&self) -> usize {
        self.bytes().count()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes().next().is_none()
    }

    /// The text as received, cut-out parts included.
    pub fn raw(&self) -> &'a [u8] {
        self.text
    }

    /// Leaves out `text[a..b]` (relative to the whole text). False if both cut slots are taken.
    fn cut(&mut self, a: usize, b: usize) -> bool {
        match self.cuts.iter_mut().find(|c| c.0 == c.1) {
            Some(c) => {
                *c = (a as u16, b as u16);
                true
            }
            None => false,
        }
    }

    /// Without spaces at either end.
    fn trim(self) -> Self {
        let keep = |i: usize| self.kept(i) && self.text[i] != b' ';
        let Some(first) = (0..self.text.len()).find(|&i| keep(i)) else {
            return Comment::plain(&[]);
        };
        let last = (0..self.text.len()).rev().find(|&i| keep(i)).unwrap();
        self.slice(first, last)
    }

    /// Without one leading `/` or space: the delimiter after data extensions and an altitude.
    fn strip_delimiter(self) -> Self {
        match (0..self.text.len()).find(|&i| self.kept(i)) {
            Some(i) if matches!(self.text[i], b'/' | b' ') => self.slice(i + 1, self.text.len().max(i + 1) - 1),
            _ => self,
        }
    }

    /// `text[first..=last]`, keeping the cuts inside it.
    fn slice(self, first: usize, last: usize) -> Self {
        if first > last || first >= self.text.len() {
            return Comment::plain(&[]);
        }
        let mut c = Comment::plain(&self.text[first..=last]);
        for &(a, b) in &self.cuts {
            let (a, b) = ((a as usize).clamp(first, last + 1) - first, (b as usize).clamp(first, last + 1) - first);
            if a < b {
                c.cut(a, b);
            }
        }
        c
    }
}

/// Weather, in the units sent; fields not sent are `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Weather {
    /// Wind direction, degrees.
    pub wind_dir: Option<u16>,
    /// Sustained wind and peak gust, mph.
    pub wind_speed: Option<u16>,
    pub wind_gust: Option<u16>,
    /// Temperature, °F.
    pub temp: Option<i16>,
    /// Rain in the last hour, the last 24 hours and since midnight, hundredths of an inch.
    pub rain_1h: Option<u16>,
    pub rain_24h: Option<u16>,
    pub rain_midnight: Option<u16>,
    /// Relative humidity, percent.
    pub humidity: Option<u8>,
    /// Barometric pressure, tenths of a hectopascal.
    pub pressure: Option<u32>,
    /// Luminosity, W/m².
    pub luminosity: Option<u16>,
}

/// What a message packet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Message,
    /// Acknowledges message `id`.
    Ack,
    /// Rejects message `id`.
    Rej,
}

/// A message, acknowledgement or rejection (`:ADDRESSEE:text{id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message<'a> {
    /// Addressee, trailing spaces removed.
    pub addressee: &'a [u8],
    pub kind: MessageKind,
    /// The message text (empty for acks and rejections).
    pub text: &'a [u8],
    /// Message number: the one to acknowledge, or the one acknowledged or rejected.
    pub id: Option<&'a [u8]>,
    /// APRS 1.1 reply-ack (`{MM}AA`): the number of a message of ours this one also acknowledges.
    pub reply_ack: Option<&'a [u8]>,
}

/// An APRS packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packet<'a> {
    /// A station's position: plain, compressed, Mic-E or NMEA.
    Position {
        pos: Position,
        /// The station can receive messages (`=`, `@`, Mic-E).
        messaging: bool,
        timestamp: Option<Timestamp>,
        /// Mic-E only.
        mic_e: Option<MicEStatus>,
        /// A weather station's report, when the symbol is `_`.
        weather: Option<Weather>,
        comment: Comment<'a>,
    },
    Message(Message<'a>),
    /// An object: something placed on the map by this station (a meeting point, a storm).
    Object { name: &'a [u8], live: bool, timestamp: Timestamp, pos: Position, comment: Comment<'a> },
    /// An item: an object without a timestamp.
    Item { name: &'a [u8], live: bool, pos: Position, comment: Comment<'a> },
    Status { timestamp: Option<Timestamp>, text: &'a [u8] },
    /// Weather without a position.
    Weather { timestamp: Option<Timestamp>, weather: Weather, comment: &'a [u8] },
    /// Station capabilities (`<IGATE,MSG_CNT=...`).
    Capabilities(&'a [u8]),
    /// Telemetry (`T#seq,v1,...`), after the `T#`.
    Telemetry(&'a [u8]),
    /// Another station's packet carried inside this one (an iGate passing internet traffic to RF). Parse its
    /// `info` with its `dest` to read it.
    ThirdParty(Tnc2<'a>),
    /// A type this parser does not read; `kind` is the type character.
    Other { kind: u8, data: &'a [u8] },
}

/// Parses an information field. `dest` is the destination callsign, without SSID: Mic-E encodes the latitude
/// in it, and NMEA positions take their symbol from it. Trailing carriage returns and line feeds (which many
/// radios add) are ignored, as APRS-IS strips them.
pub fn parse<'a>(dest: &[u8], info: &'a [u8]) -> Result<Packet<'a>, Error> {
    let mut info = info;
    while let Some((b'\r' | b'\n', rest)) = info.split_last() {
        info = rest;
    }
    let (&kind, body) = info.split_first().ok_or(Error::Short)?;
    match kind {
        b'!' | b'=' => position_report(body, kind == b'=', None),
        b'/' | b'@' => {
            let (ts, rest) = timestamp(body)?;
            position_report(rest, kind == b'@', Some(ts))
        }
        b'`' | b'\'' | 0x1C | 0x1D => mic_e(dest, body),
        b':' => message(body),
        b';' => object(body),
        b')' => item(body),
        b'>' => Ok(status(body)),
        b'_' => positionless_weather(body),
        b'<' => Ok(Packet::Capabilities(body)),
        b'T' if body.first() == Some(&b'#') => Ok(Packet::Telemetry(&body[1..])),
        b'}' => Tnc2::split(body).map(Packet::ThirdParty).ok_or(Error::Short),
        #[cfg(feature = "nmea")]
        b'$' if body.starts_with(b"GP") || body.starts_with(b"GN") => nmea(dest, body),
        #[cfg(feature = "ultimeter")]
        b'$' if body.starts_with(b"ULTW") => ultimeter(&body[4..]),
        _ => {
            // Some old trackers put text before the position: a '!' within the first 40 characters starts it.
            match info[..info.len().min(40)].iter().position(|&c| c == b'!') {
                Some(i) if i > 0 => position_report(&info[i + 1..], false, None),
                _ => Ok(Packet::Other { kind, data: body }),
            }
        }
    }
}

/// 1 to 9 decimal digits (so the value fits; a received packet may send any number).
/// 1 to 8 hex digits (without pulling in UTF-8 checks and `from_str_radix`).
#[cfg(any(feature = "nmea", feature = "ultimeter"))]
fn hex(b: &[u8]) -> Option<u32> {
    if b.is_empty() || b.len() > 8 {
        return None;
    }
    b.iter().try_fold(0, |v, &c| Some(v << 4 | (c as char).to_digit(16)?))
}

fn digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() || b.len() > 9 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(b.iter().fold(0, |v, &c| v * 10 + (c - b'0') as u32))
}

fn timestamp(b: &[u8]) -> Result<(Timestamp, &[u8]), Error> {
    let t = b.get(..7).ok_or(Error::Short)?;
    let n = |i: usize| digits(&t[i..i + 2]).map(|v| v as u8).ok_or(Error::Timestamp);
    let ts = match t[6] {
        b'z' | b'/' => Timestamp::DayHourMin { day: n(0)?, hour: n(2)?, min: n(4)?, utc: t[6] == b'z' },
        b'h' => Timestamp::HourMinSec { hour: n(0)?, min: n(2)?, sec: n(4)? },
        _ => return Err(Error::Timestamp),
    };
    let ok = match ts {
        Timestamp::DayHourMin { day, hour, min, .. } => day <= 31 && hour < 24 && min < 60,
        Timestamp::HourMinSec { hour, min, sec } => hour < 24 && min < 60 && sec < 60,
        Timestamp::MonthDayHourMin { .. } => true,
    };
    if !ok {
        return Err(Error::Timestamp);
    }
    Ok((ts, &b[7..]))
}

fn valid_table(t: u8) -> bool {
    matches!(t, b'/' | b'\\' | b'A'..=b'Z' | b'0'..=b'9')
}

fn valid_code(c: u8) -> bool {
    (b'!'..=b'~').contains(&c)
}

/// `DDMM.hh` (latitude, `width` 2) or `DDDMM.hh` (longitude, 3), spaces allowed in place of trailing digits.
/// Returns millionths of a degree (unsigned) and the number of blanked digits.
fn ddmm(b: &[u8], width: usize) -> Option<(i32, u8)> {
    let b = b.get(..width + 5)?;
    if b[width + 2] != b'.' {
        return None;
    }
    // The digits, most significant first; blanks may only replace a suffix of the minutes.
    let mut ds = [0u8; 7];
    let mut n = 0;
    let mut blanks = 0u8;
    for (i, &c) in b.iter().enumerate() {
        if i == width + 2 {
            continue;
        }
        match c {
            b'0'..=b'9' if blanks == 0 => ds[n] = c - b'0',
            b' ' if i >= width => {
                blanks += 1;
                ds[n] = 0;
            }
            _ => return None,
        }
        n += 1;
    }
    let deg = ds[..width].iter().fold(0i32, |v, &d| v * 10 + d as i32);
    // Hundredths of a minute.
    let hmin = ds[width..n].iter().fold(0i32, |v, &d| v * 10 + d as i32);
    if hmin >= 6000 {
        return None;
    }
    // A blanked position stands for the middle of the box it leaves (as aprs.fi and Ham::APRS::FAP show it).
    let half = match blanks {
        0 => 0,
        1 => 5,
        2 => 50,
        3 => 500,
        _ => 3000,
    };
    let hmin = if blanks >= 4 { 3000 } else { hmin + half };
    Some((deg * 1_000_000 + (hmin * 1000 + 3) / 6, blanks))
}

/// `DDMM.hhN` + table + `DDDMM.hhW` + code.
/// Hemispheres as sent (south, west): a position of exactly 0 keeps them, for the `!DAO!` digit.
type Hemispheres = (bool, bool);

fn uncompressed(b: &[u8]) -> Result<(Position, &[u8], Hemispheres), Error> {
    let b19 = b.get(..19).ok_or(Error::Short)?;
    let (lat, amb) = ddmm(b19, 2).ok_or(Error::Position)?;
    let lat = match b19[7] {
        b'N' | b'n' => lat,
        b'S' | b's' => -lat,
        _ => return Err(Error::Position),
    };
    let table = b19[8];
    let (lon, _) = ddmm(&b19[9..], 3).ok_or(Error::Position)?;
    let lon = match b19[17] {
        b'E' | b'e' => lon,
        b'W' | b'w' => -lon,
        _ => return Err(Error::Position),
    };
    if lat.abs() > 90_000_000 || lon.abs() > 180_000_000 {
        return Err(Error::Position);
    }
    let code = b19[18];
    if !valid_table(table) || !valid_code(code) {
        return Err(Error::Symbol);
    }
    let mut p = Position::new(lat, lon, table, code, Format::Uncompressed);
    p.ambiguity = amb;
    Ok((p, &b[19..], (matches!(b19[7], b'S' | b's'), matches!(b19[17], b'W' | b'w'))))
}

fn base91(b: &[u8]) -> Option<u32> {
    b.iter().try_fold(0u32, |v, &c| (33..=123).contains(&c).then(|| v * 91 + (c - 33) as u32))
}

/// `base`^`n` with `base` in 32.32 fixed point; the result in 16.16. For compressed speed (1.08^n, n < 92) and
/// altitude (1.002^n, n < 8281): values stay under 2^24, so 32.32 products fit 128 bits. Run per packet, not per
/// sample, so the 128-bit arithmetic costs nothing that matters.
fn pow_q16(base: u64, mut n: u32) -> u64 {
    let (mut r, mut b) = (1u128 << 32, base as u128);
    while n > 0 {
        if n & 1 != 0 {
            r = (r * b) >> 32;
        }
        n >>= 1;
        if n > 0 {
            b = (b * b) >> 32;
        }
    }
    ((r + (1 << 15)) >> 16) as u64
}

/// 1.08 and 1.002 in 32.32 fixed point.
const POW_108: u64 = 4638564680;
const POW_1002: u64 = 4303557231;

/// 1.08^`n` in 16.16 fixed point: compressed speed and range.
pub(crate) fn pow108_q16(n: u32) -> u64 {
    pow_q16(POW_108, n)
}

/// Table, 4 latitude and 4 longitude characters in base 91, code, then course/speed, range or altitude.
fn compressed(b: &[u8]) -> Result<(Position, &[u8], Hemispheres), Error> {
    let b13 = b.get(..13).ok_or(Error::Short)?;
    let table = match b13[0] {
        // Overlay digits are sent as a to j.
        t @ b'a'..=b'j' => t - b'a' + b'0',
        t => t,
    };
    let (y, x) = (base91(&b13[1..5]).ok_or(Error::Position)?, base91(&b13[5..9]).ok_or(Error::Position)?);
    let code = b13[9];
    if !valid_table(table) || !valid_code(code) {
        return Err(Error::Symbol);
    }
    // lat = 90 - y / 380926 degrees; lon = -180 + x / 190463.
    let lat = 90_000_000 - ((y as i64 * 1_000_000 + 190_463) / 380_926) as i32;
    let lon = -180_000_000 + ((x as i64 * 1_000_000 + 95_231) / 190_463) as i32;
    if lat.abs() > 90_000_000 || lon.abs() > 180_000_000 {
        return Err(Error::Position);
    }
    let mut p = Position::new(lat, lon, table, code, Format::Compressed);
    let (c, s, t) = (b13[10], b13[11], b13[12]);
    if c != b' ' && (33..=123).contains(&c) && (33..=123).contains(&s) && (33..=123).contains(&t) {
        let (c, s, t) = ((c - 33) as u32, (s - 33) as u32, (t - 33) as u32);
        if t & 0x18 == 0x10 {
            // GGA source: altitude = 1.002^cs feet.
            p.altitude = Some(((pow_q16(POW_1002, c * 91 + s) + (1 << 15)) >> 16) as i32);
        } else if c <= 89 {
            // course = c x 4 degrees, speed = 1.08^s - 1 knots.
            if c > 0 {
                p.course = Some((c * 4) as u16);
            }
            p.speed = Some(((pow_q16(POW_108, s) - (1 << 16) + (1 << 15)) >> 16) as u16);
        } else if c == 90 {
            // '{': range = 2 x 1.08^s miles.
            p.range = Some(((2 * pow_q16(POW_108, s) + (1 << 15)) >> 16) as u16);
        }
    }
    Ok((p, &b[13..], (p.lat < 0, p.lon < 0)))
}

fn position(b: &[u8]) -> Result<(Position, &[u8], Hemispheres), Error> {
    match b.first() {
        Some(c) if c.is_ascii_digit() || *c == b' ' => uncompressed(b),
        Some(_) => compressed(b),
        None => Err(Error::Short),
    }
}

/// Data extensions after an uncompressed position: `CSE/SPD`, `PHGphgd`, `RNGrrrr`.
pub(crate) fn extensions<'a>(p: &mut Position, b: &'a [u8]) -> &'a [u8] {
    if b.len() >= 7 {
        let e = &b[..7];
        if e[3] == b'/' {
            if let (Some(c), Some(s)) = (digits(&e[..3]), digits(&e[4..7])) {
                if (1..=360).contains(&c) {
                    p.course = Some(c as u16);
                }
                p.speed = Some(s as u16);
                return &b[7..];
            }
            if e[..3].iter().all(|&c| c == b'.' || c == b' ') {
                return &b[7..];
            }
        } else if e.starts_with(b"PHG") && e[3..].iter().all(u8::is_ascii_alphanumeric) {
            p.phg = Some([e[3], e[4], e[5], e[6]]);
            // PHGR (APRS 1.2): a fifth character, the beacon rate, then '/'.
            if b.len() >= 9 && b[7].is_ascii_alphanumeric() && b[8] == b'/' {
                return &b[8..];
            }
            return &b[7..];
        } else if e.starts_with(b"RNG") {
            if let Some(r) = digits(&e[3..7]) {
                p.range = Some(r as u16);
                return &b[7..];
            }
        }
    }
    b
}

/// Takes `/A=nnnnnn` (feet, or `/A=-nnnnn`) out of a comment.
fn altitude(c: &mut Comment) -> Option<i32> {
    let t = c.text;
    for i in 0..t.len().saturating_sub(8) {
        if &t[i..i + 3] == b"/A=" && c.kept(i) {
            let v = &t[i + 3..i + 9];
            let alt = match v[0] {
                b'-' => digits(&v[1..]).map(|a| -(a as i32)),
                _ => digits(v).map(|a| a as i32),
            };
            if alt.is_some() && c.cut(i, i + 9) {
                return alt;
            }
        }
    }
    None
}

/// The `!DAO!` extension (APRS 1.2): datum and one more digit of latitude and longitude, as `!W12!` (thousandths
/// of a minute) or `!w"#!` (base 91, hundredths of a minute / 91). Refines `p` and cuts it out of the comment.
fn dao(p: &mut Position, (south, west): Hemispheres, c: &mut Comment) {
    let t = c.text;
    // The last one counts, as APRS 1.2 recommends.
    for i in (0..t.len().saturating_sub(4)).rev() {
        let w = &t[i..i + 5];
        if w[0] != b'!' || w[4] != b'!' || !c.kept(i) || !c.kept(i + 4) {
            continue;
        }
        // Millionths of a degree to add: thousandths of a minute = 1/60 of 1000 µdeg.
        let off = match (w[1], w[2], w[3]) {
            (b'A'..=b'Z', a @ b'0'..=b'9', o @ b'0'..=b'9') => {
                Some((((a - b'0') as i32 * 1000 + 30) / 60, ((o - b'0') as i32 * 1000 + 30) / 60))
            }
            (b'a'..=b'z', a @ 33..=123, o @ 33..=123) => {
                // (x - 33) / 91 hundredths of a minute: x 10000 / 91 / 60 µdeg.
                Some((((a - 33) as i32 * 10_000 + 2730) / 5460, ((o - 33) as i32 * 10_000 + 2730) / 5460))
            }
            (33..=123, b' ', b' ') => Some((0, 0)),
            _ => None,
        };
        if let Some((dlat, dlon)) = off {
            if c.cut(i, i + 5) {
                p.lat += if south { -dlat } else { dlat };
                p.lon += if west { -dlon } else { dlon };
            }
            return;
        }
    }
}

/// Weather fields as letters and fixed-width values (`g005t077r000p000P000h50b09900`); returns what was read
/// and the rest.
fn weather_fields<'a>(w: &mut Weather, mut b: &'a [u8]) -> &'a [u8] {
    fn val(v: &[u8]) -> Option<Option<i32>> {
        // Unknown values are sent as dots or spaces.
        if v.iter().all(|&c| c == b'.' || c == b' ') {
            return Some(None);
        }
        // Only temperature may be negative (checked below).
        let (neg, d) = match v.split_first() {
            Some((b'-', rest)) => (true, rest),
            _ => (false, v),
        };
        let n = digits(d)? as i32;
        Some(Some(if neg { -n } else { n }))
    }
    loop {
        // Some stations send a field's dots without its letter: skip them.
        while let Some((b'.', rest)) = b.split_first() {
            b = rest;
        }
        let width = match b.first() {
            // A negative temperature can carry three digits after its sign (t-103).
            Some(b't') if b.get(1) == Some(&b'-') && b.get(2..5).is_some_and(|d| d.iter().all(u8::is_ascii_digit)) => 4,
            Some(b'c' | b's' | b'g' | b't' | b'r' | b'p' | b'P' | b'L' | b'l' | b'#') => 3,
            Some(b'h') => 2,
            Some(b'b') => 5,
            Some(b'v') => {
                // Station voltage or version (v6, v-12): not kept.
                let n = b[1..].iter().take_while(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+')).count();
                if n == 0 {
                    return b;
                }
                b = &b[1 + n..];
                continue;
            }
            _ => return b,
        };
        let Some(v) = b.get(1..1 + width).and_then(val) else {
            // A humidity of 100% is sometimes sent as h100.
            return b;
        };
        if v.is_some_and(|v| v < 0) && b[0] != b't' {
            return b; // a sign on anything but temperature: not a weather field
        }
        match b[0] {
            b'c' => w.wind_dir = v.map(|v| v as u16),
            b's' => w.wind_speed = v.map(|v| v as u16),
            b'g' => w.wind_gust = v.map(|v| v as u16),
            b't' => w.temp = v.map(|v| v as i16),
            b'r' => w.rain_1h = v.map(|v| v as u16),
            b'p' => w.rain_24h = v.map(|v| v as u16),
            b'P' => w.rain_midnight = v.map(|v| v as u16),
            b'h' => w.humidity = v.map(|v| if v == 0 { 100 } else { v.min(100) as u8 }),
            b'b' => w.pressure = v.map(|v| v as u32),
            b'L' => w.luminosity = v.map(|v| v as u16),
            b'l' => w.luminosity = v.map(|v| v as u16 + 1000),
            _ => {}
        }
        b = &b[1 + width..];
    }
}

/// After a position: extensions, weather when the symbol is `_`, altitude, comment.
fn position_tail(mut p: Position, rest: &[u8], hemi: Hemispheres) -> (Position, Option<Weather>, Comment<'_>) {
    let mut weather = None;
    let mut rest = rest;
    // A weather station's report: the `_` symbol, then wind direction and speed in the course/speed place.
    let wind = (p.code == b'_' && rest.len() >= 7 && rest[3] == b'/')
        .then(|| (wx_val(&rest[..3]), wx_val(&rest[4..7])))
        .and_then(|(d, s)| Some((d?, s?)));
    if let Some((d, s)) = wind {
        let mut w = Weather { wind_dir: d.map(|d| d as u16), wind_speed: s.map(|s| s as u16), ..Weather::default() };
        rest = weather_fields(&mut w, &rest[7..]);
        weather = Some(w);
    } else if p.format == Format::Uncompressed {
        rest = extensions(&mut p, rest);
    }
    let mut comment = Comment::plain(rest);
    if let Some(a) = altitude(&mut comment) {
        p.altitude = Some(a);
    }
    if p.format == Format::Uncompressed || p.format == Format::Compressed {
        dao(&mut p, hemi, &mut comment);
    }
    // The '/' or space that delimits extensions from the comment (a weather report's comment has none).
    let mut comment = if weather.is_none() { comment.strip_delimiter() } else { comment };
    if weather.is_some() {
        // A short code after the weather fields is the station's software and type (`wRSW`, `dU2k`), not a
        // comment.
        let rest: &[u8] = comment.raw();
        if (3..=5).contains(&rest.len()) && rest.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            comment = Comment::plain(&[]);
        }
    }
    (p, weather, comment.trim())
}

fn wx_val(v: &[u8]) -> Option<Option<u32>> {
    if v.iter().all(|&c| c == b'.' || c == b' ') {
        return Some(None);
    }
    digits(v).map(Some)
}

// Inlined: it would otherwise hold its own copy of the ~100-byte packet on the stack.
#[inline(always)]
fn position_report(b: &[u8], messaging: bool, timestamp: Option<Timestamp>) -> Result<Packet<'_>, Error> {
    let (p, rest, hemi) = position(b)?;
    let (pos, weather, comment) = position_tail(p, rest, hemi);
    Ok(Packet::Position { pos, messaging, timestamp, mic_e: None, weather, comment })
}

/// Mic-E (APRS 1.0.1 chapter 10): latitude, message bits and hemispheres in the destination address; longitude,
/// speed, course and symbol in the information field; then an optional altitude and the comment.
fn mic_e<'a>(dest: &[u8], b: &'a [u8]) -> Result<Packet<'a>, Error> {
    let d: &[u8; 6] = dest.get(..6).and_then(|d| d.try_into().ok()).ok_or(Error::MicE)?;
    let b8 = b.get(..8).ok_or(Error::Short)?;
    // Destination characters: latitude digit, message bit (standard or custom) and the flag bits.
    let mut lat_digits = [0u8; 6];
    let mut blanks = 0u8;
    let (mut bits, mut std, mut custom) = (0u8, false, false);
    let mut flags = [false; 6];
    for (i, &c) in d.iter().enumerate() {
        let (digit, flag, kind) = match c {
            b'0'..=b'9' => (Some(c - b'0'), false, 0),
            b'A'..=b'J' => (Some(c - b'A'), true, 2),
            b'K' => (None, true, 2),
            b'L' => (None, false, 0),
            b'P'..=b'Y' => (Some(c - b'P'), true, 1),
            b'Z' => (None, true, 1),
            _ => return Err(Error::MicE),
        };
        match digit {
            Some(v) if blanks == 0 => lat_digits[i] = v,
            Some(_) => return Err(Error::MicE), // a digit after a blank
            None if i >= 2 => blanks += 1,
            None => return Err(Error::MicE),
        }
        flags[i] = flag;
        if i < 3 {
            bits = bits << 1 | flag as u8;
            std |= kind == 1;
            custom |= kind == 2;
        } else if kind == 2 {
            return Err(Error::MicE); // A-K only carry message bits
        }
    }
    if std && custom {
        return Err(Error::MicE);
    }
    let deg = (lat_digits[0] * 10 + lat_digits[1]) as i32;
    let hmin = lat_digits[2..].iter().fold(0i32, |v, &x| v * 10 + x as i32);
    let half = [0, 5, 50, 500, 3000][blanks as usize];
    let hmin = if blanks >= 4 { 3000 } else { hmin + half };
    if deg > 90 || (deg == 90 && hmin > 0) || hmin >= 6000 {
        return Err(Error::MicE);
    }
    let lat = deg * 1_000_000 + (hmin * 1000 + 3) / 6;
    let lat = if flags[3] { lat } else { -lat };

    // Longitude: degrees, minutes and hundredths, each + 28.
    let mut lon_deg = b8[0] as i32 - 28;
    if flags[4] {
        lon_deg += 100;
    }
    if (180..=189).contains(&lon_deg) {
        lon_deg -= 80;
    } else if (190..=199).contains(&lon_deg) {
        lon_deg -= 190;
    }
    let mut lon_min = b8[1] as i32 - 28;
    if lon_min >= 60 {
        lon_min -= 60;
    }
    let lon_hund = b8[2] as i32 - 28;
    if !(0..=179).contains(&lon_deg) || !(0..60).contains(&lon_min) || !(0..100).contains(&lon_hund) {
        return Err(Error::MicE);
    }
    // Longitude is blanked as far as latitude is, the same way.
    let lon_hmin = lon_min * 100 + lon_hund;
    let lon_hmin = match blanks {
        0 => lon_hmin,
        1 => lon_hmin / 10 * 10 + 5,
        2 => lon_hmin / 100 * 100 + 50,
        3 => lon_hmin / 1000 * 1000 + 500,
        _ => 3000,
    };
    let lon = lon_deg * 1_000_000 + (lon_hmin * 1000 + 3) / 6;
    let lon = if flags[5] { -lon } else { lon };

    let (table, code) = (b8[7], b8[6]);
    if !valid_table(table) || !valid_code(code) {
        return Err(Error::Symbol);
    }
    let mut p = Position::new(lat, lon, table, code, Format::MicE);
    p.ambiguity = blanks;
    // Speed and course: SP DC SE, each + 28.
    let (sp, dc, se) = (b8[3] as i32 - 28, b8[4] as i32 - 28, b8[5] as i32 - 28);
    if sp < 0 || dc < 0 || se < 0 {
        return Err(Error::MicE);
    }
    let mut speed = sp * 10 + dc / 10;
    if speed >= 800 {
        speed -= 800;
    }
    let mut course = (dc % 10) * 100 + se;
    if course >= 400 {
        course -= 400;
    }
    p.speed = Some(speed as u16);
    if (1..=360).contains(&course) {
        p.course = Some(course as u16);
    }
    // Altitude: three base-91 characters and '}', metres above -10 km, at the start of the comment or after a
    // one-character radio type (`>`, `]`, `` ` ``, `'`).
    let rest = &b[8..];
    let mut comment = Comment::plain(rest);
    for skip in [0, 1] {
        if let Some(a) = rest.get(skip..skip + 4) {
            if a[3] == b'}' {
                if let Some(m) = base91(&a[..3]) {
                    // Up to 91^3 - 10000 m: feet in 64 bits.
                    let metres = m as i64 - 10_000;
                    p.altitude = Some(((metres * 10_000 + if metres < 0 { -1524 } else { 1524 }) / 3048) as i32);
                    // The radio type character stays in the comment.
                    comment.cut(skip, skip + 4);
                    break;
                }
            }
        }
    }
    Ok(Packet::Position {
        pos: p,
        messaging: true,
        timestamp: None,
        mic_e: Some(MicEStatus { bits, custom }),
        weather: None,
        comment: comment.trim(),
    })
}

/// `:ADDRESSEE:text{id` (the addressee padded to 9 characters).
fn message(b: &[u8]) -> Result<Packet<'_>, Error> {
    if b.len() < 10 || b[9] != b':' {
        return Err(Error::Message);
    }
    let mut addressee = &b[..9];
    while let Some((b' ', rest)) = addressee.split_last() {
        addressee = rest;
    }
    if addressee.is_empty() {
        return Err(Error::Message);
    }
    let body = &b[10..];
    for (prefix, kind) in [(&b"ack"[..], MessageKind::Ack), (&b"rej"[..], MessageKind::Rej)] {
        if let Some(tag) = body.strip_prefix(prefix) {
            // "ackMM", or with an APRS 1.1 reply-ack "ackMM}AA".
            let tag = trim_end(tag);
            let (id, reply) = match tag.iter().position(|&c| c == b'}') {
                Some(j) => (&tag[..j], Some(&tag[j + 1..]).filter(|r| !r.is_empty())),
                None => (tag, None),
            };
            let ok = |t: &[u8]| !t.is_empty() && t.len() <= 5 && t.iter().all(u8::is_ascii_alphanumeric);
            if ok(id) && reply.is_none_or(ok) {
                return Ok(Packet::Message(Message { addressee, kind, text: &[], id: Some(id), reply_ack: reply }));
            }
        }
    }
    // The message number: after the last '{', 1 to 5 characters, optionally "MM}AA" (reply-ack).
    let (text, id, reply_ack) = match body.iter().rposition(|&c| c == b'{') {
        Some(i) if (2..=8).contains(&(body.len() - i)) => {
            let tag = trim_end(&body[i + 1..]);
            match tag.iter().position(|&c| c == b'}') {
                Some(j) => (&body[..i], Some(&tag[..j]), Some(&tag[j + 1..]).filter(|r| !r.is_empty())),
                None if tag.len() <= 5 => (&body[..i], Some(tag), None),
                None => (body, None, None),
            }
        }
        _ => (body, None, None),
    };
    Ok(Packet::Message(Message { addressee, kind: MessageKind::Message, text, id, reply_ack }))
}

fn trim_end(mut b: &[u8]) -> &[u8] {
    while let Some((c, rest)) = b.split_last() {
        if matches!(c, b' ' | b'\r' | b'\n') {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// `;NAME     *DDHHMMzposition...` (name padded to 9; `*` live, `_` killed).
fn object(b: &[u8]) -> Result<Packet<'_>, Error> {
    if b.len() < 10 + 7 + 13 {
        return Err(Error::Short);
    }
    let live = match b[9] {
        b'*' => true,
        b'_' => false,
        _ => return Err(Error::Object),
    };
    let name = &b[..9];
    if !name.iter().all(|&c| (0x20..0x7F).contains(&c)) {
        return Err(Error::Object);
    }
    let (timestamp, rest) = timestamp(&b[10..])?;
    let (p, rest, hemi) = position(rest)?;
    let (pos, _, comment) = position_tail(p, rest, hemi);
    Ok(Packet::Object { name: trim_end(name), live, timestamp, pos, comment })
}

/// `)NAME!position...`: name 3 to 9 characters, then `!` live or `_` killed.
fn item(b: &[u8]) -> Result<Packet<'_>, Error> {
    let end = b.iter().take(10).position(|&c| c == b'!' || c == b'_').ok_or(Error::Object)?;
    if end < 3 {
        return Err(Error::Object);
    }
    let (p, rest, hemi) = position(&b[end + 1..])?;
    let (pos, _, comment) = position_tail(p, rest, hemi);
    Ok(Packet::Item { name: &b[..end], live: b[end] == b'!', pos, comment })
}

/// `>text`, or `>DDHHMMztext`.
fn status(b: &[u8]) -> Packet<'_> {
    if b.len() >= 7 && b[6] == b'z' {
        if let Ok((ts, rest)) = timestamp(b) {
            return Packet::Status { timestamp: Some(ts), text: rest };
        }
    }
    Packet::Status { timestamp: None, text: b }
}

/// `_MMDDHHMMc...s...g...t...`.
fn positionless_weather(b: &[u8]) -> Result<Packet<'_>, Error> {
    let t = b.get(..8).ok_or(Error::Short)?;
    let n = |i: usize| digits(&t[i..i + 2]).map(|v| v as u8).ok_or(Error::Timestamp);
    let timestamp = Some(Timestamp::MonthDayHourMin { month: n(0)?, day: n(2)?, hour: n(4)?, min: n(6)? });
    let mut weather = Weather::default();
    let rest = weather_fields(&mut weather, &b[8..]);
    Ok(Packet::Weather { timestamp, weather, comment: rest })
}

#[cfg(feature = "nmea")]
/// `$GPRMC`, `$GPGGA` and `$GPGLL` sentences (raw GPS output that some trackers send as they are).
fn nmea<'a>(dest: &[u8], b: &'a [u8]) -> Result<Packet<'a>, Error> {
    // Checksum: XOR of everything between '$' and '*'.
    let (body, sum) = match b.iter().position(|&c| c == b'*') {
        Some(i) => (&b[..i], b.get(i + 1..i + 3)),
        None => (trim_end(b), None),
    };
    if let Some(s) = sum {
        let want = hex(s).ok_or(Error::Nmea)? as u8;
        if body.iter().fold(0, |x, &c| x ^ c) != want {
            return Err(Error::Nmea);
        }
    }
    let mut f = [&b""[..]; 16];
    for (i, part) in body.split(|&c| c == b',').take(16).enumerate() {
        f[i] = part;
    }
    let (lat_i, alt_i) = match &f[0][2..] {
        b"RMC" => {
            if f[2] != b"A" {
                return Err(Error::Nmea);
            }
            (3, None)
        }
        b"GGA" => {
            if digits(f[6]).unwrap_or(0) == 0 {
                return Err(Error::Nmea);
            }
            (2, Some(9))
        }
        b"GLL" => {
            if f[6].first() == Some(&b'V') {
                return Err(Error::Nmea);
            }
            (1, None)
        }
        _ => return Ok(Packet::Other { kind: b'$', data: b }),
    };
    let lat = nmea_coord(f[lat_i], 2, f[lat_i + 1], b'N', b'S').ok_or(Error::Nmea)?;
    let lon = nmea_coord(f[lat_i + 2], 3, f[lat_i + 3], b'E', b'W').ok_or(Error::Nmea)?;
    let (table, code) = nmea_symbol(dest);
    let mut p = Position::new(lat, lon, table, code, Format::Nmea);
    if &f[0][2..] == b"RMC" {
        p.speed = decimal(f[7], 0).and_then(|v| u16::try_from(v).ok());
        // NMEA's true course: 0 is north, not unknown.
        p.course = decimal(f[8], 0).filter(|c| (0..=360).contains(c)).map(|c| if c == 0 { 360 } else { c as u16 });
    }
    if let Some(i) = alt_i {
        // Metres to feet.
        p.altitude = decimal(f[i], 1).map(|dm| ((dm as i64 * 10_000 + 1524) / 3048 / 10) as i32);
    }
    Ok(Packet::Position {
        pos: p,
        messaging: false,
        timestamp: None,
        mic_e: None,
        weather: None,
        comment: Comment::default(),
    })
}

#[cfg(feature = "nmea")]
/// A decimal number rounded to `places` decimals, as an integer (`12.34` with 1 place: 123).
fn decimal(t: &[u8], places: u32) -> Option<i32> {
    let (neg, t) = match t.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, t),
    };
    let (int, frac) = match t.iter().position(|&c| c == b'.') {
        Some(i) => (&t[..i], &t[i + 1..]),
        None => (t, &t[t.len()..]),
    };
    let mut v = digits(int)? as i64;
    for k in 0..=places as usize {
        let d = match frac.get(k) {
            Some(c) if c.is_ascii_digit() => (c - b'0') as i64,
            Some(_) => return None,
            None => 0,
        };
        if k < places as usize {
            v = v * 10 + d;
        } else if d >= 5 {
            v += 1; // round on the next digit
        }
    }
    let v = i32::try_from(v).ok()?;
    Some(if neg { -v } else { v })
}

#[cfg(feature = "nmea")]
/// `DDMM.mmmm` with the hemisphere letter in its own field.
fn nmea_coord(t: &[u8], width: usize, hemi: &[u8], pos: u8, neg: u8) -> Option<i32> {
    if t.len() < width + 2 {
        return None;
    }
    let deg = digits(&t[..width])? as i64;
    // Minutes to 1/10000.
    let rest = &t[width..];
    let (mi, fr) = match rest.iter().position(|&c| c == b'.') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &rest[rest.len()..]),
    };
    let mut min4 = digits(mi)? as i64 * 10_000;
    let mut scale = 1000;
    for &c in fr.iter().take(4) {
        if !c.is_ascii_digit() {
            return None;
        }
        min4 += (c - b'0') as i64 * scale;
        scale /= 10;
    }
    if min4 >= 600_000 || deg > if width == 2 { 89 } else { 179 } {
        return None;
    }
    let v = (deg * 1_000_000 + (min4 * 100 + 30) / 60) as i32;
    match hemi {
        [c] if *c == pos => Some(v),
        [c] if *c == neg => Some(-v),
        _ => None,
    }
}

#[cfg(feature = "nmea")]
/// NMEA packets carry no symbol of their own; APRS 1.0.1 (appendix 2) puts one in the destination: `GPS`,
/// `SPC` or `SYM`, then two characters for table and code, then an optional overlay; or `GPSCnn` / `GPSEnn`
/// (primary / alternate table, code number). The default is the primary table's dot, `/` `/`.
fn nmea_symbol(dest: &[u8]) -> (u8, u8) {
    dest_symbol(dest).unwrap_or((b'/', b'/'))
}

#[cfg(feature = "nmea")]
fn dest_symbol(dest: &[u8]) -> Option<(u8, u8)> {
    let (head, xy) = dest.split_at_checked(3)?;
    if !matches!(head, b"GPS" | b"SPC" | b"SYM") {
        return None;
    }
    if let (Some(&t @ (b'C' | b'E')), Some(n)) = (xy.first(), xy.get(1..3).and_then(digits)) {
        let table = if t == b'C' { b'/' } else { b'\\' };
        return (1..=94).contains(&n).then_some((table, n as u8 + 32));
    }
    let (x, y) = (*xy.first()?, *xy.get(1)?);
    // First letter: the group and the table (primary, alternate); second: the symbol within the group.
    let (primary, code) = match (x, y) {
        (b'B', b'B'..=b'P') => (true, y - b'B' + b'!'),
        (b'O', b'B'..=b'P') => (false, y - b'B' + b'!'),
        (b'P', b'0'..=b'9' | b'A'..=b'Z') => (true, y),
        (b'A', b'0'..=b'9' | b'A'..=b'Z') => (false, y),
        (b'M', b'R'..=b'X') => (true, y - b'R' + b':'),
        (b'N', b'R'..=b'X') => (false, y - b'R' + b':'),
        (b'H', b'S'..=b'X') => (true, y - b'S' + b'['),
        (b'D', b'S'..=b'X') => (false, y - b'S' + b'['),
        (b'L', b'A'..=b'Z') => (true, y - b'A' + b'a'),
        (b'S', b'A'..=b'Z') => (false, y - b'A' + b'a'),
        (b'J', b'1'..=b'4') => (true, y - b'1' + b'{'),
        (b'Q', b'1'..=b'4') => (false, y - b'1' + b'{'),
        _ => return None,
    };
    let table = match (primary, dest.get(5)) {
        (true, _) => b'/',
        (false, Some(&o)) if o.is_ascii_alphanumeric() => o,
        (false, _) => b'\\',
    };
    Some((table, code))
}

#[cfg(feature = "ultimeter")]
/// Peet Bros Ultimeter `$ULTW` weather: 4-hex-digit fields, `----` when absent: gust (0.1 km/h), direction
/// (0-255), temperature (0.1 °F), long-term rain, pressure (0.1 hPa), pressure change, two calibration words,
/// humidity (0.1 %), date, time, rain today (0.01 in), one-minute wind (0.1 km/h).
fn ultimeter(b: &[u8]) -> Result<Packet<'_>, Error> {
    let field = |i: usize| -> Option<Option<i32>> {
        let h = b.get(4 * i..4 * i + 4)?;
        if h == b"----" {
            return Some(None);
        }
        let v = hex(h)? as u16 as i16 as i32;
        Some(Some(v))
    };
    let f = |i: usize| field(i).ok_or(Error::Short);
    // 0.1 km/h to mph: x 0.0621371, rounded.
    let mph = |v: Option<i32>| v.and_then(|v| u16::try_from((v as i64 * 621_371 + 5_000_000) / 10_000_000).ok());
    let weather = Weather {
        wind_gust: mph(f(0)?),
        wind_dir: f(1)?.map(|d| ((d & 255) * 360 / 256) as u16),
        temp: f(2)?.map(|t| ((t + if t < 0 { -5 } else { 5 }) / 10) as i16),
        pressure: f(4)?.and_then(|p| u32::try_from(p).ok()),
        humidity: f(8)?.map(|h| ((h + 5) / 10).clamp(0, 100) as u8),
        rain_midnight: field(11).flatten().and_then(|r| u16::try_from(r).ok()),
        wind_speed: mph(field(12).flatten()),
        ..Weather::default()
    };
    Ok(Packet::Weather { timestamp: None, weather, comment: &[] })
}
