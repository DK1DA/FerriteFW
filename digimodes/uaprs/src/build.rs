//! Information fields to send: position reports, messages and acks. Each writes into a caller's buffer and
//! returns the length, or `None` if the buffer is too small or a field is out of range.

use crate::parse::Timestamp;

struct W<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl W<'_> {
    fn put(&mut self, b: &[u8]) -> Option<()> {
        self.out.get_mut(self.len..self.len + b.len())?.copy_from_slice(b);
        self.len += b.len();
        Some(())
    }

    /// `v` as `width` zero-padded digits.
    fn num(&mut self, v: u32, width: usize) -> Option<()> {
        let mut d = [b'0'; 10];
        let mut v = v;
        for i in (0..width).rev() {
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        (v == 0).then_some(())?;
        self.put(&d[..width])
    }
}

/// Where and what: the fields of a position report.
#[derive(Clone, Copy, Debug)]
pub struct Report<'a> {
    /// Millionths of a degree.
    pub lat: i32,
    pub lon: i32,
    pub table: u8,
    pub code: u8,
    /// Course (degrees, 1 to 360) and speed (knots).
    pub course_speed: Option<(u16, u16)>,
    /// Feet.
    pub altitude: Option<i32>,
    /// Can receive messages: `=` or `@` rather than `!` or `/`.
    pub messaging: bool,
    pub timestamp: Option<Timestamp>,
    pub comment: &'a [u8],
    /// Position ambiguity: 0 exact, 1-4 trailing digits of both latitude and longitude sent as spaces
    /// (hundredths, tenths, then the minutes' units and tens). Uncompressed reports only: [`compressed`] has no
    /// way to say it and ignores it.
    pub ambiguity: u8,
}

fn timestamp(w: &mut W, t: Timestamp) -> Option<()> {
    match t {
        Timestamp::DayHourMin { day, hour, min, utc } => {
            w.num(day as u32, 2)?;
            w.num(hour as u32, 2)?;
            w.num(min as u32, 2)?;
            w.put(if utc { b"z" } else { b"/" })
        }
        Timestamp::HourMinSec { hour, min, sec } => {
            w.num(hour as u32, 2)?;
            w.num(min as u32, 2)?;
            w.num(sec as u32, 2)?;
            w.put(b"h")
        }
        Timestamp::MonthDayHourMin { .. } => None,
    }
}

fn head(w: &mut W, r: &Report) -> Option<()> {
    if r.lat.abs() > 90_000_000 || r.lon.abs() > 180_000_000 {
        return None;
    }
    match (r.timestamp, r.messaging) {
        (None, false) => w.put(b"!"),
        (None, true) => w.put(b"="),
        (Some(t), m) => {
            w.put(if m { b"@" } else { b"/" })?;
            timestamp(w, t)
        }
    }
}

/// Degrees and hundredths of minutes, rounded: `DDMM.hh` or `DDDMM.hh`; the last `amb` digits (up to 4) blanked.
fn ddmm(w: &mut W, v: i32, width: usize, amb: u8) -> Option<()> {
    let v = v.unsigned_abs();
    let mut deg = v / 1_000_000;
    let mut hmin = ((v % 1_000_000) * 6 + 500) / 1000;
    if hmin == 6000 {
        deg += 1;
        hmin = 0;
    }
    w.num(deg, width)?;
    w.num(hmin / 100, 2)?;
    w.put(b".")?;
    w.num(hmin % 100, 2)?;
    // From the end: hundredths, tenths, then (past the '.') the minutes' units and tens.
    let end = w.len;
    for &k in &[1usize, 2, 4, 5][..(amb as usize).min(4)] {
        w.out[end - k] = b' ';
    }
    Some(())
}

fn tail(w: &mut W, r: &Report) -> Option<()> {
    if let Some(a) = r.altitude {
        w.put(b"/A=")?;
        if a < 0 {
            w.put(b"-")?;
            w.num(a.unsigned_abs(), 5)?;
        } else {
            w.num(a as u32, 6)?;
        }
    }
    // A comment that starts like data (course/speed, PHG, a '/' or space delimiter) gets a space in front, which
    // receivers strip, so it is not read as data. An `/A=` altitude or a `!DAO!` inside a comment is still read
    // by receivers as such.
    let mut probe = crate::parse::Position::new(0, 0, b'/', b'/', crate::parse::Format::Uncompressed);
    let delimiter = r.comment.first().is_some_and(|&c| c == b' ' || c == b'/');
    let extension = r.course_speed.is_none()
        && r.altitude.is_none()
        && crate::parse::extensions(&mut probe, r.comment).len() != r.comment.len();
    // After a course/speed or an altitude, a space keeps the comment readable in a raw packet ("/A=000242 On
    // the road"); the data fields are fixed width, so receivers read them the same either way (APRS-50).
    let after_data = (r.course_speed.is_some() || r.altitude.is_some()) && !r.comment.is_empty();
    if delimiter || extension || after_data {
        w.put(b" ")?;
    }
    w.put(r.comment)
}

/// An uncompressed report: `!DDMM.hhN/DDDMM.hhW>CSE/SPD/A=nnnnnn comment`. Position to about 18 m, or less
/// exact by [`Report::ambiguity`] (`4903.  N` for 2).
pub fn position(r: &Report, out: &mut [u8]) -> Option<usize> {
    let mut w = W { out, len: 0 };
    head(&mut w, r)?;
    ddmm(&mut w, r.lat, 2, r.ambiguity)?;
    w.put(if r.lat >= 0 { b"N" } else { b"S" })?;
    w.put(&[r.table])?;
    ddmm(&mut w, r.lon, 3, r.ambiguity)?;
    w.put(if r.lon >= 0 { b"E" } else { b"W" })?;
    w.put(&[r.code])?;
    if let Some((c, s)) = r.course_speed {
        w.num(c as u32, 3)?;
        w.put(b"/")?;
        w.num(s as u32, 3)?;
    }
    tail(&mut w, r)?;
    Some(w.len)
}

fn base91(w: &mut W, mut v: u32) -> Option<()> {
    let mut d = [0u8; 4];
    for i in (0..4).rev() {
        d[i] = (v % 91) as u8 + 33;
        v /= 91;
    }
    w.put(&d)
}

/// A compressed report: 13 characters for position, symbol, course and speed, then the comment. Position to
/// about 0.3 m; speed on a logarithmic scale (within 4% above 10 knots).
pub fn compressed(r: &Report, out: &mut [u8]) -> Option<usize> {
    let mut w = W { out, len: 0 };
    head(&mut w, r)?;
    let table = match r.table {
        d @ b'0'..=b'9' => d - b'0' + b'a',
        t => t,
    };
    w.put(&[table])?;
    // y = 380926 (90 - lat), x = 190463 (180 + lon), rounded.
    let y = ((90_000_000 - r.lat) as i64 * 380_926 + 500_000) / 1_000_000;
    let x = ((180_000_000 + r.lon) as i64 * 190_463 + 500_000) / 1_000_000;
    base91(&mut w, y.min(91i64.pow(4) - 1) as u32)?;
    base91(&mut w, x.min(91i64.pow(4) - 1) as u32)?;
    w.put(&[r.code])?;
    match r.course_speed {
        Some((c, s)) => {
            // c = course / 4; s = log(speed + 1) / log(1.08).
            // c = 0 is north (and also what "no course" looks like to a receiver); 90 would mean range.
            let c = (((c % 360) as u32 + 2) / 4) % 90;
            let mut k = 0u32;
            while k < 89 && crate::parse::pow108_q16(k + 1) <= (s as u64 + 1) << 16 {
                k += 1;
            }
            // Pick the nearer of k and k + 1.
            if k < 89 {
                let (lo, hi) = (crate::parse::pow108_q16(k), crate::parse::pow108_q16(k + 1));
                if ((s as u64 + 1) << 16) - lo > hi - ((s as u64 + 1) << 16) {
                    k += 1;
                }
            }
            w.put(&[c as u8 + 33, k as u8 + 33, b'C'])?;
        }
        // No course or speed: a space, then anything, then the type byte.
        None => w.put(b"  C")?,
    }
    tail(&mut w, r)?;
    Some(w.len)
}

/// A message: `:ADDRESSEE:text{id` (addressee up to 9 characters, text up to 67, id up to 5).
pub fn message(addressee: &[u8], text: &[u8], id: Option<&[u8]>, out: &mut [u8]) -> Option<usize> {
    if addressee.is_empty() || addressee.len() > 9 || text.len() > 67 || id.is_some_and(|i| i.is_empty() || i.len() > 5) {
        return None;
    }
    if text.iter().any(|&c| matches!(c, b'|' | b'~' | b'{')) {
        return None; // not allowed in message text
    }
    let mut w = W { out, len: 0 };
    w.put(b":")?;
    w.put(addressee)?;
    w.put(&b"         "[..9 - addressee.len()])?;
    w.put(b":")?;
    w.put(text)?;
    if let Some(i) = id {
        w.put(b"{")?;
        w.put(i)?;
    }
    Some(w.len)
}

/// Acknowledges message `id` from `addressee`: `:ADDRESSEE:ackid`.
pub fn ack(addressee: &[u8], id: &[u8], out: &mut [u8]) -> Option<usize> {
    reply(b"ack", addressee, id, out)
}

/// Rejects message `id` from `addressee`: `:ADDRESSEE:rejid`.
pub fn rej(addressee: &[u8], id: &[u8], out: &mut [u8]) -> Option<usize> {
    reply(b"rej", addressee, id, out)
}

fn reply(kind: &[u8], addressee: &[u8], id: &[u8], out: &mut [u8]) -> Option<usize> {
    if addressee.is_empty() || addressee.len() > 9 || id.is_empty() || id.len() > 5 {
        return None;
    }
    let mut w = W { out, len: 0 };
    w.put(b":")?;
    w.put(addressee)?;
    w.put(&b"         "[..9 - addressee.len()])?;
    w.put(b":")?;
    w.put(kind)?;
    w.put(id)?;
    Some(w.len)
}

/// Mic-E's standard messages (APRS 1.0.1 chapter 10): the message bits A, B, C, by name.
pub const MIC_E_OFF_DUTY: u8 = 0b111;
pub const MIC_E_EN_ROUTE: u8 = 0b110;
pub const MIC_E_IN_SERVICE: u8 = 0b101;
pub const MIC_E_RETURNING: u8 = 0b100;
pub const MIC_E_COMMITTED: u8 = 0b011;
pub const MIC_E_SPECIAL: u8 = 0b010;
pub const MIC_E_PRIORITY: u8 = 0b001;
pub const MIC_E_EMERGENCY: u8 = 0b000;

/// Degrees and hundredths of minutes, rounded, at most `max` degrees 59.99 minutes (179.99999 doesn't round to
/// 180, which Mic-E can't send), then the last `amb` digits dropped (floored).
fn deg_hmin(v: i32, max: u32, amb: u8) -> (u32, u32) {
    let v = v.unsigned_abs();
    let (mut deg, mut hmin) = (v / 1_000_000, ((v % 1_000_000) * 6 + 500) / 1000);
    if hmin == 6000 {
        (deg, hmin) = (deg + 1, 0);
    }
    if deg > max {
        (deg, hmin) = (max, 5999);
    }
    let unit = [1, 10, 100, 1000, 10_000][amb as usize];
    (deg, hmin / unit * unit)
}

/// A Mic-E report (APRS 1.0.1 chapter 10): the destination address's six characters to `dest` (latitude, the
/// message bits, north/south, the longitude offset and east/west) and the information field to `out` (returning
/// its length): `` ` `` (current GPS data), longitude, speed and course, symbol, then an altitude (`xxx}`, metres)
/// when there is one, then the comment. `status` is a standard message, [`MIC_E_OFF_DUTY`] to
/// [`MIC_E_EMERGENCY`]. [`Report::ambiguity`] blanks the latitude's trailing digits (the longitude's follow,
/// and are sent floored); speed is capped at 799 knots; a course of 360 is north, 0 unknown. The
/// timestamp and messaging flag aren't sent (Mic-E is always current and can take messages). A comment that a
/// receiver would read as a radio type (`` ` `` `'` `>` `]` first) or an altitude (`xxx}` at its start) gets a
/// space or two in front. Cross-checked against Daria Juniper's RustAPRS Mic-E encoder and decoder.
pub fn mic_e(r: &Report, status: u8, dest: &mut [u8; 6], out: &mut [u8]) -> Option<usize> {
    if status > 7 || r.lat.abs() > 90_000_000 || r.lon.abs() > 180_000_000 {
        return None;
    }
    let amb = r.ambiguity.min(4);
    let (lat_deg, lat_hmin) = deg_hmin(r.lat, 89, amb);
    let (lon_deg, lon_hmin) = deg_hmin(r.lon, 179, amb);
    let digits = [lat_deg / 10, lat_deg % 10, lat_hmin / 1000, lat_hmin / 100 % 10, lat_hmin / 10 % 10, lat_hmin % 10];
    // Each character: a digit (0-9 for a 0 bit, P-Y for a 1), or for a blanked digit L (0) or Z (1).
    let flags = (status as u32) << 3 | ((r.lat >= 0) as u32) << 2 | (!(10..100).contains(&lon_deg) as u32) << 1 | (r.lon < 0) as u32;
    for (i, d) in dest.iter_mut().enumerate() {
        let one = flags >> (5 - i) & 1 != 0;
        *d = match (i >= 6 - amb as usize, one) {
            (false, false) => b'0' + digits[i] as u8,
            (false, true) => b'P' + digits[i] as u8,
            (true, false) => b'L',
            (true, true) => b'Z',
        };
    }
    let mut w = W { out, len: 0 };
    let lon_d = match lon_deg {
        0..=9 => lon_deg + 118,
        10..=99 => lon_deg + 28,
        100..=109 => lon_deg + 8,
        _ => lon_deg - 72,
    };
    let min = lon_hmin / 100;
    let lon_m = if min < 10 { min + 88 } else { min + 28 };
    let (course, speed) = r.course_speed.map_or((0, 0), |(c, s)| (if c > 360 { (c as u32 - 1) % 360 + 1 } else { c as u32 }, (s as u32).min(799)));
    w.put(&[b'`', lon_d as u8, lon_m as u8, (lon_hmin % 100 + 28) as u8])?;
    w.put(&[(speed / 10 + 28) as u8, (speed % 10 * 10 + course / 100 + 28) as u8, (course % 100 + 28) as u8])?;
    w.put(&[r.code, r.table])?;
    if let Some(ft) = r.altitude {
        let m = (ft as i64 * 3048 + if ft < 0 { -5000 } else { 5000 }) / 10_000;
        let v = (m + 10_000).clamp(0, 91 * 91 * 91 - 1) as u32;
        w.put(&[(v / 8281) as u8 + 33, (v / 91 % 91) as u8 + 33, (v % 91) as u8 + 33, b'}'])?;
    } else {
        let alt = |s: &[u8]| s.get(3) == Some(&b'}') && s[..3].iter().all(|c| (33..=123).contains(c));
        let c = r.comment;
        if alt(c) {
            w.put(b"  ")?;
        } else if alt(c.get(1..).unwrap_or(&[])) || c.first().is_some_and(|f| b"`'>]".contains(f)) {
            w.put(b" ")?;
        }
    }
    w.put(r.comment)?;
    Some(w.len)
}
